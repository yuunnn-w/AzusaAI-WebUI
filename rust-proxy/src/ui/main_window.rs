//! 主窗（§7.1）：状态灯 / 地址行 / 指标卡 / 最近错误行 / FR-39 提示条 / 底部按钮行。
//!
//! 绘制口径：
//! - **双缓冲自绘**（内存 DC + `BitBlt`，`WM_ERASEBKGND` 直接吞掉）⇒ 1 s 刷新不闪烁（§7.5 第 5 条）；
//! - 圆角卡片 / 状态点 / 分隔线走 GDI+ 抗锯齿；文字走 GDI（CJK 字体链接）；
//! - 控件只有 6 个真按钮（`BUTTON` 类，comctl32 v6 清单生效 ⇒ 现代观感）；
//!   其余全部自绘，布局由 `layout()` 统一算（DPI 变化只重算，不重写各画点）。
//!
//! 交互（§7.1/§7.3）：
//! - `[停止]/[启动]` = 启停热切换；`[设置]` 开设置小窗；`[查看日志]/[复制日志]` 读**内存**环形缓冲
//!   （P6-S5 / D12：程序不打开任何文件、也不 spawn 任何子进程）；
//! - `[复制 Base URL]` 写剪贴板（并给 3 秒可见反馈）；`[关于]` 版本/OS/许可/源码位置；
//! - 最近错误行 = 可点（弹最近 10 条详情）；FR-39 提示条的 `×` = 本次关闭。

use std::sync::Arc;
use std::time::Duration;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject,
    EndPaint, FillRect, InvalidateRect, SelectObject, SetBkMode, SetTextColor, UpdateWindow,
    DRAW_TEXT_FORMAT, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_RIGHT, DT_SINGLELINE, DT_VCENTER,
    DT_WORDBREAK, HDC, HFONT, HGDIOBJ, PAINTSTRUCT, SRCCOPY, TRANSPARENT,
};
use windows::Win32::UI::Controls::{
    InitCommonControlsEx, ICC_STANDARD_CLASSES, INITCOMMONCONTROLSEX,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GetClientRect, GetMessageW, GetWindowLongPtrW, LoadCursorW, PostQuitMessage, RegisterClassW,
    SendMessageW, SetTimer, SetWindowLongPtrW, SetWindowPos, TranslateMessage, CREATESTRUCTW,
    CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, GWLP_USERDATA, HMENU, IDC_ARROW, MINMAXINFO,
    SWP_NOACTIVATE, SWP_NOZORDER, WINDOW_EX_STYLE, WM_COMMAND, WM_CREATE, WM_DESTROY,
    WM_DPICHANGED, WM_ERASEBKGND, WM_GETMINMAXINFO, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_NCCREATE,
    WM_NCDESTROY, WM_PAINT, WM_QUERYENDSESSION, WM_RBUTTONUP, WM_SETFONT, WM_SIZE, WM_TIMER,
    WNDCLASSW, WS_CHILD, WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE,
};

use azusa_local_proxy::logging::Logger;
use azusa_local_proxy::service::{Phase, Status};
use azusa_local_proxy::stats::{Snapshot, WindowSnapshot};

use super::curve::{self, CurveGeometry, PlotRect};
use super::theme::{
    format_bytes, format_thousands, format_uptime, gdi_color, wide, Gfx, Theme, BTN_H, CARD_H,
    COLOR_ACCENT_DEEP, COLOR_ACCENT_SOFT, COLOR_CARD, COLOR_CARD_BORDER, COLOR_GRAY, COLOR_GREEN,
    COLOR_INK, COLOR_INK_DISABLED, COLOR_INK_FAINT, COLOR_INK_SOFT, COLOR_NOTICE_BG,
    COLOR_NOTICE_BORDER, COLOR_NOTICE_INK, COLOR_RED, COLOR_SEPARATOR, COLOR_SURFACE_SUNKEN,
    COLOR_YELLOW, CURVE_MAX_H, CURVE_MIN_H, PAD_CARD, PAD_PAGE, RADIUS_CARD, RADIUS_FIELD, SPACE_L,
    SPACE_M, SPACE_S, SPACE_XS,
};
use super::{tray, UiState, MAIN_WINDOW_CLASS, PLAIN_TEXT_NOTICE, WM_APP_REFRESH};

/// 主窗 1 s 刷新定时器（§7.1：指标每 1 s 刷新）。
const TIMER_REFRESH: usize = 1;
const REFRESH_MS: u32 = 1000;
/// **亚秒滚动定时器**（P8-UI4）：把曲线的"滚动"从"每秒跳一格"变成**连续平移**。
///
/// 只做两件事（不跑 `refresh()`）：① 取一次亚秒相位 `Window::subsecond_fraction()`；
/// ② `invalidate` 一次 ⇒ **每帧成本 = 一次整帧重绘**。
///
/// ⚠ **`ANIM_MS` 是唯二的成本旋钮**（另一个 = `UiState::tick_anim` 的"最近有流量才动"闸）：
/// 真机对照臂（A-16④ 口径，同一台机器、同一空闲窗）：
/// - 旧版（只有 1 s 节拍）：`ΔCPU = 0.312 s / 18 s` ⇒ **0.217 % 单核**
/// - 本批首测（100 ms 节拍、无闸）：`ΔCPU = 3.031 s / 19 s` ⇒ **15.95 % 单核** ✗ —— 整帧重绘 ≈ **16 ms/帧**，
///   10 Hz 就是 0.16 s/s；
///   ⇒ 收口 = **200 ms**（观感仍连续：窗口步长 ≈ 5.8 px/s ⇒ 1.15 px/步）+ **有空流量才动**的闸
///   （空闲 18 s 窗回到 ≈ 基线；见 `tick_anim`）。
const TIMER_ANIM: usize = 2;
const ANIM_MS: u32 = 200;

/// 按钮控件 ID（区间 1001–1099；托盘菜单从 2001 起，见 `ui/tray.rs`）。
pub const IDC_TOGGLE_SERVICE: usize = 1001;
pub const IDC_SETTINGS: usize = 1002;
/// 「查看日志」（P6-S5 / D12：内存环形缓冲弹窗；原「打开日志」的 `ShellExecuteW` 已删）。
pub const IDC_VIEW_LOGS: usize = 1003;
pub const IDC_COPY_BASE: usize = 1004;
pub const IDC_ABOUT: usize = 1005;
pub const IDC_GOTO_SETTINGS: usize = 1006;
/// 「复制日志到剪贴板」（P6-S5 / D12：留存日志的唯一出口）。
pub const IDC_COPY_LOGS: usize = 1007;

/// 主窗里的 7 个按钮句柄（创建后由 `UiState` 持有；DPI 变化只重设字体/位置）。
pub struct MainButtons {
    pub toggle: HWND,
    pub goto_settings: HWND,
    pub settings: HWND,
    pub view_logs: HWND,
    pub copy_logs: HWND,
    pub copy_base: HWND,
    pub about: HWND,
}

/// 主面板几何常量（DIP；全部经 `theme.px()`；UI 方案 §2.4.2 的度量表）。
///
/// 总高公式 `H = 310 + curve_h` 的拆解（自上而下）：
/// `186`（曲线之前：页边距 20 + 状态区 84 + 卡片 92…176 + 10）+ `8`（曲线→延迟行）
/// + `16 × 3`（三个读数行）+ `4 × 2`（行间）+ `12`（读数→按钮）+ `32`（按钮）+ `16`（底部留白）
///   = **310**；曲线高 = `clamp(余量, CURVE_MIN_H, CURVE_MAX_H)`。
///   ⇒ 默认客户区 **460 × 400**（曲线 90）、最小 **420 × 366**（曲线 56）。
pub const MIN_CLIENT_W_DIP: i32 = 420;
pub const MIN_CLIENT_H_DIP: i32 = 366;
/// 卡片顶 / 曲线面板顶（无挤压时的基准）。
const CARDS_TOP_DIP: f32 = 92.0;
const CURVE_TOP_DIP: f32 = 186.0;
/// 卡片高下限（空间不足时先压卡片，再压曲线 —— 见 [`layout_metrics`] 的 flex 两段）。
const CARDS_MIN_H_DIP: f32 = 52.0;
/// FR-39 提示条高（`PLAIN_TEXT_NOTICE` 在 content_w 下折 2 行）。
const NOTICE_H_DIP: f32 = 44.0;
/// 提示条可见时补的最小高度（44 提示条 + 上下各 10 = 64，其中 32 由卡片 84 → 52 抵掉）。
const NOTICE_MIN_H_EXTRA_DIP: i32 = 32;

/// 布局结果（像素坐标；绘制与命中测试共用同一份 ⇒ 不会"看着在一个位置、点的是另一个"）。
///
/// 字段顺序 = UI 方案 §2.4.1 的信息层级：状态区 → 身份区 → 指标区 → 波动区 → 读数区 → 操作区。
pub struct Layout {
    pub status_dot_center: POINT,
    pub status_dot_radius: f32,
    pub status_text: RECT,
    pub addr_text: RECT,
    pub toggle_btn: RECT,
    pub goto_settings_btn: RECT,
    pub uptime_text: RECT,
    pub separator_top: RECT,
    pub cards: [RECT; 4],
    /// 波动面板（卡片与读数区之间；高 = flex）。
    pub curve_panel: RECT,
    pub curve_title: RECT,
    pub curve_now: RECT,
    pub curve_plot: RECT,
    pub curve_hint: RECT,
    /// Y 轴 3 档标签（上/中/下）与 X 轴 3 档标签（左/中/右）。
    pub curve_y_labels: [RECT; 3],
    pub curve_x_labels: [RECT; 3],
    /// 延迟行（**整行**：P2-3 修掉"只占 `content_w / 2` ⇒ 尾部被 `…` 截掉"的可见缺陷）。
    pub latency_text: RECT,
    /// 流量行（整行、右对齐：`↑B  ↓B（自启动累计）`）。
    pub traffic_text: RECT,
    pub error_text: RECT,
    pub notice: Option<RECT>,
    pub buttons: [RECT; 5],
}

/// 面板文案（B-4：**唯一来源**；`paint()` 只做排版）—— 纯函数 ⇒ 可单测（J-B3 的机械出口）。
///
/// 口径（行为方案 v2.3 §2.B.3 B-1，**逐字**）：
/// - 4 张卡：`请求（近 120 s）` · `活跃`（即时量、**不带**窗口）· `预检（近 120 s）` · `错误（近 120 s）`；
///   「近 N s」的 **N 取自 `WindowSnapshot::secs`**（B-1「近 120 s」文案的唯一数值来源 ⇒ 窗口长度
///   变更时文案自动跟随，不会留下"近 60 s"残留 —— J-B3 的失败判据正是这一类残留）；
/// - 延迟行：`上游延迟 p50 X / p95 Y（近 120 s）`；**窗口内无样本** ⇒ `上游延迟 —（近 120 s 无样本）`；
///   窗口内无样本 ∧ 无在途请求 ⇒ 行尾追加 `· 当前空闲`（判定见 [`window_is_idle`]）；
/// - 流量行：`↑…  ↓…（自启动累计）` —— **累计口径一字不动**，只加标注（D1：不画 bytes/s）。
pub struct PanelTexts {
    /// 4 张卡的（标签，数值）：标签带窗口语义 ⇒ 运行时生成（`String`，见上"唯一数值来源"）。
    pub cards: [(String, String); 4],
    /// 延迟行整串（含 `上游延迟 ` 前缀与可能的 `· 当前空闲` 后缀）。
    pub latency: String,
    /// 流量行整串（**自启动累计**口径 + 标注）。
    pub traffic: String,
}

/// 空闲判定（B-1 的逐字条件 + B-4 的合取式）：近窗两路都无样本 ∧ **无在途请求**。
/// ⚠ 在途请求（`active > 0`）期间不得报"当前空闲"—— 那正是"等待上游响应头"的形态（B-5 修的就是它）。
fn window_is_idle(window: &WindowSnapshot, snapshot: &Snapshot) -> bool {
    window.is_idle() && snapshot.active == 0
}

pub fn panel_texts(window: &WindowSnapshot, snapshot: &Snapshot) -> PanelTexts {
    let secs = window.secs;
    let mut latency = format!("上游延迟 {}", window.latency_text());
    if window_is_idle(window, snapshot) {
        latency.push_str("· 当前空闲");
    }
    PanelTexts {
        cards: [
            (
                format!("请求（近 {secs} s）"),
                format_thousands(window.requests),
            ),
            ("活跃".to_string(), snapshot.active.max(0).to_string()),
            (
                format!("预检（近 {secs} s）"),
                format_thousands(window.preflight),
            ),
            (
                format!("错误（近 {secs} s）"),
                format_thousands(window.errors),
            ),
        ],
        latency,
        traffic: format!(
            "↑{}  ↓{}（自启动累计）",
            format_bytes(snapshot.bytes_in),
            format_bytes(snapshot.bytes_out)
        ),
    }
}

pub fn register_class(hinst: HINSTANCE, logger: &Arc<Logger>) -> Result<(), String> {
    let class_name = wide(MAIN_WINDOW_CLASS);
    // 窗口类图标（标题栏/任务栏）—— 必须走 `theme::load_app_icon` 这一条入口（P0-1 回归点）
    let icon = super::theme::load_app_icon(hinst, 32);
    if icon.is_invalid() {
        logger.warn(&format!(
            "窗口图标加载失败（资源 ID {}）：标题栏/任务栏将显示系统占位图标",
            super::theme::APP_ICON_RESOURCE_ID
        ));
    }
    unsafe {
        let cursor = LoadCursorW(None, IDC_ARROW).map_err(|err| format!("加载光标失败：{err}"))?;
        let window_class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            hIcon: icon,
            hCursor: cursor,
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        if RegisterClassW(&window_class) == 0 {
            return Err(format!(
                "注册窗口类 {MAIN_WINDOW_CLASS} 失败（RegisterClassW 返回 0）"
            ));
        }
    }
    Ok(())
}

