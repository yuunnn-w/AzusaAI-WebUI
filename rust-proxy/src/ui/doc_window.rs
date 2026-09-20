//! 文档窗（P7-I2 / **D2**）：**一个非模态窗口类、两种模式** —— 教程模式（「关于」）与日志模式
//! （「查看日志」）。
//!
//! 类名 `AzusaAI.LocalProxy.DocWindow`（owner = 主窗，**仅**用于 Z 序与任务栏归组）；客户区基线
//! **720 × 440 DIP**（DPI 缩放），`WS_SIZEBOX` + `WM_GETMINMAXINFO` 夹到工作区 90%。
//!
//! ## 两种模式
//! - **教程模式**（Arch-B §2.7.3）：8 章正文（[`super::about_text`]）+ 自绘章节条（点击跳章）+
//!   `[复制全文]`；正文 = UI 字体 13。原「关于」的 `MessageBoxW` 路径**删除**。
//! - **日志模式**（Arch-A C 组规约 C-1…C-4）：`Consolas 12` + 顶部读数行（复用
//!   [`super::log_stat_text`]，**唯一文案真源**）+ 底部状态行（`跟随最新` /
//!   `已暂停跟随（滚到底部即恢复）`）+ `[复制全部] [跳到底部]`；原生 `WS_VSCROLL` 滚动
//!   （**不自绘滚动条**，Open-3）。
//!
//! ## 日志模式的四条硬约束（判据钉在它们上）
//! 1. **C-1 固定客户区**：尺寸只由用户拖拽 / DPI 决定 ⇒ 高度不随日志行数变化（J-C1）；文本上限
//!    `EM_SETLIMITTEXT` **必设** = [`LOG_EDIT_CHAR_LIMIT`]（`EM_GETLIMITTEXT` 可读回，A-11）。
//! 2. **C-2 只追加**：`Logger::written_count()` 是"新增了几行"的唯一判据 ⇒ `EM_SETSEL(-1,-1)` +
//!    `EM_REPLACESEL` **一次**；`new == 0` ⇒ **一次写都不发**（满环静置每秒 0 次写入，J-C4）。
//!    超 [`LOG_VIEW_MAX_LINES`] 时按行首偏移截头（标准 EDIT 手法）。
//! 3. **C-3 跟随策略**：追加前取一次滚动读数；`at_bottom := nPos >= nMax - nPage + 1`（C-3 原文）
//!    **并**用 EDIT 自己的行度量做稳健贴底（`EM_GETFIRSTVISIBLELINE + nPage >= EM_GETLINECOUNT`）；
//!    `at_bottom ⇒ follow = true`；**只有"真的往上滚过"才转 `follow = false`**（用上一轮的
//!    `nPos`/首行做比较）—— 见 §2.C.6 的实测：把 `follow` 直接写成 `= at_bottom` 会让"刚打开、
//!    视口还在顶部"的窗口永久停更（既不是用户意图，也让 J-C3 的恢复臂失效）。`follow && new > 0`
//!    ⇒ [`align_to_bottom`]（`EM_SCROLLCARET` 在"光标已在末行但视口未动"时不生效 ⇒ 追加用
//!    `EM_LINESCROLL` 按行度量强制贴底）。
//! 4. **D6 诊断绕环**：每轮的 `日志窗口刷新 N 行` 走 `output::log_line`（直写 stdout），
//!    **不经 `Logger::log`** ⇒ 否则诊断行自身入环 ⇒ debug 档每秒永续追加（J-C4 必红）。
//!
//! ## 行单位口径（**视图行数上限 = 逻辑行数**）
//! `LOG_VIEW_MAX_LINES = 2000` 数的是**日志行**（逻辑行），而 `EM_LINEINDEX` / `EM_GETFIRSTVISIBLELINE`
//! 在有**折行**时数的是**显示行** ⇒ 截头会按错误的单位少截 ⇒ 视图无界增长（I2 真机实测：满环静置后
//! `nMax` 从 2000 涨到 4311）。故 EDIT 带 **`ES_AUTOHSCROLL` + `WS_HSCROLL`**（长行横向滚动、不折行）
//! ⇒ 逻辑行 == 显示行，截头单位与上限自洽（滚动条仍为**原生**，Open-3 不变）。
//!
//! ## D3 隐藏语义 = **独立可见**
//! owner 关系不会因 `SW_HIDE(owner)` 隐藏 owned 窗口 ⇒ 本文件**不接** `WM_SHOWWINDOW` /
//! `WM_CLOSE` 联动；`ui::hide_to_tray()` 里也没有任何文档窗操作 ⇒ 主窗进托盘时文档窗保持可见。

use std::sync::Arc;
use std::time::{Duration, Instant};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, DrawTextW, EndPaint, FillRect, InvalidateRect, RedrawWindow, SelectObject,
    SetBkColor, SetBkMode, SetTextColor, DRAW_TEXT_FORMAT, DT_CENTER, DT_LEFT, DT_NOPREFIX,
    DT_SINGLELINE, DT_VCENTER, HDC, HFONT, HGDIOBJ, OPAQUE, PAINTSTRUCT, RDW_ALLCHILDREN,
    RDW_ERASE, RDW_INVALIDATE, RDW_UPDATENOW, TRANSPARENT,
};
use windows::Win32::UI::Controls::{
    DRAWITEMSTRUCT, EM_GETFIRSTVISIBLELINE, EM_GETLINECOUNT, EM_LINEFROMCHAR, EM_LINEINDEX,
    EM_LINESCROLL, EM_REPLACESEL, EM_SCROLLCARET, EM_SETLIMITTEXT, EM_SETSEL, ODS_DISABLED,
    ODS_FOCUS, ODS_SELECTED, ODT_BUTTON,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect,
    GetScrollInfo, GetSystemMetrics, GetWindowLongPtrW, GetWindowTextLengthW, GetWindowTextW,
    IsWindow, LoadCursorW, RegisterClassW, SendMessageW, SetForegroundWindow, SetWindowLongPtrW,
    SetWindowPos, SetWindowTextW, ShowWindow, SystemParametersInfoW, BS_OWNERDRAW, CREATESTRUCTW,
    CW_USEDEFAULT, ES_AUTOHSCROLL, ES_AUTOVSCROLL, ES_MULTILINE, ES_READONLY, GWLP_USERDATA, HMENU,
    IDC_ARROW, MINMAXINFO, SB_VERT, SCROLLINFO, SIF_ALL, SIF_PAGE, SM_CXSCREEN, SM_CYSCREEN,
    SPI_GETWORKAREA, SWP_NOACTIVATE, SWP_NOZORDER, SW_HIDE, SW_SHOW,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_COMMAND,
    WM_COPY, WM_CREATE, WM_CTLCOLOREDIT, WM_CTLCOLORSTATIC, WM_DESTROY, WM_DPICHANGED, WM_DRAWITEM,
    WM_ERASEBKGND, WM_GETMINMAXINFO, WM_LBUTTONUP, WM_NCCREATE, WM_NCDESTROY, WM_PAINT, WM_SETFONT,
    WM_SIZE, WNDCLASSW, WS_CAPTION, WS_CHILD, WS_CLIPCHILDREN, WS_HSCROLL, WS_OVERLAPPED,
    WS_SYSMENU, WS_TABSTOP, WS_THICKFRAME, WS_VISIBLE, WS_VSCROLL,
};

use azusa_local_proxy::logging::{Logger, LOG_LINE_MAX_BYTES, LOG_RING_CAPACITY};
use azusa_local_proxy::output::log_line;

use super::about_text::{self, AboutText};
use super::theme::{
    fit_text, gdi_color, wide, Gfx, APP_ICON_RESOURCE_ID, BTN_H, COLOR_ACCENT_DEEP,
    COLOR_ACCENT_SOFT, COLOR_BORDER_STRONG, COLOR_CARD, COLOR_CARD_BORDER, COLOR_INK,
    COLOR_INK_DISABLED, COLOR_INK_SOFT, COLOR_PRESS_ON_CARD, COLOR_SEPARATOR, RADIUS_BUTTON,
    SPACE_L, SPACE_M, SPACE_S,
};
use super::{clip_bytes, log_stat_text, UiState};

/// 单行 `STATIC` 不折行（值同 Win32 头文件；`Win32_System_SystemServices` 未启用 ⇒ 局部常量）。
const SS_LEFTNOWORDWRAP: u32 = 0x000C;

/// 窗口类名（教程/日志两模式共用一个类 ⇒ 「关于」与「查看日志」不会开出两个类）。
pub const DOC_WINDOW_CLASS: &str = "AzusaAI.LocalProxy.DocWindow";
/// 客户区基线（DIP；DPI 缩放；两模式同基线）。
pub const DOC_WINDOW_CLIENT: (f32, f32) = (720.0, 440.0);
/// 客户区下限（DIP；`WM_GETMINMAXINFO` 的 `ptMinTrackSize`）。
pub const DOC_WINDOW_MIN: (f32, f32) = (520.0, 320.0);
/// 客户区上限 = 工作区 × 本系数（`ptMaxTrackSize` / `ptMaxSize`）。
pub const DOC_MAX_WORK_FRACTION: f32 = 0.9;
/// 视口行数上限（= 环容量 2000；超额截头 —— 视图是环的只读投影）。
pub const LOG_VIEW_MAX_LINES: usize = LOG_RING_CAPACITY;
/// 视图逐行字节上限（与「复制日志」口径同值：`ui::LOG_PANEL_LINE_BYTES`）。
pub const LOG_VIEW_LINE_BYTES: usize = super::LOG_PANEL_LINE_BYTES;
/// `EM_SETLIMITTEXT` 的文本上限（**与 `logging.rs` 的 `LOG_LINE_MAX_BYTES` 同源，不写魔数**）。
///
/// 推导（保守**字符**上界；`字符数 ≤ 字节数` 恒成立 ⇒ 偏宽、绝不截断合法文本）：
/// `2000 行 × 4096 B + 4096 B 余量 = 8,196,096`。末项取同一常量只为不引入新魔数
/// （语义 = 换行与行数余量）。
pub const LOG_EDIT_CHAR_LIMIT: usize = LOG_VIEW_MAX_LINES * LOG_LINE_MAX_BYTES + LOG_LINE_MAX_BYTES;

