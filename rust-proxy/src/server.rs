//! 服务生命周期（方案 §6.10 FR-34 优雅停机；§6.5 FR-21 超时口径在此接线）。
//!
//! FR-34 写死三件事：**停止接受新连接** → **等在途请求收尾** → **超时兜底**。
//! 实现分工：
//! - 「停止接受新连接 + 等在途请求」交给 `axum::serve(..).with_graceful_shutdown(..)` —— 它会在
//!   停机信号到达后关掉空闲 keep-alive 连接、并等已进入 handler 的请求把响应写完后才返回；
//! - 「超时兜底（≤ 5 s）」由本模块的 `select!` 第二臂负责：信号触发后再等 `GRACE_SHUTDOWN`，
//!   仍未返回就放弃等待（进程随后退出，端口由 OS 释放 —— FR-4 不留 TIME_WAIT 阻塞重启）。
//!
//! ⚠ 与 FR-21 的关系：**本模块不引入任何"请求总超时"**（那会掐死 SSE，与 FR-18 互斥）；
//! 这里的 5 s 只在**停机**路径上生效，且只约束"收尾等待"，不约束任何在途请求本身的时长。

use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::watch;

use crate::proxy::AppState;

/// FR-34：停机时"等在途请求收尾"的上限。
pub const GRACE_SHUTDOWN: Duration = Duration::from_secs(5);

/// 停止信号。可被多处同时 `wait`（服务侧等它关新连接；兜底侧等它开始计时）。
#[derive(Clone)]
pub struct Shutdown {
    tx: watch::Sender<bool>,
    rx: watch::Receiver<bool>,
}

impl Default for Shutdown {
    fn default() -> Shutdown {
        Shutdown::new()
    }
}

impl Shutdown {
    pub fn new() -> Shutdown {
        let (tx, rx) = watch::channel(false);
        Shutdown { tx, rx }
    }

    /// 触发停机（幂等；重复触发无副作用）。
    pub fn trigger(&self) {
        let _ = self.tx.send(true);
    }

    pub fn is_triggered(&self) -> bool {
        *self.rx.borrow()
    }

    /// 等"停机"被触发；已触发则立即返回。
    /// 持有者（`main.rs` / 测试）必须保证 `Shutdown` 本体活得比这个 future 长 ——
    /// 否则 `changed()` 返回 `Err`（发送端已 drop），本 future 会走"已停止"分支提前返回。
    pub fn wait(&self) -> impl std::future::Future<Output = ()> + Send + 'static {
        let mut rx = self.rx.clone();
        async move {
            while !*rx.borrow_and_update() {
                if rx.changed().await.is_err() {
                    // 发送端消失 = 没人再会发信号 ⇒ 视为"永远不会停机"，挂住而不是空转返回。
                    std::future::pending::<()>().await;
                }
            }
        }
    }
}

/// 起服务直到停机收尾完成（或兜底时限到）。
///
/// 正常路径：`axum` 优雅停机完成 ⇒ 返回它的 `io::Result`。
/// 兜底路径：信号到达后 `GRACE_SHUTDOWN` 内未收尾完 ⇒ 记一条 warn 并返回 `Ok(())`
///（"放弃等待"是**有意**的：FR-34 只承诺 ≤ 5 s，不承诺无限等）。
pub async fn serve(
    listener: TcpListener,
    state: Arc<AppState>,
    shutdown: Shutdown,
) -> std::io::Result<()> {
    let serve_future = axum::serve(listener, crate::proxy::router(Arc::clone(&state)))
        .with_graceful_shutdown(shutdown.wait());

    let watchdog = {
        let state = Arc::clone(&state);
        let guard = shutdown.wait();
        async move {
            guard.await;
            state.logger.warn(&format!(
                "收到停机信号：停止接受新连接，等在途请求收尾（上限 {} ms；在途 = {}）",
                GRACE_SHUTDOWN.as_millis(),
                state.stats.snapshot().active
            ));
            tokio::time::sleep(GRACE_SHUTDOWN).await;
        }
    };

    tokio::select! {
        result = serve_future => {
            result.map(|()| {
                let snapshot = state.stats.snapshot();
                state.logger.info(&format!(
                    "服务已停止（在途请求已收尾；{}；监听地址释放）",
                    snapshot.render()
                ));
            })
        }
        () = watchdog => {
            let snapshot = state.stats.snapshot();
            state.logger.warn(&format!(
                "优雅停机超时（{} ms）：在途请求未收尾，放弃等待并退出（{}）",
                GRACE_SHUTDOWN.as_millis(),
                snapshot.render()
            ));
            Ok(())
        }
    }
}

