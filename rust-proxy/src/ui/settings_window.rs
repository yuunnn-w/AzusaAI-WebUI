//! 设置小窗（P7 I1 改造）：**行表布局（`plan()` 纯函数）** + 自绘分段控件 / 复选框 / 按钮 +
//! 三态结果行 + 失败提示条。
//!
//! 口径（P6-S4 去配置文件后的新语义，方案 §3.2）：
//! - **[应用]** = 校验 → **仅内存生效**（`Controller::apply`）→ 结果行提示；**不写任何文件**
//!   （程序没有配置文件；改动的生命周期 = 本次运行，关掉程序即回到默认/命令行值）。
//!   失败在新配置全端口绑不上时由服务侧**回滚到旧配置**（仅内存，见 `service.rs`），
//!   结果回填到本窗结果行；
//! - **没有"以文件为准"**：FR-38 的打开时读盘热生效随去配置文件一并删除；
//! - `[测试上游]` 是**唯一**的主动探测（由用户触发，FR-8）：GET `{upstream}/v1/models`，显示状态码与耗时。
//!
//! I1 的四个结构要点（判据都钉在它们上）：
//! 1. **布局唯一真源 = [`plan()`]**（纯函数：`scale × advanced × notice × 可用高度` → 客户区 + 控件表）；
//!    创建、重排（高级展开 / DPI 变化 / 提示条显隐）与单测（A-3）读同一份；
//! 2. **裁切修复**：所有单行 `STATIC` 带局部 `const SS_LEFTNOWORDWRAP`（横裁而非折行）；日志读数行
//!    独立满行 + 文案走 [`crate::ui::log_stat_text`]（与 I2 的日志窗共用）+ `fit_text` 量宽收口；
//! 3. **分段控件 / 复选框 / 按钮全部自绘**（`BS_OWNERDRAW` + 父窗 `WM_DRAWITEM`；状态自持 ——
//!    **不读 `BM_GETCHECK` / `CB_GETCURSEL`**）；EDIT 补 `WS_TABSTOP`（既有缺陷：键盘 Tab 进不去输入框）；
//! 4. **结果行三态 + 失败提示条**归 A 组行为面：文本来自 `Status.apply_message`，墨色读
//!    `Status.apply_stage`（`Failed` ⇒ `✗` + 红 + 3 DIP 竖条与顶部提示条）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, GetDC,
    InvalidateRect, ReleaseDC, SetBkColor, SetBkMode, SetTextColor, DT_CENTER, DT_LEFT,
    DT_SINGLELINE, DT_VCENTER, HDC, HFONT, HGDIOBJ, PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::UI::Controls::{
    DRAWITEMSTRUCT, ODS_DISABLED, ODS_FOCUS, ODS_SELECTED, ODT_BUTTON, ODT_STATIC,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetDlgItem,
    GetSystemMetrics, GetWindowLongPtrW, SendMessageW, SetForegroundWindow, SetWindowLongPtrW,
    SetWindowPos, SetWindowTextW, ShowWindow, SystemParametersInfoW, BS_OWNERDRAW, CREATESTRUCTW,
    CW_USEDEFAULT, ES_AUTOHSCROLL, ES_NUMBER, GWLP_USERDATA, HMENU, SM_CYCAPTION, SM_CYSIZEFRAME,
    SPI_GETWORKAREA, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_HIDE, SW_SHOW,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_COMMAND,
    WM_CREATE, WM_CTLCOLORBTN, WM_CTLCOLOREDIT, WM_CTLCOLORSTATIC, WM_DESTROY, WM_DPICHANGED,
    WM_DRAWITEM, WM_ERASEBKGND, WM_NCCREATE, WM_NCDESTROY, WM_PAINT, WM_SETFONT, WNDCLASSW,
    WS_CAPTION, WS_CHILD, WS_CLIPCHILDREN, WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
};

use azusa_local_proxy::logging::{Logger, LOG_RING_CAPACITY};
use azusa_local_proxy::service::{ApplyStage, APPLY_START_TEXT};

use super::theme::{
    fit_text, gdi_color, measure_text, wide, Gfx, BTN_H, COLOR_ACCENT_DEEP, COLOR_ACCENT_PRESSED,
    COLOR_ACCENT_RING, COLOR_ACCENT_SOFT, COLOR_BG, COLOR_CARD, COLOR_CARD_BORDER,
    COLOR_DANGER_INK, COLOR_INK, COLOR_INK_DISABLED, COLOR_INK_SOFT, COLOR_NOTICE_BG,
    COLOR_NOTICE_BORDER, COLOR_NOTICE_INK, COLOR_PRESS_ON_CARD, RADIUS_BUTTON, RADIUS_CARD,
    RADIUS_FIELD, ROW_H, ROW_STEP, SPACE_M, SPACE_S, SPACE_XL, SPACE_XS, SPACE_XXL,
};
use super::{log_stat_text, UiState, SETTINGS_WINDOW_CLASS, WM_APP_TEST_RESULT};

/// 单行 `STATIC` 不折行样式位。
///
/// `SS_LEFTNOWORDWRAP` 属 `Win32::System::SystemServices`（**该 feature 未启用**；D4 零依赖替代）
/// ⇒ 局部常量：值现取 `windows-0.62.2` 的 `STATIC_STYLES(12)`，与 Win32 头文件同值。
/// 折行 + 定高是"半截字"的唯一成因（P7 诉求 ①）。
const SS_LEFTNOWORDWRAP: u32 = 0x000C;
/// 自绘 `STATIC`（结果行 / 失败提示条由父窗 `WM_DRAWITEM` 画；crate = `STATIC_STYLES(13)`）。
const SS_OWNERDRAW: u32 = 0x000D;

/// 设置窗控件 ID（区间 3001–3099）。
const IDC_FIELD_UPSTREAM: usize = 3001;
const IDC_FIELD_ADVANCED: usize = 3002;
const IDC_FIELD_STRIP: usize = 3003;
const IDC_FIELD_PORT: usize = 3004;
const IDC_FIELD_FALLBACK: usize = 3005;
const IDC_FIELD_CONNECT: usize = 3006;
const IDC_FIELD_FIRST_BYTE: usize = 3007;
const IDC_FIELD_CORS_MAX_AGE: usize = 3009;
/// 只读读数行：「日志（内存）」的 条数 / 上限 / 已挤出（P6-S5 / D12；P7 起独立满行 + 不折行）。
const IDC_FIELD_LOG_STAT: usize = 3011;
/// 「查看日志」（I2 起改为文档窗日志模式；本批保持既有入口）。
const IDC_VIEW_LOGS: usize = 3012;
const IDC_RESULT: usize = 3016;
const IDC_TEST: usize = 3017;
const IDC_APPLY: usize = 3018;
/// 「复制日志到剪贴板」（P6-S5 / D12：留存日志的唯一出口，不落盘）。
const IDC_COPY_LOGS: usize = 3019;
/// 「响应头超时(ms)」新字段（P7 §2.6.1 行 5；单位 = ms，与 `--help`/config 同单位）。
const IDC_FIELD_RESPONSE_HEAD: usize = 3020;
/// 分段控件（3 组；**每组一段一个控件 ID**，状态自持在 `UiState.draft_*`）。
const IDC_SEG_CORS: [usize; 2] = [3021, 3022];
const IDC_SEG_LOG_LEVEL: [usize; 4] = [3023, 3024, 3025, 3026];
const IDC_SEG_CLOSE: [usize; 2] = [3027, 3028];
/// 「重新载入」（把当前内存配置重新填进字段，丢弃未提交的编辑）。
const IDC_RELOAD: usize = 3029;
/// 失败提示条（满行，仅 `Failed` 时出现）+ 其内的「查看日志」。
const IDC_NOTICE: usize = 3030;
const IDC_NOTICE_LOGS: usize = 3031;
/// 连接池两项（P7 §2.6.1 行 8；**仅高级展开**可见）。
const IDC_FIELD_POOL_MAX: usize = 3032;
const IDC_FIELD_POOL_IDLE: usize = 3033;

/// 标签类 `STATIC` 的 ID（2901–2915；**不属于** 3001+ 的动作区间 ⇒ 不会进 `WM_COMMAND` 分支）。
const IDC_LABEL_UPSTREAM: usize = 2901;
const IDC_LABEL_STRIP: usize = 2902;
const IDC_LABEL_PORT: usize = 2903;
const IDC_LABEL_FALLBACK: usize = 2904;
const IDC_LABEL_CONNECT: usize = 2905;
const IDC_LABEL_RESPONSE_HEAD: usize = 2906;
const IDC_LABEL_FIRST_BYTE: usize = 2907;
const IDC_LABEL_LOG_LEVEL: usize = 2908;
const IDC_LABEL_CORS: usize = 2909;
const IDC_LABEL_MAX_AGE: usize = 2910;
const IDC_LABEL_POOL_MAX: usize = 2911;
const IDC_LABEL_POOL_IDLE: usize = 2912;
const IDC_LABEL_LOG_STAT: usize = 2913;
const IDC_LABEL_CLOSE: usize = 2914;
const IDC_LABEL_CLOSE_NOTE: usize = 2915;

/// 「测试上游」的结果槽（工作线程写 → `WM_APP_TEST_RESULT` → UI 线程读）。
static TEST_RESULT: Mutex<Option<String>> = Mutex::new(None);
/// 读数行超宽的告警闸：**只 warn 一次**（P3-1：不用 `debug_assert!` —— win7 release 档 `panic = "abort"`）。
static LOG_STAT_WIDTH_WARNED: AtomicBool = AtomicBool::new(false);

// ── 布局常量（DIP；`plan()` 的全部输入语义见其文档）────────────────────────────────────────────

/// **列常量表（P7 布局修复轮 · 唯一真源）** —— 所有行的 x 都从这里取；
/// `plan()` 里**禁止**出现"上一个控件右缘 + gap"式的链式推导（用户报障 L1 的根因）。
///
/// 列定义（DIP）：
///
/// ```text
///   PAD_LEFT 20 │ 列 A 标签 132 │ 列 A 控件 196 │ 列 B 标签 120 │ 列 B 控件 108 │ PAD_RIGHT 20
///   20          152            356             484            600           620
/// ```
///
/// **客户区宽 620 DIP 的算法（L7 的"哪一行决定"读数）**：
/// 决定项 = 行 6「日志级别」的 **4 段分段控件**（宽段标签 `debug` 实测 43 DIP @13 DIP/600 ⇒
/// 段宽 ≥ 47 ⇒ 槽宽 ≥ 4×47 + 3×1 = 191，取整到 196）；右列决定项 = 标签 `预热 Max-Age(秒)`
/// 实测 113 DIP（⇒ 列 B 标签 120）+ 数字井 `600000` 45 DIP（⇒ 列 B 控件 108）。
/// ⇒ 20 + 132 + 8 + 196 + 8 + 120 + 8 + 108 + 20 = **620**。
const PAD_LEFT: f32 = SPACE_XL;
const PAD_RIGHT: f32 = SPACE_XL;
/// 列 A 标签（行首标签）。
const COL_LABEL_A_X: f32 = PAD_LEFT;
const COL_LABEL_A_W: f32 = 132.0;
/// 列 A 控件（数字井 / 分段控件 / 复选框 / 左侧动作按钮）。
const COL_CTRL_A_X: f32 = 160.0;
const COL_CTRL_A_W: f32 = 196.0;
/// 列间隙（列与列之间统一 8 DIP；列 B 的两条 x 都由它推出）。
const COL_GAP: f32 = SPACE_S;
/// 列 B 标签（右列标签 / **字段行内提示**；L4：提示不得占列 B 控件位）。
const COL_LABEL_B_X: f32 = COL_CTRL_A_X + COL_CTRL_A_W + COL_GAP;
const COL_LABEL_B_W: f32 = 120.0;
/// 列 B 控件（右列数字井）。
const COL_CTRL_B_X: f32 = COL_LABEL_B_X + COL_LABEL_B_W + COL_GAP;
const COL_CTRL_B_W: f32 = 108.0;
/// 设置窗客户区宽 = 列 B 控件右缘 + 右边距。
const CLIENT_W_DIP: f32 = COL_CTRL_B_X + COL_CTRL_B_W + PAD_RIGHT;
/// **整行字段**（L6：上游 / 剥离前缀 / 日志读数行 / 结果行）—— 左缘对齐列 A 控件、右缘留 `PAD_RIGHT`。
const COL_FULL_W: f32 = CLIENT_W_DIP - COL_CTRL_A_X - PAD_RIGHT;
/// 列 A 数字井（同列同类控件统一宽）。
const NUM_A_W: f32 = 96.0;
/// 动作按钮宽（同类统一；左对齐 `COL_CTRL_A_X` 或右对齐 `PAD_RIGHT`）。
const BTN_W: f32 = 120.0;
/// 动作按钮间隙。
const BTN_GAP: f32 = SPACE_S;
/// 屏幕放不下的退化行步（§2.6.1：「行步 30→26」）。
const STEP_COMPACT_DIP: f32 = 26.0;
/// 内容起点与底部余量（结构判据：**内容底余量恒 24 DIP**）。
const TOP_DIP: f32 = SPACE_M;
const BOTTOM_PAD_DIP: f32 = SPACE_XXL;
/// 结果行高（**两行**，P2-2 容量）与失败提示条高。
const RESULT_H_DIP: f32 = 36.0;
const NOTICE_H_DIP: f32 = 28.0;
/// 顶部动作行（结果行 → 底部按钮行）的**组距** = 2 × 行步（跳过一格 ⇒ 行 y 仍在同一条等差格上）。
const ACTION_GROUP_SKIP: usize = 1;
/// EDIT 在自绘"输入井"里的内缩（§2.3 输入井：EDIT 内缩 2 DIP）。
const WELL_INSET_DIP: f32 = 2.0;
/// 行表行索引（顺序 = 视觉顺序 = Tab 顺序；**每行占一个行步格**）。
const ROW_UPSTREAM: usize = 0;
const ROW_ADVANCED: usize = 1;
const ROW_STRIP: usize = 2;
const ROW_LISTEN: usize = 3;
const ROW_TIMEOUTS: usize = 4;
const ROW_FIRST_BYTE_LEVEL: usize = 5;
const ROW_CORS: usize = 6;
const ROW_POOL: usize = 7;
const ROW_LOG_STAT: usize = 8;
const ROW_LOG_BUTTONS: usize = 9;
const ROW_CLOSE_ACTION: usize = 10;
const ROW_RESULT: usize = 11;
const ROW_BUTTONS: usize = 12;
/// 失败提示条（**顶部**；仅 `Failed` 时占位 ⇒ 行表整体下移一行步）。
const ROW_NOTICE: usize = 13;
const ROW_COUNT: usize = 14;

