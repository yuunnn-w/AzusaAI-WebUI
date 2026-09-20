//! UI 宿主（§7.5 / §8.2）：消息循环、GDI+ 生命周期、跨窗共享状态 `UiState`。
//!
//! 主线程 = 消息泵（Win32 硬约束）；服务跑在 `service::Controller` 的运行时线程上。
//! `UiState` 只被 UI 线程触碰（窗口 GWLP_USERDATA 存裸指针）⇒ 无需给 UI 状态加锁；
//! 与服务侧的通信全部经 `Controller`（内部 `mpsc` + 状态快照 `Mutex`）。

pub mod about_text;
pub mod clipboard;
pub mod curve;
pub mod doc_window;
pub mod main_window;
pub mod settings_window;
pub mod theme;
pub mod tray;

use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::GdiPlus::{GdiplusShutdown, GdiplusStartup, GdiplusStartupInput};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetForegroundWindow, IsWindowVisible, MessageBoxW, PostQuitMessage, SendMessageW,
    ShowWindow, MB_ICONINFORMATION, MB_OK, SW_HIDE, SW_SHOWNORMAL, WM_SETFONT,
};

use azusa_local_proxy::logging::{Level, Logger};
use azusa_local_proxy::service::{self, Controller, Phase, Status};
use azusa_local_proxy::stats::{SeriesPoint, Snapshot, WindowSnapshot, WINDOW_SECS};

use theme::Theme;

pub const MAIN_WINDOW_CLASS: &str = "AzusaAI.LocalProxy.MainWindow";
pub const SETTINGS_WINDOW_CLASS: &str = "AzusaAI.LocalProxy.SettingsWindow";
/// 状态刷新消息（与服务侧约定一致：`WM_APP + 1`）。
pub const WM_APP_REFRESH: u32 = service::WM_APP_REFRESH;
/// 设置小窗的"测试上游"结果回投消息（`WM_APP + 3`）。
pub const WM_APP_TEST_RESULT: u32 = 0x8000 + 3;

/// 「复制日志」显示的**行数**上限（方案 §3.3：最近 100 行）。
const LOG_PANEL_LINES: usize = 100;
/// 面板**逐行**字节上限（方案 §3.3：每行 ≤ 200 B；「复制到剪贴板」不受此限，保全文）。
///
/// `pub` 的唯一原因：I2 的文档窗日志模式按**同一口径**截断（`doc_window::LOG_VIEW_LINE_BYTES`
/// = 本常量 ⇒ 视图与复制不会出现两个不同的行截断值）。
pub const LOG_PANEL_LINE_BYTES: usize = 200;

/// A-16 曲线成本诊断行的周期（秒）—— 每周期结算一次"帧数 / 单帧最大调用数 / 单帧最大耗时"。
const CURVE_DIAG_PERIOD_SECS: u64 = 60;

/// FR-39 提示条文案（**写死**，与方案 §6.3 一致：必须同时说清"探针属预期"与"真实对话要改设置"）。
pub const PLAIN_TEXT_NOTICE: &str = "检测到客户端仍在发 text/plain —— 若这是“运行自检”的诊断探针，属预期；若真实对话也如此，请关闭应用的『预检规避兼容模式』";

