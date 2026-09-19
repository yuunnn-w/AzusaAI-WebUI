//! 上游转发（方案 §6.2/§6.4/§6.5）：URL 拼接、请求头清洗、流式桥接、错误映射。
//!
//! ⚠ 本文件在「请求路径零 panic」硬规格作用域内（方案 §6.6 / §8.6 1a；
//! 由 `proxy/mod.rs` 的模块级内属性覆盖其子模块）。

use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::response::Response;
use futures_core::Stream;
use reqwest::Client;
use tokio::sync::oneshot;

use crate::config::{CorsConfig, TimeoutsConfig};
use crate::proxy::{cors, ActiveGuard, AppState};

/// FR-5/FR-6 + U1：`upstream_base`（可无尾斜杠）+ 原始 path（含查询串）直拼。
/// 不引入双斜杠、不吞查询串、不做二次解码（`%2F` 等转义原样保留）。
pub fn build_upstream_url(base: &str, path_and_query: &str) -> String {
    let base = base.trim_end_matches('/');
    if path_and_query.is_empty() {
        return format!("{base}/");
    }
    if path_and_query.starts_with('/') {
        format!("{base}{path_and_query}")
    } else {
        format!("{base}/{path_and_query}")
    }
}

/// FR-7：可选的「剥离监听侧路径前缀」。
/// - 只在**段边界**上剥（`/proxy` 剥得到 `/proxy/v1/x` 与 `/proxy`，剥不到 `/proxyfoo`）；
/// - 只作用于 path，查询串原样保留；
/// - 剥完为空 ⇒ 归一成 `/`（拼给上游的 URL 不许出现空路径拼出双斜杠）。
pub fn apply_strip_prefix(path_and_query: &str, strip_prefix: &str) -> String {
    if strip_prefix.is_empty() || strip_prefix == "/" {
        return path_and_query.to_string();
    }
    let (path, query) = match path_and_query.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (path_and_query, None),
    };
    let stripped = match path.strip_prefix(strip_prefix) {
        Some("") => "/",
        Some(rest) if rest.starts_with('/') => rest,
        _ => return path_and_query.to_string(),
    };
    match query {
        Some(query) => format!("{stripped}?{query}"),
        None => stripped.to_string(),
    }
}

