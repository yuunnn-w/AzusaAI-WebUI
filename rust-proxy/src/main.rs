//! AzusaAI 本地反向代理 —— 程序入口（**进程语义层**，方案 §2.1 / §2.3）。
//!
//! `main()` 只做四件事：**解析 CLI → 立即动作（帮助/版本）→ 参数落 `Config` + 自环预检 →
//! 启动服务（主窗/托盘 或 `--no-gui`）**。配置来源分层见 `config.rs`（去配置文件后 CLI 是唯一入口）。
//!
//! ⚠ **K13**：开机自启功能已整体移除（实现文件删除、两个自启参数删除）
//! ⇒ 程序**不写注册表、不写文件**，持久化面为零。
//!
//! ⚠ **无控制台窗口**（D6）：本文件顶层 `#![windows_subsystem = "windows"]`（无条件，一种形态）。
//! 所有用户可见文本经 `output` 模块的三通道分发（有控制台 / 重定向 / 弹窗），**不再用** `println!`。
//!
//! 线程分工（Win32 硬约束）：**主线程 = 消息泵**（窗口/托盘/绘制），**服务线程 = tokio 运行时**
//!（`Controller::spawn` 里起）。主线程退出前走 `Controller::shutdown()`（停服务 → 关运行时 → join）。
//!
//! 退出码约定（`--help` 里写明；脚本与验收剧本按此判定）：
//! 0 = 正常退出；1 = 运行期错误；2 = 参数错误（**含自环拒绝启动**）；3 = 已有实例（已尝试唤起其窗口）。

#![windows_subsystem = "windows"]

mod ui;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use azusa_local_proxy::{cli, config, logging, osver, output, service};

/// FR-32：命名互斥体名。**不带 `Global\` 前缀** ⇒ 落在当前会话的命名空间 + 创建者默认 DACL
///（只有同一用户能打开；显式传 NULL 描述符等于 "everyone 全权"，是降级，故意不用）。
const SINGLE_INSTANCE_MUTEX: &str = "AzusaAI-Local-Proxy-SingleInstance-v1";

const EXIT_RUNTIME_ERROR: u8 = 1;
const EXIT_BAD_ARGS: u8 = 2;
const EXIT_ALREADY_RUNNING: u8 = 3;

fn main() -> ExitCode {
    output::init();
    let raw_args: Vec<String> = std::env::args().skip(1).collect();

    let parsed = match cli::parse_args(raw_args.iter().cloned()) {
        Ok(parsed) => parsed,
        Err(err) => return fail_args(&format!("错误：{err}")),
    };

    match parsed.mode {
        cli::Mode::Help => {
            output::global().print_usage();
            return ExitCode::SUCCESS;
        }
        cli::Mode::Version => {
            output::global().print_version();
            return ExitCode::SUCCESS;
        }
        cli::Mode::Run => {}
    }

    // 参数落 Config（唯一入口）+ 唯一合法性闸。
    let mut app_config = config::Config::default();
    if let Err(err) = parsed.apply_overrides(&mut app_config) {
        return fail_args(&format!("错误：{err}"));
    }

    // 自环预检（方案 §3.5 时点 ①：bind 之前，用**意图**监听口）。
    if let Err(err) = ensure_not_self_loop(&app_config) {
        return err;
    }

    match acquire_single_instance() {
        Ok(Instance::Primary) => match run(parsed, app_config) {
            Ok(code) => code,
            Err(err) => {
                output::global().user_line(&format!("错误：{err}"));
                ExitCode::from(EXIT_RUNTIME_ERROR)
            }
        },
        Ok(Instance::Existing) => {
            // FR-32：重复启动 ⇒ 唤起已有窗口（有 UI 的实例）；无窗口（`--no-gui`）则只提示
            let activated = ui::try_activate_existing_window();
            let message = if activated {
                "已有实例在运行：已唤起它的主窗口（单实例互斥体同一时刻只允许一个）"
            } else {
                "已有实例在运行（可能是 --no-gui 形态，没有窗口可唤起）：请先退出它"
            };
            output::global().user_line(message);
            ExitCode::from(EXIT_ALREADY_RUNNING)
        }
        Err(message) => {
            output::global().user_text(
                "azusa-local-proxy 启动被拒",
                &format!("启动被拒：{message}"),
            );
            ExitCode::from(EXIT_RUNTIME_ERROR)
        }
    }
}

