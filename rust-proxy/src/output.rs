//! 三种用户可见输出通道（D6；方案 §2.3）。
//!
//! GUI 子系统（无控制台窗口）后，`println!` / `eprintln!` 不再可靠。本模块在 `main()` 最前面
//! **初始化一次**（`init()`），之后所有用户可见文本与日志行都经它，按下面的顺序判定通道：
//!
//! 1. `GetStdHandle(STD_OUTPUT_HANDLE)` + `GetFileType`；`t ∈ {PIPE, DISK}` ⇒ **[`Channel::Redirected`]**
//!    —— 直接 `WriteFile` 到继承句柄（**程序不打开任何文件**；文件由父进程/脚本创建）。
//! 2. 否则 `AttachConsole(ATTACH_PARENT_PROCESS)` 成功（或已附着 = `ERROR_ACCESS_DENIED`）⇒
//!    **[`Channel::Console`]** —— 用 [`open_console_device()`] 重开 `CONOUT$` / `CONERR$` 设备句柄
//!    （**不**依赖 std 的 `stdout()` 单例：`SetStdHandle` 与 Rust 内部 `OnceLock` 初始化次序有坑）。
//! 3. 以上都不成立 ⇒ **[`Channel::None`]** —— 普通日志丢弃（仍进内存环形缓冲）；
//!    **用户必须看到的文本**（帮助 / 参数错误 / 自环拒绝 …）改走 `MessageBoxW`。
//!
//! ## K9 白名单（本批**唯一**合法的设备句柄打开点）
//!
//! 两处控制台设备句柄打开调用 **只允许**出现在 [`open_console_device()`] 的函数体内，字面量
//! **只能是** `CONOUT$` / `CONERR$`。打开的是**设备**（`GetFileType` 返回 `FILE_TYPE_CHAR`）
//! 而非文件 ⇒ 不产生任何文件读写。任何在该函数体**之外**的同类调用 ⇒ 判缺陷（方案 §12.1-D12-1）。

use std::sync::OnceLock;

/// 程序版本（唯一来源；`--version` 与帮助标题都读它）。
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 编译目标三元组（`build.rs` 注入；缺失时退回 `"windows"`）。仅用于 `--version` 展示。
pub const BUILD_TARGET: &str = match option_env!("AZUSA_BUILD_TARGET") {
    Some(target) => target,
    None => "windows",
};

/// 输出通道（判定顺序见模块头）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    /// 附着到父控制台（或启动时已有控制台）：写 `CONOUT$` / `CONERR$` 设备句柄。
    Console,
    /// stdout 被重定向到管道/文件：写继承句柄（**程序不打开文件**）。
    Redirected,
    /// 无控制台且非重定向：日志丢弃；用户必须看到的文本走弹窗。
    None,
}

/// `AttachConsole` 的结局（注入用；真实判定在 [`detect`]）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attach {
    /// `AttachConsole` 成功。
    Ok,
    /// 已附着控制台（`ERROR_ACCESS_DENIED`）⇒ 等价成功。
    AlreadyAttached,
    /// 无父控制台 / 其它失败。
    Failed,
}

/// `FILE_TYPE_PIPE`（`GetFileType` 的返回值常量，注入用）。
pub const FILE_TYPE_PIPE: u32 = 0x0003;
/// `FILE_TYPE_DISK`。
pub const FILE_TYPE_DISK: u32 = 0x0001;

/// **通道分类纯函数**（可单测；真实判定把 `GetStdHandle`/`GetFileType`/`AttachConsole` 的结果
/// 作为参数注入）。判定顺序与模块头一致：重定向优先，其次控制台，最后无通道。
pub fn classify(file_type: u32, attach: Attach) -> Channel {
    if file_type == FILE_TYPE_PIPE || file_type == FILE_TYPE_DISK {
        Channel::Redirected
    } else {
        match attach {
            Attach::Ok | Attach::AlreadyAttached => Channel::Console,
            Attach::Failed => Channel::None,
        }
    }
}

/// 当前进程的输出器（进程生命周期内单例）。
pub struct Output {
    channel: Channel,
    /// 原始句柄（`isize` 以便 `Output: Sync` 放进 `OnceLock`）；0 = 无效。
    stdout_handle: isize,
    stderr_handle: isize,
}

static GLOBAL: OnceLock<Output> = OnceLock::new();

/// 在 `main()` 最前面调用一次（幂等）。返回进程级输出器。
pub fn init() -> &'static Output {
    GLOBAL.get_or_init(Output::detect)
}

