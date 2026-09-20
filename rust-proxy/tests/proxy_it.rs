//! 集成测试 **I1–I12**（方案 §10.2）+ FR-34 优雅停机（I13/I14，§10.2 无对应行，本批补）
//! + FR-25/FR-39 的行为判据。
//!
//! 装置：`tests/mock_upstream.rs`（本机回环 mock 上游）+ **进程内**起反代
//! （`azusa_local_proxy::server::serve`，与 exe 走同一套内核）。
//! 为什么进程内：I3② 要可控的"首字节 watchdog"、FR-34 要可控的"停机触发点" ——
//! 子进程 + 控制台事件在 Windows/Git Bash 下无法可靠注入（见 Phase 1 报告「未覆盖边界」）。
// 跨 `.await` 持有 `HEAVY` 串行闸门是**刻意的**（见下方 `static HEAVY`）：它是计时敏感用例的
// 进程内独占闸门，消除并发负载造成的计时假失败 ⇒ 显式豁免本 lint（NEEDS_FIX 修复轮 P0-1）。
#![allow(clippy::await_holding_lock)]

mod mock_upstream;

use std::sync::Arc;
use std::time::{Duration, Instant};

use azusa_local_proxy::config::{Config, TimeoutsConfig};
use azusa_local_proxy::logging::{Level, Logger};
use azusa_local_proxy::proxy::notice::WarnThrottle;
use azusa_local_proxy::proxy::AppState;
use azusa_local_proxy::server::{self, Shutdown, GRACE_SHUTDOWN};
use azusa_local_proxy::stats::Stats;
use mock_upstream::MockUpstream;

// ── 装置 ────────────────────────────────────────────────────────────────────────

/// 构造一个**只进内存环**的日志器（P6-S5 / D12：没有目录参数，也不落盘）。
/// `name` 只用于测试可读性，不再对应任何目录。
fn build_state(
    upstream_base: &str,
    timeouts: TimeoutsConfig,
    level: Level,
    name: &str,
) -> Arc<AppState> {
    let _ = name;
    let config = Config {
        upstream_base: upstream_base.to_string(),
        timeouts,
        ..Config::default()
    };
    config.validate().expect("测试配置必须合法");
    let logger = Logger::new(level);
    // 本修复批：构造统一走 `AppState::new`（client 变成可替换槽位；`build_client` 由它内部调）
    Arc::new(
        AppState::new(
            config,
            Arc::new(Stats::new()),
            Arc::new(logger),
            Arc::new(WarnThrottle::new()),
        )
        .expect("AppState 必须可建（含 reqwest Client）"),
    )
}

/// 重活 / 计时敏感用例的**进程内串行闸门**：`cargo test` 默认并行跑各用例，
/// 而 I3 的帧间隔判据、I7 的"首字节远早于总时长"、I8/I9 的万级请求、I11 的 100 MB、
/// I13/I14 的停机计时都要求独占机器（§7.2：同一台机器上验证套件别并行跑）。
/// 做法 = 谁起反代谁持有闸门（`ProxyHandle` 持有到 `stop()`），把"别并行"变成**代码级约束**，
/// 而不是"记得加 `--test-threads=1`"这种口头纪律 —— 换台机器也不会飘。
static HEAVY: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn heavy_guard() -> std::sync::MutexGuard<'static, ()> {
    match HEAVY.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

struct ProxyHandle {
    base: String,
    shutdown: Shutdown,
    state: Arc<AppState>,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
    /// 串行闸门（drop 顺序无所谓；持有到 `stop()` 即整段用例结束）
    _heavy: std::sync::MutexGuard<'static, ()>,
}

impl ProxyHandle {
    async fn start(upstream_base: &str, timeouts: TimeoutsConfig) -> ProxyHandle {
        ProxyHandle::start_named(upstream_base, timeouts, Level::Info, "default").await
    }

