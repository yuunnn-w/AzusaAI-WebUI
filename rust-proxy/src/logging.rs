//! **纯内存**环形日志 + 凭据脱敏（方案 §3.3 / §8-P1-B；P6-S5 落地）。
//!
//! D12（全程零文件 I/O）：日志**只有一种 sink = 内存环形缓冲** —— 不 `create_dir_all`、不 `open`、
//! 不 `write`、不 `flush`；进程退出即消失。需要外部留存时由用户在界面里「查看日志」→「复制到剪贴板」
//! （剪贴板是设备，不是文件，属 D12 豁免清单第 ③ 条）。
//!
//! 并发与锁粒度（D12 边界第 3 条）：
//! - 全对象**只剩一把** `Mutex`（`ring`）；级别判定与 `format!` 都在**锁外**完成，
//!   持锁期间只做 `push_back` + 必要时 `pop_front`（**O(1)**，无 I/O、无 `format!`、无回调 UI）。
//! - 级别过滤发生在**入环之前** ⇒ `--log-level error` 时 `info` 级访问行根本不进环（§8-P1-A 的"热路径不白付"）。
//! - 环满丢**最旧**一条并**计数**（`ring_dropped`）⇒ "被挤掉"是可见的，不是静默丢弃。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Mutex;

use chrono::Local;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
}

impl Level {
    pub fn parse(text: &str) -> Option<Level> {
        match text.to_ascii_lowercase().as_str() {
            "error" => Some(Level::Error),
            "warn" => Some(Level::Warn),
            "info" => Some(Level::Info),
            "debug" => Some(Level::Debug),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Level::Error => "ERROR",
            Level::Warn => "WARN",
            Level::Info => "INFO",
            Level::Debug => "DEBUG",
        }
    }

    fn as_u8(self) -> u8 {
        match self {
            Level::Error => 0,
            Level::Warn => 1,
            Level::Info => 2,
            Level::Debug => 3,
        }
    }
}

/// 环形缓冲容量：**2,000 条**（方案 §3.3 / D11-b）。**硬上限**，不按需长大。
pub const LOG_RING_CAPACITY: usize = 2000;
/// 单行字节上限（超出截断并附「…(截断)」）⇒ 最坏占用 ≈ 2000 × 4096 = 8 MB，典型 ≈ 0.4 MB。
pub const LOG_LINE_MAX_BYTES: usize = 4096;
/// 截断标记（**字节安全**：截断点落在 UTF-8 字符边界上）。
const TRUNCATED: &str = "…(截断)";

/// 日志器。**长生命周期共享对象**（`Arc<Logger>`）：热改配置只更新级别（`apply`），不重建对象
/// —— 这样 UI 与服务始终指向同一份日志。
pub struct Logger {
    level: AtomicU8,
    /// 唯一存储（D12：没有第二处 sink）。
    ring: Mutex<VecDeque<String>>,
    /// 被环挤掉的条数（面板头行如实显示）。
    ring_dropped: AtomicU64,
    /// **单调总写入条数**（P7-I2）：`push()` 每入环一次 +1，环满丢最旧也照加 ⇒
    /// 恒等式 `written == retained + dropped`（单测钉住）。文档窗的日志模式靠它算"新增了多少行"
    /// （`written - seen_written`）—— **只取尾部新增**，不重建全量文本（C-2 的机械判据）。
    ///
    /// ⚠ **绕环诊断不计入本计数**（D6）：每秒的 `日志窗口刷新 N 行` 走 `crate::output::log_line`
    /// 直写 stdout，**不经 [`Logger::log`]** ⇒ 否则 debug 档下诊断行自身入环 ⇒ 下一秒又有新增
    /// ⇒ 永续自增（J-C4 的"静置 30 s = 0 条"必红）。
    written: AtomicU64,
}

impl Logger {
    pub fn new(level: Level) -> Logger {
        Logger {
            level: AtomicU8::new(level.as_u8()),
            ring: Mutex::new(VecDeque::with_capacity(LOG_RING_CAPACITY)),
            ring_dropped: AtomicU64::new(0),
            written: AtomicU64::new(0),
        }
    }