/// 结果行墨色（三态；P7 §2.6.6）。三个 owner（本地校验 / 服务端 apply / 测试上游）各自设置。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ResultTone {
    /// 普通（`COLOR_INK_SOFT`）。
    #[default]
    Plain,
    /// 成功（`✓` + `COLOR_INK`）。
    Ok,
    /// 失败（`✗` + `COLOR_DANGER_INK` + 左侧 3 DIP 竖条）。
    Error,
}

/// 控件种类（决定创建用的窗口类 / 样式与自绘分档）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanKind {
    /// 单行标签（`SS_LEFTNOWORDWRAP`）。
    Label,
    /// 日志读数行（单行；文本在运行期由 [`log_stat_text`] 填，⇒ `plan()` 里文本为空）。
    LogStat,
    /// 结果行（两行；`SS_OWNERDRAW` 三态自绘）。
    Result,
    /// 失败提示条（满行；`SS_OWNERDRAW` 自绘）。
    Notice,
    /// 普通输入井（`EDIT`，去 `WS_BORDER`，补 `WS_TABSTOP`）。
    Edit,
    /// 数字输入井（`EDIT` + `ES_NUMBER`）。
    NumberEdit,
    /// 自绘复选框。
    Checkbox,
    /// 自绘分段控件成员。
    Segment,
    /// 自绘主按钮（樱花底 + 白字）。
    ButtonPrimary,
    /// 自绘次按钮（卡片底 + 描边）。
    ButtonSecondary,
}

/// 行表的一个控件（`plan()` 的输出项；坐标为**像素**，已过 `Theme::px` 口径的 `round`）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlanItem {
    pub id: usize,
    pub kind: PlanKind,
    /// 显示文本（动态文本的控件为空串：读数行 / 结果行 / 提示条）。
    pub text: &'static str,
    /// 所属列（A-3b 的机械判据依据：同列同 x / 同类同宽）。
    pub column: Column,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    /// 当前状态下是否应显示（高级折叠 / 提示条消失时隐藏，但仍参与"位置已定"）。
    pub visible: bool,
}

/// 一次布局的完整结果（客户区 + 控件表 + 档位读数）。
#[derive(Clone, Debug, PartialEq)]
pub struct SettingsPlan {
    pub scale: f32,
    pub compact: bool,
    /// 是否触发"连接池组强制折叠"（第三档退化；`advanced == false` 时无行可折叠 ⇒ 恒 `false`）。
    pub pool_folded: bool,
    pub client_w: i32,
    pub client_h: i32,
    /// 最后一个可见行的底（客户区高 − 它 = 底部余量，结构判据用）。
    pub content_bottom: i32,
    pub items: Vec<PlanItem>,
}

/// 行表原始项（DIP；`plan()` 的输入表）。
///
/// **列定位全部走 [`Column`]**（列常量表）—— 这里**没有**"上一个控件右缘 + gap"的字段，
/// 结构上就禁止了链式推导（用户报障 L1 的根因）。
struct Spec {
    id: usize,
    kind: PlanKind,
    text: &'static str,
    row: usize,
    column: Column,
    /// 宽度覆盖（DIP）；`None` = 取列宽。
    w_override: Option<f32>,
    /// 段内横向偏移（DIP；分段成员 / 动作按钮对 / 提示条按钮）。
    dx: f32,
    h: f32,
}

/// 列定义（唯一真源 = 上面的列常量表）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    /// 列 A 标签（行首标签）。
    LabelA,
    /// 列 A 控件（数字井 / 分段控件 / 复选框 / 左侧动作按钮）。
    CtrlA,
    /// 列 B 标签（右列标签 / **字段行内提示**）。
    LabelB,
    /// 列 B 控件（右列数字井）。
    CtrlB,
    /// 整行字段 / 结果行（左缘 = 列 A 控件位，右缘 = `PAD_RIGHT`）。
    FullRow,
    /// 表内动作行（左对齐列 A 控件位；同类同宽）。
    ActionLeft,
    /// 底部动作行（右对齐 `PAD_RIGHT`；同类同宽）。
    ActionRight,
    /// 结果行（**页边距整行**：左缘 `PAD_LEFT`、右缘 `PAD_RIGHT`；无标签行 ⇒ 容量优先，A-15）。
    ResultRow,
    /// 失败提示条（整行减去提示条按钮位）。
    NoticeStrip,
    /// 失败提示条里的按钮（右缘 = 客户区右缘）。
    NoticeButton,
}

const fn spec(
    id: usize,
    kind: PlanKind,
    text: &'static str,
    row: usize,
    column: Column,
    h: f32,
) -> Spec {
    Spec {
        id,
        kind,
        text,
        row,
        column,
        w_override: None,
        dx: 0.0,
        h,
    }
}

const fn spec_w(
    id: usize,
    kind: PlanKind,
    text: &'static str,
    row: usize,
    column: Column,
    w: f32,
    h: f32,
) -> Spec {
    Spec {
        id,
        kind,
        text,
        row,
        column,
        w_override: Some(w),
        dx: 0.0,
        h,
    }
}

/// 行表（§2.6.1 的 13 行；**唯一真源**：创建 / 重排 / 单测都读它）。
///
/// **整行字段**（L6，明确登记）：`上游 Base URL` · `剥离前缀`（高级） · `日志（内存）` 读数 · 结果行
/// —— 它们跨列 A+B，左右内边距与其它行一致（左缘 = 列 A 控件位，右缘 = `PAD_RIGHT`）。
///
/// **字段顺序说明（L2 的代价，已登记）**：行 5 的「日志级别」与「首字节(0=不限)」左右互换 ——
/// 4 段分段控件需要列 A 槽宽（191 DIP 机械下限），列 B 槽宽（108）放不下；其余行顺序不变。
const SPECS: &[Spec] = &[
    // 行 0：上游（整行字段）
    spec(
        IDC_LABEL_UPSTREAM,
        PlanKind::Label,
        "上游 Base URL",
        ROW_UPSTREAM,
        Column::LabelA,
        ROW_H,
    ),
    spec(
        IDC_FIELD_UPSTREAM,
        PlanKind::Edit,
        "",
        ROW_UPSTREAM,
        Column::FullRow,
        ROW_H,
    ),
    // 行 1：高级选项（自绘复选框；L5：标签不再提"剥离前缀"）
    spec(
        IDC_FIELD_ADVANCED,
        PlanKind::Checkbox,
        "高级选项",
        ROW_ADVANCED,
        // P3-②（最终验收）：原 `Column::CtrlA` —— 复选框挤在 URL 输入框正下方、同一条左对齐轴上，
        // 读起来像「URL 行的子项」。改到**右列起点**（`Column::LabelB`，x = 第二列的左缘）：
        // ① 只动列、不动行表 ⇒ 级联为零（不触碰 I1 已定的 620×398 / "内容底余量 24 DIP"等数字）；
        // ② 视觉上离开 URL 输入框的左轴 ⇒ 归属一眼明确；③ 仍在自己的一行上 ⇒ A-3b 三条全绿。
        Column::LabelB,
        ROW_H,
    ),
    // 行 2：剥离前缀（整行字段，仅高级展开）
    spec(
        IDC_LABEL_STRIP,
        PlanKind::Label,
        "剥离前缀",
        ROW_STRIP,
        Column::LabelA,
        ROW_H,
    ),
    spec(
        IDC_FIELD_STRIP,
        PlanKind::Edit,
        "",
        ROW_STRIP,
        Column::FullRow,
        ROW_H,
    ),
    // 行 3：监听端口 / 候选端口
    spec(
        IDC_LABEL_PORT,
        PlanKind::Label,
        "监听端口",
        ROW_LISTEN,
        Column::LabelA,
        ROW_H,
    ),
    spec_w(
        IDC_FIELD_PORT,
        PlanKind::NumberEdit,
        "",
        ROW_LISTEN,
        Column::CtrlA,
        NUM_A_W,
        ROW_H,
    ),
    spec(
        IDC_LABEL_FALLBACK,
        PlanKind::Label,
        "候选端口",
        ROW_LISTEN,
        Column::LabelB,
        ROW_H,
    ),
    spec(
        IDC_FIELD_FALLBACK,
        PlanKind::Edit,
        "",
        ROW_LISTEN,
        Column::CtrlB,
        ROW_H,
    ),
    // 行 4：连接超时(秒) / 响应头超时(ms)
    spec(
        IDC_LABEL_CONNECT,
        PlanKind::Label,
        "连接超时(秒)",
        ROW_TIMEOUTS,
        Column::LabelA,
        ROW_H,
    ),
    spec_w(
        IDC_FIELD_CONNECT,
        PlanKind::NumberEdit,
        "",
        ROW_TIMEOUTS,
        Column::CtrlA,
        NUM_A_W,
        ROW_H,
    ),
    spec(
        IDC_LABEL_RESPONSE_HEAD,
        PlanKind::Label,
        "响应头超时(ms)",
        ROW_TIMEOUTS,
        Column::LabelB,
        ROW_H,
    ),
    spec(
        IDC_FIELD_RESPONSE_HEAD,
        PlanKind::NumberEdit,
        "",
        ROW_TIMEOUTS,
        Column::CtrlB,
        ROW_H,
    ),
    // 行 5：日志级别（分段在列 A）/ 首字节(0=不限)
    spec(
        IDC_LABEL_LOG_LEVEL,
        PlanKind::Label,
        "日志级别",
        ROW_FIRST_BYTE_LEVEL,
        Column::LabelA,
        ROW_H,
    ),
    spec(
        IDC_LABEL_FIRST_BYTE,
        PlanKind::Label,
        "首字节(0=不限)",
        ROW_FIRST_BYTE_LEVEL,
        Column::LabelB,
        ROW_H,
    ),
    spec(
        IDC_FIELD_FIRST_BYTE,
        PlanKind::NumberEdit,
        "",
        ROW_FIRST_BYTE_LEVEL,
        Column::CtrlB,
        ROW_H,
    ),
    // 行 6：Allow-Origin 模式（分段在列 A）/ 预热 Max-Age(秒)
    spec(
        IDC_LABEL_CORS,
        PlanKind::Label,
        "Allow-Origin 模式",
        ROW_CORS,
        Column::LabelA,
        ROW_H,
    ),
    spec(
        IDC_LABEL_MAX_AGE,
        PlanKind::Label,
        "预热 Max-Age(秒)",
        ROW_CORS,
        Column::LabelB,
        ROW_H,
    ),
    spec(
        IDC_FIELD_CORS_MAX_AGE,
        PlanKind::NumberEdit,
        "",
        ROW_CORS,
        Column::CtrlB,
        ROW_H,
    ),
    // 行 7：池最大空闲/主机 / 池空闲超时(秒)（仅高级展开）
    spec(
        IDC_LABEL_POOL_MAX,
        PlanKind::Label,
        "池最大空闲/主机",
        ROW_POOL,
        Column::LabelA,
        ROW_H,
    ),
    spec_w(
        IDC_FIELD_POOL_MAX,
        PlanKind::NumberEdit,
        "",
        ROW_POOL,
        Column::CtrlA,
        NUM_A_W,
        ROW_H,
    ),
    spec(
        IDC_LABEL_POOL_IDLE,
        PlanKind::Label,
        "池空闲超时(秒)",
        ROW_POOL,
        Column::LabelB,
        ROW_H,
    ),
    spec(
        IDC_FIELD_POOL_IDLE,
        PlanKind::NumberEdit,
        "",
        ROW_POOL,
        Column::CtrlB,
        ROW_H,
    ),
    // 行 8：日志（内存）读数（整行字段）
    spec(
        IDC_LABEL_LOG_STAT,
        PlanKind::Label,
        "日志（内存）",
        ROW_LOG_STAT,
        Column::LabelA,
        ROW_H,
    ),
    spec(
        IDC_FIELD_LOG_STAT,
        PlanKind::LogStat,
        "",
        ROW_LOG_STAT,
        Column::FullRow,
        ROW_H,
    ),
    // 行 9：日志动作按钮（左对齐列 A 控件位；同类同宽 —— L3）
    spec(
        IDC_VIEW_LOGS,
        PlanKind::ButtonSecondary,
        "查看日志",
        ROW_LOG_BUTTONS,
        Column::ActionLeft,
        ROW_H,
    ),
    spec(
        IDC_COPY_LOGS,
        PlanKind::ButtonSecondary,
        "复制到剪贴板",
        ROW_LOG_BUTTONS,
        Column::ActionLeft,
        ROW_H,
    ),
    // 行 10：关闭主窗时（分段在列 A）+ 行内提示（列 B **标签**位 —— L4）
    spec(
        IDC_LABEL_CLOSE,
        PlanKind::Label,
        "关闭主窗时",
        ROW_CLOSE_ACTION,
        Column::LabelA,
        ROW_H,
    ),
    spec(
        IDC_LABEL_CLOSE_NOTE,
        PlanKind::Label,
        "（仅本次运行）",
        ROW_CLOSE_ACTION,
        Column::LabelB,
        ROW_H,
    ),
    // 行 11：结果行（整行 / 两行高）
    spec(
        IDC_RESULT,
        PlanKind::Result,
        "",
        ROW_RESULT,
        Column::ResultRow,
        RESULT_H_DIP,
    ),
    // 行 12：底部动作行（右对齐 PAD_RIGHT；同类同宽）
    spec(
        IDC_RELOAD,
        PlanKind::ButtonSecondary,
        "重新载入",
        ROW_BUTTONS,
        Column::ActionRight,
        BTN_H,
    ),
    spec(
        IDC_TEST,
        PlanKind::ButtonSecondary,
        "测试上游",
        ROW_BUTTONS,
        Column::ActionRight,
        BTN_H,
    ),
    spec(
        IDC_APPLY,
        PlanKind::ButtonPrimary,
        "应用",
        ROW_BUTTONS,
        Column::ActionRight,
        BTN_H,
    ),
];

