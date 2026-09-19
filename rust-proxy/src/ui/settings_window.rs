//! 设置小窗（§7.4）：上游 / 监听 / 超时 / CORS / 日志 / 启动 六组字段 + `[测试上游]` `[应用]`。
//!
//! 口径（P6-S4 去配置文件后的新语义，方案 §3.2）：
//! - **[应用]** = 校验 → **仅内存生效**（`Controller::apply`）→ 结果行提示；**不写任何文件**
//!   （程序没有配置文件；改动的生命周期 = 本次运行，关掉程序即回到默认/命令行值）。
//!   失败在新配置全端口绑不上时由服务侧**回滚到旧配置**（仅内存，见 `service.rs`），
//!   结果回填到本窗结果行；
//! - **没有"以文件为准"**：FR-38 的打开时读盘热生效随去配置文件一并删除；
//! - `[测试上游]` 是**唯一**的主动探测（由用户触发，FR-8）：GET `{upstream}/v1/models`，显示状态码与耗时。
//!
//! 控件全部用系统类（STATIC/EDIT/BUTTON/COMBOBOX）+ comctl32 v6 清单 ⇒ Win7 上就是原生观感；
//! 窗口固定尺寸（不可缩放），DPI 变化时按 `WM_DPICHANGED` 重建。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{FillRect, SetBkMode, TRANSPARENT};
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect,
    GetWindowLongPtrW, SendMessageW, SetForegroundWindow, SetWindowLongPtrW, SetWindowPos,
    SetWindowTextW, ShowWindow, BS_AUTOCHECKBOX, BS_PUSHBUTTON, CBS_DROPDOWNLIST, CREATESTRUCTW,
    CW_USEDEFAULT, ES_AUTOHSCROLL, ES_NUMBER, GWLP_USERDATA, HMENU, SWP_NOACTIVATE, SWP_NOZORDER,
    SW_HIDE, SW_SHOW, WINDOW_EX_STYLE, WM_CLOSE, WM_COMMAND, WM_CREATE, WM_CTLCOLORBTN,
    WM_CTLCOLORSTATIC, WM_DESTROY, WM_DPICHANGED, WM_ERASEBKGND, WM_NCCREATE, WM_NCDESTROY,
    WM_SETFONT, WNDCLASSW, WS_BORDER, WS_CAPTION, WS_CHILD, WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP,
    WS_VISIBLE, WS_VSCROLL,
};

use azusa_local_proxy::logging::{Logger, LOG_RING_CAPACITY};

use super::theme::{gdi_color, wide, COLOR_INK};
use super::{UiState, SETTINGS_WINDOW_CLASS, WM_APP_TEST_RESULT};

/// 设置窗控件 ID（区间 3001–3099）。
const IDC_FIELD_UPSTREAM: usize = 3001;
const IDC_FIELD_ADVANCED: usize = 3002;
const IDC_FIELD_STRIP: usize = 3003;
const IDC_FIELD_PORT: usize = 3004;
const IDC_FIELD_FALLBACK: usize = 3005;
const IDC_FIELD_CONNECT: usize = 3006;
const IDC_FIELD_FIRST_BYTE: usize = 3007;
const IDC_FIELD_CORS_MODE: usize = 3008;
const IDC_FIELD_CORS_MAX_AGE: usize = 3009;
const IDC_FIELD_LOG_LEVEL: usize = 3010;
/// 只读读数行：「日志（内存）」的 条数 / 上限 / 已挤出（P6-S5 / D12）。
const IDC_FIELD_LOG_STAT: usize = 3011;
/// 「查看日志」（弹窗显示内存环形缓冲最近 100 行）。
const IDC_VIEW_LOGS: usize = 3012;
const IDC_FIELD_CLOSE_ACTION: usize = 3015;
const IDC_RESULT: usize = 3016;
const IDC_TEST: usize = 3017;
const IDC_APPLY: usize = 3018;
/// 「复制日志到剪贴板」（P6-S5 / D12：留存日志的唯一出口，不落盘）。
const IDC_COPY_LOGS: usize = 3019;

