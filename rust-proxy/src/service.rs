//! 服务生命周期控制器（Phase 2 范围项 ⑤ 启停热切换 / ⑥ 端口降级提示 / 配置热生效）。
//!
//! 结构：**UI 线程**（Win32 消息泵）与**服务线程**（tokio 运行时）分离 —— 消息泵必须占据
//! 主线程（Win32 硬约束），服务跑在专用运行时线程上；两侧只经本控制器的
//! `mpsc` 命令 + 一把状态快照 `Mutex` 通信（UI 每 1 s 轮询 + 状态变化时收 WM_APP 消息即时刷新）。
//!
//! 三条口径（方案 §6.1 / §6.11 / §6.10）：
//! - **端口降级**（FR-2）：主端口绑定失败 ⇒ 依 `port_fallback` 顺序尝试；成功即记
//!   `degraded + degrade_note`（UI 的黄灯与"实际 Base URL"都读它）；
//! - **失败分类**（FR-3）：每个候选的失败原因分别记账，全失败 ⇒ 红灯 + 主端口原因 + 已试候选清单；
//! - **热切换**：`Apply` = 停旧起新；新配置全端口绑定失败 ⇒ **回滚到旧配置**（**仅内存**：
//!   P6-S4 起没有任何配置文件，回滚不再写盘）并在 `apply_message` 里说明（UI 显示）。
//!
//! 长生命周期共享：`stats` / `logger` / `notices` 是 `Arc`，跨启停热切换保留（累计数与
//! 最近错误不因点一次「停止/启动」而清零；日志器只热改级别（P6-S5 起日志只有内存环））。

use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::config::{self, Config};
use crate::logging::{Level, Logger};
use crate::proxy::notice::WarnThrottle;
use crate::proxy::AppState;
use crate::server::{self, Shutdown, GRACE_SHUTDOWN};
use crate::stats::Stats;

/// 服务状态变化时发给 UI 线程的消息号（`WM_APP + 1`；UI 侧 `ui/mod.rs` 定义同名常量）。
pub const WM_APP_REFRESH: u32 = 0x8000 + 1;

/// 停机时"等服务真的收尾"的额外上限（`GRACE_SHUTDOWN` 之后还等不到 ⇒ abort 服务任务）。
const STOP_JOIN_TIMEOUT: Duration = Duration::from_secs(GRACE_SHUTDOWN.as_secs() + 2);

/// 服务相位（UI 状态灯 / 托盘图标四态的输入）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Stopped,
    Starting,
    Running,
    Error,
}

/// UI 可读的状态快照（`Clone`：每次刷新拷一份，量级 = 几个字符串）。
#[derive(Clone)]
pub struct Status {
    pub phase: Phase,
    /// 当前配置（含未启动时的"待启动"配置；设置窗以它为初值）。
    pub config: Config,
    /// 主端口（= 用户配置的 `listen_port`，或 CLI `--port` 覆盖值）。
    pub requested_port: u16,
    /// 实际监听地址 `127.0.0.1:8000`（未运行 ⇒ `None`）。
    pub listen_addr: Option<String>,
    /// 应用侧应填的 Base URL `http://127.0.0.1:8000/v1`（未运行 ⇒ `None`）。
    pub base_url: Option<String>,
    /// 是否走了端口降级（FR-2：黄灯 + 托盘"降级"态）。
    pub degraded: bool,
    /// 降级说明（一行；UI 提示条 / 气泡用）。
    pub degrade_note: Option<String>,
    /// 最近一次错误（绑定失败/内部错误；成功启动后清空）。
    pub last_error: Option<String>,
    /// 本次运行的起点（uptime 计时）。
    pub started_at: Option<Instant>,
    /// 配置热生效的序号 + 结果（UI 靠序号变化识别"这次保存的结果"）。
    pub apply_seq: u64,
    pub apply_message: Option<String>,
    pub apply_ok: bool,
    /// CLI `--port` 覆盖（显示用；`None` = 用配置值）。
    pub cli_port: Option<u16>,
}