/// UI 线程独占的共享状态（主窗与设置窗都通过窗口的 GWLP_USERDATA 拿到同一个它）。
pub struct UiState {
    pub hwnd: HWND,
    pub hinst: HINSTANCE,
    pub controller: Arc<Controller>,
    pub logger: Arc<Logger>,
    pub theme: Theme,
    pub gdiplus_ok: bool,
    pub tray: Option<tray::Tray>,
    pub buttons: Option<main_window::MainButtons>,
    pub settings_hwnd: Option<HWND>,
    pub settings_controls: Option<settings_window::SettingsControls>,
    /// 文档窗（I2：教程模式 / 日志模式**共用一个类**；`None` = 未打开）。寿命约定同 `SettingsControls`。
    pub doc_window: Option<doc_window::DocWindowState>,
    pub status: Status,
    pub snapshot: Snapshot,
    /// D1：近窗（120 s）读数快照 —— 面板 4 张卡 / 延迟行的**唯一**数据源（B-4）。
    /// 与 `snapshot` **同帧同一次**读（同一个 `refresh()` 里连读两下 ⇒ 不会出现两帧不一致）。
    pub window: WindowSnapshot,
    /// **曲线的唯一数据出口**（A-13b③：本字段是全 `UiState` **恰一个**时间序列字段）。
    /// 同一个 `refresh()` 里与 `window` 同帧取一次（`stats.window().series(WINDOW_SECS)`，A-13b①）：
    /// 序列在 UI 侧**只读不存历史** ⇒ 不存在"UI 自建采样环"（D1 / R12 的机械面）。
    pub window_series: Vec<SeriesPoint>,
    /// 曲线几何缓冲（§2.5.2：`CurveGeometry` **无状态**，缓冲归调用方持有 ⇒ 每帧 `clear()` 复用，
    /// 绘制路径 **0 次 `Vec` 分配** —— A-16②）。
    pub curve_geometry: curve::CurveGeometry,
    /// 卡片数值的 19/17/15 档字体（21 档 = `theme.font_number`；§2.4.3 的阶梯）。
    pub number_fonts: theme::NumberFonts,
    /// A-16 诊断：本报告周期内的重绘帧数（结算后归零；隐藏到托盘时为 0 = 不重绘的证据）。
    pub curve_frames: u32,
    /// A-16 诊断：本帧绘制曲线发出的 GDI+ 调用数（每帧归零）。
    pub curve_calls: u32,
    /// A-16 诊断：本报告周期内的单帧调用数最大值（判据 ≤ 16 的读数）。
    pub curve_calls_max: u32,
    /// A-16 诊断：本报告周期内的单帧曲线段耗时最大值（µs）—— **机器相关，只作对照读数**。
    pub curve_micros_max: u128,
    /// 上一次「曲线诊断」行的时刻（≈60 s 一条）。
    pub curve_diag_at: Instant,
    /// 上一次打印的托盘 tooltip（变更才记一行 —— D8 的真机读出口）。
    pub tray_tip_last: Option<String>,
    /// B-6：上一次发出「面板口径」行的秒号（同一秒内最多 1 条；跨秒后的第一条发出）。
    pub panel_line_slice: Option<u64>,
    pub close_action: String,
    pub toast: Option<(String, Instant)>,
    pub notice_dismissed_at: Option<Instant>,
    pub minimize_balloon_shown: bool,
    pub first_start_announced: bool,
    pub exiting: bool,
    /// 应用反馈去重键（**快照去重**：`apply_seq + apply_stage + apply_message`）。
    ///
    /// 为什么不是"seq 变化"闸：服务端一次 apply 有 3 条 publish（seq 恒为 N），
    /// 用 seq 当闸 ⇒ 终态帧被当成"没变化"丢掉 ⇒ 结果行永远停在「正在应用…」（P7 诉求 ②）。
    pub last_apply_notice: Option<(u64, service::ApplyStage, Option<String>)>,
    /// 设置窗分段控件的**草稿态**（状态自持：不读 `BM_GETCHECK`/`CB_GETCURSEL`；不持久化）。
    pub draft_cors: usize,
    pub draft_log_level: usize,
    pub draft_close_action: usize,
    /// 「高级选项」是否展开（决定行表可见行与窗口高）。
    pub advanced_open: bool,
    /// 设置窗结果行墨色（三态渲染读它；三个 owner 各自设置）。
    pub result_tone: settings_window::ResultTone,
}

impl UiState {
    fn new(controller: Arc<Controller>, logger: Arc<Logger>, gdiplus_ok: bool) -> UiState {
        let status = controller.status();
        let snapshot = controller.stats().snapshot();
        let window = controller.stats().window().snapshot();
        let window_series = controller.stats().window().series(WINDOW_SECS);
        let close_action = status.config.ui.close_action.clone();
        let theme = Theme::new(theme::system_dpi());
        let number_fonts = theme::NumberFonts::new(&theme.face, theme.scale);
        UiState {
            hwnd: HWND(std::ptr::null_mut()),
            hinst: HINSTANCE::default(),
            controller,
            logger,
            theme,
            gdiplus_ok,
            tray: None,
            buttons: None,
            settings_hwnd: None,
            settings_controls: None,
            doc_window: None,
            status,
            snapshot,
            window,
            window_series,
            curve_geometry: curve::CurveGeometry::new(),
            number_fonts,
            curve_frames: 0,
            curve_calls: 0,
            curve_calls_max: 0,
            curve_micros_max: 0,
            curve_diag_at: Instant::now(),
            tray_tip_last: None,
            panel_line_slice: None,
            close_action,
            toast: None,
            notice_dismissed_at: None,
            minimize_balloon_shown: false,
            first_start_announced: false,
            exiting: false,
            last_apply_notice: None,
            draft_cors: 0,
            draft_log_level: 2,
            draft_close_action: 0,
            advanced_open: false,
            result_tone: settings_window::ResultTone::Plain,
        }
    }