/// 正文框（只读多行 `EDIT`）。
pub const IDC_DOC_EDIT: usize = 3501;
/// 底部状态行（`跟随最新` / `已暂停跟随（滚到底部即恢复）`）。
pub const IDC_DOC_STATUS: usize = 3502;
/// `[复制全文]`（教程）/ `[复制全部]`（日志）—— 同一控件、按模式改文案。
pub const IDC_DOC_COPY: usize = 3503;
/// `[跳到底部]`（仅日志模式可见）。`3505` / `3506` 预留给「清空显示」/「关闭」（Open-12）。
pub const IDC_DOC_BOTTOM: usize = 3504;
/// 顶部读数行（仅日志模式可见；文案走 [`super::log_stat_text`]）。
pub const IDC_DOC_READOUT: usize = 3507;

/// 状态行两条逐字串（判据 J-C3 依赖逐字匹配）。
pub const STATUS_FOLLOWING: &str = "跟随最新";
pub const STATUS_PAUSED: &str = "已暂停跟随（滚到底部即恢复）";

/// 两种模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocMode {
    Tutorial,
    Log,
}

/// 文档窗的 UI 线程独占状态（寿命约定同 `SettingsControls`：只在窗口活着时存在）。
pub struct DocWindowState {
    pub hwnd: HWND,
    pub edit: HWND,
    pub status: HWND,
    pub readout: HWND,
    pub buttons: [HWND; 2],
    pub mode: DocMode,
    /// 已并入视图的 `Logger::written_count()` 读数（只追加的游标）。
    pub seen_written: u64,
    /// 视图里的**行数**（截头后的实际值；`plan_append` 的截头量由它算）。
    pub rendered_lines: u64,
    /// 是否跟随最新（贴底 ⇒ true；**真的往上滚过** ⇒ false —— 见 [`next_follow`]）。
    pub follow: bool,
    /// 上一轮的 `(nPos, 首行行号)`：用来判"用户是否真的往上滚过"（`next_follow` 的 `moved_up`）。
    last_scroll: Option<(i32, i32)>,
    /// 工作区缓存（像素；`WM_GETMINMAXINFO` 不打系统调用，只在 `WM_CREATE`/`WM_DPICHANGED` 刷新）。
    pub work_area: (i32, i32, i32, i32),
    /// 最近一轮刷新的耗时（绕环诊断的读数）。
    pub last_refresh_ms: u32,
    /// 上一轮刷新时刻（把刷新钉在 ≤1 Hz：`UiState::refresh()` 也会被状态变化消息驱动）。
    last_tick: Option<Instant>,
    /// 已写进控件的读数行/状态行文本（相同则不重设，避免每秒无谓重绘）。
    last_readout: String,
    last_status: String,
    /// 教程模式的章节表（**已换算到 CRLF 正文的 UTF-16 偏移**，`EM_SETSEL` 口径）。
    chapters: Vec<(&'static str, usize)>,
}

impl DocWindowState {
    fn new(mode: DocMode) -> DocWindowState {
        DocWindowState {
            hwnd: HWND(std::ptr::null_mut()),
            edit: HWND(std::ptr::null_mut()),
            status: HWND(std::ptr::null_mut()),
            readout: HWND(std::ptr::null_mut()),
            buttons: [HWND(std::ptr::null_mut()); 2],
            mode,
            seen_written: 0,
            rendered_lines: 0,
            follow: true,
            last_scroll: None,
            work_area: (0, 0, 0, 0),
            last_refresh_ms: 0,
            last_tick: None,
            last_readout: String::new(),
            last_status: String::new(),
            chapters: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// 纯函数（判据面：布局 / 只追加 / 换行口径 / 文案）
// ---------------------------------------------------------------------------

/// 像素矩形（自算；与 `RECT` 形状不同，避免 `right/bottom` 混用 —— 单位统一 = 像素）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl DocRect {
    fn right(self) -> i32 {
        self.x + self.w
    }

    fn bottom(self) -> i32 {
        self.y + self.h
    }

    fn to_rect(self) -> RECT {
        RECT {
            left: self.x,
            top: self.y,
            right: self.right(),
            bottom: self.bottom(),
        }
    }
}

/// 文档窗布局（**唯一真源**：创建、`WM_SIZE`、`WM_PAINT`、命中测试与单测读同一份）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocLayout {
    pub client_w: i32,
    pub client_h: i32,
    pub title: DocRect,
    pub subtitle: DocRect,
    /// 两个动作按钮位（右对齐；教程模式只有一个可见 ⇒ 用 [`DocLayout::copy_action`]）。
    pub actions: [DocRect; 2],
    /// 章节条 / 读数行的共用"子行"矩形（章节条是父窗自绘，读数行是 STATIC）。
    pub chapter_bar: DocRect,
    pub edit: DocRect,
    pub footer: DocRect,
    pub status: DocRect,
}

impl DocLayout {
    /// `[复制全文]`/`[复制全部]` 的矩形：教程模式只有一个按钮 ⇒ 占最右槽（右对齐不塌）。
    pub fn copy_action(&self, single: bool) -> DocRect {
        if single {
            self.actions[1]
        } else {
            self.actions[0]
        }
    }

    /// 第 `index` 段章节条（8 段等分，末段吃余数）。
    pub fn chapter_segment(&self, index: usize) -> DocRect {
        let count = about_text::CHAPTER_LABELS.len();
        let base = self.chapter_bar.w / count as i32;
        let x = self.chapter_bar.x + base * index as i32;
        let w = if index + 1 == count {
            self.chapter_bar.w - base * (count as i32 - 1)
        } else {
            base
        };
        DocRect {
            x,
            y: self.chapter_bar.y,
            w,
            h: self.chapter_bar.h,
        }
    }

    /// 命中章节条：返回章节下标（点在条外 ⇒ `None`）。
    pub fn chapter_at(&self, x: i32, y: i32) -> Option<usize> {
        if y < self.chapter_bar.y || y >= self.chapter_bar.bottom() {
            return None;
        }
        (0..about_text::CHAPTER_LABELS.len()).find(|index| {
            let segment = self.chapter_segment(*index);
            x >= segment.x && x < segment.right()
        })
    }
}

/// 布局常量（DIP）。
const PAD_DIP: f32 = SPACE_M;
const HEADER_H_DIP: f32 = 64.0;
const SUB_ROW_H_DIP: f32 = 28.0;
const FOOTER_H_DIP: f32 = 40.0;
const TITLE_Y_DIP: f32 = SPACE_L;
const TITLE_H_DIP: f32 = 28.0;
const SUBTITLE_H_DIP: f32 = 14.0;
const ACTION_W_DIP: f32 = 88.0;
const ACTION_GAP_DIP: f32 = SPACE_S;
const STATUS_Y_DIP: f32 = 10.0;
const STATUS_H_DIP: f32 = 20.0;

/// 布局纯函数（A-3 的文档窗版：全部子矩形 ⊂ 客户区 ∧ 不重叠 ∧ 正文高 > 0）。
pub fn layout(scale: f32, client_w: i32, client_h: i32) -> DocLayout {
    let px = |dip: f32| -> i32 { (dip * scale).round() as i32 };
    let pad = px(PAD_DIP);
    let content_w = (client_w - pad * 2).max(1);
    let header_h = px(HEADER_H_DIP);
    let sub_row_h = px(SUB_ROW_H_DIP);
    let footer_h = px(FOOTER_H_DIP);
    let action_h = px(BTN_H);
    let action_w = px(ACTION_W_DIP);
    let action_gap = px(ACTION_GAP_DIP);
    let actions = [
        DocRect {
            x: client_w - pad - action_w * 2 - action_gap,
            y: px(TITLE_Y_DIP),
            w: action_w,
            h: action_h,
        },
        DocRect {
            x: client_w - pad - action_w,
            y: px(TITLE_Y_DIP),
            w: action_w,
            h: action_h,
        },
    ];
    let edit_y = header_h + sub_row_h;
    let edit_h = (client_h - footer_h - edit_y).max(1);
    DocLayout {
        client_w,
        client_h,
        title: DocRect {
            x: pad,
            y: px(TITLE_Y_DIP),
            w: content_w,
            h: px(TITLE_H_DIP),
        },
        subtitle: DocRect {
            x: pad,
            y: px(TITLE_Y_DIP) + px(TITLE_H_DIP),
            w: content_w,
            h: px(SUBTITLE_H_DIP),
        },
        actions,
        chapter_bar: DocRect {
            x: pad,
            y: header_h,
            w: content_w,
            h: sub_row_h,
        },
        edit: DocRect {
            x: pad,
            y: edit_y,
            w: content_w,
            h: edit_h,
        },
        footer: DocRect {
            x: 0,
            y: client_h - footer_h,
            w: client_w,
            h: footer_h,
        },
        status: DocRect {
            x: pad,
            y: client_h - footer_h + px(STATUS_Y_DIP),
            w: content_w,
            h: px(STATUS_H_DIP),
        },
    }
}

/// **只追加**的纯函数：算「本轮要追加几行 / 要截掉头部几行」。
///
/// - `new == 0` ⇒ `(0, 0)`：**一次写都不发**（J-C4 的"满环静置每秒 0 次写入"）。
/// - 单轮新增按视图上限钳制（环容量与视图上限同值 ⇒ 单轮最多 2000 行）。
/// - 追加后超过视图上限的部分 = 截头量（视图是环的只读投影，只保留最近 2000 行）。
pub(crate) fn plan_append(seen_written: u64, written: u64, rendered_lines: u64) -> (usize, usize) {
    let new_lines = written.saturating_sub(seen_written);
    if new_lines == 0 {
        return (0, 0);
    }
    let new_lines = new_lines.min(LOG_VIEW_MAX_LINES as u64) as usize;
    let total = rendered_lines.saturating_add(new_lines as u64);
    let trim = total.saturating_sub(LOG_VIEW_MAX_LINES as u64) as usize;
    (new_lines, trim)
}

/// 状态行文案（跟随中 / 已暂停）—— 逐字串是判据面，故收在纯函数里。
pub fn follow_status_text(follow: bool) -> &'static str {
    if follow {
        STATUS_FOLLOWING
    } else {
        STATUS_PAUSED
    }
}

/// 追加**前**的贴底判定（C-3 的口径；EDIT 的经典边界）。
///
/// 判据原文 = `nPos >= nMax - nPage + 1`；整数域上等价于本式的 `nPos > nMax - nPage`
/// （避开 `clippy::int_plus_one`，语义逐位相同）。
pub fn at_bottom(n_pos: i32, n_max: i32, n_page: u32) -> bool {
    n_pos > n_max - n_page as i32
}

/// 视图文本块：逐行按字节上限截断 + **CRLF** 分行；`leading_break` = 前面已有内容（追加时补分隔符）。
///
/// 口径：EDIT 内部以 `\r\n` 分行 ⇒ 偏移/行数一律按 CRLF 文本算（`EM_LINEFROMCHAR` / `EM_LINEINDEX`
/// 的字符下标把两个换行字符都算上）。末尾**不留**多余空行（行数读数与视图一致）。
pub(crate) fn chunk_text(lines: &[String], leading_break: bool) -> String {
    let mut out = String::new();
    for (index, line) in lines.iter().enumerate() {
        if index > 0 || leading_break {
            out.push_str("\r\n");
        }
        let (clipped, truncated) = clip_bytes(line, LOG_VIEW_LINE_BYTES);
        out.push_str(clipped);
        if truncated {
            out.push('…');
        }
    }
    out
}

/// LF 文本里的 UTF-16 偏移 → CRLF 文本里的 UTF-16 偏移（每个换行多出一个 `\r`）。
pub(crate) fn crlf_offset(lf_text: &str, offset_units: usize) -> usize {
    let mut units = 0usize;
    let mut extra = 0usize;
    for ch in lf_text.chars() {
        if units >= offset_units {
            break;
        }
        if ch == '\n' {
            extra += 1;
        }
        units += ch.len_utf16();
    }
    offset_units + extra
}

/// 教程正文 + 章节偏移（**已换算到 CRLF 正文的 UTF-16 口径**，可直接喂 `EM_SETSEL`）。
pub(crate) fn tutorial_crlf_text() -> (String, Vec<(&'static str, usize)>) {
    let AboutText { body, chapters } = about_text::tutorial_text();
    let crlf = body.replace('\n', "\r\n");
    let offsets = chapters
        .into_iter()
        .map(|(label, offset)| (label, crlf_offset(&body, offset)))
        .collect();
    (crlf, offsets)
}

/// 窗口标题（标题栏 + 自绘头部同一串）。
fn mode_title(mode: DocMode) -> &'static str {
    match mode {
        DocMode::Tutorial => "使用教程 — AzusaAI 本地反向代理",
        DocMode::Log => "日志（内存）— AzusaAI 本地反向代理",
    }
}

fn mode_subtitle(mode: DocMode) -> &'static str {
    match mode {
        DocMode::Tutorial => "8 章上手教程 · 正文可选中复制 · 点上方章节条跳转",
        DocMode::Log => "纯内存环形缓冲：退出即消失；需要留存请点「复制全部」",
    }
}

