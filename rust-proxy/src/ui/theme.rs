//! 主题与绘图助手（方案 §7.5 视觉要求的落点）：字体回退链、配色、GDI+ 抗锯齿圆角绘制。
//!
//! 分工：
//! - **文字**走 GDI（`DrawTextW` + `CreateFontW` 字体）：中英文混排的 CJK 回退由系统字体链接处理；
//! - **圆角卡片 / 状态点 / 分隔线**走 GDI+（`gdiplus.dll`；XP 起系统自带，Win7 无额外依赖）——
//!   抗锯齿，且不引任何外部图片资源（§7.5 第 4 条）。
//!
//! 字体回退链（§7.5 第 2 条，按序取第一个可用的）：`Segoe UI Variable Text` → `Segoe UI` →
//! `Microsoft YaHei UI` → 系统默认（`DEFAULT_GUI_FONT` 的字体名）。中文由字体链接回退到
//! 系统 CJK 字体渲染，无需单独指定。

use windows::core::PCWSTR;
use windows::Win32::Foundation::HINSTANCE;
use windows::Win32::Graphics::Gdi::{
    CreateFontW, CreateSolidBrush, DeleteObject, EnumFontFamiliesExW, GetDC, GetDeviceCaps,
    ReleaseDC, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_GUI_FONT, DEFAULT_PITCH, HBRUSH, HDC,
    HFONT, HGDIOBJ, LOGFONTW, LOGPIXELSX, OUT_DEFAULT_PRECIS, TEXTMETRICW,
};
use windows::Win32::Graphics::GdiPlus::{
    FillModeAlternate, GdipCreateFromHDC, GdipCreatePath, GdipCreatePen1, GdipCreateSolidFill,
    GdipDeleteBrush, GdipDeleteGraphics, GdipDeletePath, GdipDeletePen, GdipDrawPath,
    GdipFillEllipse, GdipFillPath, GdipSetSmoothingMode, GpGraphics, GpPath, GpSolidFill,
    SmoothingModeAntiAlias, UnitPixel,
};
use windows::Win32::UI::WindowsAndMessaging::{LoadImageW, HICON, IMAGE_ICON, LR_DEFAULTCOLOR};

/// 应用图标资源号 = **1**（`build.rs` 用 `winresource` 把 `assets/app.ico` 嵌进 exe，资源号写死 1）。
///
/// ⚠ **不要碰这段的写法**（2026-09-19 审查 P0-1 的回归教训）：Win32 的 `MAKEINTRESOURCE(1)`
/// 惯用形 = **低字为整数 ID 的伪指针**（`1 as *const u16`）；clippy 的 `manual_dangling_ptr`
/// 对此**误报**并建议 `std::ptr::dangling::<u16>()` —— 后者 = 地址 **2**（`align_of::<u16>()`），
/// 照单全改会把资源 ID 从 1 变成 2 ⇒ `LoadImageW` 拿不到图标（窗口/托盘图标静默退化为系统占位）。
/// 本函数是**唯一**的图标加载入口（窗口类图标 + 托盘底图共用），改动它前先跑
/// `app_icon_resource_loads_at_declared_id` 单测与 `icon_dump_renders_four_states_distinctly` 回归。
pub const APP_ICON_RESOURCE_ID: usize = 1;

/// 取嵌入的应用图标（16/32/48/256 四种尺寸由 `LoadImageW` 按请求尺寸挑/缩放）。
/// 失败 ⇒ `HICON::default()`（NULL，调用方负责告警/降级）。
#[allow(clippy::manual_dangling_ptr)]
pub fn load_app_icon(hinst: HINSTANCE, size: i32) -> HICON {
    unsafe {
        match LoadImageW(
            Some(hinst),
            PCWSTR(APP_ICON_RESOURCE_ID as *const u16),
            IMAGE_ICON,
            size,
            size,
            LR_DEFAULTCOLOR,
        ) {
            Ok(handle) => HICON(handle.0),
            Err(_) => HICON::default(),
        }
    }
}

