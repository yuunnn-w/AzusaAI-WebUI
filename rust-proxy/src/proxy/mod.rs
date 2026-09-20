#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
//! 反代内核：监听、路由（catch-all）、生命周期（方案 §9 第 5 步的落点；Phase 1 补 FR-19/FR-29/FR-39）。
//!
//! ⚠ 「请求路径零 panic」硬规格（方案 §6.6 / §8.6 1a）：本模块的内属性覆盖其子模块
//! （`forward.rs` / `cors.rs` / `notice.rs` 顶部各有一行说明）；`stats.rs` 顶部有同一组内属性。
//! Win7 配方强制 `panic = "abort"`（§5.4）⇒ 任何 panic = 进程立即退出、框架无法隔离。
//! 模块内单元测试如需 unwrap/索引，须在测试模块内 `#![allow(…)]` **显式豁免**（已示范于子模块）。

pub mod cors;
pub mod forward;
pub mod notice;

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{header, HeaderMap, Method, Request};
use axum::response::Response;
use axum::Router;

use crate::config::Config;
use crate::logging::{Level, Logger};
use crate::stats::Stats;

pub struct AppState {
    pub config: Config,
    /// 上游客户端。**可替换槽位**（本修复批 L2）：命中上游故障信号时整只换掉
    /// （旧 client 一丢，其空闲连接随引用计数归零被关闭 ⇒ 陈旧连接不再被复用）。
    /// ⚠ **必须是 `std::sync::Mutex`**：`note_upstream_fault` 会从 `UpstreamBody::poll_next`
    /// （**同步**上下文）被调用 —— 用 tokio 的异步锁会引入新的挂死面（本批要杀的正是挂死）。
    /// 锁内一律**不 await**（只做 Arc 克隆 / 指针替换）。
    client: std::sync::Mutex<reqwest::Client>,
    /// 上一次池重置的时刻（2 s 节流；与 `client` 分开两把锁，锁内同样不 await）。
    last_pool_flush: std::sync::Mutex<Option<Instant>>,
    /// Phase 2：`Arc` 共享 —— 服务启停/配置热切换时不丢累计数与最近错误（`src/service.rs`）。
    pub stats: Arc<Stats>,
    /// Phase 2：`Arc` 共享 —— 热改配置只更新级别/目录（`Logger::apply`），不重建对象。
    pub logger: Arc<Logger>,
    /// FR-39：误开提醒的去重闸门（同名告警 ≤ 1 条/小时）。同样跨热切换保留。
    pub notices: Arc<notice::WarnThrottle>,
}

/// L2 节流窗口（方案 §2.3/D6）：故障期最多每 2 s 重建一次池。
/// ⚠ 被节流跳过的 flush **不削弱修复**：首次重置后新 client 已是空池，窗口内仍失败的请求照常各自返回 504/502。
pub const POOL_FLUSH_MIN_INTERVAL: Duration = Duration::from_secs(2);

impl AppState {
    /// 构造（客户端策略见 `forward::build_client`）。
    pub fn new(
        config: Config,
        stats: Arc<Stats>,
        logger: Arc<Logger>,
        notices: Arc<notice::WarnThrottle>,
    ) -> Result<AppState, reqwest::Error> {
        let client = forward::build_client(&config.timeouts, &config.pool)?;
        Ok(AppState {
            config,
            client: std::sync::Mutex::new(client),
            last_pool_flush: std::sync::Mutex::new(None),
            stats,
            logger,
            notices,
        })
    }

    /// 取当前 client（克隆后立即放锁；锁内不 await）。
    pub fn client(&self) -> reqwest::Client {
        let guard = match self.client.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.clone()
    }