    async fn start_named(
        upstream_base: &str,
        timeouts: TimeoutsConfig,
        level: Level,
        name: &str,
    ) -> ProxyHandle {
        let state = build_state(upstream_base, timeouts, level, name);
        let heavy = heavy_guard();
        let listener = server::bind("127.0.0.1", 0)
            .await
            .expect("测试代理必须能绑定回环端口");
        let address = listener.local_addr().expect("监听地址必须可读");
        let shutdown = Shutdown::new();
        let task = tokio::spawn(server::serve(
            listener,
            Arc::clone(&state),
            shutdown.clone(),
        ));
        ProxyHandle {
            base: format!("http://{address}"),
            shutdown,
            state,
            task,
            _heavy: heavy,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// 触发停机并等"服务真的收尾完"，返回从触发到返回的耗时。
    async fn stop(self) -> Duration {
        let started = Instant::now();
        self.shutdown.trigger();
        let _ = tokio::time::timeout(Duration::from_secs(30), self.task).await;
        started.elapsed()
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .build()
        .expect("测试客户端必须可建")
}

/// I12 专用：测试客户端**自己也不跟随重定向** —— 否则它会把代理如实透传的 302 跟随掉，
/// 就看不到"代理没跟随"这件事了（proxy 侧的策略是 `Policy::none()`，客户端侧必须对齐口径）。
fn client_no_redirect() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("测试客户端必须可建")
}

fn header_text(response: &reqwest::Response, name: &str) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

fn json(response_body: &str) -> serde_json::Value {
    serde_json::from_str(response_body).expect("响应体必须是 JSON")
}

/// 轮询等待条件成立（上限 `limit`），返回是否成立。
async fn wait_until(limit: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + limit;
    loop {
        if condition() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// 读**内存快照**（P6-S5 起日志只在内存环里，替换原先"读 `logs/proxy-*.log`"的取证方式）。
/// 命名 = `log_snapshot`（P6-B3fix P3-2：旧名与实际行为不符 —— 它从不读任何文件）。
fn log_snapshot(handle: &ProxyHandle) -> String {
    let (lines, _) = handle.state.logger.snapshot_lines(usize::MAX);
    lines.join("\n")
}

/// 找一个"确定没人监听"的端口（绑定后立刻释放）。
async fn closed_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("必须能绑定回环端口");
    let port = listener.local_addr().expect("本地地址").port();
    drop(listener);
    port
}

// ── I1 ─────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn i1_preflight_answered_locally_with_full_cors() {
    let upstream = MockUpstream::start().await;
    let proxy = ProxyHandle::start(&upstream.base, TimeoutsConfig::default()).await;

    let response = client()
        .request(reqwest::Method::OPTIONS, proxy.url("/v1/models"))
        .header("Origin", "null")
        .header("Access-Control-Request-Method", "POST")
        .header(
            "Access-Control-Request-Headers",
            "content-type, authorization",
        )
        .send()
        .await
        .expect("预检请求必须发出");
    assert_eq!(response.status().as_u16(), 204, "预检必须本地 204");
    assert_eq!(
        header_text(&response, "access-control-allow-origin").as_deref(),
        Some("*")
    );
    assert_eq!(
        header_text(&response, "access-control-allow-methods").as_deref(),
        Some("GET, POST, OPTIONS")
    );
    assert_eq!(
        header_text(&response, "access-control-allow-headers").as_deref(),
        Some("content-type, authorization")
    );
    assert_eq!(
        header_text(&response, "access-control-max-age").as_deref(),
        Some("600")
    );
    drop(response);
    assert_eq!(upstream.state.count(), 0, "预检不得转发上游（FR-9）");
    assert_eq!(proxy.state.stats.snapshot().preflight, 1);

    // U8 负分支（P1-1）：裸 OPTIONS（缺 ACRM）⇒ 必须被如实转发，不吞
    let bare = client()
        .request(reqwest::Method::OPTIONS, proxy.url("/v1/models"))
        .header("Origin", "null")
        .send()
        .await
        .expect("裸 OPTIONS 必须发出");
    assert_eq!(bare.status().as_u16(), 200, "裸 OPTIONS 应被转发到 mock");
    drop(bare);
    assert_eq!(
        upstream.state.count(),
        1,
        "裸 OPTIONS 必须被转发（FR-9 的『否则按普通请求转发』分支）"
    );
    proxy.stop().await;
}

// ── I2 ─────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn i2_post_forwarding_fidelity() {
    let upstream = MockUpstream::start().await;
    let proxy = ProxyHandle::start(&upstream.base, TimeoutsConfig::default()).await;

    let body = "{\"model\":\"it\",\"messages\":[]}";
    let response = client()
        .post(proxy.url("/v1/chat/completions?trace=1"))
        .header("Content-Type", "application/json")
        .header("Authorization", "Bearer it-token-raw")
        .header("x-api-key", "sk-it-raw")
        .header("Origin", "http://127.0.0.1")
        .header("Referer", "http://127.0.0.1/index.html")
        .header("Accept-Encoding", "gzip, br")
        .body(body)
        .send()
        .await
        .expect("转发请求必须发出");
    assert_eq!(response.status().as_u16(), 200);
    let text = response.text().await.expect("读响应体");
    let payload = json(&text);
    assert_eq!(payload["method"], "POST");
    assert_eq!(payload["path"], "/v1/chat/completions");
    assert_eq!(payload["query"], "trace=1");
    assert_eq!(
        payload["content_type"], "application/json",
        "Content-Type 逐字节保留"
    );
    assert_eq!(payload["has_authorization"], true, "Authorization 原样透传");
    assert_eq!(payload["has_api_key"], true);
    assert_eq!(payload["has_origin"], false, "Origin 必须剥离（FR-15）");
    assert_eq!(payload["has_referer"], false, "Referer 必须剥离（FR-15）");
    assert_eq!(
        payload["has_accept_encoding"], false,
        "Accept-Encoding 不透传（FR-17）"
    );
    assert_eq!(payload["body_bytes"].as_u64(), Some(body.len() as u64));

    let record = upstream.state.last().expect("上游必须收到请求");
    assert_eq!(record.authorization, "Bearer it-token-raw");
    assert_eq!(record.api_key, "sk-it-raw");
    assert_eq!(
        record.host,
        upstream.base.trim_start_matches("http://"),
        "Host 重写为上游 authority"
    );
    assert_eq!(record.body_bytes, body.len() as u64);
    proxy.stop().await;
}

// ── I3 ─────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn i3_streaming_is_not_buffered_and_total_duration_is_unbounded() {
    // ① 5 帧 × 200 ms：逐帧到达、不得全堆在最后
    let upstream = MockUpstream::start().await;
    let proxy = ProxyHandle::start(&upstream.base, TimeoutsConfig::default()).await;
    let mut response = client()
        .get(proxy.url("/sse?frames=5&interval_ms=200&label=a"))
        .send()
        .await
        .expect("SSE 请求必须发出");
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(
        header_text(&response, "content-type").as_deref(),
        Some("text/event-stream")
    );

    let started = Instant::now();
    let mut arrivals: Vec<Duration> = Vec::new();
    while let Some(_chunk) = response.chunk().await.expect("读 SSE 帧") {
        arrivals.push(started.elapsed());
        if arrivals.len() >= 5 {
            break;
        }
    }
    assert_eq!(arrivals.len(), 5, "必须收到 5 帧（逐帧透传）");
    let first = arrivals.first().copied().unwrap_or_default();
    assert!(
        first < Duration::from_millis(400),
        "首帧到达 {first:?}，应 < 400 ms（§4.4 流式首帧判据）"
    );
    let gaps: Vec<Duration> = arrivals
        .windows(2)
        .map(|pair| pair[1].saturating_sub(pair[0]))
        .collect();
    for gap in &gaps {
        assert!(
            *gap >= Duration::from_millis(150) && *gap <= Duration::from_millis(400),
            "相邻帧间隔 {gap:?} 应在 [150, 400] ms 内（不得缓冲后一次吐出）"
        );
    }
    let total = arrivals.last().copied().unwrap_or_default();
    assert!(
        total >= Duration::from_millis(700) && total <= Duration::from_millis(1600),
        "5 帧总时长 {total:?} 应 ≈ 1 s"
    );
    drop(response);
    proxy.stop().await;

    // ② **流式总时长不受任何总超时约束**：watchdog 设 200 ms，首帧 50 ms、其后 3 s 长流
    let long = MockUpstream::start().await;
    let proxy = ProxyHandle::start(
        &long.base,
        TimeoutsConfig {
            connect_ms: 5_000,
            // 新键显式关掉：既有 I3②/I3③ 的语义（与"响应头之后"的相位无关）**一字不改**
            response_head_ms: 0,
            first_byte_ms: 200,
        },
    )
    .await;
    let mut response = client()
        .get(proxy.url("/sse?frames=12&interval_ms=300&first_ms=50&label=b"))
        .send()
        .await
        .expect("长流请求必须发出");
    assert_eq!(response.status().as_u16(), 200);
    let started = Instant::now();
    let mut frames = 0usize;
    let mut failure: Option<String> = None;
    loop {
        match response.chunk().await {
            Ok(Some(_)) => {
                frames += 1;
                if frames >= 12 {
                    break;
                }
            }
            Ok(None) => break,
            Err(err) => {
                failure = Some(err.to_string());
                break;
            }
        }
    }
    let elapsed = started.elapsed();
    assert!(
        failure.is_none(),
        "长流不得被首字节 watchdog 中断：{failure:?}"
    );
    assert_eq!(frames, 12, "12 帧必须全部到达（总时长 > watchdog 200 ms）");
    assert!(
        elapsed > Duration::from_secs(3),
        "12 帧 × 300 ms 实际耗时 {elapsed:?}，必须 > 3 s（证明总时长不受超时约束）"
    );
    assert_eq!(
        proxy.state.stats.snapshot().stream_errors,
        0,
        "不得出现流中断计数"
    );
    drop(response);
    proxy.stop().await;