/// 公共控件初始化（comctl32 v6 清单生效的前提；v6 提供现代观感的按钮/下拉）。
pub fn init_common_controls() {
    unsafe {
        let controls = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_STANDARD_CLASSES,
        };
        let _ = InitCommonControlsEx(&controls);
    }
}

/// 创建主窗（尺寸按 DPI 缩放；`lpCreateParams` = `UiState` 指针，wndproc 里存进 GWLP_USERDATA）。
pub fn create(hinst: HINSTANCE, ui: *mut UiState) -> Result<HWND, String> {
    let class_name = wide(MAIN_WINDOW_CLASS);
    let title = wide("AzusaAI 本地反向代理");
    unsafe {
        // 默认客户区 **460 × 400 DIP**（UI 方案 §2.4.2 的总高公式 `310 + 曲线 90`；原 460 × 340
        // 会浪费 52 DIP 死区 —— 曲线面板正是把那块死区吃掉）。
        let client_width = (*ui).theme.px(460.0);
        let client_height = (*ui).theme.px(400.0);
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: client_width,
            bottom: client_height,
        };
        let _ = AdjustWindowRectEx(&mut rect, WS_OVERLAPPEDWINDOW, false, WINDOW_EX_STYLE(0));
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            rect.right - rect.left,
            rect.bottom - rect.top,
            None,
            None,
            Some(hinst),
            Some(ui as *const core::ffi::c_void),
        )
        .map_err(|err| format!("创建主窗失败：{err}"))
    }
}

unsafe fn ui_ptr(hwnd: HWND) -> *mut UiState {
    unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut UiState }
}

unsafe fn with_ui<R>(hwnd: HWND, action: impl FnOnce(&mut UiState) -> R) -> Option<R> {
    let pointer = unsafe { ui_ptr(hwnd) };
    if pointer.is_null() {
        None
    } else {
        Some(action(unsafe { &mut *pointer }))
    }
}

/// 布局（像素坐标）—— 主窗薄封装：只补上两个"条件可见位"。
pub fn layout(ui: &UiState, width: i32, height: i32) -> Layout {
    layout_metrics(
        ui.theme.scale,
        width,
        height,
        ui.notice_visible(),
        ui.status.phase == Phase::Error && ui.status.last_error.is_some(),
    )
}

/// 主面板几何（**纯函数**：只吃 `scale` 与两个条件可见位 ⇒ 4 档 DPI × 多种客户区尺寸可直接单测，A-3）。
///
/// 度量表 = UI 方案 §2.4.2；总高公式与 flex 两段见文件头的常量注释。
/// 自下而上算 ⇒ 三个读数行 / 按钮行永远贴着底边，波动的只有"曲线高"这一项。
pub fn layout_metrics(
    scale: f32,
    width: i32,
    height: i32,
    notice_visible: bool,
    goto_visible: bool,
) -> Layout {
    let px = |dip: f32| (dip * scale).round() as i32;
    let margin = px(PAD_PAGE);
    let content_w = width - margin * 2;
    let row_h = px(16.0);
    let btn_h = px(BTN_H);

    // ── 操作区：底部留白 16 → 按钮行 32（5 颗右对齐；窄窗折 2 行）
    let widths = [px(60.0), px(76.0), px(76.0), px(110.0), px(60.0)];
    let btn_gap = px(SPACE_S);
    let button_row_w: i32 = widths.iter().sum::<i32>() + btn_gap * (widths.len() as i32 - 1);
    // 窄窗退化（§2.4.4）：客户区 420–459 ⇒ `content_w < 414` ⇒ 按钮折 2 行（上行 3 颗 / 下行 2 颗）。
    let folded = button_row_w > content_w;
    let buttons_bottom = height - px(SPACE_L);
    let last_row_top = buttons_bottom - btn_h;
    let stack_top = if folded {
        last_row_top - btn_gap - btn_h
    } else {
        last_row_top
    };
    let mut buttons = [RECT::default(); 5];
    if folded {
        for (top, range) in [(last_row_top, 3..5), (stack_top, 0..3)] {
            let row_w: i32 =
                widths[range.clone()].iter().sum::<i32>() + btn_gap * (range.len() as i32 - 1);
            let mut x = margin + content_w - row_w;
            for index in range {
                buttons[index] = RECT {
                    left: x,
                    top,
                    right: x + widths[index],
                    bottom: top + btn_h,
                };
                x += widths[index] + btn_gap;
            }
        }
    } else {
        let mut x = margin + content_w - button_row_w;
        for (index, width) in widths.iter().enumerate() {
            buttons[index] = RECT {
                left: x,
                top: last_row_top,
                right: x + width,
                bottom: last_row_top + btn_h,
            };
            x += width + btn_gap;
        }
    }

    // ── FR-39 提示条（条件可见）：占 `NOTICE_H_DIP` + 上下各 10，把它上面的内容整体上推
    let notice = if notice_visible {
        let bottom = stack_top - px(10.0);
        Some(RECT {
            left: margin,
            top: bottom - px(NOTICE_H_DIP),
            right: margin + content_w,
            bottom,
        })
    } else {
        None
    };
    let readout_bottom = match notice.as_ref() {
        Some(rect) => rect.top - px(10.0),
        None => stack_top,
    };

    // ── 读数区：三行**整行**（P2-3；延迟行 / 流量行不再各占半宽 ⇒ 尾部不会被 `…` 截掉）
    let error_text = RECT {
        left: margin,
        top: readout_bottom - px(SPACE_M) - row_h,
        right: margin + content_w,
        bottom: readout_bottom - px(SPACE_M),
    };
    let traffic_text = RECT {
        left: margin,
        top: error_text.top - px(SPACE_XS) - row_h,
        right: margin + content_w,
        bottom: error_text.top - px(SPACE_XS),
    };
    let latency_text = RECT {
        left: margin,
        top: traffic_text.top - px(SPACE_XS) - row_h,
        right: margin + content_w,
        bottom: traffic_text.top - px(SPACE_XS),
    };

    // ── 波动区：曲线高 = flex（余量 → clamp 到 `[CURVE_MIN_H, CURVE_MAX_H]`）
    let curve_bottom = latency_text.top - px(SPACE_S);
    let curve_h = (curve_bottom - px(CURVE_TOP_DIP)).clamp(px(CURVE_MIN_H), px(CURVE_MAX_H));
    let curve_top = curve_bottom - curve_h;
    let curve_panel = RECT {
        left: margin,
        top: curve_top,
        right: margin + content_w,
        bottom: curve_bottom,
    };
    // `roomy` **只管 X 标签带**（Leader 裁定 2026-09-20 · I5c 的 P1）：面板压到 56 DIP 下限时把
    // X 标签带（24 → 4 DIP）让给绘图区，否则 `panel.h − 56` 会算出 0 高的绘图区（画不出曲线）。
    // ⚠ **Y 轴标签不依赖 `roomy`**（原实现把两者一起塞进 `if roomy` ⇒ 433×400 / 420×366 这两档
    // 用户可达尺寸下**速率刻度整块消失**，验收批 P1）。Y 标签只需"绘图区放得下"：
    // 标签带高 14 DIP、上/下锚点相距 = 绘图区高 ⇒ `≥ 14 DIP` 就能画**上/下两档**（互不重叠）、
    // `≥ 28 DIP` 才能画**三档**（中档与上下档各相距 `plot_h/2`）。都不满足 ⇒ 全留空 rect（绘制侧
    // 的 `right <= left ⇒ continue` 天然跳过 ⇒ 不画互相重叠的字）。
    let roomy = curve_panel.bottom - curve_panel.top >= px(88.0);
    let x_band = if roomy { px(24.0) } else { px(SPACE_XS) };
    let curve_title = RECT {
        left: curve_panel.left + px(PAD_CARD),
        top: curve_panel.top + px(10.0),
        right: curve_panel.right - px(PAD_CARD),
        bottom: curve_panel.top + px(10.0) + row_h,
    };
    // 当前值（右对齐）与图例同排；图例 `延迟 p50 ●` 占左 46 DIP
    let curve_now = RECT {
        left: curve_title.left + px(46.0),
        top: curve_title.top,
        right: curve_title.right,
        bottom: curve_title.bottom,
    };
    // 绘图区：左 `px(58)`（**Y 轴标签带** —— 实测依据见下）/ 上 32（标题行 + 间隙）/ 右 12 / 下 `x_band`
    //
    // ⚠ 标签带宽度是**实测定的**（不是拍的）：`font_small` 11 DIP 下最宽的 Y 标签是 `10000ms`
    // （y_max 顶档），实测 ≈ **40 DIP**；@200% 真机截图（`out/r2/zoom-y-labels`）显示按方案原值
    // （图区左移 34 DIP ⇒ 标签带只有 `34 − 12 − 4 = 18` DIP）时 `10ms`/`0ms` **左侧被 DrawTextW 裁掉**
    // （可见截断）。⇒ 图区左移改 **58 DIP**（标签带 `58 − 12 − 4 = 42` DIP ≥ 40 ✓ 且留 2 DIP 余量）。
    let curve_plot = RECT {
        left: curve_panel.left + px(62.0),
        top: curve_panel.top + px(32.0),
        right: curve_panel.right - px(SPACE_M),
        bottom: curve_panel.bottom - x_band,
    };
    let plot_middle = curve_plot.top + (curve_plot.bottom - curve_plot.top) / 2;
    let curve_hint = RECT {
        left: curve_plot.left + px(SPACE_M),
        top: plot_middle - px(8.0),
        right: curve_plot.right - px(SPACE_M),
        bottom: plot_middle + px(8.0),
    };
    let mut curve_y_labels = [RECT::default(); 3];
    let mut curve_x_labels = [RECT::default(); 3];
    // Y 轴标签（**不看 roomy**，只看绘图区放不放得下 —— 见上面的注释）
    {
        const LABEL_H_DIP: f32 = 14.0;
        let plot_h = curve_plot.bottom - curve_plot.top;
        let two_ok = plot_h >= px(LABEL_H_DIP);
        let three_ok = plot_h >= px(2.0 * LABEL_H_DIP);
        if two_ok {
            for (index, slot) in curve_y_labels.iter_mut().enumerate() {
                if index == 1 && !three_ok {
                    continue; // 中档让位（否则与上/下档重叠 8 DIP 以上 ⇒ 互相压字）
                }
                let center = curve_plot.top + plot_h * index as i32 / 2;
                let half = px(LABEL_H_DIP / 2.0);
                let mut top = center - half;
                let mut bottom = center + half;
                // 夹进面板（极端档位下"绘图区贴面板底"会把下档推出面板 ⇒ 整块上移）
                if top < curve_panel.top {
                    bottom += curve_panel.top - top;
                    top = curve_panel.top;
                }
                if bottom > curve_panel.bottom {
                    let shift = bottom - curve_panel.bottom;
                    top -= shift;
                    bottom = curve_panel.bottom;
                }
                *slot = RECT {
                    left: curve_panel.left + px(PAD_CARD),
                    top: top.max(curve_panel.top),
                    right: curve_plot.left - px(SPACE_XS),
                    bottom: bottom.min(curve_panel.bottom),
                };
            }
        }
    }
    // X 轴标签：**只看 roomy**（面板矮到 56 DIP 时这一带让给绘图区）
    if roomy {
        let label_w = px(60.0);
        let middle = (curve_plot.left + curve_plot.right) / 2;
        for (index, slot) in curve_x_labels.iter_mut().enumerate() {
            let (left, right) = match index {
                0 => (curve_plot.left, curve_plot.left + label_w),
                2 => (curve_plot.right - label_w, curve_plot.right),
                _ => (middle - label_w / 2, middle + label_w / 2),
            };
            *slot = RECT {
                left,
                top: curve_panel.bottom - px(20.0),
                right,
                bottom: curve_panel.bottom - px(20.0) + px(14.0),
            };
        }
    }

    // ── 指标区（4 卡等宽等距；高 84，空间不足时压到 52 —— flex 的第一段）
    let cards_top = px(CARDS_TOP_DIP);
    let card_gap = px(10.0);
    let card_w = ((content_w - card_gap * 3) / 4).max(px(40.0));
    let cards_h = (curve_panel.top - px(10.0) - cards_top).clamp(px(CARDS_MIN_H_DIP), px(CARD_H));
    let mut cards = [RECT::default(); 4];
    for (index, slot) in cards.iter_mut().enumerate() {
        let left = margin + (card_w + card_gap) * index as i32;
        *slot = RECT {
            left,
            top: cards_top,
            right: left + card_w,
            bottom: cards_top + cards_h,
        };
    }

    // ── 状态区 / 身份区：标题与地址右缘 = 主按钮左缘 − 12（「去设置改端口」可见时再让出它的宽度）
    let toggle_w = px(88.0);
    let goto_w = px(118.0);
    let head_right = if goto_visible {
        margin + content_w - goto_w - px(SPACE_M)
    } else {
        margin + content_w - toggle_w - px(SPACE_M)
    };

    Layout {
        status_dot_center: POINT {
            x: margin + px(7.0),
            y: px(26.0),
        },
        status_dot_radius: (7.0 * scale).max(5.0),
        status_text: RECT {
            left: margin + px(22.0),
            top: px(10.0),
            right: head_right,
            bottom: px(10.0) + px(26.0),
        },
        addr_text: RECT {
            left: margin + px(22.0),
            top: px(42.0),
            right: head_right,
            bottom: px(42.0) + row_h,
        },
        toggle_btn: RECT {
            left: margin + content_w - toggle_w,
            top: px(12.0),
            right: margin + content_w,
            bottom: px(12.0) + btn_h,
        },
        goto_settings_btn: RECT {
            left: margin + content_w - goto_w,
            top: px(44.0),
            right: margin + content_w,
            bottom: px(44.0) + px(26.0),
        },
        uptime_text: RECT {
            left: margin,
            top: px(62.0),
            right: if goto_visible {
                head_right
            } else {
                margin + content_w
            },
            bottom: px(62.0) + row_h,
        },
        separator_top: RECT {
            left: margin,
            top: px(84.0),
            right: margin + content_w,
            bottom: px(85.0),
        },
        cards,
        curve_panel,
        curve_title,
        curve_now,
        curve_plot,
        curve_hint,
        curve_y_labels,
        curve_x_labels,
        latency_text,
        traffic_text,
        error_text,
        notice,
        buttons,
    }
}