/// 逐跳（hop-by-hop）头（RFC 7230 §6.1 一族；FR-15/FR-16 两侧都剥离）。
pub fn is_hop_by_hop(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "proxy-connection"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

/// FR-15 请求头清洗：hop-by-hop + `Host`（由 reqwest 按上游 URL 重写）+
/// 浏览器痕迹（`Origin`/`Referer`）+ `Accept-Encoding`（FR-17：不透传、不注入 ⇒ 上游不压缩）。
/// 业务头（`Content-Type` / `Authorization` / `x-api-key` / `anthropic-*` / `Mcp-*`…）**保留**。
pub fn strip_request_header(name: &HeaderName) -> bool {
    is_hop_by_hop(name)
        || matches!(
            name.as_str(),
            "host" | "origin" | "referer" | "accept-encoding"
        )
}

/// §5.1 客户端策略（FR-20/FR-21/FR-18；本修复批 L3/L5/D3/D4 落点）：
/// - **只允许** `connect_timeout`（+ 两个**分相位** watchdog：等待响应头 / 首字节）；**严禁**
///   `ClientBuilder::timeout()` —— 后者约束整个请求（含 body 读取），会掐死 SSE 长连接（与 FR-18 互斥，判据 §10.2 I3）；
/// - `redirect(Policy::none())` 如实透传 3xx（判据 §10.2 I12；也避免把 `Authorization` 带到第三方主机）；
/// - 连接池复用（FR-20；P6-S3 起 `max_idle_per_host` / `pool_idle_timeout` 由
///   `--pool-max-idle-per-host` / `--pool-idle-timeout-ms` 经 `Config::pool` 传入，默认 16 / 10 s）；
///   交互场景把"陈旧连接被复用"的窗口压到连续操作内；高负载（soak）连接空闲 <1 s ⇒ 复用率不受影响（I9 复跑为证）；
/// - **TCP keepalive 显式写死 = reqwest 默认同值 15 s / 15 s / 3 次（L5/D4）**：防上游默认漂移 + 文档可比对。
///   ⚠ 它只兜"对端主机/网络整块消失"型半开；对"对端活着但不回响应头"**无效**（对端栈会 ACK 探针）
///   —— 那种形态由 `response_head_ms`（L1）兜。
pub fn build_client(
    timeouts: &TimeoutsConfig,
    pool: &crate::config::PoolConfig,
) -> Result<Client, reqwest::Error> {
    let mut builder = Client::builder()
        .pool_max_idle_per_host(pool.max_idle_per_host)
        .pool_idle_timeout(Duration::from_millis(pool.idle_timeout_ms))
        .tcp_keepalive(Duration::from_secs(15))
        .tcp_keepalive_interval(Duration::from_secs(15))
        .tcp_keepalive_retries(3)
        .redirect(reqwest::redirect::Policy::none());
    // `connect_ms = 0` 在 `Config::validate` 被拒（不设连接超时 = 不可达上游永远挂住）
    if let Some(connect) = timeouts.connect() {
        builder = builder.connect_timeout(connect);
    }
    builder.build()
}

/// L1 命中的告警闸门名（与 FR-39 共用同一个 `WarnThrottle`，不同键 ⇒ 互不影响）。
const RESPONSE_HEAD_WATCHDOG_NOTICE: &str = "response-head-watchdog";
/// L1 告警节流窗口（方案 §2.6：同名 ≤ 1 条/60 s，防故障期刷屏）。
const RESPONSE_HEAD_WATCHDOG_INTERVAL: Duration = Duration::from_secs(60);

pub struct ForwardMeta {
    pub upstream_error: bool,
    pub timed_out: bool,
    /// FR-31「最近错误」的单行原因：代理合成错误 = 具体原因；上游真实 4xx/5xx = `上游返回 …`。
    /// 成功（< 400）⇒ `None`。
    pub error_reason: Option<String>,
}

/// 转发一次请求并桥接响应（**不缓冲**；状态码与响应体逐字节透传）。
///
/// 失败面的状态码口径（FR-22/FR-23，判据 §10.2 I5 + 本修复批 I15–I20）：
/// - **等待响应头 watchdog 命中（L1）** ⇒ **504** + `proxy_upstream_timeout` + 原因（含 URL/上限/已送达字节）；
/// - 上游**连接超时**类 ⇒ **504**；上游**不可达 / 连接被拒 / 其它** ⇒ **502**；
/// - 三者的体都是**带 CORS 头**的 JSON（否则应用只能看到 `Failed to fetch`）。
///
/// 本修复批新增的三层（方案 §2.1 L1/L2/L4）：
/// - **L1**：`response_head_ms` 给"等响应头"一个上界（计时起点 = 请求体送完；响应头一到计时器即 drop
///   ⇒ 物理上不约束响应体 ⇒ 长流/长思考不受影响，判据 §2.8 J2）；
/// - **L2**：命中 L1 / `send()` 传输错误（排除请求体/构造器类）/ 响应流中断 ⇒ `note_upstream_fault`
///   让连接池失效（`AppState::flush_client_pool`，2 s 节流）⇒ 陈旧连接不再被后续请求复用；
/// - **L4**：只对 `{GET,HEAD,OPTIONS} ∧ 无体 ∧ 未发字节` 做**恰一次**安全重试（走新池、新拨号）；
///   SSE/对话 = POST + 体 ⇒ **结构性永不重试**。
pub async fn forward(state: &Arc<AppState>, parts: Parts, body: Body) -> (Response, ForwardMeta) {
    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or("/");
    // FR-7：镜像到上游的路径（剥前缀只发生在这一处；查询串不受影响）
    let upstream_path = apply_strip_prefix(path_and_query, &state.config.strip_prefix);
    let url = build_upstream_url(state.config.upstream_base_trimmed(), &upstream_path);

    let mut forward_headers = HeaderMap::new();
    for (name, value) in parts.headers.iter() {
        if !strip_request_header(name) {
            forward_headers.append(name.clone(), value.clone());
        }
    }

    // FR-30「字节进出」的 ↑：请求体逐帧计数（不缓存整包；口径与响应侧对称）
    // L1 的"请求体送完"信号也挂在这里：体流结束/出错/被 drop ⇒ `oneshot` 送达 ⇒ watchdog 才起算。
    let seen = Arc::new(AtomicU64::new(0));
    let (body_done_tx, body_done_rx) = oneshot::channel::<()>();
    let counting_body = CountingBody {
        inner: body.into_data_stream(),
        stats: Arc::clone(&state.stats),
        seen: Arc::clone(&seen),
        done: Some(body_done_tx),
    };
    // L2：每次转发都从槽位取一次 client（命中重置后即拿到空池的新 client）
    let request = state
        .client()
        .request(parts.method.clone(), url.clone())
        .headers(forward_headers.clone())
        .body(reqwest::Body::wrap_stream(counting_body));

    let limit = state.config.timeouts.response_head();
    let first = attempt_send(request, Some(body_done_rx), limit).await;

    match first {
        Attempt::Sent(Ok(upstream)) => success_response(state, &parts, upstream, url),
        Attempt::Sent(Err(err)) => {
            // L2 触发面②：`send()` 传输错误 —— 谓词收窄：**排除**请求体类（客户端上传中断）
            // 与构造器类（URL 写错等本地问题），只有"上游侧"故障才清池（方案 §2.3/P2-2）。
            if !(err.is_body() || err.is_builder()) {
                state.note_upstream_fault("上游连接失败（send 传输错误）");
            }
            failure_response(state, &parts, &url, &err)
        }
        Attempt::HeadTimeout { limit } => {
            let body_bytes = seen.load(Ordering::Relaxed);
            // L2 触发面①：等待响应头超时（本批的核心：半开/静默上游）
            state.note_upstream_fault("等待响应头超时");
            log_response_head_watchdog(state, &url, limit, body_bytes);

            if retry_eligible(&parts.method, &parts.headers, body_bytes) {
                // L4：安全重试（走 L2 重置后的新 client ⇒ 新拨号；结构性不可能再落同一条陈旧连接）
                state.stats.inc_retry();
                state.logger.info(&format!(
                    "安全重试：{} {}（原因=等待响应头超时；白名单 = GET/HEAD/OPTIONS ∧ 无请求体 ∧ 未发出任何字节）",
                    parts.method, upstream_path
                ));
                let retry_request = state
                    .client()
                    .request(parts.method.clone(), url.clone())
                    .headers(forward_headers)
                    .body(reqwest::Body::from(Vec::new()));
                // ⚠ **重试腿不再 flush**（审查 P2-2，语义边界如实写死）：重试前 L1 刚命中过 ⇒ 池已被
                // 重置过（新 client = 空池）⇒ 重试腿再失败也**没有陈旧连接可留**，不 flush 无害。
                match attempt_send(retry_request, None, Some(limit)).await {
                    Attempt::Sent(Ok(upstream)) => success_response(state, &parts, upstream, url),
                    Attempt::Sent(Err(err)) => failure_response(state, &parts, &url, &err),
                    Attempt::HeadTimeout { limit } => {
                        head_timeout_response(state, &parts, &url, limit, body_bytes)
                    }
                }
            } else {
                head_timeout_response(state, &parts, &url, limit, body_bytes)
            }
        }
    }
}

/// 一次 `send()` 的结果（L1 把"等响应头超时"从"传输错误"里单独拎出来 —— 它没有错误信号）。
enum Attempt {
    Sent(Result<reqwest::Response, reqwest::Error>),
    /// 等待响应头超时命中（`limit` = 生效的上限；已送达请求体字节由调用方从 `seen` 读）。
    HeadTimeout {
        limit: Duration,
    },
}

/// 执行一次 `send()`，按需挂上 **L1 等待响应头 watchdog**。
///
/// ⚠ 计时器**只活在 `send()` 未返回期间**：`select!` 一旦由 `send` 臂胜出，watchdog 被 drop
/// ⇒ 它**物理上不可能**约束响应体（判据 §2.8 J2 / 与 FR-18 兼容的结构性保证）。
/// `select!` 取消 `send` = hyper 文档明示的取消方式（不留孤儿任务）。
async fn attempt_send(
    request: reqwest::RequestBuilder,
    body_done: Option<oneshot::Receiver<()>>,
    limit: Option<Duration>,
) -> Attempt {
    let send = request.send();
    match limit {
        None => Attempt::Sent(send.await), // 0 = 不限（显式关闭，供负例 I17）
        Some(limit) => {
            let watchdog = async move {
                // 请求体送完（或体流被提前 drop/出错 ⇒ 收到 Err）才开始计时 —— 大附件上传不计入本预算
                if let Some(rx) = body_done {
                    let _ = rx.await;
                }
                tokio::time::sleep(limit).await;
            };
            tokio::select! {
                biased;
                result = send => Attempt::Sent(result),
                () = watchdog => Attempt::HeadTimeout { limit },
            }
        }
    }
}

/// L1 命中时的告警（`WarnThrottle` 同名 ≤ 1 条/60 s，防故障期刷屏）。
fn log_response_head_watchdog(state: &AppState, url: &str, limit: Duration, body_bytes: u64) {
    if !state.notices.allow(
        RESPONSE_HEAD_WATCHDOG_NOTICE,
        RESPONSE_HEAD_WATCHDOG_INTERVAL,
    ) {
        return;
    }
    state.logger.warn(&format!(
        "上游 {url} 等待响应头 watchdog 命中（{} ms，已送达请求体 {body_bytes} B）：\
         放弃该连接并重置连接池（半开/静默上游；响应头未到 ⇒ 尚未发给客户端任何字节，故可干净地回 504）",
        limit.as_millis()
    ));
}

/// L4 重试白名单（写死，宁可少不可多）：`{GET,HEAD,OPTIONS}` ∧ 入站请求**无体** ∧ **一个字节都没发出**。
/// 两条都查，堵掉"声明有体但零字节"的极端形态；`Transfer-Encoding` 出现 ⇒ 视为可能有体。
pub fn retry_eligible(method: &Method, headers: &HeaderMap, seen: u64) -> bool {
    if seen != 0 {
        return false;
    }
    if !matches!(method.as_str(), "GET" | "HEAD" | "OPTIONS") {
        return false;
    }
    if headers.contains_key(header::TRANSFER_ENCODING) {
        return false;
    }
    match headers.get(header::CONTENT_LENGTH) {
        None => true,
        Some(value) => value
            .to_str()
            .ok()
            .and_then(|text| text.trim().parse::<u64>().ok())
            .map(|n| n == 0)
            .unwrap_or(false),
    }
}

/// 成功路径：原样透传状态码/头/体（含 FR-11 的 CORS 注入与 FR-30 的 4xx/5xx 归类）。
fn success_response(
    state: &Arc<AppState>,
    parts: &Parts,
    upstream: reqwest::Response,
    url: String,
) -> (Response, ForwardMeta) {
    let status = upstream.status();
    // 上游声明的长度（若为 chunked/SSE 则无此头）⇒ 判定"整包是否已送达完"，
    // 见 `UpstreamBody`：不这么判，客户端读满 Content-Length 后 hyper 不再 poll 到
    // `Ready(None)`，会把"正常读完"误记成客户端中止。
    let content_length = upstream
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    let mut headers = HeaderMap::new();
    for (name, value) in upstream.headers().iter() {
        if !is_hop_by_hop(name) {
            headers.append(name.clone(), value.clone());
        }
    }
    // FR-11：set/replace（含上游同名族的剥离）对**所有**响应生效
    cors::apply_cors_headers(&mut headers, &parts.headers, &state.config.cors);

    let bridge = UpstreamBody::new(
        Arc::clone(state),
        upstream.bytes_stream(),
        url,
        state.config.timeouts.first_byte(),
        content_length,
    );
    let mut response = Response::new(Body::from_stream(bridge));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    // FR-31：上游真实 4xx/5xx 的摘要（代理合成的 502/504 在错误分支里另行给原因）
    let error_reason = if status.as_u16() >= 400 {
        Some(format!("上游返回 {}", status.as_u16()))
    } else {
        None
    };
    (
        response,
        ForwardMeta {
            upstream_error: false,
            timed_out: false,
            error_reason,
        },
    )
}

/// 失败路径（`send()` 传输错误）：既有 502/504 映射 + CORS + 最近错误原因。
fn failure_response(
    state: &Arc<AppState>,
    parts: &Parts,
    url: &str,
    err: &reqwest::Error,
) -> (Response, ForwardMeta) {
    let timed_out = err.is_timeout();
    let (status, kind, message) = if timed_out {
        (
            StatusCode::GATEWAY_TIMEOUT,
            "proxy_upstream_timeout",
            format!("上游 {url} 超时（{err}）"),
        )
    } else {
        (
            StatusCode::BAD_GATEWAY,
            "proxy_upstream_unreachable",
            format!("上游 {url} 连接失败（{err}）"),
        )
    };
    // FR-25：这条日志里不含任何凭据（只有 URL 与 reqwest 的错误文本）
    state
        .logger
        .warn(&format!("转发失败：{status} {kind}（{message}）"));
    (
        proxy_error_response(status, &message, kind, &state.config.cors, &parts.headers),
        ForwardMeta {
            upstream_error: true,
            timed_out,
            error_reason: Some(if timed_out {
                "上游超时".to_string()
            } else {
                "上游连接失败".to_string()
            }),
        },
    )
}

/// L1 命中 ⇒ **504 + 带原因的 JSON + CORS**（FR-24；`meta.timed_out = true` ⇒ 走既有 `upstream_timeouts` 计数）。
fn head_timeout_response(
    state: &Arc<AppState>,
    parts: &Parts,
    url: &str,
    limit: Duration,
    body_bytes: u64,
) -> (Response, ForwardMeta) {
    let message = format!(
        "上游 {url} 等待响应头超时（{} ms，已送达请求体 {} B；已放弃该连接并重置连接池）",
        limit.as_millis(),
        body_bytes
    );
    (
        proxy_error_response(
            StatusCode::GATEWAY_TIMEOUT,
            &message,
            "proxy_upstream_timeout",
            &state.config.cors,
            &parts.headers,
        ),
        ForwardMeta {
            upstream_error: true,
            timed_out: true,
            error_reason: Some("等待上游响应头超时".to_string()),
        },
    )
}

/// FR-30 ↑（字节入）：请求体流包装 —— **只数不改**（逐帧透传，仍不缓冲）。
/// `bytes_in` 与响应侧的 `bytes_out` 对称：都按实际过手的 chunk 累计。
///
/// 本修复批（L1）另加两件事：
/// - `seen`：本请求**已交给上游的字节数**（L4 重试白名单要"一个字节都没发出"，L1 的 504 文案要它）；
/// - `done`：体流"结束/出错/被 drop"时向 L1 watchdog 发信号（计时起点 = 请求体送完；
///   提前 drop 视为"体相结束"——`let _ = rx.await` 不区分 Ok/Err）。
struct CountingBody {
    inner: axum::body::BodyDataStream,
    stats: Arc<crate::stats::Stats>,
    seen: Arc<AtomicU64>,
    done: Option<oneshot::Sender<()>>,
}

impl CountingBody {
    fn fire_done(&mut self) {
        if let Some(sender) = self.done.take() {
            let _ = sender.send(());
        }
    }
}

impl Stream for CountingBody {
    type Item = Result<Bytes, axum::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                this.stats.add_bytes_in(chunk.len() as u64);
                this.seen.fetch_add(chunk.len() as u64, Ordering::Relaxed);
                Poll::Ready(Some(Ok(chunk)))
            }
            Poll::Ready(Some(Err(err))) => {
                this.fire_done();
                Poll::Ready(Some(Err(err)))
            }
            Poll::Ready(None) => {
                this.fire_done();
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for CountingBody {
    fn drop(&mut self) {
        // hyper 可能按已知 `Content-Length` 写完后不再 poll 到 `None` ⇒ drop 也视为"体相结束"
        self.fire_done();
    }
}

/// 上游响应体桥（FR-18/FR-19/FR-21/FR-23 的落点；判据 §10.2 I3/I6/I7/I11）。
///
/// 四件事各归其位：
/// 1. **逐帧透传、不缓冲**：`poll_next` 把上游流的下一个 chunk 原样交出去，从不聚合整包
///    （`Body::from_stream` 消费本类型；故响应体既不进内存、也不受任何总超时约束）；
/// 2. **首字节 watchdog**（可配，默认关）：只在**首帧之前**计时；首帧一到 `sleep` 直接置空
///    ⇒ **流式总时长不受它约束**（§10.2 I3② 的结构性保证）。计时起点 = 上游响应头到达之后
///    （此刻状态码已发）⇒ 命中表现为**中断该流**（不伪造 `[DONE]`），而不是改写成 504；
/// 3. **客户端断开探测**（FR-19）：未读完就被 drop ⇒ `client_aborted` +1；上游连接随
///    reqwest 流一起 drop ⇒ 不留孤儿连接（`active` 也在同一时刻回落，FR-29）；
/// 4. **`active` 在途计数**：`ActiveGuard` 挂在本体上 ⇒ 流式请求的"活跃"一直计到 body 结束。
///
/// ⚠ 「读完」的判定有两条路（都必要）：① 上游流返回 `Ready(None)`（chunked/SSE 的正常收尾）；
/// ② 已送达字节数**达到上游 `Content-Length`** —— 客户端读满长度后 hyper 不会再多 poll 一次
/// 去取 `None`，只看 ① 会把"正常读完"误记成客户端中止（本批实测到的假阳性）。
struct UpstreamBody {
    inner: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    state: Arc<AppState>,
    _guard: ActiveGuard,
    watchdog: Option<Pin<Box<tokio::time::Sleep>>>,
    watchdog_limit: Option<Duration>,
    url: String,
    delivered: u64,
    expected: Option<u64>,
    first_chunk: bool,
    finished: bool,
}

impl UpstreamBody {
    fn new<S>(
        state: Arc<AppState>,
        stream: S,
        url: String,
        first_byte: Option<Duration>,
        expected: Option<u64>,
    ) -> UpstreamBody
    where
        S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
    {
        let guard = ActiveGuard::new(Arc::clone(&state));
        UpstreamBody {
            inner: Box::pin(stream),
            state,
            _guard: guard,
            watchdog: first_byte.map(|limit| Box::pin(tokio::time::sleep(limit))),
            watchdog_limit: first_byte,
            url,
            delivered: 0,
            expected,
            first_chunk: true,
            finished: false,
        }
    }
}

impl Stream for UpstreamBody {
    // 交出 `io::Error`：axum 的 `Body::from_stream` 只要求错误能进 `BoxError`。
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        // ① 首字节 watchdog：靠 `Sleep` 自己注册定时 waker（否则上游不发数据时永远醒不来）
        if this.watchdog.is_some() {
            let fired = match this.watchdog.as_mut() {
                Some(sleep) => std::future::Future::poll(sleep.as_mut(), cx).is_ready(),
                None => false,
            };
            if fired {
                this.watchdog = None;
                this.finished = true;
                this.state.stats.inc_stream_error();
                // L2 触发面（审查 P2-3 补齐）：首字节 watchdog 命中同属"上游异常相位"（相位在响应头之后）
                // ⇒ 该连接已不可信、弃池安全（flush 不杀在途请求；旧 client 的空闲连接随引用计数归零关闭）。
                this.state.note_upstream_fault("上游首字节 watchdog 命中");
                let limit = this.watchdog_limit.unwrap_or_default();
                this.state.logger.warn(&format!(
                    "上游 {} 首字节 watchdog 命中（{} ms，已送达 {} B）：中断该流（响应头已发出 ⇒ 不伪造 [DONE]，FR-23）",
                    this.url,
                    limit.as_millis(),
                    this.delivered
                ));
                return Poll::Ready(Some(Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "上游首字节超时",
                ))));
            }
        }

        // ② 上游流：逐帧透传
        match this.inner.as_mut().poll_next(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => {
                this.finished = true;
                if this.first_chunk {
                    this.state
                        .logger
                        .warn(&format!("上游 {} 响应体为空（0 帧）", this.url));
                }
                Poll::Ready(None)
            }
            Poll::Ready(Some(Ok(chunk))) => {
                this.first_chunk = false;
                // 首帧到达 ⇒ watchdog 彻底失效（§10.2 I3②）
                this.watchdog = None;
                this.watchdog_limit = None;
                this.delivered += chunk.len() as u64;
                if let Some(expected) = this.expected {
                    if this.delivered >= expected {
                        // 上游声明的长度已送完 ⇒ 整包完成（客户端不会再 poll 到 None）
                        this.finished = true;
                    }
                }
                this.state.stats.add_bytes_out(chunk.len() as u64);
                Poll::Ready(Some(Ok(chunk)))
            }
            Poll::Ready(Some(Err(err))) => {
                this.finished = true;
                this.state.stats.inc_stream_error();
                // L2 触发面③：响应流中断（事故当天真实出现过的信号）⇒ 该连接已不可信，弃池
                this.state.note_upstream_fault("响应流中断");
                this.state.logger.warn(&format!(
                    "上游 {} 响应流中断（已送达 {} B）：{err}（已发出的部分照常送达；不伪造 [DONE]，FR-23）",
                    this.url, this.delivered
                ));
                Poll::Ready(Some(Err(std::io::Error::other(err.to_string()))))
            }
        }
    }
}