    // ③ 负例：watchdog 非空转 —— 只发响应头、永不发帧 ⇒ 必须被中断（且不伪装成正常结束）
    let never = MockUpstream::start().await;
    let proxy = ProxyHandle::start(
        &never.base,
        TimeoutsConfig {
            connect_ms: 5_000,
            response_head_ms: 0,
            first_byte_ms: 200,
        },
    )
    .await;
    let mut response = client()
        .get(proxy.url("/never"))
        .send()
        .await
        .expect("请求必须发出");
    assert_eq!(
        response.status().as_u16(),
        200,
        "响应头已发出 ⇒ 状态码不能再改写"
    );
    let outcome = response.chunk().await;
    assert!(
        outcome.is_err(),
        "永不发帧的上游必须被 watchdog 中断（得到 {outcome:?}）"
    );
    drop(response);
    assert_eq!(
        proxy.state.stats.snapshot().stream_errors,
        1,
        "watchdog 命中必须计入流中断"
    );
    proxy.stop().await;
}

// ── I4 ─────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn i4_upstream_errors_pass_through_with_cors() {
    let upstream = MockUpstream::start().await;
    let proxy = ProxyHandle::start(&upstream.base, TimeoutsConfig::default()).await;

    for code in [400u16, 500] {
        let response = client()
            .post(proxy.url(&format!("/status?code={code}")))
            .header("Content-Type", "application/json")
            .body("{\"probe\":1}")
            .send()
            .await
            .expect("请求必须发出");
        assert_eq!(response.status().as_u16(), code, "状态码必须一致透传");
        assert_eq!(
            header_text(&response, "access-control-allow-origin").as_deref(),
            Some("*"),
            "4xx/5xx 同样必须带可用 ACAO（§6.3 FR-11）"
        );
        let text = response.text().await.expect("读响应体");
        let payload = json(&text);
        assert_eq!(payload["detail"], format!("detail-{code}-逐字节透传校验"));
        assert_eq!(payload["error"]["type"], "mock");
    }
    proxy.stop().await;
}

// ── I5（502）与 I5b（504；FR-23 的区分） ───────────────────────────────────────

#[tokio::test]
async fn i5_upstream_unreachable_is_502_with_reason() {
    let port = closed_port().await;
    let upstream_base = format!("http://127.0.0.1:{port}");
    let proxy = ProxyHandle::start(&upstream_base, TimeoutsConfig::default()).await;

    let response = client()
        .get(proxy.url("/v1/models"))
        .send()
        .await
        .expect("请求必须发出");
    assert_eq!(response.status().as_u16(), 502);
    assert_eq!(
        header_text(&response, "access-control-allow-origin").as_deref(),
        Some("*"),
        "502 也必须带 ACAO（否则应用只看到 Failed to fetch）"
    );
    let text = response.text().await.expect("读响应体");
    let payload = json(&text);
    assert_eq!(payload["error"]["type"], "proxy_upstream_unreachable");
    let message = payload["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(&upstream_base),
        "502 的 message 必须带上游地址：{message}"
    );
    assert_eq!(proxy.state.stats.snapshot().upstream_errors, 1);
    proxy.stop().await;
}

#[tokio::test]
async fn i5b_upstream_connect_timeout_is_504() {
    // 192.0.2.0/24 = RFC 5737 TEST-NET-1：实测**连接超时**（不回 RST），正好用来验 504 分支
    let proxy = ProxyHandle::start(
        "http://192.0.2.1:80",
        TimeoutsConfig {
            connect_ms: 300,
            response_head_ms: 0,
            first_byte_ms: 0,
        },
    )
    .await;

    let response = client()
        .get(proxy.url("/v1/models"))
        .send()
        .await
        .expect("请求必须发出");
    assert_eq!(
        response.status().as_u16(),
        504,
        "连接超时必须是 504（不是 502）"
    );
    assert_eq!(
        header_text(&response, "access-control-allow-origin").as_deref(),
        Some("*")
    );
    let text = response.text().await.expect("读响应体");
    let payload = json(&text);
    assert_eq!(payload["error"]["type"], "proxy_upstream_timeout");
    let snapshot = proxy.state.stats.snapshot();
    assert_eq!(snapshot.upstream_timeouts, 1);
    assert_eq!(snapshot.upstream_errors, 0, "超时与不可达分开计数");
    proxy.stop().await;
}

// ── I6 ─────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn i6_client_abort_cancels_upstream() {
    let upstream = MockUpstream::start().await;
    let proxy = ProxyHandle::start(&upstream.base, TimeoutsConfig::default()).await;

    let mut response = client()
        .get(proxy.url("/sse?frames=8&interval_ms=200&label=c"))
        .send()
        .await
        .expect("SSE 请求必须发出");
    let first = response.chunk().await.expect("第一帧必须可读");
    let second = response.chunk().await.expect("第二帧必须可读");
    assert!(first.is_some() && second.is_some(), "至少读到 2 帧再中止");
    drop(response); // ← 客户端中止（等价于应用点「停止生成」/关页）

    let cancelled = wait_until(Duration::from_secs(5), || {
        upstream.state.aborted_streams() >= 1
    })
    .await;
    assert!(
        cancelled,
        "客户端中止后，上游侧必须观测到连接被取消（FR-19：不留孤儿连接）"
    );
    let observed = wait_until(Duration::from_secs(2), || {
        proxy.state.stats.snapshot().client_aborted >= 1
    })
    .await;
    assert!(observed, "代理侧必须记到 client_aborted");
    assert!(
        upstream.state.frames_sent() < 8,
        "上游不应把 8 帧全部发完（实际 {} 帧）",
        upstream.state.frames_sent()
    );
    proxy.stop().await;
}