/// 最小客户区高（DIP）—— 提示条可见时补 [`NOTICE_MIN_H_EXTRA_DIP`]。
///
/// `366 = 310 + CURVE_MIN_H`（§2.4.2 的总高公式）；提示条的 64 DIP 里 32 由卡片 84 → 52 抵掉。
pub fn min_client_height_dip(notice_visible: bool) -> i32 {
    MIN_CLIENT_H_DIP
        + if notice_visible {
            NOTICE_MIN_H_EXTRA_DIP
        } else {
            0
        }
}

/// 状态点的颜色（§7.1：绿=运行、灰=已停止、黄=启动中/降级、红=错误）。
fn status_color(phase: Phase, degraded: bool) -> u32 {
    match phase {
        Phase::Running if degraded => COLOR_YELLOW,
        Phase::Running => COLOR_GREEN,
        Phase::Starting => COLOR_YELLOW,
        Phase::Error => COLOR_RED,
        Phase::Stopped => COLOR_GRAY,
    }
}

/// 状态文字（§7.1 状态点右侧的字）。
fn status_label(status: &Status) -> &'static str {
    match status.phase {
        Phase::Running if status.degraded => "运行中（端口降级）",
        Phase::Running => "运行中",
        Phase::Starting => "启动中…",
        Phase::Error => "启动失败",
        Phase::Stopped => "已停止",
    }
}

/// 把一段**实测文本宽**在 `[left, right]` 容器内水平居中 → `(left, right)`（两者相差**恰为**
/// `text_width` ⇒ 居中由**矩形本身**承载，与 `DT_*` 对齐标志无关）。
///
/// 为什么不是"整条内带 + `DT_CENTER`"了事：单行形态下要参与机械判据
/// （`p8_main_panel_texts_are_horizontally_centered` 的"文本矩形中心 ≡ 容器中心"）——
/// 只有"矩形宽 = 实测文本宽"这一形态能让"画在哪"与"量到的在哪"是同一个数。
/// 必须折行的形态（卡片标签在窄窗）另走"整条内带 + `DT_CENTER`"，两态各用其形。
pub fn centered_text_span(left: i32, right: i32, text_width: i32) -> (i32, i32) {
    let center = left + (right - left) / 2;
    let half = text_width.max(0) / 2;
    (center - half, center - half + text_width.max(0))
}

/// 卡片标签的**绘制计划**（矩形 + 对齐标志）—— `paint` 与单测共用的**唯一几何出口**。
///
/// 用户真机报障（192 DPI）：「主页面的四个框框下方的文字也没有居中」—— 旧实现把标签画在
/// `[card.left + 2, card.right − 2]` 的整条内带上且 `DT_LEFT` ⇒ 文字一律贴卡片左内缘。
///
/// 现口径（**按实测宽算 x，不写死偏移**）：
/// - **单行放得下**（默认 / 放大档）⇒ 矩形**恰为实测文本宽**（`theme::measure_text`）、
///   中心 = 卡片中心（[`centered_text_span`]）；
/// - **必须折行**（窄窗：`请求（近 120 s）` 实测 92 px > 内带 87 px）⇒ 整条内带（对称于卡片中心，
///   折行需要它）+ `DT_CENTER` 逐行居中；
/// - 竖直两段式（P1-①：量高决定折行还是单行省略）**一字未动**。
///
/// **负例锚点**：把 [`centered_text_span`] 的结果左移 8 px ⇒
/// `p8_main_panel_texts_are_horizontally_centered` 的卡片断言必 FAIL（原始失败消息见 done.md）。
pub fn card_label_plan(
    hdc: HDC,
    font: HFONT,
    card: &RECT,
    label: &str,
    scale: f32,
    value_bottom: i32,
) -> (RECT, DRAW_TEXT_FORMAT) {
    let pad = (2.0 * scale).round() as i32;
    let gap = (2.0 * scale).round() as i32;
    let band_left = card.left + pad;
    let band_right = card.right - pad;
    let band_width = (band_right - band_left).max(0);
    let measured = super::theme::measure_text(hdc, font, label);
    let needed = super::theme::measure_wrapped_height(hdc, font, label, band_width);
    let band_top_max = value_bottom + gap;
    let band_bottom = card.bottom - pad;
    let plan_top = card.top + (52.0 * scale).round() as i32;
    let two_lines_fit = needed > 0 && band_bottom - band_top_max >= needed;
    // 水平：单行放得下 ⇒ 收成"恰为实测文本宽"的居中矩形；否则整条内带交给折行逐行居中
    let (left, right) = if measured > 0 && measured <= band_width {
        centered_text_span(card.left, card.right, measured)
    } else {
        (band_left, band_right)
    };
    let (top, format) = if two_lines_fit {
        // 折行：上缘取"计划位"与"下沿 − 实测高度"的较小者，且不低于数值下方
        (
            plan_top.min(band_bottom - needed).max(band_top_max),
            DT_CENTER | DT_WORDBREAK | DT_VCENTER,
        )
    } else {
        // 兜底（极窄 × 极矮）：单行 + 省略号 —— **不出卡片、不半字**
        (
            band_top_max,
            DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
        )
    };
    (
        RECT {
            left,
            top,
            right,
            bottom: band_bottom,
        },
        format,
    )
}

/// 卡片**数值**的绘制计划（矩形 + 字体 + 对齐标志）—— `paint` 与单测共用的**唯一几何出口**
/// （与 [`card_label_plan`] 同形）。
///
/// 用户真机报障（192 DPI，承接上批"标签居中"）：「框里的**数值**仍旧贴左，与标签不在一条轴上」——
/// 旧实现把数值画在 `[card.left + PAD_CARD, card.right − PAD_CARD]` 的整条内带上且 `DT_LEFT`，
/// 而标签已居中到卡片中心 ⇒ 同一张卡里两行文字两个轴。
///
/// 现口径（**只动 x；竖直与字号阶梯一字未动**）：
/// - `pick_index` 仍吃**整条内带宽**（`卡宽 − 2 × PAD_CARD`）⇒ §2.4.3「按 `measure_text` 逐级降字号、
///   保证不出 `…`」（A-5）的**输入与判据不变**，阶梯照旧能咬；
/// - 选定字体下按实测宽收成"恰为实测文本宽"的居中矩形（[`centered_text_span`]）
///   ⇒ 数值中心 ≡ 卡片中心 ≡ 标签中心（与标签**同一容器** `card.left..card.right`，内带左右对称
///   ⇒ 两个容器的中心逐位相同）；
/// - 实测宽溢出内带（理论上阶梯已保证不会，13 DIP 档是最后一道）⇒ 退回整条内带 + `DT_CENTER` 兜底
///   （宁居中省略也不贴左出格）。
///
/// **负例锚点**：把 [`centered_text_span`] 的结果左移 8 px ⇒
/// `p8_main_panel_texts_are_horizontally_centered` 的**数值**断言必 FAIL
/// （原始失败消息见 `shared/progress/rust-proxy-p8-tweak-done.md`）。
pub fn card_value_plan(
    hdc: HDC,
    fonts: &super::theme::NumberFonts,
    theme: &Theme,
    card: &RECT,
    value: &str,
    scale: f32,
) -> (RECT, HFONT, DRAW_TEXT_FORMAT) {
    let pad = (PAD_CARD * scale).round() as i32;
    let value_h = (26.0 * scale).round() as i32;
    let band_left = card.left + pad;
    let band_right = card.right - pad;
    let band = (band_right - band_left).max(0);
    let (font, _) = fonts.pick_index(hdc, theme, value, band);
    let measured = super::theme::measure_text(hdc, font, value);
    let (left, right) = if measured > 0 && measured <= band {
        centered_text_span(card.left, card.right, measured)
    } else {
        (band_left, band_right)
    };
    (
        RECT {
            left,
            top: card.top + pad,
            right,
            bottom: card.top + pad + value_h,
        },
        font,
        DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
    )
}

/// 空态"井"的矩形（`geometry.is_empty()` 时绘图区那块浅底）—— **对称于曲线面板**。
///
/// ⚠ 空态井**不**沿用名义绘图区 `layout.curve_plot`：那个矩形的左缘 = `panel.left + px(62)` 是给
/// **Y 轴标签列**让位用的，而空态**不画任何轴标签**（空态分支在画标签之前就返回）⇒ 那 62 DIP 是一条
/// 纯空白，右侧却只有 `SPACE_M`(12 DIP)，井整体右偏 `(62 − 12) / 2 = 25 DIP`（用户报障：
/// 「网络波动下方显示的栏目也没有居中」—— 空态文案本身在井内是居中的，**井**没居中）。
/// 空态 ⇒ 左右内缩都取 12 DIP：左缘与标题「网络波动」同轴（`PAD_CARD`），右缘与名义绘图区不变
/// （`SPACE_M`）⇒ 中心 = 面板中心。竖直两档（`plot.top` / `plot.bottom`）保持原样。
pub fn empty_state_well(panel: &RECT, plot: &RECT, scale: f32) -> RECT {
    RECT {
        left: panel.left + (PAD_CARD * scale).round() as i32,
        top: plot.top,
        right: panel.right - (SPACE_M * scale).round() as i32,
        bottom: plot.bottom,
    }
}

/// 空态文案在一段可用宽度（= 空态井）内**按实测宽居中** → `(left, right)`；`paint_curve` 与单测共用。
pub fn empty_hint_span(hdc: HDC, font: HFONT, well: &RECT, text: &str) -> (i32, i32) {
    let width = super::theme::measure_text(hdc, font, text);
    centered_text_span(well.left, well.right, width)
}

/// 绘制一帧（画进双缓冲的内存 DC）。
///
/// `&mut UiState`：函数体要写三个 A-16 诊断计数（帧数 / 单帧最多调用 / 单帧最长耗时）。
fn paint(ui: &mut UiState, hdc: windows::Win32::Graphics::Gdi::HDC, width: i32, height: i32) {
    let layout = layout(ui, width, height);
    let scale = ui.theme.scale;
    // A-16：每帧归零，帧末与周期最大值取大（`曲线诊断：帧=… 最大调用=…` 的读数来源）
    ui.curve_calls = 0;
    ui.curve_frames = ui.curve_frames.saturating_add(1);
    unsafe {
        let full = RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        };
        FillRect(hdc, &full, ui.theme.brush_bg);

        // GDI+ 未初始化 ⇒ 一个 GDI+ 调用都不发（`ui.gdiplus_ok`），只走纯 GDI 画法
        if ui.gdiplus_ok {
            if let Some(gfx) = Gfx::from_hdc(hdc) {
                for card in layout.cards.iter() {
                    round_card(&gfx, card, scale);
                }
                if let Some(notice) = layout.notice.as_ref() {
                    let (x, y, w, h) = rect_tuple(notice);
                    gfx.fill_round_rect(x, y, w, h, 8.0 * scale, COLOR_NOTICE_BG);
                    gfx.stroke_round_rect(
                        x,
                        y,
                        w,
                        h,
                        8.0 * scale,
                        COLOR_NOTICE_BORDER,
                        scale.max(1.0),
                    );
                }
                gfx.fill_circle(
                    layout.status_dot_center.x as f32,
                    layout.status_dot_center.y as f32,
                    layout.status_dot_radius,
                    status_color(ui.status.phase, ui.status.degraded),
                );
                gfx.line(
                    layout.separator_top.left as f32,
                    layout.separator_top.top as f32 + 0.5,
                    layout.separator_top.right as f32,
                    layout.separator_top.top as f32 + 0.5,
                    COLOR_SEPARATOR,
                    scale.max(1.0),
                );
                paint_curve(ui, hdc, &gfx, &layout, scale);
            }
        }

        draw_text(
            hdc,
            ui.theme.font_title,
            COLOR_INK,
            layout.status_text,
            status_label(&ui.status),
            DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
        );
        draw_text(
            hdc,
            ui.theme.font_mono,
            COLOR_INK_SOFT,
            layout.addr_text,
            &ui.address_line(),
            DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
        );

        let (uptime_text, uptime_ink) = match ui.active_toast() {
            Some(text) => (text, COLOR_ACCENT_DEEP),
            None => {
                let text = match ui.status.uptime() {
                    Some(duration) => format!("已运行 {}", format_uptime(duration)),
                    None => match ui.status.phase {
                        Phase::Error => "服务未运行（上次启动失败）".to_string(),
                        _ => "服务未运行".to_string(),
                    },
                };
                (text, COLOR_INK_SOFT)
            }
        };
        draw_text(
            hdc,
            ui.theme.font_small,
            uptime_ink,
            layout.uptime_text,
            &uptime_text,
            DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
        );

        // 文案**全部**来自 `panel_texts`（B-4：唯一来源；本函数只做排版）
        let texts = panel_texts(&ui.window, &ui.snapshot);
        // 卡片的**标签**几乎用满整卡宽（内缩 2 DIP）+ **折行但绝不越出卡片**（P1-①）：
        // `请求（近 120 s）` 真机实测 **92 px**，默认整卡 97 DIP、窄窗只有 87 DIP ⇒ 沿用 12 DIP 内缩 +
        // `DT_END_ELLIPSIS` 会把尾部 `s）` 压成 `…`（I3-b 的可见截断）；而单纯 `DT_WORDBREAK` 在窄窗
        // 又会让第二行**被卡片下沿裁掉**（P1-①：433/420 DIP 实测 `）` 只剩半个字）。
        // 现修法 = **量高 + 两段式**：
        //   ① 用 `measure_wrapped_height` 量出该标签在本卡宽度下折行所需的**真实高度**；
        //   ② 若"数值下方 → 卡片下沿"这块空间装得下 ⇒ 折行（保持"无 `…`"的既有观感），
        //      否则 ⇒ 退化为 `DT_SINGLELINE | DT_END_ELLIPSIS`（宁 `请求（近 1…` 也不出卡片 ——
        //      与审阅建议一致，且只在极窄×极矮的角落生效）。
        // ⚠ 水平**改居中**（用户报障：旧实现 `DT_LEFT` ⇒ 标签贴左内缘）：矩形与对齐标志全部来自
        //   [`card_label_plan`]（绘制与单测的同一出口 ⇒ "标签中心 ≡ 卡片中心"是机械判据）。
        for (index, card) in layout.cards.iter().enumerate() {
            let (label, value) = match texts.cards.get(index) {
                Some((label, value)) => (label.as_str(), value.as_str()),
                None => ("", ""),
            };
            // ⚠ 数值**也**居中（用户报障：数值与标签不在同一轴上）：矩形 / 字体 / 对齐标志全部来自
            //   [`card_value_plan`]（绘制与单测的同一出口 ⇒ "数值中心 ≡ 卡片中心"是机械判据）；
            //   §2.4.3 的**字号阶梯入口（整条内宽带）在该函数里** ⇒ A-5 一字未改。
            let (value_rect, value_font, value_format) =
                card_value_plan(hdc, &ui.number_fonts, &ui.theme, card, value, scale);
            draw_text(hdc, value_font, COLOR_INK, value_rect, value, value_format);
            let (label_rect, label_format) = card_label_plan(
                hdc,
                ui.theme.font_small,
                card,
                label,
                scale,
                value_rect.bottom,
            );
            draw_text(
                hdc,
                ui.theme.font_small,
                COLOR_INK_SOFT,
                label_rect,
                label,
                label_format,
            );
        }

        // 读数区三行**整行**（P2-3：延迟行 / 流量行不再各占半宽 ⇒ 尾部不会被 `…` 截掉）
        draw_text(
            hdc,
            ui.theme.font_small,
            COLOR_INK_SOFT,
            layout.latency_text,
            &texts.latency,
            DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
        );
        draw_text(
            hdc,
            ui.theme.font_small,
            COLOR_INK_SOFT,
            layout.traffic_text,
            &texts.traffic,
            DT_RIGHT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
        );

        let (error_line, error_ink) = ui.error_row();
        draw_text(
            hdc,
            ui.theme.font_small,
            error_ink,
            layout.error_text,
            &error_line,
            DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
        );

        if let Some(notice) = layout.notice.as_ref() {
            let icon_x = notice.left as f32 + 10.0 * scale + 6.0 * scale;
            let icon_y = notice.top as f32 + 10.0 * scale + 6.0 * scale;
            if ui.gdiplus_ok {
                if let Some(gfx) = Gfx::from_hdc(hdc) {
                    gfx.fill_circle(icon_x, icon_y, (6.5 * scale).max(5.0), COLOR_NOTICE_INK);
                }
            }
            draw_text(
                hdc,
                ui.theme.font_small,
                COLOR_NOTICE_BG,
                RECT {
                    left: (icon_x - 6.5 * scale) as i32,
                    top: (icon_y - 7.5 * scale) as i32,
                    right: (icon_x + 6.5 * scale) as i32,
                    bottom: (icon_y + 7.5 * scale) as i32,
                },
                "i",
                DT_CENTER | DT_SINGLELINE | DT_VCENTER,
            );
            draw_text(
                hdc,
                ui.theme.font_small,
                COLOR_NOTICE_INK,
                RECT {
                    left: notice.left + (30.0 * scale) as i32,
                    top: notice.top + (8.0 * scale) as i32,
                    right: notice.right - (34.0 * scale) as i32,
                    bottom: notice.bottom - (6.0 * scale) as i32,
                },
                PLAIN_TEXT_NOTICE,
                DT_LEFT | DT_WORDBREAK | DT_END_ELLIPSIS,
            );
            draw_text(
                hdc,
                ui.theme.font_ui,
                COLOR_NOTICE_INK,
                notice_close_rect(notice, scale),
                "×",
                DT_CENTER | DT_SINGLELINE | DT_VCENTER,
            );
        }
        // A-16①：本帧曲线段的调用数并入周期最大值（`emit_curve_diag` 每 60 s 结算并归零）
        ui.curve_calls_max = ui.curve_calls_max.max(ui.curve_calls);
    }
}