    /// L2：**让整个连接池失效**（三条触发面共用；节流 `POOL_FLUSH_MIN_INTERVAL`）。
    /// 返回"这次是否真的换了 client"（被节流或者 `build` 失败 ⇒ `false`）。
    pub fn flush_client_pool(&self, reason: &str) -> bool {
        let now = Instant::now();
        {
            let last = match self.last_pool_flush.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            if let Some(prev) = *last {
                if now.duration_since(prev) < POOL_FLUSH_MIN_INTERVAL {
                    return false;
                }
            }
            // ⚠ 时间戳**只在 build 成功之后**才推进（审查 P2-4）：build 失败时不应"吃掉"这一轮窗口，
            // 否则紧接着的一次故障信号会被无谓节流掉。见下方成功分支里的 `*last = Some(now)`。
        }
        match forward::build_client(&self.config.timeouts, &self.config.pool) {
            Ok(new_client) => {
                let mut slot = match self.client.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                // 旧 client 的最后一个引用在这里消失 ⇒ 其空闲连接被关闭（在途请求仍可读完）
                *slot = new_client;
                // 节流时间戳在**成功之后**落账（P2-4）
                {
                    let mut last = match self.last_pool_flush.lock() {
                        Ok(guard) => guard,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    *last = Some(now);
                }
                self.stats.inc_pool_flush();
                self.logger.debug(&format!(
                    "连接池已重置（原因={reason}）：新请求一律走空池新 client ⇒ 陈旧连接不再被复用"
                ));
                true
            }
            Err(err) => {
                // 本方案无 TLS ⇒ 实际不会发生；失败时保留旧 client，请求照常返回 504/502
                self.logger.error(&format!(
                    "连接池重置失败（原因={reason}）：{err} ⇒ 保留旧 client"
                ));
                false
            }
        }
    }

    /// 上游故障信号的**统一出口**（L2 三触发面共用；同步、不 await）。
    pub fn note_upstream_fault(&self, reason: &str) {
        if !self.flush_client_pool(reason) {
            self.logger.debug(&format!(
                "上游故障信号（{reason}）⇒ 池重置被 {POOL_FLUSH_MIN_INTERVAL:?} 节流跳过（新 client 已是空池，陈旧连接不会被复用）"
            ));
        }
    }
}

pub type SharedState = Arc<AppState>;

/// FR-29「当前活跃连接数」/ FR-34「在途请求收尾」的守卫：构造即 +1，drop 即 −1。
///
/// **建点 = `handle_request` 的第一句**（B-5 前移；原建点在 `forward::UpstreamBody::new` ⇒
/// 「等待上游响应头」期间 `活跃 = 0` 是**假 0**）。整条请求路径**移交同一个 guard**：
/// `handle_request` → `forward::forward` → `forward::success_response` → `forward::UpstreamBody`，
/// 后者把它放进响应体 ⇒ 流式请求的"活跃"一直计到 body 结束（客户端断开也走 drop ⇒ 计数不泄漏）；
/// 失败/预检路径让它随 handler（或 `forward`）返回时 drop。
pub struct ActiveGuard {
    state: Arc<AppState>,
}

impl ActiveGuard {
    pub fn new(state: Arc<AppState>) -> ActiveGuard {
        state.stats.inc_active();
        ActiveGuard { state }
    }
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.state.stats.dec_active();
    }
}

/// 路由 = catch-all 转发（FR-6：通用路径，不白名单）。
/// `DefaultBodyLimit::disable()`：axum 默认上限只有 2 MB，而应用单附件上限 50 MB
/// 以 base64 进请求体 ⇒ 单附件即可产生 ≈67 MB 的 JSON 请求体（FR-14 的硬规格，
/// 处理器另取 `Request<Body>` 直通、不整包缓存；判据 §10.2 I11）。
pub fn router(state: SharedState) -> Router {
    Router::new()
        .fallback(handle_request)
        .layer(DefaultBodyLimit::disable())
        .with_state(state)
}