// ── I7 ─────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn i7_large_response_streams_through() {
    const BYTES: usize = 50 * 1024 * 1024;
    let upstream = MockUpstream::start().await;
    let proxy = ProxyHandle::start(&upstream.base, TimeoutsConfig::default()).await;

    let started = Instant::now();
    let mut response = client()
        .get(proxy.url(&format!("/big?bytes={BYTES}&chunk=262144&delay_ms=2")))
        .send()
        .await
        .expect("大响应请求必须发出");
    assert_eq!(response.status().as_u16(), 200);

    let mut offset: usize = 0;
    let mut mismatches: usize = 0;
    let mut first_byte_at: Option<Duration> = None;
    while let Some(chunk) = response.chunk().await.expect("读大响应") {
        if first_byte_at.is_none() {
            first_byte_at = Some(started.elapsed());
        }
        for (index, byte) in chunk.iter().enumerate() {
            if *byte != (((offset + index) % 251) as u8) {
                mismatches += 1;
            }
        }
        offset += chunk.len();
    }
    let elapsed = started.elapsed();
    assert_eq!(offset, BYTES, "字节数必须一致");
    assert_eq!(mismatches, 0, "逐字节保真（花纹校验）");
    let ttfb = first_byte_at.expect("必须收到首字节");
    assert!(
        elapsed > Duration::from_millis(300),
        "上游按块节流 ⇒ 总时长应 > 300 ms（实际 {elapsed:?}），否则本条判据失去意义"
    );
    assert!(
        ttfb < elapsed / 4 && ttfb < Duration::from_millis(250),
        "首字节 {ttfb:?} 必须远早于总时长 {elapsed:?}（整包缓存的话两者会接近）"
    );
    assert_eq!(proxy.state.stats.snapshot().bytes_out, BYTES as u64);
    proxy.stop().await;
}

// ── I8 ─────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn i8_concurrency_has_no_cross_talk() {
    const WORKERS: usize = 200;
    const PER_WORKER: usize = 50;

    let upstream = MockUpstream::start().await;
    let proxy = ProxyHandle::start(&upstream.base, TimeoutsConfig::default()).await;
    let client = client();

    let mut tasks = Vec::new();
    for worker in 0..WORKERS {
        let client = client.clone();
        let url = proxy.url("/echo");
        tasks.push(tokio::spawn(async move {
            let mut ok = 0usize;
            let mut bad = 0usize;
            for index in 0..PER_WORKER {
                let id = format!("w{worker}-r{index}");
                let sent = client
                    .post(&url)
                    .header("Content-Type", "application/json")
                    .header("x-test-id", &id)
                    .body(format!("{{\"id\":\"{id}\"}}"))
                    .send()
                    .await;
                match sent {
                    Ok(response) => match response.text().await {
                        Ok(text) => {
                            if text.contains(&format!("\"test_id\":\"{id}\"")) {
                                ok += 1;
                            } else {
                                bad += 1;
                            }
                        }
                        Err(_) => bad += 1,
                    },
                    Err(_) => bad += 1,
                }
            }
            (ok, bad)
        }));
    }

    let mut ok = 0usize;
    let mut bad = 0usize;
    for task in tasks {
        let (task_ok, task_bad) = task.await.expect("并发任务不得 panic");
        ok += task_ok;
        bad += task_bad;
    }
    assert_eq!(bad, 0, "200 并发 × 50 请求必须 0 错误");
    assert_eq!(ok, WORKERS * PER_WORKER);
    assert_eq!(upstream.state.count(), WORKERS * PER_WORKER);
    assert!(
        proxy.state.stats.snapshot().peak_active >= 2,
        "峰值活跃数必须被记录"
    );
    // B2 专项读数（`--nocapture` 落 stdout，便于留档）
    println!(
        "[B2-I8] 并发={WORKERS}×每并发 {PER_WORKER} 请求 · ok={ok} · bad={bad} · 上游计数={} · 上游 TCP 连接数={} · 峰值活跃={} · 池重置={} · 重试={}",
        upstream.state.count(),
        upstream.state.connections(),
        proxy.state.stats.snapshot().peak_active,
        proxy.state.stats.snapshot().pool_flushes,
        proxy.state.stats.snapshot().retries
    );
    proxy.stop().await;
}

// ── I9 ─────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn i9_keep_alive_connection_pool_is_reused() {
    const REQUESTS: usize = 10_000;
    let upstream = MockUpstream::start().await;
    let proxy = ProxyHandle::start(&upstream.base, TimeoutsConfig::default()).await;
    let client = client();

    for _ in 0..REQUESTS {
        let response = client
            .get(proxy.url("/echo"))
            .send()
            .await
            .expect("请求必须发出");
        assert_eq!(response.status().as_u16(), 200);
        // 必须**读完 body** 才会把连接还给池（否则每次都会新建连接 ⇒ I9 判据失效）
        let _ = response.text().await.expect("读响应体");
    }

    assert_eq!(upstream.state.count(), REQUESTS);
    let connections = upstream.state.connections();
    // B2 专项读数（`--nocapture` 落 stdout，便于留档）
    println!(
        "[B2-I9] 请求数={REQUESTS} · 上游接受的 TCP 连接数={connections} · 复用率={:.4}%（每连接 {:.1} 请求）· client_aborted={}",
        100.0 * (1.0 - connections as f64 / REQUESTS as f64),
        REQUESTS as f64 / connections.max(1) as f64,
        proxy.state.stats.snapshot().client_aborted
    );
    assert!(
        connections <= 50,
        "{REQUESTS} 次请求只允许极少数 TCP 连接（连接池生效），实测 {connections}"
    );
    assert_eq!(
        proxy.state.stats.snapshot().client_aborted,
        0,
        "读满 Content-Length 的正常响应不得记客户端中止（NEEDS_FIX 修复轮 P2-1：把报告声称变成门禁）"
    );
    proxy.stop().await;
}

// ── I10 ────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn i10_private_network_preflight_gets_allow_header() {
    let upstream = MockUpstream::start().await;
    let proxy = ProxyHandle::start(&upstream.base, TimeoutsConfig::default()).await;

    let response = client()
        .request(reqwest::Method::OPTIONS, proxy.url("/v1/chat/completions"))
        .header("Origin", "null")
        .header("Access-Control-Request-Method", "POST")
        .header("Access-Control-Request-Private-Network", "true")
        .send()
        .await
        .expect("PNA 预检必须发出");
    assert_eq!(response.status().as_u16(), 204);
    assert_eq!(
        header_text(&response, "access-control-allow-private-network").as_deref(),
        Some("true"),
        "FR-13：PNA 兜底头"
    );
    assert_eq!(upstream.state.count(), 0);
    proxy.stop().await;
}

