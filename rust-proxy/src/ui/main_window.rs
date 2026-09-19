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
    DT_WORDBREAK, HGDIOBJ, PAINTSTRUCT, SRCCOPY, TRANSPARENT,
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

use super::theme::{
    format_bytes, format_thousands, format_uptime, gdi_color, wide, Gfx, Theme, COLOR_ACCENT_DEEP,
    COLOR_CARD, COLOR_CARD_BORDER, COLOR_GRAY, COLOR_GREEN, COLOR_INK, COLOR_INK_SOFT,
    COLOR_NOTICE_BG, COLOR_NOTICE_BORDER, COLOR_NOTICE_INK, COLOR_RED, COLOR_SEPARATOR,
    COLOR_YELLOW,
};
use super::{tray, UiState, MAIN_WINDOW_CLASS, PLAIN_TEXT_NOTICE, WM_APP_REFRESH};

/// 主窗刷新定时器（§7.1：指标每 1 s 刷新）。
const TIMER_REFRESH: usize = 1;
const REFRESH_MS: u32 = 1000;

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

/// 布局结果（像素坐标；绘制与命中测试共用同一份 ⇒ 不会"看着在一个位置、点的是另一个"）。
pub struct Layout {
    pub status_dot_center: POINT,
    pub status_dot_radius: f32,
    pub status_text: RECT,
    pub addr_text: RECT,
    pub toggle_btn: RECT,
    pub goto_settings_btn: RECT,
    pub uptime_text: RECT,
    pub separator_top: RECT,
    pub separator_bottom: RECT,
    pub cards: [RECT; 4],
    pub latency_text: RECT,
    pub bytes_text: RECT,
    pub error_text: RECT,
    pub notice: Option<RECT>,
    pub buttons: [RECT; 5],
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
        let client_width = (*ui).theme.px(460.0);
        let client_height = (*ui).theme.px(340.0);
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

/// 布局（像素坐标）。§7.1 的字段顺序即本函数的行序。
pub fn layout(ui: &UiState, width: i32, height: i32) -> Layout {
    let scale = ui.theme.scale;
    let px = |value: f32| (value * scale).round() as i32;
    let margin = px(16.0);
    let content_w = width - margin * 2;

    let button_h = px(30.0);
    let bottom_y = height - px(14.0) - button_h;
    let notice = if ui.notice_visible() {
        Some(RECT {
            left: margin,
            top: bottom_y - px(10.0) - px(44.0),
            right: margin + content_w,
            bottom: bottom_y - px(10.0),
        })
    } else {
        None
    };
    let error_top = match &notice {
        Some(rect) => rect.top - px(24.0),
        None => bottom_y - px(10.0) - px(18.0),
    };
    let error_text = RECT {
        left: margin,
        top: error_top,
        right: margin + content_w,
        bottom: error_top + px(18.0),
    };
    let latency_top = error_text.top - px(24.0);
    let cards_bottom = latency_top - px(12.0);
    let cards_top = px(96.0);
    let cards_h = (cards_bottom - cards_top).clamp(px(52.0), px(84.0));
    let gap = px(10.0);
    let card_w = (content_w - gap * 3) / 4;
    let mut cards = [RECT::default(); 4];
    for (index, slot) in cards.iter_mut().enumerate() {
        let left = margin + (card_w + gap) * index as i32;
        *slot = RECT {
            left,
            top: cards_top,
            right: left + card_w,
            bottom: cards_top + cards_h,
        };
    }

    let toggle_w = px(78.0);
    let toggle_h = px(28.0);
    let goto_w = px(118.0);
    // 「去设置改端口」只在绑定失败态出现（与 `UiState::refresh` 同一判据）⇒ 头部三行要让出它占的宽度，
    // 否则失败态下地址行会被它盖住（实测 2026-09-19：`截图 21-main-error.png` 命中）。
    let goto_visible = ui.status.phase == Phase::Error && ui.status.last_error.is_some();
    let head_right = if goto_visible {
        margin + content_w - goto_w - px(12.0)
    } else {
        margin + content_w - toggle_w - px(12.0)
    };
    // 5 个按钮：设置 / 查看日志 / 复制日志 / 复制 Base URL / 关于。
    // 总宽（含 4 个 8px 间隔）必须 ≤ content_w（460 − 16×2 = 428 @100%）：
    // 62 + 78 + 78 + 112 + 62 + 32 = 424 ✓（P6-S5 新增「复制日志」后收窄了各键宽度）。
    let widths = [px(62.0), px(78.0), px(78.0), px(112.0), px(62.0)];
    let btn_gap = px(8.0);
    let total: i32 = widths.iter().sum::<i32>() + btn_gap * (widths.len() as i32 - 1);
    let mut buttons = [RECT::default(); 5];
    let mut x = margin + content_w - total;
    for (index, slot) in buttons.iter_mut().enumerate() {
        let w = widths.get(index).copied().unwrap_or(0);
        *slot = RECT {
            left: x,
            top: bottom_y,
            right: x + w,
            bottom: bottom_y + button_h,
        };
        x += w + btn_gap;
    }

    Layout {
        status_dot_center: POINT {
            x: margin + px(8.0),
            y: px(12.0) + px(16.0),
        },
        status_dot_radius: (7.0 * scale).max(5.0),
        status_text: RECT {
            left: margin + px(26.0),
            top: px(10.0),
            right: head_right,
            bottom: px(10.0) + px(26.0),
        },
        addr_text: RECT {
            left: margin + px(26.0),
            top: px(38.0),
            right: head_right,
            bottom: px(38.0) + px(18.0),
        },
        toggle_btn: RECT {
            left: margin + content_w - toggle_w,
            top: px(14.0),
            right: margin + content_w,
            bottom: px(14.0) + toggle_h,
        },
        goto_settings_btn: RECT {
            left: margin + content_w - goto_w,
            top: px(44.0),
            right: margin + content_w,
            bottom: px(44.0) + px(26.0),
        },
        uptime_text: RECT {
            left: margin,
            top: px(64.0),
            right: if goto_visible {
                head_right
            } else {
                margin + content_w
            },
            bottom: px(64.0) + px(16.0),
        },
        separator_top: RECT {
            left: margin,
            top: px(86.0),
            right: margin + content_w,
            bottom: px(87.0),
        },
        separator_bottom: RECT {
            left: margin,
            top: latency_top - px(12.0),
            right: margin + content_w,
            bottom: latency_top - px(11.0),
        },
        cards,
        latency_text: RECT {
            left: margin,
            top: latency_top,
            right: margin + content_w / 2,
            bottom: latency_top + px(16.0),
        },
        bytes_text: RECT {
            left: margin + content_w / 2,
            top: latency_top,
            right: margin + content_w,
            bottom: latency_top + px(16.0),
        },
        error_text,
        notice,
        buttons,
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

/// 绘制一帧（画进双缓冲的内存 DC）。
fn paint(ui: &UiState, hdc: windows::Win32::Graphics::Gdi::HDC, width: i32, height: i32) {
    let layout = layout(ui, width, height);
    let scale = ui.theme.scale;
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
                for separator in [&layout.separator_top, &layout.separator_bottom] {
                    gfx.line(
                        separator.left as f32,
                        separator.top as f32 + 0.5,
                        separator.right as f32,
                        separator.top as f32 + 0.5,
                        COLOR_SEPARATOR,
                        scale.max(1.0),
                    );
                }
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

        let values: [(&str, String); 4] = [
            ("请求", format_thousands(ui.snapshot.total)),
            ("活跃", ui.snapshot.active.max(0).to_string()),
            ("预检", format_thousands(ui.snapshot.preflight)),
            ("错误", format_thousands(ui.error_total())),
        ];
        let card_padding = (10.0 * scale).round() as i32;
        for (index, card) in layout.cards.iter().enumerate() {
            let (label, value) = match values.get(index) {
                Some(item) => item.clone(),
                None => ("", String::new()),
            };
            let middle = card.top + (card.bottom - card.top) * 52 / 100;
            draw_text(
                hdc,
                ui.theme.font_number,
                COLOR_INK,
                RECT {
                    left: card.left + card_padding,
                    top: card.top + card_padding,
                    right: card.right - card_padding,
                    bottom: middle,
                },
                &value,
                DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );
            draw_text(
                hdc,
                ui.theme.font_small,
                COLOR_INK_SOFT,
                RECT {
                    left: card.left + card_padding,
                    top: middle,
                    right: card.right - card_padding,
                    bottom: card.bottom - card_padding / 2,
                },
                label,
                DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );
        }

        let latency = format!(
            "上游延迟 p50 {} / p95 {}",
            ui.latency_p50.clone().unwrap_or_else(|| "—".to_string()),
            ui.latency_p95.clone().unwrap_or_else(|| "—".to_string()),
        );
        draw_text(
            hdc,
            ui.theme.font_small,
            COLOR_INK_SOFT,
            layout.latency_text,
            &latency,
            DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
        );
        let traffic = format!(
            "↑{}  ↓{}",
            format_bytes(ui.snapshot.bytes_in),
            format_bytes(ui.snapshot.bytes_out)
        );
        draw_text(
            hdc,
            ui.theme.font_small,
            COLOR_INK_SOFT,
            layout.bytes_text,
            &traffic,
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
    }
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
                    let ui = &*pointer;
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
                    let (min_width, min_height) = min_window_size(&(*pointer).theme);
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

fn min_window_size(theme: &Theme) -> (i32, i32) {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: theme.px(420.0),
        bottom: theme.px(300.0),
    };
    unsafe {
        let _ = AdjustWindowRectEx(&mut rect, WS_OVERLAPPEDWINDOW, false, WINDOW_EX_STYLE(0));
    }
    (rect.right - rect.left, rect.bottom - rect.top)
}

/// 供 `UiState` 使用的"最小绘制刷新"。
pub fn invalidate(ui: &UiState) {
    if !ui.hwnd.is_invalid() {
        unsafe {
            let _ = InvalidateRect(Some(ui.hwnd), None, false);
        }
    }
}

/// 消息循环（主线程；返回时说明收到了 `WM_QUIT`）。
pub fn message_loop() {
    let mut message = windows::Win32::UI::WindowsAndMessaging::MSG::default();
    unsafe {
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
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

    #[test]
    fn min_size_and_hit_testing() {
        let theme = Theme::new(96);
        let (width, height) = min_window_size(&theme);
        assert!(width > theme.px(420.0), "最小宽度必须覆盖客户区 420");
        assert!(height > theme.px(300.0), "最小高度必须覆盖客户区 300");
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
}
