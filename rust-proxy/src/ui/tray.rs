//! 托盘（§7.2）：`Shell_NotifyIconW` + `TrackPopupMenu`，图标四态（运行 / 停止 / 降级 / 错误）。
//!
//! 图标来源 = 内嵌 `assets/app.ico`（16/32/48/256，Phase 0 起就入库）→ 运行期用
//! GDI（`DrawIconEx` 画底图）+ GDI+（状态点抗锯齿）合成为四个 `HICON`：
//! - 运行 = 原彩色图标；
//! - 停止 = 灰度化（去饱和）；
//! - 降级 = 原图 + 右下角黄点；
//! - 错误 = 原图 + 右下角红点。
//!
//! **不引入任何外部图片资源**（§7.5 第 4 条）；`CreateIconIndirect` 是 XP 起就有的 API。
//!
//! 气泡通知（§7.2）：只有三类**主动**通知 —— 服务异常 / 端口降级 / 首次启动；
//! 另外"关闭主窗隐藏到托盘"（P6-S7 / D2）提示一次、"经托盘菜单启停服务且主窗不可见"时提示一次（作为操作反馈）。

use std::sync::Arc;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
    SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HGDIOBJ,
};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_ERROR, NIIF_INFO,
    NIIF_WARNING, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyIcon, DestroyMenu, DrawIconEx, GetCursorPos, PostMessageW,
    SetForegroundWindow, TrackPopupMenu, DI_NORMAL, HICON, HMENU, MF_GRAYED, MF_SEPARATOR,
    MF_STRING, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_NULL,
};

use super::theme::{self, wide, Gfx};
use azusa_local_proxy::logging::Logger;
use azusa_local_proxy::service::{Phase, Status};

/// 托盘回调消息（`WM_APP + 2`；`WM_APP + 1` 是 service 的状态刷新消息）。
pub const WM_APP_TRAY: u32 = 0x8000 + 2;
pub const TRAY_UID: u32 = 1;

/// 托盘菜单项 ID（与窗口按钮 ID 分区间：按钮 = 1001+，托盘菜单 = 2001+）。
pub const IDM_SHOW: usize = 2001;
pub const IDM_TOGGLE_SERVICE: usize = 2002;
pub const IDM_COPY_BASE: usize = 2003;
/// 「查看日志」（P6-S5 / D12：内存环形缓冲面板；原「打开日志目录」已删）。
pub const IDM_VIEW_LOGS: usize = 2004;
/// 「复制日志到剪贴板」（P6-S5 / D12：留存日志的唯一出口，不落盘）。
pub const IDM_COPY_LOGS: usize = 2008;
// K13：原 `IDM_AUTOSTART`（2005）随开机自启功能整体移除 —— ID 号留空不回收（避免复用旧号）。
pub const IDM_ABOUT: usize = 2006;
pub const IDM_EXIT: usize = 2007;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconState {
    Running,
    Stopped,
    Degraded,
    Error,
}

impl IconState {
    pub fn label(self) -> &'static str {
        match self {
            IconState::Running => "运行中",
            IconState::Stopped => "已停止",
            IconState::Degraded => "已降级",
            IconState::Error => "错误",
        }
    }

    fn index(self) -> usize {
        match self {
            IconState::Running => 0,
            IconState::Stopped => 1,
            IconState::Degraded => 2,
            IconState::Error => 3,
        }
    }
}

/// 状态快照 → 托盘四态（§7.2 定义；"启动中"归入降级色 = 黄，与 §7.1 状态点口径一致）。
pub fn icon_state_for(status: &Status) -> IconState {
    match status.phase {
        Phase::Running if status.degraded => IconState::Degraded,
        Phase::Running => IconState::Running,
        Phase::Starting => IconState::Degraded,
        Phase::Error => IconState::Error,
        Phase::Stopped => IconState::Stopped,
    }
}