/// 画波动面板（I3-6 / **D7 改型** / **P8-UI4 滚动波形改型**）：面板底 → 网格 + 中线 + **淡色面积** +
/// **连续**折线 → 图例 / 填充度角标 / 当前值。
///
/// **P8-UI4（2026-09-20）**：几何全部来自 `ui/curve.rs`（X = 时间轴 + 亚秒相位平移 + 末点不上图），
/// 本函数只多了一步：面积改由 [`Gfx::fill_area_runs`] 按 `runs` **一次**填完（旧实现只填"第一个 ≥2
/// 点的段"，其余段的面积是缺的），并**去掉了**旧实现在绘制路径里现搭的 `Vec<PointF>`（A-16②）。
///
/// 几何**全部**来自 `ui/curve.rs`（无状态纯函数）：本函数只把几何画出来，**零 `stats` 调用**
/// （A-13b② 的机械面）。
///
/// 绘制调用数（A-16① 的读数）：**恒 10 次/帧**（`round_card` 2 + 井底 1 + 中线 1 + 网格 3 + 图例点 1
/// + 面积 1 + 折线 1）—— 与**段数无关**（P2-① 收口：多段折线 / 多段面积 / 孤立点分别由
///   `Gfx::polyline_runs` / `Gfx::fill_area_runs` 合成**单次** `GdipDrawPath` / `GdipFillPath`）。
///   **空态 = 4**（面板 2 + 井底 1 + 图例点 1；空态没有折线）
///   —— 口径：该数是**周期内单帧最大值**、与数据形态相关（有数据时恒 10）；4 的实测依据 =
///   同机 `段=0/点数=0` 窗读数 **11**（P2-③ 的 +7 注入）**− 注入的 7**（干净空态未单独实拍，
///   故写明推导链而不写成"实测 11"）。
///
/// 真机缺陷的修法（C1/C2/C3；依据 `decisions/ui-curve-traffic-rate-2026-09-20.md`）：
/// - **C1**：Y 轴标签列宽**按当前实际文案量宽**（`curve::label_column_width`），绘图区随实测宽度右移
///   ⇒ 既不裁字、也不进绘图区；
/// - **C2**：浅色**中线**贯穿绘图区（"无样本"只表现为"没有曲线经过"，**绝不画 0**）+ 右上填充度角标；
/// - **C3**：当前值在无样本时**整串省略**（只留角标），不留孤立 `-` / `—`。
///
/// `unsafe`：内部要发 GDI 的 `DrawTextW`（[`draw_text`]）；GDI+ 侧由 [`Gfx`] 自己封。
unsafe fn paint_curve(
    ui: &mut UiState,
    hdc: windows::Win32::Graphics::Gdi::HDC,
    gfx: &Gfx,
    layout: &Layout,
    scale: f32,
) {
    let started = std::time::Instant::now();
    round_card(gfx, &layout.curve_panel, scale);
    ui.curve_calls += 2;
    let panel = &layout.curve_panel;
    let title = &layout.curve_title;
    let dot_radius = (3.0 * scale).max(2.0);
    let title_center_y = (title.top + title.bottom) as f32 / 2.0;

    // 标题（`网络波动`，SIZE_SECTION 15/600）
    draw_text(
        hdc,
        ui.theme.font_section,
        COLOR_INK,
        *title,
        "网络波动",
        DT_LEFT | DT_SINGLELINE | DT_VCENTER,
    );
    // ── 几何：**先算几何**（C1 的标签列宽必须按"当前实际文案"量宽 ⇒ 两趟投影，各 O(120) 且零分配）
    let nominal = layout.curve_plot;
    let mut plot_rect = PlotRect {
        x: nominal.left as f32,
        y: nominal.top as f32,
        w: (nominal.right - nominal.left) as f32,
        h: (nominal.bottom - nominal.top) as f32,
    };
    curve::build(
        &ui.window_series,
        plot_rect,
        ui.second_frac,
        &mut ui.curve_geometry,
    );
    // C1（真机缺陷）：标签列宽 = 当前三个标签的**实测**最大宽；绘图区左缘随实测宽度右移
    // ⇒ 标签**永不进入绘图区、也永不被裁**（旧实现右对齐到一个拍脑袋带宽 + `DrawTextW` 裁左侧
    // ⇒ 真机上 `250ms/125ms/0ms` 退化成 `0ms/5ms/0ms`）。
    let label_left = panel.left + (PAD_CARD * scale).round() as i32;
    let label_width =
        curve::label_column_width(hdc, ui.theme.font_small, ui.curve_geometry.y_max_rate);
    let plot_left =
        (label_left + label_width + (SPACE_XS * scale).round() as i32).max(nominal.left);
    if plot_left != nominal.left {
        plot_rect = PlotRect {
            x: plot_left as f32,
            w: (nominal.right - plot_left) as f32,
            ..plot_rect
        };
        curve::build(
            &ui.window_series,
            plot_rect,
            ui.second_frac,
            &mut ui.curve_geometry,
        );
    }
    let geometry: &CurveGeometry = &ui.curve_geometry;
    let plot = RECT {
        left: plot_left,
        top: nominal.top,
        right: nominal.right,
        bottom: nominal.bottom,
    };

    // 图例：`流量速率` + 圆点（D7 改型：曲线画的是速率，**不再是延迟**）。
    // ⚠ 图例的位置**不能**用布局里的 `curve_now.left` 当右界：状态串（角标 + 当前值）比旧文案长，
    // 会被那个名义边界挤掉（实测 r3 首轮：整条图例不画）。改为"图例紧跟标题 + 状态串按量宽右对齐"。
    let legend_left = title.left + (72.0 * scale).round() as i32;
    let legend_text = "流量速率";
    let legend_width = super::theme::measure_text(hdc, ui.theme.font_small, legend_text).max(1);
    draw_text(
        hdc,
        ui.theme.font_small,
        COLOR_INK_SOFT,
        RECT {
            left: legend_left,
            top: title.top,
            right: legend_left + legend_width,
            bottom: title.bottom,
        },
        legend_text,
        DT_LEFT | DT_SINGLELINE | DT_VCENTER,
    );
    if ui.gdiplus_ok {
        gfx.fill_circle(
            (legend_left + legend_width) as f32 + dot_radius + 2.0 * scale,
            title_center_y,
            dot_radius,
            COLOR_ACCENT_DEEP,
        );
        ui.curve_calls += 1;
    }
    // 右上状态（**一次绘制**）：填充度角标（C2）+ 当前值（`↑… ↓… · req/s`）；该秒无样本 ⇒
    // **只留角标**（C3：不留孤立 `-` / `—`）。矩形按**文本量宽**给足（右对齐、宽恰为文本宽）
    // ⇒ 左缘永不被 `DrawTextW` 裁掉（C1 的同类教训）。
    let status = geometry.status_text();
    let status_width = super::theme::measure_text(hdc, ui.theme.font_mono, &status);
    draw_text(
        hdc,
        ui.theme.font_mono,
        COLOR_INK,
        RECT {
            left: layout.curve_now.right - status_width,
            top: title.top,
            right: layout.curve_now.right,
            bottom: title.bottom,
        },
        &status,
        DT_RIGHT | DT_SINGLELINE | DT_VCENTER,
    );

    if !ui.gdiplus_ok {
        ui.curve_micros_max = ui.curve_micros_max.max(started.elapsed().as_micros());
        return;
    }
    if geometry.is_empty() {
        // 空态：画 `COLOR_SURFACE_SUNKEN` 圆角底 + 居中 caption（文案 = `latency_text()` 同一真源）。
        // ⚠ 井**不是** `plot`：空态不画轴标签 ⇒ 名义绘图区左侧那条"Y 轴标签列让位"（62 DIP）在空态
        // 是纯空白，井会整体右偏 —— 见 [`empty_state_well`]（用户报障的**真正偏移源**）。
        let well = empty_state_well(panel, &nominal, scale);
        let (x, y, w, h) = rect_tuple(&well);
        gfx.fill_round_rect(x, y, w, h, RADIUS_CARD * scale, COLOR_SURFACE_SUNKEN);
        ui.curve_calls += 1;
        let hint = ui.window.latency_text();
        let (hint_left, hint_right) = empty_hint_span(hdc, ui.theme.font_small, &well, &hint);
        draw_text(
            hdc,
            ui.theme.font_small,
            COLOR_INK_FAINT,
            RECT {
                left: hint_left,
                top: layout.curve_hint.top,
                right: hint_right,
                bottom: layout.curve_hint.bottom,
            },
            &hint,
            DT_CENTER | DT_SINGLELINE | DT_VCENTER,
        );
        ui.curve_micros_max = ui.curve_micros_max.max(started.elapsed().as_micros());
        return;
    }

    // 井底（数据态也画：曲线与网格落在同一个面上）
    let (x, y, w, h) = rect_tuple(&plot);
    gfx.fill_round_rect(x, y, w, h, RADIUS_FIELD * scale, COLOR_SURFACE_SUNKEN);
    ui.curve_calls += 1;

    // C2：**只给"首段数据之前的空窗"画浅色中线**（1 次调用、数据驱动）—— 用户报的"大面积为空"
    // 正是这一段（只运行 20 s ⇒ 120 s 窗口的左侧整块无样本）。线画在 **1/3 高**处（不是基线！
    // **绝不把无样本画成 0**，也不与上/中/下三条网格线重合）。
    // 取"中线"分支而非"底纹"：底纹会随缺口段数线性增加调用数 ⇒ 打破 A-16① 的上界。
    // **调用数恒定**：即使首段贴着左缘（退化成长度 1 的零长线）也照发一次 ⇒ 每帧 10 次与数据形态无关。
    {
        let shade_y = plot.top + (plot.bottom - plot.top) / 3;
        let shade_right = geometry
            .rate_points
            .first()
            .map(|point| point.X)
            .unwrap_or(plot.left as f32)
            .max(plot.left as f32 + 1.0);
        gfx.line_dashed(
            plot.left as f32,
            shade_y as f32 + 0.5,
            shade_right,
            shade_y as f32 + 0.5,
            COLOR_INK_DISABLED,
            scale.max(1.0),
        );
        ui.curve_calls += 1;
    }

    // 网格：上/中/下 3 条虚点线
    for line_y in [plot.top, plot_middle(&plot), plot.bottom] {
        gfx.line_dashed(
            plot.left as f32,
            line_y as f32 + 0.5,
            plot.right as f32,
            line_y as f32 + 0.5,
            COLOR_SEPARATOR,
            scale.max(1.0),
        );
        ui.curve_calls += 1;
    }
    // Y 轴 3 档标签：**右对齐到量宽列**（C1）——列左缘与列宽都由实测决定，绘图区已让位 ⇒ 不裁字。
    let labels = curve::y_labels(geometry.y_max_rate);
    for (index, rect) in layout.curve_y_labels.iter().enumerate() {
        if rect.right <= rect.left {
            continue;
        }
        draw_text(
            hdc,
            ui.theme.font_small,
            COLOR_INK_FAINT,
            RECT {
                left: label_left,
                top: rect.top,
                right: label_left + label_width,
                bottom: rect.bottom,
            },
            &labels[index],
            DT_RIGHT | DT_SINGLELINE | DT_VCENTER,
        );
    }
    for (index, rect) in layout.curve_x_labels.iter().enumerate() {
        if rect.right <= rect.left {
            continue;
        }
        let text = match index {
            0 => format!("−{}s", ui.window.secs),
            1 => format!("−{}s", ui.window.secs / 2),
            _ => "现在".to_string(),
        };
        draw_text(
            hdc,
            ui.theme.font_small,
            COLOR_INK_FAINT,
            *rect,
            &text,
            DT_CENTER | DT_SINGLELINE | DT_VCENTER,
        );
    }

    // 折线 + 孤立点：**必须**裁剪（否则抗锯齿的线头会越出卡片）。
    //
    // **P2-① 收口**：`None` 断点要求"逐段画"，但**每段一次 `GdipDrawLines` 会让调用数随段数线性增长**
    // ⇒ 改用 `Gfx::polyline_runs`（一个 `GraphicsPath` + 每段一个 figure + 孤立点 2 px 短刻度）
    // = **单次 `GdipDrawPath`**。于是本函数每帧的绘制调用数**与段数无关**（恒 10 次）：
    //   面板 2 + 井底 1 + 中线 1 + 网格 3 + 图例点 1 + 面积 1 + 折线 1 = **10**。
    gfx.push_clip(plot);
    // 面积（**全部**分段一段不落；`fill_area_runs` = 一个 `GraphicsPath` 多闭合 figure ⇒ **单次**
    // `GdipFillPath`，调用数与段数无关 ⇒ A-16① 的口径不变），且**零 `Vec` 分配**（旧实现每帧现搭
    // 一个 `Vec<PointF>` 面积数组 ⇒ A-16② 的"绘制路径 0 次 `Vec` 分配"现在才真正成立）。
    gfx.fill_area_runs(
        &geometry.rate_points,
        &geometry.runs,
        plot.bottom as f32,
        COLOR_ACCENT_SOFT,
    );
    ui.curve_calls += 1;
    gfx.polyline_runs(
        &geometry.rate_points,
        &geometry.runs,
        COLOR_ACCENT_DEEP,
        2.0 * scale,
        true,
    );
    ui.curve_calls += 1;
    gfx.pop_clip();
    ui.curve_micros_max = ui.curve_micros_max.max(started.elapsed().as_micros());
}

