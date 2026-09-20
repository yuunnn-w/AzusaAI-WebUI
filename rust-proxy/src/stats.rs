#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
//! 指标（方案 §6.9 FR-29～FR-31）：累计计数 / 活跃数 / 字节进出 / 延迟分位 / 最近错误环形缓冲。
//! 另有 **D1 的近窗时间序列真源**（[`Window`]：120 桶 × 1 s）—— 面板「（近 120 s）」读数与波动曲线的
//! **唯一**数据出口（UI 不得自建采样环、不得重算"哪些秒有效"）。
//!
//! ⚠ 本文件在「请求路径零 panic」硬规格作用域内（方案 §6.6 / §8.6 1a）：
//! 禁止 `unwrap()` / `expect()` / `panic!()` / 显式索引取值 —— Win7 配方强制
//! `panic = "abort"`（§5.4），任何 panic = 进程立即退出。
//! 落地手法（本文件）：`Mutex` 一律用 poison-tolerant 模式；数组/切片取值一律 `get()`；
//! 直方图求和/取分位用 `iter().enumerate()`（不索引）。
//!
//! **长生命周期**：Phase 2 起 `Stats` 由 `Arc` 共享（服务启停热切换不丢累计数与最近错误，
//! 见 `src/service.rs`）⇒ 只允许 `&self` 方法（原子/内部可变）。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

/// 不 derive `Default`：`Window` 不是 `Default` 能落地的字段（`Instant` 无 `Default`、
/// `[WinBucket; 120]` 超数组 `Default` 的 32 上限 ⇒ §4.1 的 P2-6）⇒ 全字段在 `Stats::new()` 里显式构造。
pub struct Stats {
    total: AtomicU64,
    preflight: AtomicU64,
    forwarded: AtomicU64,
    upstream_errors: AtomicU64,
    upstream_timeouts: AtomicU64,
    stream_errors: AtomicU64,
    client_aborted: AtomicU64,
    /// FR-30：上游 4xx / 5xx（**上游真实返回**的状态码，不含代理自己合成的 502/504 ——
    /// 那两个走 `upstream_errors` / `upstream_timeouts`）。
    upstream_4xx: AtomicU64,
    upstream_5xx: AtomicU64,
    /// **全局累计字节计数**（面板 `↑B/↓B` 的唯一口径）。曲线侧的"同源"由 [`Stats::add_bytes_in`] /
    /// [`Stats::add_bytes_out`] 在**同一处**顺带写近窗每秒桶保证（单一调用点 ⇒ 不会各说各话）。
    ///
    /// ⚠ **P8-UI4 的显式口径变更（如实登记）**：D1/D7 曾明令"**不得新增每 chunk 原子写**"，所以曲线
    /// 当时只能读"每秒首样本的**边界快照**"——而边界快照只由**请求级事件**触发 ⇒ 一条 20 s 的流式响应
    /// 期间一个桶都不产生、曲线**整条不出现**（真机实测：实传 136 KB、面板角标仍 `0 s / 120 s`）。
    /// 本批为修根因，**每 chunk 新增 1 次 `Relaxed` load + 1~2 次 `fetch_add`**（[`Window::record_bytes`]）
    /// —— 代价读数与理由见 `shared/progress/rust-proxy-p8-ui4-done.md`；**接线点仍只有这一处**
    /// （`forward.rs` 每 chunk 一次）。
    bytes: std::sync::Arc<ByteTotals>,
    /// 本修复批（L2/L4）：连接池重置次数（"失败即弃池"的机制计数，**不是**错误 ⇒ 不进 `total_errors`）。
    pool_flushes: AtomicU64,
    /// 本修复批（L4）：安全重试次数（含成功与失败的重试；只对 GET/HEAD/OPTIONS ∧ 无体 ∧ 未发字节）。
    retries: AtomicU64,
    active: AtomicI64,
    peak_active: AtomicI64,
    /// FR-30「上游延迟 p50/p95」：请求 → 上游响应头到达（TTFB）的直方图（**自启动累计**口径，保留）。
    latency_buckets: Vec<AtomicU64>,
    /// D1：近窗时间序列真源（120 桶 × 1 s）。**与累计计数器并存**：累计仍是 `snapshot()` 的唯一口径。
    window: Window,
    /// FR-31：最近错误（环形，容量 `RECENT_ERRORS_CAPACITY`；同一条连续出现时合并计数）。
    errors: Mutex<VecDeque<ErrorEntry>>,
}

/// FR-30：延迟直方图上界（毫秒）。桶 `i` 覆盖 `(upper[i-1], upper[i]]`；
/// 超过最后一个上界的样本进末桶（> 10 s 归入"≥10 s"档）。12 桶 = 够看，又不至于让
/// 分位读数抖动（每 1 s 刷新的 UI 上"38ms / 210ms"这种量级就够用）。
const LATENCY_BUCKET_UPPER_MS: [u64; 12] =
    [5, 10, 25, 50, 100, 250, 500, 1000, 2500, 5000, 10000, 30000];

/// FR-31：最近错误保留条数（方案写死 10）。
pub const RECENT_ERRORS_CAPACITY: usize = 10;

/// 末桶（`> 10 s`）的显示口径（FR-30 数字诚实性；审查 P2-7）：**不**谎报桶上界 30000ms。
pub const LATENCY_OVERFLOW_LABEL: &str = "≥10s";

/// 一条错误摘要（时间 + 单行 + 连续次数）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErrorEntry {
    pub time: String,
    pub line: String,
    pub count: u32,
}

/// 计数器快照（访问日志行 + Phase 2 的 UI 指标卡共用）。
pub struct Snapshot {
    pub total: u64,
    pub preflight: u64,
    pub forwarded: u64,
    pub upstream_errors: u64,
    pub upstream_timeouts: u64,
    pub stream_errors: u64,
    pub client_aborted: u64,
    pub upstream_4xx: u64,
    pub upstream_5xx: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    /// L2：连接池重置次数（`池重置=` 读数；判据 I15/I16/I19/I20 读它）。
    pub pool_flushes: u64,
    /// L4：安全重试次数。
    pub retries: u64,
    pub active: i64,
    pub peak_active: i64,
}

impl Stats {
    /// 构造面 = **显式**全字段（含 `window: Window::new()`）：`Window` 不能 derive `Default`
    /// （`Instant` 无 `Default`、`[WinBucket; 120]` 超数组 `Default` 的 32 上限 ⇒ §4.1 的 P2-6），
    /// 方案 §4.1 明写"不要让 `Stats` 的 `#[derive(Default)]` 去推导 `Window`" ⇒ `Stats` 不再 derive `Default`。
    #[allow(clippy::new_without_default)] // 同上：构造必经本函数（它再经 `Window::new()`）⇒ 不提供 Default
    pub fn new() -> Stats {
        Stats {
            total: AtomicU64::new(0),
            preflight: AtomicU64::new(0),
            forwarded: AtomicU64::new(0),
            upstream_errors: AtomicU64::new(0),
            upstream_timeouts: AtomicU64::new(0),
            stream_errors: AtomicU64::new(0),
            client_aborted: AtomicU64::new(0),
            upstream_4xx: AtomicU64::new(0),
            upstream_5xx: AtomicU64::new(0),
            bytes: std::sync::Arc::new(ByteTotals::default()),
            pool_flushes: AtomicU64::new(0),
            retries: AtomicU64::new(0),
            active: AtomicI64::new(0),
            peak_active: AtomicI64::new(0),
            latency_buckets: (0..LATENCY_BUCKET_UPPER_MS.len())
                .map(|_| AtomicU64::new(0))
                .collect(),
            window: Window::new(),
            errors: Mutex::new(VecDeque::new()),
        }
    }