    /// 读一次状态/指标并同步 UI（按钮文字、提示条、托盘图标、气泡）。
    pub fn refresh(&mut self) {
        let previous_phase = self.status.phase;
        let previous_degraded = self.status.degraded;

        self.status = self.controller.status();
        let stats = self.controller.stats();
        // D1 / B-4：面板近窗读数**与累计快照同帧同一次**取（同一 `refresh()` 里连读 ⇒ 不会两帧不一致）。
        self.snapshot = stats.snapshot();
        self.window = stats.window().snapshot();
        // 曲线数据面（A-13b①：本文件里 `.series(` **恰一次**、就在这一行；A-13b③：序列字段恰一个）。
        // 几何**不在这里算**：绘图区尺寸随窗口变（`layout()`）⇒ 由 `paint()` 每帧按当前布局投影，
        // 否则拖窗口后曲线会停在旧比例上（最坏 1 s）。
        self.window_series = stats.window().series(WINDOW_SECS);
        self.close_action = self.status.config.ui.close_action.clone();

        // 按钮：启停热切换的就地文字 + "去设置改端口"只在绑定失败态出现
        if let Some(buttons) = self.buttons.as_ref() {
            let running = self.status.phase == Phase::Running;
            set_button_text(buttons.toggle, if running { "停止" } else { "启动" });
            let show_goto = self.status.phase == Phase::Error && self.status.last_error.is_some();
            unsafe {
                let _ = ShowWindow(
                    buttons.goto_settings,
                    if show_goto {
                        windows::Win32::UI::WindowsAndMessaging::SW_SHOW
                    } else {
                        SW_HIDE
                    },
                );
            }
        }

        // 托盘：图标四态 + tooltip（D8：tooltip 补 `req/s` 与错误数 ⇒ 需要近窗快照）
        let icon_state = tray::icon_state_for(&self.status);
        let tooltip = tray::tooltip_for(&self.status, &self.window);
        if let Some(tray) = self.tray.as_mut() {
            tray.update(icon_state, &tooltip);
        }
        emit_tray_tip(self, &tooltip);

        // 气泡（§7.2：服务异常 / 端口降级 / 首次启动；另加"经托盘启停且主窗不可见"的操作反馈）
        if let Some(tray) = self.tray.as_ref() {
            if self.status.degraded && !previous_degraded {
                let note = self
                    .status
                    .degrade_note
                    .clone()
                    .unwrap_or_else(|| "端口降级".to_string());
                tray.balloon("端口降级", &note, tray::Balloon::Warn);
            }
            if self.status.phase == Phase::Error && previous_phase != Phase::Error {
                let reason = self
                    .status
                    .last_error
                    .clone()
                    .unwrap_or_else(|| "服务异常".to_string());
                tray.balloon("服务异常", &reason, tray::Balloon::Error);
            }
            let just_started =
                previous_phase != Phase::Running && self.status.phase == Phase::Running;
            if just_started {
                if !self.first_start_announced {
                    self.first_start_announced = true;
                    let note = self
                        .status
                        .base_url
                        .clone()
                        .map(|url| format!("服务已启动；应用侧 Base URL 填 {url}"))
                        .unwrap_or_else(|| "服务已启动".to_string());
                    tray.balloon("AzusaAI 本地反代", &note, tray::Balloon::Info);
                } else if !self.window_visible() {
                    tray.balloon("AzusaAI 本地反代", "服务已启动", tray::Balloon::Info);
                }
            }
            let just_stopped =
                previous_phase == Phase::Running && self.status.phase == Phase::Stopped;
            if just_stopped && !self.window_visible() {
                tray.balloon(
                    "AzusaAI 本地反代",
                    "服务已停止（端口已释放）",
                    tray::Balloon::Info,
                );
            }
        }

        // 配置热生效的结果回填（设置窗开着时才写结果行；FR-37/FR-38）
        //
        // **快照去重**（P7 D5/A-3）：键 = `apply_seq + apply_stage + apply_message`。
        // 判据 J-A4 的负例注入面就在这一段：还原成 `if self.status.apply_seq != previous_apply_seq`
        // ⇒ 一次 apply 只回填一帧 ⇒ 终态文本被丢 ⇒ J-A1 必 FAIL（历史缺陷的机制面）。
        let notice = (
            self.status.apply_seq,
            self.status.apply_stage,
            self.status.apply_message.clone(),
        );
        if self.last_apply_notice.as_ref() != Some(&notice) {
            self.last_apply_notice = Some(notice);
            settings_window::refresh_apply_feedback(self);
        }

        // 文档窗（I2）：**复用既有 1 s 节拍**（不新增定时器）；日志模式只追加 + 跟随策略。
        // 内部自带 ≤1 Hz 节流（本函数也会被 `WM_APP_REFRESH` 状态变化消息驱动）。
        doc_window::refresh(self);

        // B-6：面板口径行（debug 档、≈1 Hz、**绕环**）—— 面板文案的机械读出口（J-B2/J-B3）。
        emit_panel_line(self);
        // A-16：曲线成本诊断行（debug 档、≈60 s 一条、**绕环**）—— 调用数/帧数的机械读出口。
        emit_curve_diag(self);

        main_window::invalidate(self);
    }

