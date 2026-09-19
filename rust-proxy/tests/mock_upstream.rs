//! 测试用 **mock 上游**（方案 §10.2 的装置；由 `tests/proxy_it.rs` 以 `mod mock_upstream;` 引入）。
//!
//! ⚠ `#![allow(dead_code)]` 的由来：本文件被 `proxy_it.rs` 当模块引入的**同时**，
//! Cargo 还会把它当成一个独立的测试 target 编译（`tests/*.rs` 都会被编译）——
//! 那种形态下没有任何调用方，dead_code 会刷屏，且 `clippy --all-targets -- -D warnings` 会因此**红**。
//!
//! 设计要点：
//! - 只监听 `127.0.0.1:0`（回环 + OS 选端口）⇒ 无端口冲突、可多实例并存（§10.2「本机回环」）；
//! - **一条 catch-all 按路径分派**，行为由路径 + 查询串决定（一个上游服务全部用例）；
//! - 三个观测面：请求记录（头/路径/查询/body 字节数）、**接受的 TCP 连接数**（I9 连接复用）、
//!   **被提前 drop 的响应流数**（I6 客户端中止 ⇒ 代理 drop 掉上游流）；
//! - SSE 帧由自实现的 `Stream` 产出（帧间隔可控），`Drop` 里记"未读完就被丢" —— 这就是 I6 的观测点。
#![allow(dead_code)]

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;
use futures_core::Stream;
use tokio::net::{TcpListener, TcpStream};

/// 一条上游收到的请求的观测记录。
#[derive(Clone, Debug)]
pub struct Record {
    pub method: String,
    pub path: String,
    pub query: String,
    pub host: String,
    pub content_type: String,
    pub authorization: String,
    pub api_key: String,
    /// P6-S5 / P1-B：Cookie 族也必须"原样透传"（脱敏只发生在日志侧）。
    pub cookie: String,
    pub origin: Option<String>,
    pub referer: Option<String>,
    pub accept_encoding: Option<String>,
    pub body_bytes: u64,
    pub test_id: String,
    pub at: Instant,
}

#[derive(Default)]
pub struct MockState {
    records: Mutex<Vec<Record>>,
    connections: AtomicU64,
    aborted_streams: AtomicU64,
    frames_sent: AtomicU64,
    /// 本修复批（I15/I17/I20）：**静默模式** —— `/hold` 收下请求但不回响应头、也不关连接。
    silent: std::sync::atomic::AtomicBool,
    /// 被静默扣住的请求数（= "入站请求未被处理"的读数；挂死复现的"静默确证"）。
    silent_hits: AtomicU64,
}