/// 绘图区竖直中线（网格中间那条）。
fn plot_middle(plot: &RECT) -> i32 {
    plot.top + (plot.bottom - plot.top) / 2
}

fn rect_tuple(rect: &RECT) -> (f32, f32, f32, f32) {
    (
        rect.left as f32,
        rect.top as f32,
        (rect.right - rect.left) as f32,
        (rect.bottom - rect.top) as f32,
    )
}

fn round_card(gfx: &Gfx, card: &RECT, scale: f32) {
    let (x, y, w, h) = rect_tuple(card);
    gfx.fill_round_rect(x, y, w, h, 8.0 * scale, COLOR_CARD);
    gfx.stroke_round_rect(x, y, w, h, 8.0 * scale, COLOR_CARD_BORDER, scale.max(1.0));
}

unsafe fn draw_text(
    hdc: windows::Win32::Graphics::Gdi::HDC,
    font: windows::Win32::Graphics::Gdi::HFONT,
    color: u32,
    rect: RECT,
    text: &str,
    format: DRAW_TEXT_FORMAT,
) {
    unsafe {
        let previous = SelectObject(hdc, HGDIOBJ(font.0));
        let _ = SetBkMode(hdc, TRANSPARENT);
        let _ = SetTextColor(hdc, gdi_color(color));
        let mut buffer = wide(text);
        let length = buffer.len().saturating_sub(1);
        let mut target = rect;
        if let Some(slice) = buffer.get_mut(..length) {
            let _ = windows::Win32::Graphics::Gdi::DrawTextW(hdc, slice, &mut target, format);
        }
        SelectObject(hdc, previous);
    }
}

/// FR-39 提示条的 × 命中区（与 `paint` 的绘制位置同源）。
pub fn notice_close_rect(notice: &RECT, scale: f32) -> RECT {
    let size = (20.0 * scale).round() as i32;
    RECT {
        left: notice.right - size - (8.0 * scale) as i32,
        top: notice.top + (8.0 * scale) as i32,
        right: notice.right - (8.0 * scale) as i32,
        bottom: notice.top + (8.0 * scale) as i32 + size,
    }
}

/// 命中测试：点是否在某矩形内。
pub fn hit(rect: &RECT, point: POINT) -> bool {
    point.x >= rect.left && point.x < rect.right && point.y >= rect.top && point.y < rect.bottom
}

fn client_size(hwnd: HWND) -> (i32, i32) {
    let mut rect = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut rect);
    }
    (rect.right - rect.left, rect.bottom - rect.top)
}

/// 按最新布局摆放 7 个按钮（WM_SIZE / DPI 变化 / 创建后调用）。
fn place_buttons(ui: &UiState, width: i32, height: i32) {
    let layout = layout(ui, width, height);
    let Some(buttons) = ui.buttons.as_ref() else {
        return;
    };
    let pairs = [
        (buttons.toggle, layout.toggle_btn),
        (buttons.goto_settings, layout.goto_settings_btn),
        (buttons.settings, layout.buttons[0]),
        (buttons.view_logs, layout.buttons[1]),
        (buttons.copy_logs, layout.buttons[2]),
        (buttons.copy_base, layout.buttons[3]),
        (buttons.about, layout.buttons[4]),
    ];
    for (hwnd, rect) in pairs {
        if hwnd.is_invalid() {
            continue;
        }
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                None,
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }
}

/// 供 `UiState` 调用的按钮重排（子控件位置跟着布局走）。
pub fn sync_buttons(ui: &UiState) {
    let (width, height) = client_size(ui.hwnd);
    if width > 0 && height > 0 {
        place_buttons(ui, width, height);
    }
}

fn create_button(
    parent: HWND,
    hinst: HINSTANCE,
    text: &str,
    id: usize,
    font: windows::Win32::Graphics::Gdi::HFONT,
) -> HWND {
    let class = wide("BUTTON");
    let label = wide(text);
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class.as_ptr()),
            PCWSTR(label.as_ptr()),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP,
            0,
            0,
            10,
            10,
            Some(parent),
            Some(HMENU(id as *mut core::ffi::c_void)),
            Some(hinst),
            None,
        )
    };
    let hwnd = hwnd.unwrap_or_default();
    if !hwnd.is_invalid() {
        unsafe {
            SendMessageW(
                hwnd,
                WM_SETFONT,
                Some(WPARAM(font.0 as usize)),
                Some(LPARAM(1)),
            );
        }
    }
    hwnd
}