/// Tooltip：`AzusaAI 本地反代 · 运行中 · 127.0.0.1:80`（§7.2）。
pub fn tooltip_for(status: &Status) -> String {
    let state = icon_state_for(status);
    let address = status
        .listen_addr
        .clone()
        .unwrap_or_else(|| format!("未监听（主端口 {}）", status.requested_port));
    let text = format!("AzusaAI 本地反代 · {} · {address}", state.label());
    super::theme::truncate_line(&text, 120)
}

pub enum Balloon {
    Info,
    Warn,
    Error,
}

pub struct Tray {
    hwnd: HWND,
    icons: [HICON; 4],
    base_icon: HICON,
    added: bool,
    state: Option<IconState>,
    tooltip: String,
}

impl Tray {
    /// 建托盘图标（NIM_ADD）。合成图标失败 ⇒ 退回底图（保证托盘一定有图标）；
    /// **底图失败要告警**（P0-1 教训 2026-09-19：资源 ID 一旦不对，四态图标全静默变 NULL）。
    pub fn create(
        hwnd: HWND,
        hinst: HINSTANCE,
        state: IconState,
        tooltip: &str,
        logger: &Arc<Logger>,
    ) -> Tray {
        let size = small_icon_size();
        let base = theme::load_app_icon(hinst, size);
        if base.is_invalid() {
            logger.warn(&format!(
                "托盘底图加载失败（资源 ID {}）：托盘图标会变成系统占位/空白（检查 exe 资源嵌入与资源号）",
                theme::APP_ICON_RESOURCE_ID
            ));
        }
        let icons = [
            compose_icon(base, size, None).unwrap_or(base),
            compose_icon(base, size, Some((theme::COLOR_GRAY, true))).unwrap_or(base),
            compose_icon(base, size, Some((theme::COLOR_YELLOW, false))).unwrap_or(base),
            compose_icon(base, size, Some((theme::COLOR_RED, false))).unwrap_or(base),
        ];

        let mut tray = Tray {
            hwnd,
            icons,
            base_icon: base,
            added: false,
            state: None,
            tooltip: String::new(),
        };
        tray.add(state, tooltip);
        tray
    }