/// 取进程级输出器（未显式 `init()` 也会惰性初始化）。
pub fn global() -> &'static Output {
    init()
}

impl Output {
    /// 真实通道判定（只做一次）。
    fn detect() -> Output {
        #[cfg(windows)]
        {
            use windows::Win32::Foundation::{
                GetLastError, ERROR_ACCESS_DENIED, INVALID_HANDLE_VALUE,
            };
            use windows::Win32::Storage::FileSystem::{GetFileType, FILE_TYPE_UNKNOWN};
            use windows::Win32::System::Console::{
                AttachConsole, GetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE,
                STD_OUTPUT_HANDLE,
            };

            let std_out =
                unsafe { GetStdHandle(STD_OUTPUT_HANDLE) }.unwrap_or(INVALID_HANDLE_VALUE);
            let file_type = if std_out.is_invalid() {
                FILE_TYPE_UNKNOWN
            } else {
                unsafe { GetFileType(std_out) }
            };
            let attach = match unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } {
                Ok(()) => Attach::Ok,
                Err(_) => {
                    if unsafe { GetLastError() } == ERROR_ACCESS_DENIED {
                        Attach::AlreadyAttached
                    } else {
                        Attach::Failed
                    }
                }
            };
            let channel = classify(file_type.0, attach);
            let (stdout_handle, stderr_handle) = match channel {
                Channel::Console => (
                    handle_to_isize(open_console_device(ConsoleDevice::Out)),
                    handle_to_isize(open_console_device(ConsoleDevice::Err)),
                ),
                Channel::Redirected => {
                    let out = handle_to_isize(std_out);
                    let err = unsafe { GetStdHandle(STD_ERROR_HANDLE) }
                        .map(handle_to_isize)
                        .unwrap_or(out);
                    (out, err)
                }
                Channel::None => (0, 0),
            };
            Output {
                channel,
                stdout_handle,
                stderr_handle,
            }
        }
        #[cfg(not(windows))]
        {
            // 本项目目标平台只有 Windows；非 Windows 构建仅供静态检查。
            Output {
                channel: Channel::Redirected,
                stdout_handle: 0,
                stderr_handle: 0,
            }
        }
    }

    /// 通道读数（诊断/单测用）。
    pub fn channel(&self) -> Channel {
        self.channel
    }

    /// 普通日志行：写选定通道；`None` 通道丢弃（仍进内存环形缓冲）。
    pub fn log_line(&self, text: &str) {
        self.write(false, text);
    }

    /// 用户必须看到的**单行**（错误 / 版本 / 提醒）：`None` 通道改走弹窗。
    pub fn user_line(&self, text: &str) {
        self.user_text("azusa-local-proxy", text);
    }

    /// 用户必须看到的**文本块**（帮助 / 自环拒绝 / 参数错误）：`None` 通道改走弹窗。
    pub fn user_text(&self, title: &str, text: &str) {
        if self.channel == Channel::None {
            self.message_box(title, text);
        } else {
            self.write(false, text);
        }
    }

    /// 帮助：有控制台/重定向时输出全文；无控制台时弹窗显示精简版。
    pub fn print_usage(&self) {
        if self.channel == Channel::None {
            self.message_box("azusa-local-proxy 用法", &popup_usage_text());
        } else {
            self.write(false, &usage_text());
        }
    }

    /// 版本：三通道都可见。
    pub fn print_version(&self) {
        self.user_line(&version_text());
    }

    fn write(&self, to_stderr: bool, text: &str) {
        if self.channel == Channel::None {
            return;
        }
        let mut line = String::with_capacity(text.len() + 1);
        line.push_str(text);
        if !line.ends_with('\n') {
            line.push('\n');
        }
        let handle = if to_stderr {
            self.stderr_handle
        } else {
            self.stdout_handle
        };
        write_handle(handle, line.as_bytes());
    }

    fn message_box(&self, title: &str, text: &str) {
        #[cfg(windows)]
        {
            use windows::core::PCWSTR;
            use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONWARNING, MB_OK};
            let caption = wide(title);
            let body = wide(&truncate_for_popup(text));
            unsafe {
                let _ = MessageBoxW(
                    None,
                    PCWSTR(body.as_ptr()),
                    PCWSTR(caption.as_ptr()),
                    MB_OK | MB_ICONWARNING,
                );
            }
        }
        #[cfg(not(windows))]
        {
            let _ = title;
            let _ = text;
        }
    }
}