impl MockState {
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Record>> {
        match self.records.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn push(&self, record: Record) {
        self.lock().push(record);
    }

    pub fn records(&self) -> Vec<Record> {
        self.lock().clone()
    }

    pub fn count(&self) -> usize {
        self.lock().len()
    }

    pub fn last(&self) -> Option<Record> {
        self.lock().last().cloned()
    }

    /// I9：本 mock 接受的 TCP 连接总数（连接池生效 ⇒ 远小于请求数）。
    pub fn connections(&self) -> u64 {
        self.connections.load(Ordering::SeqCst)
    }

    /// I6：未读完就被 drop 的响应流数（客户端中止 ⇒ 代理取消了上游请求）。
    pub fn aborted_streams(&self) -> u64 {
        self.aborted_streams.load(Ordering::SeqCst)
    }

    pub fn frames_sent(&self) -> u64 {
        self.frames_sent.load(Ordering::SeqCst)
    }

    /// 本修复批：切换"静默"（`/hold` 的行为开关）。
    pub fn set_silent(&self, value: bool) {
        self.silent.store(value, Ordering::SeqCst);
    }

    pub fn silent(&self) -> bool {
        self.silent.load(Ordering::SeqCst)
    }

    /// 被静默扣住的请求数（I15/I17/I20 的"静默确证"读数）。
    pub fn silent_hits(&self) -> u64 {
        self.silent_hits.load(Ordering::SeqCst)
    }
}

/// 计数用的监听器（I9）：每个被 accept 的连接 +1，再交给 axum。
struct CountingListener {
    inner: TcpListener,
    state: Arc<MockState>,
}

impl axum::serve::Listener for CountingListener {
    type Io = TcpStream;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (TcpStream, SocketAddr) {
        // 契约（axum 0.8 `Listener`）：accept 出错要**自己重试**，不得把错误抛出去
        loop {
            match self.inner.accept().await {
                Ok((stream, addr)) => {
                    self.state.connections.fetch_add(1, Ordering::SeqCst);
                    return (stream, addr);
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(5)).await,
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.inner.local_addr()
    }
}

pub struct MockUpstream {
    pub base: String,
    pub state: Arc<MockState>,
}

impl MockUpstream {
    pub async fn start() -> MockUpstream {
        let state = Arc::new(MockState::default());
        let app = Router::new()
            .fallback(handle)
            .with_state(Arc::clone(&state));
        let inner = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock 上游必须能绑定回环端口");
        let addr = inner.local_addr().expect("mock 上游必须有本地地址");
        let listener = CountingListener {
            inner,
            state: Arc::clone(&state),
        };
        tokio::spawn(async move {
            // 测试结束时监听器随任务一起被丢弃；错误（如测试退出时的 accept 失败）无意义
            let _ = axum::serve(listener, app).await;
        });
        MockUpstream {
            base: format!("http://{addr}"),
            state,
        }
    }
}

// ── 分派 ────────────────────────────────────────────────────────────────────────

async fn handle(State(state): State<Arc<MockState>>, request: Request) -> Response {
    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    let query = request.uri().query().unwrap_or("").to_string();
    let params = parse_query(&query);
    let headers = request.headers().clone();
    let body = request.into_body();

    // 本修复批（I19）：`/sse_break` —— 发 2 帧后**以错误中断**（响应头已发之后的"流中断"形态）
    if path.starts_with("/sse_break") {
        state.push(record_of(&method, &path, &query, &headers, 0));
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from_stream(BreakStream {
                sent: 0,
                sleep: Box::pin(tokio::time::sleep(Duration::from_millis(50))),
            }))
            .expect("构造 /sse_break 响应");
    }

    // 本修复批（I15/I17/I20）：`/hold` —— 静默时**收下请求但不回响应头、也不关连接**（半开/黑洞形态）；
    // 非静默时按普通 JSON 应答（I18 的 GET 版要它能回一次正常响应）。
    if path.starts_with("/hold") {
        state.push(record_of(&method, &path, &query, &headers, 0));
        if state.silent() {
            state.silent_hits.fetch_add(1, Ordering::SeqCst);
            // 永不完成、也永不关闭：把"等响应头"无限期挂住（这正是被测相位）
            return std::future::pending::<Response>().await;
        }
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
            .body(Body::from("{\"hold\":true}"))
            .expect("构造 /hold 响应");
    }

    if path.starts_with("/sse") || path == "/never" {
        state.push(record_of(&method, &path, &query, &headers, 0));
        return if path == "/never" {
            // 只发响应头，**永不发帧**（首字节 watchdog 的负例：证明它非空转）
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from_stream(NeverStream))
                .expect("构造 /never 响应")
        } else {
            let frames = param_usize(&params, "frames", 5);
            let interval_ms = param_u64(&params, "interval_ms", 200);
            let first_ms = param_u64(&params, "first_ms", 0);
            let label = params
                .iter()
                .find(|(key, _)| key == "label")
                .map(|(_, value)| value.clone())
                .unwrap_or_else(|| "f".to_string());
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/event-stream")
                .header(header::CACHE_CONTROL, "no-cache")
                .body(Body::from_stream(SseStream::new(
                    Arc::clone(&state),
                    frames,
                    Duration::from_millis(interval_ms),
                    Duration::from_millis(first_ms),
                    label,
                )))
                .expect("构造 SSE 响应")
        };
    }

    if path == "/big" {
        let bytes = param_usize(&params, "bytes", 1024);
        let chunk = param_usize(&params, "chunk", 262_144);
        let delay_ms = param_u64(&params, "delay_ms", 0);
        state.push(record_of(&method, &path, &query, &headers, 0));
        // 分块 + 可配节流：既避免 mock 自己整包缓存，也让"首字节远早于总时长"成为**判据**而非巧合
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .body(Body::from_stream(PatternStream::new(
                bytes,
                chunk.max(1),
                Duration::from_millis(delay_ms),
            )))
            .expect("构造大响应");
    }

    if path == "/status" {
        let code = param_u64(&params, "code", 400) as u16;
        let body_bytes = drain(body, None).await;
        state.push(record_of(&method, &path, &query, &headers, body_bytes));
        return (
            StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST),
            [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
            format!(
                "{{\"error\":{{\"message\":\"mock 上游故意返回 {code}\",\"type\":\"mock\"}},\
                  \"detail\":\"detail-{code}-逐字节透传校验\"}}"
            ),
        )
            .into_response();
    }