    fn data(&self, flags: windows::Win32::UI::Shell::NOTIFY_ICON_DATA_FLAGS) -> NOTIFYICONDATAW {
        let mut data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: TRAY_UID,
            uFlags: flags,
            uCallbackMessage: WM_APP_TRAY,
            hIcon: self.icon_handle(),
            ..Default::default()
        };
        copy_wide(&mut data.szTip, &self.tooltip);
        data
    }

    fn icon_handle(&self) -> HICON {
        let index = self.state.unwrap_or(IconState::Stopped).index();
        *self.icons.get(index).unwrap_or(&self.base_icon)
    }

    fn add(&mut self, state: IconState, tooltip: &str) {
        self.state = Some(state);
        self.tooltip = tooltip.to_string();
        let data = self.data(NIF_MESSAGE | NIF_ICON | NIF_TIP);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_ADD, &data);
        }
        self.added = true;
    }

    /// 状态/提示变化时刷新（相同则不动，避免无谓的 NIM_MODIFY）。
    pub fn update(&mut self, state: IconState, tooltip: &str) {
        if !self.added {
            self.add(state, tooltip);
            return;
        }
        if self.state == Some(state) && self.tooltip == tooltip {
            return;
        }
        self.state = Some(state);
        self.tooltip = tooltip.to_string();
        let data = self.data(NIF_MESSAGE | NIF_ICON | NIF_TIP);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
        }
    }

    /// 气泡通知（§7.2；三类主动通知 + 两条操作反馈）。
    pub fn balloon(&self, title: &str, text: &str, kind: Balloon) {
        if !self.added {
            return;
        }
        let mut data = self.data(NIF_INFO);
        copy_wide(&mut data.szInfoTitle, title);
        copy_wide(&mut data.szInfo, text);
        data.dwInfoFlags = match kind {
            Balloon::Info => NIIF_INFO,
            Balloon::Warn => NIIF_WARNING,
            Balloon::Error => NIIF_ERROR,
        };
        unsafe {
            data.Anonymous.uTimeout = 10_000;
            let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
        }
    }

    /// 弹右键菜单（§7.2 的项与顺序），返回被选中的命令 ID（0 = 未选）。
    ///
    /// ⚠ 证据口径（审查 P2-5）：菜单窗（类名 `#32768`）的 `PrintWindow` **只渲染首项** ⇒
    /// 菜单位图**不能自证**菜单项；内容一律以 `build_menu()` + `menu_contains_expected_items`
    /// 单测为准（本函数只负责"弹出来"）。
    pub fn show_menu(&self, status: &Status) -> usize {
        let menu = match build_menu(status) {
            Some(menu) => menu,
            None => return 0,
        };
        unsafe {
            // 弹菜单前的标准动作：置前台 + 取光标位置；结束后补一条 WM_NULL（经典要求）
            let _ = SetForegroundWindow(self.hwnd);
            let mut point = POINT::default();
            let _ = GetCursorPos(&mut point);
            let selected = TrackPopupMenu(
                menu,
                TPM_RIGHTBUTTON | TPM_RETURNCMD,
                point.x,
                point.y,
                None,
                self.hwnd,
                None,
            );
            let _ = PostMessageW(Some(self.hwnd), WM_NULL, WPARAM(0), LPARAM(0));
            let _ = DestroyMenu(menu);
            selected.0 as usize
        }
    }

    /// 提示"已隐藏到托盘"（P6-S7 / D2：**关闭**主窗时首次给一次；最小化不再触发，D1）。
    pub fn balloon_minimized(&self) {
        self.balloon(
            "AzusaAI 本地反代",
            "已隐藏到托盘，程序仍在后台运行：双击图标可恢复主窗口",
            Balloon::Info,
        );
    }

    /// 退出前移除图标（避免"幽灵图标"停留到下次探测）。
    pub fn remove(&mut self) {
        if self.added {
            let data = self.data(NIF_MESSAGE);
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &data);
            }
            self.added = false;
        }
    }

    /// 释放所有 HICON（进程退出前调用；`NIM_DELETE` 之后才允许）。
    pub fn destroy(&mut self) {
        self.remove();
        unsafe {
            for icon in self.icons.iter() {
                if !icon.is_invalid() {
                    let _ = DestroyIcon(*icon);
                }
            }
            if !self.base_icon.is_invalid() {
                let _ = DestroyIcon(self.base_icon);
            }
        }
        self.icons = [HICON::default(); 4];
        self.base_icon = HICON::default();
    }
}

/// 构建托盘菜单（**只建不弹**）：`show_menu` 与单测共用 ⇒ 菜单内容可被机械断言。
///
/// 证据口径（审查 P2-5）：菜单窗（类名 `#32768`）的 `PrintWindow` **只渲染首项** ⇒ 截图不能自证
/// 菜单项；内容一律以本函数 + `menu_contains_expected_items` 单测为准。
///
/// 结构（§7.2；P6-S5 起）= 9 项含 **2 条分隔线**：显示主窗口 / 启停（按状态二选一）/
/// 复制 Base URL〔未监听时置灰〕/ 查看日志 / 复制日志到剪贴板 ─ 关于 ─ 退出。
/// **K13**：原「开机自动运行」（含 `MF_CHECKED` 勾选态）整项移除 —— 程序不再有注册表面。
fn build_menu(status: &Status) -> Option<HMENU> {
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        // ⚠ 每个菜单项文本都必须**先把宽字符串绑成局部量**再取指针 ——
        // 直接传 `PCWSTR(wide(text).as_ptr())` 会让临时 Vec 在调用前就被释放（悬垂指针）。
        unsafe fn append(
            menu: HMENU,
            flags: windows::Win32::UI::WindowsAndMessaging::MENU_ITEM_FLAGS,
            id: usize,
            text: &str,
        ) {
            let text = wide(text);
            let _ = AppendMenuW(menu, flags, id, PCWSTR(text.as_ptr()));
        }

        let running = status.phase == Phase::Running;
        append(menu, MF_STRING, IDM_SHOW, "显示主窗口 (&W)");
        append(
            menu,
            MF_STRING,
            IDM_TOGGLE_SERVICE,
            if running {
                "停止服务 (&S)"
            } else {
                "启动服务 (&S)"
            },
        );
        append(
            menu,
            if status.base_url.is_some() {
                MF_STRING
            } else {
                MF_STRING | MF_GRAYED
            },
            IDM_COPY_BASE,
            "复制 Base URL (&B)",
        );
        append(menu, MF_STRING, IDM_VIEW_LOGS, "查看日志 (&L)");
        append(menu, MF_STRING, IDM_COPY_LOGS, "复制日志到剪贴板 (&C)");
        // K13：原「开机自动运行」项与它前面的分隔线随自启功能整体移除。
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        append(menu, MF_STRING, IDM_ABOUT, "关于 (&R)");
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        append(menu, MF_STRING, IDM_EXIT, "退出 (&X)");
        Some(menu)
    }
}