    /// 地址行（§7.1：始终显示**实际**监听地址 + 上游）。
    pub fn address_line(&self) -> String {
        let upstream = self.status.config.upstream_base_trimmed();
        match self.status.listen_addr.clone() {
            Some(address) => format!("{address}  →  {upstream}"),
            None => format!(
                "未监听（主端口 {}  →  {upstream}）",
                self.status.requested_port
            ),
        }
    }

    /// 「最近错误」行（可点）。返回（文本, 墨色）。
    pub fn error_row(&self) -> (String, u32) {
        if let Some(error) = self.controller.stats().latest_error() {
            let text = if error.count > 1 {
                format!("最近错误  {}  {}  ×{}", error.time, error.line, error.count)
            } else {
                format!("最近错误  {}  {}", error.time, error.line)
            };
            (theme::truncate_line(&text, 62), theme::COLOR_RED)
        } else if let Some(failure) = self.status.last_error.clone() {
            (
                theme::truncate_line(&format!("启动失败  {failure}"), 62),
                theme::COLOR_RED,
            )
        } else {
            ("最近错误  —（暂无）".to_string(), theme::COLOR_INK_FAINT)
        }
    }

    /// FR-39 提示条是否可见（命中过 + 未被本次关闭）。
    pub fn notice_visible(&self) -> bool {
        let seen = self
            .controller
            .notices()
            .last_seen(azusa_local_proxy::proxy::notice::PLAIN_CONTENT_TYPE);
        match (seen, self.notice_dismissed_at) {
            (Some(seen), Some(dismissed)) => seen > dismissed,
            (Some(_), None) => true,
            _ => false,
        }
    }

    pub fn dismiss_notice(&mut self) {
        self.notice_dismissed_at = Some(Instant::now());
        main_window::invalidate(self);
    }

    pub fn active_toast(&self) -> Option<String> {
        self.toast.as_ref().and_then(|(text, at)| {
            if at.elapsed() <= Duration::from_secs(3) {
                Some(text.clone())
            } else {
                None
            }
        })
    }

    pub fn set_toast(&mut self, text: String) {
        self.toast = Some((text, Instant::now()));
    }

    pub fn base_url(&self) -> Option<String> {
        self.status.base_url.clone()
    }

    pub fn window_visible(&self) -> bool {
        !self.hwnd.is_invalid() && unsafe { IsWindowVisible(self.hwnd).as_bool() }
    }

    /// §7.1 停止/启动：就地切换服务状态（不退出程序）。
    pub fn toggle_service(&mut self) {
        match self.status.phase {
            Phase::Running => {
                self.logger
                    .info("用户操作：停止服务（就地切换，不退出程序）");
                self.controller.stop();
            }
            _ => {
                self.logger.info("用户操作：启动服务");
                self.controller.start();
            }
        }
    }

    pub fn open_settings(&mut self) {
        settings_window::open(self);
    }

    /// §7.1「复制 Base URL」：写剪贴板 + 3 秒可见反馈（进度行变色）。
    pub fn copy_base_url(&mut self) {
        match self.base_url() {
            Some(url) => {
                if clipboard::set_text(self.hwnd, &url) {
                    self.logger.info(&format!("已复制 Base URL：{url}"));
                    self.set_toast(format!("已复制 Base URL：{url}"));
                } else {
                    self.logger
                        .warn("复制 Base URL 失败：打开剪贴板失败（可能被其它程序占用）");
                    self.set_toast("复制失败：剪贴板被其它程序占用".to_string());
                }
            }
            None => {
                self.set_toast("服务未运行：还没有 Base URL 可复制".to_string());
            }
        }
        main_window::invalidate(self);
    }

    /// 「查看日志」（**P7-I2 / D2**）：打开文档窗的**日志模式**（定高 + 原生滚动 + 只追加 + 跟随）。
    ///
    /// 旧实现是 `MessageBoxW`：高度随文本量自适应（实测 5 行 496×347 → 45 行 1081×782），文本子控件
    /// 无 `WS_VSCROLL`、`WM_VSCROLL`/`WM_MOUSEWHEEL` 全无效 ⇒ 既无高度上限也无滚动面。现在**不再使用
    /// `MessageBoxW`**；「复制日志到剪贴板」仍是留存日志的唯一出口（剪贴板不是文件）。
    pub fn show_logs(&mut self) {
        doc_window::open(self, doc_window::DocMode::Log);
    }