    /// FR-37 热生效：只更新级别（D12 后没有目录/份数可改）。
    pub fn apply(&self, level: Level) {
        self.level.store(level.as_u8(), Ordering::Relaxed);
    }

    pub fn enabled(&self, level: Level) -> bool {
        level.as_u8() <= self.level.load(Ordering::Relaxed)
    }

    pub fn error(&self, message: &str) {
        self.log(Level::Error, message);
    }

    pub fn warn(&self, message: &str) {
        self.log(Level::Warn, message);
    }

    pub fn info(&self, message: &str) {
        self.log(Level::Info, message);
    }

    pub fn debug(&self, message: &str) {
        self.log(Level::Debug, message);
    }

    /// 记一行：**级别过滤 → 锁外格式化/截断 → 通道输出 → 持锁 O(1) 入环**。
    pub fn log(&self, level: Level, message: &str) {
        if !self.enabled(level) {
            return;
        }
        let line = format!(
            "[{}] {:5} {}",
            Local::now().format("%Y-%m-%d %H:%M:%S"),
            level.label(),
            message
        );
        let line = truncate_line(line);
        crate::output::log_line(&line);
        self.push(line);
    }

    /// 持锁段：只做 `push_back`（满则先 `pop_front` 并计数）—— **O(1)**，无 I/O。
    fn push(&self, line: String) {
        let mut ring = match self.ring.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if ring.len() >= LOG_RING_CAPACITY {
            ring.pop_front();
            self.ring_dropped.fetch_add(1, Ordering::Relaxed);
        }
        ring.push_back(line);
        // 计数在**持锁段内**自增（与 `ring_dropped` 同临界区）⇒ 读者拿到的 `written` 与
        // `retained + dropped` 不会出现"已计数但还没入环"的瞬时错配。
        self.written.fetch_add(1, Ordering::Relaxed);
    }

    /// 只读快照：**最后 `max` 行**（旧 → 新）+ 被挤出条数（方案 §3.3）。
    /// 面板/剪贴板唯一读出口；持锁期间只做一次 `cloned()`，不做格式化、不做 I/O。
    pub fn snapshot_lines(&self, max: usize) -> (Vec<String>, u64) {
        let ring = match self.ring.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let take = ring.len().min(max);
        let lines: Vec<String> = ring.iter().skip(ring.len() - take).cloned().collect();
        let dropped = self.ring_dropped.load(Ordering::Relaxed);
        (lines, dropped)
    }

    /// 当前环内条数（面板头行「共 N 条」用；上限恒为 [`LOG_RING_CAPACITY`]）。
    pub fn retained_count(&self) -> usize {
        match self.ring.lock() {
            Ok(guard) => guard.len(),
            Err(poisoned) => poisoned.into_inner().len(),
        }
    }

    /// 被环挤掉的条数（面板/设置窗头行「已挤出 M 条」用）。
    pub fn dropped_count(&self) -> u64 {
        self.ring_dropped.load(Ordering::Relaxed)
    }

    /// **单调总写入条数**（文档窗日志模式算"尾部新增了几行"的唯一读出口）。
    ///
    /// 恒等式 `written_count() == retained_count() + dropped_count()` 在**持锁段内**成立
    /// （`push()` 的两处计数在同一临界区自增；单测 `written_equals_retained_plus_dropped` 钉住）。
    pub fn written_count(&self) -> u64 {
        self.written.load(Ordering::Relaxed)
    }
}

/// 单行封顶（**字节安全**）：超 `LOG_LINE_MAX_BYTES` 时截到字符边界 + 附截断标记。
fn truncate_line(line: String) -> String {
    if line.len() <= LOG_LINE_MAX_BYTES {
        return line;
    }
    let mut cut = LOG_LINE_MAX_BYTES - TRUNCATED.len();
    while cut > 0 && !line.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut truncated = line;
    truncated.truncate(cut);
    truncated.push_str(TRUNCATED);
    truncated
}