/// 「测试上游」的结果槽（工作线程写 → `WM_APP_TEST_RESULT` → UI 线程读）。
static TEST_RESULT: Mutex<Option<String>> = Mutex::new(None);

/// 设置窗的全部控件句柄（创建后挂到 `UiState::settings_controls`）。
pub struct SettingsControls {
    pub upstream: HWND,
    pub advanced: HWND,
    pub strip_label: HWND,
    pub strip: HWND,
    pub port: HWND,
    pub fallback: HWND,
    pub connect: HWND,
    pub first_byte: HWND,
    pub cors_mode: HWND,
    pub cors_max_age: HWND,
    pub log_level: HWND,
    /// 只读读数行（`共 N / 上限 2000 条（挤出 M）`）。
    pub log_stat: HWND,
    pub view_logs: HWND,
    pub copy_logs: HWND,
    pub close_action: HWND,
    pub result: HWND,
    pub test_button: HWND,
    pub apply_button: HWND,
}

pub fn register_class(hinst: HINSTANCE, logger: &Arc<Logger>) -> Result<(), String> {
    let class_name = wide(SETTINGS_WINDOW_CLASS);
    // 窗口类图标（标题栏/任务栏）—— 与主窗**同一条入口** `theme::load_app_icon`（NEW-1 回归点：
    // 本窗曾漏设 `WNDCLASSW.hIcon` ⇒ 标题栏显示系统占位图标；资源 ID 真值只此一处来源，
    // 不得在此复制粘贴第二份）。
    let icon = super::theme::load_app_icon(hinst, 32);
    if icon.is_invalid() {
        logger.warn(&format!(
            "设置窗图标加载失败（资源 ID {}）：标题栏将显示系统占位图标",
            super::theme::APP_ICON_RESOURCE_ID
        ));
    }
    unsafe {
        let cursor = windows::Win32::UI::WindowsAndMessaging::LoadCursorW(
            None,
            windows::Win32::UI::WindowsAndMessaging::IDC_ARROW,
        )
        .map_err(|err| format!("加载光标失败：{err}"))?;
        let window_class = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            hIcon: icon,
            hCursor: cursor,
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        if register_class_w_checked(&window_class) == 0 {
            return Err(format!("注册窗口类 {SETTINGS_WINDOW_CLASS} 失败"));
        }
    }
    Ok(())
}

/// `RegisterClassW` 薄包装（同一函数名在 windows crate 里返回 ATOM；这里只判 0）。
unsafe fn register_class_w_checked(class: &WNDCLASSW) -> u16 {
    unsafe { windows::Win32::UI::WindowsAndMessaging::RegisterClassW(class) }
}

/// 打开（或聚焦）设置小窗。
pub fn open(ui: &mut UiState) {
    if let Some(hwnd) = ui.settings_hwnd {
        if !hwnd.is_invalid()
            && unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindow(Some(hwnd)).as_bool() }
        {
            unsafe {
                let _ = ShowWindow(hwnd, SW_SHOW);
                let _ = SetForegroundWindow(hwnd);
            }
            return;
        }
    }
    // P6-S4：不再有"以文件为准"（配置文件已彻底移除）—— 打开设置窗直接以内存配置填字段。
    let class_name = wide(SETTINGS_WINDOW_CLASS);
    let title = wide("设置 — AzusaAI 本地反向代理");
    let scale = ui.theme.scale;
    let client_width = (420.0 * scale).round() as i32;
    let client_height = (362.0 * scale).round() as i32;
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: client_width,
        bottom: client_height,
    };
    unsafe {
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
        let _ = AdjustWindowRectEx(&mut rect, style, false, WINDOW_EX_STYLE(0));
        let pointer = ui as *mut UiState;
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(title.as_ptr()),
            style,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            rect.right - rect.left,
            rect.bottom - rect.top,
            Some(ui.hwnd),
            None,
            Some(ui.hinst),
            Some(pointer as *const core::ffi::c_void),
        );
        match hwnd {
            Ok(hwnd) => {
                ui.settings_hwnd = Some(hwnd);
                let _ = ShowWindow(hwnd, SW_SHOW);
                let _ = SetForegroundWindow(hwnd);
            }
            Err(err) => {
                ui.logger.error(&format!("创建设置窗口失败：{err}"));
            }
        }
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