/// 自环预检；命中 ⇒ 打印改法并返回退出码 2（归入"参数错误"，不新增退出码）。
fn ensure_not_self_loop(config: &config::Config) -> Result<(), ExitCode> {
    match cli::check_self_loop(
        &config.upstream_base,
        &config.listen_host,
        config.listen_port,
    ) {
        Ok(check) if check.is_loop => {
            let message = output::loop_refusal_text(
                &config.upstream_base,
                &check.upstream_key,
                &check.listen_key,
            );
            output::global().user_text("启动被拒绝（自环保护）", &message);
            Err(ExitCode::from(EXIT_BAD_ARGS))
        }
        Ok(_) => Ok(()),
        Err(err) => {
            output::global().user_text("azusa-local-proxy 参数错误", &format!("错误：{err}"));
            Err(ExitCode::from(EXIT_BAD_ARGS))
        }
    }
}

/// 参数错误出口：错误文本 + 用法（三通道各自呈现），退出码 2。
fn fail_args(message: &str) -> ExitCode {
    output::global().user_text("azusa-local-proxy 参数错误", message);
    output::global().print_usage();
    ExitCode::from(EXIT_BAD_ARGS)
}

fn run(parsed: cli::Cli, app_config: config::Config) -> Result<ExitCode, String> {
    let exe_dir = exe_dir()?;
    let level = logging::Level::parse(&app_config.logging.level)
        .ok_or_else(|| format!("logging.level 非法：{}", app_config.logging.level))?;
    // P6-S5 / D12：日志**只在内存**（2,000 条环形缓冲）—— 不建目录、不开文件、不写盘。
    let logger = Arc::new(logging::Logger::new(level));

    let version = env!("CARGO_PKG_VERSION");
    logger.info(&format!(
        "AzusaAI 本地反代 v{version} 启动（exe 目录：{}）",
        exe_dir.display()
    ));
    match osver::current() {
        Some(version) => logger.info(&format!(
            "运行 OS：{} {}（深色标题栏{}）",
            version.name(),
            version.display(),
            if version.supports_dark_title_bar() {
                "可用"
            } else {
                "按版本闸跳过"
            }
        )),
        None => logger
            .warn("无法经 RtlGetVersion 取到 OS 版本：按最保守形态运行（不启用任何新系统增强）"),
    }

    // 生效值公示（去配置文件后的补偿；P6-S5 起日志去向 = 内存环形缓冲）。
    logger.info(&format!(
        "生效参数：监听 {}:{}（回退 {}）→ {} | 关闭={} | 日志=内存({} 条) | 启动即服务={}",
        app_config.listen_host,
        app_config.listen_port,
        app_config
            .port_fallback
            .iter()
            .map(|port| port.to_string())
            .collect::<Vec<_>>()
            .join(","),
        app_config.upstream_base_trimmed(),
        app_config.ui.close_action,
        logging::LOG_RING_CAPACITY,
        !parsed.start_paused,
    ));

    // FR-35：默认启动即服务；`--start-paused` 是"本次启动是否立即起服务"的进程语义
    // （K13：`Config` 里原「启动即开始服务」字段已随自启功能删除 ⇒ 这里直接读 `Cli::start_paused`）。
    let start_service = !parsed.start_paused;

    let controller = Arc::new(service::Controller::spawn(
        app_config,
        Arc::clone(&logger),
        parsed.listen_port,
    )?);

    let code = if parsed.no_gui || !cfg!(windows) {
        run_headless(&controller, parsed.no_gui, &logger)?
    } else {
        ui::run(Arc::clone(&controller), Arc::clone(&logger), start_service)
    };

    controller.shutdown();
    Ok(code)
}