/// 分段控件组（**统一策略：全部占所在列的槽宽，组内均分 + 1 DIP 间隙**；L2）。
struct SegmentGroup {
    row: usize,
    ids: &'static [usize],
    texts: &'static [&'static str],
}

const SEGMENT_GROUPS: &[SegmentGroup] = &[
    SegmentGroup {
        row: ROW_FIRST_BYTE_LEVEL,
        ids: &IDC_SEG_LOG_LEVEL,
        texts: &["error", "warn", "info", "debug"],
    },
    SegmentGroup {
        row: ROW_CORS,
        ids: &IDC_SEG_CORS,
        texts: &["*（通配）", "回显 Origin"],
    },
    SegmentGroup {
        row: ROW_CLOSE_ACTION,
        ids: &IDC_SEG_CLOSE,
        texts: &["隐藏到托盘", "直接退出"],
    },
];

/// 分段控件成员总数（`SEGMENT_GROUPS` 的 id 之和；const 上下文里算，避免手抄）。
const fn segment_member_count() -> usize {
    let mut total = 0_usize;
    let mut index = 0_usize;
    while index < SEGMENT_GROUPS.len() {
        total += SEGMENT_GROUPS[index].ids.len();
        index += 1;
    }
    total
}

/// `plan()` 的项数（**创建顺序 = plan 顺序** ⇒ `SettingsControls::all` 必须按它定长；
/// 短一格会让尾部控件收不到 `SetWindowPos`/`ShowWindow` —— 会表现为"停不掉的提示条"）。
const PLAN_ITEM_COUNT: usize = SPECS.len() + segment_member_count() + NOTICE_SPECS.len();

/// 提示条行（失败时占一行）：条在整行字段位（**减掉按钮位**），按钮右缘 = 客户区右缘 ⇒ 两者不重叠。
const NOTICE_SPECS: &[Spec] = &[
    Spec {
        id: IDC_NOTICE,
        kind: PlanKind::Notice,
        text: "",
        row: ROW_NOTICE,
        column: Column::NoticeStrip,
        w_override: None,
        dx: 0.0,
        h: NOTICE_H_DIP,
    },
    Spec {
        id: IDC_NOTICE_LOGS,
        kind: PlanKind::ButtonSecondary,
        text: "查看日志",
        row: ROW_NOTICE,
        column: Column::NoticeButton,
        w_override: None,
        dx: 0.0,
        h: 22.0,
    },
];

/// 行的可见性（唯一真源：`plan()` 与单测都读它）。提示条行单独判（`match entry.row`）。
fn row_visible(row: usize, advanced: bool, pool_folded: bool) -> bool {
    match row {
        ROW_STRIP => advanced,
        ROW_POOL => advanced && !pool_folded,
        _ => true,
    }
}

/// 行的最大高（DIP；行内控件可更低）。
fn row_height(row: usize) -> f32 {
    let mut height = ROW_H;
    for entry in SPECS.iter().chain(NOTICE_SPECS.iter()) {
        if entry.row == row && entry.h > height {
            height = entry.h;
        }
    }
    height
}

/// 列宽解析（`dx` 之外的全部横向定位都在这里；**不读任何"上一个控件"**）。
fn column_layout(column: Column, w_override: Option<f32>) -> (f32, f32) {
    match column {
        Column::LabelA => (COL_LABEL_A_X, COL_LABEL_A_W),
        Column::CtrlA => (COL_CTRL_A_X, w_override.unwrap_or(COL_CTRL_A_W)),
        Column::LabelB => (COL_LABEL_B_X, COL_LABEL_B_W),
        Column::CtrlB => (COL_CTRL_B_X, w_override.unwrap_or(COL_CTRL_B_W)),
        Column::FullRow => (COL_CTRL_A_X, COL_FULL_W),
        Column::ResultRow => (PAD_LEFT, CLIENT_W_DIP - PAD_LEFT - PAD_RIGHT),
        // 动作行的 x 由"分组起点 + 下标"算（见 `layout_with`）
        Column::ActionLeft | Column::ActionRight => (f32::NAN, w_override.unwrap_or(BTN_W)),
        Column::NoticeStrip => (COL_CTRL_A_X, COL_FULL_W - BTN_W - BTN_GAP),
        Column::NoticeButton => (
            COL_CTRL_A_X + COL_FULL_W - BTN_W,
            w_override.unwrap_or(BTN_W),
        ),
    }
}

/// 分段成员矩形（组内第 `index` 段；槽宽均分，余数给末段；间隙 1 DIP）。
fn segment_widths(slot_w: f32, count: usize) -> Vec<f32> {
    let total = count as f32;
    let base = ((slot_w - (total - 1.0)) / total).floor().max(1.0);
    let extra = (slot_w - (total - 1.0) - base * total).max(0.0);
    let mut widths = vec![base; count];
    if let Some(last) = widths.last_mut() {
        *last += extra;
    }
    widths
}

/// 底部动作行的分组起点（右对齐 `PAD_RIGHT`；等宽 + 等间隙 ⇒ 与列常量同源，不链式推导）。
fn action_row_origin(count: usize) -> f32 {
    let total = count as f32 * BTN_W + (count as f32 - 1.0) * BTN_GAP;
    CLIENT_W_DIP - PAD_RIGHT - total
}
/// **布局唯一真源**（纯函数）：`scale × 高级展开 × 提示条 × 可用客户区高` ⇒ 客户区 + 控件表。
///
/// 退化链（§2.6.1）：正常（行步 30）→ 屏幕放不下 ⇒ 行步 26 → 仍放不下 ⇒ **连接池组强制折叠**；
/// 三者都不放得下 ⇒ 返回最退化档（调用方按 `client_h` 放弃精确余量，见 I1-7 的钳制分支单测）。
///
/// **栅格（布局修复轮）**：行 y = `TOP_DIP + 行步格 × 行步`（同一条等差格；隐藏行不占格；
/// 结果行之后跳过 `ACTION_GROUP_SKIP` 格 = 动作区组距）；横向定位只读列常量表 + 组内下标
/// ⇒ A-3b 可机械断言"同列同 x / 同类同宽 / y 在格上"。
pub fn plan(scale: f32, advanced: bool, notice: bool, available_client_dip: f32) -> SettingsPlan {
    let normal = layout_with(scale, advanced, notice, ROW_STEP, false);
    if available_client_dip.is_infinite() || normal.client_h as f32 / scale <= available_client_dip
    {
        return normal;
    }
    let compact = layout_with(scale, advanced, notice, STEP_COMPACT_DIP, false);
    if compact.client_h as f32 / scale <= available_client_dip {
        return compact;
    }
    // 第三档：仍放不下 ⇒ 连接池组强制折叠（`advanced == false` 时该行本就不可见 ⇒ 折叠语义取 `advanced`）
    layout_with(scale, advanced, notice, STEP_COMPACT_DIP, advanced)
}

fn layout_with(
    scale: f32,
    advanced: bool,
    notice: bool,
    step: f32,
    pool_folded: bool,
) -> SettingsPlan {
    let px = |value: f32| (value * scale).round() as i32;
    let row_visible_at = |row: usize| {
        if row == ROW_NOTICE {
            notice
        } else {
            row_visible(row, advanced, pool_folded)
        }
    };
    // 行 → 行步格（隐藏行不占格）
    let mut row_slot = [0_usize; ROW_COUNT];
    let mut slot = 0_usize;
    if notice {
        row_slot[ROW_NOTICE] = slot;
        slot += 1;
    }
    for (row, assigned) in row_slot.iter_mut().enumerate() {
        if row == ROW_NOTICE || !row_visible_at(row) {
            continue;
        }
        *assigned = slot;
        slot += 1;
        if row == ROW_RESULT {
            slot += ACTION_GROUP_SKIP;
        }
    }
    let row_y = |row: usize| TOP_DIP + row_slot[row] as f32 * step;

    let mut items: Vec<PlanItem> = Vec::with_capacity(SPECS.len() + NOTICE_SPECS.len() + 8);
    // 底部动作行：按组起点 + 下标排（右对齐；**不读**"上一个按钮的右缘"）
    let right_count = SPECS
        .iter()
        .filter(|entry| entry.column == Column::ActionRight)
        .count();
    let mut action_index = [0_usize; 2];
    for entry in SPECS {
        let (column_x, column_w) = column_layout(entry.column, entry.w_override);
        let mut x = column_x;
        // 动作行按"分组起点 + 下标"排（左行 = 列 A 控件位；右行 = 右对齐 `PAD_RIGHT`）
        match entry.column {
            Column::ActionLeft => {
                x = COL_CTRL_A_X + action_index[0] as f32 * (BTN_W + BTN_GAP);
                action_index[0] += 1;
            }
            Column::ActionRight => {
                x = action_row_origin(right_count) + action_index[1] as f32 * (BTN_W + BTN_GAP);
                action_index[1] += 1;
            }
            _ => {}
        }
        items.push(PlanItem {
            id: entry.id,
            kind: entry.kind,
            text: entry.text,
            column: entry.column,
            x: px(x + entry.dx),
            y: px(row_y(entry.row)),
            w: px(column_w),
            h: px(entry.h),
            visible: row_visible_at(entry.row),
        });
    }
    // 分段成员（组表生成；统一槽宽 + 均分）
    for group in SEGMENT_GROUPS {
        let widths = segment_widths(COL_CTRL_A_W, group.ids.len());
        let mut cursor = COL_CTRL_A_X;
        for (index, id) in group.ids.iter().enumerate() {
            let width = widths.get(index).copied().unwrap_or(COL_CTRL_A_W);
            items.push(PlanItem {
                id: *id,
                kind: PlanKind::Segment,
                text: group.texts.get(index).copied().unwrap_or(""),
                column: Column::CtrlA,
                x: px(cursor),
                y: px(row_y(group.row)),
                w: px(width),
                h: px(ROW_H),
                visible: row_visible_at(group.row),
            });
            cursor += width + 1.0;
        }
    }
    for entry in NOTICE_SPECS {
        let (x, w) = column_layout(entry.column, entry.w_override);
        items.push(PlanItem {
            id: entry.id,
            kind: entry.kind,
            text: entry.text,
            column: entry.column,
            x: px(x + entry.dx),
            y: px(row_y(entry.row)),
            w: px(w),
            h: px(entry.h),
            visible: row_visible_at(entry.row),
        });
    }
    let mut content_bottom = TOP_DIP;
    for row in 0..ROW_COUNT {
        if row_visible_at(row) {
            content_bottom = content_bottom.max(row_y(row) + row_height(row));
        }
    }
    SettingsPlan {
        scale,
        compact: step < ROW_STEP,
        pool_folded,
        client_w: px(CLIENT_W_DIP),
        client_h: px(content_bottom + BOTTOM_PAD_DIP),
        content_bottom: px(content_bottom),
        items,
    }
}

/// 分段控件组（状态各自持在 `UiState.draft_*`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SegGroup {
    Cors,
    LogLevel,
    Close,
}