/// 托盘小图标尺寸（16 @100% DPI；高 DPI 下 24/32 —— 用系统指标而不是写死 16）。
fn small_icon_size() -> i32 {
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSMICON};
        let size = GetSystemMetrics(SM_CXSMICON);
        if size >= 16 {
            size
        } else {
            16
        }
    }
}

/// 离屏渲染**四态图标**并落盘（取证/回归用，**不依赖可见桌面** —— 审查 P2-2 的建议路径）。
///
/// 走的是与运行期**同一条**合成链：`theme::load_app_icon` → `compose_icon` → `DrawIconEx` 渲染；
/// 每态出两张：真实托盘尺寸 + 2×（肉眼核对色点）。返回写出的文件数。
/// 底图无效（P0-1 类回归）⇒ `Err`。
///
/// ⚠ **仅 `cargo test` 下编译**（D12：`--dump-icons` 参数与 release 面实现已删除；
/// 四态图标的 exe 级取证入口随之消失，回归能力由本测试保留 —— 方案 §3.3 末表 / K1）。
#[cfg(test)]
pub fn dump_icon_states(dir: &std::path::Path) -> Result<usize, String> {
    use windows::Win32::Graphics::GdiPlus::{GdiplusShutdown, GdiplusStartup, GdiplusStartupInput};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;

    let hinst: HINSTANCE = unsafe { GetModuleHandleW(PCWSTR::null()) }
        .unwrap_or_default()
        .into();
    let size = small_icon_size();
    let base = theme::load_app_icon(hinst, size);
    if base.is_invalid() {
        return Err(format!(
            "底图加载失败（资源 ID {}）—— exe 资源嵌入或资源号有问题",
            theme::APP_ICON_RESOURCE_ID
        ));
    }
    // 状态点靠 GDI+ 抗锯齿：本路径自备 GdiplusStartup（运行期由 `ui::run` 启动，
    // 但 `cargo test` 入口不会经过它）
    let mut token: usize = 0;
    let input = GdiplusStartupInput {
        GdiplusVersion: 1,
        ..Default::default()
    };
    let gdiplus_ok = unsafe { GdiplusStartup(&mut token, &input, std::ptr::null_mut()).0 == 0 };

    let result = (|| -> Result<usize, String> {
        std::fs::create_dir_all(dir).map_err(|err| format!("建目录失败：{err}"))?;
        let mut written = 0usize;
        for (key, dot) in [
            ("1-running", None),
            ("2-stopped", Some((theme::COLOR_GRAY, true))),
            ("3-degraded", Some((theme::COLOR_YELLOW, false))),
            ("4-error", Some((theme::COLOR_RED, false))),
        ] {
            let icon = compose_icon(base, size, dot).unwrap_or(base);
            for target in [size, size * 2] {
                let path = dir.join(format!("icon-{key}-{target}px.bmp"));
                write_icon_bmp(&path, icon, target)?;
                written += 1;
            }
        }
        Ok(written)
    })();

    if gdiplus_ok {
        unsafe {
            GdiplusShutdown(token);
        }
    }
    unsafe {
        let _ = DestroyIcon(base);
    }
    result
}