/// 无界面形态：启动服务，等待 `Ctrl-C` 控制台事件后停机（真正的停机由 `Controller::shutdown` 收尾）。
///
/// **不承诺 `Ctrl-C` 可用**：P6-B3 的实测未能证实到达性（装置无阳性对照，
/// 见 `shared/progress/rust-proxy-p6-b3b-done.md` §1.5 的 `?`）⇒ 脚本 / 无控制台场景请用
/// `Stop-Process -Id <PID>` 停止。
fn run_headless(
    controller: &service::Controller,
    explicit: bool,
    logger: &Arc<logging::Logger>,
) -> Result<ExitCode, String> {
    if explicit {
        logger.info("按 --no-gui（无界面）模式运行");
    } else {
        logger.info("非 Windows 构建：按无界面模式运行（本项目目标平台只有 Windows）");
    }
    // 无控制台且非重定向 ⇒ 用户看不到任何东西：给一次弹窗提示（方案 §2.3 的 must-see 之一）。
    if output::global().channel() == output::Channel::None {
        output::global().user_text(
            "azusa-local-proxy（--no-gui）",
            "无控制台：--no-gui 的输出无处可去。若用于脚本，请把 stdout 重定向到文件；\
             本进程日志仍保留在内存（可经界面查看）。\
             停止请用 Stop-Process -Id <PID>（本形态不承诺 Ctrl-C 停机）。",
        );
    }
    controller.start();
    output::log_line(
        "等待 Ctrl-C 控制台事件（到达性未确定；脚本 / 无控制台请用 Stop-Process -Id <PID> 停止）",
    );
    controller
        .runtime()
        .block_on(async { tokio::signal::ctrl_c().await })
        .map_err(|err| format!("无法注册 Ctrl-C 处理器：{err}"))?;
    logger.info("收到 Ctrl-C：开始优雅停机");
    Ok(ExitCode::SUCCESS)
}

fn exe_dir() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|err| format!("取程序路径失败：{err}"))?;
    exe.parent()
        .map(std::path::Path::to_path_buf)
        .ok_or_else(|| "无法确定程序所在目录（current_exe 无父目录）".to_string())
}

enum Instance {
    Primary,
    Existing,
}

/// FR-32 单实例：`CreateMutexW` 命名互斥体。
///
/// `CreateMutexW` 在"名字已存在"时**成功返回**（不报错），只把 LastError 置成
/// `ERROR_ALREADY_EXISTS` ⇒ 必须**紧接着**读 LastError 才能判出来。
#[cfg(windows)]
fn acquire_single_instance() -> Result<Instance, String> {
    use std::sync::atomic::{AtomicIsize, Ordering};

    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;

    // 句柄**有意不关闭**：进程退出时 OS 释放；主动 CloseHandle 会让单实例当场失效。
    static MUTEX_HANDLE: AtomicIsize = AtomicIsize::new(0);

    let name: Vec<u16> = SINGLE_INSTANCE_MUTEX
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // 第一个参数传 `None` ⇒ 用创建者默认 DACL（只有同一用户能打开）。
    // 这也是 `Win32_Security` feature 的落点：`SECURITY_ATTRIBUTES` 是签名的组成（审查 P1-2）。
    let handle = unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) }
        .map_err(|err| format!("创建单实例互斥体失败：{err}"))?;

    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        return Ok(Instance::Existing);
    }

    MUTEX_HANDLE.store(handle.0 as isize, Ordering::SeqCst);
    Ok(Instance::Primary)
}

/// 非 Windows：本项目目标平台只有 Windows（方案 §5.4 构建矩阵），
/// 这里不写"假实现"占位 —— 直接声明无单实例约束，非 Windows 构建仅供静态检查用。
#[cfg(not(windows))]
fn acquire_single_instance() -> Result<Instance, String> {
    Ok(Instance::Primary)
}