/// 日志行入口（`logging.rs` 用；未初始化也能工作）。
pub fn log_line(text: &str) {
    global().log_line(text);
}

// ---------------------------------------------------------------------------
// 用户可见文本常量（方案 §2.2 / §3.5）
// ---------------------------------------------------------------------------

/// 帮助全文（方案 §2.2 原稿；标题版本号 `__VER__`、默认上游 `__DEFAULT__` 由 [`usage_text()`]
/// 在**运行期**替换 —— K11-a：`--help` 必须打印生效的默认值，不得硬编码字面量）。
const USAGE_TEMPLATE: &str = r#"AzusaAI 本地反向代理 v__VER__（Win7 SP1+ · 便携单文件 · 无配置文件 · 零文件读写）

用法：azusa-local-proxy.exe --upstream <上游地址> [选项]

⚠ 上游与监听不能同址（同址 = "转发给自己"的自环 ⇒ 拒绝启动）：
  本程序不写任何配置文件，也不预置任何内网地址。默认上游 __DEFAULT__ 与
  默认监听 127.0.0.1:80 不同址 ⇒ 不带参数可直接启动（上游默认指向本机 8080 的模型服务）。
  （判定只看"上游的 host:port"与"实际监听的 host:port"是否相同：把上游配成与实际监听同址
   才会被拒绝。）
  若你的模型服务不在本机 8080，请用 --upstream 指定真实上游，例如：
    内网部署   azusa-local-proxy.exe --upstream http://<内网模型主机>
    本机测试   azusa-local-proxy.exe --upstream http://127.0.0.1:8080

默认值（未另指定时）：
  监听   127.0.0.1:80                80 被占则按 --port-fallback 回退，界面会显示实际地址
  上游   __DEFAULT__       ← 与默认监听不同址 ⇒ 可直接启动；请改成你的模型服务地址
  客户端 Base URL 填：http://127.0.0.1/v1
  关闭按钮 = 最小化到托盘；最小化按钮 = 进任务栏
  日志 = 只在内存（最近 2000 条），不写磁盘；不写注册表、不提供开机自启

本程序运行期不读取、不写入任何文件（日志也只在内存，退出即消失）。
  要留存日志请在界面里点「查看日志」→「复制到剪贴板」。
  改默认值用下面的参数（两种写法都行：--参数 值 / --参数=值）：

监听与转发
  --listen-host <ip>          只接受回环地址（默认 127.0.0.1；非回环一律拒绝）
  --listen-port <n>           监听端口 1-65535（默认 80）；别名 --port
  --port-fallback <列表>      被占用时的候选端口，逗号分隔（默认 8000,18080；none = 不回退）
  --upstream <url>            上游 http:// 地址（默认 __DEFAULT__；示例 http://<内网模型主机>；
                              不支持 https）
  --strip-prefix <路径>       剥掉监听侧路径前缀（默认空 = 不动；例：/proxy）

超时（单位毫秒）
  --connect-timeout-ms <ms>           连接建立超时，默认 10000（不允许 0）
  --response-head-timeout-ms <ms>     等上游响应头的上限，默认 120000；0 = 无限等（不推荐）
  --first-byte-timeout-ms <ms>        响应头之后、首帧之前的看门狗，默认 0 = 不设

连接池与 CORS
  --pool-max-idle-per-host <n>        每个主机的空闲连接上限，默认 16
  --pool-idle-timeout-ms <ms>         空闲连接保留期，默认 10000
  --cors-mode <*|star|echo>           跨源头模式，默认 *（star 是 * 的别名，等价）
  --cors-max-age <秒>                 预检 Max-Age，默认 600

日志（只在内存，不写磁盘）
  --log-level <error|warn|info|debug> 默认 info

启动与退出
  --start-paused              启动后不自动开始服务（到界面点「启动」）；与 --no-gui 互斥（同给 ⇒ 退出码 2）
  --close-action <tray|exit>  关闭按钮行为：tray = 最小化到托盘（默认）；exit = 直接退出

其它
  --no-gui                    无界面运行（脚本 / 排障）；与 --start-paused 互斥（同给 ⇒ 退出码 2）
  --version, -V               显示版本
  -h, --help                  显示本帮助