/// 把 `HICON` 渲染成 32bpp BMP（离屏；背景 = 深灰 0x20 便于看状态点；alpha 一律置 255）。
/// **仅 `cargo test` 下编译**（配合 `dump_icon_states`）。
#[cfg(test)]
fn write_icon_bmp(path: &std::path::Path, icon: HICON, size: i32) -> Result<(), String> {
    if icon.is_invalid() || size <= 0 {
        return Err("图标句柄无效".to_string());
    }
    unsafe {
        let screen = GetDC(None);
        let mut info = BITMAPINFO::default();
        info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = size;
        info.bmiHeader.biHeight = -size; // top-down
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB.0;
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(Some(screen), &info, DIB_RGB_COLORS, &mut bits, None, 0)
            .map_err(|err| format!("CreateDIBSection 失败：{err}"))?;
        let mem = CreateCompatibleDC(Some(screen));
        let previous = SelectObject(mem, HGDIOBJ(dib.0));
        if !bits.is_null() {
            // 预填深灰不透明底（DrawIconEx 不保证写 alpha）
            let pixels =
                std::slice::from_raw_parts_mut(bits as *mut u8, (size * size * 4) as usize);
            for chunk in pixels.chunks_exact_mut(4) {
                chunk.copy_from_slice(&[0x20, 0x20, 0x20, 0xFF]);
            }
        }
        let _ = DrawIconEx(mem, 0, 0, icon, size, size, 0, None, DI_NORMAL);
        let result = if bits.is_null() {
            Err("DIB 位不可读".to_string())
        } else {
            let pixels = std::slice::from_raw_parts(bits as *const u8, (size * size * 4) as usize);
            write_bmp(path, size, pixels).map_err(|err| format!("写 BMP 失败：{err}"))
        };
        SelectObject(mem, previous);
        let _ = DeleteObject(HGDIOBJ(dib.0));
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        result
    }
}

/// 32bpp top-down 像素 → BMP 文件（bottom-up 行序；不做压缩）。
/// **仅 `cargo test` 下编译**（配合 `dump_icon_states`）。
#[cfg(test)]
fn write_bmp(path: &std::path::Path, size: i32, top_down: &[u8]) -> std::io::Result<()> {
    let row = (size * 4) as usize;
    let expected = row * size as usize;
    let body = match top_down.get(..expected) {
        Some(body) => body,
        None => return Err(std::io::Error::other("像素缓冲长度不符")),
    };
    let mut out: Vec<u8> = Vec::with_capacity(54 + expected);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((54 + expected) as u32).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(expected as u32).to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for y in (0..size as usize).rev() {
        out.extend_from_slice(&body[y * row..(y + 1) * row]);
    }
    std::fs::write(path, out)
}

/// 合成一个状态图标：可选的"右下角状态点"（`dot` = `(ARGB, 是否同时去饱和)`）。
/// 去饱和用于"停止"态（灰）；`None` ⇒ 原图直接转 HICON。
fn compose_icon(base: HICON, size: i32, dot: Option<(u32, bool)>) -> Option<HICON> {
    if base.is_invalid() || size <= 0 {
        return None;
    }
    unsafe {
        let screen = GetDC(None);
        let mut info = BITMAPINFO::default();
        info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = size;
        info.bmiHeader.biHeight = -size; // top-down
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB.0;
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(Some(screen), &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        let mem = CreateCompatibleDC(Some(screen));
        let previous = SelectObject(mem, HGDIOBJ(dib.0));

        // 透明底 + 画原图（32bpp 图标走 alpha 通道）
        if !bits.is_null() {
            std::ptr::write_bytes(bits as *mut u8, 0, (size * size * 4) as usize);
        }
        let _ = DrawIconEx(mem, 0, 0, base, size, size, 0, None, DI_NORMAL);

        if let Some((color, desaturate)) = dot {
            if desaturate {
                desaturate_pixels(bits, size);
            }
            let radius = (size as f32 * 0.30).max(2.5);
            let center = size as f32 - radius - 0.5;
            if let Some(gfx) = Gfx::from_hdc(mem) {
                // 先画一圈白边（任何底色上都看得清），再画状态色
                gfx.fill_circle(center, center, radius + 1.0, 0xFFFFFFFF);
                gfx.fill_circle(center, center, radius, color);
            }
        }

        SelectObject(mem, previous);
        // 单色掩码位图：32bpp 图标走 alpha 通道，掩码只作形式要求 ⇒ 全 0（但**必须真分配够字节**：
        // monochrome 的步长 = ((width + 31) / 32) * 4 字节/行）
        let mask_bytes = vec![0u8; ((size + 31) / 32 * 4 * size) as usize];
        let mask = CreateBitmap(
            size,
            size,
            1,
            1,
            Some(mask_bytes.as_ptr() as *const core::ffi::c_void),
        );
        let icon_info = windows::Win32::UI::WindowsAndMessaging::ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: dib,
        };
        let icon = windows::Win32::UI::WindowsAndMessaging::CreateIconIndirect(&icon_info).ok();
        let _ = DeleteObject(HGDIOBJ(mask.0));
        let _ = DeleteObject(HGDIOBJ(dib.0));
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        icon
    }
}