// ── I11 ────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn i11_large_request_body_is_not_rejected() {
    const BYTES: usize = 100 * 1024 * 1024;
    let upstream = MockUpstream::start().await;
    let proxy = ProxyHandle::start(&upstream.base, TimeoutsConfig::default()).await;

    // 伪随机花纹（`(i * 31 + 7) & 0xFF`，可复算）——与方案 §10.2 I11「随机字节」的字面对齐
    let payload: Vec<u8> = (0..BYTES)
        .map(|index| ((index * 31 + 7) & 0xFF) as u8)
        .collect();
    let response = client()
        .post(proxy.url("/sink"))
        .header("Content-Type", "application/json")
        .body(payload)
        .send()
        .await
        .expect("大请求体必须发出");
    assert_eq!(response.status().as_u16(), 200, "≥ 100 MB 请求体不得 413");
    let text = response.text().await.expect("读响应体");
    let payload_json = json(&text);
    assert_eq!(payload_json["received_bytes"].as_u64(), Some(BYTES as u64));
    assert_eq!(
        upstream.state.last().expect("上游必须收到请求").body_bytes,
        BYTES as u64,
        "mock 上游收到的字节数必须与发出完全一致"
    );
    proxy.stop().await;
}

// ── I12 ────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn i12_redirect_is_not_followed() {
    let upstream = MockUpstream::start().await;
    let proxy = ProxyHandle::start(&upstream.base, TimeoutsConfig::default()).await;

    let response = client_no_redirect()
        .get(proxy.url("/redirect"))
        .send()
        .await
        .expect("请求必须发出");
    assert_eq!(
        response.status().as_u16(),
        302,
        "3xx 必须原样透传（Policy::none()）"
    );
    assert_eq!(
        header_text(&response, "location").as_deref(),
        Some("http://127.0.0.1:1/third-party"),
        "Location 必须逐字透传"
    );
    let text = response.text().await.expect("读响应体");
    assert_eq!(text, "mock redirect", "响应体逐字节透传");
    assert_eq!(
        upstream.state.count(),
        1,
        "代理不得向 Location 指向的第三方主机再发请求（I12）"
    );
    proxy.stop().await;
}

// ── I13/I14（FR-34；§10.2 表内无对应行，本批补） ────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn i13_graceful_shutdown_completes_inflight_request() {
    let upstream = MockUpstream::start().await;
    let proxy = ProxyHandle::start(&upstream.base, TimeoutsConfig::default()).await;
    let base = proxy.base.clone();

    let url = proxy.url("/slow?ms=1200");
    let inflight = tokio::spawn(async move {
        let response = client().get(url).send().await;
        match response {
            Ok(response) => {
                let status = response.status().as_u16();
                let body = response.text().await.unwrap_or_default();
                Ok::<(u16, String), String>((status, body))
            }
            Err(err) => Err(err.to_string()),
        }
    });
    // 等"上游确实收到了这个请求"⇒ 它已经在途
    let seen = wait_until(Duration::from_secs(5), || upstream.state.count() >= 1).await;
    assert!(seen, "在途请求必须先被上游收到");

    let elapsed = proxy.stop().await;
    let completed = inflight.await.expect("在途任务不得 panic");
    let (status, body) = completed.expect("在途请求必须成功完成（不能被停机打断）");
    assert_eq!(status, 200);
    assert!(
        body.contains("slow 1200 ms 完成"),
        "在途请求必须拿到完整响应体"
    );
    assert!(
        elapsed >= Duration::from_millis(1000),
        "停机必须**等**这个在途请求（耗时 {elapsed:?}）"
    );
    assert!(
        elapsed < GRACE_SHUTDOWN,
        "正常路径不应触发兜底（耗时 {elapsed:?}）"
    );

    // 端口必须已释放：新连接被拒
    let refused = client().get(format!("{base}/echo")).send().await;
    assert!(refused.is_err(), "停机后端口必须已释放（新连接应被拒绝）");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn i14_graceful_shutdown_hard_cap_gives_up() {
    let upstream = MockUpstream::start().await;
    let proxy = ProxyHandle::start(&upstream.base, TimeoutsConfig::default()).await;

    let url = proxy.url("/slow?ms=60000");
    let inflight = tokio::spawn(async move {
        let _ = client().get(url).send().await;
    });
    let seen = wait_until(Duration::from_secs(5), || upstream.state.count() >= 1).await;
    assert!(seen, "在途请求必须先被上游收到");

    let elapsed = proxy.stop().await;
    assert!(
        elapsed >= GRACE_SHUTDOWN - Duration::from_millis(200),
        "必须真的等满兜底时限（耗时 {elapsed:?}）"
    );
    assert!(
        elapsed <= GRACE_SHUTDOWN + Duration::from_millis(2000),
        "兜底后必须及时返回、不得无限等（耗时 {elapsed:?}）"
    );
    inflight.abort();
}

// ── FR-25 / FR-39（§6.7 / §6.3 的行为判据） ─────────────────────────────────────

#[tokio::test]
async fn fr25_credentials_redacted_in_logs_but_forwarded_intact() {
    let upstream = MockUpstream::start().await;
    let proxy = ProxyHandle::start_named(
        &upstream.base,
        TimeoutsConfig::default(),
        Level::Debug,
        "fr25",
    )
    .await;

    let response = client()
        .post(proxy.url("/v1/messages"))
        .header("Content-Type", "application/json")
        .header("Authorization", "Bearer sk-raw-secret-001")
        .header("x-api-key", "sk-raw-secret-002")
        .header("Cookie", "session=sk-raw-secret-003")
        .body("{}")
        .send()
        .await
        .expect("请求必须发出");
    assert_eq!(response.status().as_u16(), 200);
    drop(response);

    // ① 原样透传（FR-25：代理不动凭据）——含 P1-B 的 Cookie 族
    let record = upstream.state.last().expect("上游必须收到请求");
    assert_eq!(record.authorization, "Bearer sk-raw-secret-001");
    assert_eq!(record.api_key, "sk-raw-secret-002");
    assert_eq!(record.cookie, "session=sk-raw-secret-003");

    // ② 内存日志里必须脱敏（debug 级别才会记头 —— FR-28；P6-S5 起日志只在内存环）
    let log = log_snapshot(&proxy);
    assert!(
        !log.contains("sk-raw-secret-001"),
        "日志不得出现 Authorization 原文"
    );
    assert!(
        !log.contains("sk-raw-secret-002"),
        "日志不得出现 x-api-key 原文"
    );
    assert!(
        !log.contains("sk-raw-secret-003"),
        "日志不得出现 Cookie 原文（P1-B：Cookie 族必须脱敏）"
    );
    assert!(
        log.contains("Bearer ***"),
        "Authorization 必须脱敏成 `Bearer ***`"
    );
    assert!(log.contains("x-api-key: ***"), "x-api-key 必须脱敏成 `***`");
    assert!(log.contains("cookie: ***"), "Cookie 必须整值脱敏成 `***`");
    proxy.stop().await;
}