impl Status {
    /// 初始（停止态）快照；UI 首帧与测试也用它。
    pub fn for_config(config: Config, cli_port: Option<u16>) -> Status {
        let requested_port = cli_port.unwrap_or(config.listen_port);
        Status {
            phase: Phase::Stopped,
            config,
            requested_port,
            listen_addr: None,
            base_url: None,
            degraded: false,
            degrade_note: None,
            last_error: None,
            started_at: None,
            apply_seq: 0,
            apply_message: None,
            apply_ok: true,
            cli_port,
        }
    }

    /// FR-29：运行时长（未运行 ⇒ `None`）。
    pub fn uptime(&self) -> Option<Duration> {
        self.started_at.map(|started| started.elapsed())
    }
}

/// 一个"在跑的服务实例"。
struct Running {
    shutdown: Shutdown,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
    listen_addr: String,
    base_url: String,
    degraded: bool,
    degrade_note: Option<String>,
    started_at: Instant,
}

enum Command {
    Start,
    Stop,
    Apply(Box<Config>),
    Shutdown(std::sync::mpsc::Sender<()>),
}

/// 控制器句柄（UI 线程持有；线程安全）。
pub struct Controller {
    tx: mpsc::UnboundedSender<Command>,
    status: Arc<Mutex<Status>>,
    notify_hwnd: Arc<AtomicIsize>,
    stats: Arc<Stats>,
    logger: Arc<Logger>,
    notices: Arc<WarnThrottle>,
    runtime: tokio::runtime::Handle,
    /// 运行时线程的退出开关（`shutdown()` 时发一次）。
    runtime_stop: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Controller {
    /// 起运行时线程 + 命令循环；返回控制器句柄。失败（运行时建不起来）⇒ `Err(可读原因)`。
    pub fn spawn(
        initial: Config,
        logger: Arc<Logger>,
        cli_port: Option<u16>,
    ) -> Result<Controller, String> {
        let (ready_tx, ready_rx) =
            std::sync::mpsc::channel::<Result<tokio::runtime::Handle, String>>();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("azusa-proxy-runtime".to_string())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(err) => {
                        let _ = ready_tx.send(Err(format!("Tokio 运行时启动失败：{err}")));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(runtime.handle().clone()));
                // 等主线程喊停：喊停前命令循环已把服务停干净（`shutdown()` 的两段式）
                runtime.block_on(async move {
                    let _ = stop_rx.await;
                });
                // runtime 在此 drop：任务已收尾，端口已释放
            })
            .map_err(|err| format!("服务线程创建失败：{err}"))?;

        let runtime = match ready_rx.recv() {
            Ok(Ok(handle)) => handle,
            Ok(Err(message)) => return Err(message),
            Err(_) => return Err("服务线程提前退出（未能取得 Tokio 运行时句柄）".to_string()),
        };

        let stats = Arc::new(Stats::new());
        let notices = Arc::new(WarnThrottle::new());
        let (tx, rx) = mpsc::unbounded_channel::<Command>();

        let inner = Inner {
            config: initial.clone(),
            cli_port,
            running: None,
            last_error: None,
            apply_seq: 0,
            apply_message: None,
            apply_ok: true,
            stats: Arc::clone(&stats),
            logger: Arc::clone(&logger),
            notices: Arc::clone(&notices),
        };
        let shared = SharedStatus {
            status_slot: Arc::new(Mutex::new(Status::for_config(initial, cli_port))),
            notify_hwnd: Arc::new(AtomicIsize::new(0)),
        };
        {
            let shared = shared.clone();
            runtime.spawn(command_loop(rx, inner, shared));
        }