fn mode_copy_label(mode: DocMode) -> &'static str {
    match mode {
        DocMode::Tutorial => "复制全文",
        DocMode::Log => "复制全部",
    }
}

// ---------------------------------------------------------------------------
// 窗口类注册 / 打开 / 关闭
// ---------------------------------------------------------------------------

/// 窗口样式：可缩放 + 系统菜单（标题栏关闭按钮）；`WS_CLIPCHILDREN` 让自绘 chrome 不与子控件抢像素。
fn window_style() -> WINDOW_STYLE {
    WINDOW_STYLE((WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_THICKFRAME | WS_CLIPCHILDREN).0)
}

/// 注册窗口类（与主窗/设置窗同形：`hIcon` 走同一条 `theme::load_app_icon` 入口）。
pub fn register_class(hinst: HINSTANCE, logger: &Arc<Logger>) -> Result<(), String> {
    let class_name = wide(DOC_WINDOW_CLASS);
    let icon = super::theme::load_app_icon(hinst, 32);
    if icon.is_invalid() {
        logger.warn(&format!(
            "文档窗图标加载失败（资源 ID {APP_ICON_RESOURCE_ID}）：标题栏将显示系统占位图标"
        ));
    }
    unsafe {
        let cursor = LoadCursorW(None, IDC_ARROW).map_err(|err| format!("加载光标失败：{err}"))?;
        let window_class = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            hIcon: icon,
            hCursor: cursor,
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        if RegisterClassW(&window_class) == 0 {
            return Err(format!("注册窗口类 {DOC_WINDOW_CLASS} 失败"));
        }
    }
    Ok(())
}

/// 打开（或聚焦）文档窗；已开 ⇒ **切模式**并聚焦（与 `settings_window::open` 同形）。
pub fn open(ui: &mut UiState, mode: DocMode) {
    let existing = ui
        .doc_window
        .as_ref()
        .map(|state| state.hwnd)
        .filter(|hwnd| !hwnd.is_invalid() && unsafe { IsWindow(Some(*hwnd)).as_bool() });
    if let Some(hwnd) = existing {
        if ui.doc_window.as_ref().map(|state| state.mode) != Some(mode) {
            switch_mode(ui, mode);
        }
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
        }
        return;
    }

    let (client_w, client_h) = (
        ui.theme.px(DOC_WINDOW_CLIENT.0),
        ui.theme.px(DOC_WINDOW_CLIENT.1),
    );
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: client_w,
        bottom: client_h,
    };
    let class_name = wide(DOC_WINDOW_CLASS);
    let title = wide(mode_title(mode));
    // 状态先落 `UiState`（`lpCreateParams` 只传 `UiState*`）⇒ `WM_CREATE` 里读它完成建控件。
    ui.doc_window = Some(DocWindowState::new(mode));
    unsafe {
        let _ = AdjustWindowRectEx(&mut rect, window_style(), false, WINDOW_EX_STYLE(0));
        let pointer = ui as *mut UiState;
        let created = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(title.as_ptr()),
            window_style(),
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            rect.right - rect.left,
            rect.bottom - rect.top,
            Some(ui.hwnd),
            None,
            Some(ui.hinst),
            Some(pointer as *const core::ffi::c_void),
        );
        match created {
            Ok(hwnd) => {
                if let Some(state) = ui.doc_window.as_mut() {
                    state.hwnd = hwnd;
                }
                let _ = ShowWindow(hwnd, SW_SHOW);
                let _ = SetForegroundWindow(hwnd);
            }
            Err(err) => {
                ui.doc_window = None;
                ui.logger.error(&format!("创建文档窗失败：{err}"));
            }
        }
    }
}

/// 销毁（退出路径用；`WM_DESTROY` 自己会清 `ui.doc_window`）。
pub fn destroy(ui: &mut UiState) {
    let hwnd = ui.doc_window.as_ref().map(|state| state.hwnd);
    if let Some(hwnd) = hwnd {
        if !hwnd.is_invalid() {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
        }
    }
    ui.doc_window = None;
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

fn client_size(hwnd: HWND) -> (i32, i32) {
    let mut rect = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut rect);
    }
    (rect.right - rect.left, rect.bottom - rect.top)
}

fn current_layout(ui: &UiState, hwnd: HWND) -> DocLayout {
    let (width, height) = client_size(hwnd);
    layout(ui.theme.scale, width, height)
}

// ---------------------------------------------------------------------------
// 建控件 / 载内容 / 切模式
// ---------------------------------------------------------------------------

/// `WM_CREATE`：建子控件（正文框 + 读数行 + 状态行 + 两个动作按钮）并载入当前模式的内容。
fn build_controls(ui: &mut UiState, hwnd: HWND) -> bool {
    let (mode, layout, theme_scale) = match ui.doc_window.as_ref() {
        Some(state) => (state.mode, current_layout(ui, hwnd), ui.theme.scale),
        None => return false,
    };
    let _ = theme_scale;
    let hinst = ui.hinst;
    let font_body = match mode {
        DocMode::Tutorial => ui.theme.font_ui,
        DocMode::Log => ui.theme.font_mono,
    };
    let font_small = ui.theme.font_small;
    let edit_style = (WS_CHILD
        | WS_VISIBLE
        | WS_TABSTOP
        | WS_VSCROLL
        | WS_HSCROLL
        | WINDOW_STYLE(ES_MULTILINE as u32)
        | WINDOW_STYLE(ES_READONLY as u32)
        | WINDOW_STYLE(ES_AUTOVSCROLL as u32)
        | WINDOW_STYLE(ES_AUTOHSCROLL as u32))
    .0;
    let static_style = (WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_LEFTNOWORDWRAP)).0;
    let button_style = (WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32)).0;

    let single = mode == DocMode::Tutorial;
    let edit = create_child(
        hinst,
        hwnd,
        "EDIT",
        "",
        edit_style,
        layout.edit,
        IDC_DOC_EDIT,
    );
    let readout = create_child(
        hinst,
        hwnd,
        "STATIC",
        "",
        static_style,
        layout.chapter_bar,
        IDC_DOC_READOUT,
    );
    let status = create_child(
        hinst,
        hwnd,
        "STATIC",
        "",
        static_style,
        layout.status,
        IDC_DOC_STATUS,
    );
    let copy = create_child(
        hinst,
        hwnd,
        "BUTTON",
        mode_copy_label(mode),
        button_style,
        layout.copy_action(single),
        IDC_DOC_COPY,
    );
    let bottom = create_child(
        hinst,
        hwnd,
        "BUTTON",
        "跳到底部",
        button_style,
        layout.actions[1],
        IDC_DOC_BOTTOM,
    );
    if edit.is_invalid()
        || readout.is_invalid()
        || status.is_invalid()
        || copy.is_invalid()
        || bottom.is_invalid()
    {
        ui.logger
            .error("创建文档窗子控件失败（正文框/读数行/状态行/按钮）");
        return false;
    }
    for (control, font) in [
        (edit, font_body),
        (readout, font_small),
        (status, font_small),
        (copy, font_small),
        (bottom, font_small),
    ] {
        unsafe {
            let _ = SendMessageW(
                control,
                WM_SETFONT,
                Some(WPARAM(font.0 as usize)),
                Some(LPARAM(1)),
            );
        }
    }
    // C-1 必设：文本上限（未设 ⇒ 交给 EDIT 默认上限 = Win7 上可能只有 32K 量级 ⇒ 静默截断）
    unsafe {
        let _ = SendMessageW(
            edit,
            EM_SETLIMITTEXT,
            Some(WPARAM(LOG_EDIT_CHAR_LIMIT)),
            Some(LPARAM(0)),
        );
    }

    let work_area = query_work_area();
    if let Some(state) = ui.doc_window.as_mut() {
        state.edit = edit;
        state.readout = readout;
        state.status = status;
        state.buttons = [copy, bottom];
        state.work_area = work_area;
        state.last_tick = None;
        state.last_readout.clear();
        state.last_status.clear();
    }
    if let Some(state) = ui.doc_window.as_mut() {
        match mode {
            DocMode::Tutorial => load_tutorial(state),
            DocMode::Log => load_log(state, &ui.logger),
        }
    }
    apply_mode_visibility(ui, layout, single);
    // 布局收口之后再贴底（`load_log` 期间的滚动会被随后的 `WM_SIZE`/重排清掉）
    if let Some(state) = ui.doc_window.as_mut() {
        if mode == DocMode::Log && state.follow {
            align_to_bottom(state.edit);
            let reading = scroll_state(state.edit);
            state.last_scroll = Some((reading.n_pos, reading.first));
        }
    }
    true
}