    /// 「复制日志到剪贴板」（P6-S5 / D12）：留存日志的唯一出口（**剪贴板不是文件**）。
    pub fn copy_logs(&mut self) {
        let text = self.log_text(LOG_PANEL_LINES);
        if clipboard::set_text(self.hwnd, &text) {
            self.logger.info("已复制内存日志到剪贴板（最近 100 行）");
            self.set_toast("已复制日志到剪贴板（最近 100 行）".to_string());
        } else {
            self.logger
                .warn("复制日志失败：打开剪贴板失败（可能被其它程序占用）");
            self.set_toast("复制失败：剪贴板被其它程序占用".to_string());
        }
        main_window::invalidate(self);
    }

    /// 剪贴板文本：头行（共 N 条 / 上限 / 已挤出）+ 最近 `max_lines` 行（**不截断**，保全文）。
    fn log_text(&self, max_lines: usize) -> String {
        let (lines, dropped) = self.logger.snapshot_lines(max_lines);
        let mut text = format!(
            "日志（纯内存环形缓冲，不写磁盘）：共 {} 条 / 上限 {}；已挤出 {} 条；本视图最近 {} 行\n\
             ────────────────────────────────────────────────\n",
            self.logger.retained_count(),
            azusa_local_proxy::logging::LOG_RING_CAPACITY,
            dropped,
            lines.len()
        );
        if lines.is_empty() {
            text.push_str("（暂无日志）\n");
            return text;
        }
        for line in &lines {
            text.push_str(line);
            text.push('\n');
        }
        text
    }

    /// §7.1「关于」→ **使用教程**（P7-I2 / D2：打开文档窗的教程模式；8 章 + 章节跳转 + 可复制）。
    ///
    /// 旧实现是 `MessageBoxW`（488 × 317 DIP、318 字符、不可滚动不可复制）。版本 / 运行 OS / 深色
    /// 标题栏三行按 **Open-6** 移到教程**第八章末「环境」小段**（由 `ui::about_text` 现算）。
    pub fn show_about(&mut self) {
        doc_window::open(self, doc_window::DocMode::Tutorial);
    }

    /// §7.1「最近错误」详情（最多 10 条，最新在前）。
    pub fn show_recent_errors(&mut self) {
        let errors = self.controller.stats().recent_errors();
        let text = if errors.is_empty() {
            "（暂无错误记录）".to_string()
        } else {
            errors
                .iter()
                .map(|entry| format!("{}  {}  ×{}", entry.time, entry.line, entry.count))
                .collect::<Vec<_>>()
                .join("\n")
        };
        message_box(
            self.hwnd,
            "最近错误（最新在前，最多 10 条）",
            &text,
            MB_ICONINFORMATION,
        );
    }

    /// **P6-S7 / D2**：关闭主窗 ⇒ 隐藏到托盘（首次给一次气泡，D11-e：托盘图标**不摘**）。
    ///
    /// 调用面收窄（P6-S7 / D1）：**只有**关闭按钮（`main_window.rs::WM_CLOSE`）与托盘左键/
    /// 菜单的反向动作（`toggle_window_visibility`）可调用本函数 —— **最小化不再走这里**，
    /// 最小化交系统默认进任务栏。
    pub fn hide_to_tray(&mut self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
        if !self.minimize_balloon_shown {
            self.minimize_balloon_shown = true;
            if let Some(tray) = self.tray.as_ref() {
                tray.balloon_minimized();
            }
        }
    }

    /// 托盘左键：可见且在前台 ⇒ 隐藏；否则显示并聚焦（避免"点一下反而消失"的恼人行为）。
    pub fn toggle_window_visibility(&mut self) {
        if self.window_visible() {
            let foreground = unsafe { GetForegroundWindow() };
            if foreground == self.hwnd {
                self.hide_to_tray();
            } else {
                main_window::show_and_focus(self.hwnd);
            }
        } else {
            main_window::show_and_focus(self.hwnd);
        }
    }

    /// 弹托盘菜单（返回选中的菜单 ID；0 = 未选）。
    pub fn show_tray_menu(&mut self) -> usize {
        match self.tray.as_ref() {
            Some(tray) => tray.show_menu(&self.status),
            None => 0,
        }
    }

    /// 托盘菜单命令分发（WM_APP_TRAY 与 WM_COMMAND 共用）。
    pub fn run_tray_command(&mut self, id: usize) {
        match tray::command_from_id(id) {
            tray::TrayCommand::ShowWindow => main_window::show_and_focus(self.hwnd),
            tray::TrayCommand::ToggleService => self.toggle_service(),
            tray::TrayCommand::CopyBaseUrl => self.copy_base_url(),
            tray::TrayCommand::ViewLogs => self.show_logs(),
            tray::TrayCommand::CopyLogs => self.copy_logs(),
            tray::TrayCommand::About => self.show_about(),
            tray::TrayCommand::Exit => self.request_exit(),
            tray::TrayCommand::None => {}
        }
    }