        Ok(Controller {
            tx,
            status: Arc::clone(&shared.status_slot),
            notify_hwnd: Arc::clone(&shared.notify_hwnd),
            stats,
            logger,
            notices,
            runtime,
            runtime_stop: Mutex::new(Some(stop_tx)),
            thread: Mutex::new(Some(thread)),
        })
    }

    /// UI 主窗创建后登记窗口句柄：此后每次状态变化都会给它 `PostMessage(WM_APP_REFRESH)`。
    pub fn set_notify_hwnd(&self, hwnd: isize) {
        self.notify_hwnd.store(hwnd, Ordering::SeqCst);
    }

    pub fn status(&self) -> Status {
        match self.status.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    pub fn stats(&self) -> Arc<Stats> {
        Arc::clone(&self.stats)
    }

    pub fn notices(&self) -> Arc<WarnThrottle> {
        Arc::clone(&self.notices)
    }

    pub fn logger(&self) -> Arc<Logger> {
        Arc::clone(&self.logger)
    }

    pub fn runtime(&self) -> tokio::runtime::Handle {
        self.runtime.clone()
    }

    pub fn start(&self) {
        let _ = self.tx.send(Command::Start);
    }

    pub fn stop(&self) {
        let _ = self.tx.send(Command::Stop);
    }

    /// FR-37：写盘由 UI 完成后调用本方法做"热生效"（停旧起新 / 失败回滚）。
    pub fn apply(&self, config: Config) {
        let _ = self.tx.send(Command::Apply(Box::new(config)));
    }

    /// 退出：停服务 → 关运行时线程 → join；整体有上限（超时即放弃等待，进程退出兜底）。
    pub fn shutdown(&self) {
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        if self.tx.send(Command::Shutdown(done_tx)).is_err() {
            return; // 循环已退出（运行时已收尾）
        }
        let _ = done_rx.recv_timeout(STOP_JOIN_TIMEOUT + Duration::from_secs(3));
        if let Ok(mut guard) = self.runtime_stop.lock() {
            if let Some(stop) = guard.take() {
                let _ = stop.send(());
            }
        }
        let thread = match self.thread.lock() {
            Ok(mut guard) => guard.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        if let Some(thread) = thread {
            let _ = thread.join();
        }
    }
}

/// 命令循环与 UI 之间共享的两个槽位（`Arc` 包住内部可变位，便于任务与控制器共享）。
struct SharedStatus {
    status_slot: Arc<Mutex<Status>>,
    notify_hwnd: Arc<AtomicIsize>,
}

impl Clone for SharedStatus {
    fn clone(&self) -> SharedStatus {
        SharedStatus {
            status_slot: Arc::clone(&self.status_slot),
            notify_hwnd: Arc::clone(&self.notify_hwnd),
        }
    }
}

impl SharedStatus {
    /// 发布新状态（非阻塞）：写快照 + 通知 UI 线程。
    fn publish(&self, status: Status) {
        match self.status_slot.lock() {
            Ok(mut guard) => *guard = status,
            Err(poisoned) => *poisoned.into_inner() = status,
        }
        let hwnd = self.notify_hwnd.load(Ordering::SeqCst);
        if hwnd != 0 {
            post_refresh(hwnd);
        }
    }
}

/// 通知 UI 线程刷新（消息投递失败无非阻塞窗口，忽略）。
fn post_refresh(hwnd: isize) {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
        let _ = PostMessageW(
            Some(HWND(hwnd as *mut core::ffi::c_void)),
            WM_APP_REFRESH,
            WPARAM(0),
            LPARAM(0),
        );
    }
    #[cfg(not(windows))]
    let _ = hwnd;
}

/// 命令循环独占的可变状态（只在此任务内使用 ⇒ 无需加锁）。
struct Inner {
    config: Config,
    cli_port: Option<u16>,
    running: Option<Running>,
    last_error: Option<String>,
    apply_seq: u64,
    apply_message: Option<String>,
    apply_ok: bool,
    stats: Arc<Stats>,
    logger: Arc<Logger>,
    notices: Arc<WarnThrottle>,
}