/// DIB 像素去饱和（BGRA 排列；"停止"态图标）。
unsafe fn desaturate_pixels(bits: *mut core::ffi::c_void, size: i32) {
    if bits.is_null() || size <= 0 {
        return;
    }
    let count = (size * size) as usize;
    let pixels = bits as *mut u8;
    for index in 0..count {
        let offset = index * 4;
        let blue = unsafe { *pixels.add(offset) } as u32;
        let green = unsafe { *pixels.add(offset + 1) } as u32;
        let red = unsafe { *pixels.add(offset + 2) } as u32;
        let alpha = unsafe { *pixels.add(offset + 3) };
        // Rec.601 亮度；alpha 略降（"灰"的视觉语义 = 未启用）
        let luma = ((red * 299 + green * 587 + blue * 114) / 1000).min(255) as u8;
        unsafe {
            *pixels.add(offset) = luma;
            *pixels.add(offset + 1) = luma;
            *pixels.add(offset + 2) = luma;
            *pixels.add(offset + 3) = (alpha as u32 * 3 / 4) as u8;
        }
    }
}

/// `[u16; N]` 定长缓冲里写宽字符串（截断保证结尾 0）。
fn copy_wide(target: &mut [u16], text: &str) {
    let limit = target.len().saturating_sub(1);
    let mut count = 0;
    for unit in text.encode_utf16() {
        if count >= limit {
            break;
        }
        if let Some(slot) = target.get_mut(count) {
            *slot = unit;
        }
        count += 1;
    }
    if let Some(slot) = target.get_mut(count) {
        *slot = 0;
    }
}

/// 菜单项 ID → 命令分发（供 `main_window` 的 WM_APP_TRAY / WM_COMMAND 共用）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayCommand {
    ShowWindow,
    ToggleService,
    CopyBaseUrl,
    ViewLogs,
    CopyLogs,
    About,
    Exit,
    None,
}

pub fn command_from_id(id: usize) -> TrayCommand {
    match id {
        IDM_SHOW => TrayCommand::ShowWindow,
        IDM_TOGGLE_SERVICE => TrayCommand::ToggleService,
        IDM_COPY_BASE => TrayCommand::CopyBaseUrl,
        IDM_VIEW_LOGS => TrayCommand::ViewLogs,
        IDM_COPY_LOGS => TrayCommand::CopyLogs,
        IDM_ABOUT => TrayCommand::About,
        IDM_EXIT => TrayCommand::Exit,
        _ => TrayCommand::None,
    }
}