说明
  · 本程序运行期不打开、不读取、不写入任何文件（日志只在内存）。本程序**不写注册表、
    不提供开机自启**，也没有任何持久化写入点。
  · 自环保护：--upstream 的 host:port 与"实际监听"的 host:port（含端口回退后的实际口）
    相同 ⇒ 拒绝启动并提示你改用哪个参数（退出码 2）。
  · 没有控制台时（例如从快捷方式启动）：本帮助与参数错误改用弹窗显示。
  · 输出被重定向到文件/管道时（脚本场景）：本帮助与日志照常写入该文件，不丢行。
  · --no-gui 的停止方式以实测为准（见方案 §8-P2-G）；实测前本文不承诺 Ctrl-C 可用。
  · 退出码：0 正常 / 1 运行期错误 / 2 参数错误 / 3 已有实例在运行（已尝试唤起其窗口）"#;

/// 帮助全文（三种呈现共用**同一份**，不会文案漂移）。
/// 版本号与默认上游都在**运行期**填（K11-a：两个变体的帮助文本必须各自如实）。
pub fn usage_text() -> String {
    USAGE_TEMPLATE
        .replace("__VER__", APP_VERSION)
        .replace("__DEFAULT__", crate::config::default_upstream())
}

/// 无控制台时的**精简版**弹窗文本（≤ 约 1,500 字符 / ≤ 45 行；方案 §2.3）。
pub fn popup_usage_text() -> String {
    let default = crate::config::default_upstream();
    format!(
        "azusa-local-proxy v{ver}\n\
         用法：azusa-local-proxy.exe --upstream <上游地址> [选项]\n\n\
         ⚠ 上游与监听不能同址（同址 = 自环 ⇒ 拒绝启动）\n\
         \x20 默认上游 {default} 与默认监听 127.0.0.1:80 不同址 ⇒ 不带参数可直接启动；\n\
         \x20 上游默认指向本机 8080 的模型服务。若不在本机 8080，请用 --upstream 指定真实上游：\n\
         \x20   内网部署   azusa-local-proxy.exe --upstream http://<内网模型主机>\n\
         \x20   本机测试   azusa-local-proxy.exe --upstream http://127.0.0.1:8080\n\
         \x20 （判定只看\"上游 host:port\"与\"实际监听 host:port\"是否相同。）\n\n\
         默认值（未另指定时）：\n\
         \x20 监听   127.0.0.1:80（80 被占则按 --port-fallback 回退）\n\
         \x20 上游   {default}     ← 与默认监听不同址 ⇒ 可直接启动；请改成你的模型服务地址\n\
         \x20 客户端 Base URL 填：http://127.0.0.1/v1\n\n\
         常用参数\n\
         \x20 --listen-port <n>           监听端口（默认 80）；别名 --port\n\
         \x20 --upstream <url>            上游 http:// 地址（不支持 https）\n\
         \x20 --port-fallback <列表>      被占用时的候选端口（默认 8000,18080；none = 不回退）\n\
         \x20 --strip-prefix <路径>       剥掉监听侧路径前缀（默认空）\n\
         \x20 --connect-timeout-ms <ms>   连接建立超时（默认 10000）\n\
         \x20 --cors-mode <*|star|echo>   跨源头模式（默认 *）\n\
         \x20 --log-level <error|warn|info|debug>  日志级别（默认 info）\n\
         \x20 --no-gui                    无界面运行\n\n\
         本程序不读、不写任何文件（日志只在内存，查看走界面「查看日志」→「复制到剪贴板」）。\n\
         完整参数一览：azusa-local-proxy.exe --help（有控制台或重定向时输出全文）；亦可查 rust-proxy/README.md",
        ver = APP_VERSION
    )
}

/// `--version` 文本（方案 §2.1：`azusa-local-proxy <ver> (<target>)`）。
pub fn version_text() -> String {
    format!("azusa-local-proxy {APP_VERSION} ({BUILD_TARGET})")
}

/// 自环拒绝启动文案（方案 §3.5；三通道共用同一份，**含"怎么改"且不含任何内网 IP**）。
///
/// `upstream_key` / `listen_key` 由 `cli::is_self_loop` 的单点归一产出（如 `127.0.0.1:80`）。
pub fn loop_refusal_text(upstream: &str, upstream_key: &str, listen_key: &str) -> String {
    format!(
        "启动被拒绝：上游 {upstream}（{upstream_key}）与本程序实际监听地址 {listen_key} 相同，\
         会形成\"转发给自己\"的自环。请用 --upstream 指向真实上游，例如：\
         --upstream http://<内网模型主机>（内网部署）或 --upstream http://127.0.0.1:8080（本机测试）。"
    )
}