fn create_child(
    hinst: HINSTANCE,
    parent: HWND,
    class: &str,
    text: &str,
    style: u32,
    rect: DocRect,
    id: usize,
) -> HWND {
    let class_wide = wide(class);
    let text_wide = wide(text);
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class_wide.as_ptr()),
            PCWSTR(text_wide.as_ptr()),
            WINDOW_STYLE(style),
            rect.x,
            rect.y,
            rect.w,
            rect.h,
            Some(parent),
            Some(HMENU(id as *mut core::ffi::c_void)),
            Some(hinst),
            None,
        )
    }
    .unwrap_or_default()
}

/// 教程模式：正文 = 8 章教程（CRLF）+ 章节偏移表；状态行 = 提示语。
fn load_tutorial(state: &mut DocWindowState) {
    let (body, chapters) = tutorial_crlf_text();
    state.chapters = chapters;
    set_window_text(state.edit, &body);
    state.seen_written = 0;
    state.rendered_lines = 0;
    state.follow = false;
    let hint = "点上方章节条跳转；正文可整段选中复制";
    set_window_text(state.status, hint);
    state.last_status = hint.to_string();
    set_window_text(state.readout, "");
    state.last_readout.clear();
    unsafe {
        let _ = SendMessageW(state.edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(0)));
        let _ = SendMessageW(state.edit, EM_SCROLLCARET, Some(WPARAM(0)), Some(LPARAM(0)));
    }
}

/// 日志模式：打开时**一次性**载入最近 [`LOG_VIEW_MAX_LINES`] 行（最坏 ≈400 KB，只此一次）。
fn load_log(state: &mut DocWindowState, logger: &Logger) {
    let (lines, _) = logger.snapshot_lines(LOG_VIEW_MAX_LINES);
    let text = chunk_text(&lines, false);
    set_window_text(state.edit, &text);
    state.rendered_lines = lines.len() as u64;
    state.seen_written = logger.written_count();
    state.follow = true;
    state.chapters.clear();
    state.last_readout.clear();
    state.last_status.clear();
    update_readout(state, logger);
    update_status(state, true);
    align_to_bottom(state.edit);
}

/// 切模式（已开窗口时）：正文字体 + 内容 + 按钮文案/位置 + 标题。
fn switch_mode(ui: &mut UiState, mode: DocMode) {
    let hwnd = match ui.doc_window.as_ref() {
        Some(state) => state.hwnd,
        None => return,
    };
    if let Some(state) = ui.doc_window.as_mut() {
        state.mode = mode;
    }
    let layout = current_layout(ui, hwnd);
    let single = mode == DocMode::Tutorial;
    let font_body = match mode {
        DocMode::Tutorial => ui.theme.font_ui,
        DocMode::Log => ui.theme.font_mono,
    };
    if let Some(state) = ui.doc_window.as_mut() {
        unsafe {
            let _ = SendMessageW(
                state.edit,
                WM_SETFONT,
                Some(WPARAM(font_body.0 as usize)),
                Some(LPARAM(1)),
            );
        }
        set_window_text(state.buttons[0], mode_copy_label(mode));
        match mode {
            DocMode::Tutorial => load_tutorial(state),
            DocMode::Log => load_log(state, &ui.logger),
        }
    }
    set_window_text(hwnd, mode_title(mode));
    apply_mode_visibility(ui, layout, single);
    apply_layout(ui, layout, hwnd);
    // 布局收口之后再贴底（`load_log` 期间的滚动会被随后的 `WM_SIZE`/重排清掉）
    if mode == DocMode::Log {
        if let Some(state) = ui.doc_window.as_mut() {
            align_to_bottom(state.edit);
            let reading = scroll_state(state.edit);
            state.last_scroll = Some((reading.n_pos, reading.first));
        }
    }
}