#[tokio::test]
async fn fr39_plain_content_type_warns_once_and_never_rewrites() {
    let upstream = MockUpstream::start().await;
    let proxy = ProxyHandle::start_named(
        &upstream.base,
        TimeoutsConfig::default(),
        Level::Info,
        "fr39",
    )
    .await;

    for _ in 0..3 {
        let response = client()
            .post(proxy.url("/v1/chat/completions"))
            .header("Content-Type", "text/plain")
            .body("{}")
            .send()
            .await
            .expect("请求必须发出");
        assert_eq!(response.status().as_u16(), 200);
        drop(response);
    }

    // ① 三次请求都**原样**到达上游（FR-39：只提示、绝不改包）
    assert_eq!(upstream.state.count(), 3);
    for record in upstream.state.records() {
        assert_eq!(record.content_type, "text/plain", "Content-Type 不得被改写");
    }

    // ② 同名告警 ≤ 1 条/小时 ⇒ 三次请求只留 1 条
    let log = log_snapshot(&proxy);
    assert_eq!(
        log.matches("检测到入站 Content-Type: text/plain").count(),
        1,
        "同名告警每小时最多 1 条（FR-39）"
    );

    // ③ 非 text/plain 不得触发（负例）
    let response = client()
        .post(proxy.url("/v1/chat/completions"))
        .header("Content-Type", "application/json")
        .body("{}")
        .send()
        .await
        .expect("请求必须发出");
    assert_eq!(response.status().as_u16(), 200);
    drop(response);
    let log = log_snapshot(&proxy);
    assert_eq!(
        log.matches("检测到入站 Content-Type: text/plain").count(),
        1,
        "application/json 不得触发 FR-39 告警"
    );
    proxy.stop().await;
}
// ── I15–I20（本修复批：「上游异常相位无界」） ────────────────────────────────────
//
// 口径（方案 §6.1/§6.2）：V = 测试档 `response_head_ms`（下面统一 800 ms）；
//   N₁（不可重试：POST / 带体）= V + ε（测试档断言上界取 V + 1.5 s）
//   N₂（可重试：GET/HEAD/OPTIONS 且无体且确实重试过）= 2V + ε（测试档 2V + 1.5 s）
// 「立即」= 恢复后的请求 ≤ 1 s（一次回环新拨号）。

/// 本修复批的统一测试档：等待响应头上限 800 ms。
fn head_timeout_test_config() -> TimeoutsConfig {
    TimeoutsConfig {
        connect_ms: 5_000,
        response_head_ms: 800,
        first_byte_ms: 0,
    }
}

/// 预热一发（GET /echo 并读干净）⇒ 让连接**回到代理的连接池**（I15/I18 的"池内复用"前提）。
async fn warm_pool(proxy: &ProxyHandle) {
    let response = client()
        .get(proxy.url("/echo"))
        .send()
        .await
        .expect("预热请求必须发出");
    assert_eq!(response.status().as_u16(), 200, "预热必须成功");
    let text = response.text().await.expect("预热响应体必须读完");
    assert!(text.contains("body_bytes"), "预热响应形状异常：{text}");
}

/// I15：池内有陈旧连接 + 上游静默 ⇒ **有界 504**（而不是永久挂死）。
#[tokio::test]
async fn i15_stale_pooled_conn_gets_504_within_bound() {
    let upstream = MockUpstream::start().await;
    let proxy = ProxyHandle::start(&upstream.base, head_timeout_test_config()).await;
    warm_pool(&proxy).await;
    let conns_before = upstream.state.connections();
    assert!(conns_before >= 1, "预热后必须已有上游连接");

    upstream.state.set_silent(true);
    let started = Instant::now();
    let response = client()
        .post(proxy.url("/hold"))
        .header("Content-Type", "application/json")
        .body("{\"probe\":1}")
        .send()
        .await
        .expect("请求必须发出（不得挂死）");
    let elapsed = started.elapsed();

    assert_eq!(response.status().as_u16(), 504, "必须是有界的 504");
    let kind = header_text(&response, "content-type").unwrap_or_default();
    assert!(
        kind.contains("application/json"),
        "Content-Type 必须是 JSON：{kind}"
    );
    assert_eq!(
        header_text(&response, "access-control-allow-origin").as_deref(),
        Some("*"),
        "504 必须带可用 ACAO（FR-24）"
    );
    let text = response.text().await.expect("读 504 响应体");
    let payload = json(&text);
    assert_eq!(payload["error"]["type"], "proxy_upstream_timeout");
    let message = payload["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(&upstream.base) && message.contains("等待响应头"),
        "原因必须含上游 URL 与「等待响应头」：{message}"
    );
    assert!(
        elapsed >= Duration::from_millis(800) && elapsed <= Duration::from_millis(2_300),
        "耗时必须落在 [V, V+1.5s]（实测 {elapsed:?}）"
    );
    let snapshot = proxy.state.stats.snapshot();
    assert!(
        snapshot.upstream_timeouts >= 1,
        "必须计入 upstream_timeouts"
    );
    assert_eq!(snapshot.upstream_errors, 0, "这不是 502 面");
    assert!(snapshot.pool_flushes >= 1, "L2 必须重置连接池");
    assert!(
        upstream.state.silent_hits() >= 1,
        "静默确证：请求到达但未回响应头"
    );

    // 恢复：上游回话 ⇒ 同一进程内下一发 ≤1 s 成功
    upstream.state.set_silent(false);
    let started = Instant::now();
    let response = client()
        .post(proxy.url("/echo"))
        .header("Content-Type", "application/json")
        .body("{\"probe\":2}")
        .send()
        .await
        .expect("恢复后的请求必须发出");
    assert_eq!(response.status().as_u16(), 200, "恢复后必须成功");
    let _ = response.text().await;
    assert!(
        started.elapsed() <= Duration::from_secs(1),
        "恢复后必须 ≤1 s（实测 {:?}）",
        started.elapsed()
    );
    proxy.stop().await;
}

/// I16：恢复**无需重启**（同一代理进程内自愈）+ 全链不得靠"客户端断开"收场。
#[tokio::test]
async fn i16_recovery_needs_no_restart() {
    let upstream = MockUpstream::start().await;
    let proxy = ProxyHandle::start(&upstream.base, head_timeout_test_config()).await;
    warm_pool(&proxy).await;
    upstream.state.set_silent(true);
    let response = client()
        .post(proxy.url("/hold"))
        .send()
        .await
        .expect("请求必须发出");
    assert_eq!(response.status().as_u16(), 504);
    let _ = response.text().await;

    upstream.state.set_silent(false);
    let started = Instant::now();
    let response = client()
        .get(proxy.url("/echo"))
        .send()
        .await
        .expect("恢复后的请求必须发出");
    assert_eq!(response.status().as_u16(), 200);
    let _ = response.text().await;
    assert!(
        started.elapsed() <= Duration::from_secs(1),
        "恢复后 ≤1 s（实测 {:?}）",
        started.elapsed()
    );
    assert_eq!(
        proxy.state.stats.snapshot().client_aborted,
        0,
        "不得靠客户端断开来收场（本用例没有中途 drop）"
    );
    proxy.stop().await;
}