    if path == "/redirect" {
        state.push(record_of(&method, &path, &query, &headers, 0));
        // 目标主机 = 测试进程里**没有监听**的端口 ⇒ 一旦代理跟随重定向，立刻会失败并被计数
        return Response::builder()
            .status(StatusCode::FOUND)
            .header(header::LOCATION, "http://127.0.0.1:1/third-party")
            .header(header::CONTENT_TYPE, "text/plain")
            .body(Body::from("mock redirect"))
            .expect("构造 302 响应");
    }

    if path == "/slow" {
        let ms = param_u64(&params, "ms", 500);
        let body_bytes = drain(body, None).await;
        state.push(record_of(&method, &path, &query, &headers, body_bytes));
        tokio::time::sleep(Duration::from_millis(ms)).await;
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/plain")
            .body(Body::from(format!("slow {ms} ms 完成")))
            .expect("构造慢响应");
    }

    // /sink?delay_ms=N：**边收边丢**（不整包缓存），并把收到的字节数回给客户端
    if path == "/sink" {
        let delay = param_u64(&params, "delay_ms", 0);
        let delay = if delay == 0 {
            None
        } else {
            Some(Duration::from_millis(delay))
        };
        let body_bytes = drain(body, delay).await;
        state.push(record_of(&method, &path, &query, &headers, body_bytes));
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(format!(
                "{{\"received_bytes\":{body_bytes},\"got_body\":{}}}",
                if body_bytes > 0 { "true" } else { "false" }
            )))
            .expect("构造 /sink 响应");
    }

    // 默认 = /echo：读掉整个请求体并回一份观测 JSON（I2/I4/I8 的主判据）
    let body_bytes = drain(body, None).await;
    let record = record_of(&method, &path, &query, &headers, body_bytes);
    let payload = format!(
        "{{\"method\":\"{}\",\"path\":\"{}\",\"query\":\"{}\",\"host\":\"{}\",\
          \"content_type\":\"{}\",\"has_authorization\":{},\"has_origin\":{},\"has_referer\":{},\
          \"has_accept_encoding\":{},\"has_api_key\":{},\"body_bytes\":{},\"test_id\":\"{}\",\
          \"count\":{}}}",
        record.method,
        record.path,
        record.query,
        record.host,
        record.content_type,
        !record.authorization.is_empty(),
        record.origin.is_some(),
        record.referer.is_some(),
        record.accept_encoding.is_some(),
        !record.api_key.is_empty(),
        record.body_bytes,
        record.test_id,
        state.count() + 1,
    );
    state.push(record);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .body(Body::from(payload))
        .expect("构造 echo 响应")
}

fn record_of(
    method: &str,
    path: &str,
    query: &str,
    headers: &axum::http::HeaderMap,
    body_bytes: u64,
) -> Record {
    let text = |name: header::HeaderName| -> Option<String> {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    };
    Record {
        method: method.to_string(),
        path: path.to_string(),
        query: query.to_string(),
        host: text(header::HOST).unwrap_or_default(),
        content_type: text(header::CONTENT_TYPE).unwrap_or_default(),
        authorization: text(header::AUTHORIZATION).unwrap_or_default(),
        api_key: text(header::HeaderName::from_static("x-api-key")).unwrap_or_default(),
        cookie: text(header::COOKIE).unwrap_or_default(),
        origin: text(header::ORIGIN),
        referer: text(header::REFERER),
        accept_encoding: text(header::ACCEPT_ENCODING),
        body_bytes,
        test_id: text(header::HeaderName::from_static("x-test-id")).unwrap_or_default(),
        at: Instant::now(),
    }
}

/// 逐 chunk 读掉请求体并计数（**不整包缓存** —— I11 的 100 MB 用例要靠它）。
/// 用 `poll_fn` + `Stream::poll_next` 而不是 `StreamExt::next`：直接依赖只有 `futures-core`。
async fn drain(body: Body, delay: Option<Duration>) -> u64 {
    let mut stream = body.into_data_stream();
    let mut total: u64 = 0;
    loop {
        let next = std::future::poll_fn(|cx| Pin::new(&mut stream).poll_next(cx)).await;
        match next {
            Some(Ok(chunk)) => {
                total += chunk.len() as u64;
                if let Some(delay) = delay {
                    tokio::time::sleep(delay).await;
                }
            }
            Some(Err(_)) => break,
            None => break,
        }
    }
    total
}

/// `/sse_break`：先发 2 帧、第 3 次 poll **返回 Err**（上游响应流中断；I19 的装置）。
struct BreakStream {
    sent: usize,
    sleep: Pin<Box<tokio::time::Sleep>>,
}