/// 分段控件的成员判定（CtlID ⇒ (组, 下标)）；非分段控件返回 `None`。
fn segment_member(id: usize) -> Option<(SegGroup, usize)> {
    let groups: [(SegGroup, &[usize]); 3] = [
        (SegGroup::Cors, &IDC_SEG_CORS),
        (SegGroup::LogLevel, &IDC_SEG_LOG_LEVEL),
        (SegGroup::Close, &IDC_SEG_CLOSE),
    ];
    for (group, ids) in groups {
        if let Some(index) = ids.iter().position(|candidate| *candidate == id) {
            return Some((group, index));
        }
    }
    None
}

fn segment_selected(ui: &UiState, group: SegGroup, index: usize) -> bool {
    match group {
        SegGroup::Cors => ui.draft_cors == index,
        SegGroup::LogLevel => ui.draft_log_level == index,
        SegGroup::Close => ui.draft_close_action == index,
    }
}

/// 设置窗的全部控件句柄（创建后挂到 `UiState::settings_controls`）。
///
/// `all` 与 [`SPECS`] + [`NOTICE_SPECS`] 的**创建顺序逐一对应**（`plan().items` 与 `all` 同序 ⇒
/// 重排 / 换字体都靠这个不变量 zip）。动作类控件（按钮 / 复选框 / 分段）与标签**不进本结构**：
/// 它们按 ID 经 `GetDlgItem` 取（置灰 / 失效都走同一条路）。
#[derive(Clone, Copy)]
pub struct SettingsControls {
    pub parent: HWND,
    pub upstream: HWND,
    pub strip: HWND,
    pub port: HWND,
    pub fallback: HWND,
    pub connect: HWND,
    pub first_byte: HWND,
    pub response_head: HWND,
    pub cors_max_age: HWND,
    pub pool_max: HWND,
    pub pool_idle: HWND,
    /// 只读读数行（`共 N / 上限 2000 条（已挤出 M）`）。
    pub log_stat: HWND,
    pub result: HWND,
    pub notice: HWND,
    pub all: [HWND; PLAN_ITEM_COUNT],
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

/// 设置窗的窗口样式（`WS_CLIPCHILDREN`：父窗自绘"输入井"时不与子控件抢同一片像素）。
fn window_style() -> WINDOW_STYLE {
    WINDOW_STYLE((WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN).0)
}

/// 当前布局（`plan()` 的调用点唯一收口；重排 / 绘制 / 创建都读它）。
fn current_plan(ui: &UiState) -> SettingsPlan {
    let notice = ui.status.apply_stage == ApplyStage::Failed;
    plan(
        ui.theme.scale,
        ui.advanced_open,
        notice,
        available_client_dip(ui),
    )
}

/// 屏幕可用客户区高（DIP）：`SPI_GETWORKAREA` − 标题栏/边框估计 − 一点余量。
/// 取不到 ⇒ `INFINITY`（不降级 —— 宁可窗口超出也不误判）。
fn available_client_dip(ui: &UiState) -> f32 {
    let mut work = RECT::default();
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut work as *mut RECT as *mut core::ffi::c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    if ok.is_err() {
        return f32::INFINITY;
    }
    let chrome = unsafe { GetSystemMetrics(SM_CYCAPTION) + 2 * GetSystemMetrics(SM_CYSIZEFRAME) };
    let available = (work.bottom - work.top) - chrome - ui.theme.px(SPACE_M);
    available as f32 / ui.theme.scale
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
    let settings = current_plan(ui);
    let class_name = wide(SETTINGS_WINDOW_CLASS);
    let title = wide("设置 — AzusaAI 本地反向代理");
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: settings.client_w,
        bottom: settings.client_h,
    };
    unsafe {
        let _ = AdjustWindowRectEx(&mut rect, window_style(), false, WINDOW_EX_STYLE(0));
        let pointer = ui as *mut UiState;
        let hwnd = CreateWindowExW(
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

/// 控件矩形（`GetDlgItem` 取不到 ⇒ `None`；重绘/量宽用）。
fn control_rect(parent: HWND, id: usize) -> Option<RECT> {
    let control = unsafe { GetDlgItem(Some(parent), id as i32) }.unwrap_or_default();
    if control.is_invalid() {
        return None;
    }
    let mut rect = RECT::default();
    unsafe {
        let _ = GetClientRect(control, &mut rect);
    }
    Some(rect)
}

fn invalidate_control(parent: HWND, id: usize) {
    let control = unsafe { GetDlgItem(Some(parent), id as i32) }.unwrap_or_default();
    if !control.is_invalid() {
        unsafe {
            let _ = InvalidateRect(Some(control), None, true);
        }
    }
}

/// 分段控件 + 复选框的选中态重绘（状态自持 ⇒ 改完必须自己失效）。
fn invalidate_selection(parent: HWND) {
    for id in IDC_SEG_CORS
        .iter()
        .chain(IDC_SEG_LOG_LEVEL.iter())
        .chain(IDC_SEG_CLOSE.iter())
        .copied()
    {
        invalidate_control(parent, id);
    }
    invalidate_control(parent, IDC_FIELD_ADVANCED);
}

fn find(parent: HWND, id: usize) -> HWND {
    unsafe { GetDlgItem(Some(parent), id as i32) }.unwrap_or_default()
}

/// 用当前配置填满控件（并把分段草稿态与配置对齐）。
fn fill_fields(ui: &mut UiState, controls: &SettingsControls) {
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
    // 响应头超时：**单位 ms**（与 `--help`/config 同单位 ⇒ 2500 就是 2500，不再被显示成 2 秒）
    set_text(
        controls.response_head,
        &config.timeouts.response_head_ms.to_string(),
    );
    set_text(
        controls.first_byte,
        &(config.timeouts.first_byte_ms / 1000).to_string(),
    );
    set_text(
        controls.cors_max_age,
        &config.cors.max_age_seconds.to_string(),
    );
    set_text(
        controls.pool_max,
        &config.pool.max_idle_per_host.to_string(),
    );
    set_text(
        controls.pool_idle,
        &(config.pool.idle_timeout_ms / 1000).to_string(),
    );
    ui.draft_cors = if config.cors.mode == "echo" { 1 } else { 0 };
    ui.draft_log_level = match config.logging.level.as_str() {
        "error" => 0,
        "warn" => 1,
        "debug" => 3,
        _ => 2,
    };
    ui.draft_close_action = if config.ui.close_action == "exit" {
        1
    } else {
        0
    };
    update_log_stat(ui, controls);
    invalidate_selection(controls.parent);
}

/// 结果行（三态：普通 / 成功 / 失败；文本前缀与墨色由 `tone` 决定）。
fn set_result(ui: &mut UiState, text: &str, tone: ResultTone) {
    ui.result_tone = tone;
    let Some(result) = ui
        .settings_controls
        .as_ref()
        .map(|controls| controls.result)
    else {
        return;
    };
    let prefix = match tone {
        ResultTone::Ok => "✓ ",
        ResultTone::Error => "✗ ",
        ResultTone::Plain => "",
    };
    set_text(result, &format!("{prefix}{text}"));
    // `SS_OWNERDRAW` 的 STATIC 不会因 `SetWindowTextW` 自动重绘 ⇒ 手动失效
    unsafe {
        let _ = InvalidateRect(Some(result), None, true);
    }
}

/// 只读读数行（P6-S5 / D12）：内存环的 条数 / 硬上限 / 被挤出条数。
///
/// **裁切防御**（P7 诉求 ① 的机制面）：文案 = [`log_stat_text`]（与 I2 的日志窗共用）；超宽 ⇒
/// `fit_text` 收口 + `logger.warn` **一次**（不用 `debug_assert!`：win7 release 档 `panic = "abort"`）。
fn update_log_stat(ui: &UiState, controls: &SettingsControls) {
    let text = log_stat_text(
        ui.logger.retained_count(),
        LOG_RING_CAPACITY,
        ui.logger.dropped_count(),
    );
    let available = control_rect(controls.parent, IDC_FIELD_LOG_STAT)
        .map(|rect| rect.right - rect.left)
        .unwrap_or(0);
    let shown = unsafe {
        let hdc = GetDC(None);
        let measured = measure_text(hdc, ui.theme.font_ui, &text);
        let fitted = fit_text(hdc, ui.theme.font_ui, &text, available);
        ReleaseDC(None, hdc);
        if measured > available && !LOG_STAT_WIDTH_WARNED.swap(true, Ordering::Relaxed) {
            ui.logger.warn(&format!(
                "设置窗日志读数行超宽：需 {measured} px、可用 {available} px —— 已按 fit_text 截断\
                 （读数为 共 N / 上限 {LOG_RING_CAPACITY} 条（已挤出 M））"
            ));
        }
        fitted
    };
    set_text(controls.log_stat, &shown);
}

/// 点「查看日志」/「复制到剪贴板」前刷新读数行（条数与挤出数随运行时间变化）。
fn refresh_log_stat(ui: &UiState) {
    if let Some(controls) = ui.settings_controls.as_ref() {
        update_log_stat(ui, controls);
    }
}

/// 秒 → 毫秒（**先上界校验再乘**；范式⑤：修既有 `秒 × 1000` 的溢出缺陷）。
///
/// `10^19` 这类输入在 debug 档会 panic、release 档回绕成"合法"值（静默错误）—— 这里一律报错。
fn seconds_to_ms(seconds: u64, field: &str) -> Result<u64, String> {
    const MAX_SECONDS: u64 = 86_400;
    if seconds > MAX_SECONDS {
        return Err(format!(
            "{field} 不得超过 {MAX_SECONDS} 秒（24 小时；当前 {seconds}）"
        ));
    }
    seconds
        .checked_mul(1000)
        .ok_or_else(|| format!("{field} 数值过大（{seconds} 秒）"))
}

/// 应用流程：校验 → **仅内存生效**（P6-S4：不写任何文件；失败回滚由服务侧做，
/// 结果经 `refresh_apply_feedback` 回填）。
fn apply_fields(ui: &mut UiState) {
    let Some(controls) = ui.settings_controls.as_ref().copied() else {
        return;
    };
    // 先把字段读成局部值（后面的 `set_result(&mut ui)` 需要独占借用）
    let upstream = get_text(controls.upstream);
    let strip = get_text(controls.strip);
    let port = get_text(controls.port);
    let fallback = get_text(controls.fallback);
    let connect = get_text(controls.connect);
    let first_byte = get_text(controls.first_byte);
    let response_head = get_text(controls.response_head);
    let cors_max_age = get_text(controls.cors_max_age);
    let pool_max = get_text(controls.pool_max);
    let pool_idle = get_text(controls.pool_idle);
    let draft_cors = ui.draft_cors;
    let draft_log_level = ui.draft_log_level;
    let draft_close_action = ui.draft_close_action;
    let mut config = ui.status.config.clone();

    let parse_u16 = |text: &str, field: &str| -> Result<u16, String> {
        text.trim()
            .parse::<u16>()
            .map_err(|_| format!("{field} 必须是 1–65535 的整数（当前「{}」）", text.trim()))
    };
    let parse_u64 = |text: &str, field: &str| -> Result<u64, String> {
        text.trim()
            .parse::<u64>()
            .map_err(|_| format!("{field} 必须是非负整数（当前「{}」）", text.trim()))
    };
    let parse_usize = |text: &str, field: &str| -> Result<usize, String> {
        let value = parse_u64(text, field)?;
        usize::try_from(value).map_err(|_| format!("{field} 数值过大（{value}）"))
    };

    let validated: Result<(), String> = (|| {
        config.upstream_base = upstream.trim().to_string();
        config.strip_prefix = strip.trim().to_string();
        config.listen_port = parse_u16(&port, "监听端口")?;
        let mut candidates = Vec::new();
        for part in fallback
            .split([',', '，', ' ', ';', '、'])
            .filter(|piece| !piece.trim().is_empty())
        {
            candidates.push(parse_u16(part, &format!("候选端口「{}」", part.trim()))?);
        }
        config.port_fallback = candidates;
        // 连接超时（秒，≥1；上界 + checked_mul 见 seconds_to_ms）
        let connect_seconds = parse_u64(&connect, "连接超时（秒）")?;
        if connect_seconds == 0 {
            return Err("连接超时必须 ≥ 1 秒（0 会让不可达上游永远挂住）".to_string());
        }
        config.timeouts.connect_ms = seconds_to_ms(connect_seconds, "连接超时")?;
        // 首字节（秒；**0 = 不限** —— 与 connect 的 ≥1 语义差异写进字段提示）
        config.timeouts.first_byte_ms =
            seconds_to_ms(parse_u64(&first_byte, "首字节超时（秒）")?, "首字节超时")?;
        // 响应头超时（**ms**；0 = 不限 ⇒ 半开/静默上游会永久挂死，不推荐）
        config.timeouts.response_head_ms = parse_u64(&response_head, "响应头超时（ms）")?;
        config.cors.mode = if draft_cors == 1 { "echo" } else { "*" }.to_string();
        config.cors.max_age_seconds = parse_u64(&cors_max_age, "预热 Max-Age（秒）")?;
        config.pool.max_idle_per_host = parse_usize(&pool_max, "池最大空闲/主机")?;
        config.pool.idle_timeout_ms =
            seconds_to_ms(parse_u64(&pool_idle, "池空闲超时（秒）")?, "池空闲超时")?;
        config.logging.level = match draft_log_level {
            0 => "error".to_string(),
            1 => "warn".to_string(),
            3 => "debug".to_string(),
            _ => "info".to_string(),
        };
        config.ui.close_action = if draft_close_action == 1 {
            "exit".to_string()
        } else {
            "tray".to_string()
        };
        config.validate().map_err(|err| err.to_string())?;
        Ok(())
    })();

    if let Err(err) = validated {
        // 本地校验失败：**不更新** `last_apply_notice`（`ui/mod.rs` 的快照去重键只覆盖服务端
        // apply 面）⇒ 下一帧不会被服务端旧快照冲掉（J-A6）
        set_result(ui, &err, ResultTone::Error);
        return;
    }

    // 点击 → 服务端首帧之间的**即时**反馈（文案与服务端 APPLY_START_TEXT 同源）
    set_result(ui, APPLY_START_TEXT, ResultTone::Plain);
    ui.controller.apply(config);
}

/// 服务端 apply 阶段的回填（`UiState::refresh` 里**快照去重后**调用）。
///
/// - 文本 = `Status.apply_message`（三 owner 之一：本函数只写服务端 apply 面）；
/// - 墨色 / 前缀 / 提示条**只读** `Status.apply_stage`（`Failed` ⇒ `✗` + 红 + 提示条）；
/// - 提示条显隐会改变行表高度 ⇒ 要重排（`apply_layout`）。
pub fn refresh_apply_feedback(ui: &mut UiState) {
    if ui.settings_hwnd.is_none() {
        return;
    }
    let stage = ui.status.apply_stage;
    let message = ui.status.apply_message.clone();
    if let Some(message) = message {
        let tone = match stage {
            ApplyStage::Failed => ResultTone::Error,
            ApplyStage::Applied => ResultTone::Ok,
            _ => ResultTone::Plain,
        };
        set_result(ui, &message, tone);
        if stage == ApplyStage::Failed {
            if let Some(controls) = ui.settings_controls.as_ref().copied() {
                set_text(controls.notice, &format!("应用失败：{message}"));
                unsafe {
                    let _ = InvalidateRect(Some(controls.notice), None, true);
                }
            }
        }
    }
    if let Some(hwnd) = ui.settings_hwnd {
        if !hwnd.is_invalid() {
            apply_layout(ui, hwnd);
        }
    }
}

/// 「测试上游」：唯一的主动探测（用户触发）。
fn test_upstream(ui: &mut UiState) {
    let Some(controls) = ui.settings_controls.as_ref().copied() else {
        return;
    };
    let base = get_text(controls.upstream)
        .trim()
        .trim_end_matches('/')
        .to_string();
    if base.is_empty() {
        set_result(ui, "上游 Base URL 为空", ResultTone::Error);
        return;
    }
    let url = format!("{base}/v1/models");
    // `HWND` 不是 `Send` ⇒ 跨线程只传裸地址，回投时重建（窗口已销毁时 `PostMessageW` 是无害 no-op）
    let hwnd_raw = ui.settings_hwnd.unwrap_or(ui.hwnd).0 as isize;
    set_result(
        ui,
        &format!("测试中…（GET {url}，超时 10 s）"),
        ResultTone::Plain,
    );
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

/// `[高级选项]` 勾选 ⇒ 展开"剥离前缀"与"连接池"两行 + 窗口原位增高（§2.6.1/I1-7）。
fn toggle_advanced(ui: &mut UiState, hwnd: HWND) {
    ui.advanced_open = !ui.advanced_open;
    apply_layout(ui, hwnd);
}

/// 分段控件的点击（状态自持：改草稿态 + 失效重绘；**不读 `BM_GETCHECK`**）。
fn select_segment(ui: &mut UiState, id: usize) {
    let Some((group, index)) = segment_member(id) else {
        return;
    };
    match group {
        SegGroup::Cors => ui.draft_cors = index,
        SegGroup::LogLevel => ui.draft_log_level = index,
        SegGroup::Close => ui.draft_close_action = index,
    }
    if let Some(controls) = ui.settings_controls.as_ref() {
        invalidate_selection(controls.parent);
    }
}

/// 重排（**唯一入口**：高级展开 / 提示条显隐 / DPI 变化 / 窗口创建后收口都走它）。
///
/// 一步一图（§2.6.1 的退化链由 `plan()` 决定）：窗口尺寸（保持位置）→ 逐控件 `SetWindowPos` +
/// 显隐 → 父窗整幅失效（重画"输入井"）。
fn apply_layout(ui: &UiState, hwnd: HWND) {
    let Some(controls) = ui.settings_controls.as_ref() else {
        return;
    };
    let settings = current_plan(ui);
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: settings.client_w,
        bottom: settings.client_h,
    };
    unsafe {
        let _ = AdjustWindowRectEx(&mut rect, window_style(), false, WINDOW_EX_STYLE(0));
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            rect.right - rect.left,
            rect.bottom - rect.top,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
    let inset = ui.theme.px(WELL_INSET_DIP);
    for (item, control) in settings.items.iter().zip(controls.all.iter()) {
        if control.is_invalid() {
            continue;
        }
        let (x, y, w, h) = match item.kind {
            PlanKind::Edit | PlanKind::NumberEdit => (
                item.x + inset,
                item.y + inset,
                item.w - inset * 2,
                item.h - inset * 2,
            ),
            _ => (item.x, item.y, item.w, item.h),
        };
        unsafe {
            let _ = SetWindowPos(*control, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
            let _ = ShowWindow(*control, if item.visible { SW_SHOW } else { SW_HIDE });
        }
    }
    unsafe {
        let _ = InvalidateRect(Some(hwnd), None, true);
    }
}

/// DPI 变化后重发字体（自绘控件用的是 `WM_DRAWITEM` 里的主题字体，但 STATIC/EDIT 要吃 WM_SETFONT）。
fn refresh_fonts(ui: &UiState) {
    let Some(controls) = ui.settings_controls.as_ref() else {
        return;
    };
    for control in controls.all.iter() {
        if control.is_invalid() {
            continue;
        }
        unsafe {
            let _ = SendMessageW(
                *control,
                WM_SETFONT,
                Some(WPARAM(ui.theme.font_ui.0 as usize)),
                Some(LPARAM(1)),
            );
            let _ = InvalidateRect(Some(*control), None, true);
        }
    }
}

/// 自绘一个控件（父窗 `WM_DRAWITEM`；返回 `true` = 已处理）。
unsafe fn draw_item(ui: &UiState, lparam: LPARAM) -> bool {
    let pointer = lparam.0 as *const DRAWITEMSTRUCT;
    if pointer.is_null() {
        return false;
    }
    let item = unsafe { &*pointer };
    if item.hDC.is_invalid() {
        return false;
    }
    if item.CtlType == ODT_BUTTON {
        draw_button(ui, item);
        return true;
    }
    if item.CtlType == ODT_STATIC {
        draw_owner_static(ui, item);
        return true;
    }
    false
}

/// 自绘按钮 / 复选框 / 分段控件。
fn draw_button(ui: &UiState, item: &DRAWITEMSTRUCT) {
    let id = item.CtlID as usize;
    let rect = item.rcItem;
    let pressed = item.itemState.0 & ODS_SELECTED.0 != 0;
    let disabled = item.itemState.0 & ODS_DISABLED.0 != 0;
    let focused = item.itemState.0 & ODS_FOCUS.0 != 0;
    let text = get_text(item.hwndItem);
    fill_rect_color(item.hDC, &rect, COLOR_BG);
    let gfx = if ui.gdiplus_ok {
        Gfx::from_hdc(item.hDC)
    } else {
        None
    };
    let radius = RADIUS_BUTTON * ui.theme.scale;

    if id == IDC_FIELD_ADVANCED {
        // 复选框：18×18 圆角 4 + 三段折线勾（`GdipDrawLines`）；状态自持（`advanced_open`）
        let box_size = ui.theme.px(18.0);
        let box_x = rect.left + ui.theme.px(SPACE_XS);
        let box_y = rect.top + (rect.bottom - rect.top - box_size) / 2;
        let box_rect = RECT {
            left: box_x,
            top: box_y,
            right: box_x + box_size,
            bottom: box_y + box_size,
        };
        if let Some(gfx) = gfx.as_ref() {
            let box_radius = 4.0 * ui.theme.scale;
            gfx.fill_round_rect(
                box_x as f32,
                box_y as f32,
                box_size as f32,
                box_size as f32,
                box_radius,
                if ui.advanced_open {
                    COLOR_ACCENT_DEEP
                } else {
                    COLOR_CARD
                },
            );
            gfx.stroke_round_rect(
                box_x as f32,
                box_y as f32,
                box_size as f32,
                box_size as f32,
                box_radius,
                if ui.advanced_open {
                    COLOR_ACCENT_DEEP
                } else {
                    COLOR_CARD_BORDER
                },
                ui.theme.scale.max(1.0),
            );
            if ui.advanced_open {
                let unit = box_size as f32;
                let points = [
                    windows::Win32::Graphics::GdiPlus::PointF {
                        X: box_x as f32 + unit * 0.22,
                        Y: box_y as f32 + unit * 0.52,
                    },
                    windows::Win32::Graphics::GdiPlus::PointF {
                        X: box_x as f32 + unit * 0.42,
                        Y: box_y as f32 + unit * 0.72,
                    },
                    windows::Win32::Graphics::GdiPlus::PointF {
                        X: box_x as f32 + unit * 0.78,
                        Y: box_y as f32 + unit * 0.28,
                    },
                ];
                gfx.polyline(&points, 0xFFFFFFFF, ui.theme.scale * 2.0, true);
            }
        } else {
            fill_rect_color(item.hDC, &box_rect, COLOR_CARD_BORDER);
        }
        let text_rect = RECT {
            left: box_rect.right + ui.theme.px(SPACE_S),
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        };
        draw_text_line(
            item.hDC,
            ui.theme.font_ui,
            &text,
            &text_rect,
            COLOR_INK,
            DT_LEFT,
        );
        return;
    }

    if let Some((group, index)) = segment_member(id) {
        let selected = segment_selected(ui, group, index);
        let (fill, border, ink) = if disabled {
            (COLOR_CARD, COLOR_CARD_BORDER, COLOR_INK_DISABLED)
        } else if selected {
            (COLOR_ACCENT_SOFT, COLOR_ACCENT_DEEP, COLOR_INK)
        } else if pressed {
            (COLOR_PRESS_ON_CARD, COLOR_CARD_BORDER, COLOR_INK)
        } else {
            (COLOR_CARD, COLOR_CARD_BORDER, COLOR_INK)
        };
        if let Some(gfx) = gfx.as_ref() {
            gfx.fill_round_rect(
                rect.left as f32,
                rect.top as f32,
                (rect.right - rect.left) as f32,
                (rect.bottom - rect.top) as f32,
                radius,
                fill,
            );
            gfx.stroke_round_rect(
                rect.left as f32,
                rect.top as f32,
                (rect.right - rect.left) as f32,
                (rect.bottom - rect.top) as f32,
                radius,
                border,
                if selected { ui.theme.scale } else { 1.0 },
            );
            if focused {
                // 焦点环 = 全仓唯一 alpha 令牌，只走 GDI+
                gfx.stroke_round_rect(
                    rect.left as f32 + 1.0,
                    rect.top as f32 + 1.0,
                    (rect.right - rect.left) as f32 - 2.0,
                    (rect.bottom - rect.top) as f32 - 2.0,
                    radius,
                    COLOR_ACCENT_RING,
                    ui.theme.scale * 2.0,
                );
            }
        } else {
            fill_rect_color(item.hDC, &rect, fill);
        }
        let font = if selected {
            ui.theme.font_ui_bold
        } else {
            ui.theme.font_ui
        };
        draw_text_line(item.hDC, font, &text, &rect, ink, DT_CENTER);
        return;
    }

    // 普通按钮：主按钮 = 樱花底 + 白字；次按钮 = 卡片底 + 描边
    let primary = id == IDC_APPLY;
    let (fill, border, ink) = if disabled {
        if primary {
            (COLOR_INK_DISABLED, COLOR_INK_DISABLED, COLOR_CARD)
        } else {
            (COLOR_CARD, COLOR_CARD_BORDER, COLOR_INK_DISABLED)
        }
    } else if primary {
        let fill = if pressed {
            COLOR_ACCENT_PRESSED
        } else {
            COLOR_ACCENT_DEEP
        };
        (fill, fill, COLOR_CARD)
    } else if pressed {
        (COLOR_PRESS_ON_CARD, COLOR_CARD_BORDER, COLOR_INK)
    } else {
        (COLOR_CARD, COLOR_CARD_BORDER, COLOR_INK)
    };
    if let Some(gfx) = gfx.as_ref() {
        gfx.fill_round_rect(
            rect.left as f32,
            rect.top as f32,
            (rect.right - rect.left) as f32,
            (rect.bottom - rect.top) as f32,
            radius,
            fill,
        );
        gfx.stroke_round_rect(
            rect.left as f32,
            rect.top as f32,
            (rect.right - rect.left) as f32,
            (rect.bottom - rect.top) as f32,
            radius,
            border,
            ui.theme.scale.max(1.0),
        );
        if focused && !primary {
            gfx.stroke_round_rect(
                rect.left as f32 + 1.0,
                rect.top as f32 + 1.0,
                (rect.right - rect.left) as f32 - 2.0,
                (rect.bottom - rect.top) as f32 - 2.0,
                radius,
                COLOR_ACCENT_RING,
                ui.theme.scale * 2.0,
            );
        }
    } else {
        fill_rect_color(item.hDC, &rect, fill);
    }
    let font = if primary {
        ui.theme.font_ui_bold
    } else {
        ui.theme.font_ui
    };
    draw_text_line(item.hDC, font, &text, &rect, ink, DT_CENTER);
}

/// 自绘 `STATIC`：结果行（三态）与失败提示条。
fn draw_owner_static(ui: &UiState, item: &DRAWITEMSTRUCT) {
    let id = item.CtlID as usize;
    let rect = item.rcItem;
    let text = get_text(item.hwndItem);
    if id == IDC_RESULT {
        fill_rect_color(item.hDC, &rect, COLOR_BG);
        let (ink, bar) = match ui.result_tone {
            ResultTone::Ok => (COLOR_INK, false),
            ResultTone::Error => (COLOR_DANGER_INK, true),
            ResultTone::Plain => (COLOR_INK_SOFT, false),
        };
        let mut left = rect.left;
        if bar {
            let bar_width = ui.theme.px(3.0);
            let bar_rect = RECT {
                left: rect.left,
                top: rect.top + ui.theme.px(2.0),
                right: rect.left + bar_width,
                bottom: rect.bottom - ui.theme.px(2.0),
            };
            fill_rect_color(item.hDC, &bar_rect, COLOR_DANGER_INK);
            left = bar_rect.right + ui.theme.px(SPACE_S);
        }
        let width = rect.right - left;
        let (first, second) = split_two_lines(item.hDC, ui.theme.font_ui, &text, width);
        let line_height = ui.theme.px(18.0);
        let first_rect = RECT {
            left,
            top: rect.top,
            right: rect.right,
            bottom: rect.top + line_height,
        };
        draw_text_line(
            item.hDC,
            ui.theme.font_ui,
            &first,
            &first_rect,
            ink,
            DT_LEFT,
        );
        if let Some(second) = second {
            let second_rect = RECT {
                left,
                top: rect.top + line_height,
                right: rect.right,
                bottom: rect.bottom,
            };
            draw_text_line(
                item.hDC,
                ui.theme.font_ui,
                &second,
                &second_rect,
                ink,
                DT_LEFT,
            );
        }
        return;
    }
    if id == IDC_NOTICE {
        let gfx = if ui.gdiplus_ok {
            Gfx::from_hdc(item.hDC)
        } else {
            None
        };
        if let Some(gfx) = gfx.as_ref() {
            let radius = RADIUS_CARD * ui.theme.scale;
            gfx.fill_round_rect(
                rect.left as f32,
                rect.top as f32,
                (rect.right - rect.left) as f32,
                (rect.bottom - rect.top) as f32,
                radius,
                COLOR_NOTICE_BG,
            );
            gfx.stroke_round_rect(
                rect.left as f32,
                rect.top as f32,
                (rect.right - rect.left) as f32,
                (rect.bottom - rect.top) as f32,
                radius,
                COLOR_NOTICE_BORDER,
                ui.theme.scale.max(1.0),
            );
        } else {
            fill_rect_color(item.hDC, &rect, COLOR_NOTICE_BG);
        }
        let text_rect = RECT {
            left: rect.left + ui.theme.px(SPACE_M),
            top: rect.top,
            right: rect.right - ui.theme.px(SPACE_S),
            bottom: rect.bottom,
        };
        let width = text_rect.right - text_rect.left;
        let measured = measure_text(item.hDC, ui.theme.font_ui, &text);
        let shown = if measured > width {
            fit_text(item.hDC, ui.theme.font_ui, &text, width)
        } else {
            text.clone()
        };
        draw_text_line(
            item.hDC,
            ui.theme.font_ui,
            &shown,
            &text_rect,
            COLOR_NOTICE_INK,
            DT_LEFT,
        );
    }
}

/// 纯色填充（GDI；仅不透明令牌 —— 分通道口径见 `theme` 模块注释）。
fn fill_rect_color(hdc: HDC, rect: &RECT, color: u32) {
    unsafe {
        let brush = CreateSolidBrush(gdi_color(color));
        if !brush.is_invalid() {
            FillRect(hdc, rect, brush);
            let _ = DeleteObject(HGDIOBJ(brush.0));
        }
    }
}

/// 单行文本（GDI `DrawTextW`；`DT_*` 由调用方给：`DT_LEFT` / `DT_CENTER` 都与
/// [`DT_SINGLELINE`] `|` [`DT_VCENTER`] 组合）。
fn draw_text_line(
    hdc: HDC,
    font: HFONT,
    text: &str,
    rect: &RECT,
    color: u32,
    align: windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT,
) {
    if text.is_empty() {
        return;
    }
    let mut buffer = wide(text);
    buffer.pop();
    let mut target = *rect;
    unsafe {
        let old = windows::Win32::Graphics::Gdi::SelectObject(hdc, HGDIOBJ(font.0));
        let _ = SetBkMode(hdc, TRANSPARENT);
        let _ = SetTextColor(hdc, gdi_color(color));
        let _ = DrawTextW(
            hdc,
            &mut buffer,
            &mut target,
            DT_SINGLELINE | DT_VCENTER | align,
        );
        windows::Win32::Graphics::Gdi::SelectObject(hdc, old);
    }
}

/// 结果行的两行切分：一行放得下 ⇒ 单行；否则按**字符**贪心切两行，第二行用 `fit_text` 截断
/// （⇒ 两条都 ≤ 行宽，"两行高 36"的容量有机械判据）。
fn split_two_lines(hdc: HDC, font: HFONT, text: &str, max_width: i32) -> (String, Option<String>) {
    if max_width <= 0 {
        return (String::new(), None);
    }
    if measure_text(hdc, font, text) <= max_width {
        return (text.to_string(), None);
    }
    let chars: Vec<char> = text.chars().collect();
    let mut first = String::new();
    let mut used = 0;
    for (index, ch) in chars.iter().enumerate() {
        let mut probe = first.clone();
        probe.push(*ch);
        if measure_text(hdc, font, &probe) > max_width {
            break;
        }
        first = probe;
        used = index + 1;
    }
    let rest: String = chars[used..].iter().collect();
    let second = fit_text(hdc, font, &rest, max_width);
    (first, Some(second))
}

fn build_controls(ui: &mut UiState, hwnd: HWND) {
    let settings = current_plan(ui);
    let hinst = ui.hinst;
    let font = ui.theme.font_ui;
    let mut all: Vec<HWND> = Vec::with_capacity(settings.items.len());
    for item in &settings.items {
        all.push(create_from_plan(hinst, hwnd, item, font));
    }
    let mut controls = SettingsControls {
        parent: hwnd,
        upstream: find(hwnd, IDC_FIELD_UPSTREAM),
        strip: find(hwnd, IDC_FIELD_STRIP),
        port: find(hwnd, IDC_FIELD_PORT),
        fallback: find(hwnd, IDC_FIELD_FALLBACK),
        connect: find(hwnd, IDC_FIELD_CONNECT),
        first_byte: find(hwnd, IDC_FIELD_FIRST_BYTE),
        response_head: find(hwnd, IDC_FIELD_RESPONSE_HEAD),
        cors_max_age: find(hwnd, IDC_FIELD_CORS_MAX_AGE),
        pool_max: find(hwnd, IDC_FIELD_POOL_MAX),
        pool_idle: find(hwnd, IDC_FIELD_POOL_IDLE),
        log_stat: find(hwnd, IDC_FIELD_LOG_STAT),
        result: find(hwnd, IDC_RESULT),
        notice: find(hwnd, IDC_NOTICE),
        all: [HWND::default(); PLAN_ITEM_COUNT],
    };
    for (slot, control) in controls.all.iter_mut().zip(all.iter()) {
        *slot = *control;
    }
    fill_fields(ui, &controls);
    ui.settings_controls = Some(controls);
    // 创建期的显示态/尺寸收口（含 `advanced_open` 折叠与提示条）
    apply_layout(ui, hwnd);
}

/// 按 [`PlanItem`] 创建控件。
fn create_from_plan(hinst: HINSTANCE, parent: HWND, item: &PlanItem, font: HFONT) -> HWND {
    let base = (WS_CHILD | WS_VISIBLE).0;
    let (class, style) = match item.kind {
        PlanKind::Label | PlanKind::LogStat => ("STATIC", base | SS_LEFTNOWORDWRAP),
        PlanKind::Result | PlanKind::Notice => ("STATIC", base | SS_OWNERDRAW),
        // 去 `WS_BORDER`（自绘"输入井"接手描边）；补 `WS_TABSTOP`（既有缺陷：键盘 Tab 进不去输入框）
        PlanKind::Edit => ("EDIT", base | WS_TABSTOP.0 | ES_AUTOHSCROLL as u32),
        PlanKind::NumberEdit => (
            "EDIT",
            base | WS_TABSTOP.0 | ES_AUTOHSCROLL as u32 | ES_NUMBER as u32,
        ),
        PlanKind::Checkbox
        | PlanKind::Segment
        | PlanKind::ButtonPrimary
        | PlanKind::ButtonSecondary => ("BUTTON", base | WS_TABSTOP.0 | BS_OWNERDRAW as u32),
    };
    let class_wide = wide(class);
    let text_wide = wide(item.text);
    let control = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class_wide.as_ptr()),
            PCWSTR(text_wide.as_ptr()),
            WINDOW_STYLE(style),
            item.x,
            item.y,
            item.w,
            item.h,
            Some(parent),
            Some(HMENU(item.id as *mut core::ffi::c_void)),
            Some(hinst),
            None,
        )
    }
    .unwrap_or_default();
    if !control.is_invalid() {
        unsafe {
            SendMessageW(
                control,
                WM_SETFONT,
                Some(WPARAM(font.0 as usize)),
                Some(LPARAM(1)),
            );
        }
    }
    control
}

/// 画"输入井"（父窗 `WM_PAINT`）：圆角 6 + 卡片底 + 1 DIP 描边；EDIT 子控件内缩 2 DIP 坐在里面。
fn paint_wells(ui: &UiState, hdc: HDC) {
    if !ui.gdiplus_ok {
        return;
    }
    let Some(gfx) = Gfx::from_hdc(hdc) else {
        return;
    };
    let settings = current_plan(ui);
    let radius = RADIUS_FIELD * ui.theme.scale;
    for item in &settings.items {
        if !item.visible {
            continue;
        }
        if !matches!(item.kind, PlanKind::Edit | PlanKind::NumberEdit) {
            continue;
        }
        gfx.fill_round_rect(
            item.x as f32,
            item.y as f32,
            item.w as f32,
            item.h as f32,
            radius,
            COLOR_CARD,
        );
        gfx.stroke_round_rect(
            item.x as f32,
            item.y as f32,
            item.w as f32,
            item.h as f32,
            radius,
            COLOR_CARD_BORDER,
            ui.theme.scale.max(1.0),
        );
    }
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
                with_ui(hwnd, |ui| {
                    if segment_member(id).is_some() {
                        select_segment(ui, id);
                        return;
                    }
                    match id {
                        IDC_APPLY => apply_fields(ui),
                        IDC_TEST => test_upstream(ui),
                        IDC_RELOAD => {
                            if let Some(controls) = ui.settings_controls.as_ref().copied() {
                                fill_fields(ui, &controls);
                                set_result(ui, "已从当前配置重新载入", ResultTone::Plain);
                            }
                        }
                        IDC_VIEW_LOGS | IDC_NOTICE_LOGS => {
                            refresh_log_stat(ui);
                            ui.show_logs();
                        }
                        IDC_COPY_LOGS => {
                            refresh_log_stat(ui);
                            ui.copy_logs();
                        }
                        IDC_FIELD_ADVANCED => toggle_advanced(ui, hwnd),
                        _ => {}
                    }
                });
                LRESULT(0)
            }
            WM_DRAWITEM => {
                let pointer = ui_ptr(hwnd);
                if pointer.is_null() {
                    return DefWindowProcW(hwnd, message, wparam, lparam);
                }
                let handled = draw_item(&*pointer, lparam);
                LRESULT(if handled { 1 } else { 0 })
            }
            WM_PAINT => {
                let pointer = ui_ptr(hwnd);
                let mut paint = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut paint);
                if !pointer.is_null() {
                    paint_wells(&*pointer, hdc);
                }
                let _ = EndPaint(hwnd, &paint);
                LRESULT(0)
            }
            WM_APP_TEST_RESULT => {
                let text = TEST_RESULT.lock().ok().and_then(|slot| slot.clone());
                with_ui(hwnd, |ui| {
                    if let Some(text) = text {
                        set_result(ui, &format!("测试结果：{text}"), ResultTone::Plain);
                    }
                });
                LRESULT(0)
            }
            WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => {
                let hdc = windows::Win32::Graphics::Gdi::HDC(wparam.0 as *mut core::ffi::c_void);
                let pointer = ui_ptr(hwnd);
                if !pointer.is_null() {
                    let _ = SetBkMode(hdc, TRANSPARENT);
                    let _ = SetTextColor(hdc, gdi_color(COLOR_INK));
                    return LRESULT((*pointer).theme.brush_bg.0 as isize);
                }
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
            WM_CTLCOLOREDIT => {
                let hdc = windows::Win32::Graphics::Gdi::HDC(wparam.0 as *mut core::ffi::c_void);
                let pointer = ui_ptr(hwnd);
                if !pointer.is_null() {
                    // 输入井的底色（井是自绘的、EDIT 坐在里面 ⇒ 两者必须同色）
                    let _ = SetBkMode(hdc, TRANSPARENT);
                    let _ = SetTextColor(hdc, gdi_color(COLOR_INK));
                    let _ = SetBkColor(hdc, gdi_color(COLOR_CARD));
                    return LRESULT((*pointer).theme.brush_card.0 as isize);
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
                    // 重算 plan() + 逐个 SetWindowPos（布局随新 scale 整体重排）
                    apply_layout(ui, hwnd);
                    if !suggested.is_null() {
                        let rect = *suggested;
                        let _ = SetWindowPos(
                            hwnd,
                            None,
                            rect.left,
                            rect.top,
                            0,
                            0,
                            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                        );
                    }
                    refresh_fonts(ui);
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

#[cfg(test)]
mod tests {
    use super::super::theme::Theme;
    use super::*;
    use azusa_local_proxy::config::Config;
    use azusa_local_proxy::service::{APPLY_DRAIN_TEXT, STOPPED_TEXT, STOP_TEXT};
    use windows::Win32::Graphics::Gdi::{GetDC, ReleaseDC};

    fn visible_items(settings: &SettingsPlan) -> Vec<&PlanItem> {
        settings.items.iter().filter(|item| item.visible).collect()
    }

    /// **A-3b 的检查体**（正负例共用）：同列同 x · 同列同类同宽 · y 在行步等差格上 · 整行字段同宽同左缘。
    fn column_checks(settings: &SettingsPlan, step_dip: f32) -> Result<(), String> {
        let items = visible_items(settings);
        // ① 同列同 x（列 A 标签 / 列 A 控件 / 列 B 标签 / 列 B 控件；**分段与动作行是"组"**，见 ⑤）
        for column in [Column::LabelA, Column::CtrlA, Column::LabelB, Column::CtrlB] {
            let mut expected: Option<i32> = None;
            for item in items
                .iter()
                .filter(|item| item.column == column && item.kind != PlanKind::Segment)
            {
                match expected {
                    None => expected = Some(item.x),
                    Some(x) if x == item.x => {}
                    Some(x) => {
                        return Err(format!(
                            "同列 x 不一致：列 {column:?} id={} x={} ≠ {x}",
                            item.id, item.x
                        ));
                    }
                }
            }
        }
        // ② 同列同类同宽（≥2 个成员才算）
        for column in [
            Column::LabelA,
            Column::CtrlA,
            Column::LabelB,
            Column::CtrlB,
            Column::FullRow,
            Column::ActionLeft,
            Column::ActionRight,
        ] {
            for kind in [
                PlanKind::Label,
                PlanKind::Edit,
                PlanKind::NumberEdit,
                PlanKind::Segment,
                PlanKind::ButtonSecondary,
                PlanKind::ButtonPrimary,
            ] {
                // 分段成员的组内等分不在本条（见 ⑤ 的"组总宽 = 槽宽"），其余同类必须等宽
                let mut widths: Vec<(usize, i32)> = items
                    .iter()
                    .filter(|item| {
                        item.column == column && item.kind == kind && !(kind == PlanKind::Segment)
                    })
                    .map(|item| (item.id, item.w))
                    .collect();
                widths.sort_unstable();
                if widths.len() >= 2 {
                    let first = widths[0].1;
                    if let Some((id, width)) = widths.iter().find(|(_, w)| *w != first) {
                        return Err(format!(
                            "同列同类宽度不一致：列 {column:?} 类 {kind:?} 首个={first} id={id} w={width}"
                        ));
                    }
                }
            }
        }
        // ③ y ∈ 行步等差格（换算回 DIP 再取模，容忍 DPI 取整的 ±0.5 px）
        let top_px = (TOP_DIP * settings.scale).round() as i32;
        for item in &items {
            let offset_dip = (item.y - top_px) as f32 / settings.scale;
            let ratio = offset_dip / step_dip;
            if (ratio - ratio.round()).abs() > 0.04 {
                return Err(format!(
                    "y 不在行步等差格上：id={} y={}（offset={offset_dip:.2} DIP / step={step_dip}）",
                    item.id, item.y
                ));
            }
        }
        // ⑤ 组内定位：动作行 = 分组起点 + 下标 × (BTN_W + BTN_GAP)；分段组 = 列 A 起点 + 总宽 = 槽宽
        for (column, origin_dip) in [
            (Column::ActionLeft, COL_CTRL_A_X),
            (
                Column::ActionRight,
                action_row_origin(
                    items
                        .iter()
                        .filter(|item| item.column == Column::ActionRight)
                        .count(),
                ),
            ),
        ] {
            let mut xs: Vec<i32> = items
                .iter()
                .filter(|item| item.column == column)
                .map(|item| item.x)
                .collect();
            xs.sort_unstable();
            for (index, x) in xs.iter().enumerate() {
                let expect = ((origin_dip + index as f32 * (BTN_W + BTN_GAP)) * settings.scale)
                    .round() as i32;
                if *x != expect {
                    return Err(format!(
                        "动作行 x 不在分组格上：列 {column:?} 第 {index} 项 x={x} ≠ {expect}"
                    ));
                }
            }
        }
        for group in SEGMENT_GROUPS {
            let members: Vec<&PlanItem> = items
                .iter()
                .filter(|item| group.ids.contains(&item.id))
                .copied()
                .collect();
            if members.len() != group.ids.len() {
                continue; // 该组本状态下不可见（与 row 可见性同源，下一轮循环覆盖）
            }
            let first = members.iter().map(|item| item.x).min().unwrap_or(0);
            let expect_first = (COL_CTRL_A_X * settings.scale).round() as i32;
            if first != expect_first {
                return Err(format!(
                    "分段组起点不在列 A 控件位：{first} ≠ {expect_first}"
                ));
            }
            let total: i32 = members.iter().map(|item| item.w).sum::<i32>()
                + (members.len() as i32 - 1) * (1.0 * settings.scale).round() as i32;
            let expect_total = (COL_CTRL_A_W * settings.scale).round() as i32;
            // 段宽按 DIP 均分后逐段取整 ⇒ 允许每段 ±1 px 的累积（组起点与槽宽都由列常量决定）
            if (total - expect_total).abs() > members.len() as i32 {
                return Err(format!(
                    "分段组总宽 ≠ 列槽宽（超 ±1px/段）：{total} ≠ {expect_total}"
                ));
            }
        }
        // ④ 整行字段（FullRow）：左缘 = 列 A 控件位、宽 = COL_FULL_W
        let full_x = (COL_CTRL_A_X * settings.scale).round() as i32;
        let full_w = (COL_FULL_W * settings.scale).round() as i32;
        for item in items.iter().filter(|item| item.column == Column::FullRow) {
            if item.x != full_x || item.w != full_w {
                return Err(format!(
                    "整行字段不在整行位：id={} x={} w={}（期望 x={full_x} w={full_w}）",
                    item.id, item.x, item.w
                ));
            }
        }
        // ③b 结果行 = 页边距整行（左缘 PAD_LEFT、宽 = 客户区 − 2 × PAD）
        let row_x = (PAD_LEFT * settings.scale).round() as i32;
        let row_w = CLIENT_W_DIP - 2.0 * PAD_LEFT;
        for item in items.iter().filter(|item| item.column == Column::ResultRow) {
            if item.x != row_x || item.w != (row_w * settings.scale).round() as i32 {
                return Err(format!(
                    "结果行不在页边距整行位：id={} x={} w={}（期望 x={row_x} w={}）",
                    item.id,
                    item.x,
                    item.w,
                    (row_w * settings.scale).round() as i32
                ));
            }
        }
        // ⑥ 提示条（`NoticeStrip` / `NoticeButton`）—— UI 方案 §6.2 A-3b ⑥ 的枚举面（I1b 登记项）：
        //   条左缘 = 列 A 控件位、宽 = `COL_FULL_W − BTN_W − BTN_GAP`；条上按钮**右缘** = 客户区 − `PAD_RIGHT`。
        //   （条与按钮的 **y** 已由 ③ 的行步格覆盖；两条都由常量一次性算出、无链式推导 ⇒ 断言边际成本 ≈ 0，
        //    且只有加上才满足"枚举**全部**子控件"的字面要求。）
        let strip_x = (COL_CTRL_A_X * settings.scale).round() as i32;
        let strip_w = ((COL_FULL_W - BTN_W - BTN_GAP) * settings.scale).round() as i32;
        for item in items
            .iter()
            .filter(|item| item.column == Column::NoticeStrip)
        {
            if item.x != strip_x || item.w != strip_w {
                return Err(format!(
                    "提示条不在列位：id={} x={} w={}（期望 x={strip_x} w={strip_w} = COL_FULL_W − BTN_W − BTN_GAP）",
                    item.id, item.x, item.w
                ));
            }
        }
        let notice_right = ((CLIENT_W_DIP - PAD_RIGHT) * settings.scale).round() as i32;
        for item in items
            .iter()
            .filter(|item| item.column == Column::NoticeButton)
        {
            if item.x + item.w != notice_right {
                return Err(format!(
                    "提示条按钮右缘不对齐客户区右缘：id={} right={}（期望 {notice_right} = 客户区 − PAD_RIGHT）",
                    item.id,
                    item.x + item.w
                ));
            }
        }
        Ok(())
    }

    /// **A-3b（判据；带负例）**：布局栅格 —— 同列同 x / 同列同类同宽 / y 在行步等差格 / 整行字段同位。
    ///
    /// **负例（[E13] 强制，随测试每次执行）**：把任一行的 x 偏 8 px ⇒ 同一检查体**必须**报错。
    #[test]
    fn plan_columns_are_aligned() {
        for dpi in [96_u32, 120, 144, 192] {
            let theme = Theme::new(dpi);
            for advanced in [false, true] {
                for notice in [false, true] {
                    let settings = plan(theme.scale, advanced, notice, f32::INFINITY);
                    let step = if settings.compact {
                        STEP_COMPACT_DIP
                    } else {
                        ROW_STEP
                    };
                    column_checks(&settings, step).unwrap_or_else(|err| {
                        panic!("dpi={dpi} advanced={advanced} notice={notice} A-3b 失败：{err}")
                    });
                }
            }
        }
        // 负例：x 偏 8 px ⇒ 必须被咬住（同列同 x 项）
        let mut broken = plan(2.0, false, false, f32::INFINITY);
        let target = broken
            .items
            .iter_mut()
            .find(|item| item.id == IDC_FIELD_PORT)
            .expect("列 A 控件必须存在");
        target.x += 8;
        let step = if broken.compact {
            STEP_COMPACT_DIP
        } else {
            ROW_STEP
        };
        assert!(
            column_checks(&broken, step).is_err(),
            "负例：某一行 x 偏 8 px 必须被 A-3b 咬住"
        );
        // 负例 2：y 偏一格半 ⇒ 必须被咬住（等差格项）
        let mut broken_y = plan(2.0, false, false, f32::INFINITY);
        let target = broken_y
            .items
            .iter_mut()
            .find(|item| item.id == IDC_FIELD_PORT)
            .expect("列 A 控件必须存在");
        target.y += 8;
        assert!(
            column_checks(&broken_y, step).is_err(),
            "负例：某一行 y 偏 8 px 必须被 A-3b 咬住"
        );
        // 负例 3（**A-3b ⑥ 的强制负例**，I1b 登记项）：把提示条宽改成 `COL_FULL_W`
        //（不再减按钮宽与间隙）⇒ ⑥（提示条列位）必须报错。
        let mut broken_strip = plan(2.0, false, true, f32::INFINITY);
        let strip = broken_strip
            .items
            .iter_mut()
            .find(|item| item.column == Column::NoticeStrip)
            .expect("提示条必须存在（notice = true）");
        strip.w = (COL_FULL_W * 2.0).round() as i32; // 2.0 = 本 plan 的 scale（= 192 DPI）
        assert!(
            column_checks(&broken_strip, ROW_STEP).is_err(),
            "负例：提示条宽 = COL_FULL_W（未减按钮宽与间隙）必须被 A-3b ⑥ 咬住"
        );
    }

    /// `plan()` 的项数 == [`PLAN_ITEM_COUNT`]（`SettingsControls::all` 的定长依据）。
    /// 负例注入面 = 往 `SPECS` 加一行但忘了它 ⇒ 本断言必红（会表现为"停不掉的提示条"）。
    #[test]
    fn plan_item_count_matches_control_table() {
        for advanced in [false, true] {
            for notice in [false, true] {
                let settings = plan(2.0, advanced, notice, f32::INFINITY);
                assert_eq!(settings.items.len(), PLAN_ITEM_COUNT);
                // 每个控件 id 只出现一次（zip 对齐的前提）
                let mut ids: Vec<usize> = settings.items.iter().map(|item| item.id).collect();
                ids.sort_unstable();
                let before = ids.len();
                ids.dedup();
                assert_eq!(ids.len(), before, "plan() 里存在重复控件 id");
            }
        }
    }

    /// 列常量表自洽（`CLIENT_W_DIP` = 列 B 控件右缘 + `PAD_RIGHT`；列内不重叠）。
    #[test]
    fn column_table_is_consistent() {
        assert_eq!(COL_LABEL_A_X, PAD_LEFT);
        assert_eq!(COL_LABEL_B_X, COL_CTRL_A_X + COL_CTRL_A_W + COL_GAP);
        assert_eq!(COL_CTRL_B_X, COL_LABEL_B_X + COL_LABEL_B_W + COL_GAP);
        assert_eq!(CLIENT_W_DIP, COL_CTRL_B_X + COL_CTRL_B_W + PAD_RIGHT);
        assert_eq!(COL_FULL_W, CLIENT_W_DIP - COL_CTRL_A_X - PAD_RIGHT);
        // 分段槽宽必须容得下最宽的段标签（`debug` 实测 43 DIP + 余量）
        assert!(segment_widths(COL_CTRL_A_W, 4).iter().all(|w| *w >= 47.0));
    }

    fn overlaps(first: &PlanItem, second: &PlanItem) -> bool {
        first.x < second.x + second.w
            && second.x < first.x + first.w
            && first.y < second.y + second.h
            && second.y < first.y + first.h
    }

    /// **A-3（判据）**：4 档 DPI × {公开, 高级} × {无提示条, 有提示条} ⇒
    /// ① 每个带文本的控件：文本宽 ≤ 控件宽（分段用 `font_ui_bold` 量最坏情况）
    /// ② 可见矩形两两不重叠 ③ 全部 ⊂ 客户区。
    #[test]
    fn plan_holds_at_four_dpis() {
        for dpi in [96_u32, 120, 144, 192] {
            let theme = Theme::new(dpi);
            let hdc = unsafe { GetDC(None) };
            for advanced in [false, true] {
                for notice in [false, true] {
                    let settings = plan(theme.scale, advanced, notice, f32::INFINITY);
                    let items = visible_items(&settings);
                    for item in &items {
                        assert!(item.x >= 0 && item.y >= 0, "dpi={dpi} 负坐标：{item:?}");
                        assert!(
                            item.x + item.w <= settings.client_w
                                && item.y + item.h <= settings.client_h,
                            "dpi={dpi} advanced={advanced} notice={notice} 越出客户区：{item:?} \
                             client={}x{}",
                            settings.client_w,
                            settings.client_h
                        );
                        if item.text.is_empty() {
                            continue;
                        }
                        let font = if item.kind == PlanKind::Segment {
                            theme.font_ui_bold
                        } else {
                            theme.font_ui
                        };
                        let width = measure_text(hdc, font, item.text);
                        assert!(
                            width <= item.w,
                            "dpi={dpi} advanced={advanced} notice={notice} id={} 文本放不下：\
                             <{}> 需 {width} px / 可用 {} px",
                            item.id,
                            item.text,
                            item.w
                        );
                    }
                    for (index, first) in items.iter().enumerate() {
                        for second in items.iter().skip(index + 1) {
                            assert!(
                                !overlaps(first, second),
                                "dpi={dpi} advanced={advanced} notice={notice} 矩形重叠：\
                                 id {} {:?} vs id {} {:?}",
                                first.id,
                                (first.x, first.y, first.w, first.h),
                                second.id,
                                (second.x, second.y, second.w, second.h)
                            );
                        }
                    }
                }
            }
            unsafe { ReleaseDC(None, hdc) };
        }
    }

    /// **I1-7 结构判据**：展开前后高差 == 2 × 行步（60 DIP）；内容底余量恒 24 DIP。
    #[test]
    fn plan_keeps_row_step_and_bottom_margin() {
        for dpi in [96_u32, 120, 144, 192] {
            let theme = Theme::new(dpi);
            let scale = theme.scale;
            for notice in [false, true] {
                let public = plan(scale, false, notice, f32::INFINITY);
                let advanced = plan(scale, true, notice, f32::INFINITY);
                assert!(!public.compact && !advanced.compact && !public.pool_folded);
                let diff = (advanced.client_h - public.client_h) as f32;
                let expected = 2.0 * ROW_STEP * scale;
                assert!(
                    (diff - expected).abs() <= 1.0,
                    "dpi={dpi} notice={notice} 展开高差 {diff} px ≠ 2×行步 {expected} px"
                );
                for settings in [&public, &advanced] {
                    let margin = (settings.client_h - settings.content_bottom) as f32 / scale;
                    assert!(
                        (margin - BOTTOM_PAD_DIP).abs() <= 1.0,
                        "dpi={dpi} notice={notice} 内容底余量 {margin} DIP ≠ {BOTTOM_PAD_DIP} DIP"
                    );
                }
            }
        }
    }

    /// **`plan()` 的钳制分支**（I1-7）：屏幕放不下 ⇒ 行步 26 ⇒ 仍放不下 ⇒ 连接池组强制折叠。
    #[test]
    fn plan_degrades_when_work_area_is_small() {
        let scale = 2.0;
        let tall = plan(scale, true, false, f32::INFINITY);
        assert!(!tall.compact && !tall.pool_folded);

        // 只差一点点：正常档放不下、降档放得下 ⇒ 只降行步，不折叠
        let smaller = plan(scale, true, false, tall.client_h as f32 / scale - 1.0);
        assert!(smaller.compact, "放不下 ⇒ 行步必须降档");
        assert!(smaller.client_h < tall.client_h);
        assert!(!smaller.pool_folded, "降档后放得下 ⇒ 不该折叠连接池");
        assert!(
            smaller
                .items
                .iter()
                .any(|item| item.id == IDC_FIELD_POOL_MAX && item.visible),
            "降档档里连接池行仍须可见（高级展开）"
        );
        assert!(
            smaller
                .items
                .iter()
                .any(|item| item.id == IDC_FIELD_STRIP && item.visible),
            "降档不影响剥离前缀行"
        );

        // 极小屏：两档都放不下 ⇒ 连接池组强制折叠（其余行照旧）
        let tiny = plan(scale, true, false, 200.0);
        assert!(tiny.compact && tiny.pool_folded);
        assert!(
            !tiny
                .items
                .iter()
                .any(|item| item.id == IDC_FIELD_POOL_MAX && item.visible),
            "折叠后连接池两个字段都必须隐藏"
        );
        assert!(tiny
            .items
            .iter()
            .any(|item| item.id == IDC_FIELD_STRIP && item.visible));

        // 公开态在极小屏下没有可折叠的行（不 panic、不产生负尺寸）
        let tiny_public = plan(scale, false, false, 100.0);
        assert!(tiny_public.compact && !tiny_public.pool_folded);
        assert!(tiny_public.client_h > 0);
    }

    /// **A-15（容量断言）**：Arch-A §9-1/§9-2 的逐字串必须在结果行（**两行** × 行宽）内；
    /// 日志读数行的极端文案必须在读数行（满行）内。
    #[test]
    fn result_row_and_log_stat_hold_their_strings() {
        let theme = Theme::new(96);
        let settings = plan(1.0, false, false, f32::INFINITY); // scale = 1 ⇒ px == DIP
        let result_w = settings
            .items
            .iter()
            .find(|item| item.id == IDC_RESULT)
            .map(|item| item.w)
            .unwrap_or(0);
        let log_w = settings
            .items
            .iter()
            .find(|item| item.id == IDC_FIELD_LOG_STAT)
            .map(|item| item.w)
            .unwrap_or(0);
        assert!(result_w > 0 && log_w > 0);

        let applied = "已应用到本次运行（不写文件；关掉程序即回到默认）";
        let stopped_running =
            "已应用到本次运行（不写文件；关掉程序即回到默认）；服务当前是停止状态，改动将在下次启动时生效";
        let degraded = "已应用到本次运行（不写文件；关掉程序即回到默认）；端口 8000 被占用，\
                        已自动降级到 8001；应用侧 Base URL 请填 http://127.0.0.1:8001/v1";
        let rollback = "新配置未生效：监听 127.0.0.1:8000 失败：端口被占用（常见占用者：IIS / \
                        其它本地服务 / 上一实例未退出）；已回滚到原配置并继续服务";
        let hdc = unsafe { GetDC(None) };
        for text in [
            APPLY_START_TEXT,
            APPLY_DRAIN_TEXT,
            STOP_TEXT,
            STOPPED_TEXT,
            applied,
            stopped_running,
            degraded,
            rollback,
        ] {
            let width = measure_text(hdc, theme.font_ui, text);
            assert!(
                width <= result_w * 2,
                "结果行两行放不下：{text}（需 {width} px / 容量 {} px）",
                result_w * 2
            );
        }
        let extreme = log_stat_text(2000, LOG_RING_CAPACITY, 99_999_999);
        let width = measure_text(hdc, theme.font_ui, &extreme);
        assert!(
            width <= log_w,
            "日志读数行放不下：{extreme}（需 {width} px / 可用 {log_w} px）"
        );
        assert!(extreme.contains("2000") && extreme.contains("99999999"));
        unsafe { ReleaseDC(None, hdc) };
    }

    /// 结果行两行切分：两条都 ≤ 行宽；一行放得下 ⇒ 单行（否则"两行高 36"的容量判据是空话）。
    #[test]
    fn split_two_lines_never_exceeds_width() {
        let theme = Theme::new(96);
        let hdc = unsafe { GetDC(None) };
        let width = 360;
        let long = "新配置未生效：监听 127.0.0.1:8000 失败：端口被占用（常见占用者：IIS / \
                    其它本地服务 / 上一实例未退出）；已回滚到原配置并继续服务";
        let (first, second) = split_two_lines(hdc, theme.font_ui, long, width);
        assert!(measure_text(hdc, theme.font_ui, &first) <= width);
        let second = second.unwrap_or_default();
        assert!(!second.is_empty());
        assert!(measure_text(hdc, theme.font_ui, &second) <= width);
        // 短句 ⇒ 单行（无第二行）
        let (first, second) = split_two_lines(hdc, theme.font_ui, "已停止", width);
        assert_eq!(first, "已停止");
        assert!(second.is_none());
        // 零宽 ⇒ 空（不 panic、不越界）
        let (first, second) = split_two_lines(hdc, theme.font_ui, long, 0);
        assert!(first.is_empty() && second.is_none());
        unsafe { ReleaseDC(None, hdc) };
    }

    /// 范式⑤：`秒 × 1000` 的上界与溢出（`connect`/`first_byte` 保留秒但先校验再乘）。
    #[test]
    fn seconds_to_ms_bounds() {
        assert_eq!(seconds_to_ms(0, "首字节超时"), Ok(0)); // 0 = 不限（connect 的 ≥1 由调用点拦）
        assert_eq!(seconds_to_ms(86_400, "连接超时"), Ok(86_400_000));
        let too_big = seconds_to_ms(86_401, "连接超时").unwrap_err();
        assert!(too_big.contains("86400"), "报错必须点名上界：{too_big}");
        // 10^19：debug 档会 panic、release 档回绕 ⇒ 这里必须报错
        assert!(seconds_to_ms(10_000_000_000_000_000_000, "连接超时").is_err());
    }

    /// 新字段三例（范式⑤ 的输入面）：`0` 合法 / 非数字报错 / 超大值报错（`config.validate()` 收口）。
    #[test]
    fn new_field_validation_examples() {
        // 非数字：走与 `apply_fields` 同一套 `parse`。
        assert!("not-a-number".trim().parse::<u64>().is_err());
        let mut config = Config::default();
        // 0 合法（响应头超时 0 = 不限）
        config.timeouts.response_head_ms = 0;
        assert!(config.validate().is_ok());
        // 超大值报错（10^19 远超 3,600,000 上限）
        config.timeouts.response_head_ms = 10_000_000_000_000_000_000;
        assert!(config.validate().is_err());
        // 连接池两项的上限（1024 / 600,000 ms）同样由 validate 收口
        config.timeouts.response_head_ms = 120_000;
        config.pool.max_idle_per_host = 1025;
        assert!(config.validate().is_err());
        config.pool.max_idle_per_host = 16;
        config.pool.idle_timeout_ms = seconds_to_ms(600, "池空闲超时").unwrap_or(0);
        assert!(config.validate().is_ok());
        assert!(seconds_to_ms(601, "池空闲超时").is_ok());
    }

    /// 分段控件成员表（状态自持 + 反查完整）。
    #[test]
    fn segment_membership_is_complete() {
        assert_eq!(segment_member(IDC_SEG_CORS[0]), Some((SegGroup::Cors, 0)));
        assert_eq!(segment_member(IDC_SEG_CORS[1]), Some((SegGroup::Cors, 1)));
        assert_eq!(
            segment_member(IDC_SEG_LOG_LEVEL[3]),
            Some((SegGroup::LogLevel, 3))
        );
        assert_eq!(segment_member(IDC_SEG_CLOSE[1]), Some((SegGroup::Close, 1)));
        assert_eq!(segment_member(IDC_APPLY), None);
        assert_eq!(segment_member(IDC_FIELD_ADVANCED), None);
    }
}