    /// 真正退出：移走托盘图标 + 结束消息循环（服务停机由 `main` 里的 `Controller::shutdown` 收尾）。
    pub fn request_exit(&mut self) {
        if self.exiting {
            return;
        }
        self.exiting = true;
        self.logger
            .info("退出：停止服务 → 等在途请求收尾 → 释放端口（托盘图标已移除）");
        if let Some(tray) = self.tray.as_mut() {
            tray.remove();
        }
        unsafe {
            PostQuitMessage(0);
        }
    }

    /// 系统注销/关机：只做能做的（移除图标），不阻塞会话结束。
    pub fn prepare_session_end(&mut self) {
        if let Some(tray) = self.tray.as_mut() {
            tray.remove();
        }
    }

    /// DPI 变化（WM_DPICHANGED）：重建字体（含卡片数值阶梯）+ 重排按钮（§7.5 第 1 条）。
    pub fn apply_dpi(&mut self, dpi: u32) {
        self.theme.rebuild(dpi);
        self.number_fonts.delete();
        self.number_fonts = theme::NumberFonts::new(&self.theme.face, self.theme.scale);
        if let Some(buttons) = self.buttons.as_ref() {
            for hwnd in [
                buttons.toggle,
                buttons.goto_settings,
                buttons.settings,
                buttons.view_logs,
                buttons.copy_logs,
                buttons.copy_base,
                buttons.about,
            ] {
                if !hwnd.is_invalid() {
                    unsafe {
                        SendMessageW(
                            hwnd,
                            WM_SETFONT,
                            Some(WPARAM(self.theme.font_ui.0 as usize)),
                            Some(LPARAM(1)),
                        );
                    }
                }
            }
        }
        self.logger.info(&format!(
            "DPI 变化：{dpi}（缩放 {:.2}×；字体与布局已重建）",
            dpi as f32 / 96.0
        ));
        main_window::sync_buttons(self);
    }

    /// 退出前的资源清理（托盘图标 / 文档窗 / 字体画刷）。
    fn shutdown(&mut self) {
        if let Some(tray) = self.tray.as_mut() {
            tray.destroy();
        }
        self.tray = None;
        // 文档窗若还开着 ⇒ 先销毁（它吃 `theme` 的字体，必须在 `theme.delete()` 之前）
        doc_window::destroy(self);
        // 卡片数值阶梯字体（19/17/15 档）先于 `theme.delete()` 释放（A-10 的 GDI 基线法）
        self.number_fonts.delete();
        self.theme.delete();
    }
}

/// B-6 面板口径行：面板文案（`panel_texts`）的**机械读出口**
/// （`--log-level debug` + stdout 重定向即可读到；GUI 模式同样有效）。
///
/// 两条硬约束：
/// ① **≈1 Hz 显式节流**：`UiState::refresh()` 同时被 1 s `WM_TIMER` 与状态变化消息
///    （`WM_APP_REFRESH`）驱动 ⇒ 不节流会随状态抖动（同一秒最多 1 条，跨秒后的第一条发出）；
/// ② **必须绕环**：直写 `output::log_line`，不经 `Logger::log` ⇒ 不入环、不计入 `written`
///    （1 条/秒进 2000 行环会在 ~33 分钟内把环吃光并污染"已挤出"读数，D6/J-C4 不变量）。
fn emit_panel_line(ui: &mut UiState) {
    if !ui.logger.enabled(Level::Debug) {
        return;
    }
    let slice = ui.controller.stats().window().slice_now();
    if ui.panel_line_slice == Some(slice) {
        return;
    }
    ui.panel_line_slice = Some(slice);
    let texts = main_window::panel_texts(&ui.window, &ui.snapshot);
    azusa_local_proxy::output::log_line(&format!(
        "面板口径：{} | {} | 请求={} 活跃={} 预检={} 错误={} | 窗口={}s 样本={} 丢弃={}",
        texts.latency,
        texts.traffic,
        ui.window.requests,
        ui.snapshot.active.max(0),
        ui.window.preflight,
        ui.window.errors,
        ui.window.secs,
        ui.window.latency_samples,
        ui.window.dropped_samples,
    ));
}