async fn handle_request(State(state): State<SharedState>, request: Request<Body>) -> Response {
    let started = Instant::now();
    // B-5：`活跃` 从**请求进入**就计（含"等待上游响应头"与本地预检）—— 原建点在
    // `forward::UpstreamBody::new` ⇒ 等待响应头期间 `活跃 = 0`（§0.2 A2 的"在途=0 与 请求=1 并存"）。
    let guard = ActiveGuard::new(Arc::clone(&state));
    let (parts, body) = request.into_parts();
    // D1 / B-3：窗口秒号**每请求只算一次**后向下传（避免每个记录点各读一次时钟）。
    let slice = state.stats.window().slice_now();
    state.stats.inc_total();
    state.stats.window().record_request(slice);

    let method = parts.method.clone();
    let path = parts
        .uri
        .path_and_query()
        .map(|value| value.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());

    // FR-39：入站 text/plain ⇒ warn（同名 ≤ 1 条/小时）；**只读判定，绝不改包**（N2）
    notice_plain_content_type(&state, &parts.headers, &path);

    if state.logger.enabled(Level::Debug) {
        log_headers(&state, "请求", &parts.headers);
    }

    if cors::is_preflight(&parts.method, &parts.headers) {
        // 本地预检应答：body 极小且已全量在内存 ⇒ 守卫（顶层那个）随本函数返回即 drop
        // ⚠ **不得**在此新建 `ActiveGuard`（J-B6：否则预检 `活跃 = 2`）
        state.stats.inc_preflight();
        state.stats.window().record_preflight(slice);
        let response = cors::preflight_response(&parts.headers, &state.config.cors);
        log_access(&state, &method, &path, 204, started, true, None);
        return response;
    }

    let (response, meta) = forward::forward(&state, parts, body, guard).await;

    // FR-30：上游延迟（请求 → 上游响应头到达）—— 成功与失败都记（失败也含真实等待时长）。
    let latency_ms = started.elapsed().as_millis() as u64;
    state.stats.record_upstream_latency_ms(latency_ms);
    // D1 / B-3：同一时刻落近窗桶（同一 `slice`，不重复读时钟）
    state.stats.window().record_latency_ms(slice, latency_ms);

    let status = response.status().as_u16();
    if meta.timed_out {
        state.stats.inc_upstream_timeout();
    } else if meta.upstream_error {
        state.stats.inc_upstream_error();
    } else {
        state.stats.inc_forwarded();
        // FR-30：上游**真实** 4xx/5xx（代理合成的 502/504 已计入上面的错误族，不重复归类）
        if status >= 500 {
            state.stats.inc_upstream_5xx();
        } else if status >= 400 {
            state.stats.inc_upstream_4xx();
        }
    }
    // FR-31：最近错误（合成错误 = 具体原因；上游 4xx/5xx = "上游返回 …"）
    if status >= 400 {
        // D1 / B-3：近窗错误桶（与 `record_error(&summary)` 同一条件 ⇒ `status >= 400`）
        state.stats.window().record_error(slice);
        let summary = match &meta.error_reason {
            Some(reason) => format!("HTTP {status} {reason}"),
            None => format!("HTTP {status}"),
        };
        state.stats.record_error(&summary);
    }
    if state.logger.enabled(Level::Debug) {
        log_headers(&state, "响应", response.headers());
    }
    log_access(
        &state,
        &method,
        &path,
        status,
        started,
        false,
        Some(response.headers()),
    );
    response
}

/// FR-39（误开提醒）：入站 `Content-Type` 为 `text/plain` ⇒ 记一条 `warn`，
/// **同名告警每小时 ≤ 1 条**（`notice::WarnThrottle`）。
///
/// ⚠ 两条写死的边界：① **绝不修改请求头/请求体**（本函数只读）；② 文案必须区分
/// "自检探针（预期）"与"真实对话（问题）" —— 应用「运行自检」的 P3 探针**恒发** `text/plain`
/// （方案 §3.3）⇒ 本告警会**周期出现**，模糊文案对目标用户（不看日志）等于误导。
fn notice_plain_content_type(state: &AppState, headers: &HeaderMap, path: &str) {
    let content_type = match headers.get(header::CONTENT_TYPE) {
        Some(value) => value.to_str().unwrap_or(""),
        None => return,
    };
    if !notice::is_plain_text(content_type) {
        return;
    }
    if !state
        .notices
        .allow(notice::PLAIN_CONTENT_TYPE, notice::NOTICE_INTERVAL)
    {
        return;
    }
    state.logger.warn(&format!(
        "检测到入站 Content-Type: text/plain（{path}）—— 若这是应用「运行自检」的诊断探针属预期；\
         若真实对话也如此，请关闭应用的『预检规避兼容模式』（FR-39：每小时最多 1 条，请求未被修改）"
    ));
}