/// 弹窗文本兜底截断（`MessageBoxW` 对超长文本会截断/难读；上限标 `?`，本实现保守取 4,000 字符）。
fn truncate_for_popup(text: &str) -> String {
    const LIMIT: usize = 4_000;
    if text.chars().count() <= LIMIT {
        return text.to_string();
    }
    let mut out: String = text.chars().take(LIMIT).collect();
    out.push('…');
    out
}

/// 宽字符串（NUL 结尾）。
#[cfg(windows)]
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn handle_to_isize(handle: windows::Win32::Foundation::HANDLE) -> isize {
    handle.0 as isize
}

/// 写继承/设备句柄（`WriteFile`）。`handle == 0` ⇒ 视为无效，丢弃。
fn write_handle(handle: isize, bytes: &[u8]) {
    if handle == 0 {
        return;
    }
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::Storage::FileSystem::WriteFile;
        let handle = HANDLE(handle as *mut core::ffi::c_void);
        unsafe {
            let _ = WriteFile(handle, Some(bytes), None, None);
        }
    }
    #[cfg(not(windows))]
    {
        use std::io::Write;
        let _ = handle;
        let _ = std::io::stdout().write_all(bytes);
    }
}

/// 控制台设备（K9 白名单内的两个字面量之一）。
#[cfg(windows)]
#[derive(Clone, Copy)]
enum ConsoleDevice {
    Out,
    Err,
}