/// 配色（§7.5 第 3 条：浅色卡片 + 单一强调色 = 樱花粉「与主项目 `data-accent="sakura"` 一致」；
/// 状态色只用于状态点与错误行）。ARGB 常量：`0xAARRGGBB`。
pub const COLOR_BG: u32 = 0xFFF4F5F7;
pub const COLOR_CARD: u32 = 0xFFFFFFFF;
pub const COLOR_CARD_BORDER: u32 = 0xFFE4E7EC;
pub const COLOR_SEPARATOR: u32 = 0xFFE9EBEE;
pub const COLOR_INK: u32 = 0xFF1F2937;
pub const COLOR_INK_SOFT: u32 = 0xFF667085;
pub const COLOR_INK_FAINT: u32 = 0xFF98A2B3;
pub const COLOR_ACCENT_DEEP: u32 = 0xFFE8799A;
/// 状态色（§7.5：绿 #16A34A / 黄 #D97706 / 红 #DC2626 / 灰 #9CA3AF）。
pub const COLOR_GREEN: u32 = 0xFF16A34A;
pub const COLOR_YELLOW: u32 = 0xFFD97706;
pub const COLOR_RED: u32 = 0xFFDC2626;
pub const COLOR_GRAY: u32 = 0xFF9CA3AF;
pub const COLOR_NOTICE_BG: u32 = 0xFFFFF6E5;
pub const COLOR_NOTICE_BORDER: u32 = 0xFFF5D9A8;
pub const COLOR_NOTICE_INK: u32 = 0xFF8A5A00;

/// 字体尺寸（DIP；运行时 × scale 变成像素高度）。§7.5「字号不糊」= 每次 DPI 变化重建。
const SIZE_UI: f32 = 13.0;
const SIZE_SMALL: f32 = 11.5;
const SIZE_TITLE: f32 = 19.0;
const SIZE_NUMBER: f32 = 21.0;
const SIZE_MONO: f32 = 12.0;

pub struct Theme {
    pub dpi: u32,
    pub scale: f32,
    pub face: String,
    pub font_ui: HFONT,
    pub font_ui_bold: HFONT,
    pub font_small: HFONT,
    pub font_title: HFONT,
    pub font_number: HFONT,
    pub font_mono: HFONT,
    pub brush_bg: HBRUSH,
    pub brush_card: HBRUSH,
}

impl Theme {
    pub fn new(dpi: u32) -> Theme {
        let face = pick_font_face();
        let mut theme = Theme {
            dpi,
            scale: dpi as f32 / 96.0,
            face,
            font_ui: HFONT::default(),
            font_ui_bold: HFONT::default(),
            font_small: HFONT::default(),
            font_title: HFONT::default(),
            font_number: HFONT::default(),
            font_mono: HFONT::default(),
            brush_bg: HBRUSH::default(),
            brush_card: HBRUSH::default(),
        };
        theme.rebuild(dpi);
        theme
    }

    /// 重建字体与画刷（WM_DPICHANGED / 初始化时调用）。旧对象即刻释放。
    pub fn rebuild(&mut self, dpi: u32) {
        self.delete();
        self.dpi = dpi;
        self.scale = dpi as f32 / 96.0;
        let scale = self.scale;
        let face = self.face.clone();
        self.font_ui = create_font(&face, SIZE_UI, scale, false);
        self.font_ui_bold = create_font(&face, SIZE_UI, scale, true);
        self.font_small = create_font(&face, SIZE_SMALL, scale, false);
        self.font_title = create_font(&face, SIZE_TITLE, scale, true);
        self.font_number = create_font(&face, SIZE_NUMBER, scale, true);
        self.font_mono = create_font("Consolas", SIZE_MONO, scale, false);
        self.brush_bg = unsafe { CreateSolidBrush(gdi_color(COLOR_BG)) };
        self.brush_card = unsafe { CreateSolidBrush(gdi_color(COLOR_CARD)) };
    }