pub const REDACTED: &str = "***";

/// **凭据族头名（唯一名单）** —— [`redact_header`] 与 [`render_header_for_log`] 共用同一份，
/// 不存在"两处名单各写一遍"的漂移面（单测 `credential_list_matches_redact_header` 钉住）。
///
/// 12 个模式（B2 扩展后的现取名单）：`authorization` / `proxy-authorization` / `cookie` /
/// `set-cookie` / `x-api-key` / `api-key` / `api_key` / `api_token` / `api-token` /
/// `x-auth-token` / `x-access-token` / `x-goog-api-key`。
pub const CREDENTIAL_HEADERS: [&str; 12] = [
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
    "x-api-key",
    "api-key",
    "api_key",
    "api_token",
    "api-token",
    "x-auth-token",
    "x-access-token",
    "x-goog-api-key",
];

/// debug 档**打全值**的头（排障白名单；§8-P1-B 的结构保险）。
///
/// 收录口径 = "排障必需 **且** 一定不含凭据"：协商/内容类型、上游协议版本、来源与请求标识、
/// 常见 hop-by-hop 与服务标识头。**不含**任何凭据族、也不含会话标识（如 `mcp-session-id`
/// 一类"标识即凭据"的头 —— 它们落到第三档，只打字节数）。
pub const LOGGABLE_HEADERS: [&str; 17] = [
    "accept",
    "accept-encoding",
    "accept-language",
    "anthropic-beta",
    "anthropic-version",
    "cache-control",
    "content-encoding",
    "content-length",
    "content-type",
    "date",
    "host",
    "mcp-protocol-version",
    "origin",
    "referer",
    "server",
    "user-agent",
    "x-request-id",
];

fn is_credential_header(lower_name: &str) -> bool {
    CREDENTIAL_HEADERS.contains(&lower_name)
}

fn is_loggable_header(lower_name: &str) -> bool {
    LOGGABLE_HEADERS.contains(&lower_name)
}

/// **debug 档头打印的唯一渲染入口**（§8-P1-B 的结构保险 / S9）。
///
/// 三分支（**结构上不可能把未列名的头原样透传**）：
/// ① 凭据族（[`CREDENTIAL_HEADERS`]）⇒ [`redact_header`]（`Authorization` 保留 scheme）；
/// ② 排障白名单（[`LOGGABLE_HEADERS`]）⇒ 打全值；
/// ③ **其余一律** ⇒ 只打**头名 + 值字节数**，值一个字节都不出。
///
/// 这条保险的意义：`redact_header` 本身是**名单式**的（名单外的新凭据头会被原样返回 ——
/// 见其文档与单测），而打印路径走本函数 ⇒ 即使名单漏了某个头，日志里也**不会**出现它的值。
/// 代价 = debug 档对名单外头的可读性（如实登记在 §8-P1-B 的风险项里）。
pub fn render_header_for_log(name: &str, value: &str, value_bytes: usize) -> String {
    let lower = name.to_ascii_lowercase();
    if is_credential_header(&lower) {
        return redact_header(name, value);
    }
    if is_loggable_header(&lower) {
        return value.to_string();
    }
    format!("<{value_bytes} 字节值已隐藏>")
}