/// FR-26：每请求一行（时间戳由日志器加；含 方法/路径/状态/上游耗时/是否本地预检/响应字节）。
///
/// **P1-A / P2-F（S9）**：级别早退是本函数的**第一句** ⇒ `Content-Length` 取值与
/// `stats.snapshot()` 都落在 guard 之后。`--log-level error` 时每个请求**不再**白付
/// 15 次原子 load（`stats.rs::snapshot`）+ 2 次 `format!` 分配（`Snapshot::render` 与访问行本身）
/// —— 2 h soak 实测 29,890 请求即 29,890 次白付（§8-P1-A）。
/// 环满只影响**内容**，`enabled()` 早退发生在**级别**判定之后、进环之前，两者不冲突
/// ⇒ 计数（`ring_dropped`）不丢。
///
/// 两处调用点（预检 / 正常分支）**合并到本函数**（P2-F），响应字节以
/// `headers: Option<&HeaderMap>` 区分（`None` = 本地预检 ⇒ 固定 `0`，与改动前逐字一致）。
fn log_access(
    state: &AppState,
    method: &Method,
    path: &str,
    status: u16,
    started: Instant,
    preflight: bool,
    headers: Option<&HeaderMap>,
) {
    if !state.logger.enabled(Level::Info) {
        return;
    }
    let size = match headers {
        Some(headers) => headers
            .get(header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("-"),
        None => "0",
    };
    state.logger.info(&format!(
        "{method} {path} -> {status} 耗时={}ms 预检={} 响应字节={size} [{}]",
        started.elapsed().as_millis(),
        if preflight { "是" } else { "否" },
        state.stats.snapshot().render(),
    ));
}

/// FR-28：`debug` 级别记录请求/响应头（仍经 FR-25 脱敏 + §8-P1-B 的**白名单结构保险**）。
fn log_headers(state: &AppState, direction: &str, headers: &HeaderMap) {
    for (name, value) in headers.iter() {
        let text = value.to_str().unwrap_or("<非 UTF-8>");
        state.logger.debug(&format!(
            "  {direction} {name}: {}",
            crate::logging::render_header_for_log(name.as_str(), text, value.as_bytes().len())
        ));
    }
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
    use crate::proxy::notice::WarnThrottle;

    fn state_with(level: Level) -> AppState {
        AppState::new(
            Config::default(),
            Arc::new(Stats::new()),
            Arc::new(Logger::new(level)),
            Arc::new(WarnThrottle::new()),
        )
        .expect("测试用 reqwest client 应可构建（本项目无 TLS、纯本地配置）")
    }

    fn headers_with_length(value: &'static str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_LENGTH, value.parse().expect("静态长度串"));
        headers
    }

    /// **P1-A 的结构判据**（确定性，非计时）：访问行的构造与入环都受 level 门控；
    /// 同一条 helper 覆盖预检（`None` ⇒ 响应字节 `0`）与正常分支（`Some` ⇒ 取 `Content-Length`）
    /// ⇒ P2-F 的"两处 log_access 合一"与改动前的可观测面**逐字一致**。
    #[test]
    fn p1a_access_line_is_built_only_when_info_enabled() {
        let quiet = state_with(Level::Error);
        log_access(
            &quiet,
            &Method::GET,
            "/v1/chat/completions",
            200,
            Instant::now(),
            false,
            None,
        );
        assert_eq!(
            quiet.logger.retained_count(),
            0,
            "error 档不得构造（更不得入环）访问行 —— 这正是 P1-A 要省的那笔开销"
        );

        let loud = state_with(Level::Info);
        log_access(
            &loud,
            &Method::GET,
            "/v1/chat/completions",
            200,
            Instant::now(),
            true,
            None,
        );
        let headers = headers_with_length("42");
        log_access(
            &loud,
            &Method::POST,
            "/v1/models",
            502,
            Instant::now(),
            false,
            Some(&headers),
        );
        let (lines, dropped) = loud.logger.snapshot_lines(usize::MAX);
        assert_eq!(lines.len(), 2);
        assert_eq!(dropped, 0);
        assert!(
            lines[0].contains("GET /v1/chat/completions -> 200"),
            "{}",
            lines[0]
        );
        assert!(lines[0].contains("预检=是"), "{}", lines[0]);
        assert!(lines[0].contains("响应字节=0"), "{}", lines[0]);
        assert!(lines[1].contains("POST /v1/models -> 502"), "{}", lines[1]);
        assert!(lines[1].contains("预检=否"), "{}", lines[1]);
        assert!(lines[1].contains("响应字节=42"), "{}", lines[1]);
        assert!(
            lines[1].contains("累计 请求="),
            "访问行仍带统计快照（格式与改动前一致）：{}",
            lines[1]
        );
        assert_eq!(
            loud.stats.snapshot().total,
            0,
            "log_access 只读统计、不改任何计数"
        );
        // 无 Content-Length 的响应（SSE 等）⇒ 与改动前同样落 "-"
        let no_length = HeaderMap::new();
        log_access(
            &loud,
            &Method::GET,
            "/v1/stream",
            200,
            Instant::now(),
            false,
            Some(&no_length),
        );
        let (lines, _) = loud.logger.snapshot_lines(1);
        assert!(lines[0].contains("响应字节=-"), "{}", lines[0]);
    }

    /// **J-B7（热路径预算）**：同机两臂对照 —— 「接线前」（只有既有的 `inc_total()`）vs
    /// 「接线后」（`slice_now()` **每请求 1 次** + `record_request` + `record_latency_ms`）。
    ///
    /// 口径（B-3）：
    /// - 两臂都**不含** `ActiveGuard` —— 本批把它的建点从 `forward::UpstreamBody::new` 前移到
    ///   `handle_request` 首句，对**非预检**请求只是提前建，`inc_active`/`dec_active` 的次数不变；
    ///   预检请求新增的那一对由第三臂单独报数（`inc_active`+`dec_active` = 2 次原子 RMW）；
    /// - 「每请求算一次 `slice`」按生产写法落在臂内（不额外多读时钟）。
    ///
    /// 报数用（`cargo test -- --nocapture`）；断言只咬**结构性失控**（差值 < 1 µs/请求 —— 那意味着
    /// 意外引入了锁/分配），阈值判据（J-B7：≤5% 或 ≤100 ns）在 done.md 里按实测值判定。
    #[test]
    fn p7_i3b_hot_path_cost_reading() {
        const ITERS: usize = 200_000;
        const ROUNDS: usize = 5;
        let state = state_with(Level::Error);
        let window = state.stats.window();
        let mut before_ns = u128::MAX;
        let mut after_ns = u128::MAX;
        let mut guard_ns = u128::MAX;
        for _ in 0..ROUNDS {
            // 臂 A：接线前（每请求只做既有的累计计数）
            let started = Instant::now();
            for _ in 0..ITERS {
                state.stats.inc_total();
            }
            before_ns = before_ns.min(started.elapsed().as_nanos());

            // 臂 B：接线后（+ 1 次秒号 + 2 次触桶）
            let started = Instant::now();
            for _ in 0..ITERS {
                state.stats.inc_total();
                let slice = window.slice_now();
                window.record_request(slice);
                window.record_latency_ms(slice, 12);
            }
            after_ns = after_ns.min(started.elapsed().as_nanos());

            // 臂 C：预检新增面（在途计数一对；`ActiveGuard` 的构造与 drop 就这两下）
            let started = Instant::now();
            for _ in 0..ITERS {
                state.stats.inc_active();
                state.stats.dec_active();
            }
            guard_ns = guard_ns.min(started.elapsed().as_nanos());
        }
        let per = |ns: u128| ns as f64 / ITERS as f64;
        let (before, after, guard) = (per(before_ns), per(after_ns), per(guard_ns));
        println!(
            "J-B7 热路径读数（{ROUNDS} 轮取 min，每轮 {ITERS} 次）：接线前 {before:.1} ns/请求 · \
             接线后 {after:.1} ns/请求 · 差 {:.1} ns（{:.1}%）· 预检在途计数一对 {guard:.1} ns",
            after - before,
            if before > 0.0 {
                (after - before) / before * 100.0
            } else {
                f64::INFINITY
            }
        );
        assert!(
            after - before < 1_000.0,
            "接线后每请求不得多付 ≥1 µs（那意味着热路径意外引入了锁/分配）：{before:.1} → {after:.1} ns"
        );
    }

    /// **J-B6 的单测半边**：预检分支**不得**再新建 `ActiveGuard`（B-5 的删除项 ⑤）。
    /// 真机读数（`--no-gui` + `OPTIONS` 预检 ⇒ 访问行 `活跃=1`）另见 done.md；
    /// 这里钉住**结构**：`handle_request` 是本模块生产面里唯一的 `ActiveGuard::new` 调用点。
    #[test]
    fn p7_i3b_preflight_reuses_the_top_level_guard() {
        // 针串在**运行期**拼出：否则本测试自己的源码会把计数抬高（自咬）
        let needle = concat!("ActiveGuard", "::new(", "Arc::clone(&state))");
        let source = include_str!("mod.rs");
        let production = source
            .split_once("#[cfg(test)]")
            .map(|(head, _)| head)
            .unwrap_or(source);
        assert_eq!(
            production.matches(needle).count(),
            1,
            "生产面只允许一处 `{needle}`（`handle_request` 首句）—— 预检分支不得再建"
        );
    }

    /// **§8-P1-A 的定点读数**：1e6 量级下「仅级别早退」（改动后）vs
    /// 「完整构造访问行」（改动前每请求都走的形态）两臂计时。
    ///
    /// 报数用（`cargo test -- --nocapture` 可见）；断言取**极松**的形式
    /// （早退臂不慢于构造臂），避免机器噪声把门禁变成掷骰子 —— 读数本身写进 S9 报告。
    #[test]
    fn p1a_hot_path_cost_reading() {
        const ITERS: usize = 200_000;
        const ROUNDS: usize = 5;
        let quiet = state_with(Level::Error);
        let loud = state_with(Level::Info);
        let mut guarded_ns = u128::MAX;
        let mut eager_ns = u128::MAX;
        for _ in 0..ROUNDS {
            let started = Instant::now();
            for _ in 0..ITERS {
                log_access(
                    &quiet,
                    &Method::GET,
                    "/v1/chat/completions",
                    200,
                    started,
                    false,
                    None,
                );
            }
            guarded_ns = guarded_ns.min(started.elapsed().as_nanos());

            let started = Instant::now();
            for _ in 0..ITERS {
                let line = format!(
                    "GET /v1/chat/completions -> 200 耗时={}ms 预检={} 响应字节={} [{}]",
                    started.elapsed().as_millis(),
                    "否",
                    "-",
                    loud.stats.snapshot().render(),
                );
                std::hint::black_box(&line);
            }
            eager_ns = eager_ns.min(started.elapsed().as_nanos());
        }
        let guarded_per = guarded_ns as f64 / ITERS as f64;
        let eager_per = eager_ns as f64 / ITERS as f64;
        println!(
            "P1-A 定点读数（{} 轮取 min，每轮 {} 次）：早退臂 {:.1} ns/次 · 构造臂 {:.1} ns/次 · 省 {:.1} ns/次（{:.1}×）",
            ROUNDS,
            ITERS,
            guarded_per,
            eager_per,
            eager_per - guarded_per,
            if guarded_per > 0.0 {
                eager_per / guarded_per
            } else {
                f64::INFINITY
            }
        );
        assert!(
            guarded_ns <= eager_ns,
            "早退臂不应慢于构造臂（早退 {guarded_ns} ns vs 构造 {eager_ns} ns）"
        );
    }
}