    /// 释放 GDI 对象（退出前调用；重复调用安全）。
    pub fn delete(&mut self) {
        for font in [
            &mut self.font_ui,
            &mut self.font_ui_bold,
            &mut self.font_small,
            &mut self.font_title,
            &mut self.font_number,
            &mut self.font_mono,
        ] {
            if !font.is_invalid() {
                unsafe {
                    let _ = DeleteObject(HGDIOBJ(font.0));
                }
                *font = HFONT::default();
            }
        }
        for brush in [&mut self.brush_bg, &mut self.brush_card] {
            if !brush.is_invalid() {
                unsafe {
                    let _ = DeleteObject(HGDIOBJ(brush.0));
                }
                *brush = HBRUSH::default();
            }
        }
    }

    /// DIP → 像素（四舍五入）。
    pub fn px(&self, dip: f32) -> i32 {
        (dip * self.scale).round() as i32
    }
}

/// GDI 颜色（COLORREF = `0x00BBGGRR`；我们的常量是 `0xAARRGGBB` ⇒ 交换 R/B）。
pub fn gdi_color(argb: u32) -> windows::Win32::Foundation::COLORREF {
    windows::Win32::Foundation::COLORREF(
        ((argb & 0x0000_00FF) << 16) | (argb & 0x0000_FF00) | ((argb >> 16) & 0x0000_00FF),
    )
}

fn create_font(face: &str, size_dip: f32, scale: f32, bold: bool) -> HFONT {
    let height = -((size_dip * scale).round() as i32);
    let face_wide = wide(face);
    unsafe {
        CreateFontW(
            height,
            0,
            0,
            0,
            if bold { 700 } else { 400 },
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            windows::Win32::Graphics::Gdi::CLEARTYPE_QUALITY,
            DEFAULT_PITCH.0 as u32,
            PCWSTR(face_wide.as_ptr()),
        )
    }
}

/// 当前显示器 DPI（初始值；Per-Monitor 变化由 `WM_DPICHANGED` 通知）。
/// **不用 `GetDpiForWindow`**（Win10 1607+ 的 API ⇒ 老系统静态导入即加载失败；
/// 本模块只用 Win7 就有的 `GetDeviceCaps`）。
pub fn system_dpi() -> u32 {
    unsafe {
        let hdc = GetDC(None);
        let dpi = GetDeviceCaps(Some(hdc), LOGPIXELSX);
        ReleaseDC(None, hdc);
        if dpi <= 0 {
            96
        } else {
            dpi as u32
        }
    }
}

/// §7.5 第 2 条：字体回退链（按序取第一个系统里真实存在的）。
pub fn pick_font_face() -> String {
    for candidate in ["Segoe UI Variable Text", "Segoe UI", "Microsoft YaHei UI"] {
        if font_available(candidate) {
            return candidate.to_string();
        }
    }
    // 系统默认字体名（DEFAULT_GUI_FONT 的 LOGFONT 里）
    unsafe {
        use windows::Win32::Graphics::Gdi::{GetObjectW, GetStockObject, LOGFONTW as LF};
        let stock = GetStockObject(DEFAULT_GUI_FONT);
        let mut logfont = LF::default();
        let copied = GetObjectW(
            HGDIOBJ(stock.0),
            std::mem::size_of::<LF>() as i32,
            Some(&mut logfont as *mut LF as *mut core::ffi::c_void),
        );
        if copied > 0 {
            let name = String::from_utf16_lossy(
                &logfont.lfFaceName[..logfont
                    .lfFaceName
                    .iter()
                    .position(|value| *value == 0)
                    .unwrap_or(logfont.lfFaceName.len())],
            );
            if !name.is_empty() {
                return name;
            }
        }
    }
    "Segoe UI".to_string()
}