/// FR-25 / D11-b：日志内的**凭据头**一律脱敏（内存日志与剪贴板导出走同一条函数）。
///
/// - `Authorization` / `Proxy-Authorization`：保留 scheme（`Bearer ***`），无 scheme ⇒ `***`；
/// - `Cookie` / `Set-Cookie` / `*-api-key` / `*-auth-token` / `*-access-token` / `api_token`
///   等凭据族（[`CREDENTIAL_HEADERS`]）：**整值替换**（§8-P1-B 名单扩展）。
///
/// ⚠ **名单式**脱敏的边界（如实登记）：**未列名**的新凭据头会被本函数原样返回 ——
/// 单测 `unlisted_header_is_never_printed_by_the_log_path` 同时钉住两件事：
/// ① 本函数对未列名头确实返回原值（判据不是永真的证明面）；
/// ② **打印路径不看本函数** —— [`render_header_for_log`] 对未列名头只出字节数 ⇒ 结构保险在。
pub fn redact_header(name: &str, value: &str) -> String {
    let lower = name.to_ascii_lowercase();
    if !is_credential_header(&lower) {
        return value.to_string();
    }
    if lower == "authorization" || lower == "proxy-authorization" {
        return match value.split_once(' ') {
            Some((scheme, _)) if !scheme.is_empty() => format!("{scheme} {REDACTED}"),
            _ => REDACTED.to_string(),
        };
    }
    REDACTED.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn logger_with(level: Level) -> Logger {
        Logger::new(level)
    }

    /// U6：脱敏（方案 §10.1）+ P1-B 的 Cookie 族（S5 扩展）。
    #[test]
    fn u6_redacts_credentials() {
        assert_eq!(
            redact_header("Authorization", "Bearer sk-123"),
            "Bearer ***"
        );
        assert_eq!(
            redact_header("authorization", "Bearer sk-123"),
            "Bearer ***"
        );
        assert_eq!(redact_header("Authorization", "sk-123"), "***");
        assert_eq!(redact_header("X-Api-Key", "sk-123"), "***");
        assert_eq!(redact_header("api_key", "sk-123"), "***");
        assert_eq!(
            redact_header("Content-Type", "application/json"),
            "application/json"
        );
    }

    /// P1-B：Cookie / Set-Cookie / token 族必须整值脱敏（D11-b 明文点名）。
    #[test]
    fn p1b_cookie_family_is_redacted() {
        for name in [
            "Cookie",
            "cookie",
            "Set-Cookie",
            "set-cookie",
            "x-auth-token",
            "X-Access-Token",
            "api_token",
            "api-token",
        ] {
            let redacted = redact_header(name, "session=sk-raw-secret-001");
            assert_eq!(
                redacted, REDACTED,
                "{name} 的凭据必须整值脱敏（P1-B / D11-b）"
            );
            assert!(
                !redacted.contains("sk-raw-secret-001"),
                "{name} 不得泄漏原值"
            );
        }
        // 大小写不敏感（HTTP 头名大小写无关）
        assert_eq!(redact_header("CoOkIe", "a=b"), REDACTED);
    }

    /// **名单与实现同源**：`CREDENTIAL_HEADERS` 里每一个头名都必须被 `redact_header` 脱敏
    /// —— 防止"const 名单加了、match 忘了改"的静默漂移（S9 把 match 改成读 const 后仍钉一遍）。
    #[test]
    fn credential_list_matches_redact_header() {
        assert_eq!(CREDENTIAL_HEADERS.len(), 12, "B2 扩展后的名单 = 12 个模式");
        for name in CREDENTIAL_HEADERS {
            let redacted = redact_header(name, "session=sk-raw-secret-001");
            assert_ne!(
                redacted, "session=sk-raw-secret-001",
                "{name} 在名单里但未被脱敏 ⇒ 名单与实现漂移"
            );
            assert!(
                !redacted.contains("sk-raw-secret-001"),
                "{name} 不得泄漏原值：{redacted}"
            );
        }
    }

    /// **[E13] 结构保险的两面**（S9 / P1-B）：
    /// ① `redact_header` 是**名单式**的 —— 未列名的新凭据头它照样原样返回（"明文 = 0"不是永真命题，
    ///    这就是那条判据的证明面）；
    /// ② 但**打印路径不看它**：`render_header_for_log` 对未列名头只出头名与字节数 ⇒
    ///    即使名单漏了某个头，日志里也一个字节都不出（结构保险在）。
    #[test]
    fn unlisted_header_is_never_printed_by_the_log_path() {
        let raw = "sk-raw-secret-001";
        assert_eq!(
            redact_header("x-unlisted-token", raw),
            raw,
            "名单外的头在纯函数面仍是原值 ⇒ 光靠名单不可能保证『明文 = 0』"
        );

        let rendered = render_header_for_log("x-unlisted-token", raw, raw.len());
        assert_eq!(rendered, format!("<{} 字节值已隐藏>", raw.len()));
        assert!(
            !rendered.contains(raw),
            "结构保险必须挡住未列名头的值：{rendered}"
        );
        // 大小写不敏感 + 名单内头仍打全值（排障可读性不受影响）
        assert!(!render_header_for_log("X-Unlisted-Token", raw, raw.len()).contains(raw));
        assert_eq!(
            render_header_for_log("Content-Type", "application/json", 16),
            "application/json"
        );
        assert_eq!(
            render_header_for_log("User-Agent", "curl/8.0", 8),
            "curl/8.0"
        );
        assert_eq!(render_header_for_log("Cookie", "session=abc", 11), REDACTED);
        assert_eq!(
            render_header_for_log("Authorization", "Bearer sk-1", 12),
            "Bearer ***"
        );
    }

    #[test]
    fn level_parses_and_orders() {
        assert!(Level::parse("debug").is_some());
        assert!(Level::parse("DEBUG").is_some());
        assert!(Level::parse("trace").is_none());
        assert!(Level::Error < Level::Warn);
        assert!(Level::Warn < Level::Info);
        assert!(Level::Info < Level::Debug);
    }

    /// 口径常量：容量 2,000 / 单行 4,096（方案 §3.3；改这两个数必须同步方案与 README）。
    #[test]
    fn capacity_and_line_cap_are_pinned() {
        assert_eq!(LOG_RING_CAPACITY, 2000);
        assert_eq!(LOG_LINE_MAX_BYTES, 4096);
        let logger = logger_with(Level::Info);
        assert_eq!(logger.retained_count(), 0);
        assert_eq!(logger.snapshot_lines(10).0.len(), 0);
    }

    /// 环形上限 + 挤出计数：超过容量的部分丢**最旧**，`ring_dropped` 递增。
    #[test]
    fn ring_respects_capacity_and_counts_dropped() {
        let logger = logger_with(Level::Info);
        let total = LOG_RING_CAPACITY + 5;
        for index in 0..total {
            logger.info(&format!("line-{index}"));
        }
        assert_eq!(logger.retained_count(), LOG_RING_CAPACITY, "上限必须硬");
        let (lines, dropped) = logger.snapshot_lines(usize::MAX);
        assert_eq!(lines.len(), LOG_RING_CAPACITY);
        assert_eq!(dropped, 5, "被挤掉的条数必须可见地计数");
        assert!(
            lines[0].ends_with("line-5"),
            "丢的必须是最旧的（首行应为第 6 条）：{}",
            lines[0]
        );
        assert!(
            lines[lines.len() - 1].ends_with(&format!("line-{}", total - 1)),
            "最后一行必须是最新的"
        );
        // 多次快照读数一致（只读，不消费环）
        assert_eq!(logger.snapshot_lines(10).1, 5);
        assert_eq!(logger.retained_count(), LOG_RING_CAPACITY);
    }

    /// **恒等式**（P7-I2 新增计数）：`written == retained + dropped`；单调不减；级别过滤不计入。
    #[test]
    fn written_equals_retained_plus_dropped() {
        let logger = logger_with(Level::Info);
        assert_eq!(logger.written_count(), 0, "新 logger 的写入计数必须为 0");
        // 未达容量：written == retained，dropped == 0
        for index in 0..10 {
            logger.info(&format!("line-{index}"));
        }
        assert_eq!(logger.written_count(), 10);
        assert_eq!(
            logger.written_count(),
            logger.retained_count() as u64 + logger.dropped_count()
        );
        // 超容量：written 继续单调增，恒等式仍成立
        for index in 0..(LOG_RING_CAPACITY + 5) {
            logger.info(&format!("overflow-{index}"));
        }
        // 总写入 = 10 + 2005 = 2015；环内 2000 ⇒ 被挤掉的是**多出来的 15 条**（前 10 条 + 5 条）
        assert_eq!(logger.written_count(), 10 + LOG_RING_CAPACITY as u64 + 5);
        assert_eq!(logger.dropped_count(), 15);
        assert_eq!(logger.retained_count(), LOG_RING_CAPACITY);
        assert_eq!(
            logger.written_count(),
            logger.retained_count() as u64 + logger.dropped_count(),
            "恒等式 written == retained + dropped 必须成立"
        );
        // 级别过滤发生在入环之前 ⇒ 被过滤的行**不进环也不计数**（否则日志窗会看到"幽灵新增"）
        logger.apply(Level::Warn);
        logger.debug("filtered-out");
        assert_eq!(logger.written_count(), 10 + LOG_RING_CAPACITY as u64 + 5);
    }

    /// `snapshot_lines` 条数语义：只取**最后** `max` 行，且 ≤ 环内实际条数。
    #[test]
    fn snapshot_lines_takes_the_tail_only() {
        let logger = logger_with(Level::Debug);
        for index in 0..10 {
            logger.info(&format!("m-{index}"));
        }
        let (lines, dropped) = logger.snapshot_lines(3);
        assert_eq!(lines.len(), 3);
        assert_eq!(dropped, 0);
        assert!(lines[0].ends_with("m-7"), "{}", lines[0]);
        assert!(lines[2].ends_with("m-9"), "{}", lines[2]);
        let (all, _) = logger.snapshot_lines(usize::MAX);
        assert_eq!(all.len(), 10, "max 超过环内条数 ⇒ 返回全部");
    }

    /// 超长行截断：**字节**上限 + 截断标记 + 多字节字符不被切开。
    #[test]
    fn long_line_is_truncated_on_char_boundary() {
        let logger = logger_with(Level::Info);
        logger.info(&"a".repeat(LOG_LINE_MAX_BYTES * 2));
        // 多字节：每个「中」3 字节 ⇒ 9000 B 会被截到字符边界
        logger.info(&"中".repeat(LOG_LINE_MAX_BYTES));

        let (lines, _) = logger.snapshot_lines(usize::MAX);
        assert_eq!(lines.len(), 2);
        for line in &lines {
            assert!(
                line.len() <= LOG_LINE_MAX_BYTES,
                "单行不得超上限（当前 {} B）",
                line.len()
            );
            assert!(line.ends_with(TRUNCATED), "截断必须带可见标记：{line}");
        }
        // 多字节那行：截断后仍是合法 UTF-8（`String` 本身保证），且长度落在边界内
        assert!(lines[1].is_char_boundary(lines[1].len()));
        // 未超限的行不得被截断
        logger.info("short line");
        let (lines, _) = logger.snapshot_lines(1);
        assert!(lines[0].ends_with("short line"), "{}", lines[0]);
    }

    /// 级别过滤在**入环前**：早退的行不进环、也不占用容量。
    #[test]
    fn level_filter_runs_before_the_ring() {
        let logger = logger_with(Level::Error);
        logger.debug("d");
        logger.info("i");
        logger.warn("w");
        assert_eq!(logger.retained_count(), 0, "低于级别的行不得进环");
        logger.error("boom");
        assert_eq!(logger.retained_count(), 1);
        let (lines, _) = logger.snapshot_lines(10);
        assert!(lines[0].contains("ERROR"));
        assert!(lines[0].ends_with("boom"));
    }

    /// Phase 2：`apply` 热改级别（FR-37；D12 后只剩级别一个可变位）。
    #[test]
    fn apply_updates_level() {
        let logger = logger_with(Level::Info);
        assert!(logger.enabled(Level::Info));
        assert!(!logger.enabled(Level::Debug));
        logger.apply(Level::Debug);
        assert!(logger.enabled(Level::Debug));
        logger.apply(Level::Error);
        assert!(logger.enabled(Level::Error));
        assert!(!logger.enabled(Level::Info));
    }
}