async fn command_loop(
    mut rx: mpsc::UnboundedReceiver<Command>,
    mut inner: Inner,
    shared: SharedStatus,
) {
    while let Some(command) = rx.recv().await {
        match command {
            Command::Start => {
                shared.publish(inner.snapshot(Phase::Starting));
                inner.start(&shared).await;
            }
            Command::Stop => {
                inner.stop(&shared).await;
            }
            Command::Apply(config) => {
                inner.apply(*config, &shared).await;
            }
            Command::Shutdown(done) => {
                inner.stop(&shared).await;
                let _ = done.send(());
                break;
            }
        }
    }
}

impl Inner {
    /// 组装当前状态快照（相位由调用点给：启动中/运行/停止/错误）。
    fn snapshot(&self, phase: Phase) -> Status {
        let mut status = Status::for_config(self.config.clone(), self.cli_port);
        status.phase = phase;
        status.apply_seq = self.apply_seq;
        status.apply_message = self.apply_message.clone();
        status.apply_ok = self.apply_ok;
        match &self.running {
            Some(running) => {
                status.listen_addr = Some(running.listen_addr.clone());
                status.base_url = Some(running.base_url.clone());
                status.degraded = running.degraded;
                status.degrade_note = running.degrade_note.clone();
                status.started_at = Some(running.started_at);
            }
            None => {
                status.last_error = self.last_error.clone();
            }
        }
        status
    }

    /// 应用配置里的级别到共享日志器（P6-S5 / D12：日志只剩内存环，没有目录/份数可改）。
    fn apply_logging(&self) {
        let level = Level::parse(&self.config.logging.level).unwrap_or(Level::Info);
        self.logger.apply(level);
    }