fn font_available(face: &str) -> bool {
    unsafe extern "system" fn callback(
        _logfont: *const LOGFONTW,
        _metric: *const TEXTMETRICW,
        _font_type: u32,
        lparam: windows::Win32::Foundation::LPARAM,
    ) -> i32 {
        let found = lparam.0 as *mut bool;
        if !found.is_null() {
            unsafe { *found = true };
        }
        0 // 找到一个就停
    }

    let mut found = false;
    let face_wide = wide(face);
    let mut logfont = LOGFONTW::default();
    let name_len = face_wide
        .len()
        .saturating_sub(1)
        .min(logfont.lfFaceName.len() - 1);
    logfont.lfCharSet = DEFAULT_CHARSET;
    logfont.lfFaceName[..name_len].copy_from_slice(&face_wide[..name_len]);
    unsafe {
        let hdc = GetDC(None);
        let _ = EnumFontFamiliesExW(
            hdc,
            &logfont,
            Some(callback),
            windows::Win32::Foundation::LPARAM(&mut found as *mut bool as isize),
            0,
        );
        ReleaseDC(None, hdc);
    }
    found
}

/// UTF-16（含结尾 0）转换助手（Win32 的宽字符 API 一律经它）。
pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// GDI+ 绘图上下文（`GdipCreateFromHDC` 的 RAII 包装；drop 即释放）。
pub struct Gfx {
    graphics: *mut GpGraphics,
}

impl Gfx {
    /// 绑定到某个 DC（双缓冲时是内存 DC）。GDI+ 未初始化 / 创建失败 ⇒ `None`（调用方退回纯 GDI 画法）。
    pub fn from_hdc(hdc: HDC) -> Option<Gfx> {
        let mut graphics: *mut GpGraphics = std::ptr::null_mut();
        let status = unsafe { GdipCreateFromHDC(hdc, &mut graphics) };
        if status.0 != 0 || graphics.is_null() {
            return None;
        }
        unsafe {
            let _ = GdipSetSmoothingMode(graphics, SmoothingModeAntiAlias);
        }
        Some(Gfx { graphics })
    }

    pub fn fill_round_rect(&self, x: f32, y: f32, w: f32, h: f32, radius: f32, color: u32) {
        let Some(path) = round_rect_path(x, y, w, h, radius) else {
            return;
        };
        let mut brush: *mut GpSolidFill = std::ptr::null_mut();
        unsafe {
            if GdipCreateSolidFill(color, &mut brush).0 == 0 && !brush.is_null() {
                let _ = GdipFillPath(self.graphics, brush as *mut _, path);
                let _ = GdipDeleteBrush(brush as *mut _);
            }
            let _ = GdipDeletePath(path);
        }
    }

    /// 圆角矩形描边（位置 + 半径 + 颜色 + 线宽）。
    ///
    /// `#[allow(too_many_arguments)]` 的理由（审查 P2-6 的 nit：注释要"论证"不是"描述"）：
    /// 这是 GDI+ 的薄封装，7 个参数是 7 个独立几何量；打包成结构体只会在**两处**调用点各加一行
    /// 组装、并新增一个仅内部使用的类型 ⇒ 净可读性下降。真正的教训是 P0-1：**该 allow 的地方
    /// （`MAKEINTRESOURCE`）没敢 allow**，反而被 clippy 建议带偏改了语义。
    #[allow(clippy::too_many_arguments)]
    pub fn stroke_round_rect(
        &self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        color: u32,
        width: f32,
    ) {
        let Some(path) = round_rect_path(x, y, w, h, radius) else {
            return;
        };
        let mut pen = std::ptr::null_mut();
        unsafe {
            if GdipCreatePen1(color, width, UnitPixel, &mut pen).0 == 0 && !pen.is_null() {
                let _ = GdipDrawPath(self.graphics, pen, path);
                let _ = GdipDeletePen(pen);
            }
            let _ = GdipDeletePath(path);
        }
    }