/// I17（**负例**）：把等待响应头 watchdog 关掉（`response_head_ms = 0`）⇒ 同一装置**必复现挂死**。
/// 与 I15 成对：同一二进制、同一装置，只差这一个键 ⇒ 判据与缺陷一一对应。
#[tokio::test]
async fn i17_watchdog_off_reproduces_hang() {
    let upstream = MockUpstream::start().await;
    let mut config = head_timeout_test_config();
    config.response_head_ms = 0; // 显式关闭 = 旧行为
    let proxy = ProxyHandle::start(&upstream.base, config).await;
    warm_pool(&proxy).await;
    upstream.state.set_silent(true);

    let outcome = tokio::time::timeout(
        Duration::from_millis(1_600), // 2×V
        client().post(proxy.url("/hold")).send(),
    )
    .await;
    assert!(
        outcome.is_err(),
        "watchdog 关掉后必须复现挂死（2×V 内拿不到任何响应）；实际得到 {outcome:?}"
    );
    let snapshot = proxy.state.stats.snapshot();
    assert_eq!(snapshot.upstream_timeouts, 0, "关闭后不会有 L1 命中");
    assert_eq!(
        snapshot.pool_flushes, 0,
        "本场景无其它触发面 ⇒ 池重置必须为 0（证明没有任何机制兜住它）"
    );
    upstream.state.set_silent(false);
    proxy.stop().await;
}

/// I18：安全重试**只**给 `{GET,HEAD,OPTIONS} ∧ 无体 ∧ 未发字节`；POST 结构性永不重试。
#[tokio::test]
async fn i18_retry_only_for_safe_bodyless_requests() {
    let upstream = MockUpstream::start().await;
    let proxy = ProxyHandle::start(&upstream.base, head_timeout_test_config()).await;
    warm_pool(&proxy).await;
    let conns_after_warmup = upstream.state.connections();

    // ① GET（无体）⇒ 首试命中静默（复用池内连接）⇒ 重试恰 1 次（新池、新拨号）⇒ 仍静默 ⇒ 504
    upstream.state.set_silent(true);
    let retries_before = proxy.state.stats.snapshot().retries;
    let started = Instant::now();
    let response = client()
        .get(proxy.url("/hold"))
        .send()
        .await
        .expect("请求必须发出");
    let elapsed = started.elapsed();
    assert_eq!(response.status().as_u16(), 504, "重试后仍是 504");
    let _ = response.text().await;
    assert_eq!(
        proxy.state.stats.snapshot().retries - retries_before,
        1,
        "必须恰好重试 1 次"
    );
    assert!(
        elapsed >= Duration::from_millis(1_600) && elapsed <= Duration::from_millis(3_100),
        "耗时必须落在 [2V, 2V+1.5s]（N₂ 档；实测 {elapsed:?}）"
    );
    assert_eq!(
        upstream.state.connections(),
        conns_after_warmup + 1,
        "重试必须是**新拨号**（连接数 +1）"
    );

    // ② POST（有体）⇒ 结构性不可重试：retries 不增
    let retries_before = proxy.state.stats.snapshot().retries;
    let response = client()
        .post(proxy.url("/hold"))
        .header("Content-Type", "application/json")
        .body("{\"probe\":1}")
        .send()
        .await
        .expect("请求必须发出");
    assert_eq!(response.status().as_u16(), 504);
    let _ = response.text().await;
    assert_eq!(
        proxy.state.stats.snapshot().retries,
        retries_before,
        "POST 永不重试（SSE/对话 = POST + 体）"
    );
    upstream.state.set_silent(false);
    proxy.stop().await;
}

/// I19：响应流中断（已发头之后）⇒ 记 `stream_errors` 且**弃池**；下一请求走新拨号。
#[tokio::test]
async fn i19_stream_break_flushes_pool() {
    let upstream = MockUpstream::start().await;
    let proxy = ProxyHandle::start(&upstream.base, head_timeout_test_config()).await;
    warm_pool(&proxy).await;
    let conns_before = upstream.state.connections();

    let mut response = client()
        .get(proxy.url("/sse_break"))
        .send()
        .await
        .expect("请求必须发出");
    assert_eq!(response.status().as_u16(), 200, "响应头已发出");
    let mut got = 0;
    // Err（中断）或 None（正常结束）都收手；上限 10 防呆
    while let Ok(Some(_)) = response.chunk().await {
        got += 1;
        if got > 10 {
            break;
        }
    }
    let broke = got <= 2;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let snapshot = proxy.state.stats.snapshot();
    assert!(broke, "上游第 3 帧前应中断（实际读到 {got} 块）");
    assert!(snapshot.stream_errors >= 1, "必须计入流中断");
    assert!(
        snapshot.pool_flushes >= 1,
        "响应流中断必须触发弃池（L2 第三面）"
    );

    let response = client()
        .get(proxy.url("/echo"))
        .send()
        .await
        .expect("后续请求必须发出");
    assert_eq!(response.status().as_u16(), 200);
    let _ = response.text().await;
    assert!(
        upstream.state.connections() > conns_before,
        "弃池后必须新拨号（连接数应增加）"
    );
    proxy.stop().await;
}