impl Drop for UpstreamBody {
    fn drop(&mut self) {
        if !self.finished {
            // FR-19：客户端断开 / 停止生成 ⇒ 本 body 被丢 ⇒ 上游流一起被 drop（取消该请求）
            self.state.stats.inc_client_aborted();
            self.state.logger.debug(&format!(
                "{} 响应流未读完即结束（客户端断开 ⇒ drop 上游流取消该请求，FR-19；已送达 {} B）",
                self.url, self.delivered
            ));
        }
    }
}

/// FR-22/FR-24：代理自身产生的错误也必须是**带 CORS 头**的 JSON
/// （应用据此显示"带原因的 502"，而不是 `Failed to fetch`）。
pub fn proxy_error_response(
    status: StatusCode,
    message: &str,
    kind: &str,
    cors_config: &CorsConfig,
    request_headers: &HeaderMap,
) -> Response {
    let payload = serde_json::json!({
        "error": { "message": message, "type": kind }
    });
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    cors::apply_cors_headers(&mut headers, request_headers, cors_config);

    let mut response = Response::new(Body::from(payload.to_string()));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    response
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

    /// U1：路径拼接矩阵（方案 §10.1 U1）。
    #[test]
    fn u1_url_joining_matrix() {
        assert_eq!(
            build_upstream_url("http://host/gateway", "/v1/models?x=1"),
            "http://host/gateway/v1/models?x=1"
        );
        assert_eq!(
            build_upstream_url("http://host/gateway/", "/v1/models"),
            "http://host/gateway/v1/models"
        );
        assert_eq!(
            build_upstream_url("http://host/gateway/", "v1/models"),
            "http://host/gateway/v1/models"
        );
        assert_eq!(build_upstream_url("http://host", "/"), "http://host/");
        assert_eq!(build_upstream_url("http://host", ""), "http://host/");
        // 加入点不得出现双斜杠
        assert_eq!(
            build_upstream_url("http://host/gateway///", "/v1/x"),
            "http://host/gateway/v1/x"
        );
        // 查询串与转义原样保留（不做二次解码）
        assert_eq!(
            build_upstream_url("http://host/base", "/a/%2Fb?q=%20&r=%E4%B8%AD"),
            "http://host/base/a/%2Fb?q=%20&r=%E4%B8%AD"
        );
    }

    /// U2：头清洗策略（方案 §10.1 U2）。
    #[test]
    fn u2_request_header_policy() {
        let stripped = [
            "Connection",
            "Keep-Alive",
            "Proxy-Connection",
            "TE",
            "Trailer",
            "Transfer-Encoding",
            "Upgrade",
            "Host",
            "Origin",
            "Referer",
            "Accept-Encoding",
        ];
        let kept = [
            "Content-Type",
            "Authorization",
            "x-api-key",
            "Accept",
            "anthropic-version",
            "Mcp-Session-Id",
            "MCP-Protocol-Version",
            "Content-Length",
            "User-Agent",
        ];
        for name in stripped {
            let name = HeaderName::from_bytes(name.as_bytes()).unwrap();
            assert!(strip_request_header(&name), "{name} 必须剥离");
        }
        for name in kept {
            let name = HeaderName::from_bytes(name.as_bytes()).unwrap();
            assert!(!strip_request_header(&name), "{name} 必须保留");
        }
    }
}