/// Win32 控件创建的标准形态（`CreateWindowExW` 的位置/尺寸/样式 + ID + 字体）；
/// 调用点近 20 处，拆结构体只是把同一堆参数搬个地方 ⇒ 显式豁免"参数过多"。
#[allow(clippy::too_many_arguments)]
fn create_control(
    parent: HWND,
    hinst: HINSTANCE,
    class: &str,
    text: &str,
    style: u32,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    id: usize,
    font: windows::Win32::Graphics::Gdi::HFONT,
) -> HWND {
    let class = wide(class);
    let label = wide(text);
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class.as_ptr()),
            PCWSTR(label.as_ptr()),
            windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(style),
            x,
            y,
            width,
            height,
            Some(parent),
            Some(HMENU(id as *mut core::ffi::c_void)),
            Some(hinst),
            None,
        )
    }
    .unwrap_or_default();
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

fn set_text(hwnd: HWND, text: &str) {
    if hwnd.is_invalid() {
        return;
    }
    let label = wide(text);
    unsafe {
        let _ = SetWindowTextW(hwnd, PCWSTR(label.as_ptr()));
    }
}

fn get_text(hwnd: HWND) -> String {
    if hwnd.is_invalid() {
        return String::new();
    }
    let mut buffer = vec![0u16; 1024];
    let length =
        unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowTextW(hwnd, &mut buffer) };
    if length <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buffer[..length as usize])
}

fn combo_set_selection(hwnd: HWND, index: i32) {
    unsafe {
        SendMessageW(
            hwnd,
            windows::Win32::UI::WindowsAndMessaging::CB_SETCURSEL,
            Some(WPARAM(index.max(0) as usize)),
            Some(LPARAM(0)),
        );
    }
}

fn combo_selection(hwnd: HWND) -> i32 {
    unsafe {
        SendMessageW(
            hwnd,
            windows::Win32::UI::WindowsAndMessaging::CB_GETCURSEL,
            Some(WPARAM(0)),
            Some(LPARAM(0)),
        )
        .0 as i32
    }
}

fn combo_add(hwnd: HWND, items: &[&str]) {
    for item in items {
        let text = wide(item);
        unsafe {
            SendMessageW(
                hwnd,
                windows::Win32::UI::WindowsAndMessaging::CB_ADDSTRING,
                Some(WPARAM(0)),
                Some(LPARAM(text.as_ptr() as isize)),
            );
        }
    }
}

fn checkbox_set(hwnd: HWND, checked: bool) {
    unsafe {
        SendMessageW(
            hwnd,
            windows::Win32::UI::WindowsAndMessaging::BM_SETCHECK,
            Some(WPARAM(if checked { 1 } else { 0 })),
            Some(LPARAM(0)),
        );
    }
}

fn checkbox_get(hwnd: HWND) -> bool {
    unsafe {
        SendMessageW(
            hwnd,
            windows::Win32::UI::WindowsAndMessaging::BM_GETCHECK,
            Some(WPARAM(0)),
            Some(LPARAM(0)),
        )
        .0 != 0
    }
}