/// 托盘菜单项 ID 区间判定（WM_COMMAND 里区分按钮 vs 托盘菜单）。
pub fn is_tray_menu_id(id: usize) -> bool {
    (2001..=2099).contains(&id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use azusa_local_proxy::config::Config;
    use azusa_local_proxy::service::Phase;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetMenuItemCount, GetMenuState, GetMenuStringW, MF_BYPOSITION,
    };

    #[test]
    fn icon_state_mapping_matches_spec() {
        let mut status = azusa_local_proxy::service::Status::for_config(Config::default(), None);
        assert_eq!(icon_state_for(&status), IconState::Stopped);

        status.phase = Phase::Starting;
        assert_eq!(icon_state_for(&status), IconState::Degraded);

        status.phase = Phase::Running;
        assert_eq!(icon_state_for(&status), IconState::Running);
        status.degraded = true;
        assert_eq!(icon_state_for(&status), IconState::Degraded);

        status.phase = Phase::Error;
        assert_eq!(icon_state_for(&status), IconState::Error);
    }

    #[test]
    fn tooltip_mentions_state_and_address() {
        let mut status = azusa_local_proxy::service::Status::for_config(Config::default(), None);
        status.phase = Phase::Running;
        status.listen_addr = Some("127.0.0.1:80".to_string());
        let tooltip = tooltip_for(&status);
        assert!(tooltip.contains("运行中"), "{tooltip}");
        assert!(tooltip.contains("127.0.0.1:80"), "{tooltip}");

        status.phase = Phase::Stopped;
        status.listen_addr = None;
        let tooltip = tooltip_for(&status);
        assert!(tooltip.contains("已停止"), "{tooltip}");
        assert!(tooltip.contains("未监听"), "{tooltip}");
    }

    #[test]
    fn wide_copy_truncates_safely() {
        // 定长缓冲 = 最多 N-1 个单元 + 结尾 0（`szTip[128]` 等 Win32 缓冲的口径）
        let mut buffer = [0u16; 8];
        copy_wide(&mut buffer, "中华人ABCDEFG");
        assert_eq!(buffer[7], 0, "必须以 0 结尾");
        let expected: Vec<u16> = "中华人ABCD".encode_utf16().collect();
        assert_eq!(&buffer[..7], &expected[..], "超长串必须按 N-1 截断");

        // 放得下：整串写入 + 结尾 0，其余保持 0
        let mut small = [0u16; 4];
        copy_wide(&mut small, "ab");
        assert_eq!(&small, &['a' as u16, 'b' as u16, 0, 0]);

        // 恰好占满：N-1 个字符正好放下，结尾 0 落在最后一个单元
        let mut exact = [0u16; 4];
        copy_wide(&mut exact, "abc");
        assert_eq!(&exact, &['a' as u16, 'b' as u16, 'c' as u16, 0]);
    }

    #[test]
    fn menu_ids_dispatch() {
        assert_eq!(command_from_id(IDM_EXIT), TrayCommand::Exit);
        assert_eq!(command_from_id(IDM_VIEW_LOGS), TrayCommand::ViewLogs);
        assert_eq!(command_from_id(IDM_COPY_LOGS), TrayCommand::CopyLogs);
        assert_eq!(command_from_id(1001), TrayCommand::None);
        assert!(is_tray_menu_id(IDM_SHOW));
        assert!(is_tray_menu_id(IDM_COPY_LOGS));
        assert!(!is_tray_menu_id(1001));
    }

    /// 菜单项文本（`MF_BYPOSITION` 读法）。
    fn menu_text(menu: HMENU, position: u32) -> String {
        let mut buffer = [0u16; 128];
        let length = unsafe { GetMenuStringW(menu, position, Some(&mut buffer), MF_BYPOSITION) };
        let length = usize::try_from(length.max(0))
            .unwrap_or(0)
            .min(buffer.len());
        String::from_utf16_lossy(&buffer[..length])
    }

    fn menu_flags(menu: HMENU, position: u32) -> u32 {
        unsafe { GetMenuState(menu, position, MF_BYPOSITION) }
    }

    /// **菜单内容门**（审查 P2-5）：菜单窗 `#32768` 的截图只渲染首项 ⇒ 项/序/态一律在这里断言。
    /// §7.2（K13 后）= 9 项含 **2 条分隔线**（0 显示 / 1 启停 / 2 复制 Base URL / 3 查看日志 /
    /// 4 复制日志到剪贴板 ─ 5 关于 ─ 6 退出）：原「开机自动运行」项随自启功能整体移除。
    #[test]
    fn menu_contains_expected_items() {
        let mut status = azusa_local_proxy::service::Status::for_config(Config::default(), None);
        status.phase = Phase::Running;
        status.base_url = Some("http://127.0.0.1:80/v1".to_string());

        let menu = build_menu(&status).expect("菜单必须可建");
        unsafe {
            assert_eq!(
                GetMenuItemCount(Some(menu)),
                9,
                "项数 = 7 项 + 2 条分隔线（§7.2；K13 删「开机自动运行」）"
            );
        }
        assert_eq!(menu_text(menu, 0), "显示主窗口 (&W)");
        assert_eq!(menu_text(menu, 1), "停止服务 (&S)", "运行中 ⇒ 显示「停止」");
        assert_eq!(menu_text(menu, 2), "复制 Base URL (&B)");
        assert_eq!(menu_text(menu, 3), "查看日志 (&L)");
        assert_eq!(menu_text(menu, 4), "复制日志到剪贴板 (&C)");
        assert_eq!(menu_text(menu, 5), "", "第 1 条分隔线");
        assert_eq!(menu_text(menu, 6), "关于 (&R)");
        assert_eq!(menu_text(menu, 7), "", "第 2 条分隔线");
        assert_eq!(menu_text(menu, 8), "退出 (&X)");
        assert_eq!(
            menu_flags(menu, 2) & MF_GRAYED.0,
            0,
            "有 Base URL ⇒ 复制项不置灰"
        );
        unsafe {
            let _ = DestroyMenu(menu);
        }

        // 停止态 + 未监听：文案二选一 / 复制置灰
        let mut stopped = azusa_local_proxy::service::Status::for_config(Config::default(), None);
        stopped.phase = Phase::Stopped;
        let menu = build_menu(&stopped).expect("菜单必须可建");
        assert_eq!(menu_text(menu, 1), "启动服务 (&S)", "已停止 ⇒ 显示「启动」");
        assert_ne!(
            menu_flags(menu, 2) & MF_GRAYED.0,
            0,
            "无 Base URL ⇒ 复制项必须置灰"
        );
        unsafe {
            let _ = DestroyMenu(menu);
        }
    }

    /// **四态图标门**（审查 P2-2 + P0-1 的运行时投影）：离屏渲染四态并落盘，不依赖可见桌面。
    /// 断言：8 个文件都写出且带像素；四态**两两可辨**（去饱和 / 黄点 / 红点）。
    #[test]
    fn icon_dump_renders_four_states_distinctly() {
        let dir = std::env::temp_dir().join(format!(
            "azusa-proxy-icons-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or(0)
        ));
        let written = dump_icon_states(&dir).expect("四态图标必须可渲染（底图资源 ID 必须正确）");
        assert_eq!(written, 8, "4 态 × 2 尺寸（真实 32 + 放大）");
        let read = |name: &str| std::fs::read(dir.join(name)).expect("图标文件必须存在");
        let running = read("icon-1-running-32px.bmp");
        let stopped = read("icon-2-stopped-32px.bmp");
        let degraded = read("icon-3-degraded-32px.bmp");
        let error = read("icon-4-error-32px.bmp");
        for (name, bytes) in [
            ("running", &running),
            ("stopped", &stopped),
            ("degraded", &degraded),
            ("error", &error),
        ] {
            assert!(bytes.len() > 54, "{name} 必须含像素数据（>54 B 头）");
        }
        assert_ne!(running, stopped, "停止态 = 去饱和 ⇒ 必须与原图不同");
        assert_ne!(running, degraded, "降级态 = 黄点 ⇒ 必须与原图不同");
        assert_ne!(running, error, "错误态 = 红点 ⇒ 必须与原图不同");
        assert_ne!(degraded, error, "黄点 vs 红点 ⇒ 必须不同");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