/// 按模式设置"读数行 / 状态行 / 第二个按钮"的显隐与 `[复制*]` 的位置（右对齐不塌）。
fn apply_mode_visibility(ui: &UiState, layout: DocLayout, single: bool) {
    let Some(state) = ui.doc_window.as_ref() else {
        return;
    };
    unsafe {
        let _ = ShowWindow(state.readout, if single { SW_HIDE } else { SW_SHOW });
        let _ = ShowWindow(state.buttons[1], if single { SW_HIDE } else { SW_SHOW });
        let rect = layout.copy_action(single);
        let _ = SetWindowPos(
            state.buttons[0],
            None,
            rect.x,
            rect.y,
            rect.w,
            rect.h,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

// ---------------------------------------------------------------------------
// 每秒刷新（C-2 只追加 + C-3 跟随）
// ---------------------------------------------------------------------------

/// 1 s 节拍：算新增 ⇒ 只追加 ⇒ 跟随策略 ⇒ 读数行/状态行 ⇒ 绕环诊断。
///
/// 刷新被钉在 **≤1 Hz**：`UiState::refresh()` 除了 1 s `WM_TIMER` 也会被状态变化消息
/// （`WM_APP_REFRESH`）驱动 ⇒ 不节流的话"每秒恰好 1 条诊断"会随状态变化抖动。
pub fn refresh(ui: &mut UiState) {
    {
        let Some(state) = ui.doc_window.as_mut() else {
            return;
        };
        if state.hwnd.is_invalid() || state.edit.is_invalid() || state.mode != DocMode::Log {
            return;
        }
        match state.last_tick {
            Some(tick) if tick.elapsed() < Duration::from_millis(950) => return,
            _ => state.last_tick = Some(Instant::now()),
        }
    }

    let logger = Arc::clone(&ui.logger);
    let Some(state) = ui.doc_window.as_mut() else {
        return;
    };
    let written = logger.written_count();
    let (new_lines, trim) = plan_append(state.seen_written, written, state.rendered_lines);
    // C-3：追加**前**取一次滚动读数（贴底判定 + 自动恢复判据）
    let scroll = scroll_state(state.edit);
    let moved_up = state
        .last_scroll
        .map(|(n_pos, first)| scroll.n_pos < n_pos || scroll.first < first)
        .unwrap_or(false);
    let follow = next_follow(state.follow, scroll.at_bottom, moved_up);
    state.follow = follow;
    state.last_scroll = Some((scroll.n_pos, scroll.first));
    let started = Instant::now();
    let mut appended = 0usize;
    if new_lines > 0 {
        let (lines, _) = logger.snapshot_lines(new_lines);
        if !lines.is_empty() {
            if trim > 0 {
                trim_head(state.edit, trim);
            }
            let leading_break = state.rendered_lines > trim as u64;
            let text = chunk_text(&lines, leading_break);
            append_text(state.edit, &text);
            appended = lines.len();
            state.seen_written = written;
            state.rendered_lines = state
                .rendered_lines
                .saturating_add(appended as u64)
                .saturating_sub(trim as u64);
        }
    }
    if appended > 0 {
        state.last_refresh_ms = started.elapsed().as_millis().min(u32::MAX as u128) as u32;
    }
    if follow && appended > 0 {
        // 贴底用**确定性对齐**（`EM_SCROLLCARET` 在"光标已在末行、视口却没动"时不生效）
        align_to_bottom(state.edit);
    } else if appended > 0 {
        // C-3：`!follow ⇒ 不动视图`。截头用 `EM_SETSEL(0, …)` 会**把光标带走** ⇒ 视口会跳到顶部，
        // 故按行度量把它对回来：同一内容在截头后上移 `trim` 行，重新贴到视口首行。
        let current = unsafe {
            SendMessageW(
                state.edit,
                EM_GETFIRSTVISIBLELINE,
                Some(WPARAM(0)),
                Some(LPARAM(0)),
            )
            .0 as i32
        };
        // **前置条件（P2-①）**：`first >= trim` 时才存在"同一内容"可对回；`first < trim` ⇒ 用户
        // 正看的头部**已被截掉**（视图被顶到最顶），此时取 0（贴顶）并把条件写进 [`restore_line_after_trim`]。
        let target = restore_line_after_trim(scroll.first, trim);
        let delta = target - current;
        if delta != 0 {
            unsafe {
                let _ = SendMessageW(
                    state.edit,
                    EM_LINESCROLL,
                    Some(WPARAM(0)),
                    Some(LPARAM(delta as isize)),
                );
            }
        }
    }
    if appended > 0 {
        // 程序性追加/截头/滚动之后同步重绘（否则旧像素留在屏幕上 = 重影，见 `repaint_now`）
        repaint_now(state.edit);
    }
    update_readout(state, &logger);
    let last_ms = state.last_refresh_ms;
    let (n_pos, n_max, n_page) = (scroll.n_pos, scroll.n_max, scroll.n_page);
    update_status(state, follow);
    if appended > 0 {
        // D6：**绕环** —— 直写 stdout，不经 `Logger::log`（否则自反馈入环 ⇒ J-C4 必红）
        log_line(&format!(
            "日志窗口刷新 {appended} 行（截头 {trim} 行；滚动 nPos={n_pos} nMax={n_max} nPage={n_page}）耗时 {last_ms} ms"
        ));
    }
}

/// 追加前的滚动读数：C-3 口径（`GetScrollInfo`）+ EDIT 自己行度量的**稳健贴底**。
#[derive(Clone, Copy, Debug)]
struct ScrollReading {
    /// C-3 原文 `nPos >= nMax - nPage + 1`，**或**"视口末行已到文末"（`EM_GETFIRSTVISIBLELINE + nPage >= EM_GETLINECOUNT`）。
    at_bottom: bool,
    n_pos: i32,
    n_max: i32,
    n_page: u32,
    first: i32,
}

impl ScrollReading {
    fn short() -> ScrollReading {
        ScrollReading {
            at_bottom: true,
            n_pos: 0,
            n_max: 0,
            n_page: 0,
            first: 0,
        }
    }
}

fn scroll_state(edit: HWND) -> ScrollReading {
    let mut info = SCROLLINFO {
        cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
        fMask: SIF_ALL,
        ..Default::default()
    };
    let ok = unsafe { GetScrollInfo(edit, SB_VERT, &mut info) }.is_ok();
    let first = unsafe {
        SendMessageW(
            edit,
            EM_GETFIRSTVISIBLELINE,
            Some(WPARAM(0)),
            Some(LPARAM(0)),
        )
        .0 as i32
    };
    let total =
        unsafe { SendMessageW(edit, EM_GETLINECOUNT, Some(WPARAM(0)), Some(LPARAM(0))).0 as i32 };
    if !ok {
        return ScrollReading::short();
    }
    let by_c3 = at_bottom(info.nPos, info.nMax, info.nPage);
    // 稳健臂：视口末行 ≥ 总行数 ⇒ 已到底（EDIT 的行度量不受 GetScrollInfo 的口径影响）
    let robust = total > 0 && first.saturating_add(info.nPage as i32) >= total;
    ScrollReading {
        at_bottom: by_c3 || robust,
        n_pos: info.nPos,
        n_max: info.nMax,
        n_page: info.nPage,
        first,
    }
}

/// `follow` 的下一状态（**纯函数**，可单测）：
///
/// - 贴底 ⇒ 恢复跟随（C-3 的自动恢复）；
/// - **只有"真的往上滚过"才暂停**（`moved_up`；首次读数为 `None` ⇒ 不改变）——
///   否则"刚打开、视口还在顶部"的窗口会被永久判定为已暂停（I2 真机实测到的缺陷）；
/// - 其余情况保持原值（用户没动、内容也没变 ⇒ 不抖动）。
pub fn next_follow(follow: bool, at_bottom: bool, moved_up: bool) -> bool {
    if at_bottom {
        true
    } else if moved_up {
        false
    } else {
        follow
    }
}

/// 截头后"同一内容"应落在的视口首行（**纯函数**，可单测）：
///
/// **前置条件（P2-①）**：只有 `first >= trim` 时才存在"用户正看着的那一行"（截头把内容整体上移
/// `trim` 行）；`first < trim` ⇒ 那一行**已经被截掉**（不在视图里了）⇒ 取 `0`（视口贴顶，不越界、
/// 也不假装还看得见）。判据面：`restore_line_after_trim(first, trim) == max(first − trim, 0)`。
pub fn restore_line_after_trim(first: i32, trim: usize) -> i32 {
    first.saturating_sub(trim as i32).max(0)
}

/// 同步重绘一个控件（**I2b 的重影修法**）：程序性改视图（`EM_REPLACESEL`/截头/`EM_LINESCROLL`）之后，
/// 控件自己的失效区域可能不覆盖"被滚动挪走的旧像素" ⇒ 屏幕上留下**两层文字**（用户报的"重影/叠加"）。
/// 用 `RDW_INVALIDATE|RDW_ERASE|RDW_UPDATENOW` 立刻整块重绘 ⇒ 屏幕内容 = 控件当前状态（判据 A vs B = 0）。
pub(crate) fn repaint_now(control: HWND) {
    unsafe {
        let _ = RedrawWindow(
            Some(control),
            None,
            None,
            RDW_INVALIDATE | RDW_ERASE | RDW_UPDATENOW | RDW_ALLCHILDREN,
        );
    }
}

/// 确定性贴底：光标移到文末 + 按**行度量**强制下滚（`EM_SCROLLCARET` 单独用不可靠）。
pub(crate) fn align_to_bottom(edit: HWND) {
    unsafe {
        let _ = SendMessageW(edit, EM_SETSEL, Some(WPARAM(usize::MAX)), Some(LPARAM(-1)));
        let _ = SendMessageW(edit, EM_SCROLLCARET, Some(WPARAM(0)), Some(LPARAM(0)));
        let total = SendMessageW(edit, EM_GETLINECOUNT, Some(WPARAM(0)), Some(LPARAM(0))).0 as i32;
        let first = SendMessageW(
            edit,
            EM_GETFIRSTVISIBLELINE,
            Some(WPARAM(0)),
            Some(LPARAM(0)),
        )
        .0 as i32;
        let mut info = SCROLLINFO {
            cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
            fMask: SIF_PAGE,
            ..Default::default()
        };
        let page = if GetScrollInfo(edit, SB_VERT, &mut info).is_ok() && info.nPage > 0 {
            info.nPage as i32
        } else {
            1
        };
        let delta = total - page - first;
        if delta > 0 {
            let _ = SendMessageW(
                edit,
                EM_LINESCROLL,
                Some(WPARAM(0)),
                Some(LPARAM(delta as isize)),
            );
        }
    }
    repaint_now(edit);
}

/// 截头：删掉视图最前的 `lines` 行（`EM_SETSEL(0, 行首偏移)` + `EM_REPLACESEL("")`）。
fn trim_head(edit: HWND, lines: usize) {
    let start = unsafe { SendMessageW(edit, EM_LINEINDEX, Some(WPARAM(lines)), Some(LPARAM(0))).0 };
    if start < 0 {
        return;
    }
    let empty = wide("");
    unsafe {
        let _ = SendMessageW(edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(start)));
        let _ = SendMessageW(
            edit,
            EM_REPLACESEL,
            Some(WPARAM(0)),
            Some(LPARAM(empty.as_ptr() as isize)),
        );
    }
}

/// 追加（**一次** `EM_SETSEL(-1,-1)` + **一次** `EM_REPLACESEL`）。
fn append_text(edit: HWND, text: &str) {
    if text.is_empty() {
        return;
    }
    let wide_text = wide(text);
    unsafe {
        let _ = SendMessageW(edit, EM_SETSEL, Some(WPARAM(usize::MAX)), Some(LPARAM(-1)));
        let _ = SendMessageW(
            edit,
            EM_REPLACESEL,
            Some(WPARAM(0)),
            Some(LPARAM(wide_text.as_ptr() as isize)),
        );
    }
}

/// 顶部读数行 —— **复用 I1 的 [`super::log_stat_text`]（唯一文案真源）**。
fn update_readout(state: &mut DocWindowState, logger: &Logger) {
    let text = log_stat_text(
        logger.retained_count(),
        LOG_RING_CAPACITY,
        logger.dropped_count(),
    );
    if text != state.last_readout {
        set_window_text(state.readout, &text);
        state.last_readout = text;
    }
}

/// 底部状态行（逐字串见 [`STATUS_FOLLOWING`] / [`STATUS_PAUSED`]）。
fn update_status(state: &mut DocWindowState, follow: bool) {
    let text = follow_status_text(follow).to_string();
    if text != state.last_status {
        set_window_text(state.status, &text);
        state.last_status = text;
    }
}

/// `[跳到底部]`：立刻贴底 + `follow = true`（与自动恢复同一条路径）。
fn jump_to_bottom(ui: &mut UiState) {
    let Some(state) = ui.doc_window.as_mut() else {
        return;
    };
    align_to_bottom(state.edit);
    state.follow = true;
    // 贴底后把"上一轮读数"同步成底端 ⇒ 下一轮不会被误判成"用户往上滚"
    let reading = scroll_state(state.edit);
    state.last_scroll = Some((reading.n_pos, reading.first));
    let text = follow_status_text(true).to_string();
    if state.last_status != text {
        set_window_text(state.status, &text);
        state.last_status = text;
    }
}

/// `[复制全文]` / `[复制全部]`：教程模式按方案原样 `EM_SETSEL(0,-1)` + `WM_COPY`；日志模式读回正文框
/// 全文再进剪贴板（**不改选区、不动滚动** ⇒ 不打扰跟随状态；剪贴板内容 = 视图全文，逐字符一致）。
fn copy_all(ui: &mut UiState) {
    let (mode, edit) = match ui.doc_window.as_ref() {
        Some(state) => (state.mode, state.edit),
        None => return,
    };
    let done = match mode {
        DocMode::Tutorial => {
            unsafe {
                let _ = SendMessageW(edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
                let _ = SendMessageW(edit, WM_COPY, Some(WPARAM(0)), Some(LPARAM(0)));
            }
            true
        }
        DocMode::Log => match edit_text(edit) {
            Some(text) => super::clipboard::set_text(ui.hwnd, &text),
            None => false,
        },
    };
    if done {
        ui.logger.info(match mode {
            DocMode::Tutorial => "已复制教程全文到剪贴板",
            DocMode::Log => "已复制日志窗口全文到剪贴板",
        });
    } else {
        ui.logger
            .warn("复制失败：剪贴板被其它程序占用（或正文框不可读）");
    }
}

/// 读回正文框全文（`WM_GETTEXTLENGTH` + `GetWindowTextW`）。
fn edit_text(edit: HWND) -> Option<String> {
    let length = unsafe { GetWindowTextLengthW(edit) };
    if length <= 0 {
        return Some(String::new());
    }
    let mut buffer = vec![0u16; length as usize + 1];
    let copied = unsafe { GetWindowTextW(edit, &mut buffer) };
    if copied <= 0 {
        return None;
    }
    buffer.truncate(copied as usize);
    Some(String::from_utf16_lossy(&buffer))
}

/// 跳章节：`EM_SETSEL(offset)` + `EM_SCROLLCARET`，再把该行对齐到视口首行（落点确定，见 I2-6）。
fn jump_to_chapter(ui: &mut UiState, index: usize) {
    let Some(state) = ui.doc_window.as_mut() else {
        return;
    };
    let Some(offset) = state.chapters.get(index).map(|(_, offset)| *offset) else {
        return;
    };
    unsafe {
        let _ = SendMessageW(
            state.edit,
            EM_SETSEL,
            Some(WPARAM(offset)),
            Some(LPARAM(offset as isize)),
        );
        let _ = SendMessageW(state.edit, EM_SCROLLCARET, Some(WPARAM(0)), Some(LPARAM(0)));
        // 把章节首行顶到视口第一行（`EM_SCROLLCARET` 只保证"可见"）
        let line = SendMessageW(
            state.edit,
            EM_LINEFROMCHAR,
            Some(WPARAM(offset)),
            Some(LPARAM(0)),
        )
        .0;
        let first = SendMessageW(
            state.edit,
            EM_GETFIRSTVISIBLELINE,
            Some(WPARAM(0)),
            Some(LPARAM(0)),
        )
        .0;
        if line >= 0 && first >= 0 {
            let delta = line - first;
            if delta != 0 {
                let _ = SendMessageW(
                    state.edit,
                    EM_LINESCROLL,
                    Some(WPARAM(0)),
                    Some(LPARAM(delta as isize)),
                );
            }
        }
        repaint_now(state.edit);
    }
    let label = about_text::chapter_heading(index);
    let text = format!("已跳到 {label}");
    set_window_text(state.status, &text);
    state.last_status = text;
}

// ---------------------------------------------------------------------------
// 绘制（自绘 chrome：标题 / 副标题 / 章节条 / 分隔线）
// ---------------------------------------------------------------------------

fn paint(ui: &UiState, hwnd: HWND, hdc: HDC) {
    let Some(state) = ui.doc_window.as_ref() else {
        return;
    };
    let layout = current_layout(ui, hwnd);
    let theme = &ui.theme;
    let scale = theme.scale;
    let tutorial = state.mode == DocMode::Tutorial;
    let selected = if tutorial { current_chapter(state) } else { 0 };

    if let Some(gfx) = Gfx::from_hdc(hdc) {
        // 头部下缘分隔线 + 注脚上缘分隔线
        let head_y = (layout.chapter_bar.y - 1) as f32;
        gfx.line(
            layout.title.x as f32,
            head_y,
            layout.title.right() as f32,
            head_y,
            COLOR_SEPARATOR,
            1.0 * scale,
        );
        gfx.line(
            0.0,
            layout.footer.y as f32,
            layout.client_w as f32,
            layout.footer.y as f32,
            COLOR_SEPARATOR,
            1.0 * scale,
        );
        if tutorial {
            for index in 0..about_text::CHAPTER_LABELS.len() {
                let segment = layout.chapter_segment(index);
                let (fill, border) = if index == selected {
                    (COLOR_ACCENT_SOFT, COLOR_ACCENT_DEEP)
                } else {
                    (COLOR_CARD, COLOR_CARD_BORDER)
                };
                gfx.fill_round_rect(
                    segment.x as f32,
                    segment.y as f32,
                    segment.w as f32,
                    segment.h as f32,
                    RADIUS_BUTTON * scale,
                    fill,
                );
                gfx.stroke_round_rect(
                    segment.x as f32,
                    segment.y as f32,
                    segment.w as f32,
                    segment.h as f32,
                    RADIUS_BUTTON * scale,
                    border,
                    1.0 * scale,
                );
            }
        }
    }

    draw_text_line(
        hdc,
        theme.font_title,
        mode_title(state.mode),
        layout.title,
        COLOR_INK,
        DT_LEFT,
    );
    draw_text_line(
        hdc,
        theme.font_small,
        mode_subtitle(state.mode),
        layout.subtitle,
        COLOR_INK_SOFT,
        DT_LEFT,
    );
    if tutorial {
        for index in 0..about_text::CHAPTER_LABELS.len() {
            let segment = layout.chapter_segment(index);
            let label = format!("{} {}", index + 1, about_text::CHAPTER_LABELS[index]);
            let inset = DocRect {
                x: segment.x + 4,
                y: segment.y,
                w: (segment.w - 8).max(1),
                h: segment.h,
            };
            let fitted = fit_text(hdc, theme.font_small, &label, inset.w);
            draw_text_line(hdc, theme.font_small, &fitted, inset, COLOR_INK, DT_CENTER);
        }
    }
}

/// 单行文本绘制（左对齐或居中；`DT_NOPREFIX` 关掉 `&` 助记符处理）。
fn draw_text_line(
    hdc: HDC,
    font: HFONT,
    text: &str,
    rect: DocRect,
    color: u32,
    align: DRAW_TEXT_FORMAT,
) {
    if text.is_empty() || rect.w <= 0 || rect.h <= 0 {
        return;
    }
    let mut buffer = wide(text);
    buffer.pop();
    let mut target = rect.to_rect();
    unsafe {
        let old = SelectObject(hdc, HGDIOBJ(font.0));
        let _ = SetBkMode(hdc, TRANSPARENT);
        let _ = SetTextColor(hdc, gdi_color(color));
        let _ = DrawTextW(
            hdc,
            &mut buffer,
            &mut target,
            DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | align,
        );
        SelectObject(hdc, old);
    }
}

/// 当前章节 = 视口首行落在哪一章（`EM_GETFIRSTVISIBLELINE` → `EM_LINEINDEX`）。
fn current_chapter(state: &DocWindowState) -> usize {
    if state.chapters.is_empty() || state.edit.is_invalid() {
        return 0;
    }
    let line = unsafe {
        SendMessageW(
            state.edit,
            EM_GETFIRSTVISIBLELINE,
            Some(WPARAM(0)),
            Some(LPARAM(0)),
        )
        .0
    };
    if line < 0 {
        return 0;
    }
    let index = unsafe {
        SendMessageW(
            state.edit,
            EM_LINEINDEX,
            Some(WPARAM(line as usize)),
            Some(LPARAM(0)),
        )
        .0
    };
    if index < 0 {
        return 0;
    }
    let index = index as usize;
    let mut selected = 0;
    for (position, (_, offset)) in state.chapters.iter().enumerate() {
        if *offset <= index {
            selected = position;
        }
    }
    selected
}

/// 自绘动作按钮（`BS_OWNERDRAW`；次按钮口径 = 卡片底 + 描边 + 按下换不透明叠色）。
unsafe fn draw_action_button(ui: &UiState, item: &DRAWITEMSTRUCT) {
    let rect = item.rcItem;
    let pressed = item.itemState.0 & ODS_SELECTED.0 != 0;
    let disabled = item.itemState.0 & ODS_DISABLED.0 != 0;
    let focused = item.itemState.0 & ODS_FOCUS.0 != 0;
    let theme = &ui.theme;
    let radius = RADIUS_BUTTON * theme.scale;
    let width = (rect.right - rect.left) as f32;
    let height = (rect.bottom - rect.top) as f32;
    let base = if pressed {
        COLOR_PRESS_ON_CARD
    } else {
        COLOR_CARD
    };
    if let Some(gfx) = Gfx::from_hdc(item.hDC) {
        gfx.fill_round_rect(
            rect.left as f32,
            rect.top as f32,
            width,
            height,
            radius,
            base,
        );
        gfx.stroke_round_rect(
            rect.left as f32,
            rect.top as f32,
            width,
            height,
            radius,
            if focused {
                COLOR_BORDER_STRONG
            } else {
                COLOR_CARD_BORDER
            },
            1.0 * theme.scale,
        );
    }
    let text = control_text(item.hwndItem);
    let color = if disabled {
        COLOR_INK_DISABLED
    } else {
        COLOR_INK
    };
    let target = DocRect {
        x: rect.left,
        y: rect.top,
        w: rect.right - rect.left,
        h: rect.bottom - rect.top,
    };
    draw_text_line(item.hDC, theme.font_small, &text, target, color, DT_CENTER);
}

/// 控件当前文本（`BUTTON` 的文案由 `SetWindowTextW` 维护 ⇒ 自绘直接读它，不另存一份真源）。
fn control_text(control: HWND) -> String {
    edit_text(control).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// 消息处理
// ---------------------------------------------------------------------------

/// 工作区（像素；取不到 ⇒ 退回主屏全屏，宁可夹得宽一点也不误判）。
fn query_work_area() -> (i32, i32, i32, i32) {
    let mut work = RECT::default();
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut work as *mut RECT as *mut core::ffi::c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    if ok.is_err() || work.right <= work.left || work.bottom <= work.top {
        return unsafe {
            (
                0,
                0,
                GetSystemMetrics(SM_CXSCREEN),
                GetSystemMetrics(SM_CYSCREEN),
            )
        };
    }
    (work.left, work.top, work.right, work.bottom)
}

fn set_window_text(hwnd: HWND, text: &str) {
    if hwnd.is_invalid() {
        return;
    }
    let wide_text = wide(text);
    unsafe {
        let _ = SetWindowTextW(hwnd, PCWSTR(wide_text.as_ptr()));
    }
}

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
                    IDC_DOC_COPY => copy_all(ui),
                    IDC_DOC_BOTTOM => jump_to_bottom(ui),
                    _ => {}
                });
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let x = (lparam.0 & 0xFFFF) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
                with_ui(hwnd, |ui| {
                    let tutorial =
                        ui.doc_window.as_ref().map(|state| state.mode) == Some(DocMode::Tutorial);
                    if !tutorial {
                        return;
                    }
                    let layout = current_layout(ui, hwnd);
                    if let Some(index) = layout.chapter_at(x, y) {
                        jump_to_chapter(ui, index);
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                });
                LRESULT(0)
            }
            WM_DRAWITEM => {
                let pointer = ui_ptr(hwnd);
                let item = lparam.0 as *const DRAWITEMSTRUCT;
                if pointer.is_null() || item.is_null() || (*item).hDC.is_invalid() {
                    return LRESULT(0);
                }
                if (*item).CtlType == ODT_BUTTON {
                    draw_action_button(&*pointer, &*item);
                    return LRESULT(1);
                }
                LRESULT(0)
            }
            WM_PAINT => {
                let mut paint_struct = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut paint_struct);
                let pointer = ui_ptr(hwnd);
                if !pointer.is_null() {
                    paint(&*pointer, hwnd, hdc);
                }
                let _ = EndPaint(hwnd, &paint_struct);
                LRESULT(0)
            }
            WM_CTLCOLORSTATIC => {
                let hdc = HDC(wparam.0 as *mut core::ffi::c_void);
                let pointer = ui_ptr(hwnd);
                if !pointer.is_null() {
                    let child = HWND(lparam.0 as *mut core::ffi::c_void);
                    let soft = (*pointer)
                        .doc_window
                        .as_ref()
                        .map(|state| state.status == child)
                        .unwrap_or(false);
                    let _ = SetBkMode(hdc, TRANSPARENT);
                    let _ = SetTextColor(
                        hdc,
                        gdi_color(if soft { COLOR_INK_SOFT } else { COLOR_INK }),
                    );
                    return LRESULT((*pointer).theme.brush_bg.0 as isize);
                }
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
            WM_CTLCOLOREDIT => {
                let hdc = HDC(wparam.0 as *mut core::ffi::c_void);
                let pointer = ui_ptr(hwnd);
                if !pointer.is_null() {
                    // ⚠ **不得**用 `SetBkMode(TRANSPARENT)`（I2b 的根因）：多行 EDIT 在滚动/追加时
                    // 靠"每个字形背景格重绘"擦掉旧内容；透明模式只画字形不擦底 ⇒ 旧字形残留，
                    // 新行压上去 = 用户报的"重影/叠加"。
                    // 实测（同装置、修前）：空闲态 A vs 强制整窗重绘后 B 在 EDIT 区内 **diff = 119,763 像素**；
                    // 修后同装置 **diff = 0**。
                    let _ = SetBkMode(hdc, OPAQUE);
                    let _ = SetTextColor(hdc, gdi_color(COLOR_INK));
                    let _ = SetBkColor(hdc, gdi_color(COLOR_CARD));
                    return LRESULT((*pointer).theme.brush_card.0 as isize);
                }
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
            WM_ERASEBKGND => {
                let hdc = HDC(wparam.0 as *mut core::ffi::c_void);
                let pointer = ui_ptr(hwnd);
                if !pointer.is_null() {
                    let mut rect = RECT::default();
                    let _ = GetClientRect(hwnd, &mut rect);
                    FillRect(hdc, &rect, (*pointer).theme.brush_bg);
                    return LRESULT(1);
                }
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
            WM_SIZE => {
                let pointer = ui_ptr(hwnd);
                if !pointer.is_null() {
                    let layout = current_layout(&*pointer, hwnd);
                    with_ui(hwnd, |ui| apply_layout(ui, layout, hwnd));
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                LRESULT(0)
            }
            WM_GETMINMAXINFO => {
                let info = lparam.0 as *mut MINMAXINFO;
                let pointer = ui_ptr(hwnd);
                if !info.is_null() && !pointer.is_null() {
                    let ui = &*pointer;
                    let scale = ui.theme.scale;
                    // 最小尺寸：客户区下限 → 窗口尺寸
                    let mut min = RECT {
                        left: 0,
                        top: 0,
                        right: (DOC_WINDOW_MIN.0 * scale).round() as i32,
                        bottom: (DOC_WINDOW_MIN.1 * scale).round() as i32,
                    };
                    let _ = AdjustWindowRectEx(&mut min, window_style(), false, WINDOW_EX_STYLE(0));
                    (*info).ptMinTrackSize.x = min.right - min.left;
                    (*info).ptMinTrackSize.y = min.bottom - min.top;
                    // 最大尺寸 / 位置：工作区 × 0.9（缓存读数，不打系统调用）
                    let (left, top, work_right, work_bottom) = ui
                        .doc_window
                        .as_ref()
                        .map(|state| state.work_area)
                        .unwrap_or((0, 0, 0, 0));
                    if work_right > left && work_bottom > top {
                        let max_w = ((work_right - left) as f32 * DOC_MAX_WORK_FRACTION) as i32;
                        let max_h = ((work_bottom - top) as f32 * DOC_MAX_WORK_FRACTION) as i32;
                        (*info).ptMaxTrackSize.x = max_w;
                        (*info).ptMaxTrackSize.y = max_h;
                        (*info).ptMaxSize.x = max_w;
                        (*info).ptMaxSize.y = max_h;
                        (*info).ptMaxPosition.x = left;
                        (*info).ptMaxPosition.y = top;
                    }
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
                    let work_area = query_work_area();
                    if let Some(state) = ui.doc_window.as_mut() {
                        state.work_area = work_area;
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
                    refresh_fonts(ui);
                    let layout = current_layout(ui, hwnd);
                    apply_layout(ui, layout, hwnd);
                });
                LRESULT(0)
            }
            WM_CLOSE => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                with_ui(hwnd, |ui| {
                    ui.doc_window = None;
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

/// 重排（唯一入口：创建后收口 / `WM_SIZE` / 模式切换 / DPI 变化）。
fn apply_layout(ui: &UiState, layout: DocLayout, hwnd: HWND) {
    let Some(state) = ui.doc_window.as_ref() else {
        return;
    };
    let single = state.mode == DocMode::Tutorial;
    for (control, rect) in [
        (state.edit, layout.edit),
        (state.readout, layout.chapter_bar),
        (state.status, layout.status),
        (state.buttons[0], layout.copy_action(single)),
        (state.buttons[1], layout.actions[1]),
    ] {
        if control.is_invalid() {
            continue;
        }
        let target = rect.to_rect();
        unsafe {
            let _ = SetWindowPos(
                control,
                None,
                target.left,
                target.top,
                target.right - target.left,
                target.bottom - target.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }
    unsafe {
        let _ = InvalidateRect(Some(hwnd), None, true);
    }
}

/// DPI 变化后重发字体（`EDIT`/`STATIC`/`BUTTON` 吃 `WM_SETFONT`；自绘按钮用主题字体）。
fn refresh_fonts(ui: &UiState) {
    let Some(state) = ui.doc_window.as_ref() else {
        return;
    };
    let body_font = match state.mode {
        DocMode::Tutorial => ui.theme.font_ui,
        DocMode::Log => ui.theme.font_mono,
    };
    for (control, font) in [
        (state.edit, body_font),
        (state.readout, ui.theme.font_small),
        (state.status, ui.theme.font_small),
        (state.buttons[0], ui.theme.font_small),
        (state.buttons[1], ui.theme.font_small),
    ] {
        if control.is_invalid() {
            continue;
        }
        unsafe {
            let _ = SendMessageW(
                control,
                WM_SETFONT,
                Some(WPARAM(font.0 as usize)),
                Some(LPARAM(1)),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 换行风格判据：每个 `\r` 必须紧跟 `\n`，每个 `\n` 必须紧跟 `\r`（纯 CRLF ⇒ true）。
    fn crlf_only(text: &str) -> bool {
        let chars: Vec<char> = text.chars().collect();
        chars.iter().enumerate().all(|(index, ch)| match ch {
            '\r' => chars.get(index + 1) == Some(&'\n'),
            '\n' => index > 0 && chars[index - 1] == '\r',
            _ => true,
        })
    }

    /// `EM_SETLIMITTEXT` 的上限值（P1-1 / D2）：公式同源 + 具体值 8,196,096。
    #[test]
    fn char_limit_matches_formula_and_value() {
        let limit = LOG_EDIT_CHAR_LIMIT;
        assert_eq!(limit, 2000 * 4096 + 4096);
        assert_eq!(limit, 8_196_096);
        // 覆盖最坏情形（2000 行 × 每行 4096 单字节字符 + 换行余量）⇒ 不会被静默截断
        let worst_view = LOG_VIEW_MAX_LINES * LOG_VIEW_LINE_BYTES;
        assert!(
            limit > worst_view,
            "上限必须覆盖最坏视图（{worst_view} 字节）"
        );
    }

    /// 模式常量（D2 基线 / 工作区夹取系数）。
    #[test]
    fn doc_window_metrics_are_pinned() {
        assert_eq!(DOC_WINDOW_CLIENT, (720.0, 440.0));
        assert_eq!(DOC_WINDOW_MIN, (520.0, 320.0));
        assert_eq!(DOC_MAX_WORK_FRACTION, 0.9);
        assert_eq!(LOG_VIEW_MAX_LINES, LOG_RING_CAPACITY);
        assert_eq!(LOG_VIEW_LINE_BYTES, 200);
    }

    /// **只追加**：`new == 0` ⇒ 一次写都不发；新增照算；超上限截头。
    #[test]
    fn plan_append_only_takes_the_tail() {
        // 无新增 ⇒ 0/0（"满环静置每秒 0 次写入"的结构面）
        assert_eq!(plan_append(100, 100, 2000), (0, 0));
        assert_eq!(plan_append(100, 100, 0), (0, 0));
        // 单轮新增
        assert_eq!(plan_append(100, 105, 10), (5, 0));
        // 追加后超视图上限 ⇒ 截头 = 超出量
        assert_eq!(plan_append(0, 2001, 0), (2000, 0));
        assert_eq!(plan_append(100, 200, 2000), (100, 100));
        // 单轮新增超过视图上限 ⇒ 先钳到上限（尾 2000 行），再按 rendered 截头
        assert_eq!(plan_append(0, 5000, 0), (2000, 0));
        assert_eq!(plan_append(0, 5000, 500), (2000, 500));
        // 反向读数（written 回退，理论上不该发生）⇒ saturating，不 panic 不下溢
        assert_eq!(plan_append(200, 100, 10), (0, 0));
    }

    /// **P2-①**：截头对回的前置条件 —— `first >= trim` 才有"同一内容"；`first < trim` 取 0（贴顶）。
    #[test]
    fn restore_line_after_trim_respects_the_precondition() {
        assert_eq!(restore_line_after_trim(1939, 1), 1938);
        assert_eq!(
            restore_line_after_trim(5, 5),
            0,
            "整除边界：同一内容正好落在首行"
        );
        assert_eq!(
            restore_line_after_trim(3, 7),
            0,
            "first < trim ⇒ 该内容已被截掉 ⇒ 贴顶"
        );
        assert_eq!(restore_line_after_trim(0, 0), 0);
        assert_eq!(restore_line_after_trim(0, 7), 0);
        assert_eq!(restore_line_after_trim(-3, 0), 0, "异常负值也不越界");
    }

    /// `follow` 的下一状态（纯函数）：贴底恢复 / 真的往上滚才暂停 / 否则保持。
    #[test]
    fn next_follow_is_sticky_against_the_initial_state() {
        // 贴底 ⇒ 一律恢复（C-3 的自动恢复臂）
        assert!(next_follow(false, true, false));
        assert!(next_follow(false, true, true));
        assert!(next_follow(true, true, false));
        // 未贴底 + 真的往上滚 ⇒ 暂停（用户意图）
        assert!(!next_follow(true, false, true));
        assert!(!next_follow(false, false, true));
        // 未贴底 + 没动过 ⇒ **保持原值**（"刚打开、视口还在顶部"不得被判成已暂停 ——
        // I2 真机实测：写成 `follow = at_bottom` 会让窗口永久停更）
        assert!(next_follow(true, false, false));
        assert!(!next_follow(false, false, false));
    }

    /// 状态行两条逐字串（判据 J-C3 依赖）。
    #[test]
    fn follow_status_text_is_verbatim() {
        assert_eq!(follow_status_text(true), "跟随最新");
        assert_eq!(follow_status_text(false), "已暂停跟随（滚到底部即恢复）");
        assert_eq!(STATUS_FOLLOWING, "跟随最新");
        assert_eq!(STATUS_PAUSED, "已暂停跟随（滚到底部即恢复）");
    }

    /// 贴底判定（C-3 的口径；含"内容比视口短"与"差一行"两个边界）。
    #[test]
    fn at_bottom_boundaries() {
        // 内容刚好一屏（nMax < nPage）⇒ 恒为贴底
        assert!(at_bottom(0, 5, 20));
        // nMax - nPage + 1 = 91 ⇒ nPos ≥ 91 才算贴底
        assert!(!at_bottom(90, 100, 10));
        assert!(at_bottom(91, 100, 10));
        assert!(at_bottom(95, 100, 10));
    }

    /// 视图分块：逐行按字节上限截断（字节安全）+ CRLF 分行 + **末尾不多余空行**。
    #[test]
    fn chunk_text_splits_with_crlf_and_clips_long_lines() {
        let lines = vec!["第一行".to_string(), "第二行".to_string()];
        assert_eq!(chunk_text(&lines, false), "第一行\r\n第二行");
        assert_eq!(chunk_text(&lines, true), "\r\n第一行\r\n第二行");
        assert_eq!(chunk_text(&[], false), "");
        assert_eq!(chunk_text(&[], true), "");
        // 超长行：截到 200 字节内且附省略号；不劈开 UTF-8 字符
        let long = "中".repeat(300);
        let chunk = chunk_text(&[long], false);
        assert!(chunk.ends_with('…'));
        assert!(chunk.len() - '…'.len_utf8() <= LOG_VIEW_LINE_BYTES);
        assert!(chunk.starts_with('中'));
        // 恰好 200 字节 ⇒ 原样（不加省略号）
        let exact = "a".repeat(LOG_VIEW_LINE_BYTES);
        assert_eq!(chunk_text(std::slice::from_ref(&exact), false), exact);
    }

    /// LF → CRLF 的偏移换算：换行各多一个 `\r`，偏移仍指向同一处。
    ///
    /// ⚠ 偏移口径 = **UTF-16 码元**（`EM_SETSEL`），不是 UTF-8 字节 ⇒ 本测试把 `str::find` 的**字节**
    /// 下标显式换算成字符下标（正文全 BMP ⇒ 字符下标 == 码元下标）。
    #[test]
    fn crlf_offsets_stay_on_the_same_heading() {
        let lf = "第一章 · X\n\n内容\n第二章 · Y\n\n更多\n";
        let crlf = lf.replace('\n', "\r\n");
        let lf_chars: Vec<char> = lf.chars().collect();
        let crlf_chars: Vec<char> = crlf.chars().collect();
        // `str::find` 给的是**字节**下标；偏移口径是**字符/码元** ⇒ 显式换算（正文全 BMP）
        let char_offset = |needle: &str| lf[..lf.find(needle).unwrap()].chars().count();
        for offset in [0usize, char_offset("第二章"), char_offset("内")] {
            let converted = crlf_offset(lf, offset);
            // 逐字符比到换行为止（换行本身在两份文本里占的宽度不同 ⇒ 不参与比较）
            for step in 0..8usize {
                let (Some(a), Some(b)) = (
                    lf_chars.get(offset + step),
                    crlf_chars.get(converted + step),
                ) else {
                    break;
                };
                if *a == '\n' || *b == '\n' {
                    break;
                }
                assert_eq!(
                    a, b,
                    "换算后的偏移必须仍指向同一处（offset={offset} → {converted}，step={step}）"
                );
            }
        }
        assert_eq!(crlf_offset(lf, 0), 0);
        assert_eq!(crlf_offset("", 0), 0);
        assert_eq!(crlf_offset("a\nb", 2), 3, "b 在 CRLF 文本里是第 3 个码元");
        assert_eq!(crlf_offset("a\nb", 3), 4, "末位偏移之后的换算也要单调");
    }

    /// 教程正文的章节偏移**已换算到 CRLF 口径**，8 章各就各位且落在行首。
    #[test]
    fn tutorial_crlf_chapters_point_at_headings() {
        let (body, chapters) = tutorial_crlf_text();
        assert_eq!(chapters.len(), 8);
        let units: Vec<u16> = body.encode_utf16().collect();
        for (index, (label, offset)) in chapters.iter().enumerate() {
            assert_eq!(*label, about_text::CHAPTER_LABELS[index]);
            let heading = about_text::chapter_heading(index);
            let expected: Vec<u16> = heading.encode_utf16().collect();
            assert_eq!(
                &units[*offset..*offset + expected.len()],
                &expected[..],
                "第 {} 章偏移 {offset} 处不是标题 {heading:?}",
                index + 1
            );
            assert!(
                *offset == 0 || units[*offset - 1] == '\n' as u16,
                "第 {} 章偏移必须落在行首",
                index + 1
            );
        }
        // 换行归一自证：每个 `\r` 必须跟 `\n`、每个 `\n` 必须紧跟 `\r`（逐字符 + 失败给上下文）
        if !crlf_only(&body) {
            let chars: Vec<char> = body.chars().collect();
            let position = chars
                .iter()
                .enumerate()
                .find(|(index, ch)| {
                    (**ch == '\r' && chars.get(index + 1) != Some(&'\n'))
                        || (**ch == '\n' && (*index == 0 || chars.get(index - 1) != Some(&'\r')))
                })
                .map(|(index, _)| index)
                .unwrap_or(0);
            let from = position.saturating_sub(60);
            let to = (position + 60).min(chars.len());
            let context: String = chars[from..to].iter().collect();
            panic!("换行未归一到 CRLF：首个可疑位置 {position}；上下文 = {context:?}");
        }
        // 负例（[E13] ③）：判定函数必须能红 —— 裸 LF / 裸 CR / 重复 CR 都要被判假
        assert!(!crlf_only("a\nb"), "裸 LF 必须被判红");
        assert!(!crlf_only("a\rb"), "裸 CR 必须被判红");
        assert!(!crlf_only("a\r\r\nb"), "重复 CR 必须被判红");
        assert!(crlf_only("a\r\nb"), "纯 CRLF 必须被判真");
    }

    /// 布局纯函数：全部子矩形 ⊂ 客户区 ∧ 纵向不重叠 ∧ 章节条 8 段铺满内容宽。
    #[test]
    fn layout_stays_inside_the_client_and_does_not_overlap() {
        for scale in [1.0f32, 1.25, 1.5, 2.0] {
            for (width, height) in [(720, 440), (520, 320), (1400, 900)] {
                let client_w = (width as f32 * scale).round() as i32;
                let client_h = (height as f32 * scale).round() as i32;
                let l = layout(scale, client_w, client_h);
                let rects = [
                    ("title", l.title),
                    ("subtitle", l.subtitle),
                    ("action0", l.actions[0]),
                    ("action1", l.actions[1]),
                    ("chapter_bar", l.chapter_bar),
                    ("edit", l.edit),
                    ("footer", l.footer),
                    ("status", l.status),
                ];
                for (name, rect) in rects {
                    assert!(rect.w > 0 && rect.h > 0, "{name} 必须非空");
                    assert!(
                        rect.x >= 0
                            && rect.y >= 0
                            && rect.right() <= client_w
                            && rect.bottom() <= client_h,
                        "{name} 越出客户区：{rect:?}（客户区 {client_w}×{client_h}）"
                    );
                }
                assert!(l.subtitle.bottom() <= l.chapter_bar.y, "副标题不得压到子行");
                assert!(l.actions[0].bottom() <= l.chapter_bar.y, "按钮不得压到子行");
                assert_eq!(l.edit.y, l.chapter_bar.bottom(), "正文紧贴子行下缘");
                assert_eq!(l.edit.bottom(), l.footer.y, "正文下缘 = 注脚上缘");
                assert!(l.status.y >= l.footer.y && l.status.bottom() <= client_h);
                assert_eq!(
                    l.actions[1].right(),
                    l.title.right(),
                    "末个按钮右缘对齐内容右缘"
                );
                assert!(l.actions[0].right() <= l.actions[1].x, "两个按钮不得重叠");
                // 章节条 8 段铺满 + 不重叠
                let mut cursor = l.chapter_bar.x;
                for index in 0..about_text::CHAPTER_LABELS.len() {
                    let segment = l.chapter_segment(index);
                    assert_eq!(segment.x, cursor, "第 {index} 段必须紧接上一段");
                    assert!(segment.w > 0);
                    cursor = segment.right();
                }
                assert_eq!(cursor, l.chapter_bar.right(), "8 段必须铺满章节条");
            }
        }
    }

    /// 章节条命中测试：段内命中、段间边界（左闭右开）、条外不命中。
    #[test]
    fn chapter_hit_testing_covers_every_segment() {
        let l = layout(1.0, 720, 440);
        for index in 0..8 {
            let segment = l.chapter_segment(index);
            assert_eq!(
                l.chapter_at(segment.x + 1, segment.y + 1),
                Some(index),
                "第 {index} 段内点必须命中该段"
            );
            assert_eq!(
                l.chapter_at(segment.right(), segment.y + 1),
                // 右缘属下一段（左闭右开）；**末段右缘 = 章节条右缘 ⇒ 条外 ⇒ None**
                if index == 7 { None } else { Some(index + 1) },
                "右边界属于下一段（左闭右开）"
            );
        }
        assert_eq!(
            l.chapter_at(l.chapter_bar.x + 1, l.chapter_bar.y - 5),
            None,
            "条上方不命中"
        );
        assert_eq!(
            l.chapter_at(l.chapter_bar.x + 1, l.chapter_bar.bottom() + 5),
            None,
            "条下方不命中"
        );
    }

    /// 模式文案（标题 / 副标题 / 按钮标签 —— 三处都是用户可见串）。
    #[test]
    fn mode_strings_are_complete() {
        for mode in [DocMode::Tutorial, DocMode::Log] {
            assert!(mode_title(mode).contains("AzusaAI 本地反向代理"));
            assert!(!mode_subtitle(mode).is_empty());
            assert!(!mode_copy_label(mode).is_empty());
        }
        assert!(mode_title(DocMode::Tutorial).starts_with("使用教程"));
        assert!(mode_title(DocMode::Log).starts_with("日志"));
        assert_eq!(mode_copy_label(DocMode::Tutorial), "复制全文");
        assert_eq!(mode_copy_label(DocMode::Log), "复制全部");
        assert!(mode_subtitle(DocMode::Log).contains("内存"));
    }

    /// 读数行文案与设置窗**同源**（`log_stat_text`）—— 文案漂移会在这里红。
    #[test]
    fn readout_uses_the_shared_stat_text() {
        assert_eq!(
            log_stat_text(2000, LOG_RING_CAPACITY, 99),
            "共 2000 / 上限 2000 条（已挤出 99）"
        );
    }
}