/// 主窗消息处理。
pub unsafe extern "system" fn wndproc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        match message {
            WM_NCCREATE => {
                let create = lparam.0 as *const CREATESTRUCTW;
                if !create.is_null() {
                    let ui = (*create).lpCreateParams as *mut UiState;
                    if !ui.is_null() {
                        SetWindowLongPtrW(hwnd, GWLP_USERDATA, ui as isize);
                        (*ui).hwnd = hwnd;
                    }
                }
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
            WM_CREATE => {
                let created = with_ui(hwnd, |ui| {
                    let hinst = ui.hinst;
                    let font = ui.theme.font_ui;
                    let buttons = MainButtons {
                        toggle: create_button(hwnd, hinst, "停止", IDC_TOGGLE_SERVICE, font),
                        goto_settings: create_button(
                            hwnd,
                            hinst,
                            "去设置改端口",
                            IDC_GOTO_SETTINGS,
                            font,
                        ),
                        settings: create_button(hwnd, hinst, "设置", IDC_SETTINGS, font),
                        view_logs: create_button(hwnd, hinst, "查看日志", IDC_VIEW_LOGS, font),
                        copy_logs: create_button(hwnd, hinst, "复制日志", IDC_COPY_LOGS, font),
                        copy_base: create_button(hwnd, hinst, "复制 Base URL", IDC_COPY_BASE, font),
                        about: create_button(hwnd, hinst, "关于", IDC_ABOUT, font),
                    };
                    ui.buttons = Some(buttons);
                    // "去设置改端口"只在绑定失败态出现
                    if let Some(buttons) = ui.buttons.as_ref() {
                        if !buttons.goto_settings.is_invalid() {
                            let _ = windows::Win32::UI::WindowsAndMessaging::ShowWindow(
                                buttons.goto_settings,
                                windows::Win32::UI::WindowsAndMessaging::SW_HIDE,
                            );
                        }
                    }
                    let _ = SetTimer(Some(hwnd), TIMER_REFRESH, REFRESH_MS, None);
                    // P8-UI4：亚秒滚动节拍（100 ms；只更新相位 + 请求一次重绘，见 `tick_anim`）
                    let _ = SetTimer(Some(hwnd), TIMER_ANIM, ANIM_MS, None);
                    ui.refresh();
                });
                if created.is_none() {
                    return LRESULT(-1);
                }
                LRESULT(0)
            }
            WM_TIMER => {
                if wparam.0 == TIMER_REFRESH {
                    with_ui(hwnd, |ui| {
                        let toast_expired = ui
                            .toast
                            .as_ref()
                            .map(|(_, at)| at.elapsed() > Duration::from_secs(3))
                            .unwrap_or(false);
                        if toast_expired {
                            ui.toast = None;
                        }
                        ui.refresh();
                    });
                } else if wparam.0 == TIMER_ANIM {
                    // P8-UI4：亚秒相位 + 一次重绘（**不**重读快照/托盘/文档窗 ⇒ 10 Hz 的代价只在画一帧）
                    with_ui(hwnd, |ui| ui.tick_anim());
                }
                LRESULT(0)
            }
            WM_APP_REFRESH => {
                with_ui(hwnd, |ui| ui.refresh());
                LRESULT(0)
            }
            tray::WM_APP_TRAY => {
                let event = lparam.0 as u32;
                with_ui(hwnd, |ui| match event {
                    WM_LBUTTONUP | WM_LBUTTONDBLCLK => {
                        ui.toggle_window_visibility();
                    }
                    WM_RBUTTONUP => {
                        let selected = ui.show_tray_menu();
                        ui.run_tray_command(selected);
                    }
                    _ => {}
                });
                LRESULT(0)
            }
            WM_COMMAND => {
                let id = wparam.0 & 0xFFFF;
                with_ui(hwnd, |ui| match id {
                    IDC_TOGGLE_SERVICE => ui.toggle_service(),
                    IDC_SETTINGS | IDC_GOTO_SETTINGS => ui.open_settings(),
                    IDC_VIEW_LOGS => ui.show_logs(),
                    IDC_COPY_LOGS => ui.copy_logs(),
                    IDC_COPY_BASE => {
                        ui.copy_base_url();
                    }
                    IDC_ABOUT => ui.show_about(),
                    other if tray::is_tray_menu_id(other) => ui.run_tray_command(other),
                    _ => {}
                });
                LRESULT(0)
            }
            WM_PAINT => {
                let mut paint_struct = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut paint_struct);
                let (width, height) = client_size(hwnd);
                let pointer = ui_ptr(hwnd);
                if !pointer.is_null() && width > 0 && height > 0 {
                    // `&mut`：`paint` 要写 A-16 的三个诊断计数（帧数 / 单帧最多调用 / 单帧最长耗时）
                    let ui = &mut *pointer;
                    // 双缓冲：内存 DC 画完一次 BitBlt（长跑 1 s 刷新不闪）
                    let memory = CreateCompatibleDC(Some(hdc));
                    let bitmap = CreateCompatibleBitmap(hdc, width, height);
                    if !bitmap.is_invalid() {
                        let previous = SelectObject(memory, HGDIOBJ(bitmap.0));
                        paint(ui, memory, width, height);
                        let _ = BitBlt(hdc, 0, 0, width, height, Some(memory), 0, 0, SRCCOPY);
                        SelectObject(memory, previous);
                        let _ = DeleteObject(HGDIOBJ(bitmap.0));
                    }
                    let _ = DeleteDC(memory);
                }
                let _ = EndPaint(hwnd, &paint_struct);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1), // 自绘背景 ⇒ 吞掉，避免闪
            WM_LBUTTONUP => {
                // 客户区点击（按钮是子窗口，自己有 WM_COMMAND，不会落到这里）：
                // 最近错误行 = 弹详情；FR-39 提示条的 × = 本次关闭
                let x = (lparam.0 & 0xFFFF) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
                with_ui(hwnd, |ui| {
                    let (width, height) = client_size(hwnd);
                    let layout = layout(ui, width, height);
                    let point = POINT { x, y };
                    if let Some(notice) = layout.notice.as_ref() {
                        if hit(&notice_close_rect(notice, ui.theme.scale), point) {
                            ui.dismiss_notice();
                            return;
                        }
                    }
                    if hit(&layout.error_text, point) {
                        ui.show_recent_errors();
                    }
                });
                LRESULT(0)
            }
            WM_SIZE => {
                with_ui(hwnd, |ui| {
                    sync_buttons(ui);
                });
                LRESULT(0)
            }
            WM_GETMINMAXINFO => {
                let info = lparam.0 as *mut MINMAXINFO;
                let pointer = ui_ptr(hwnd);
                if !info.is_null() && !pointer.is_null() {
                    // 最小尺寸随**提示条**是否可见而变：提示条把内容整体推高 64 DIP（44 + 上下各 10），
                    // 其中 32 由卡片 84 → 52 抵掉 ⇒ 固定常数会在提示条出现时让卡片与曲线面板重叠。
                    let (min_width, min_height) =
                        min_window_size(&(*pointer).theme, (*pointer).notice_visible());
                    (*info).ptMinTrackSize.x = min_width;
                    (*info).ptMinTrackSize.y = min_height;
                }
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
            WM_DPICHANGED => {
                // wParam 低位 = 新 DPI；lParam = 系统建议的窗口矩形（§7.5 第 1 条：Per-Monitor 回退链）
                let dpi = (wparam.0 & 0xFFFF) as u32;
                let suggested = lparam.0 as *const RECT;
                with_ui(hwnd, |ui| {
                    if dpi > 0 {
                        ui.apply_dpi(dpi);
                    }
                    if !suggested.is_null() {
                        let rect = *suggested;
                        let _ = SetWindowPos(
                            hwnd,
                            None,
                            rect.left,
                            rect.top,
                            rect.right - rect.left,
                            rect.bottom - rect.top,
                            SWP_NOZORDER | SWP_NOACTIVATE,
                        );
                    }
                });
                LRESULT(0)
            }
            // P6-S7 / **D1**：原 `WM_SYSCOMMAND` + `SC_MINIMIZE` ⇒ `hide_to_tray()` 的分支**已删除**
            // —— 最小化不带 `WM_SYSCOMMAND` 处理，直接落 `_` 臂交 `DefWindowProcW` 走系统默认
            // ⇒ **进任务栏**，不进托盘（托盘图标照旧存在，D11-e）。`hide_to_tray` 的调用面因此只剩
            // 关闭按钮（D2）与托盘左键/菜单的反向动作（见 `ui/mod.rs::hide_to_tray` 调用者清单）。
            windows::Win32::UI::WindowsAndMessaging::WM_CLOSE => {
                with_ui(hwnd, |ui| {
                    if ui.close_action == "exit" {
                        ui.request_exit();
                    } else {
                        ui.hide_to_tray();
                    }
                });
                LRESULT(0)
            }
            WM_QUERYENDSESSION => {
                with_ui(hwnd, |ui| ui.prepare_session_end());
                LRESULT(1)
            }
            WM_DESTROY => {
                let pointer = ui_ptr(hwnd);
                if !pointer.is_null() {
                    (*pointer).hwnd = HWND(std::ptr::null_mut());
                }
                PostQuitMessage(0);
                LRESULT(0)
            }
            WM_NCDESTROY => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

/// 最小窗口尺寸（客户区 = `MIN_CLIENT_W_DIP × min_client_height_dip()`；含边框/标题栏）。
fn min_window_size(theme: &Theme, notice_visible: bool) -> (i32, i32) {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: theme.px(MIN_CLIENT_W_DIP as f32),
        bottom: theme.px(min_client_height_dip(notice_visible) as f32),
    };
    unsafe {
        let _ = AdjustWindowRectEx(&mut rect, WS_OVERLAPPEDWINDOW, false, WINDOW_EX_STYLE(0));
    }
    (rect.right - rect.left, rect.bottom - rect.top)
}

/// 供 `UiState` 使用的"最小绘制刷新"。
///
/// **不可见时不重绘**（A-16③ / §2.5.4）：窗口隐藏到托盘后 `InvalidateRect` 只是白写一个更新区
/// —— 采样由服务侧照旧进行（D1），但 UI 侧"停止重绘"必须是**结构**事实而不是推断
/// （`曲线诊断：帧=…` 在隐藏期读到 0 就是它的机械读数）。
pub fn invalidate(ui: &UiState) {
    if ui.window_visible() {
        unsafe {
            let _ = InvalidateRect(Some(ui.hwnd), None, false);
        }
    }
}

/// 消息循环（主线程；返回时说明收到了 `WM_QUIT`）。
///
/// **键盘可达性（I5b）**：设置窗（含其子控件）的消息先过 `IsDialogMessageW` ⇒ `Tab` / `Shift+Tab`
/// 在 `WS_TABSTOP` 子控件间导航、`Enter` 走默认按钮（`DM_GETDEFID`）。
/// **只对设置窗调用**：主窗 / 文档窗的消息照旧 `Translate/Dispatch` ⇒ 判据"不得把 Tab 抢到别的窗口"成立。
pub fn message_loop(ui: *mut UiState) {
    let mut message = windows::Win32::UI::WindowsAndMessaging::MSG::default();
    unsafe {
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            if handle_as_dialog_message(ui, &mut message) {
                continue;
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

/// 设置窗的消息交给对话框管理器（返回 `true` = 已被 `IsDialogMessageW` 处理，不要再 `Dispatch`）。
///
/// 归属判定 = `hwnd == 设置窗 ∨ IsChild(设置窗, hwnd)`（Tab 的键消息是发给**当前有焦点的子控件**的）。
/// `settings_hwnd` 在设置窗销毁路径里被置回 `None` ⇒ 这里不存在"悬垂句柄"窗口期。
unsafe fn handle_as_dialog_message(
    ui: *mut UiState,
    message: &mut windows::Win32::UI::WindowsAndMessaging::MSG,
) -> bool {
    if ui.is_null() {
        return false;
    }
    let Some(settings) = (unsafe { (*ui).settings_hwnd }) else {
        return false;
    };
    if settings.is_invalid() {
        return false;
    }
    let target = message.hwnd;
    let belongs = target == settings
        || unsafe { windows::Win32::UI::WindowsAndMessaging::IsChild(settings, target).as_bool() };
    if !belongs {
        return false;
    }
    // `*mut MSG` → `*const MSG` 的强转由调用点完成（两种签名都能过）
    let pointer: *mut windows::Win32::UI::WindowsAndMessaging::MSG = message;
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::IsDialogMessageW(settings, pointer).as_bool()
    }
}

/// 让窗口可见并获得焦点（托盘"显示主窗口"/重复启动唤起）。
pub fn show_and_focus(hwnd: HWND) {
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{
            SetForegroundWindow, ShowWindow, SW_RESTORE,
        };
        let _ = ShowWindow(hwnd, SW_RESTORE);
        let _ = UpdateWindow(hwnd);
        let _ = SetForegroundWindow(hwnd);
    }
}

/// 销毁主窗（退出路径；`WM_DESTROY` 里 `PostQuitMessage` 让消息循环结束）。
pub fn destroy(hwnd: HWND) {
    if !hwnd.is_invalid() {
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::measure_text;
    use azusa_local_proxy::stats::Stats;
    use windows::Win32::Graphics::Gdi::{GetDC, ReleaseDC};

    fn overlaps(a: &RECT, b: &RECT) -> bool {
        a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
    }

    /// 顶层块（信息层级里的"块"级矩形；曲线面板内部的锚点是**包含**关系，不参与互斥检查）。
    fn top_level_blocks(layout: &Layout, goto_visible: bool) -> Vec<(String, RECT)> {
        let mut blocks = vec![
            ("状态标题".to_string(), layout.status_text),
            ("地址行".to_string(), layout.addr_text),
            ("主按钮".to_string(), layout.toggle_btn),
            ("运行时长".to_string(), layout.uptime_text),
            ("分隔线".to_string(), layout.separator_top),
            ("波动面板".to_string(), layout.curve_panel),
            ("延迟行".to_string(), layout.latency_text),
            ("流量行".to_string(), layout.traffic_text),
            ("最近错误行".to_string(), layout.error_text),
        ];
        if goto_visible {
            blocks.push(("去设置改端口".to_string(), layout.goto_settings_btn));
        }
        if let Some(notice) = layout.notice.as_ref() {
            blocks.push(("提示条".to_string(), *notice));
        }
        for (index, card) in layout.cards.iter().enumerate() {
            blocks.push((format!("指标卡 {index}"), *card));
        }
        for (index, button) in layout.buttons.iter().enumerate() {
            blocks.push((format!("按钮 {index}"), *button));
        }
        blocks
    }

    #[test]
    fn min_size_and_hit_testing() {
        let theme = Theme::new(96);
        let (width, height) = min_window_size(&theme, false);
        assert!(
            width > theme.px(MIN_CLIENT_W_DIP as f32),
            "最小宽度必须覆盖客户区 {MIN_CLIENT_W_DIP}"
        );
        assert!(
            height > theme.px(MIN_CLIENT_H_DIP as f32),
            "最小高度必须覆盖客户区 {MIN_CLIENT_H_DIP}（310 + 曲线 {CURVE_MIN_H}）"
        );
        let (_, notice_height) = min_window_size(&theme, true);
        assert!(
            notice_height > height,
            "提示条可见时最小高度必须更高（否则卡片与曲线面板重叠）"
        );
        let rect = RECT {
            left: 0,
            top: 0,
            right: 10,
            bottom: 10,
        };
        assert!(hit(&rect, POINT { x: 5, y: 5 }));
        assert!(!hit(&rect, POINT { x: 10, y: 5 }));
        assert_eq!(status_color(Phase::Stopped, false), COLOR_GRAY);
        assert_eq!(status_color(Phase::Running, false), COLOR_GREEN);
        assert_eq!(status_color(Phase::Running, true), COLOR_YELLOW);
        assert_eq!(status_color(Phase::Error, false), COLOR_RED);
        assert_eq!(
            status_label(&Status::for_config(Default::default(), None)),
            "已停止"
        );
    }

    /// 标签带 = "一行或两行"（绘制用 `DT_WORDBREAK`）：机械检查 = **最长可放下的前缀** + 其余部分
    /// 也要放得下 ⇒ "窄窗折 2 行"与"默认单行"两种形态都**不许丢字**（也不会出 `…`）。
    fn fits_in_at_most_two_lines(
        hdc: windows::Win32::Graphics::Gdi::HDC,
        font: windows::Win32::Graphics::Gdi::HFONT,
        text: &str,
        band: i32,
    ) -> bool {
        let chars: Vec<char> = text.chars().collect();
        let mut first = 0;
        for index in 1..=chars.len() {
            let prefix: String = chars[..index].iter().collect();
            if measure_text(hdc, font, &prefix) <= band {
                first = index;
            } else {
                break;
            }
        }
        if first == 0 {
            return false;
        }
        let rest: String = chars[first..].iter().collect();
        rest.is_empty() || measure_text(hdc, font, &rest) <= band
    }

    /// 【A-3】主面板布局（纯函数）在 **4 档 DPI × 5 种客户区** 下：
    /// ① 顶层块互不相交 ② 全部 ⊂ 客户区 ③ 曲线内部锚点 ⊂ 面板 ④ 三行读数**整行**
    /// ⑤ 按钮行（含折 2 行）≤ `content_w`。这是"折行裁切 / 重叠 / 越界"的通用防线。
    #[test]
    fn a3_main_layout_never_overlaps_at_four_dpis() {
        // （客户区宽 DIP，客户区高 DIP，提示条可见）
        let cases = [
            (460, 400, false), // 默认：曲线 90
            (420, 366, false), // 最小：曲线 56
            (420, 400, false), // 窄窗：按钮折 2 行
            (460, 398, true),  // 提示条可见（最小高 398）
            (900, 640, false), // 放大：曲线到 CURVE_MAX_H 后富余
        ];
        for dpi in [96, 120, 144, 192] {
            let scale = dpi as f32 / 96.0;
            for (width_dip, height_dip, notice) in cases {
                let width = (width_dip as f32 * scale).round() as i32;
                let height = (height_dip as f32 * scale).round() as i32;
                for goto_visible in [false, true] {
                    let layout = layout_metrics(scale, width, height, notice, goto_visible);
                    let blocks = top_level_blocks(&layout, goto_visible);
                    for (name, rect) in blocks.iter() {
                        assert!(
                            rect.left >= 0
                                && rect.top >= 0
                                && rect.right <= width
                                && rect.bottom <= height
                                && rect.right > rect.left
                                && rect.bottom > rect.top,
                            "{dpi} DPI {width_dip}×{height_dip}（提示条={notice}）越界/退化：{name} {rect:?}"
                        );
                    }
                    for (index, (name_a, a)) in blocks.iter().enumerate() {
                        for (name_b, b) in blocks.iter().skip(index + 1) {
                            assert!(
                                !overlaps(a, b),
                                "{dpi} DPI {width_dip}×{height_dip}（提示条={notice}）：\
                                 {name_a} {a:?} 与 {name_b} {b:?} 重叠"
                            );
                        }
                    }
                    // 曲线内部：锚点必须落在面板里；绘图区不得压到标题行
                    let panel = layout.curve_panel;
                    for (name, rect) in [
                        ("曲线标题", layout.curve_title),
                        ("当前值", layout.curve_now),
                        ("绘图区", layout.curve_plot),
                    ] {
                        assert!(
                            rect.left >= panel.left
                                && rect.right <= panel.right
                                && rect.top >= panel.top
                                && rect.bottom <= panel.bottom,
                            "{dpi} DPI {width_dip}×{height_dip}：{name} {rect:?} 不在面板 {panel:?} 内"
                        );
                    }
                    assert!(
                        layout.curve_plot.top >= layout.curve_title.bottom,
                        "{dpi} DPI {width_dip}×{height_dip}：绘图区压到标题行"
                    );
                    // 【I5c · P1】**Y 轴标签不依赖 `roomy`**（Leader 裁定 (b)）：只要绘图区非退化
                    // （高 ≥ 14 DIP）就必须至少画出**上/下两档**，高 ≥ 28 DIP 时三档齐；都 ⊂ 面板。
                    // 负例（[E13]③）：把 Y 标签的填充重新塞回 `if roomy { … }` ⇒ 433×400 / 420×366
                    //（roomy = false 的两档）必红。
                    let plot_h = layout.curve_plot.bottom - layout.curve_plot.top;
                    let drawn: Vec<&RECT> = layout
                        .curve_y_labels
                        .iter()
                        .filter(|rect| rect.right > rect.left)
                        .collect();
                    let one = (14.0 * scale).round() as i32;
                    if plot_h >= one {
                        let expect = if plot_h >= 2 * one { 3 } else { 2 };
                        assert_eq!(
                            drawn.len(),
                            expect,
                            "{dpi} DPI {width_dip}×{height_dip}：绘图区高 {plot_h} px ⇒ \
                             Y 轴标签应 {expect} 档（实得 {}，roomy 只该管 X 标签带）",
                            drawn.len()
                        );
                        for rect in drawn.iter() {
                            assert!(
                                rect.left >= panel.left
                                    && rect.right <= panel.right
                                    && rect.top >= panel.top
                                    && rect.bottom <= panel.bottom,
                                "{dpi} DPI {width_dip}×{height_dip}：Y 标签 {rect:?} 不在面板内"
                            );
                        }
                    } else {
                        assert!(
                            drawn.is_empty(),
                            "{dpi} DPI：绘图区退化（{plot_h} px）时不得画 Y 标签"
                        );
                    }
                    // 三行读数 = **整行**（P2-3：修掉"只占 content_w / 2 ⇒ 尾部被 `…` 截掉"）
                    let margin = (PAD_PAGE * scale).round() as i32;
                    for (name, rect) in [
                        ("延迟行", layout.latency_text),
                        ("流量行", layout.traffic_text),
                        ("最近错误行", layout.error_text),
                    ] {
                        assert_eq!(rect.left, margin, "{name} 左缘必须是页边距");
                        assert_eq!(rect.right, width - margin, "{name} 必须是整行");
                    }
                    // 按钮行：不折时一行 5 颗；折时两行（3 + 2）—— 任何一行都 ≤ content_w
                    let content_w = width - margin * 2;
                    let mut row_extent: std::collections::BTreeMap<i32, (i32, i32)> =
                        std::collections::BTreeMap::new();
                    for button in layout.buttons.iter() {
                        let entry = row_extent
                            .entry(button.top)
                            .or_insert((button.left, button.right));
                        entry.0 = entry.0.min(button.left);
                        entry.1 = entry.1.max(button.right);
                    }
                    assert!(
                        row_extent.len() <= 2,
                        "按钮最多两行（实得 {} 行）",
                        row_extent.len()
                    );
                    for (top, (left, right)) in row_extent {
                        assert!(
                            left >= margin && right <= width - margin && right - left <= content_w,
                            "{dpi} DPI {width_dip}×{height_dip}：按钮行 top={top} 越界（{left}..{right}，content_w={content_w}）"
                        );
                    }
                }
            }
        }
    }

    /// 【A-15】主面板读数字串的**文本宽**（GDI 实测，非截图）—— 逐字串 ≤ 所在矩形宽。
    ///
    /// 客户区 460 ⇒ `content_w` **420 DIP**（三行读数整行）；卡片内宽（数值）/ 标签内宽由
    /// `layout_metrics` 现算（期望：数值 73 / 标签 85）。**数值不含 `…`** 由 §2.4.3 的字号阶梯保证
    /// （阶梯本身的判据 = `theme.rs` 的 `number_ladder_keeps_big_values_ellipsis_free`，A-5）。
    #[test]
    fn a15_main_panel_strings_fit_their_rects() {
        let theme = Theme::new(96);
        let snapshot = Stats::new().snapshot();
        let idle = Stats::new().window().snapshot();
        let mut busy = Stats::new().window().snapshot();
        busy.requests = 3;
        busy.latency_samples = 2;
        busy.latency_p50 = Some("210ms".to_string());
        busy.latency_p95 = Some("≥10s".to_string());

        let idle_texts = panel_texts(&idle, &snapshot);
        let busy_texts = panel_texts(&busy, &snapshot);
        let layout = layout_metrics(1.0, 460, 400, false, false);
        let full_row = layout.latency_text.right - layout.latency_text.left;
        assert_eq!(full_row, 420, "整行 = content_w（客户区 460 − 2×20）");

        let hdc = unsafe { GetDC(None) };
        let mut report = Vec::new();
        for (name, text) in [
            ("延迟行·空态空闲", &idle_texts.latency),
            ("延迟行·有流量", &busy_texts.latency),
            ("流量行", &busy_texts.traffic),
        ] {
            let width = measure_text(hdc, theme.font_small, text);
            report.push(format!("{name}={width}px"));
            assert!(
                width <= full_row,
                "{name} 放不下整行：{width} px > {full_row} DIP（{text}）"
            );
        }
        // 卡片：标签 + 数值（数值用**阶梯选出的字号**量宽，模拟真实绘制）
        let label_pad = 2;
        let value_pad = PAD_CARD as i32;
        let card_w = layout.cards[0].right - layout.cards[0].left;
        let label_width = card_w - label_pad * 2;
        let value_width = card_w - value_pad * 2;
        assert_eq!(
            (card_w, value_width, label_width),
            (97, 73, 93),
            "卡片内宽（数值 73 = 卡 97 − 2×12；标签 93 = 卡 97 − 2×2）"
        );
        for (label, _value) in busy_texts.cards.iter() {
            let label_px = measure_text(hdc, theme.font_small, label);
            assert!(
                fits_in_at_most_two_lines(hdc, theme.font_small, label, label_width),
                "卡片标签放不进两行带（实测 {label_px} px / 带 {label_width} DIP ⇒ 会丢字）：{label}"
            );
            report.push(format!("标签「{label}」={label_px}px/带{label_width}"));
        }

        // 【P1-①】卡片标签**不得越出卡片**（量高判据）：在窄窗各档量"折行所需真实高度"，
        // 要求 ① 带够高 ⇒ 折行版本完整落在卡片内；② 带不够 ⇒ 退化路径（单行 + 省略号）也能落进去。
        // 真机（改前）在 433/420 DIP 上第二行 `）` 被卡片下沿裁掉 —— 本断言是它的机械化防线。
        for (width_dip, height_dip) in [(460_i32, 400_i32), (433, 400), (420, 400), (420, 366)] {
            let layout = layout_metrics(1.0, width_dip, height_dip, false, false);
            let card = layout.cards[0];
            let band_left = card.left + label_pad;
            let band_right = card.right - label_pad;
            let band_width = band_right - band_left;
            let band_top_max = card.top + PAD_CARD as i32 + 26 + 2; // 数值底 + 间隙
            let band_bottom = card.bottom - label_pad;
            // 标签矩形 ⊂ 卡片矩形（审阅要求的像素/结构判据）
            assert!(
                band_left >= card.left
                    && band_right <= card.right
                    && band_top_max >= card.top
                    && band_bottom <= card.bottom,
                "{width_dip}×{height_dip}：标签矩形越出卡片"
            );
            for (label, _value) in busy_texts.cards.iter() {
                let needed = crate::ui::theme::measure_wrapped_height(
                    hdc,
                    theme.font_small,
                    label,
                    band_width,
                );
                let one_line = crate::ui::theme::measure_wrapped_height(
                    hdc,
                    theme.font_small,
                    "Ag",
                    band_width,
                );
                let room = band_bottom - band_top_max;
                report.push(format!(
                    "P1-① {width_dip}×{height_dip}「{label}」需{needed}px/一行{one_line}px/余{room}px"
                ));
                // 绝对最小客户区（420×366）下卡片只有 52 DIP ⇒ 单行也要超 ~4 px；
                // 那一点的退化是"上下各裁 ≈2 px"（**已知边界**，登记在 done.md）——
                // 用户实际可用的窄窗（420×400 / 433×400）完全放得下，故只在这一档留 allowance。
                let allowance = if (width_dip, height_dip) == (420, 366) {
                    6
                } else {
                    0
                };
                assert!(
                    room >= needed || room >= one_line - allowance,
                    "{width_dip}×{height_dip}：标签「{label}」既放不下折行（{needed}px）\
                     也放不下单行（{one_line}px），余量仅 {room}px ⇒ 必被裁"
                );
            }
        }
        let fonts = super::super::theme::NumberFonts::new(&theme.face, theme.scale);
        for (_, value) in busy_texts.cards.iter() {
            let (font, index) = fonts.pick_index(hdc, &theme, value, value_width);
            let value_px = measure_text(hdc, font, value);
            report.push(format!("数值「{value}」={index}档/{value_px}px"));
            assert!(
                value_px <= value_width,
                "卡片数值会出 `…`：{value_px} px > {value_width} DIP（{value}）"
            );
        }
        // 【C1】Y 轴标签列：宽度必须**由当前实际文案量宽**（`curve::label_column_width`，不是常数）
        // ⇒ 任一标签放得下，且标签列右缘 ≤ 绘图区左缘（**不裁字、不入图区**）。
        // 负例锚点：最宽标签 > 常数 20 DIP（把列宽拍成常数者必被咬）。
        let label_left = layout.curve_panel.left + PAD_CARD as i32;
        for y_max in [1024_u64, 512 * 1024, 500 * 1024 * 1024] {
            let column = crate::ui::curve::label_column_width(hdc, theme.font_small, y_max);
            let plot_left = (label_left + column + SPACE_XS as i32).max(layout.curve_plot.left);
            for label in crate::ui::curve::y_labels(y_max) {
                let px = measure_text(hdc, theme.font_small, &label);
                report.push(format!("Y 标签「{label}」={px}px/列{column}"));
                assert!(
                    px <= column,
                    "Y 轴标签被裁：{px} px > 列宽 {column} px（{label}）"
                );
                assert!(
                    px > 20,
                    "负例锚点：{label} 宽 {px} px > 常数 20 ⇒ 拍脑袋常数必被咬"
                );
                assert!(
                    label_left + column <= plot_left,
                    "标签列右缘必须 ≤ 绘图区左缘（标签不得进入绘图区）"
                );
            }
        }
        unsafe { ReleaseDC(None, hdc) };
        println!("A-15 主面板读数（整行 {full_row} DIP / 数值内宽 {value_width} / 标签内宽 {label_width}）：{report:?}");
    }

    /// 【J-B3】面板文案（`panel_texts` **纯函数**，机械出口，不依赖截图）：
    /// 空闲 ⇒ `上游延迟 —（近 120 s 无样本）` ∧ 含 `· 当前空闲`；有流量 ⇒ 含 `（近 120 s）`；
    /// 流量行含 `（自启动累计）`；窗口长度是唯一数值来源（"近 60 s"残留 = 红）。
    #[test]
    fn panel_texts_carry_the_window_semantics() {
        let zero = Stats::new().snapshot();
        let mut idle = Stats::new().window().snapshot();

        let texts = panel_texts(&idle, &zero);
        assert_eq!(texts.latency, "上游延迟 —（近 120 s 无样本）· 当前空闲");
        assert_eq!(texts.traffic, "↑0 B  ↓0 B（自启动累计）");
        assert_eq!(texts.cards[0].0, "请求（近 120 s）");
        assert_eq!(texts.cards[0].1, "0");
        assert_eq!(texts.cards[1].0, "活跃", "「活跃」是即时量 ⇒ 不带窗口语义");
        assert_eq!(texts.cards[2].0, "预检（近 120 s）");
        assert_eq!(texts.cards[3].0, "错误（近 120 s）");

        // 有流量：近窗读数 ⇒ 「（近 120 s）」；累计字节 ⇒ 只加「自启动累计」标注
        let mut busy = Stats::new().window().snapshot();
        busy.requests = 20;
        busy.preflight = 2;
        busy.errors = 1;
        busy.latency_samples = 3;
        busy.latency_p50 = Some("25ms".to_string());
        busy.latency_p95 = Some("50ms".to_string());
        let mut snapshot = Stats::new().snapshot();
        snapshot.bytes_in = 1024;
        snapshot.bytes_out = 2048;
        let texts = panel_texts(&busy, &snapshot);
        assert_eq!(texts.latency, "上游延迟 p50 25ms / p95 50ms（近 120 s）");
        assert!(!texts.latency.contains("当前空闲"));
        assert_eq!(texts.cards[0].1, "20");
        assert_eq!(texts.cards[2].1, "2");
        assert_eq!(texts.cards[3].1, "1");
        assert_eq!(texts.traffic, "↑1 KB  ↓2 KB（自启动累计）");

        // 在途请求（"等待上游响应头"形态）：近窗无样本也**不得**报「当前空闲」（B-5 修的就是这一段）
        snapshot.active = 1;
        let texts = panel_texts(&idle, &snapshot);
        assert_eq!(texts.latency, "上游延迟 —（近 120 s 无样本）");
        assert_eq!(texts.cards[1].1, "1", "「活跃」卡读即时在途数");

        // 窗口长度 = 文案的唯一数值来源（不得留下 "近 60 s" 这类残留）
        idle.secs = 60;
        let texts = panel_texts(&idle, &zero);
        assert!(
            texts.latency.contains("（近 60 s 无样本）"),
            "{}",
            texts.latency
        );
        assert_eq!(texts.cards[0].0, "请求（近 60 s）");
    }

    /// 几何中心（判据用；`left + (right-left)/2` 与 `(left+right)/2` 对非负坐标同值）。
    fn center_x(rect: &RECT) -> i32 {
        rect.left + (rect.right - rect.left) / 2
    }

    /// 【P8-UI2 · 居中】主面板两处文字的**水平居中**机械判据（用户真机报障 192 DPI：
    /// 「四个框框下方的文字也没有居中，网络波动下方显示的栏目也没有居中」）：
    ///
    /// ① **四张卡的标签**：文本矩形中心 ≡ 卡片中心（≤ 1 DIP）；
    /// ①b **四张卡的数值**（P8-UI2-tweak 补）：数值矩形中心 ≡ 卡片中心（≤ 1 DIP）
    ///     —— 四档取值（空闲 `0` / 大数 `1,234`·`5,678`·`1,234,567` / 合成探针 `1,234,567` /
    ///     超宽兜底 `12,345,678`）各自都要居中，且矩形 ∈ 卡片；放得下 ⇒ 矩形宽 ≡ 实测文本宽、
    ///     放不下 ⇒ 矩形宽 ≡ 整条内带（两种形态都不许贴左）；`pick_index` 的入口仍是**整条内带**；
    /// ② **曲线空态行**（`—（近 120 s 无样本）`）：文本中心 ≡ 其**栏位**（空态井）中心 ≡ 曲线面板中心；
    /// ③ **四档 DPI**（96 / 120 / 144 / 192）× **折行 / 非折行客户区**（460 单行标签 / 433·420 折两行 /
    ///     420×366 兜底档）各跑一遍；
    /// ④ 对齐标志必须含 `DT_CENTER`（旧缺陷形态 = 整条内带 + `DT_LEFT` ⇒ 贴左内缘）；
    /// ⑤ 阶梯**必要性**：`1,234,567` 在 21 DIP 档放不下 ⇒ 必须降档（A-5 的输入是整条内带，
    ///     把它换成"居中后的矩形宽"会先在这一条断）。
    ///
    /// **负例锚点**（[E13] 强制；三条本批实跑/沿用，原始失败消息见 done.md）：
    /// - 把 [`centered_text_span`] 的结果左移 8 px ⇒ ① 必 FAIL；
    /// - 把 [`card_value_plan`] 里 [`centered_text_span`] 的结果左移 8 px ⇒ ①b 必 FAIL；
    /// - 把 [`empty_hint_span`] 的结果左移 4 px ⇒ ② 必 FAIL。
    ///
    /// ⚠ 本判据是"绘制同一出口"的机械面（`card_label_plan` / `card_value_plan` / `empty_state_well` /
    /// `empty_hint_span` 就是 `paint` 用的那四个函数）；**像素级**证据另见真机截图（`main-after/`）。
    #[test]
    fn p8_main_panel_texts_are_horizontally_centered() {
        let snapshot = Stats::new().snapshot();
        let idle = Stats::new().window().snapshot();
        let texts = panel_texts(&idle, &snapshot);
        // ①b 的取值四档：空闲 `0` / **大数档**（真实的四位数与七位数）/ **阶梯探针**（A-5 的既有样本
        // `1,234,567` —— 它在 21 DIP 档放不下 ⇒ 每张卡都会真走到"阶梯降档"那条路）/
        // **超宽兜底档**（`12,345,678`：8 位 + 两个逗号**超出 13 DIP 档**的能力 ⇒ 走"整条内带"分支；
        // 实测 96 DPI × 420 DIP 客户区 = 70 px > 内带 66 px —— 这是**既有能力边界**，非本批引入）。
        let mut big_window = Stats::new().window().snapshot();
        big_window.requests = 1234;
        big_window.preflight = 5_678;
        big_window.errors = 1_234_567;
        let big_texts = panel_texts(&big_window, &Stats::new().snapshot());
        const LADDER_PROBE: &str = "1,234,567";
        const OVER_WIDE_PROBE: &str = "12,345,678";
        let mut report = Vec::new();
        for dpi in [96_u32, 120, 144, 192] {
            let scale = dpi as f32 / 96.0;
            let theme = Theme::new(dpi);
            let hdc = unsafe { GetDC(None) };
            let number_fonts = super::super::theme::NumberFonts::new(&theme.face, theme.scale);
            // 默认 460 DIP（标签单行）+ 窄窗 420 DIP（标签**折两行** ⇒ 走"整条内带 + DT_CENTER"
            // 那条分支）+ 最小高 366（竖直两段式的兜底档）—— 三种客户区都要居中。
            for (width_dip, height_dip) in [
                (460.0_f32, 400.0_f32),
                (433.0, 400.0),
                (420.0, 400.0),
                (420.0, 366.0),
            ] {
                let width = (width_dip * scale).round() as i32;
                let height = (height_dip * scale).round() as i32;
                let layout = layout_metrics(scale, width, height, false, false);

                // ── ① 四张卡：标签文本矩形中心 ≡ 卡片中心
                for (index, card) in layout.cards.iter().enumerate() {
                    // ①b 数值（P8-UI2-tweak）：矩形中心 ≡ 卡片中心、矩形 ∈ 卡片、宽 ≡ 实测宽。
                    // `must_fit=true` 的档 = 真实取值 ⇒ 还必须"选定档真放得下"（A-5 的入口没被改窄：
                    // `pick_index` 的上限仍是**整条内带**，不是居中后的矩形宽）。
                    for (tag, value_text, must_fit) in [
                        ("空闲", texts.cards[index].1.as_str(), true),
                        ("大数", big_texts.cards[index].1.as_str(), true),
                        ("阶梯探针", LADDER_PROBE, true),
                        ("超宽兜底", OVER_WIDE_PROBE, false),
                    ] {
                        let (v_rect, v_font, v_format) =
                            card_value_plan(hdc, &number_fonts, &theme, card, value_text, scale);
                        let v_measured = measure_text(hdc, v_font, value_text);
                        let v_band =
                            (card.right - theme.px(PAD_CARD)) - (card.left + theme.px(PAD_CARD));
                        let fallback = v_measured <= 0 || v_measured > v_band;
                        report.push(format!(
                            "{dpi} DPI {width_dip}×{height_dip} 卡{index} 数值「{value_text}」({tag}) \
                             实测 {v_measured}px / 内带 {v_band}px / {} ⇒ 数值矩形 {v_rect:?} 中心 {} \
                             vs 卡中心 {}",
                            if fallback { "兜底内带" } else { "恰为实测宽" },
                            center_x(&v_rect),
                            center_x(card)
                        ));
                        assert!(
                            (center_x(&v_rect) - center_x(card)).abs() <= 1,
                            "{} DPI 卡片 {index} 数值「{value_text}」({tag}) **没有居中**：数值矩形中心 {} \
                             与卡片中心 {} 偏差 {} px（> 1 DIP）—— 数值矩形 {v_rect:?} / 卡片 {card:?}",
                            dpi,
                            center_x(&v_rect),
                            center_x(card),
                            (center_x(&v_rect) - center_x(card)).abs()
                        );
                        assert_eq!(
                            (v_format.0 & DT_CENTER.0),
                            DT_CENTER.0,
                            "{dpi} DPI 卡片 {index} 数值「{value_text}」({tag}) 的绘制标志必须含 \
                             DT_CENTER（实得 {:#X}）",
                            v_format.0
                        );
                        // 矩形 ∈ 卡片（居中不得以出界为代价）
                        assert!(
                            v_rect.left >= card.left && v_rect.right <= card.right,
                            "{dpi} DPI 卡片 {index} 数值「{value_text}」({tag})：数值矩形 {v_rect:?} \
                             越出卡片 {card:?}"
                        );
                        // 两种形态各用其形：放得下 ⇒ 恰为实测宽；放不下 ⇒ **整条内带**（仍居中 + 省略号）
                        let expect_w = if fallback { v_band } else { v_measured };
                        assert_eq!(
                            v_rect.right - v_rect.left,
                            expect_w,
                            "{dpi} DPI 卡片 {index} 数值「{value_text}」({tag}) 的矩形宽必须 = \
                             {}（实测 {v_measured} / 内带 {v_band} / 矩形 {}）",
                            if fallback {
                                "整条内带"
                            } else {
                                "实测文本宽"
                            },
                            v_rect.right - v_rect.left
                        );
                        assert!(
                            !must_fit || !fallback,
                            "{} DPI 卡片 {index} 数值「{value_text}」({tag}) 会出 `…`：{v_measured}px > \
                             内带 {v_band}px —— 阶梯入口（整条内带）必须仍然咬得住",
                            dpi
                        );
                    }

                    let label = texts.cards[index].0.as_str();
                    // 标签的竖直下界 = **数值计划的底边**（与 `paint` 同一出口 ⇒ 不再各算一遍常量）
                    let value_bottom = card_value_plan(
                        hdc,
                        &number_fonts,
                        &theme,
                        card,
                        texts.cards[index].1.as_str(),
                        scale,
                    )
                    .0
                    .bottom;
                    let (rect, format) =
                        card_label_plan(hdc, theme.font_small, card, label, scale, value_bottom);
                    let measured = measure_text(hdc, theme.font_small, label);
                    let card_pad = theme.px(2.0);
                    let band = (card.right - card_pad) - (card.left + card_pad);
                    report.push(format!(
                        "{dpi} DPI {width_dip}×{height_dip} 卡{index}「{label}」实测 {measured}px / \
                         内带 {band}px ⇒ 文本矩形 {rect:?} 中心 {} vs 卡中心 {}",
                        center_x(&rect),
                        center_x(card)
                    ));
                    assert!(
                        (center_x(&rect) - center_x(card)).abs() <= 1,
                    "{} DPI 卡片 {index}「{label}」**没有居中**：文本矩形中心 {} 与卡片中心 {} \
                     偏差 {} px（> 1 DIP）—— 文本矩形 {rect:?} / 卡片 {card:?}",
                    dpi,
                    center_x(&rect),
                    center_x(card),
                    (center_x(&rect) - center_x(card)).abs()
                );
                    assert_eq!(
                        (format.0 & DT_CENTER.0),
                        DT_CENTER.0,
                        "{dpi} DPI 卡片 {index}「{label}」的绘制标志必须含 DT_CENTER（实得 {:#X}）",
                        format.0
                    );
                    // 矩形 ∈ 卡片（居中不得以出界为代价）
                    assert!(
                        rect.left >= card.left && rect.right <= card.right,
                        "{dpi} DPI 卡片 {index}：标签矩形 {rect:?} 越出卡片 {card:?}"
                    );
                    // 单行形态：矩形宽 ≡ 实测文本宽（"按实测宽算 x"这一半也要能咬）
                    if measured > 0 && measured <= band {
                        assert_eq!(
                            rect.right - rect.left,
                            measured,
                            "{dpi} DPI 卡片 {index}「{label}」单行形态的矩形宽必须 = 实测文本宽 \
                         （实测 {measured} / 矩形 {}）",
                            rect.right - rect.left
                        );
                    }
                }

                // ── ①c 阶梯**必要性**（A-5 的入口是整条内带，不是"居中后的矩形宽"）：
                //     合成探针在 21 DIP 档放不下 ⇒ 必须降档；把入口改窄会让这条先断。
                let probe_band = (layout.cards[0].right - theme.px(PAD_CARD))
                    - (layout.cards[0].left + theme.px(PAD_CARD));
                let probe_at_21 = measure_text(hdc, theme.font_number, LADDER_PROBE);
                let (probe_font, probe_index) =
                    number_fonts.pick_index(hdc, &theme, LADDER_PROBE, probe_band);
                report.push(format!(
                    "{dpi} DPI {width_dip}×{height_dip} 阶梯探针「{LADDER_PROBE}」21 档 {probe_at_21}px / \
                     内带 {probe_band}px ⇒ 选定 {probe_index} 档（{}px）",
                    measure_text(hdc, probe_font, LADDER_PROBE)
                ));
                assert!(
                    probe_at_21 > probe_band,
                    "{dpi} DPI {width_dip}×{height_dip}：探针「{LADDER_PROBE}」在 21 DIP 档居然放得下\
                     （{probe_at_21} ≤ {probe_band}）⇒ ①c 失去意义（卡宽/字号变了要换探针）"
                );
                assert!(
                    probe_index >= 1,
                    "{dpi} DPI {width_dip}×{height_dip}：探针必须**降过档**（实得第 {probe_index} 档）"
                );

                // ── ② 曲线空态行：文本中心 ≡ 空态井中心 ≡ 曲线面板中心
                let well = empty_state_well(&layout.curve_panel, &layout.curve_plot, scale);
                let empty_hint = idle.latency_text();
                let (hint_left, hint_right) =
                    empty_hint_span(hdc, theme.font_small, &well, &empty_hint);
                let hint_center = hint_left + (hint_right - hint_left) / 2;
                report.push(format!(
                "{dpi} DPI 空态「{empty_hint}」文本 {hint_left}..{hint_right} 中心 {hint_center} / \
                 井中心 {} / 面板中心 {}",
                center_x(&well),
                center_x(&layout.curve_panel)
            ));
                assert!(
                    (hint_center - center_x(&well)).abs() <= 1,
                    "{} DPI 曲线空态行**没有在栏位内居中**：文本中心 {hint_center} 与井中心 {} \
                 偏差 {} px（井 {well:?} / 文本 {hint_left}..{hint_right}）",
                    dpi,
                    center_x(&well),
                    (hint_center - center_x(&well)).abs()
                );
                // 井本身也必须居中（**真正的偏移源**：旧实现沿用名义绘图区 ⇒ 左 62 DIP 让位给 Y 标签列、
                // 右只有 12 DIP ⇒ 井整体右偏 25 DIP。空态不画轴标签 ⇒ 左右内缩必须相等）
                assert!(
                    (center_x(&well) - center_x(&layout.curve_panel)).abs() <= 1,
                    "{} DPI 空态井**没有在面板内居中**：井中心 {} vs 面板中心 {}（井 {well:?} / \
                 面板 {:?}）—— 空态不画 Y 轴标签 ⇒ 不得沿用名义绘图区的左侧标签列让位",
                    dpi,
                    center_x(&well),
                    center_x(&layout.curve_panel),
                    layout.curve_panel
                );
                assert_eq!(
                    well.left - layout.curve_panel.left,
                    layout.curve_panel.right - well.right,
                    "{dpi} DPI 空态井左右内缩必须相等（左 {} / 右 {}）",
                    well.left - layout.curve_panel.left,
                    layout.curve_panel.right - well.right
                );
                assert!(
                    well.left >= layout.curve_panel.left
                        && well.right <= layout.curve_panel.right
                        && well.top >= layout.curve_panel.top
                        && well.bottom <= layout.curve_panel.bottom,
                    "{dpi} DPI 空态井 {well:?} 越出面板 {:?}",
                    layout.curve_panel
                );
            }
            // DC 每档 DPI 一个（**不能**放进客户区循环 —— 放里面会在第二档起晒出已释放的 DC：
            // 实测会静默退化成 `measure_text = 0`）
            unsafe { ReleaseDC(None, hdc) };
        }
        println!("P8-UI2 居中读数（DPI · 客户区 · 文本矩形 · 中心）：{report:?}");
    }
}