/// I20（**P1-1 收口**）：**空池** + 静默（= ④b 黑洞形态）——两条进入路径分别走 N₁ / N₂。
#[tokio::test]
async fn i20_blackhole_empty_pool_is_bounded() {
    let upstream = MockUpstream::start().await;
    let proxy = ProxyHandle::start(&upstream.base, head_timeout_test_config()).await;
    // 不预热：池是空的（这一条证明 L1 **不依赖**"池内复用路径"武装）
    upstream.state.set_silent(true);

    // ① POST（不可重试 ⇒ N₁ 档）
    let started = Instant::now();
    let response = client()
        .post(proxy.url("/hold"))
        .header("Content-Type", "application/json")
        .body("{\"probe\":1}")
        .send()
        .await
        .expect("请求必须发出");
    let elapsed = started.elapsed();
    assert_eq!(response.status().as_u16(), 504);
    assert_eq!(
        header_text(&response, "access-control-allow-origin").as_deref(),
        Some("*")
    );
    let text = response.text().await.expect("读响应体");
    assert_eq!(json(&text)["error"]["type"], "proxy_upstream_timeout");
    assert!(
        elapsed >= Duration::from_millis(800) && elapsed <= Duration::from_millis(2_300),
        "N₁ 档：{elapsed:?}"
    );
    let snapshot = proxy.state.stats.snapshot();
    assert!(snapshot.pool_flushes >= 1);
    assert_eq!(snapshot.upstream_errors, 0);

    // ② GET（可重试 ⇒ N₂ 档；空池 + 静默）
    let retries_before = proxy.state.stats.snapshot().retries;
    let started = Instant::now();
    let response = client()
        .get(proxy.url("/hold"))
        .send()
        .await
        .expect("请求必须发出");
    let elapsed = started.elapsed();
    assert_eq!(response.status().as_u16(), 504);
    let _ = response.text().await;
    assert_eq!(
        proxy.state.stats.snapshot().retries - retries_before,
        1,
        "GET 必须重试恰 1 次"
    );
    assert!(
        elapsed >= Duration::from_millis(1_600) && elapsed <= Duration::from_millis(3_100),
        "N₂ 档：{elapsed:?}"
    );
    upstream.state.set_silent(false);
    proxy.stop().await;
}
// ── I21/I22 ── P7 小批 I4（`client_aborted` 假阳性）───────────────────────────────
//
// 复现（exe 级，40 次串行 GET 打「`Content-Length: 0` 的 404」上游）= 第 40 行读数 `客户端中止=39`
// —— 见 `shared/tmp/rust-proxy-p7/i4/out/r1-pre-cl0/`。机制：hyper 对**零长体**写完响应头即
// `Encoder::length(0)`（`is_eof()` 为真）⇒ 认为无体可写 ⇒ 响应体流**一次都不被 poll**、直接被 drop。
// 下面 I21 把"三种 body 形状的 404 都不得记中止"钉成门禁，I22 是它的**负例**（读到一半断连
// 必须仍然计数）—— 两条成对，缺一不可。

/// I21（正例）：**三种 body 形状**的 404 各跑 3 次 ⇒ `client_aborted` 恒为 0。
/// 形状 = ① `/empty404`（`content-length: 0`，本次假阳性的形状）② `/status?code=404`（有 body + CL）
/// ③ `/chunked404`（chunked 零帧）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn i21_three_404_body_shapes_are_not_client_aborts() {
    const ROUNDS: usize = 3;
    let paths = ["/empty404", "/status?code=404", "/chunked404"];
    let upstream = MockUpstream::start().await;
    let proxy = ProxyHandle::start(&upstream.base, TimeoutsConfig::default()).await;
    let client = client();

    for round in 0..ROUNDS {
        for path in paths {
            let response = client
                .get(proxy.url(path))
                .send()
                .await
                .expect("请求必须发出");
            assert_eq!(response.status().as_u16(), 404, "第 {round} 轮 {path}");
            let body = response.text().await.expect("读响应体");
            // 三种形状各自可辨（否则本用例的"三形状"是自我欺骗）
            match path {
                "/empty404" => assert!(body.is_empty(), "空体形状必须真的没有 body"),
                "/chunked404" => assert!(body.is_empty(), "chunked 零帧也必须没有 body"),
                _ => assert!(!body.is_empty(), "有 body 的形状必须有 body"),
            }
        }
    }

    assert_eq!(
        upstream.state.count(),
        paths.len() * ROUNDS,
        "上游必须收到全部请求"
    );
    let snapshot = proxy.state.stats.snapshot();
    assert_eq!(
        snapshot.client_aborted, 0,
        "客户端把 404 完整读完（含零长体）⇒ 不得记客户端中止（实测 {}）",
        snapshot.client_aborted
    );
    assert_eq!(snapshot.stream_errors, 0, "上游侧无错");
    assert_eq!(snapshot.active, 0, "三次都收尾后活跃必须回落");
    proxy.stop().await;
}

/// I22（**负例**）：客户端**读到一半就断** ⇒ 必须恰好计一次。
/// 与 I21 同一判据、只差"读没读完"—— 这是"新判据仍能抓真缺陷"的端到端证明。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn i22_client_abort_midway_is_still_counted() {
    let upstream = MockUpstream::start().await;
    let proxy = ProxyHandle::start(&upstream.base, TimeoutsConfig::default()).await;

    let mut response = client()
        .get(proxy.url("/sse?frames=8&interval_ms=200&label=i22"))
        .send()
        .await
        .expect("请求必须发出");
    let first = response.chunk().await.expect("第一帧必须可读");
    let second = response.chunk().await.expect("第二帧必须可读");
    assert!(first.is_some() && second.is_some(), "至少读到 2 帧再中止");
    drop(response); // ← 客户端读到一半断连

    let counted = wait_until(Duration::from_secs(5), || {
        proxy.state.stats.snapshot().client_aborted >= 1
    })
    .await;
    assert!(counted, "读到一半断连必须记客户端中止");
    let snapshot = proxy.state.stats.snapshot();
    assert_eq!(
        snapshot.client_aborted, 1,
        "本用例只有一次请求 ⇒ 必须恰好计一次（实测 {}）",
        snapshot.client_aborted
    );
    assert!(
        upstream.state.frames_sent() < 8,
        "上游不应把 8 帧全部发完（实际 {} 帧）",
        upstream.state.frames_sent()
    );
    proxy.stop().await;
}

/// I23：**同一根因的第二张脸** —— `HEAD` 请求的响应头带上游真实 `Content-Length`，但客户端可见体恒为 0
/// （hyper `!can_have_body(HEAD, …) ⇒ Encoder::length(0)`，同样**从不 poll** 响应体流）。
/// 修前这条也会被误记成客户端中止；修后按"客户端可见长度 = 0"判为读完。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn i23_head_request_is_not_a_client_abort() {
    let upstream = MockUpstream::start().await;
    let proxy = ProxyHandle::start(&upstream.base, TimeoutsConfig::default()).await;
    let client = client();

    for round in 0..3 {
        let response = client
            .head(proxy.url("/echo"))
            .send()
            .await
            .expect("HEAD 请求必须发出");
        assert_eq!(response.status().as_u16(), 200, "第 {round} 轮");
        assert!(
            header_text(&response, "content-length").is_some(),
            "HEAD 响应应带上游声明的 Content-Length（这正是修前误判的来源）"
        );
        let body = response
            .text()
            .await
            .expect("HEAD 响应体必须可读（应为空）");
        assert!(body.is_empty(), "HEAD 不得有 body");
    }

    let snapshot = proxy.state.stats.snapshot();
    assert_eq!(
        snapshot.client_aborted, 0,
        "HEAD 的客户端可见体长为 0 ⇒ 不得记客户端中止（实测 {}）",
        snapshot.client_aborted
    );
    assert_eq!(snapshot.active, 0, "收尾后活跃必须回落");
    proxy.stop().await;
}