/// A-16 曲线成本诊断行（debug 档、≈[`CURVE_DIAG_PERIOD_SECS`] 一条、**绕环**）。读数口径：
/// - `帧=` 本周期内的重绘帧数 —— **隐藏到托盘时为 0** ⇒「停止重绘」是机械读数而不是推断；
/// - `最大调用=` 单帧曲线段发出的 GDI+ 调用数最大值（判据 ≤ 16；结构上 = `7 + 段数`）；
/// - `段=` / `点数=` 绘制输入的形状（段数由数据决定：`None` 处断开，见 `ui/curve.rs`）；
/// - `帧耗时最大=` **机器相关的对照读数**（数值门归档口径：不作 PASS/FAIL 依据）。
fn emit_curve_diag(ui: &mut UiState) {
    if ui.curve_diag_at.elapsed() < Duration::from_secs(CURVE_DIAG_PERIOD_SECS) {
        return;
    }
    ui.curve_diag_at = Instant::now();
    let frames = std::mem::take(&mut ui.curve_frames);
    let calls_max = std::mem::take(&mut ui.curve_calls_max);
    let micros = std::mem::take(&mut ui.curve_micros_max);
    if !ui.logger.enabled(Level::Debug) {
        return;
    }
    let segments = if ui.curve_geometry.is_empty() {
        0
    } else {
        ui.curve_geometry.seg_breaks.len() + 1
    };
    azusa_local_proxy::output::log_line(&format!(
        "曲线诊断：帧={frames} 最大调用={calls_max} 段={segments} 点数={} 帧耗时最大={micros}µs",
        ui.curve_geometry.rate_points.len(),
    ));
}

/// D8 的真机读出口：托盘 tooltip **内容变更**时记一行（`--log-level debug`；绕环 ⇒ 不污染日志环、
/// 不计入"已挤出"读数）。读数串本身来自 `tray::tooltip_for`（唯一真源）。
fn emit_tray_tip(ui: &mut UiState, tooltip: &str) {
    if ui.tray_tip_last.as_deref() == Some(tooltip) {
        return;
    }
    ui.tray_tip_last = Some(tooltip.to_string());
    if !ui.logger.enabled(Level::Debug) {
        return;
    }
    azusa_local_proxy::output::log_line(&format!("托盘提示：{tooltip}"));
}

fn set_button_text(hwnd: HWND, text: &str) {
    if hwnd.is_invalid() {
        return;
    }
    let wide = theme::wide(text);
    unsafe {
        let _ =
            windows::Win32::UI::WindowsAndMessaging::SetWindowTextW(hwnd, PCWSTR(wide.as_ptr()));
    }
}

/// 「日志（内存）」读数行文案 —— **唯一真源**（设置窗读数行与 I2 的日志窗模式共用同一串）。
pub fn log_stat_text(retained: usize, capacity: usize, dropped: u64) -> String {
    format!("共 {retained} / 上限 {capacity} 条（已挤出 {dropped}）")
}

/// 按**字节**截断（落点必须落在 UTF-8 字符边界上）；返回 `(切片, 是否截断)`。
///
/// `pub(crate)`：I2 的文档窗日志模式按同一口径截断每条日志行（`doc_window::chunk_text`）。
pub(crate) fn clip_bytes(text: &str, limit: usize) -> (&str, bool) {
    if text.len() <= limit {
        return (text, false);
    }
    let mut cut = limit;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    (&text[..cut], true)
}

fn message_box(
    owner: HWND,
    title: &str,
    text: &str,
    icon: windows::Win32::UI::WindowsAndMessaging::MESSAGEBOX_STYLE,
) {
    let title = theme::wide(title);
    let text = theme::wide(text);
    unsafe {
        let _ = MessageBoxW(
            Some(owner),
            PCWSTR(text.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | icon,
        );
    }
}

/// 首帧显示主窗（`SW_SHOWNORMAL` + 立即重画一帧，避免"白板窗口"闪烁）。
fn show_main_window(hwnd: HWND) {
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        let _ = windows::Win32::Graphics::Gdi::UpdateWindow(hwnd);
    }
}

/// §7.1 深色标题栏（Win10 1809+；Win7/8/8.1 与更早 Win10 一律跳过）。
///
/// **动态取** `dwmapi.dll!DwmSetWindowAttribute`（`GetProcAddress`）：本函数不产生任何静态导入
/// ⇒ Analyzer 的静态导入面不受影响，Win7 上因版本闸**一次都不调用**（指南 §5.3 / 方案 §5.2）。
/// 属性号先试 20（正式常量，20H1 起），失败再试 19（1809–1903 的早期编号）；两者都失败只记日志。
fn apply_dark_title_bar(hwnd: HWND, logger: &Arc<Logger>) {
    let supported = azusa_local_proxy::osver::current()
        .map(|version| version.supports_dark_title_bar())
        .unwrap_or(false);
    if !supported {
        logger.info("深色标题栏：按版本闸跳过（OS 低于 Win10 1809）");
        return;
    }
    unsafe {
        use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
        let module = match LoadLibraryW(windows::core::w!("dwmapi.dll")) {
            Ok(module) => module,
            Err(err) => {
                logger.warn(&format!(
                    "深色标题栏：加载 dwmapi.dll 失败（{err}）—— 外观增强，跳过"
                ));
                return;
            }
        };
        let Some(proc) = GetProcAddress(module, windows::core::s!("DwmSetWindowAttribute")) else {
            logger.info("深色标题栏：dwmapi.dll 无 DwmSetWindowAttribute —— 按跳过处理");
            return;
        };
        type SetAttribute =
            unsafe extern "system" fn(HWND, u32, *const core::ffi::c_void, u32) -> i32;
        let set_attribute: SetAttribute = std::mem::transmute(proc);
        let value: i32 = 1; // BOOL TRUE
        let pointer = &value as *const i32 as *const core::ffi::c_void;
        let mut hr = set_attribute(hwnd, 20, pointer, 4);
        if hr != 0 {
            hr = set_attribute(hwnd, 19, pointer, 4);
        }
        if hr == 0 {
            logger.info("深色标题栏：已启用");
        } else {
            logger.warn(&format!(
                "深色标题栏：设置失败（HRESULT 0x{:08X}）—— 外观增强，不影响功能",
                hr as u32
            ));
        }
    }
}