/// 便捷函数：绑定地址（端口 0 = 让 OS 选，测试用）并按 FR-3 区分绑定失败原因。
///
/// **P1-C（S9）**：host 经**单点归一**（`cli::normalize_host` —— 与自环守卫 / `--listen-host`
/// 校验**逐字同一套规则**）后再建 `SocketAddr`。去配置批里 CLI 已在 `apply_overrides` 归一，
/// 但 `Config::listen_host` 还有另一个来源面（内建默认 / 未来新增的配置入口），而原先这里走的是
/// `format!("{host}:{port}").parse::<SocketAddr>()` —— 那是**第二套字符串规则**：
/// `localhost:80` / `127.1:80` 这类"CLI 认、bind 不认"的写法会以"监听地址非法"收场，
/// 与同一份 host 在守卫侧的判定不一致（§8-P1-C 的残余分歧）。
/// 归一后 `Config::listen_host` 与 bind 的输入恒为字面量 IP，两侧规则不可能再漂移。
pub async fn bind(
    listen_host: &str,
    listen_port: u16,
) -> Result<TcpListener, (String, Option<i32>)> {
    let ip = match crate::cli::normalize_host(listen_host) {
        Some(ip) => ip,
        None => {
            let reason = format!(
                "监听地址非法：{listen_host}（检查 --listen-host 与 --listen-port 是不是合法的 IP:端口）"
            );
            return Err((reason, None));
        }
    };
    let parsed = std::net::SocketAddr::new(ip, listen_port);
    let address = parsed.to_string();
    match TcpListener::bind(parsed).await {
        Ok(listener) => Ok(listener),
        Err(err) => {
            // FR-3：分清"端口被占用"与"权限"两类绑定失败
            let hint = match err.raw_os_error() {
                Some(10048) => "端口被占用（常见占用者：IIS / 其它本地服务 / 上一实例未退出）",
                Some(10013) => "权限不足（可换用非保留端口，或以管理员运行）",
                _ => "绑定失败",
            };
            Err((
                format!("监听 {address} 失败：{hint}（{err}）"),
                err.raw_os_error(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P6-S4：去配置文件后，绑定报错的用户可见文案必须指向 **CLI 参数**
    /// （旧文案让用户去改一个不再存在的配置文件）。
    #[tokio::test]
    async fn bind_error_text_points_at_cli_params() {
        let (reason, code) = bind("not-an-ip", 80)
            .await
            .expect_err("非法的 listen_host 必须被拒绝");
        assert!(
            reason.contains("--listen-host") && reason.contains("--listen-port"),
            "文案须指向 CLI 参数：{reason}"
        );
        assert!(code.is_none(), "地址解析失败没有 OS 错误码：{code:?}");
    }

    /// **P1-C（S9）**：归一后，`bind` 与自环守卫 / `--listen-host` 校验**同一套等价类**
    /// —— `localhost` / `[::1]` / 数值型 `127.1` 都不再落到"监听地址非法"。
    #[tokio::test]
    async fn bind_normalizes_host_aliases_like_the_self_loop_guard() {
        for host in ["localhost", "[::1]", "::1", "127.1"] {
            let listener = bind(host, 0)
                .await
                .unwrap_or_else(|(reason, _)| panic!("{host} 归一后必须可绑定：{reason}"));
            let addr = listener.local_addr().expect("刚绑上的 listener 必有地址");
            assert_eq!(
                addr.ip().to_string(),
                "127.0.0.1",
                "{host} 必须归一到 127.0.0.1（与守卫同源）：{addr}"
            );
            assert_ne!(addr.port(), 0, "端口 0 = 让 OS 选，绑定后应拿到真实端口");
        }
    }

    /// **P1-C（S9）**：`0.0.0.0`（通配）仍照旧可绑 —— 这条同时证明 `service.rs` 的
    /// DG9「非回环告警」分支**不是死分支**（§8-P1-C 的中间证据句）。
    #[tokio::test]
    async fn bind_accepts_wildcard_host() {
        let listener = bind("0.0.0.0", 0)
            .await
            .expect("通配地址必须可绑（DG9 的告警分支依赖它可绑）");
        let addr = listener.local_addr().expect("刚绑上的 listener 必有地址");
        assert_eq!(addr.ip().to_string(), "0.0.0.0");
        assert!(
            !addr.ip().is_loopback(),
            "通配地址非回环 ⇒ service.rs 的告警分支会被求值"
        );
    }
}