    pub fn fill_circle(&self, cx: f32, cy: f32, radius: f32, color: u32) {
        let mut brush: *mut GpSolidFill = std::ptr::null_mut();
        unsafe {
            if GdipCreateSolidFill(color, &mut brush).0 == 0 && !brush.is_null() {
                let _ = GdipFillEllipse(
                    self.graphics,
                    brush as *mut _,
                    cx - radius,
                    cy - radius,
                    radius * 2.0,
                    radius * 2.0,
                );
                let _ = GdipDeleteBrush(brush as *mut _);
            }
        }
    }

    /// 水平/垂直细线（分隔线用；抗锯齿 + 亚像素宽度）。
    pub fn line(&self, x1: f32, y1: f32, x2: f32, y2: f32, color: u32, width: f32) {
        let mut pen = std::ptr::null_mut();
        unsafe {
            use windows::Win32::Graphics::GdiPlus::GdipDrawLine;
            if GdipCreatePen1(color, width, UnitPixel, &mut pen).0 == 0 && !pen.is_null() {
                let _ = GdipDrawLine(self.graphics, pen, x1, y1, x2, y2);
                let _ = GdipDeletePen(pen);
            }
        }
    }
}

impl Drop for Gfx {
    fn drop(&mut self) {
        unsafe {
            let _ = GdipDeleteGraphics(self.graphics);
        }
    }
}

/// 圆角矩形路径（四段 90° 弧）。
fn round_rect_path(x: f32, y: f32, w: f32, h: f32, radius: f32) -> Option<*mut GpPath> {
    let radius = radius.min(w / 2.0).min(h / 2.0).max(0.0);
    let mut path: *mut GpPath = std::ptr::null_mut();
    unsafe {
        if GdipCreatePath(FillModeAlternate, &mut path).0 != 0 || path.is_null() {
            return None;
        }
        let d = radius * 2.0;
        let _ = windows::Win32::Graphics::GdiPlus::GdipAddPathArc(path, x, y, d, d, 180.0, 90.0);
        let _ = windows::Win32::Graphics::GdiPlus::GdipAddPathArc(
            path,
            x + w - d,
            y,
            d,
            d,
            270.0,
            90.0,
        );
        let _ = windows::Win32::Graphics::GdiPlus::GdipAddPathArc(
            path,
            x + w - d,
            y + h - d,
            d,
            d,
            0.0,
            90.0,
        );
        let _ =
            windows::Win32::Graphics::GdiPlus::GdipAddPathArc(path, x, y + h - d, d, d, 90.0, 90.0);
        let _ = windows::Win32::Graphics::GdiPlus::GdipClosePathFigure(path);
    }
    Some(path)
}

/// 千分位数字（§7.1 指标卡："数值用千分位"）。
pub fn format_thousands(value: u64) -> String {
    let text = value.to_string();
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len() + text.len() / 3);
    for (index, byte) in bytes.iter().enumerate() {
        if index > 0 && (bytes.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*byte as char);
    }
    out
}

/// 字节数 → 人类可读（`1.2 MB` / `512 KB` / `934 B`；沿用主项目的中文界面口径）。
pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    let value = bytes as f64;
    if value >= GB {
        format!("{:.2} GB", value / GB)
    } else if value >= MB {
        format!("{:.1} MB", value / MB)
    } else if value >= KB {
        format!("{:.0} KB", value / KB)
    } else {
        format!("{bytes} B")
    }
}