/// 用当前配置填满控件。
fn fill_fields(ui: &UiState, controls: &SettingsControls) {
    let config = ui.status.config.clone();
    set_text(controls.upstream, config.upstream_base.trim());
    set_text(controls.strip, &config.strip_prefix);
    set_text(controls.port, &config.listen_port.to_string());
    let fallback = config
        .port_fallback
        .iter()
        .map(|port| port.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    set_text(controls.fallback, &fallback);
    set_text(
        controls.connect,
        &(config.timeouts.connect_ms / 1000).max(1).to_string(),
    );
    set_text(
        controls.first_byte,
        &(config.timeouts.first_byte_ms / 1000).to_string(),
    );
    combo_set_selection(
        controls.cors_mode,
        if config.cors.mode == "echo" { 1 } else { 0 },
    );
    set_text(
        controls.cors_max_age,
        &config.cors.max_age_seconds.to_string(),
    );
    let level_index = match config.logging.level.as_str() {
        "error" => 0,
        "warn" => 1,
        "debug" => 3,
        _ => 2,
    };
    combo_set_selection(controls.log_level, level_index);
    update_log_stat(ui, controls);
    combo_set_selection(
        controls.close_action,
        if config.ui.close_action == "exit" {
            1
        } else {
            0
        },
    );
    unsafe {
        let _ = ShowWindow(controls.strip, SW_HIDE);
        let _ = ShowWindow(controls.strip_label, SW_HIDE);
    }
    checkbox_set(controls.advanced, false);
}

/// 结果行（普通提示 / 错误提示）。
fn set_result(ui: &UiState, text: &str, error: bool) {
    if let Some(controls) = ui.settings_controls.as_ref() {
        let prefix = if error { "✗ " } else { "" };
        set_text(controls.result, &format!("{prefix}{text}"));
    }
}

/// 只读读数行（P6-S5 / D12）：内存环的 条数 / 硬上限 / 被挤出条数。
fn update_log_stat(ui: &UiState, controls: &SettingsControls) {
    set_text(
        controls.log_stat,
        &format!(
            "{}/{} 条（挤出 {}）",
            ui.logger.retained_count(),
            LOG_RING_CAPACITY,
            ui.logger.dropped_count()
        ),
    );
}

/// 点「查看日志」/「复制到剪贴板」前刷新读数行（条数与挤出数随运行时间变化）。
fn refresh_log_stat(ui: &UiState) {
    if let Some(controls) = ui.settings_controls.as_ref() {
        update_log_stat(ui, controls);
    }
}

/// 应用流程：校验 → **仅内存生效**（P6-S4：不写任何文件；失败回滚由服务侧做，
/// 结果经 `refresh_apply_feedback` 回填）。
/// （K13：原"另需把「开机自动运行」勾选框落到注册表"的半句随自启功能整体移除。）
fn apply_fields(ui: &UiState) {
    let Some(controls) = ui.settings_controls.as_ref() else {
        return;
    };
    let mut config = ui.status.config.clone();
    let parse_u16 = |text: String, field: &str| -> Result<u16, String> {
        text.trim()
            .parse::<u16>()
            .map_err(|_| format!("{field} 必须是 1–65535 的整数（当前「{}」）", text.trim()))
    };
    let parse_u64 = |text: String, field: &str| -> Result<u64, String> {
        text.trim()
            .parse::<u64>()
            .map_err(|_| format!("{field} 必须是非负整数（当前「{}」）", text.trim()))
    };

    let validated: Result<(), String> = (|| {
        config.upstream_base = get_text(controls.upstream).trim().to_string();
        config.strip_prefix = get_text(controls.strip).trim().to_string();
        config.listen_port = parse_u16(get_text(controls.port), "监听端口")?;
        let fallback_text = get_text(controls.fallback);
        let mut fallback = Vec::new();
        for part in fallback_text
            .split([',', '，', ' ', ';', '、'])
            .filter(|piece| !piece.trim().is_empty())
        {
            fallback.push(
                part.trim()
                    .parse::<u16>()
                    .map_err(|_| format!("候选端口「{}」不是合法端口", part.trim()))?,
            );
        }
        config.port_fallback = fallback;
        let connect_seconds = parse_u64(get_text(controls.connect), "连接超时（秒）")?;
        if connect_seconds == 0 {
            return Err("连接超时必须 ≥ 1 秒（0 会让不可达上游永远挂住）".to_string());
        }
        config.timeouts.connect_ms = connect_seconds * 1000;
        config.timeouts.first_byte_ms =
            parse_u64(get_text(controls.first_byte), "首字节超时（秒）")? * 1000;
        config.cors.mode = if combo_selection(controls.cors_mode) == 1 {
            "echo".to_string()
        } else {
            "*".to_string()
        };
        config.cors.max_age_seconds =
            parse_u64(get_text(controls.cors_max_age), "预热 Max-Age（秒）")?;
        config.logging.level = match combo_selection(controls.log_level) {
            0 => "error".to_string(),
            1 => "warn".to_string(),
            3 => "debug".to_string(),
            _ => "info".to_string(),
        };
        config.ui.close_action = if combo_selection(controls.close_action) == 1 {
            "exit".to_string()
        } else {
            "tray".to_string()
        };
        config.validate().map_err(|err| err.to_string())?;
        Ok(())
    })();

    if let Err(err) = validated {
        set_result(ui, &err, true);
        return;
    }

    set_result(ui, "正在应用到本次运行…", false);
    ui.controller.apply(config);
}

/// 配置热生效的结果回填（`UiState::refresh` 里在 `apply_seq` 变化时调用）。
pub fn refresh_apply_feedback(ui: &mut UiState) {
    if ui.settings_hwnd.is_none() {
        return;
    }
    let message = ui.status.apply_message.clone();
    let ok = ui.status.apply_ok;
    if let Some(message) = message {
        set_result(ui, &message, !ok);
    }
}

/// 「测试上游」：唯一的主动探测（用户触发）。
fn test_upstream(ui: &mut UiState) {
    let Some(controls) = ui.settings_controls.as_ref() else {
        return;
    };
    let base = get_text(controls.upstream)
        .trim()
        .trim_end_matches('/')
        .to_string();
    if base.is_empty() {
        set_result(ui, "上游 Base URL 为空", true);
        return;
    }
    let url = format!("{base}/v1/models");
    // `HWND` 不是 `Send` ⇒ 跨线程只传裸地址，回投时重建（窗口已销毁时 `PostMessageW` 是无害 no-op）
    let hwnd_raw = ui.settings_hwnd.unwrap_or(ui.hwnd).0 as isize;
    set_result(ui, &format!("测试中…（GET {url}，超时 10 s）"), false);
    let logger = ui.logger.clone();
    ui.controller.runtime().spawn(async move {
        let started = Instant::now();
        let text = match probe(&url).await {
            Ok(status) => format!(
                "HTTP {status} · {} ms（{url}）",
                started.elapsed().as_millis()
            ),
            Err(err) => format!(
                "失败：{err} · {} ms（{url}）",
                started.elapsed().as_millis()
            ),
        };
        logger.info(&format!("测试上游：{text}"));
        if let Ok(mut slot) = TEST_RESULT.lock() {
            *slot = Some(text);
        }
        unsafe {
            use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
            let window = HWND(hwnd_raw as *mut std::ffi::c_void);
            // 设置窗可能已被用户关掉：先 `IsWindow` 预检再投递（审查 P2-1；残余风险 = 句柄被系统复用）
            if windows::Win32::UI::WindowsAndMessaging::IsWindow(Some(window)).as_bool() {
                let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                    Some(window),
                    WM_APP_TEST_RESULT,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        }
    });
}

async fn probe(url: &str) -> Result<u16, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|err| err.to_string())?;
    match client.get(url).send().await {
        Ok(response) => Ok(response.status().as_u16()),
        Err(err) => {
            if err.is_timeout() {
                Err("超时（10 s）".to_string())
            } else if err.is_connect() {
                Err("连接失败（不可达 / 拒绝）".to_string())
            } else {
                Err(err.to_string())
            }
        }
    }
}