    pub fn inc_total(&self) {
        self.total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_preflight(&self) {
        self.preflight.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_forwarded(&self) {
        self.forwarded.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_upstream_error(&self) {
        self.upstream_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// FR-23：上游超时（504）与"不可达/其它失败"（502）分开计数。
    pub fn inc_upstream_timeout(&self) {
        self.upstream_timeouts.fetch_add(1, Ordering::Relaxed);
    }

    /// FR-23：响应流中途出错/首字节 watchdog 命中（已发头之后）。
    pub fn inc_stream_error(&self) {
        self.stream_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// FR-19：客户端断开（未读完就 drop 上游流 ⇒ 该请求被取消）。
    pub fn inc_client_aborted(&self) {
        self.client_aborted.fetch_add(1, Ordering::Relaxed);
    }

    /// FR-30：上游真实返回 4xx（不含代理合成的 502/504）。
    pub fn inc_upstream_4xx(&self) {
        self.upstream_4xx.fetch_add(1, Ordering::Relaxed);
    }

    /// FR-30：上游真实返回 5xx（不含代理合成的 502/504）。
    pub fn inc_upstream_5xx(&self) {
        self.upstream_5xx.fetch_add(1, Ordering::Relaxed);
    }

    /// 本修复批（L2）：连接池被重置一次（三条触发面：等待响应头超时 / `send()` 传输错误 / 响应流中断）。
    pub fn inc_pool_flush(&self) {
        self.pool_flushes.fetch_add(1, Ordering::Relaxed);
    }

    /// 本修复批（L4）：一次安全重试（白名单 = GET/HEAD/OPTIONS ∧ 无体 ∧ 已发字节 0）。
    pub fn inc_retry(&self) {
        self.retries.fetch_add(1, Ordering::Relaxed);
    }

    /// 请求体字节（发往上游的 ↑）—— **与曲线同源的一处**（P8-UI4：全局累计与近窗每秒速率一起动，
    /// 见 [`Stats::window`] 的 `record_bytes`）。
    pub fn add_bytes_in(&self, bytes: u64) {
        self.bytes.bytes_in.fetch_add(bytes, Ordering::Relaxed);
        let window = self.window();
        window.record_bytes(window.slice_now(), bytes, 0);
    }

    /// 响应体字节（上游回给客户端的 ↓）—— 同 [`Stats::add_bytes_in`]。
    pub fn add_bytes_out(&self, bytes: u64) {
        self.bytes.bytes_out.fetch_add(bytes, Ordering::Relaxed);
        let window = self.window();
        window.record_bytes(window.slice_now(), 0, bytes);
    }

    /// FR-30：记录一次"请求 → 上游响应头"延迟（毫秒）。
    pub fn record_upstream_latency_ms(&self, millis: u64) {
        let index = latency_bucket_index(millis);
        if let Some(bucket) = self.latency_buckets.get(index) {
            bucket.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// FR-30：延迟分位（`p` = 0–100）。无样本 ⇒ `None`（UI 显示 `—`）。
    /// 返回值 = 命中桶的上界（保守读数；桶宽见 `LATENCY_BUCKET_UPPER_MS`）。
    pub fn latency_percentile_ms(&self, p: u32) -> Option<u64> {
        percentile_ms_from_counts(&self.latency_counts(), p)
    }

    /// FR-30 的**显示口径**（审查 P2-7 修）：末桶（`> 10 s` 的全部样本都归它）**不能谎报成
    /// 桶上界 30000ms** —— 一个 10.2 s 的超时会被读成"30 秒"。末桶一律显示 `≥10s`。
    /// 普通桶照旧显示上界（`38ms` / `210ms` 这种量级就是它的用途）。
    pub fn latency_percentile_label(&self, p: u32) -> Option<String> {
        Some(percentile_label(self.latency_percentile_ms(p)?))
    }

    /// 累计直方图的计数副本（12 桶；固定数组 ⇒ 零堆分配、零索引）。
    fn latency_counts(&self) -> [u64; LATENCY_BUCKET_COUNT] {
        std::array::from_fn(|index| {
            self.latency_buckets
                .get(index)
                .map_or(0, |bucket| bucket.load(Ordering::Relaxed))
        })
    }

    /// FR-31：记一条错误（时间由本函数取，`line` 不含时间）。
    /// 与上一条**完全同文本**时合并（`count += 1` 并刷新时间），否则入队；
    /// 超过容量丢最旧。
    pub fn record_error(&self, line: &str) {
        let time = chrono::Local::now().format("%H:%M:%S").to_string();
        let mut guard = match self.errors.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(last) = guard.back_mut() {
            if last.line == line {
                last.time = time;
                last.count = last.count.saturating_add(1);
                return;
            }
        }
        guard.push_back(ErrorEntry {
            time,
            line: line.to_string(),
            count: 1,
        });
        while guard.len() > RECENT_ERRORS_CAPACITY {
            guard.pop_front();
        }
    }

    /// FR-31：最近的错误（**最新在前**）。
    pub fn recent_errors(&self) -> Vec<ErrorEntry> {
        let guard = match self.errors.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.iter().rev().cloned().collect()
    }

    /// FR-31：单行摘要（UI 的"最近错误"行）。
    pub fn latest_error(&self) -> Option<ErrorEntry> {
        let guard = match self.errors.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.back().cloned()
    }

    /// FR-29「当前活跃连接数」的分子：在途请求 +1（配 `dec_active` 成对使用；见 `proxy::ActiveGuard`）。
    pub fn inc_active(&self) {
        let current = self.active.fetch_add(1, Ordering::Relaxed) + 1;
        // CAS 循环而非 fetch_max：win7 目标的 std 里 `fetch_max` 也在，但这里保持最小原子面
        let mut peak = self.peak_active.load(Ordering::Relaxed);
        while current > peak {
            match self.peak_active.compare_exchange_weak(
                peak,
                current,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(observed) => peak = observed,
            }
        }
    }

    pub fn dec_active(&self) {
        self.active.fetch_sub(1, Ordering::Relaxed);
    }

    /// 错误总数（UI「错误」指标卡 = 上游 4xx/5xx + 代理自身错误 + 流中断）。
    pub fn total_errors(&self) -> u64 {
        self.upstream_errors.load(Ordering::Relaxed)
            + self.upstream_timeouts.load(Ordering::Relaxed)
            + self.stream_errors.load(Ordering::Relaxed)
            + self.upstream_4xx.load(Ordering::Relaxed)
            + self.upstream_5xx.load(Ordering::Relaxed)
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            total: self.total.load(Ordering::Relaxed),
            preflight: self.preflight.load(Ordering::Relaxed),
            forwarded: self.forwarded.load(Ordering::Relaxed),
            upstream_errors: self.upstream_errors.load(Ordering::Relaxed),
            upstream_timeouts: self.upstream_timeouts.load(Ordering::Relaxed),
            stream_errors: self.stream_errors.load(Ordering::Relaxed),
            client_aborted: self.client_aborted.load(Ordering::Relaxed),
            upstream_4xx: self.upstream_4xx.load(Ordering::Relaxed),
            upstream_5xx: self.upstream_5xx.load(Ordering::Relaxed),
            bytes_in: self.bytes.bytes_in.load(Ordering::Relaxed),
            bytes_out: self.bytes.bytes_out.load(Ordering::Relaxed),
            pool_flushes: self.pool_flushes.load(Ordering::Relaxed),
            retries: self.retries.load(Ordering::Relaxed),
            active: self.active.load(Ordering::Relaxed),
            peak_active: self.peak_active.load(Ordering::Relaxed),
        }
    }

    /// D1：近窗时间序列真源的唯一入口（记录点从这里进：`slice` 由调用方**每请求算一次**后向下传）。
    pub fn window(&self) -> &Window {
        &self.window
    }
}

/// 延迟 → 桶下标（首个"上界 ≥ 样本"的桶；超过全部上界 ⇒ 末桶）。
fn latency_bucket_index(millis: u64) -> usize {
    for (index, upper) in LATENCY_BUCKET_UPPER_MS.iter().enumerate() {
        if millis <= *upper {
            return index;
        }
    }
    LATENCY_BUCKET_UPPER_MS.len().saturating_sub(1)
}

impl Snapshot {
    pub fn render(&self) -> String {
        format!(
            "累计 请求={} 预检={} 转发={} 上游错误={} 上游超时={} 流中断={} 客户端中止={} 字节入={} 字节出={} 活跃={} 峰值活跃={} 池重置={} 重试={}",
            self.total,
            self.preflight,
            self.forwarded,
            self.upstream_errors,
            self.upstream_timeouts,
            self.stream_errors,
            self.client_aborted,
            self.bytes_in,
            self.bytes_out,
            self.active,
            self.peak_active,
            self.pool_flushes,
            self.retries,
        )
    }
}

// ---------------------------------------------------------------------------
// D1：唯一时间序列真源（120 桶 × 1 s）
// ---------------------------------------------------------------------------

/// D1：窗口长度（秒）= **120 桶 × 1 s**。面板「（近 120 s）」文案与 `SeriesPoint` 长度的唯一数值来源。
pub const WINDOW_SECS: u64 = 120;

/// 延迟直方图桶数（= `LATENCY_BUCKET_UPPER_MS` 的长度）。**不进公开面**：UI 只消费 `SeriesPoint`
/// ⇒ 曲线不需要桶上界表（B-7「接口面硬约束」）。
const LATENCY_BUCKET_COUNT: usize = LATENCY_BUCKET_UPPER_MS.len();

/// 桶戳两相之一（D1 / B-2）：`u64::MAX` = **有写者正在清零**。另一相 = 该桶所属的 `slice`（"就绪"）
/// —— 就绪相不设独立哨兵值，`slice` 自身即"就绪"。
const CLEARING: u64 = u64::MAX;

/// 曲线 y 值口径（B-7 ②）：末桶（`≥10s` 档；其**下界** = `LATENCY_BUCKET_UPPER_MS` 的倒数第二个上界
/// = 10,000 ms）在图上落 **10,000 ms**，并由 `SeriesPoint::latency_overflow` 标注 `≥10s`；
/// **禁止**画成 30,000。面板累计口径的 `30000` 语义仍由 `latency_percentile_label` 的 `≥10s` 标签承担。
/// 与桶表的绑定见单测 `series_overflow_y_value_matches_the_bucket_table`。
///
/// **D7 之后**：曲线改画速率 ⇒ 这个 y 值不再进曲线，但**保留**在 `SeriesPoint` 上 ——
/// 面板延迟读数与 J-B1b/J-B1c 的诚实性判据仍以它为观测面（删掉 = 既有判据失去观测点）。
const SERIES_OVERFLOW_Y_MS: u64 = 10_000;

/// D7：**全局累计字节计数**（面板 `↑B/↓B` 的同一来源；**P8-UI4** 起曲线的每秒速率也由这两个计数的
/// **同一处出口**（[`Stats::add_bytes_in`] / [`Stats::add_bytes_out`]）顺带写进近窗桶）。
///
/// `Window` 只在**每秒首样本**各 `load` 一次（2 次 `load` + 2 次 `store`/秒）；
/// **热路径的写入面一字不变** —— 仍然只有 `forward.rs` 每 chunk 那一次 `fetch_add`。
#[derive(Default)]
pub(crate) struct ByteTotals {
    pub(crate) bytes_in: AtomicU64,
    pub(crate) bytes_out: AtomicU64,
}

/// 一个 1 秒桶：**18 个 `AtomicU64`** = `stamp` + `requests` + `preflight` + `errors` + `latency[12]`
/// + **`bytes_in` / `bytes_out`**（**P8-UI4**：该秒内**实际记录到的**请求/响应体字节，直接累计；
///   不再做相邻秒边界差分）。
///   内存 = `18 × 8 B = 144 B`/桶 ⇒ 窗口 **`120 × 144 B = 17,280 B`**（一次性常数、无增长）。
struct WinBucket {
    /// 两相：`slice`（本桶所属秒 = "就绪"）· `CLEARING`（有写者正在清零 ⇒ 读侧跳过）。
    stamp: AtomicU64,
    requests: AtomicU64,
    preflight: AtomicU64,
    errors: AtomicU64,
    /// 复用既有 12 桶上界表（末桶 = `≥10s` 档）。
    latency: [AtomicU64; LATENCY_BUCKET_COUNT],
    /// **P8-UI4**：该秒的入向字节（发往上游的请求体）—— 速率 = 本值（不再读全局累计做差分）。
    bytes_in: AtomicU64,
    /// **P8-UI4**：该秒的出向字节（上游回给客户端的响应体）；语义同 `bytes_in`。
    bytes_out: AtomicU64,
}

impl WinBucket {
    fn new() -> WinBucket {
        WinBucket {
            // 初值 = 0（= 进程首个采样秒 `slice 0`）：`touch` 认领新桶必须能看到**非 `CLEARING`** 的戳，
            // 否则两条早退分支（`seen == CLEARING` / `slice < seen`）都够不着它 ⇒ 该槽**永久**丢样本
            //（没有任何写路径能把 `CLEARING` 推成新 slice）。
            stamp: AtomicU64::new(0),
            requests: AtomicU64::new(0),
            preflight: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            latency: std::array::from_fn(|_| AtomicU64::new(0)),
            bytes_in: AtomicU64::new(0),
            bytes_out: AtomicU64::new(0),
        }
    }

    /// 清零 17 个计数（**不含** `stamp`）—— 每秒首样本认领新秒时调一次。
    fn clear_counts(&self) {
        self.requests.store(0, Ordering::Relaxed);
        self.preflight.store(0, Ordering::Relaxed);
        self.errors.store(0, Ordering::Relaxed);
        for cell in self.latency.iter() {
            cell.store(0, Ordering::Relaxed);
        }
        self.bytes_in.store(0, Ordering::Relaxed);
        self.bytes_out.store(0, Ordering::Relaxed);
    }
}

/// D1：**唯一**时间序列真源（服务侧桶环，窗口 [`WINDOW_SECS`] 秒）。`Stats` 的累计计数器与之**并存**
/// —— 累计口径仍是 `Stats::snapshot()` 的唯一口径（访问行 `累计 …` 与既有判据不受影响）。
///
/// 成本口径：`record_*` **只做原子加**（零新增锁、零新增分配；`slice` 由调用方每请求算一次）；
/// 每秒首样本多 1 次 CAS + 17 次 `store`（认领新秒时清零）；
/// **P8-UI4** 另加：**每 chunk** 1 次 `Relaxed` load + 1~2 次 `fetch_add`（[`Window::record_bytes`]）；
/// `snapshot()` / `series()` 是读出口，**只在 1 Hz 节拍被调用**（`subsecond_fraction()` 另有每 100 ms
/// 一次时钟读，**不碰任何桶**），都不进请求路径。
pub struct Window {
    /// 进程内起点（构造时取一次）⇒ `slice_now()` = 自起点起的秒号。
    base: Instant,
    buckets: [WinBucket; WINDOW_SECS as usize],
    /// 有界丢弃计数（`CLEARING` 期间 / 回退戳的样本）——可见读数，判据用 `≥/≤`（D1）。
    dropped: AtomicU64,
}

impl Window {
    /// 构造面（**不** derive `Default`：`Instant` 无 `Default`、`[WinBucket; 120]` 超数组 `Default` 的
    /// 32 上限 ⇒ 走 `std::array::from_fn`；方案 §4.1 的 P2-6）。
    #[allow(clippy::new_without_default)] // 构造即取 base = Instant::now()（显式初始化动作）⇒ 不提供 Default
    pub fn new() -> Window {
        Window {
            base: Instant::now(),
            buckets: std::array::from_fn(|_| WinBucket::new()),
            dropped: AtomicU64::new(0),
        }
    }

    /// 当前秒号（自 `base` 起算）。请求路径**每请求算一次**后向下传（B-3：避免每个记录点各读一次时钟）；
    /// 字节路径**每 chunk 算一次**（P8-UI4：`record_bytes` 的 `slice` 由调用点传入，见 `Stats::add_bytes_in/out`）。
    pub fn slice_now(&self) -> u64 {
        self.base.elapsed().as_secs()
    }

    /// 每请求 1 次（与 `Stats::inc_total()` 同一处调用 ⇒ 口径 = 到达的全部请求，含预检）。
    pub fn record_request(&self, slice: u64) {
        if let Some(bucket) = self.touch(slice) {
            bucket.requests.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 每预检 1 次（预检早退分支；与 `Stats::inc_preflight()` 同一条件）。
    pub fn record_preflight(&self, slice: u64) {
        if let Some(bucket) = self.touch(slice) {
            bucket.preflight.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 出错时 1 次（`status >= 400`；与 `Stats::record_error()` 同一条件）。
    pub fn record_error(&self, slice: u64) {
        if let Some(bucket) = self.touch(slice) {
            bucket.errors.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 每**非预检**请求 1 次：TTFB（毫秒）落桶（桶上界表复用 `LATENCY_BUCKET_UPPER_MS`）。
    pub fn record_latency_ms(&self, slice: u64, millis: u64) {
        if let Some(bucket) = self.touch(slice) {
            let index = latency_bucket_index(millis);
            if let Some(cell) = bucket.latency.get(index) {
                cell.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// 每 **chunk** 1 次（P8-UI4；`Stats::add_bytes_in/out` 的同源出口）：把该 chunk 的字节记进
    /// **它到达的那一秒**的桶 ⇒ 该秒速率 = 桶内直接累计（不再做相邻秒边界差分）。
    ///
    /// **为什么必须按 chunk 记（P8-UI4 的根因修法）**：旧实现只在**请求级事件**（`record_request` /
    /// `record_preflight` / `record_latency_ms` / `record_error`）上写"边界快照"，而一条 20 s 的流式
    /// 响应期间**一个请求级事件都没有** ⇒ 期间每个点都 `None` ⇒ 曲线**整条不出现**（真机实测：
    /// 流式 chat 实传 136 KB，面板角标仍 `0 s / 120 s`）。
    /// 成本：常见路径 = 1 次 `Relaxed` load（本秒桶已就绪）+ 1~2 次 `fetch_add`；**每 chunk 每方向 1 次**
    /// `slice_now()`（时钟读）——该成本声明已由独立复验对照 `forward.rs:487/649` 两个出口核实。
    pub fn record_bytes(&self, slice: u64, bytes_in: u64, bytes_out: u64) {
        if let Some(bucket) = self.touch(slice) {
            if bytes_in > 0 {
                bucket.bytes_in.fetch_add(bytes_in, Ordering::Relaxed);
            }
            if bytes_out > 0 {
                bucket.bytes_out.fetch_add(bytes_out, Ordering::Relaxed);
            }
        }
    }

    /// **亚秒相位**（P8-UI4）：当前秒内已过的比例 `[0, 1)` —— 与 [`Window::slice_now`] 同一个
    /// `base` 时钟（曲线按它**平滑左移**；只影响 X 平移，不产生任何新读数）。
    pub fn subsecond_fraction(&self) -> f32 {
        self.base.elapsed().as_secs_f32().fract()
    }

    /// 近窗聚合（面板 4 张卡 / 延迟行 / 流量行的近窗部分）。**只在 1 Hz 节拍调用**（B-6）。
    pub fn snapshot(&self) -> WindowSnapshot {
        self.snapshot_at(self.slice_now())
    }

    /// 曲线唯一 API（B-7）：**旧→新**、固定 1 s 步长、长度 = `min(secs, WINDOW_SECS)`；
    /// 索引 `i` 对应 `slice_now() - (len - 1 - i)` 秒（第 0 个 = 最旧）。调用固定 `series(120)`。
    pub fn series(&self, secs: u64) -> Vec<SeriesPoint> {
        self.series_at(self.slice_now(), secs)
    }

    /// 有界丢弃的可见读数（`CLEARING` 期间与回退戳两类样本的累计次数）。
    pub fn dropped_samples(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// 近窗聚合的实现（`cur` 显式传入 ⇒ 单测可注入时间戳：纯逻辑、无计时）。
    fn snapshot_at(&self, cur: u64) -> WindowSnapshot {
        let mut requests: u64 = 0;
        let mut preflight: u64 = 0;
        let mut errors: u64 = 0;
        let mut latency = [0_u64; LATENCY_BUCKET_COUNT];
        for bucket in self.buckets.iter() {
            if !sample_inside_window(bucket.stamp.load(Ordering::Relaxed), cur) {
                continue;
            }
            requests += bucket.requests.load(Ordering::Relaxed);
            preflight += bucket.preflight.load(Ordering::Relaxed);
            errors += bucket.errors.load(Ordering::Relaxed);
            for (slot, count) in latency.iter_mut().zip(bucket_latency_counts(bucket)) {
                *slot += count;
            }
        }
        WindowSnapshot {
            secs: WINDOW_SECS,
            requests,
            preflight,
            errors,
            latency_samples: latency.iter().sum(),
            latency_p50: percentile_ms_from_counts(&latency, 50).map(percentile_label),
            latency_p95: percentile_ms_from_counts(&latency, 95).map(percentile_label),
            dropped_samples: self.dropped.load(Ordering::Relaxed),
        }
    }

    /// 逐秒序列的实现（`cur` 显式传入 ⇒ 单测可注入时间戳）。
    ///
    /// **P8-UI4**：速率 = 该秒桶内**直接累计的字节**（`bytes_in` / `bytes_out`）—— 只要该秒有桶
    /// （该秒里发生过任何请求/字节事件）就有速率；**零字节也是真读数**（`Some(0)`）。
    /// 无桶 / 陈旧桶 / `CLEARING` ⇒ 整点 `None`（**断线**，不是 0）。
    /// 序列**末点 = 进行中的那一秒**（值还在长）：`ui/curve.rs` 把它**排除在折线之外**，也不拿它当
    /// "当前值"（后者读**最新的完整秒**）—— 调用方不要自己丢，丢的规则在几何层（有单测钉住）。
    fn series_at(&self, cur: u64, secs: u64) -> Vec<SeriesPoint> {
        let len = secs.min(WINDOW_SECS);
        let mut points = Vec::with_capacity(len as usize);
        for index in 0..len {
            // 旧→新：第 0 个 = `len - 1` 秒前，最后一个 = `cur`（当前秒）。
            let back = len - 1 - index;
            points.push(match cur.checked_sub(back) {
                Some(slice) => self.point_at(slice),
                // 进程启动前的秒（`cur < back`）：本窗口没有该秒 ⇒ 无桶三字段。
                // **不得**用饱和减法把它折叠到 `slice 0` —— 那会把首秒的桶复制成一串"过去点"。
                None => SeriesPoint::absent(),
            });
        }
        points
    }

    /// 单点读法（**唯一实现处**，§4.1 / B-7 / **D7**）：`stamp == 该秒` 才读该桶；否则（无桶 /
    /// 上一圈的陈旧桶 / `CLEARING`）**该点整体不可读** —— 全字段取默认值，**禁止读该桶的任何计数器**
    ///（陈旧桶里是 120 s 前的计数 ⇒ 直接读会把旧值画成"这一秒的 req/s" = 幽灵尖峰）。
    fn point_at(&self, slice: u64) -> SeriesPoint {
        let Some(current) = self.sample_at(slice) else {
            return SeriesPoint::absent();
        };
        // **P8-UI4**：速率 = 该秒桶内**实际记录到的**字节（直接累计，见 `touch`/`record_bytes`）——
        // 只要该秒有桶（该秒里发生过任何请求/字节事件）就**有速率**；**零字节也是真读数**（`Some(0)`）。
        // 无桶 / 陈旧桶 / `CLEARING` ⇒ 上面那一步已返回整点 `None`（**断线**，绝不补 0、不插值）。
        let p50 = percentile_ms_from_counts(&bucket_latency_counts(current.bucket), 50);
        // 末桶（`≥10s` 档）⇒ 该点 y 值落 10,000（**不**谎报 30,000），由 `latency_overflow` 标注
        let overflow = p50 == latency_overflow_upper_ms();
        SeriesPoint {
            latency_p50_ms: if overflow {
                Some(SERIES_OVERFLOW_Y_MS)
            } else {
                p50
            },
            latency_overflow: overflow,
            bytes_in_per_sec: Some(current.bytes_in),
            bytes_out_per_sec: Some(current.bytes_out),
            requests: current.requests,
        }
    }

    /// 单秒读数（[`Window::sample_at`] 的返回体；借桶以复用延迟直方图读数）—— **唯一可信读法**：
    /// `stamp == slice` 才可信（无桶 / 上一圈的陈旧桶 / `CLEARING` ⇒ `None`）。
    ///
    /// 用 `Acquire` 读就绪相 ⇒ 与 [`Window::touch`] 的 `Release` 发布配对 ⇒ 看到戳就**必然**看到
    /// 已经清过零的字节格（否则会读到上一个 120 s 周期留下的**残留值** = 假的巨峰）。
    fn sample_at(&self, slice: u64) -> Option<Sample<'_>> {
        let bucket = self.buckets.get((slice % WINDOW_SECS) as usize)?;
        if bucket.stamp.load(Ordering::Acquire) != slice {
            return None;
        }
        Some(Sample {
            bucket,
            bytes_in: bucket.bytes_in.load(Ordering::Relaxed),
            bytes_out: bucket.bytes_out.load(Ordering::Relaxed),
            requests: bucket.requests.load(Ordering::Relaxed),
        })
    }

    // `Boundary` / `SeriesPoint` 的**模块级**定义在本 `impl` 块之后（嵌套在 `impl` 里的类型对其它
    // 模块不可见 —— `ui/curve.rs` 需要 `azusa_local_proxy::stats::SeriesPoint`）。

    /// 两相 CAS 认领（D1 / B-2）：只接受 `new_slice > observed`；**禁止"先加后清"**（会丢增量）。
    /// 返回 `None` = 本样本**有界丢弃**（`dropped += 1`，可见）；调用方一律跳过计数。
    fn touch(&self, slice: u64) -> Option<&WinBucket> {
        let bucket = self.buckets.get((slice % WINDOW_SECS) as usize)?;
        loop {
            let observed = bucket.stamp.load(Ordering::Relaxed);
            if observed == slice {
                return Some(bucket); // 本秒桶已就绪
            }
            if observed == CLEARING || slice < observed {
                // 有写者正在清零（CLEARING）/ 回退戳（防 ABA 把新秒的数据清掉）⇒ 丢弃（有界且可见）
                self.dropped.fetch_add(1, Ordering::Relaxed);
                return None;
            }
            if bucket
                .stamp
                .compare_exchange(observed, CLEARING, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                bucket.clear_counts();
                // 内存序仍是判据面的一部分：**清零必须发生在 `stamp` 置成就绪相之前**，且就绪相用
                // `Release` 发布（读侧 `Acquire`）⇒ 读侧不可能看到"戳已就绪但格子里还是**上一个
                // 120 s 周期**的残留值" —— 那会把旧值当成"这一秒的速率" = 假的巨峰。
                bucket.stamp.store(slice, Ordering::Release);
                return Some(bucket);
            }
        }
    }
}

/// 单秒读数（[`Window::sample_at`] 的返回体；借桶以复用延迟直方图读数）。
///
/// **P8-UI4**：字段 = 该秒桶内**直接累计**的字节（不再是"边界快照"）⇒ 速率 = 本结构体的
/// `bytes_in` / `bytes_out`（无差分、无相邻秒前提）。
struct Sample<'a> {
    bucket: &'a WinBucket,
    bytes_in: u64,
    bytes_out: u64,
    requests: u64,
}

/// 曲线单点（`Window::series` 的元素；序列 **旧→新**）。
///
/// **D7 之后**曲线画的是速率（`bytes_in_per_sec` / `bytes_out_per_sec`）；延迟字段**保留**：
/// 面板延迟读数行与 J-B1b/J-B1c 的诚实性判据以它为观测面（删掉 = 既有判据失去观测点）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeriesPoint {
    /// `Some` = 该秒有桶且桶内有延迟样本（值 = 命中桶的上界；末桶给 `SERIES_OVERFLOW_Y_MS`）；
    /// `None` = 该秒**无延迟样本 / 无桶** ⇒ 折线断开（不补 0、不插值、不沿用前值）。
    pub latency_p50_ms: Option<u64>,
    /// `true` = 该秒 p50 落**末桶**（`≥10s` 档）⇒ 该点标 `≥10s`、y 值 = 10,000 ms（**禁止** 30,000）。
    pub latency_overflow: bool,
    /// **P8-UI4**：该秒的入向速率（bytes/s）= 该秒桶内**直接累计**的请求体字节
    ///（`Stats::add_bytes_in` 每 chunk 调一次）；**零字节也是真读数**（`Some(0)`）。
    /// `None` = **断线**（该秒无桶 / 陈旧桶 / `CLEARING`）⇒ 折线断开、**不补 0、不插值**。
    pub bytes_in_per_sec: Option<u64>,
    /// **P8-UI4**：该秒的出向速率（bytes/s）；语义同 `bytes_in_per_sec`（两者同生同灭）。
    pub bytes_out_per_sec: Option<u64>,
    /// 该秒桶内请求数；**不可读的秒恒为 0**（不得读陈旧桶计数器 —— 幽灵尖峰的来源）。
    pub requests: u64,
}

impl SeriesPoint {
    /// "无桶秒"（无桶 / 陈旧桶 / `CLEARING` / 进程启动前的秒）—— 全字段整体不可读。
    fn absent() -> SeriesPoint {
        SeriesPoint {
            latency_p50_ms: None,
            latency_overflow: false,
            bytes_in_per_sec: None,
            bytes_out_per_sec: None,
            requests: 0,
        }
    }
}

/// 近窗读数快照（面板与曲线的**同一帧同一次**读出口；`Clone` 供 UI 每帧拷一份）。
#[derive(Clone, Debug)]
pub struct WindowSnapshot {
    /// = `WINDOW_SECS`（120）——「（近 120 s）」文案的唯一数值来源。
    pub secs: u64,
    pub requests: u64,
    pub preflight: u64,
    pub errors: u64,
    /// 窗口内延迟样本总数（0 ⇒ 空窗）。
    pub latency_samples: u64,
    /// 复用既有显示口径（含 `≥10s`；无样本 ⇒ `None`）。
    pub latency_p50: Option<String>,
    pub latency_p95: Option<String>,
    /// 有界丢弃的可见读数。
    pub dropped_samples: u64,
}

impl WindowSnapshot {
    /// 空闲态判定的一半（另一半是 `Snapshot::active == 0`，见 B-4）。
    pub fn is_idle(&self) -> bool {
        self.requests == 0 && self.latency_samples == 0
    }

    /// 延迟行正文（B-1 逐字文案）：空窗 ⇒ `—（近 120 s 无样本）`；否则 `p50 X / p95 Y（近 120 s）`。
    pub fn latency_text(&self) -> String {
        match (self.latency_p50.as_deref(), self.latency_p95.as_deref()) {
            (Some(p50), Some(p95)) => format!("p50 {p50} / p95 {p95}（近 {} s）", self.secs),
            _ => format!("—（近 {} s 无样本）", self.secs),
        }
    }
}

/// 计入条件（D1 / B-2；修 P0-2）：`stamp != CLEARING ∧ stamp <= cur ∧ age < WINDOW_SECS`。
/// **禁止** `stamp ∈ {cur, cur-1}` —— 那等于 1–2 s 窗口，与「近 120 s」自相矛盾。
fn sample_inside_window(stamp: u64, cur: u64) -> bool {
    if stamp == CLEARING || stamp > cur {
        return false;
    }
    cur.saturating_sub(stamp) < WINDOW_SECS
}

/// 12 桶计数 → 分位值（`p` = 0–100）：返回**命中桶的上界**（保守读数）；无样本 ⇒ `None`。
/// **唯一实现处**：累计口径（`Stats::latency_percentile_ms`）与近窗口径（`Window`）共用同一算法
///（两份实现必然漂移，正是 P0-2 那一类缺陷的温床）。
fn percentile_ms_from_counts(counts: &[u64], p: u32) -> Option<u64> {
    let total: u64 = counts.iter().copied().sum();
    if total == 0 {
        return None;
    }
    let target = ((u64::from(p.clamp(1, 100)) * total).div_ceil(100)).max(1);
    let mut accumulated: u64 = 0;
    for (index, count) in counts.iter().enumerate() {
        accumulated += *count;
        if accumulated >= target {
            return LATENCY_BUCKET_UPPER_MS.get(index).copied();
        }
    }
    LATENCY_BUCKET_UPPER_MS.last().copied()
}

/// 分位值 → 显示标签（FR-30 数字诚实性；审查 P2-7）：末桶一律 `≥10s`，**不**谎报桶上界 30000ms。
fn percentile_label(value: u64) -> String {
    if Some(value) == latency_overflow_upper_ms() {
        LATENCY_OVERFLOW_LABEL.to_string()
    } else {
        format!("{value}ms")
    }
}

/// 末桶上界（表值 `30000`）：**只用于判定"该分位是否落末桶"**，不得作为显示值或绘制值
///（显示 = `≥10s`；曲线 y = `SERIES_OVERFLOW_Y_MS`）。
fn latency_overflow_upper_ms() -> Option<u64> {
    LATENCY_BUCKET_UPPER_MS.last().copied()
}

/// 桶内延迟直方图的计数副本（固定数组 ⇒ 零堆分配、零索引）。
fn bucket_latency_counts(bucket: &WinBucket) -> [u64; LATENCY_BUCKET_COUNT] {
    std::array::from_fn(|index| {
        bucket
            .latency
            .get(index)
            .map_or(0, |cell| cell.load(Ordering::Relaxed))
    })
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;

    use std::sync::Arc;

    /// 审查 P2-7：末桶（> 10 s）读数**不得**谎报为桶上界 30000ms ⇒ 显示 `≥10s`。
    #[test]
    fn latency_overflow_bucket_is_labelled_honestly() {
        let stats = Stats::new();
        stats.record_upstream_latency_ms(12); // 普通桶 ⇒ 25ms
        assert_eq!(stats.latency_percentile_label(50).as_deref(), Some("25ms"));

        let stats = Stats::new();
        stats.record_upstream_latency_ms(10_008); // 10 s 连接超时样本 ⇒ 末桶
        assert_eq!(
            stats.latency_percentile_label(50).as_deref(),
            Some(LATENCY_OVERFLOW_LABEL),
            "10.2 s 的样本必须读作 ≥10s，不是 30000ms"
        );
        assert_eq!(
            stats.latency_percentile_ms(50),
            Some(30_000),
            "底层仍返回桶上界（口径不变，只是显示层改）"
        );

        let stats = Stats::new();
        assert_eq!(
            stats.latency_percentile_label(50),
            None,
            "无样本 ⇒ None（UI 显示 —）"
        );
    }

    /// Phase 2：直方图分位（无样本 ⇒ None；单调；末桶兜底）。
    #[test]
    fn latency_percentiles_are_monotonic() {
        let stats = Stats::new();
        assert_eq!(stats.latency_percentile_ms(50), None);
        assert_eq!(stats.latency_percentile_ms(95), None);

        for millis in [3_u64, 4, 8, 30, 60, 90, 120, 240, 700, 1500] {
            stats.record_upstream_latency_ms(millis);
        }
        let p50 = stats.latency_percentile_ms(50).expect("有样本");
        let p95 = stats.latency_percentile_ms(95).expect("有样本");
        assert!(p50 <= p95, "p50 {p50} 必须 <= p95 {p95}");
        // 样本落桶：3,4→5ms 桶；8→10；30→50；60,90→100；120,240→250；700→1000；1500→2500
        // p50 target = 5 ⇒ 累计到 100ms 桶（6 个）才够 ⇒ p50 = 100
        assert_eq!(p50, 100);
        // p95 target = 10 ⇒ 最后一个样本的桶 ⇒ p95 = 2500
        assert_eq!(p95, 2500);

        // 超过全部上界 ⇒ 末桶（30000）
        let huge = Stats::new();
        huge.record_upstream_latency_ms(86_400_000);
        assert_eq!(huge.latency_percentile_ms(50), Some(30000));
    }

    /// Phase 2：最近错误环形缓冲（合并连续同文本、容量封顶、最新在前）。
    #[test]
    fn recent_errors_merge_and_stay_bounded() {
        let stats = Stats::new();
        assert!(stats.latest_error().is_none());

        stats.record_error("HTTP 502 上游连接失败");
        stats.record_error("HTTP 502 上游连接失败");
        let latest = stats.latest_error().expect("有一条");
        assert_eq!(latest.count, 2, "连续同文本必须合并计数");
        assert_eq!(latest.line, "HTTP 502 上游连接失败");

        stats.record_error("HTTP 504 上游超时");
        assert_eq!(stats.recent_errors().len(), 2);
        assert_eq!(stats.latest_error().unwrap().line, "HTTP 504 上游超时");

        // 灌满 + 溢出：容量封顶在 RECENT_ERRORS_CAPACITY，最旧的被挤掉
        for index in 0..(RECENT_ERRORS_CAPACITY + 5) {
            stats.record_error(&format!("错误 {index}"));
        }
        let recent = stats.recent_errors();
        assert_eq!(recent.len(), RECENT_ERRORS_CAPACITY);
        assert_eq!(
            recent[0].line,
            format!("错误 {}", RECENT_ERRORS_CAPACITY + 4)
        );

        // 错误总数 = 上游 4xx/5xx + 代理错误 + 流中断
        let counters = Stats::new();
        counters.inc_upstream_4xx();
        counters.inc_upstream_5xx();
        counters.inc_upstream_error();
        counters.inc_upstream_timeout();
        counters.inc_stream_error();
        assert_eq!(counters.total_errors(), 5);
    }

    /// Phase 2：字节进出分开计（↑ 请求体 / ↓ 响应体）。
    #[test]
    fn bytes_in_and_out_are_separate() {
        let stats = Stats::new();
        stats.add_bytes_in(1024);
        stats.add_bytes_out(2048);
        let snapshot = stats.snapshot();
        assert_eq!(snapshot.bytes_in, 1024);
        assert_eq!(snapshot.bytes_out, 2048);
        assert!(snapshot.render().contains("字节入=1024"));
    }

    // -----------------------------------------------------------------------
    // D1：近窗时间序列真源（`Window`：120 桶 × 1 s）
    // -----------------------------------------------------------------------

    /// 单测专用：把 `slot`（= `slice % WINDOW_SECS`）直接置成 `stamp` + 哨兵计数。
    /// `slot` 直给 ⇒ `CLEARING` 也能落在指定槽上。
    fn plant(window: &Window, slot: usize, stamp: u64, requests: u64, overflow_latency: u64) {
        if let Some(bucket) = window.buckets.get(slot) {
            bucket.stamp.store(stamp, Ordering::Relaxed);
            bucket.requests.store(requests, Ordering::Relaxed);
            if let Some(cell) = bucket.latency.get(LATENCY_BUCKET_COUNT - 1) {
                cell.store(overflow_latency, Ordering::Relaxed);
            }
        }
    }

    /// 桶内 **18** 个 `AtomicU64` 的结构性上界（**D7 设计点**：`18 × 8 B = 144 B`/桶
    /// ⇒ `× 120 = `**`17,280 B`**）。多一个字段本测试立刻红 —— 这是"桶字段面"的机械咬合面。
    ///
    /// D1 的 `16 × 8 = 128 B` / `15,360 B`（"不含字节"）**已由 D7 作废**：曲线改画速率 ⇒
    /// 每桶 +2 格字节，`17,280 B` 从"上限口径"转为**设计点**。
    /// **P8-UI4** 换的是这 2 格的**语义**（边界快照 → 该秒直接累计），**格数一字未动**（仍是 18）。
    #[test]
    fn window_bucket_stays_within_the_18_atomic_budget() {
        assert_eq!(std::mem::size_of::<AtomicU64>(), 8);
        assert_eq!(
            std::mem::size_of::<WinBucket>(),
            18 * std::mem::size_of::<AtomicU64>(),
            "WinBucket 必须恰为 18 个 AtomicU64（stamp + requests + preflight + errors + latency[12] \
             + bytes_in + bytes_out）"
        );
        assert_eq!(
            std::mem::size_of::<WinBucket>() * WINDOW_SECS as usize,
            17_280,
            "120 桶 × 144 B = 17,280 B（D7 设计点：18 原子/桶 —— 现取算术）"
        );
    }

    /// 单测专用：直接写某秒的**字节格**（不动全局计数，避免依赖真实时钟）。
    /// **P8-UI4**：写的就是速率本身（每秒直接累计）⇒ 不再需要"造相邻秒边界"这一层。
    fn plant_bytes(window: &Window, slice: u64, bytes_in: u64, bytes_out: u64) {
        if let Some(bucket) = window.buckets.get(slice as usize % WINDOW_SECS as usize) {
            bucket.stamp.store(slice, Ordering::Release);
            bucket.bytes_in.store(bytes_in, Ordering::Relaxed);
            bucket.bytes_out.store(bytes_out, Ordering::Relaxed);
        }
    }

    /// 【P8-UI4】速率 = 该秒桶内**直接累计的字节**（不再是相邻秒边界差分）：**有桶就有速率**
    ///（**零字节也是真读数** `Some(0)`），无桶 / 陈旧桶 ⇒ 整点 `None`（**断线**，绝不补 0）。
    ///
    /// **负例（可咬）**：把 `point_at` 的 `stamp == slice` 校验去掉（读桶不看戳）⇒ 陈旧桶那条断言
    /// 立刻红（9_999 会被当成"这一秒的速率" = 幽灵尖峰）。
    #[test]
    fn p8ui4_rate_is_the_bytes_measured_inside_that_second() {
        let stats = Stats::new();
        let window = stats.window();
        // 第 11 秒 = (1,000, 2,000)；第 12 秒 = (3,000, 2,500)
        plant_bytes(window, 11, 1_000, 2_000);
        plant_bytes(window, 12, 3_000, 2_500);
        // 同槽的**陈旧桶**（另一圈的戳）：本秒 13 只看到 9_999 的残留 ⇒ 必须判不可读
        if let Some(bucket) = window.buckets.get(13 % WINDOW_SECS as usize) {
            bucket.stamp.store(13 + WINDOW_SECS, Ordering::Release);
            bucket.bytes_in.store(9_999, Ordering::Relaxed);
            bucket.bytes_out.store(9_999, Ordering::Relaxed);
        }

        let points = window.series_at(13, 4); // 覆盖 slice 10..13（旧→新）
        assert_eq!(points.len(), 4);
        assert_eq!(
            points[0].bytes_in_per_sec, None,
            "slice 10 无桶 ⇒ None（不补 0）"
        );
        assert_eq!(points[0].bytes_out_per_sec, None);
        assert_eq!(
            points[1].bytes_in_per_sec,
            Some(1_000),
            "slice 11 = 桶内累计"
        );
        assert_eq!(points[1].bytes_out_per_sec, Some(2_000));
        assert_eq!(
            points[2].bytes_in_per_sec,
            Some(3_000),
            "slice 12 = 桶内累计"
        );
        assert_eq!(points[2].bytes_out_per_sec, Some(2_500));
        assert_eq!(points[3].bytes_in_per_sec, None, "陈旧桶 ⇒ 整点不可读");
        assert_ne!(points[3].bytes_in_per_sec, Some(9_999));
    }

    /// 【P8-UI4】**与面板同源**：`Stats::add_bytes_in/out` 一处**同时**动"面板累计 ↑/↓"与"曲线每秒桶"
    /// —— 生产路径上这就是**同一个调用点**（`forward.rs` 每 chunk 一次）⇒ 曲线与面板不可能各说各话。
    #[test]
    fn p8ui4_byte_accounting_is_wired_at_the_same_call_site_as_the_panel_totals() {
        let stats = Stats::new();
        // 与生产路径同一条出口
        stats.add_bytes_in(777);
        stats.add_bytes_out(333);
        let snapshot = stats.snapshot();
        assert_eq!(snapshot.bytes_in, 777, "面板累计 ↑B");
        assert_eq!(snapshot.bytes_out, 333, "面板累计 ↓B");

        // 曲线侧：同源字节落进**当前秒**的桶。⚠ 两次 add 之间可能跨秒（极小概率）⇒ 按"最近两秒之和"
        // 断言，避开时钟抖动的假失败（覆盖面不变：仍是"这一处出口"的记录面）。
        let window = stats.window();
        let cur = window.slice_now();
        let points = window.series_at(cur, 2);
        let sum_in: u64 = points
            .iter()
            .filter_map(|point| point.bytes_in_per_sec)
            .sum();
        let sum_out: u64 = points
            .iter()
            .filter_map(|point| point.bytes_out_per_sec)
            .sum();
        assert_eq!(sum_in, 777, "曲线速率 = 与面板同源的字节");
        assert_eq!(sum_out, 333);
        for point in points.iter() {
            assert_eq!(point.requests, 0, "字节事件不冒充请求数");
        }
    }

    /// 【A-14'】速率侧的诚实性：**"没测到"与"测到 0"必须可区分** —— 无桶的那一秒是 `None`
    ///（断线，**绝不补 0**），真有桶而一个字节都没过的那一秒是 `Some(0)`（真读数）。
    ///
    /// P8-UI4 改型说明：旧的"边界差分会算出假 0（饱和减法）"这条路径**已不存在**（直接累计无减法），
    /// 但**"无桶不许当 0"** 这条口径**一字未改**，换成了"无桶 ⇒ `None`"这个更短的机械面。
    #[test]
    fn a14_no_bucket_is_none_while_a_measured_zero_is_some_zero() {
        let stats = Stats::new();
        let window = stats.window();
        // 第 21 秒：有桶（一次零字节事件）但一字节未过 ⇒ 真 0；第 20 / 22 秒：无桶
        window.record_request(21);
        let points = window.series_at(22, 3);
        assert_eq!(points.len(), 3);
        assert_eq!(
            points[0].bytes_in_per_sec, None,
            "slice 20 无桶 ⇒ None（不是 0）"
        );
        assert_eq!(points[0].bytes_out_per_sec, None, "一对同生同灭");
        assert_eq!(
            points[1].bytes_in_per_sec,
            Some(0),
            "slice 21 有桶、真 0 ⇒ Some(0)"
        );
        assert_eq!(points[1].bytes_out_per_sec, Some(0));
        assert_eq!(points[1].requests, 1);
        assert_eq!(points[2].bytes_in_per_sec, None, "slice 22 无桶 ⇒ None");
    }

    /// 曲线 y 值口径与桶表同源：末桶（`≥10s`）的 y 值 = 倒数第二个桶上界（10,000 ms）。
    #[test]
    fn series_overflow_y_value_matches_the_bucket_table() {
        assert_eq!(
            SERIES_OVERFLOW_Y_MS,
            LATENCY_BUCKET_UPPER_MS
                .get(LATENCY_BUCKET_UPPER_MS.len() - 2)
                .copied()
                .unwrap_or(0),
            "末桶下界（= 倒数第二个桶上界）= 曲线在 `≥10s` 档的 y 值"
        );
    }

    /// 【J-B1】老样本边界（注入时间戳，`now = 100`）：**只有真 120 s 窗口能全过**。
    /// `stamp = 41`（age 59）计入 · `stamp = 45`（age **55**）计入 · `stamp = 39`（age 61）**计入**
    /// （⚠ 方案原句写"不计入"，属 v1 60 s 窗口残留 —— 见本测试体内的裁定记录）·
    /// 未来戳（101）不计入 · `CLEARING` 桶跳过；空窗 ⇒ `p50/p95 == None`；
    /// 窗口的精确边界另由 `cur = 200`：age 119 计入 / age 120 不计入 覆盖。
    #[test]
    fn window_reads_the_full_120s_window() {
        let empty = Window::new();
        let snapshot = empty.snapshot_at(100);
        assert_eq!(snapshot.requests, 0);
        assert!(snapshot.is_idle());
        assert_eq!(snapshot.latency_p50, None);
        assert_eq!(snapshot.latency_p95, None);

        // **J-B5 的咬合点**（本断言放在最前 ⇒ 负例注入后失败点就是"age 55 计入"这一条）：
        // `stamp = 45`（age 55）只有 **≥56 s 的窗口**能通过（120 s ✅ / `{cur,cur-1}` ❌ / WINDOW_SECS = 1 ❌）。
        let age55 = Window::new();
        age55.record_request(45);
        assert_eq!(
            age55.snapshot_at(100).requests,
            1,
            "`stamp = 45`（age 55）必须计入 —— 窗口语义不得退化成 1–2 s"
        );

        let window = Window::new();
        window.record_request(41);
        window.record_latency_ms(41, 12);
        window.record_request(45); // age 55
        window.record_latency_ms(45, 12);
        window.record_request(39); // age 61（见下"60 s 残留"注）
        window.record_request(101); // 未来戳 ⇒ 不计入（`stamp <= cur`）
        plant(&window, 77, CLEARING, 999, 7); // CLEARING 桶（哨兵计数必须被跳过）

        // ⚠ 方案 J-B1 的原句 "`stamp = 39`（age 61）**不计入**" 与**同一判据的读法**
        //（`age < WINDOW_SECS`，WINDOW_SECS = 120）自相矛盾 —— 那是 v1 **60 s 窗口**的残留
        //（§10.2 ① 的"为何仍能抓真缺陷"只论证 age 55 那一条；此处 age 61 < 120 ⇒ 属于窗口内）。
        // 本批按 D1 的 120 s 落地（120 桶 / 「近 120 s」文案 / J-B2 的 130 s 静置都以此为据），
        // 并把"窗口外样本不计入"的**原意**改由 `cur = 200` 的精确边界（age 119 计入 / 120 不计入）等价覆盖。
        let snapshot = window.snapshot_at(100);
        assert_eq!(
            snapshot.requests, 3,
            "age 59 / age 61 与 age 55 都在 120 s 窗口内 ⇒ 计入；未来戳不计入；CLEARING 桶跳过"
        );
        assert_eq!(snapshot.latency_samples, 2);
        assert_eq!(snapshot.latency_p50.as_deref(), Some("25ms"));
        assert_eq!(snapshot.latency_p95.as_deref(), Some("25ms"));
        assert_eq!(snapshot.secs, WINDOW_SECS);

        // 窗口的精确边界（`age < WINDOW_SECS`）：age 119 计入 / age 120 不计入
        let edge = Window::new();
        edge.record_request(81); // cur = 200 ⇒ age 119
        edge.record_request(80); // cur = 200 ⇒ age 120（= WINDOW_SECS）⇒ 窗口外
        assert_eq!(
            edge.snapshot_at(200).requests,
            1,
            "age 119 计入 / age 120 不计入"
        );
    }

    /// 【J-B1b】`series()` 的"无桶秒"三字段（v2.1 勘误 P1-1①）：`stamp != 该秒` ⇒ `requests == 0` ∧
    /// `latency_p50_ms == None` ∧ `latency_overflow == false`，且**禁止读该桶的任何计数器**。
    /// 咬合手法 = 哨兵值（陈旧桶 `requests = 9999` / `latency[11] = 7`）。
    #[test]
    fn series_no_bucket_yields_zero_requests_and_none_latency() {
        let window = Window::new();
        // 第 299 秒（`cur - 1`）的槽里蹲着**上一圈**的桶（stamp = 179，同槽 59）：
        // 无条件读桶的实现会把 9999 画成"这一秒的 req/s" = 幽灵尖峰
        plant(&window, 59, 179, 9999, 7);
        // 第 298 秒的槽：`CLEARING`（有写者正在清零）+ 同样的哨兵
        plant(&window, 58, CLEARING, 9999, 7);
        window.record_request(300); // 当前秒真值（对照组）
        window.record_latency_ms(300, 12);

        let points = window.series_at(300, WINDOW_SECS);
        assert_eq!(points.len(), WINDOW_SECS as usize);

        let stale = points.get(118).copied().expect("第 299 秒的点"); // index 118 ⇒ 299
        assert_eq!(stale.requests, 0, "陈旧桶的 9999 不得泄漏成这一秒的 req/s");
        assert_eq!(stale.latency_p50_ms, None);
        assert!(!stale.latency_overflow);

        let clearing = points.get(117).copied().expect("第 298 秒的点"); // index 117 ⇒ 298
        assert_eq!(clearing.requests, 0);
        assert_eq!(clearing.latency_p50_ms, None);
        assert!(!clearing.latency_overflow);

        let current = points.get(119).copied().expect("当前秒的点"); // index 119 ⇒ 300
        assert_eq!(current.requests, 1, "当前秒的桶必须真读");
        assert_eq!(current.latency_p50_ms, Some(25));
        assert!(!current.latency_overflow);

        assert_eq!(
            points.iter().map(|point| point.requests).sum::<u64>(),
            1,
            "两个哨兵槽都不得贡献请求数"
        );
    }

    /// 【J-B1c】末桶诚实（v2.1 勘误 P1-1②）：`10_008 ms` ⇒ `latency_overflow == true` ∧ y 值 = 10,000 ms
    ///（**禁止** `Some(30_000)`）；`12 ms` ⇒ `false` ∧ `Some(25)`。
    #[test]
    fn series_marks_the_overflow_bucket() {
        let overflow = Window::new();
        overflow.record_request(100);
        overflow.record_latency_ms(100, 10_008);
        let points = overflow.series_at(100, WINDOW_SECS);
        let last = points.get(119).copied().expect("当前秒的点");
        assert!(last.latency_overflow, "10.008 s 的样本落末桶 ⇒ `≥10s` 档");
        assert_ne!(last.latency_p50_ms, Some(30_000), "末桶不得谎报 30000 ms");
        assert_eq!(last.latency_p50_ms, Some(10_000), "曲线 y 值 = 末桶下界");
        assert_eq!(last.requests, 1);

        let normal = Window::new();
        normal.record_latency_ms(100, 12);
        let points = normal.series_at(100, WINDOW_SECS);
        let last = points.get(119).copied().expect("当前秒的点");
        assert!(!last.latency_overflow);
        assert_eq!(last.latency_p50_ms, Some(25));
        assert_eq!(normal.snapshot_at(100).latency_p50.as_deref(), Some("25ms"));
    }

    /// 恒等：`series(120)` 各点 `requests` 之和 == `snapshot()` 的近窗请求数（同一窗口、同一读法）；
    /// 且序列索引与秒号一一对应（旧→新）。
    #[test]
    fn window_totals_match_the_recorded_samples() {
        let window = Window::new();
        for slice in [100_u64, 150, 199, 200, 200, 200] {
            window.record_request(slice);
        }
        window.record_latency_ms(199, 8);

        let snapshot = window.snapshot_at(200);
        assert_eq!(snapshot.requests, 6);
        assert_eq!(snapshot.latency_samples, 1);
        assert_eq!(snapshot.latency_p50.as_deref(), Some("10ms"));

        let points = window.series_at(200, WINDOW_SECS);
        assert_eq!(
            points.iter().map(|point| point.requests).sum::<u64>(),
            snapshot.requests,
            "序列与聚合必须同源（同一窗口、同一读法）"
        );
        assert_eq!(points.get(19).copied().expect("秒 100").requests, 1); // 200 - (119 - 19) = 100
        assert_eq!(points.get(119).copied().expect("秒 200").requests, 3);
        assert_eq!(
            points.first().copied().expect("秒 81").requests,
            0,
            "最旧一端（秒 81）没有桶 ⇒ 0"
        );
    }

    /// 边界：`series(0)` ⇒ 空 · 长度上钳到 `WINDOW_SECS` · 索引旧→新 · 进程启动前的秒（`cur < back`）
    /// ⇒ 无桶三字段（**不得**折叠成 `slice 0` 的桶）。
    #[test]
    fn series_returns_one_point_per_second_and_clamps_the_length() {
        let window = Window::new();
        assert!(window.series_at(100, 0).is_empty(), "series(0) ⇒ 空序列");
        assert_eq!(
            window.series_at(100, WINDOW_SECS + 1_000).len(),
            WINDOW_SECS as usize,
            "长度上钳到 WINDOW_SECS"
        );

        let short = window.series_at(100, 5);
        assert_eq!(short.len(), 5);
        for point in short.iter() {
            assert_eq!(point.requests, 0);
            assert_eq!(point.latency_p50_ms, None);
            assert!(!point.latency_overflow);
        }

        // 首秒的桶只能出现在"第 0 秒"（启动前的秒必须是无桶三字段；饱和减法会把它复制成一串过去点）
        let early = Window::new();
        early.record_request(0);
        let points = early.series_at(0, WINDOW_SECS);
        assert_eq!(points.len(), WINDOW_SECS as usize);
        assert_eq!(
            points
                .get(WINDOW_SECS as usize - 1)
                .copied()
                .expect("当前秒（0）")
                .requests,
            1
        );
        assert_eq!(
            points.iter().map(|point| point.requests).sum::<u64>(),
            1,
            "启动前的秒必须是无桶三字段（不得折叠到 slice 0）"
        );

        // 旧→新：第 0 点 = `cur - (len - 1)`，末点 = `cur`
        let order = Window::new();
        order.record_request(95);
        let points = order.series_at(99, 5);
        assert_eq!(points.first().copied().expect("最旧").requests, 1);
        assert_eq!(points.last().copied().expect("最新").requests, 0);
    }

    /// `dropped_samples()` 的两条触发面 + 正常回收（D1：有界丢弃、计数可见、绝不静默）。
    #[test]
    fn window_counts_bounded_discards() {
        let window = Window::new();
        assert_eq!(window.dropped_samples(), 0);

        // ① `CLEARING` 期间（有写者正在清零）⇒ 本样本丢弃
        plant(&window, 7, CLEARING, 0, 0);
        window.record_request(7);
        assert_eq!(
            window.dropped_samples(),
            1,
            "CLEARING 期间的样本必须计入丢弃数"
        );
        assert_eq!(window.snapshot_at(7).requests, 0, "被丢弃的样本不得进桶");

        // ② 回退戳（`slice < observed`：防 ABA 把新秒的数据清掉）⇒ 丢弃
        plant(&window, 80, 200, 0, 0); // 槽 80 蹲着 slice 200
        window.record_request(80); // 80 < 200 ⇒ 丢弃
        assert_eq!(window.dropped_samples(), 2);
        assert_eq!(window.snapshot_at(200).requests, 0);

        // ③ 正常认领（`slice > observed`）⇒ 回收旧桶、零新增丢弃
        window.record_request(320); // 槽 80：320 > 200 ⇒ CAS 回收
        assert_eq!(window.dropped_samples(), 2);
        assert_eq!(window.snapshot_at(320).requests, 1);
    }

    /// 公共同钟路径（`slice_now()` / `snapshot()` / `series()`）口径自洽；且 `Window` 挂在 `Stats` 上
    /// （`Stats::window()`）与**累计口径并存**（互不干扰）。
    #[test]
    fn window_snapshot_matches_recorded_samples() {
        let stats = Stats::new();
        let window = stats.window();
        let slice = window.slice_now();
        window.record_request(slice);
        window.record_preflight(slice);
        window.record_error(slice);
        window.record_latency_ms(slice, 12);

        let snapshot = window.snapshot();
        assert_eq!(snapshot.secs, WINDOW_SECS);
        assert_eq!(snapshot.requests, 1);
        assert_eq!(snapshot.preflight, 1);
        assert_eq!(snapshot.errors, 1);
        assert_eq!(snapshot.latency_samples, 1);
        assert_eq!(snapshot.latency_p50.as_deref(), Some("25ms"));
        assert_eq!(snapshot.latency_p95.as_deref(), Some("25ms"));
        assert_eq!(snapshot.dropped_samples, 0);
        assert!(!snapshot.is_idle());
        assert_eq!(snapshot.latency_text(), "p50 25ms / p95 25ms（近 120 s）");

        let points = window.series(WINDOW_SECS);
        assert_eq!(points.len(), WINDOW_SECS as usize);
        assert_eq!(
            points.iter().map(|point| point.requests).sum::<u64>(),
            1,
            "刚记的 1 次请求必须落在序列里"
        );

        // 累计口径与之并存：`Stats::snapshot()` 仍是累计的唯一口径（窗口记录点不动它）
        stats.inc_total();
        assert_eq!(stats.snapshot().total, 1, "窗口记录点不改变累计计数器");
        assert_eq!(window.snapshot().preflight, 1);
    }

    /// 空窗文案（逐字；J-B2 / J-B3 依赖的同一真源）。
    #[test]
    fn empty_window_reports_no_samples() {
        let snapshot = Window::new().snapshot();
        assert!(snapshot.is_idle());
        assert_eq!(snapshot.latency_p50, None);
        assert_eq!(snapshot.latency_p95, None);
        assert_eq!(snapshot.latency_text(), "—（近 120 s 无样本）");
    }

    /// **I3-a 审查 P2-①**：把 J-B1 的 `== 3` 拆成两条**单戳**独立断言 —— "窗口外样本不计入"与
    /// "未来戳不计入"各自可判（组合断言在两条守卫里的任一条被改坏时可能**仍凑出 3**）。
    #[test]
    fn window_window_edge_and_future_stamp_are_independently_decidable() {
        // ① 只有 age 61（`stamp = 39`，`cur = 100`）：120 s 窗口内 ⇒ 必须计入
        let inside = Window::new();
        inside.record_request(39);
        assert_eq!(
            inside.snapshot_at(100).requests,
            1,
            "age 61 < WINDOW_SECS ⇒ 计入（只有真 120 s 窗口成立；60 s 窗口会红）"
        );

        // ② 只有未来戳（`stamp = 101 > cur = 100`）：必须不计入
        let future = Window::new();
        future.record_request(101);
        assert_eq!(
            future.snapshot_at(100).requests,
            0,
            "未来戳必须不计入（`stamp <= cur` 守卫）"
        );
        assert_eq!(
            future.snapshot_at(101).requests,
            1,
            "同一桶在 `cur = 101` 时是**本秒** ⇒ 计入（咬住：守卫只排除未来戳，不误杀本秒）"
        );
    }

    /// **I3-a 审查 P2-②（隔离用例）**：`CLEARING` 守卫与 `stamp <= cur` 在正常 `cur` 下**语义重叠**
    /// （`CLEARING = u64::MAX` 必然 > `cur`）⇒ 只去掉 `CLEARING` 分支时 J-B1 的 CLEARING 断言仍绿。
    /// 本用例取 `cur = u64::MAX`（此时 `stamp <= cur` **不再**能兜住 `CLEARING`）⇒ 唯一拦下它的是
    /// `CLEARING` 分支本身。**负例面**：删掉 `sample_inside_window` 里的 `stamp == CLEARING` 判断，
    /// 本断言立刻红（哨兵 999 会被计入）。
    #[test]
    fn clearing_bucket_is_skipped_even_when_cur_cannot_outrank_it() {
        let window = Window::new();
        plant(&window, 11, CLEARING, 999, 7);
        let snapshot = window.snapshot_at(u64::MAX);
        assert_eq!(
            snapshot.requests, 0,
            "`CLEARING`（正在清零）的桶必须跳过 —— `cur = u64::MAX` 时没有第二条守卫能兜它"
        );
        assert_eq!(snapshot.latency_samples, 0);
        assert_eq!(snapshot.latency_p50, None);
    }

    /// **并发 smoke（I3-a 审查 §9-② 的交接建议）**：多线程并发 `record_*` + 主线程
    /// `snapshot()` / `series()` —— 无 panic、读数不倒退、逐秒和不超过总记录数。
    ///
    /// **关键安全性质**（本测试不证明、但口径依赖它，见接线报告的"并发性质声明"）：
    /// 读侧有 `stamp != slice` 守卫 + 写侧**先 CAS 到 `CLEARING`** ⇒ 读侧**不会**把"正在清空"的桶
    /// 当成"本秒的桶"（看到 `CLEARING` 即整点跳过 ⇒ 至多丢掉本秒的一小段，不会把上一圈的计数读成本秒）。
    /// 不追求压测：这里只咬"不崩、不倒退、不超界"（丢弃量由 `dropped_samples()` 可见 ⇒ 用 `≤/≥`）。
    #[test]
    fn window_survives_concurrent_writers_and_readers() {
        const WORKERS: u64 = 4;
        const ROUNDS: u64 = 2_000;
        let stats = Arc::new(Stats::new());
        let mut handles = Vec::new();
        for worker in 0..WORKERS {
            let stats = Arc::clone(&stats);
            handles.push(std::thread::spawn(move || {
                for index in 0..ROUNDS {
                    // 真实时钟（跨秒边界也会发生）⇒ 走的是与生产同一条 `touch` 路径
                    let slice = stats.window().slice_now();
                    stats.window().record_request(slice);
                    stats.window().record_preflight(slice);
                    stats.window().record_error(slice);
                    stats
                        .window()
                        .record_latency_ms(slice, 12 + (index + worker) % 40);
                }
            }));
        }
        // 主线程：与写者并发读（撕裂快照是允许的；只要求不崩 + 单调 + 不超界）
        let mut previous = 0;
        for _ in 0..200 {
            let snapshot = stats.window().snapshot();
            assert!(
                snapshot.requests >= previous,
                "近窗读数不得倒退（同一次采样间单调）：{previous} → {}",
                snapshot.requests
            );
            assert!(snapshot.requests <= WORKERS * ROUNDS);
            previous = snapshot.requests;
            let points = stats.window().series(WINDOW_SECS);
            assert_eq!(points.len(), WINDOW_SECS as usize);
            assert!(points.iter().map(|point| point.requests).sum::<u64>() <= WORKERS * ROUNDS);
        }
        for handle in handles {
            handle.join().expect("写线程不得 panic");
        }

        let snapshot = stats.window().snapshot();
        let recorded = WORKERS * ROUNDS;
        assert!(
            snapshot.requests + snapshot.dropped_samples >= recorded,
            "每个样本要么入桶要么计入丢弃（{} + {} < {recorded}）",
            snapshot.requests,
            snapshot.dropped_samples
        );
        assert!(snapshot.requests <= recorded);
    }
}