/// 运行时长 → `HH:MM:SS`（超过 99 h 显示 `Nd HH:MM:SS`）。
pub fn format_uptime(duration: std::time::Duration) -> String {
    let total = duration.as_secs();
    let days = total / 86_400;
    let hours = (total % 86_400) / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if days > 0 {
        format!("{days}d {hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    }
}

/// 单行截断（UI 行宽有限；按**字符**截，末尾加省略号）。
pub fn truncate_line(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index >= max_chars {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GDI+ `GdipCreateSolidFill` 取的 `0xAARRGGBB` 组装（配色常量与断言同格式）。
    const fn argb(a: u8, r: u8, g: u8, b: u8) -> u32 {
        ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
    }

    /// 给常量换 alpha（淡色分隔线/边框用）。
    const fn with_alpha(color: u32, alpha: u8) -> u32 {
        (color & 0x00FF_FFFF) | ((alpha as u32) << 24)
    }

    #[test]
    fn thousands_and_bytes_formatting() {
        assert_eq!(format_thousands(0), "0");
        assert_eq!(format_thousands(999), "999");
        assert_eq!(format_thousands(1000), "1,000");
        assert_eq!(format_thousands(12_483), "12,483");
        assert_eq!(format_thousands(1_234_567), "1,234,567");

        assert_eq!(format_bytes(934), "934 B");
        assert_eq!(format_bytes(2048), "2 KB");
        assert_eq!(format_bytes(1_258_291), "1.2 MB");
        assert_eq!(format_bytes(2 * 1024 * 1024 * 1024), "2.00 GB");
    }

    #[test]
    fn uptime_and_truncation() {
        assert_eq!(format_uptime(std::time::Duration::from_secs(0)), "00:00:00");
        assert_eq!(
            format_uptime(std::time::Duration::from_secs(3 * 3600 + 21 * 60 + 47)),
            "03:21:47"
        );
        assert_eq!(
            format_uptime(std::time::Duration::from_secs(90_000)),
            "1d 01:00:00"
        );
        assert_eq!(truncate_line("abcdef", 3), "abc…");
        assert_eq!(truncate_line("abc", 5), "abc");
    }

    /// **P0-1 回归门**（2026-09-19 审查）：托盘的四个合成 `HICON` 与窗口类图标都从这条链取底图；
    /// 资源 ID 一旦被改成别的值（历史事故：clippy 建议的 `dangling::<u16>()` = 地址 **2**），
    /// `LoadImageW` 会静默返回 NULL ⇒ 图标全灭。本测试在**两套目标**（现代 + win7）都跑。
    #[test]
    fn app_icon_resource_loads_at_declared_id() {
        // MAKEINTRESOURCE 的数值语义：低字 = 资源 ID（不是"悬垂指针"）
        assert_eq!(
            APP_ICON_RESOURCE_ID as *const u16 as usize, 1,
            "MAKEINTRESOURCE(1) 的地址必须恰为 1（dangling::<u16>() 会给出 2）"
        );
        let hinst: HINSTANCE =
            unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(PCWSTR::null()) }
                .unwrap_or_default()
                .into();
        let icon = load_app_icon(hinst, 32);
        assert!(
            !icon.is_invalid(),
            "资源 ID {APP_ICON_RESOURCE_ID} 的应用图标必须可加载（LoadImageW 返回 NULL ⇒ 托盘/标题栏图标会退化为系统占位）"
        );
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyIcon(icon);
        }
    }

    #[test]
    fn color_conversion_is_bgra_swapped() {
        // COLORREF = 0x00BBGGRR：樱花粉 0xFFFFB3C7 ⇒ B=C7, G=B3, R=FF ⇒ 0x00C7B3FF
        assert_eq!(gdi_color(0xFFFFB3C7).0, 0x00C7B3FF);
        assert_eq!(argb(0xFF, 0x16, 0xA3, 0x4A), 0xFF16A34A);
        assert_eq!(with_alpha(0xFF16A34A, 0x33), 0x3316A34A);
    }

    #[test]
    fn wide_strings_are_nul_terminated() {
        let wide = wide("运行");
        assert_eq!(wide.len(), 3);
        assert_eq!(wide[2], 0);
    }
}