/// `[高级选项]` 勾选 ⇒ 显示"剥离前缀"行。
fn toggle_advanced(ui: &UiState) {
    let Some(controls) = ui.settings_controls.as_ref() else {
        return;
    };
    let show = checkbox_get(controls.advanced);
    unsafe {
        let _ = ShowWindow(controls.strip, if show { SW_SHOW } else { SW_HIDE });
        let _ = ShowWindow(controls.strip_label, if show { SW_SHOW } else { SW_HIDE });
    }
}

fn build_controls(ui: &mut UiState, hwnd: HWND) {
    let hinst = ui.hinst;
    let font = ui.theme.font_ui;
    let scale = ui.theme.scale;
    let px = |value: f32| (value * scale).round() as i32;
    let label_w = px(118.0);
    let field_x = px(140.0);
    let field_w = px(264.0);
    let row_h = px(22.0);
    let step = px(30.0);
    let base_y = px(12.0);
    let row = |index: i32| base_y + step * index;
    let small_w = px(64.0);

    // 控件样式：`WS_*` 是 WINDOW_STYLE（u32 newtype），`ES_*`/`BS_*`/`CBS_*` 是 i32 常量 ⇒ 统一成 u32
    let base = (WS_CHILD | WS_VISIBLE).0;
    let static_style = base;
    let make_label = |text: &str, x: i32, y: i32, width: i32| {
        create_control(
            hwnd,
            hinst,
            "STATIC",
            text,
            static_style,
            x,
            y,
            width,
            row_h,
            0,
            font,
        )
    };
    let edit_style = base | WS_BORDER.0 | ES_AUTOHSCROLL as u32;
    let number_style = edit_style | ES_NUMBER as u32;
    let combo_style = base | WS_TABSTOP.0 | CBS_DROPDOWNLIST as u32 | WS_VSCROLL.0;
    let checkbox_style = base | WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32;
    let button_style = base | WS_TABSTOP.0 | BS_PUSHBUTTON as u32;
    let _ = make_label("", 0, 0, 0);

    let upstream_label = make_label("上游 Base URL", px(16.0), row(0), label_w);
    let upstream = create_control(
        hwnd,
        hinst,
        "EDIT",
        "",
        edit_style,
        field_x,
        row(0),
        field_w,
        row_h,
        IDC_FIELD_UPSTREAM,
        font,
    );
    let advanced = create_control(
        hwnd,
        hinst,
        "BUTTON",
        "高级选项（剥离前缀）",
        checkbox_style,
        px(16.0),
        row(1),
        px(220.0),
        row_h,
        IDC_FIELD_ADVANCED,
        font,
    );
    let strip_label = make_label("剥离前缀", px(16.0), row(2), label_w);
    let strip = create_control(
        hwnd,
        hinst,
        "EDIT",
        "",
        edit_style,
        field_x,
        row(2),
        field_w,
        row_h,
        IDC_FIELD_STRIP,
        font,
    );
    let port_label = make_label("监听端口", px(16.0), row(3), label_w);
    let port = create_control(
        hwnd,
        hinst,
        "EDIT",
        "",
        number_style,
        field_x,
        row(3),
        small_w,
        row_h,
        IDC_FIELD_PORT,
        font,
    );
    let fallback_label = make_label("候选端口", px(216.0), row(3), px(56.0));
    let fallback = create_control(
        hwnd,
        hinst,
        "EDIT",
        "",
        edit_style,
        px(276.0),
        row(3),
        px(128.0),
        row_h,
        IDC_FIELD_FALLBACK,
        font,
    );
    let connect_label = make_label("连接超时(秒)", px(16.0), row(4), label_w);
    let connect = create_control(
        hwnd,
        hinst,
        "EDIT",
        "",
        number_style,
        field_x,
        row(4),
        small_w,
        row_h,
        IDC_FIELD_CONNECT,
        font,
    );
    let first_byte_label = make_label("首字节(0=不限)", px(216.0), row(4), px(96.0));
    let first_byte = create_control(
        hwnd,
        hinst,
        "EDIT",
        "",
        number_style,
        px(316.0),
        row(4),
        px(88.0),
        row_h,
        IDC_FIELD_FIRST_BYTE,
        font,
    );
    let cors_label = make_label("Allow-Origin 模式", px(16.0), row(5), label_w);
    // 审查 P2-8：96 px 装不下原文案「*（固定通配）」⇒ 组合框调宽到 100 + 文案收短（语义不变）；
    // 右边「预热 Max-Age」标签保留 88 px（实测：再窄会把 Max-Age 截掉）
    let cors_mode = create_control(
        hwnd,
        hinst,
        "COMBOBOX",
        "",
        combo_style,
        field_x,
        row(5),
        px(100.0),
        px(120.0),
        IDC_FIELD_CORS_MODE,
        font,
    );
    let max_age_label = make_label("预热 Max-Age", px(244.0), row(5), px(88.0));
    let cors_max_age = create_control(
        hwnd,
        hinst,
        "EDIT",
        "",
        number_style,
        px(336.0),
        row(5),
        px(68.0),
        row_h,
        IDC_FIELD_CORS_MAX_AGE,
        font,
    );
    let level_label = make_label("日志级别", px(16.0), row(6), label_w);
    let log_level = create_control(
        hwnd,
        hinst,
        "COMBOBOX",
        "",
        combo_style,
        field_x,
        row(6),
        px(96.0),
        px(120.0),
        IDC_FIELD_LOG_LEVEL,
        font,
    );
    // P6-S5 / D12：日志只剩内存环 ⇒ 删「保留文件数」「日志目录」两个字段，
    // 改为只读读数行 + 两个动作按钮（查看日志 / 复制日志到剪贴板）。
    let stat_label = make_label("日志（内存）", px(248.0), row(6), px(84.0));
    let log_stat = create_control(
        hwnd,
        hinst,
        "STATIC",
        "",
        static_style,
        px(336.0),
        row(6),
        px(68.0),
        row_h,
        IDC_FIELD_LOG_STAT,
        font,
    );
    let view_logs = create_control(
        hwnd,
        hinst,
        "BUTTON",
        "查看日志",
        button_style,
        px(16.0),
        row(7),
        px(88.0),
        px(26.0),
        IDC_VIEW_LOGS,
        font,
    );
    let copy_logs = create_control(
        hwnd,
        hinst,
        "BUTTON",
        "复制到剪贴板",
        button_style,
        px(112.0),
        row(7),
        px(120.0),
        px(26.0),
        IDC_COPY_LOGS,
        font,
    );
    let close_label = make_label("关闭主窗时", px(16.0), row(8), label_w);
    let close_action = create_control(
        hwnd,
        hinst,
        "COMBOBOX",
        "",
        combo_style,
        field_x,
        row(8),
        px(160.0),
        px(120.0),
        IDC_FIELD_CLOSE_ACTION,
        font,
    );
    // P6-S4：设置窗的全部字段都只对**本次运行**生效（没有配置文件）—— 在关闭行为旁注明。
    let close_note = make_label("（仅本次运行）", px(306.0), row(8), px(110.0));
    let result = create_control(
        hwnd,
        hinst,
        "STATIC",
        "",
        static_style,
        px(16.0),
        row(9),
        px(388.0),
        px(30.0),
        IDC_RESULT,
        font,
    );
    let test_button = create_control(
        hwnd,
        hinst,
        "BUTTON",
        "测试上游",
        button_style,
        px(16.0),
        row(10),
        px(88.0),
        px(26.0),
        IDC_TEST,
        font,
    );
    let apply_button = create_control(
        hwnd,
        hinst,
        "BUTTON",
        "应用",
        button_style,
        px(332.0),
        row(10),
        px(72.0),
        px(26.0),
        IDC_APPLY,
        font,
    );

    combo_add(cors_mode, &["*（通配）", "回显 Origin"]);
    combo_add(log_level, &["error", "warn", "info", "debug"]);
    combo_add(close_action, &["隐藏到托盘", "直接退出"]);
    let _ = (
        upstream_label,
        port_label,
        fallback_label,
        connect_label,
        first_byte_label,
        cors_label,
        max_age_label,
        level_label,
        stat_label,
        close_label,
        close_note,
    );

    let controls = SettingsControls {
        upstream,
        advanced,
        strip_label,
        strip,
        port,
        fallback,
        connect,
        first_byte,
        cors_mode,
        cors_max_age,
        log_level,
        log_stat,
        view_logs,
        copy_logs,
        close_action,
        result,
        test_button,
        apply_button,
    };
    fill_fields(ui, &controls);
    ui.settings_controls = Some(controls);
}