impl Stream for BreakStream {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.sent >= 2 {
            return Poll::Ready(Some(Err(std::io::Error::other(
                "mock: 上游响应流中断（I19）",
            ))));
        }
        if std::future::Future::poll(this.sleep.as_mut(), cx).is_pending() {
            return Poll::Pending;
        }
        this.sleep = Box::pin(tokio::time::sleep(Duration::from_millis(50)));
        let index = this.sent;
        this.sent += 1;
        Poll::Ready(Some(Ok(Bytes::from(format!(
            "data: break frame {index}

"
        )))))
    }
}

/// 永不产出帧、也永不结束（`/never`）。
struct NeverStream;

impl Stream for NeverStream {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Pending
    }
}

/// `/big` 的响应体：按 `chunk` 分块吐 `bytes` 字节的固定花纹（`i % 251`），块间可节流。
/// 花纹不是全零 ⇒ "抄成功"不能靠巧合；分块 ⇒ 既不用 mock 整包缓存，
/// 也让"首字节远早于总时长"成为结构性判据（I7 的"不整包缓存"证据）。
struct PatternStream {
    total: usize,
    offset: usize,
    chunk: usize,
    delay: Duration,
    sleep: Pin<Box<tokio::time::Sleep>>,
}

impl PatternStream {
    fn new(total: usize, chunk: usize, delay: Duration) -> PatternStream {
        PatternStream {
            total,
            offset: 0,
            chunk,
            delay,
            sleep: Box::pin(tokio::time::sleep(Duration::ZERO)),
        }
    }
}

impl Stream for PatternStream {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.offset >= this.total {
            return Poll::Ready(None);
        }
        if std::future::Future::poll(this.sleep.as_mut(), cx).is_pending() {
            return Poll::Pending;
        }
        let start = this.offset;
        let length = this.chunk.min(this.total - start);
        this.offset += length;
        this.sleep = Box::pin(tokio::time::sleep(this.delay));
        let payload: Vec<u8> = (start..start + length)
            .map(|index| (index % 251) as u8)
            .collect();
        Poll::Ready(Some(Ok(Bytes::from(payload))))
    }
}

/// 按固定间隔吐 SSE 帧的流；`Drop` 时若未读完 ⇒ 记一次"被对端提前丢弃"（I6 的观测点）。
struct SseStream {
    state: Arc<MockState>,
    frames: usize,
    interval: Duration,
    sent: usize,
    sleep: Pin<Box<tokio::time::Sleep>>,
    label: String,
    finished: bool,
}

impl SseStream {
    fn new(
        state: Arc<MockState>,
        frames: usize,
        interval: Duration,
        first_delay: Duration,
        label: String,
    ) -> SseStream {
        SseStream {
            state,
            frames,
            interval,
            sent: 0,
            sleep: Box::pin(tokio::time::sleep(first_delay)),
            label,
            finished: false,
        }
    }
}

impl Stream for SseStream {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.sent >= this.frames {
            this.finished = true;
            return Poll::Ready(None);
        }
        if std::future::Future::poll(this.sleep.as_mut(), cx).is_pending() {
            return Poll::Pending;
        }
        this.sleep = Box::pin(tokio::time::sleep(this.interval));
        let index = this.sent;
        this.sent += 1;
        this.state.frames_sent.fetch_add(1, Ordering::SeqCst);
        let payload = format!("data: {} frame {index}\n\n", this.label);
        Poll::Ready(Some(Ok(Bytes::from(payload))))
    }
}

impl Drop for SseStream {
    fn drop(&mut self) {
        if !self.finished {
            self.state.aborted_streams.fetch_add(1, Ordering::SeqCst);
        }
    }
}

// ── 小工具 ──────────────────────────────────────────────────────────────────────

fn parse_query(query: &str) -> Vec<(String, String)> {
    if query.is_empty() {
        return Vec::new();
    }
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((key, value)) => (key.to_string(), value.to_string()),
            None => (pair.to_string(), String::new()),
        })
        .collect()
}

fn param_value<'a>(params: &'a [(String, String)], key: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

fn param_usize(params: &[(String, String)], key: &str, fallback: usize) -> usize {
    param_value(params, key)
        .and_then(|value| value.parse().ok())
        .unwrap_or(fallback)
}

fn param_u64(params: &[(String, String)], key: &str, fallback: u64) -> u64 {
    param_value(params, key)
        .and_then(|value| value.parse().ok())
        .unwrap_or(fallback)
}
