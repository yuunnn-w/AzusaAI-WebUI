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
//!
//! **分通道口径（P7 §2.1.1，写死）**：颜色令牌一律 `0xAARRGGBB`。
//! - **GDI+ 路径**（`fill_round_rect` / `stroke_round_rect` / `fill_circle` / `line` / `polyline` /
//!   `fill_polygon` / `fill_soft_shadow`）**直传 ARGB 常量** ⇒ alpha 生效（两个带 alpha 的令牌 ——
//!   `COLOR_ACCENT_RING` 焦点环与 `COLOR_SHADOW` 柔和投影 —— 只允许走这条路径）；
//! - **仅 GDI 路径**（`CreateSolidBrush` / `SetTextColor` / `WM_CTLCOLOR*` 返回的画刷）**必须经
//!   [`gdi_color()`]** ⇒ **只允许不透明令牌**（α = `0xFF`）：`gdi_color` 产 `COLORREF = 0x00BBGGRR`，
//!   **alpha 被丢弃**（本模块单测自证）—— 把半透明令牌喂给它 = 纯黑实心块。

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HINSTANCE, RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    CreateFontW, CreateSolidBrush, DeleteObject, EnumFontFamiliesExW, GetDC, GetDeviceCaps,
    GetTextExtentPoint32W, ReleaseDC, SelectObject, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET,
    DEFAULT_GUI_FONT, DEFAULT_PITCH, HBRUSH, HDC, HFONT, HGDIOBJ, LOGFONTW, LOGPIXELSX,
    OUT_DEFAULT_PRECIS, TEXTMETRICW,
};
use windows::Win32::Graphics::GdiPlus::{
    CombineModeReplace, FillModeAlternate, GdipCreateFromHDC, GdipCreatePath, GdipCreatePen1,
    GdipCreateSolidFill, GdipDeleteBrush, GdipDeleteGraphics, GdipDeletePath, GdipDeletePen,
    GdipDrawLines, GdipDrawPath, GdipFillEllipse, GdipFillPath, GdipFillPolygon, GdipResetClip,
    GdipSetClipRect, GdipSetPenEndCap, GdipSetPenLineJoin, GdipSetPenStartCap,
    GdipSetSmoothingMode, GpGraphics, GpPath, GpSolidFill, LineCapRound, LineJoinRound, PointF,
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
///
/// P7 §2.1.1：既有常量名**一律不改名**（5 文件 67 处引用），只改值 + 新增缺失令牌；
/// **叠色（悬停/按下）一律不透明预混实色**（禁半透明令牌直传 —— 见模块注释的分通道口径）。
pub const COLOR_BG: u32 = 0xFFF5F5F7;
pub const COLOR_CARD: u32 = 0xFFFFFFFF;
pub const COLOR_CARD_BORDER: u32 = 0xFFDCDCE1;
pub const COLOR_SEPARATOR: u32 = 0xFFE5E5EA;
pub const COLOR_INK: u32 = 0xFF1D1D1F;
pub const COLOR_INK_SOFT: u32 = 0xFF6E6E73;
pub const COLOR_INK_FAINT: u32 = 0xFF8E8E93;
pub const COLOR_ACCENT_DEEP: u32 = 0xFFE8799A;
/// 状态色（§7.5：绿 / 黄 / 红 / 灰）。P7 起**只用于状态点**；错误**文字**改用 [`COLOR_DANGER_INK`]。
pub const COLOR_GREEN: u32 = 0xFF34C759;
pub const COLOR_YELLOW: u32 = 0xFFFF9500;
pub const COLOR_RED: u32 = 0xFFFF3B30;
pub const COLOR_GRAY: u32 = 0xFF8E8E93;
pub const COLOR_NOTICE_BG: u32 = 0xFFFFF4E5;
pub const COLOR_NOTICE_BORDER: u32 = 0xFFF0D9A8;
pub const COLOR_NOTICE_INK: u32 = 0xFF8A5A00;

/// 曲线底 / 输入井（P7 §2.1.1 新增；**I3 曲线面板消费** ⇒ 本批显式 allow）。
#[allow(dead_code)]
pub const COLOR_SURFACE_SUNKEN: u32 = 0xFFFAFAFC;
/// 聚焦态描边（**I3**：聚焦环描边；本批只落令牌）。
#[allow(dead_code)]
pub const COLOR_BORDER_STRONG: u32 = 0xFFC7C7CC;
/// 禁用文本。
pub const COLOR_INK_DISABLED: u32 = 0xFFC7C7CC;
/// 错误**文字**（白底可读；与仅用于状态点的 [`COLOR_RED`] 分工）。
pub const COLOR_DANGER_INK: u32 = 0xFFD70015;
/// 主按钮悬停 / 按下（**不透明**）。按下态本批已用；悬停态待 I3 的自绘按钮悬停（需子类化）。
pub const COLOR_ACCENT_PRESSED: u32 = 0xFFD9648A;
#[allow(dead_code)]
pub const COLOR_ACCENT_HOVER: u32 = 0xFFF08CA8;
/// 焦点环 —— 带 alpha 的两个令牌之一（另一个 = [`COLOR_SHADOW`]），只准走 GDI+（`stroke_round_rect` 直传 ARGB）。
pub const COLOR_ACCENT_RING: u32 = 0x59E8799A;
/// 叠色（不透明预混）：`base × (1 − a)`，悬停 `a = 6%`（悬停态待 I3 的自绘按钮悬停）。
#[allow(dead_code)]
pub const COLOR_HOVER_ON_CARD: u32 = 0xFFF0F0F0; // #FFFFFF × 0.94
#[allow(dead_code)]
pub const COLOR_HOVER_ON_CANVAS: u32 = 0xFFE6E6E8; // #F5F5F7 × 0.94
/// 叠色（不透明预混）：按下 `a = 12%`。
pub const COLOR_PRESS_ON_CARD: u32 = 0xFFE0E0E0; // #FFFFFF × 0.88
#[allow(dead_code)]
pub const COLOR_PRESS_ON_CANVAS: u32 = 0xFFD8D8D9; // #F5F5F7 × 0.88
/// 分段控件选中底（`blend(#FFFFFF, #E8799A, 10%)`，不透明）。
pub const COLOR_ACCENT_SOFT: u32 = 0xFFFDF2F5;
/// 卡片下缘 1 DIP 的假阴影（不透明；**I3 卡片**消费）。
#[allow(dead_code)]
pub const COLOR_HAIRLINE_SHADE: u32 = 0xFFECECEE;

/// **互斥选项组（P8-UI2「托盘 + 选中胶囊」）**：托盘底 = 浅灰（比窗口画布 `COLOR_BG` 深一档，
/// 比卡片白浅一档），把一整组选项括起来；选中项在托盘上浮起一枚白色胶囊（不透明 + 1 DIP 极淡描边）。
///
/// 为什么换掉旧的「每段一个圆角框 + 粉色描边选中」：粉色描边把**颜色**当选中信号 ⇒ 与主按钮
/// （樱花底）争夺注意力，且并排的框会读成「两个半截的盒子」（用户原话：不精致、间距太近）。
///
/// ⚠ **P8-UI2-tweak（用户报障：远看认不出"这是一组互斥选项"）**：旧值 `#F1F2F4` 与画布 `#F5F5F7`
/// 只差 3–4/通道（相对亮度差 0.027，WCAG 1.03）⇒ 托盘在真机上几乎看不见。现**加深一档**到
/// `#E7E8EC`：对画布的相对亮度差 0.107（WCAG 1.12，是旧值的 ~4 倍），与下方白胶囊的层次**反而更强**
/// （胶囊 vs 托盘 0.113 → 0.192）。本批**不动任何几何常量**（`SEG_*`，A-3b ⑤b/⑤c 判据依赖它们）。
pub const COLOR_SEG_TRAY: u32 = 0xFFE7E8EC;
/// 托盘 1 DIP 极淡描边（不描边 ⇒ 托盘边界靠圆角自身，四角的层级感会糊）：随托盘**同步加深**
/// （`#E8E9ED` → `#D8DAE0`；托盘变深后描边若不动就与托盘同色 = 描边消失）。
pub const COLOR_SEG_TRAY_EDGE: u32 = 0xFFD8DAE0;
/// 选中胶囊面（**白**，与卡片同色 ⇒ 与「输入井」同一套层级语言）。
pub const COLOR_SEG_CAPSULE: u32 = 0xFFFFFFFF;
/// 选中胶囊 1 DIP 极淡描边（把胶囊从托盘上「拎」出来）。**随托盘同步加深一档**
/// （`#E3E4E9` → `#DCDEE4`）：托盘变深后，若胶囊描边不动，它对托盘的对比会从 1.13 掉到 1.04
/// ⇒ 描边"消失"；现 1.10 ⇒ 与改前的"极淡但看得见"同一档（胶囊面 vs 托盘的身体色差另已由 0.113 涨到 0.192）。
pub const COLOR_SEG_CAPSULE_EDGE: u32 = 0xFFDCDEE4;
/// **柔和投影**（胶囊 / 下拉面板）：GDI+ 无 blur ⇒ 用 2 层递减 alpha 的外扩圆角矩形模拟
/// （§2.1.4 的等价手段）。**这是全仓第二个带 alpha 的令牌**（第一个 = [`COLOR_ACCENT_RING`]），
/// **只准走 GDI+ 路径**（`fill_round_rect` 直传 ARGB；喂给 `gdi_color()` 会变纯黑实心块）。
pub const COLOR_SHADOW: u32 = 0x0000_0000; // 基色 = 黑；alpha 逐层给（见 `fill_soft_shadow`）
/// 胶囊投影的层 alpha（由外到内：越外越淡）。
pub const SHADOW_ALPHAS: [u32; 2] = [0x0A, 0x16];

/// 圆角（DIP；消费点一律经 `Theme::px()`）。
pub const RADIUS_CARD: f32 = 10.0;
pub const RADIUS_FIELD: f32 = 6.0;
pub const RADIUS_BUTTON: f32 = 6.0;

/// **互斥选项组几何**（P8-UI2；DIP）—— 组 = 托盘 + N 个自绘段，段宽**按文字实测**定。
///
/// 托盘：`[段1][间距 12][段2]` 外包 `SEG_TRAY_PAD` 的横向余量（圆角 `SEG_TRAY_RADIUS`）；
/// 选中段在托盘上画一枚纵向内缩 `SEG_CAPSULE_INSET` 的胶囊（圆角 `SEG_CAPSULE_RADIUS`）。
pub const SEG_TRAY_PAD: f32 = 2.0;
pub const SEG_TRAY_RADIUS: f32 = 8.0;
/// 两个互斥选项之间的间距（用户诉求：**间距不要那么近**；旧实现是 1 DIP 的「并排盒子」）。
pub const SEG_ITEM_GAP: f32 = 12.0;
/// 段内文字左右留白（**段宽 = 文字实测（粗体）+ 2 × 本值** ⇒ 不再写死宽度、不撑满列槽）。
pub const SEG_ITEM_PAD_H: f32 = 5.0;
pub const SEG_CAPSULE_INSET: f32 = 2.0;
pub const SEG_CAPSULE_RADIUS: f32 = 6.0;

/// 间距阶梯（DIP）。
/// `SPACE_L` / `SPACE_XL` 的主面板消费点在 I3 ⇒ 本批显式 allow（避免与 `-D warnings` 冲突）。
pub const SPACE_XS: f32 = 4.0;
pub const SPACE_S: f32 = 8.0;
pub const SPACE_M: f32 = 12.0;
#[allow(dead_code)]
pub const SPACE_L: f32 = 16.0;
#[allow(dead_code)]
pub const SPACE_XL: f32 = 20.0;
#[allow(dead_code)]
pub const SPACE_XXL: f32 = 24.0;

/// 行高 / 行步 / 按钮高 / 页边距（设置窗行表的唯一真源）。
pub const ROW_H: f32 = 24.0;
pub const ROW_STEP: f32 = 30.0;
pub const BTN_H: f32 = 32.0;
/// 页边距（**I3 主面板**消费；设置窗用 `ui/settings_window.rs` 的 `PAD_LEFT/PAD_RIGHT`）。
#[allow(dead_code)]
pub const PAD_PAGE: f32 = 20.0;
/// 卡片内 padding（**I3 卡片**消费）。
#[allow(dead_code)]
pub const PAD_CARD: f32 = 12.0;

/// 主面板指标卡高（**I3 消费**；本批只落令牌）。
#[allow(dead_code)]
pub const CARD_H: f32 = 84.0;
/// 曲线面板最小 / 最大高（**I3 消费**；本批只落令牌）。
#[allow(dead_code)]
pub const CURVE_MIN_H: f32 = 56.0;
#[allow(dead_code)]
pub const CURVE_MAX_H: f32 = 140.0;

/// 字体尺寸（DIP；运行时 × scale 变成像素高度）。§7.5「字号不糊」= 每次 DPI 变化重建。
const SIZE_UI: f32 = 13.0;
const SIZE_SMALL: f32 = 11.0;
const SIZE_TITLE: f32 = 22.0;
const SIZE_NUMBER: f32 = 21.0;
const SIZE_MONO: f32 = 12.0;
/// 分组标题 / 曲线标题（P7 新增；**I3 曲线标题消费**）。
#[allow(dead_code)]
pub const SIZE_SECTION: f32 = 15.0;
/// 正文 callout（地址行 mono 亦 12.0；**I3 消费**）。
#[allow(dead_code)]
pub const SIZE_CALLOUT: f32 = 12.0;
/// 字重：正文 400 / 语义强调 600（P7：700 → 600，macOS 系统字体观感）。
const WEIGHT_REGULAR: i32 = 400;
const WEIGHT_SEMIBOLD: i32 = 600;

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
    /// 分组标题字（P7 第 7 个字体对象；`delete()`/`rebuild()` 数组必须同步 —— A-10）。
    #[allow(dead_code)]
    pub font_section: HFONT,
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
            font_section: HFONT::default(),
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
        self.font_ui = create_font(&face, SIZE_UI, scale, WEIGHT_REGULAR);
        self.font_ui_bold = create_font(&face, SIZE_UI, scale, WEIGHT_SEMIBOLD);
        self.font_small = create_font(&face, SIZE_SMALL, scale, WEIGHT_REGULAR);
        self.font_title = create_font(&face, SIZE_TITLE, scale, WEIGHT_SEMIBOLD);
        self.font_number = create_font(&face, SIZE_NUMBER, scale, WEIGHT_SEMIBOLD);
        self.font_mono = create_font("Consolas", SIZE_MONO, scale, WEIGHT_REGULAR);
        self.font_section = create_font(&face, SIZE_SECTION, scale, WEIGHT_SEMIBOLD);
        self.brush_bg = unsafe { CreateSolidBrush(gdi_color(COLOR_BG)) };
        self.brush_card = unsafe { CreateSolidBrush(gdi_color(COLOR_CARD)) };
    }

    /// 释放 GDI 对象（退出前调用；重复调用安全）。
    /// **字段清单必须与 `rebuild()` 的赋值一一对应**（漏一个 ⇒ GDI 基线法 A-10 必红）。
    pub fn delete(&mut self) {
        for font in [
            &mut self.font_ui,
            &mut self.font_ui_bold,
            &mut self.font_small,
            &mut self.font_title,
            &mut self.font_number,
            &mut self.font_mono,
            &mut self.font_section,
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

impl Drop for Theme {
    /// 析构即释放（与 [`Theme::delete`] 幂等；防止"建了就丢"的路径漏 GDI 对象 —— A-10 的覆盖面）。
    fn drop(&mut self) {
        self.delete();
    }
}

/// GDI 颜色（COLORREF = `0x00BBGGRR`；我们的常量是 `0xAARRGGBB` ⇒ 交换 R/B）。
pub fn gdi_color(argb: u32) -> windows::Win32::Foundation::COLORREF {
    windows::Win32::Foundation::COLORREF(
        ((argb & 0x0000_00FF) << 16) | (argb & 0x0000_FF00) | ((argb >> 16) & 0x0000_00FF),
    )
}

fn create_font(face: &str, size_dip: f32, scale: f32, weight: i32) -> HFONT {
    let height = -((size_dip * scale).round() as i32);
    let face_wide = wide(face);
    unsafe {
        CreateFontW(
            height,
            0,
            0,
            0,
            weight,
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

/// 文本像素宽（`GetTextExtentPoint32W`；调用方自备 DC）。
///
/// 这是**裁切防御的唯一量宽入口**：设置窗的单行读数行在创建期用它复核，
/// 超宽 ⇒ `logger.warn`（P3-1：不用 `debug_assert!` —— win7 release 档 `panic = "abort"`）。
pub fn measure_text(hdc: HDC, font: HFONT, text: &str) -> i32 {
    if text.is_empty() || font.is_invalid() {
        return 0;
    }
    let mut buffer = wide(text);
    buffer.pop(); // 去掉结尾 0：`GetTextExtentPoint32W` 吃 (指针, 字符数)
    let mut size = SIZE::default();
    unsafe {
        let old = SelectObject(hdc, HGDIOBJ(font.0));
        let ok = GetTextExtentPoint32W(hdc, &buffer, &mut size);
        SelectObject(hdc, old);
        if ok.as_bool() {
            size.cx
        } else {
            0
        }
    }
}

/// 放不下 ⇒ 从尾部逐字符裁并加 `…`（结果总宽 ≤ `max_width`）；放得下 ⇒ 原样返回。
/// 上限小到连 `…` 都放不下 ⇒ 返回空串（仍满足"总宽 ≤ 上限"）。
pub fn fit_text(hdc: HDC, font: HFONT, text: &str, max_width: i32) -> String {
    if max_width <= 0 {
        return String::new();
    }
    if measure_text(hdc, font, text) <= max_width {
        return text.to_string();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let mut candidate: String = chars.iter().collect();
        candidate.push('…');
        if measure_text(hdc, font, &candidate) <= max_width {
            return candidate;
        }
    }
    String::new()
}

/// 当前显示器 DPI（初始值；Per-Monitor 变化由 `WM_DPICHANGED` 通知）。
/// **不用 `GetDpiForWindow`**（Win10 1607+ 的 API ⇒ 老系统静态导入即加载失败；
/// 本模块只用 Win7 就有的 `GetDeviceCaps`）。
/// 指定宽度下**折行后的文本高度**（`DrawTextW` + `DT_CALCRECT`）—— "裁切防御"的**量高**入口。
///
/// 为什么需要它（真机缺陷 P1-①）：卡片标签在窄窗折 2 行，而"2 行是否放得下"若按**常量**猜
/// （如 `2 × 16 DIP`）就会漏算真实行高（`Segoe UI` 11 DIP 的行高 ≈ 13–15 DIP）⇒ 第二行被矩形底边裁掉。
/// 与 [`measure_text`] 一起构成"量宽 + 量高"双判据的全部工具面。
pub fn measure_wrapped_height(hdc: HDC, font: HFONT, text: &str, width: i32) -> i32 {
    if text.is_empty() || font.is_invalid() || width <= 0 {
        return 0;
    }
    let mut buffer = wide(text);
    let length = buffer.len().saturating_sub(1);
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: width,
        bottom: 0,
    };
    unsafe {
        use windows::Win32::Graphics::Gdi::{
            DrawTextW, DT_CALCRECT, DT_LEFT, DT_NOPREFIX, DT_WORDBREAK,
        };
        let previous = SelectObject(hdc, HGDIOBJ(font.0));
        let mut measured = 0;
        if let Some(slice) = buffer.get_mut(..length) {
            measured = DrawTextW(
                hdc,
                slice,
                &mut rect,
                DT_LEFT | DT_WORDBREAK | DT_CALCRECT | DT_NOPREFIX,
            );
        }
        SelectObject(hdc, previous);
        measured
    }
}

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

    /// **柔和投影**（P8-UI2）：GDI+ 无高斯模糊 ⇒ 用 [`SHADOW_ALPHAS`] 的 2 层**递减 alpha**外扩
    /// 圆角矩形模拟（§2.1.4 的等价手段；`COLOR_SHADOW` 是 alpha 令牌 ⇒ 只能走这条 GDI+ 路径）。
    ///
    /// 调用方的矩形应当是**投影的内边界**（胶囊 / 下拉面板）；本函数只向外扩。
    /// 超出控件矩形的部分由 DRAWITEM / 子窗的裁剪面自然吃掉（不会脏别的控件）。
    pub fn fill_soft_shadow(&self, x: f32, y: f32, w: f32, h: f32, radius: f32, scale: f32) {
        for (index, alpha) in SHADOW_ALPHAS.iter().enumerate() {
            let grow = (index as f32 + 1.0) * scale;
            let argb = (alpha << 24) | (COLOR_SHADOW & 0x00FF_FFFF);
            self.fill_round_rect(
                x - grow,
                y - grow,
                w + grow * 2.0,
                h + grow * 2.0,
                radius + grow,
                argb,
            );
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

    /// 虚点线（曲线网格；§2.5.3 要求 `DashStyleDot`）。
    ///
    /// 与 [`Gfx::line`] 的唯一差别是笔的虚线样式；颜色同样**直传 ARGB**（GDI+ 通道 ⇒ 只有它安全）。
    pub fn line_dashed(&self, x1: f32, y1: f32, x2: f32, y2: f32, color: u32, width: f32) {
        let mut pen = std::ptr::null_mut();
        unsafe {
            use windows::Win32::Graphics::GdiPlus::{
                DashStyleDot, GdipDrawLine, GdipSetPenDashStyle,
            };
            if GdipCreatePen1(color, width, UnitPixel, &mut pen).0 == 0 && !pen.is_null() {
                let _ = GdipSetPenDashStyle(pen, DashStyleDot);
                let _ = GdipDrawLine(self.graphics, pen, x1, y1, x2, y2);
                let _ = GdipDeletePen(pen);
            }
        }
    }

    /// 折线（勾选标记 / 曲线；`round_join` ⇒ 圆角连接 + 圆头端点）。
    ///
    /// **GDI+ 路径直传 ARGB**（§2.1.1 分通道口径）⇒ 只有这条路径能安全吃 [`COLOR_ACCENT_RING`]。
    pub fn polyline(&self, points: &[PointF], color: u32, width: f32, round_join: bool) {
        if points.len() < 2 {
            return;
        }
        let mut pen = std::ptr::null_mut();
        unsafe {
            if GdipCreatePen1(color, width, UnitPixel, &mut pen).0 == 0 && !pen.is_null() {
                if round_join {
                    let _ = GdipSetPenLineJoin(pen, LineJoinRound);
                    let _ = GdipSetPenStartCap(pen, LineCapRound);
                    let _ = GdipSetPenEndCap(pen, LineCapRound);
                }
                let _ = GdipDrawLines(self.graphics, pen, points.as_ptr(), points.len() as i32);
                let _ = GdipDeletePen(pen);
            }
        }
    }

    /// 填充多边形（面积图 / 自绘三角；**I3 曲线**消费）。
    #[allow(dead_code)]
    pub fn fill_polygon(&self, points: &[PointF], color: u32) {
        if points.len() < 3 {
            return;
        }
        let mut brush: *mut GpSolidFill = std::ptr::null_mut();
        unsafe {
            if GdipCreateSolidFill(color, &mut brush).0 == 0 && !brush.is_null() {
                let _ = GdipFillPolygon(
                    self.graphics,
                    brush as *mut _,
                    points.as_ptr(),
                    points.len() as i32,
                    FillModeAlternate,
                );
                let _ = GdipDeleteBrush(brush as *mut _);
            }
        }
    }

    /// 裁剪（矩形，替换模式）；与 [`Gfx::pop_clip`] 配对（曲线/井内绘制不越界；**I3 曲线**消费）。
    #[allow(dead_code)]
    pub fn push_clip(&self, rect: RECT) {
        unsafe {
            let _ = GdipSetClipRect(
                self.graphics,
                rect.left as f32,
                rect.top as f32,
                (rect.right - rect.left) as f32,
                (rect.bottom - rect.top) as f32,
                CombineModeReplace,
            );
        }
    }

    /// 恢复裁剪（`GdipResetClip`；**I3 曲线**消费）。
    #[allow(dead_code)]
    pub fn pop_clip(&self) {
        unsafe {
            let _ = GdipResetClip(self.graphics);
        }
    }
    /// **多段折线 + 孤立点**：一个 `GraphicsPath`、多个 figure ⇒ **单次 `GdipDrawPath`**。
    ///
    /// 为什么需要它（审查 **P2-①**）：`None` 断点要求"逐段画"（不能跨缺口连起来），若每段一次
    /// `GdipDrawLines`，调用数就随**数据形态**（段数）线性增长 ⇒ A-16① 的"≤16"变成数据依赖的界
    /// （段数 > 7 即顶破，而实现并无错）。收口 = 把多段塞进**同一个路径**（每段一个 figure），
    /// 孤立点画成 2 px 短刻度 ⇒ 每帧**恒定一次**绘制调用，与段数**无关**。
    pub fn polyline_runs(
        &self,
        points: &[PointF],
        runs: &[(usize, usize)],
        color: u32,
        width: f32,
        round_join: bool,
    ) {
        if runs.is_empty() || points.is_empty() {
            return;
        }
        let mut path: *mut GpPath = std::ptr::null_mut();
        let mut pen = std::ptr::null_mut();
        unsafe {
            use windows::Win32::Graphics::GdiPlus::{
                GdipAddPathLine2, GdipDrawPath, GdipStartPathFigure,
            };
            if GdipCreatePath(FillModeAlternate, &mut path).0 != 0 || path.is_null() {
                return;
            }
            if GdipCreatePen1(color, width, UnitPixel, &mut pen).0 != 0 || pen.is_null() {
                let _ = GdipDeletePath(path);
                return;
            }
            if round_join {
                let _ = GdipSetPenLineJoin(pen, LineJoinRound);
                let _ = GdipSetPenStartCap(pen, LineCapRound);
                let _ = GdipSetPenEndCap(pen, LineCapRound);
            }
            for (start, end) in runs.iter().copied() {
                if end > points.len() || end <= start {
                    continue;
                }
                let _ = GdipStartPathFigure(path);
                if end - start >= 2 {
                    // 连续 `PointF` 切片直接喂给 GDI+（`GdipAddPathLine2` 吃 (指针, 个数)）
                    let _ =
                        GdipAddPathLine2(path, points[start..end].as_ptr(), (end - start) as i32);
                } else {
                    // 孤立点（前后都是缺口）⇒ 2 px 短刻度：保住"有样本却看不见"的反面
                    let lone = points[start];
                    let tick = [
                        PointF {
                            X: lone.X - 1.0,
                            Y: lone.Y,
                        },
                        PointF {
                            X: lone.X + 1.0,
                            Y: lone.Y,
                        },
                    ];
                    let _ = GdipAddPathLine2(path, tick.as_ptr(), 2);
                }
            }
            let _ = GdipDrawPath(self.graphics, pen, path);
            let _ = GdipDeletePen(pen);
            let _ = GdipDeletePath(path);
        }
    }

    /// **分段面积**（曲线下的淡色填充；**P8-UI4** 的"连续波形"外观件之一）。
    ///
    /// 与 [`Gfx::polyline_runs`] 同一收口口径（A-16①）：一个 `GraphicsPath`、**每段一个闭合 figure**
    /// （折线段 + 基线回程）⇒ **单次 `GdipFillPath`**，调用数与段数**无关**。
    /// 单点段**不填**（零宽多边形）—— 与折线侧的 2 px 短刻度对齐，不制造"一格宽的色块"。
    pub fn fill_area_runs(
        &self,
        points: &[PointF],
        runs: &[(usize, usize)],
        baseline: f32,
        color: u32,
    ) {
        if runs.is_empty() || points.is_empty() {
            return;
        }
        let mut path: *mut GpPath = std::ptr::null_mut();
        let mut brush: *mut GpSolidFill = std::ptr::null_mut();
        unsafe {
            use windows::Win32::Graphics::GdiPlus::{
                GdipAddPathLine2, GdipClosePathFigure, GdipFillPath, GdipStartPathFigure,
            };
            if GdipCreatePath(FillModeAlternate, &mut path).0 != 0 || path.is_null() {
                return;
            }
            if GdipCreateSolidFill(color, &mut brush).0 != 0 || brush.is_null() {
                let _ = GdipDeletePath(path);
                return;
            }
            for (start, end) in runs.iter().copied() {
                if end > points.len() || end <= start || end - start < 2 {
                    continue;
                }
                let _ = GdipStartPathFigure(path);
                let _ = GdipAddPathLine2(path, points[start..end].as_ptr(), (end - start) as i32);
                let (first, last) = (points[start], points[end - 1]);
                let back = [
                    PointF {
                        X: last.X,
                        Y: baseline,
                    },
                    PointF {
                        X: first.X,
                        Y: baseline,
                    },
                ];
                let _ = GdipAddPathLine2(path, back.as_ptr(), 2);
                let _ = GdipClosePathFigure(path);
            }
            let _ = GdipFillPath(self.graphics, brush as *mut _, path);
            let _ = GdipDeleteBrush(brush as *mut _);
            let _ = GdipDeletePath(path);
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

/// 卡片数值的**自适应字号阶梯**（UI 方案 §2.4.3；DIP）：21 → 19 → 17 → 15（+ **13 兜底档**）。
///
/// ⚠ **实施期实测补第 5 档 13**（依据 = `a15_main_panel_strings_fit_their_rects` 的 GDI 读数，
/// 2026-09-20 现取）：`1,234,567` 在 **15 档下实测 69 px**，而 §2.4.3 的窄窗内宽只有 **63 DIP**
/// ⇒ 四档阶梯在窄窗下**必然出 `…`**，与"禁 `…`"这条硬要求（A-5）直接冲突。补 13 档后窄窗
/// 实测 ≈ 60 px ≤ 63 ✓，而"选定档 ≤ 15"的期望**依然成立**（13 ≤ 15）⇒ 这是**能力补强**，
/// 不改 21/19/17/15 四档的顺序与语义。
pub const NUMBER_LADDER_DIP: [f32; 5] = [21.0, 19.0, 17.0, 15.0, 13.0];

/// 卡片数值的 19/17/15/13 四档字体（21 档见 [`Theme::font_number`]）。
pub struct NumberFonts {
    f19: HFONT,
    f17: HFONT,
    f15: HFONT,
    f13: HFONT,
}

impl NumberFonts {
    pub fn new(face: &str, scale: f32) -> NumberFonts {
        NumberFonts {
            f19: create_font(face, NUMBER_LADDER_DIP[1], scale, WEIGHT_SEMIBOLD),
            f17: create_font(face, NUMBER_LADDER_DIP[2], scale, WEIGHT_SEMIBOLD),
            f15: create_font(face, NUMBER_LADDER_DIP[3], scale, WEIGHT_SEMIBOLD),
            f13: create_font(face, NUMBER_LADDER_DIP[4], scale, WEIGHT_SEMIBOLD),
        }
    }

    /// 释放四个字体对象（幂等；DPI 重建前与退出前调用 —— 漏释放即 GDI 句柄泄漏）。
    pub fn delete(&mut self) {
        for font in [&mut self.f19, &mut self.f17, &mut self.f15, &mut self.f13] {
            if !font.is_invalid() {
                unsafe {
                    let _ = DeleteObject(HGDIOBJ(font.0));
                }
                *font = HFONT::default();
            }
        }
    }

    /// 逐级降字号，返回**放得下**的最大档（`(字体, 档位下标 ∈ 0..=4)`）。
    ///
    /// 判据 A-5：`1,234,567` 在卡片内宽 73 DIP（默认）与 63 DIP（窄窗）下选定档位 ≤ 15
    /// ∧ 渲染文本**不含** `…`（现实现用 `DT_END_ELLIPSIS`，超宽会被压成 `1,23…` = 信息丢失）。
    /// 全档都放不下 ⇒ 返回最小档（13），由调用方按需再兜底。
    pub fn pick_index(
        &self,
        hdc: HDC,
        theme: &Theme,
        text: &str,
        max_width: i32,
    ) -> (HFONT, usize) {
        let candidates = [theme.font_number, self.f19, self.f17, self.f15, self.f13];
        for (index, font) in candidates.iter().enumerate() {
            if font.is_invalid() {
                continue;
            }
            if measure_text(hdc, *font, text) <= max_width {
                return (*font, index);
            }
        }
        (self.f13, NUMBER_LADDER_DIP.len() - 1)
    }
}

impl Drop for NumberFonts {
    fn drop(&mut self) {
        self.delete();
    }
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

    /// WCAG 相对亮度（sRGB 线性化；纯算术 ⇒ 与渲染无关）。
    fn relative_luminance(argb: u32) -> f64 {
        fn channel(value: u8) -> f64 {
            let value = f64::from(value) / 255.0;
            if value <= 0.039_28 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        }
        let red = ((argb >> 16) & 0xFF) as u8;
        let green = ((argb >> 8) & 0xFF) as u8;
        let blue = (argb & 0xFF) as u8;
        0.2126 * channel(red) + 0.7152 * channel(green) + 0.0722 * channel(blue)
    }

    fn contrast_ratio(foreground: u32, background: u32) -> f64 {
        let first = relative_luminance(foreground);
        let second = relative_luminance(background);
        let (high, low) = if first > second {
            (first, second)
        } else {
            (second, first)
        };
        (high + 0.05) / (low + 0.05)
    }

    /// **A-4（判据）**：正文对照度 ≥ 4.5:1（改色最容易改坏可读性 ⇒ 纯算术防线）。
    #[test]
    fn text_contrast_meets_wcag_aa() {
        for (name, foreground, background) in [
            ("INK/BG", COLOR_INK, COLOR_BG),
            ("INK_SOFT/CARD", COLOR_INK_SOFT, COLOR_CARD),
            ("DANGER_INK/CARD", COLOR_DANGER_INK, COLOR_CARD),
            ("NOTICE_INK/NOTICE_BG", COLOR_NOTICE_INK, COLOR_NOTICE_BG),
        ] {
            let ratio = contrast_ratio(foreground, background);
            assert!(
                ratio >= 4.5,
                "{name} 对比度 {ratio:.2}:1 < 4.5:1（前景 {foreground:#010X} / 背景 {background:#010X}）"
            );
        }
    }

    /// **A-4b（判据）**：叠色令牌必须是**不透明预混实色**且 ≠ 纯黑；alpha 位只允许存在于焦点环。
    ///
    /// 咬的是这一灾难：把半透明令牌喂给 [`gdi_color()`]（抹掉 alpha）⇒ 悬停/按下变纯黑实心块。
    /// A-4 抓不到它（A-4 只测不透明对），所以这里单独钉三条。
    #[test]
    fn blend_tokens_are_opaque_solids_never_black() {
        // ① "≠ 纯黑"（4 条叠色令牌）
        for (name, token) in [
            ("HOVER_ON_CARD", COLOR_HOVER_ON_CARD),
            ("HOVER_ON_CANVAS", COLOR_HOVER_ON_CANVAS),
            ("PRESS_ON_CARD", COLOR_PRESS_ON_CARD),
            ("PRESS_ON_CANVAS", COLOR_PRESS_ON_CANVAS),
        ] {
            assert_ne!(
                token, 0xFF00_0000,
                "COLOR_{name} 不得是纯黑（半透明令牌误喂 GDI 的后果）"
            );
            assert_eq!(token >> 24, 0xFF, "COLOR_{name} 必须不透明");
        }
        // ② 焦点环 = 全仓唯一 alpha 令牌，alpha 位 = 0x59 且未被改写
        assert_eq!(COLOR_ACCENT_RING >> 24, 0x59);
        assert_eq!(
            COLOR_ACCENT_RING & 0x00FF_FFFF,
            COLOR_ACCENT_DEEP & 0x00FF_FFFF
        );
        // ③ GDI 白名单：凡允许经 `gdi_color()` 的令牌，α 必须 = 0xFF
        for (name, token) in [
            ("BG", COLOR_BG),
            ("CARD", COLOR_CARD),
            ("CARD_BORDER", COLOR_CARD_BORDER),
            ("SEPARATOR", COLOR_SEPARATOR),
            ("INK", COLOR_INK),
            ("INK_SOFT", COLOR_INK_SOFT),
            ("INK_FAINT", COLOR_INK_FAINT),
            ("ACCENT_DEEP", COLOR_ACCENT_DEEP),
            ("ACCENT_HOVER", COLOR_ACCENT_HOVER),
            ("ACCENT_PRESSED", COLOR_ACCENT_PRESSED),
            ("ACCENT_SOFT", COLOR_ACCENT_SOFT),
            ("GREEN", COLOR_GREEN),
            ("YELLOW", COLOR_YELLOW),
            ("RED", COLOR_RED),
            ("GRAY", COLOR_GRAY),
            ("NOTICE_BG", COLOR_NOTICE_BG),
            ("NOTICE_BORDER", COLOR_NOTICE_BORDER),
            ("NOTICE_INK", COLOR_NOTICE_INK),
            ("SURFACE_SUNKEN", COLOR_SURFACE_SUNKEN),
            ("BORDER_STRONG", COLOR_BORDER_STRONG),
            ("INK_DISABLED", COLOR_INK_DISABLED),
            ("DANGER_INK", COLOR_DANGER_INK),
            ("HAIRLINE_SHADE", COLOR_HAIRLINE_SHADE),
            ("HOVER_ON_CARD", COLOR_HOVER_ON_CARD),
            ("HOVER_ON_CANVAS", COLOR_HOVER_ON_CANVAS),
            ("PRESS_ON_CARD", COLOR_PRESS_ON_CARD),
            ("PRESS_ON_CANVAS", COLOR_PRESS_ON_CANVAS),
            ("SEG_TRAY", COLOR_SEG_TRAY),
            ("SEG_TRAY_EDGE", COLOR_SEG_TRAY_EDGE),
            ("SEG_CAPSULE", COLOR_SEG_CAPSULE),
            ("SEG_CAPSULE_EDGE", COLOR_SEG_CAPSULE_EDGE),
        ] {
            assert_eq!(token >> 24, 0xFF, "COLOR_{name} 走 GDI 路径 ⇒ α 必须 0xFF");
        }
        // ②b 投影令牌（P8-UI2 新增的第二个 alpha 令牌）：基色透明 + 层 alpha ∈ (0, 0xFF)；
        //     它**只准走 GDI+**（`fill_soft_shadow`），断言层数 ≥ 2（"2–3 层模拟 blur"的机械面）。
        assert_eq!(
            COLOR_SHADOW >> 24,
            0x00,
            "COLOR_SHADOW 只带基色，alpha 由层表给"
        );
        assert!(SHADOW_ALPHAS.len() >= 2);
        assert!(SHADOW_ALPHAS
            .iter()
            .all(|alpha| *alpha > 0 && *alpha < 0xFF));
        // ④ 机理自证：`gdi_color()` 抹 alpha（不透明令牌安全、alpha 令牌必坏）
        assert_eq!(gdi_color(COLOR_ACCENT_RING).0 >> 24, 0);
        assert_ne!(
            gdi_color(COLOR_HOVER_ON_CARD).0,
            gdi_color(0xFF00_0000).0,
            "叠色经 GDI 路径不得退化成纯黑"
        );
        // ⑤ 预混公式复算（分通道；防"改值改错一格"）
        assert_eq!(COLOR_HOVER_ON_CARD, argb(0xFF, 0xF0, 0xF0, 0xF0));
        assert_eq!(COLOR_HOVER_ON_CANVAS, argb(0xFF, 0xE6, 0xE6, 0xE8));
        assert_eq!(COLOR_PRESS_ON_CARD, argb(0xFF, 0xE0, 0xE0, 0xE0));
        assert_eq!(COLOR_PRESS_ON_CANVAS, argb(0xFF, 0xD8, 0xD8, 0xD9));
        assert_eq!(COLOR_ACCENT_SOFT, argb(0xFF, 0xFD, 0xF2, 0xF5));
    }

    /// **A-4c（判据，P8-UI2-tweak）**：互斥选项组的**三层顺序**（画布 → 托盘 → 白胶囊）必须逐层变亮，
    /// 且"托盘 vs 画布"要**一眼可见**（用户报障：旧值 `#F1F2F4` 与画布只差 3–4/通道 ⇒ 远看糊成一片）。
    ///
    /// 阈值取"改前实测的 2 倍以上"这一档——纯算术防线，不依赖截图；**负例锚点**：把 `COLOR_SEG_TRAY`
    /// 改回 `#F1F2F4` ⇒ 第一条断言必 FAIL（实测 Δ 0.027 < 0.06）。
    #[test]
    fn seg_tray_layers_stay_ordered_and_visible() {
        let step = |first: u32, second: u32| {
            (relative_luminance(first) - relative_luminance(second)).abs()
        };
        let canvas_to_tray = step(COLOR_SEG_TRAY, COLOR_BG);
        let tray_to_edge = step(COLOR_SEG_TRAY_EDGE, COLOR_SEG_TRAY);
        let tray_to_capsule = step(COLOR_SEG_CAPSULE, COLOR_SEG_TRAY);
        let tray_to_capsule_edge = step(COLOR_SEG_CAPSULE_EDGE, COLOR_SEG_TRAY);
        println!(
            "托盘层次（相对亮度差）：画布→托盘 {canvas_to_tray:.4}（改前 #F1F2F4 = {:.4}）/ \
             托盘→描边 {tray_to_edge:.4} / 托盘→白胶囊 {tray_to_capsule:.4}（改前 = {:.4}）/ \
             托盘→胶囊描边 {tray_to_capsule_edge:.4}",
            step(0xFFF1F2F4, COLOR_BG),
            step(COLOR_SEG_CAPSULE, 0xFFF1F2F4)
        );
        assert!(
            canvas_to_tray >= 0.06,
            "托盘 vs 画布相对亮度差 {canvas_to_tray:.4} < 0.06 ⇒ 远看认不出托盘（回退到了旧值 #F1F2F4 的档位）"
        );
        assert!(
            relative_luminance(COLOR_SEG_TRAY) < relative_luminance(COLOR_BG),
            "托盘必须比画布深"
        );
        assert!(
            relative_luminance(COLOR_SEG_TRAY_EDGE) < relative_luminance(COLOR_SEG_TRAY),
            "托盘描边必须比托盘深（否则等于没有描边）"
        );
        assert!(
            relative_luminance(COLOR_SEG_CAPSULE) > relative_luminance(COLOR_SEG_TRAY),
            "白胶囊必须比托盘亮（选中项的层级）"
        );
        assert!(
            tray_to_capsule >= 0.15,
            "白胶囊 vs 托盘相对亮度差 {tray_to_capsule:.4} < 0.15 ⇒ 选中层次不够"
        );
        assert!(
            tray_to_capsule_edge >= 0.06,
            "胶囊描边 vs 托盘相对亮度差 {tray_to_capsule_edge:.4} < 0.06 ⇒ 描边会被托盘吃掉（托盘加深后必须同步加深描边）"
        );
    }

    /// **A-8（判据）**：`fit_text` 恰好放得下 ⇒ 原样；超 1 px ⇒ 末尾 `…` 且总宽 ≤ 上限。
    #[test]
    fn fit_text_never_exceeds_its_budget() {
        let theme = Theme::new(96);
        unsafe {
            let hdc = GetDC(None);
            let text = "共 2000 / 上限 2000 条（已挤出 0）";
            let exact = measure_text(hdc, theme.font_ui, text);
            assert!(exact > 0, "量宽必须可用（GetDC/GetTextExtentPoint32W）");
            assert_eq!(
                fit_text(hdc, theme.font_ui, text, exact),
                text,
                "恰好放得下"
            );
            let tight = fit_text(hdc, theme.font_ui, text, exact - 1);
            assert!(tight.ends_with('…'), "超宽必须带省略号：{tight}");
            assert!(
                measure_text(hdc, theme.font_ui, &tight) < exact,
                "截断结果必须 ≤ 上限"
            );
            assert!(
                text.starts_with(tight.trim_end_matches('…')),
                "截断必须保留原串前缀：{tight}"
            );
            // 上限小到放不下省略号 ⇒ 空串（仍满足 ≤ 上限）
            assert!(measure_text(hdc, theme.font_ui, &fit_text(hdc, theme.font_ui, text, 2)) <= 2);
            // 空串 / 零宽边界
            assert_eq!(fit_text(hdc, theme.font_ui, "", 10), "");
            assert_eq!(fit_text(hdc, theme.font_ui, text, 0), "");
            ReleaseDC(None, hdc);
        }
    }

    fn gdi_object_count() -> u32 {
        unsafe {
            windows::Win32::System::Threading::GetGuiResources(
                windows::Win32::System::Threading::GetCurrentProcess(),
                windows::Win32::System::Threading::GR_GDIOBJECTS,
            )
        }
    }

    /// **A-10（判据）**：GDI 基线法 —— 50 次 `rebuild` + `delete` 后 GDI 对象数回到基线；
    /// `delete()` 后全部字段 `is_invalid()`。**负例（[E13] 强制）**：临时删掉 `delete()`
    /// 数组里任意一行字体释放 ⇒ 本断言必须 FAIL（本批已注入验证一次，见 done.md）。
    ///
    /// 计数是**进程级**的且测试并行 ⇒ 允许并行同伴的瞬时占用：只在"读数仍高于基线"时重测
    /// （真泄漏是多达数百个对象的单调上升，重测不会自己消失）。
    #[test]
    fn gdi_objects_return_to_baseline_after_rebuild_delete_cycles() {
        let mut theme = Theme::new(96);
        let baseline = gdi_object_count();
        for index in 0..50 {
            theme.rebuild(120 + index % 3);
        }
        theme.delete();
        let mut after = gdi_object_count();
        for _ in 0..20 {
            if after <= baseline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
            after = gdi_object_count();
        }
        assert!(
            after <= baseline,
            "50 次 rebuild/delete 后 GDI 对象 {after} > 基线 {baseline}（漏释放 ⇒ 长跑爆句柄）"
        );
        for (name, font) in [
            ("font_ui", theme.font_ui),
            ("font_ui_bold", theme.font_ui_bold),
            ("font_small", theme.font_small),
            ("font_title", theme.font_title),
            ("font_number", theme.font_number),
            ("font_mono", theme.font_mono),
            ("font_section", theme.font_section),
        ] {
            assert!(font.is_invalid(), "delete() 后 {name} 必须已释放");
        }
        assert!(theme.brush_bg.is_invalid());
        assert!(theme.brush_card.is_invalid());
    }

    /// 【A-5】卡片数值自适应字号（§2.4.3）：`1,234,567` 在卡片内宽 **73 DIP（默认）** 与
    /// **63 DIP（窄窗 460 → 420）** 下都必须**放得下**（放得下 ⇒ 不会出 `…`）。
    ///
    /// 内宽 = `card.w − 24`（`PAD_CARD` 两侧各 12），`card.w = (content_w − 3×10) / 4`：
    /// 客户区 460 ⇒ `content_w 420` ⇒ `card.w 97` ⇒ 内宽 **73**；客户区 420 ⇒ `content_w 380`
    /// ⇒ `card.w 87` ⇒ 内宽 **63**（与 UI 方案 §2.4.3 的两个数字逐一对应）。
    #[test]
    fn number_ladder_keeps_big_values_ellipsis_free() {
        let theme = Theme::new(96);
        let fonts = NumberFonts::new(&theme.face, theme.scale);
        let hdc = unsafe { GetDC(None) };
        let mut readings = Vec::new();
        for inner_dip in [73, 63] {
            let (font, index) = fonts.pick_index(hdc, &theme, "1,234,567", inner_dip);
            let width = measure_text(hdc, font, "1,234,567");
            readings.push((inner_dip, NUMBER_LADDER_DIP[index], width));
            assert!(
                width <= inner_dip,
                "选定档放不下：{width} px > {inner_dip} DIP（会出 `…`）"
            );
            assert!(
                NUMBER_LADDER_DIP[index] <= 15.0,
                "内宽 {inner_dip} DIP 下必须降到 ≤ 15 档（实得 {})",
                NUMBER_LADDER_DIP[index]
            );
        }
        // 21 档放不下 9 字符（否则这条阶梯就是死代码）
        let (_, wide_index) = fonts.pick_index(hdc, &theme, "1,234,567", 200);
        assert_eq!(wide_index, 0, "内宽充足时应当用 21 档");
        unsafe { ReleaseDC(None, hdc) };
        println!("卡片数值字号自适应读数（内宽 DIP, 选定字号 DIP, 文本宽 px）：{readings:?}");
    }
}