/// 设置窗消息处理。
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
                    }
                }
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
            WM_CREATE => {
                let created = with_ui(hwnd, |ui| build_controls(ui, hwnd));
                if created.is_none() {
                    return LRESULT(-1);
                }
                LRESULT(0)
            }
            WM_COMMAND => {
                let id = wparam.0 & 0xFFFF;
                with_ui(hwnd, |ui| match id {
                    IDC_APPLY => apply_fields(ui),
                    IDC_TEST => test_upstream(ui),
                    IDC_VIEW_LOGS => {
                        refresh_log_stat(ui);
                        ui.show_logs();
                    }
                    IDC_COPY_LOGS => {
                        refresh_log_stat(ui);
                        ui.copy_logs();
                    }
                    IDC_FIELD_ADVANCED => toggle_advanced(ui),
                    _ => {}
                });
                LRESULT(0)
            }
            WM_APP_TEST_RESULT => {
                let text = TEST_RESULT.lock().ok().and_then(|slot| slot.clone());
                with_ui(hwnd, |ui| {
                    if let (Some(text), Some(controls)) = (text, ui.settings_controls.as_ref()) {
                        set_text(controls.result, &format!("测试结果：{text}"));
                    }
                });
                LRESULT(0)
            }
            WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => {
                let hdc = windows::Win32::Graphics::Gdi::HDC(wparam.0 as *mut core::ffi::c_void);
                let pointer = ui_ptr(hwnd);
                if !pointer.is_null() {
                    let _ = SetBkMode(hdc, TRANSPARENT);
                    let _ = windows::Win32::Graphics::Gdi::SetTextColor(hdc, gdi_color(COLOR_INK));
                    return LRESULT((*pointer).theme.brush_bg.0 as isize);
                }
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
            WM_ERASEBKGND => {
                let hdc = windows::Win32::Graphics::Gdi::HDC(wparam.0 as *mut core::ffi::c_void);
                let pointer = ui_ptr(hwnd);
                if !pointer.is_null() {
                    let mut rect = RECT::default();
                    let _ = GetClientRect(hwnd, &mut rect);
                    FillRect(hdc, &rect, (*pointer).theme.brush_bg);
                    return LRESULT(1);
                }
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
            WM_DPICHANGED => {
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
                    // 字体变了：控件字号跟着更新（位置保持 → 允许轻微不精确，见报告"未覆盖边界"）
                    if let Some(controls) = ui.settings_controls.as_ref() {
                        for hwnd in [
                            controls.upstream,
                            controls.advanced,
                            controls.strip,
                            controls.port,
                            controls.fallback,
                            controls.connect,
                            controls.first_byte,
                            controls.cors_mode,
                            controls.cors_max_age,
                            controls.log_level,
                            controls.log_stat,
                            controls.view_logs,
                            controls.copy_logs,
                            controls.close_action,
                            controls.result,
                            controls.test_button,
                            controls.apply_button,
                        ] {
                            if !hwnd.is_invalid() {
                                SendMessageW(
                                    hwnd,
                                    WM_SETFONT,
                                    Some(WPARAM(ui.theme.font_ui.0 as usize)),
                                    Some(LPARAM(1)),
                                );
                            }
                        }
                    }
                });
                LRESULT(0)
            }
            WM_CLOSE => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                with_ui(hwnd, |ui| {
                    ui.settings_hwnd = None;
                    ui.settings_controls = None;
                });
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
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