    /// 起服务（含 FR-2 端口降级 + FR-3 失败分类）。已在跑 ⇒ 原地不动。
    async fn start(&mut self, shared: &SharedStatus) {
        if self.running.is_some() {
            return;
        }
        self.apply_logging();

        let state = match AppState::new(
            self.config.clone(),
            Arc::clone(&self.stats),
            Arc::clone(&self.logger),
            Arc::clone(&self.notices),
        ) {
            Ok(state) => Arc::new(state),
            Err(err) => {
                self.last_error = Some(format!("HTTP 客户端构建失败：{err}"));
                self.logger
                    .error(&format!("服务启动失败：HTTP 客户端构建失败（{err}）"));
                shared.publish(self.snapshot(Phase::Error));
                return;
            }
        };
        let upstream = state.config.upstream_base_trimmed().to_string();

        let primary = self.cli_port.unwrap_or(self.config.listen_port);
        let (_, candidates) = config::port_candidates(primary, &self.config.port_fallback);
        let mut attempts: Vec<(u16, String)> = Vec::new();
        let mut listener = None;
        let mut used_port = primary;
        let sequence: Vec<u16> = std::iter::once(primary)
            .chain(candidates.iter().copied())
            .collect();
        for port in &sequence {
            match server::bind(&self.config.listen_host, *port).await {
                Ok(bound) => {
                    // 第二次自环复检（方案 §3.5 时点 ②）：bind 成功后按**实际**监听口再比对一次。
                    // 命中 ⇒ **放弃该候选端口**（继续试下一个），避免"回退到恰是上游的口"。
                    let self_loop = crate::cli::check_self_loop(
                        &self.config.upstream_base,
                        &self.config.listen_host,
                        *port,
                    )
                    .map(|check| check.is_loop)
                    .unwrap_or(false);
                    if self_loop {
                        self.logger.warn(&format!(
                            "端口 {port} 放弃：上游 {} 与本程序实际监听同址（自环保护）",
                            self.config.upstream_base_trimmed()
                        ));
                        attempts.push((
                            *port,
                            format!(
                                "自环：上游 {} 与本程序实际监听 {port} 同址",
                                self.config.upstream_base_trimmed()
                            ),
                        ));
                        drop(bound);
                        continue;
                    }
                    used_port = *port;
                    listener = Some(bound);
                    break;
                }
                Err((message, _code)) => {
                    self.logger
                        .warn(&format!("端口 {port} 绑定失败：{message}"));
                    attempts.push((*port, message));
                }
            }
        }

        let listener = match listener {
            Some(listener) => listener,
            None => {
                // FR-3：全候选失败 ⇒ 红灯 + 主端口原因 + 已试候选
                let primary_reason = attempts
                    .first()
                    .map(|(_, message)| message.clone())
                    .unwrap_or_else(|| "绑定失败".to_string());
                let tried = sequence
                    .iter()
                    .map(|port| port.to_string())
                    .collect::<Vec<_>>()
                    .join("、");
                let summary =
                    format!("{primary_reason}（已尝试候选端口：{tried}；请到「设置」改端口）");
                self.logger.error(&format!("服务启动失败：{summary}"));
                self.last_error = Some(summary);
                shared.publish(self.snapshot(Phase::Error));
                return;
            }
        };

        let listen_addr = listener
            .local_addr()
            .map(|addr| addr.to_string())
            .unwrap_or_else(|_| format!("{}:{}", self.config.listen_host, used_port));
        let base_url = format!("http://{listen_addr}/v1");
        let degraded = used_port != primary;
        let degrade_note = if degraded {
            let reason = attempts
                .first()
                .map(|(_, message)| shorten_bind_reason(message))
                .unwrap_or_else(|| "绑定失败".to_string());
            Some(format!(
                "端口 {primary} {reason}，已自动降级到 {used_port}；应用侧 Base URL 请填 {base_url}"
            ))
        } else {
            None
        };

        if let Some(note) = &degrade_note {
            self.logger.warn(note);
        }
        // DG9：默认仅回环；非回环监听属有意配置 ⇒ 明确告警
        if !is_loopback_host(&self.config.listen_host) {
            self.logger.warn(&format!(
                "监听地址 {} 不是回环地址（DG9 默认仅 127.0.0.1；请确认这是有意配置）",
                self.config.listen_host
            ));
        }

        let shutdown = Shutdown::new();
        let task = tokio::spawn(server::serve(
            listener,
            Arc::clone(&state),
            shutdown.clone(),
        ));
        self.logger.info(&format!(
            "服务已启动：http://{listen_addr}/ -> {upstream}（应用侧 Base URL 填 {base_url}）"
        ));

        self.last_error = None;
        self.running = Some(Running {
            shutdown,
            task,
            listen_addr,
            base_url,
            degraded,
            degrade_note,
            started_at: Instant::now(),
        });
        shared.publish(self.snapshot(Phase::Running));
    }

    /// 停服务（FR-34：停止接受新连接 → 等在途收尾 ≤ GRACE_SHUTDOWN → 释放端口）。
    async fn stop(&mut self, shared: &SharedStatus) {
        if let Some(running) = self.running.take() {
            running.shutdown.trigger();
            let mut task = running.task;
            match tokio::time::timeout(STOP_JOIN_TIMEOUT, &mut task).await {
                Ok(Ok(Ok(()))) => {
                    self.logger.info("服务已停止（在途请求已收尾；端口已释放）");
                }
                Ok(Ok(Err(err))) => {
                    self.logger
                        .warn(&format!("服务停止时 IO 错误：{err}（端口已释放）"));
                }
                Ok(Err(join_err)) => {
                    self.logger
                        .warn(&format!("服务任务异常结束：{join_err}（端口已释放）"));
                }
                Err(_) => {
                    // 超时兜底：abort 任务，保证端口立刻回到内核（不 detached 挂着）
                    task.abort();
                    let _ = task.await;
                    self.logger.warn(&format!(
                        "优雅停机等待超时（{} ms）：已强制停止服务任务（端口已释放）",
                        STOP_JOIN_TIMEOUT.as_millis()
                    ));
                }
            }
        }
        shared.publish(self.snapshot(Phase::Stopped));
    }