/// UI 主入口（阻塞到消息循环结束）。返回进程退出码。
/// `start_service` = 本次启动是否立即起服务（FR-35；传 `--start-paused` 时为 `false`）。
pub fn run(controller: Arc<Controller>, logger: Arc<Logger>, start_service: bool) -> ExitCode {
    unsafe {
        // GDI+ 初始化（失败 ⇒ 圆角退化为直角，功能不受影响；报告里如实登记）
        let mut token: usize = 0;
        let input = GdiplusStartupInput {
            GdiplusVersion: 1,
            ..Default::default()
        };
        let gdiplus_ok = GdiplusStartup(&mut token, &input, std::ptr::null_mut()).0 == 0;
        if !gdiplus_ok {
            logger.warn("GDI+ 初始化失败：圆角卡片/抗锯齿退化为直角（功能不受影响）");
        }

        main_window::init_common_controls();
        // windows 0.62 起 `HMODULE` 与 `HINSTANCE` 是两个类型 ⇒ 显式转换一次
        let hinst: HINSTANCE = GetModuleHandleW(PCWSTR::null()).unwrap_or_default().into();

        if let Err(err) = main_window::register_class(hinst, &logger) {
            logger.error(&err);
            if gdiplus_ok {
                GdiplusShutdown(token);
            }
            return ExitCode::from(1);
        }
        if let Err(err) = settings_window::register_class(hinst, &logger) {
            logger.error(&err);
            if gdiplus_ok {
                GdiplusShutdown(token);
            }
            return ExitCode::from(1);
        }
        // 文档窗类（I2：教程 / 日志两模式共用；**不注册全局类**）
        if let Err(err) = doc_window::register_class(hinst, &logger) {
            logger.error(&err);
            if gdiplus_ok {
                GdiplusShutdown(token);
            }
            return ExitCode::from(1);
        }

        let mut ui = Box::new(UiState::new(
            Arc::clone(&controller),
            Arc::clone(&logger),
            gdiplus_ok,
        ));
        ui.hinst = hinst;
        let pointer = &mut *ui as *mut UiState;
        let hwnd = match main_window::create(hinst, pointer) {
            Ok(hwnd) => hwnd,
            Err(err) => {
                logger.error(&err);
                if gdiplus_ok {
                    GdiplusShutdown(token);
                }
                return ExitCode::from(1);
            }
        };
        ui.hwnd = hwnd;
        controller.set_notify_hwnd(hwnd.0 as isize);
        // §7.1 的深色标题栏：版本闸 + 动态取 dwmapi（Win7 上零调用、零静态导入）
        apply_dark_title_bar(hwnd, &logger);
        ui.tray = Some(tray::Tray::create(
            hwnd,
            hinst,
            tray::icon_state_for(&ui.status),
            &tray::tooltip_for(&ui.status, &ui.window),
            &logger,
        ));
        ui.refresh();
        // FR-35：默认启动即服务；传 `--start-paused` 时 main 给 `start_service = false`（到界面点「启动」）。
        if start_service {
            controller.start();
        }
        // 首帧：主窗按默认形态显示（隐藏到托盘 = 用户动作 —— 关闭按钮（D2）/ 托盘左键，不是启动默认；最小化进任务栏 D1）
        show_main_window(hwnd);

        main_window::message_loop(pointer);
        main_window::destroy(hwnd);
        ui.shutdown();
        if gdiplus_ok {
            GdiplusShutdown(token);
        }
        ExitCode::SUCCESS
    }
}

/// FR-32：重复启动时唤起已有实例的主窗（找不到 ⇒ `false`，调用方给出文字提示）。
pub fn try_activate_existing_window() -> bool {
    let class_name = theme::wide(MAIN_WINDOW_CLASS);
    unsafe {
        match FindWindowW(PCWSTR(class_name.as_ptr()), PCWSTR::null()) {
            Ok(hwnd) if !hwnd.is_invalid() => {
                main_window::show_and_focus(hwnd);
                true
            }
            _ => false,
        }
    }
}