/// **K9 白名单**：本批唯一允许打开控制台设备句柄的函数体，字面量只能是 `CONOUT$` / `CONERR$`。
///
/// 返回值 = 设备句柄，打开失败退回 `INVALID_HANDLE_VALUE`（`isize` 化后由调用方按 0/无效处理）。
///
/// `#[rustfmt::skip]`：D12-1 判定式要求"两处命中行须各含设备字面量" ⇒ 两行须保持单行形态
/// （`fmt` 默认会按宽度换行、把字面量挤到下一行，使 ③ 误红）。
#[cfg(windows)]
#[rustfmt::skip]
fn open_console_device(device: ConsoleDevice) -> windows::Win32::Foundation::HANDLE {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows::Win32::Storage::FileSystem as fs;
    use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
    let acc = 0x8000_0000_u32 | 0x4000_0000_u32; // GENERIC_READ | GENERIC_WRITE
    let shr = FILE_SHARE_READ | FILE_SHARE_WRITE;
    let crt = OPEN_EXISTING;
    let flg = FILE_ATTRIBUTE_NORMAL;
    match device {
        ConsoleDevice::Out => unsafe {
            let h = fs::CreateFileW(PCWSTR(wide("CONOUT$").as_ptr()), acc, shr, None, crt, flg, None);
            h.unwrap_or(INVALID_HANDLE_VALUE)
        },
        ConsoleDevice::Err => unsafe {
            let h = fs::CreateFileW(PCWSTR(wide("CONERR$").as_ptr()), acc, shr, None, crt, flg, None);
            h.unwrap_or(INVALID_HANDLE_VALUE)
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 通道分类纯函数（注入式判定；定点复查点名的软点）。
    #[test]
    fn classify_prefers_redirect_then_console() {
        // 重定向优先：管道/磁盘无论 attach 结局如何都是 Redirected
        assert_eq!(
            classify(FILE_TYPE_PIPE, Attach::Failed),
            Channel::Redirected
        );
        assert_eq!(classify(FILE_TYPE_PIPE, Attach::Ok), Channel::Redirected);
        assert_eq!(
            classify(FILE_TYPE_DISK, Attach::Failed),
            Channel::Redirected
        );
        // 非重定向：附着成功/已附着 ⇒ Console
        assert_eq!(classify(0x0002, Attach::Ok), Channel::Console);
        assert_eq!(classify(0x0002, Attach::AlreadyAttached), Channel::Console);
        assert_eq!(classify(0x0000, Attach::Ok), Channel::Console);
        // 非重定向且附着失败 ⇒ None
        assert_eq!(classify(0x0000, Attach::Failed), Channel::None);
        assert_eq!(classify(0x0002, Attach::Failed), Channel::None);
    }

    /// 帮助全文：**正向**钉子（方案 §4-S1 / §6.4-T1）。
    #[test]
    fn usage_text_has_required_needles() {
        let text = usage_text();
        for needle in [
            "http://127.0.0.1/v1",
            "http://<内网模型主机>",
            "自环",
            "复制到剪贴板",
            "不写磁盘",
            "star",
            "-V",
        ] {
            assert!(text.contains(needle), "帮助全文必须含 {needle}");
        }
        // 头部标题按 §2.2（**不是**"必须显式给出上游"的旧心智模型，P1-1）
        assert!(text.contains("上游与监听不能同址"));
        assert!(!text.contains("必须显式"));
    }

    /// 构造「私有网段形态」的 needle —— **运行期分段拼接**，源码里任一常量都不构成完整地址
    /// （方案 §0.0 纪律：字面量写法会让断言自身成为 D13-1 的新命中）。
    fn private_prefix_needles() -> [String; 3] {
        let first = ["1", "0"].join("");
        let second = ["1", "9", "2"].join("");
        let third = ["1", "7", "2"].join("");
        [
            format!("{first}."),
            format!("{second}.168."),
            format!("{third}.16."),
        ]
    }

    /// K11-a / K12：`--help` 必须打印**生效的**默认上游（运行期取，两个变体各自如实）。
    #[test]
    fn usage_text_shows_runtime_default_upstream() {
        let default = crate::config::default_upstream();
        let text = usage_text();
        assert!(
            text.contains(default),
            "帮助全文必须含生效默认上游 {default}"
        );
        assert!(text.contains("默认上游"), "帮助全文必须讲清默认上游");
        assert!(popup_usage_text().contains(default));
        // 旧文案（"默认配置即同址 ⇒ 拒绝启动"）不得残留（K12 后不再成立）
        for stale in ["默认配置即同址", "必须显式"] {
            assert!(!text.contains(stale), "K12 后不得再有旧自环文案：{stale}");
        }
    }

    /// 帮助全文：**负向**钉子（D12/D13 回退闸）。
    #[test]
    fn usage_text_has_no_forbidden_needles() {
        let text = usage_text();
        // ① 已删除的参数面（D12-4）
        let removed = [
            ["--log", "-dir"].concat(),
            ["--log", "-file"].concat(),
            ["--log", "-retain-files"].concat(),
            ["--dump", "-icons"].concat(),
        ];
        for needle in &removed {
            assert!(
                !text.contains(needle.as_str()),
                "帮助全文不得含已删除参数 {needle}"
            );
        }
        // ② 私有网段形态：D13-6 的对象是**源码与示例**（"默认值改了、示例没改"的半清状态）。
        //    内网版的默认值**本身**就是内网地址（来自 gitignored 源、绝不入库）⇒ 先把"生效默认值"
        //    从待断言的文本里摘掉，再断言其余部分零私有形态；开源版（未注入）等于全文断言。
        let text = match option_env!("AZUSA_DEFAULT_UPSTREAM") {
            None => text,
            Some(injected) => text.replace(injected, ""),
        };
        for prefix in private_prefix_needles() {
            assert!(
                !text.contains(prefix.as_str()),
                "帮助全文不得含私有网段形态 {prefix}"
            );
        }
    }

    /// 自环拒绝文案：含"自环"与改法，且不含任何私有网段形态。
    #[test]
    fn loop_refusal_text_is_actionable_and_clean() {
        let text = loop_refusal_text("http://127.0.0.1", "127.0.0.1:80", "127.0.0.1:80");
        assert!(text.contains("自环"));
        assert!(text.contains("--upstream"));
        assert!(text.contains("http://<内网模型主机>"));
        for prefix in private_prefix_needles() {
            assert!(!text.contains(prefix.as_str()));
        }
    }

    /// 弹窗精简版：含自环警告与 `--upstream` 示例，末行给全文入口。
    #[test]
    fn popup_usage_text_mentions_loop_and_examples() {
        let text = popup_usage_text();
        assert!(text.contains("自环"));
        assert!(text.contains("http://<内网模型主机>"));
        assert!(text.contains("http://127.0.0.1:8080"));
        assert!(text.contains("完整参数一览：azusa-local-proxy.exe --help"));
        assert!(text.chars().count() < 2_000);
    }

    /// 版本串形态。
    #[test]
    fn version_text_shape() {
        let text = version_text();
        assert!(text.starts_with("azusa-local-proxy "));
        assert!(text.contains(APP_VERSION));
        assert!(text.starts_with(&format!("azusa-local-proxy {APP_VERSION} (")));
        assert!(text.ends_with(')'));
    }
}