    /// 配置热生效（**仅内存**：P6-S4 起不再有任何配置文件，改动的生命周期 = 本次运行）。
    /// 这里负责"停旧起新 / 失败回滚"；校验与提示由 UI 侧完成。
    async fn apply(&mut self, config: Config, shared: &SharedStatus) {
        let previous = self.config.clone();
        let was_running = self.running.is_some();
        self.config = config;
        self.apply_logging();
        self.apply_seq += 1;
        self.apply_message = None;
        self.apply_ok = true;

        if !was_running {
            self.apply_message = Some(
                "已应用到本次运行（不写文件；关掉程序即回到默认）；\
                 服务当前是停止状态，改动将在下次启动时生效"
                    .to_string(),
            );
            shared.publish(self.snapshot(Phase::Stopped));
            return;
        }

        self.stop(shared).await;
        self.start(shared).await;
        if self.running.is_none() {
            // 新配置起不来 ⇒ 回滚到旧配置（仅内存：没有文件可回滚）
            let failure = self
                .last_error
                .clone()
                .unwrap_or_else(|| "新配置无法绑定端口".to_string());
            self.config = previous;
            self.apply_logging();
            self.start(shared).await;
            let restored = if self.running.is_some() {
                "已回滚到原配置并继续服务".to_string()
            } else {
                format!(
                    "回滚也失败（{}）——请修正端口后重试",
                    self.last_error
                        .clone()
                        .unwrap_or_else(|| "未知原因".to_string())
                )
            };
            self.apply_ok = false;
            self.apply_message = Some(format!("新配置未生效：{failure}；{restored}"));
            self.logger.warn(&format!(
                "配置热生效失败：{failure}；{restored}（仅内存配置已回滚）"
            ));
        } else {
            let mut message = "已应用到本次运行（不写文件；关掉程序即回到默认）".to_string();
            if self
                .running
                .as_ref()
                .map(|item| item.degraded)
                .unwrap_or(false)
            {
                if let Some(note) = self
                    .running
                    .as_ref()
                    .and_then(|item| item.degrade_note.clone())
                {
                    message = format!("{message}；{note}");
                }
            }
            self.apply_message = Some(message);
        }
        let phase = if self.running.is_some() {
            Phase::Running
        } else {
            Phase::Error
        };
        shared.publish(self.snapshot(phase));
    }
}

/// 绑定失败原因的长文本 → 短句（UI 提示条/气泡的空间有限）。
fn shorten_bind_reason(message: &str) -> String {
    if message.contains("端口被占用") {
        "被占用".to_string()
    } else if message.contains("权限不足") {
        "权限不足（需要管理员或换端口）".to_string()
    } else {
        "绑定失败".to_string()
    }
}

/// P1-C：用**单点归一**（`cli::normalize_host`）判回环，不再靠字符串字面量比对。
/// 归一后 `Config::listen_host` 是字面量 IP ⇒ 这里对 `IpAddr` 判定；`localhost`/`::1` 已被 CLI 归一，
/// 该分支属**防御性**（正常路径不可达 —— 那类值 bind 必失败）。
fn is_loopback_host(host: &str) -> bool {
    crate::cli::normalize_host(host)
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_reason_is_shortened() {
        assert_eq!(
            shorten_bind_reason("监听 127.0.0.1:80 失败：端口被占用（常见占用者：IIS / 其它本地服务 / 上一实例未退出）（…）"),
            "被占用"
        );
        assert_eq!(
            shorten_bind_reason("监听 … 失败：权限不足（可换用非保留端口，或以管理员运行）"),
            "权限不足（需要管理员或换端口）"
        );
        assert_eq!(
            shorten_bind_reason("监听 … 失败：绑定失败（…）"),
            "绑定失败"
        );
    }

    #[test]
    fn uptime_reflects_started_at() {
        let config = Config::default();
        let mut status = Status::for_config(config, None);
        assert!(status.uptime().is_none());
        status.started_at = Some(Instant::now() - Duration::from_secs(3));
        assert!(status.uptime().map(|value| value.as_secs()).unwrap_or(0) >= 3);
    }
}
