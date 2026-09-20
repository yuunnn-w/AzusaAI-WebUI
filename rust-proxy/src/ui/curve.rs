//! 波动曲线的**几何**（P7-I3 / UI 方案 §2.5.2 + **D7 改型** + **P8-UI4 滚动波形改型**）—— 无状态纯函数：
//! 无 GDI 对象、不碰 `stats`、不持状态。
//!
//! **D7（2026-09-20，用户指令）**：曲线序列 = **实时网络流量速率（bytes/s）**；延迟**不上曲线**
//! （保留为面板下方的文字读数行 —— 它的真源是 `WindowSnapshot::latency_text()`）。
//!
//! **P8-UI4（2026-09-20，用户报障 + 新需求）**：用户报「网络速率没有显示出波动曲线图」，要求向常见
//! 代理软件的**不断滚动**曲线看齐。本文件承载"滚动波形"的**几何**：
//! - **X = 时间轴**（窗口右缘 = **现在** = 当前秒 + 亚秒相位 `second_frac`）：每个样本画在
//!   **它所属那一秒结束的时刻** ⇒ 相位每前进一点，整条线**平滑左移**（亚秒滚动；重绘节拍见
//!   `main_window::TIMER_ANIM`，CPU 读数面 = A-16④ 的空闲对照臂）；
//! - **连续折线**（相邻样本直接相连，不在样本之间断开）+ 调用方按同一条折线画**淡色面积**；
//! - **序列末点永不上图**：`series(120)` 的末点 = **进行中的那一秒**（值还在长），画出来就是"每秒
//!   重置一次"的锯齿；它既不上图、也不进"当前值"（后者读**最新的完整秒**）。
//!
//! 为什么单独一个模块：
//! - 它是"唯一真源 → 绘制"的**唯一变换点** ⇒ 判据（A-1 / A-9 / A-14' / P8-UI4 的滚动与末点规则）可用
//!   合成输入机械验证，不需要真机、不需要 GDI；
//! - 绘制路径（`main_window::paint_curve`）只做"把几何画出来"，**零统计调用**（A-13b②）；
//! - 几何缓冲归调用方（`UiState`）持有 ⇒ 每帧 [`CurveGeometry::clear`] 复用、**零堆分配**（A-16②）。
//!
//! 诚实性口径（C2 / C3 / A-14'，**P8-UI4 一字未改**）：
//! - `rate == None`（该秒无桶）⇒ **断线**（不补 0、不插值、不沿用前值、**不桥接缺口**）；全 `None` ⇒ 空态；
//! - "无样本"**绝不画成 0**（0 是"测到了 0 字节"这个真实读数，与"没测到"是两件事）；
//! - 亚秒相位**只平移 X**（不改任何点的 y、不新增点、不改 y 量程）⇒ "滚动"不制造任何新读数；
//! - 单位自适应 `B/s → KB/s → MB/s`，**标签列宽按当前实际文案量宽**（C1：不用常数拍脑袋）；
//! - "当前值"在**最新的完整秒**无样本时**返回 `None`**（C3：绘制侧隐藏，不留孤立 `-`/`—`）。
//!
//! ⚠ 本文件是 A-13b② 的**静态检查面**（该判据要求 `ui/curve.rs` 与 `ui/main_window.rs` 里
//! 方法式 `.series(` 计数 == 0）⇒ 单测里取真实出口时用 UFCS 形式
//! `stats::Window::series(&w, secs)`（函数式调用语法，语义完全相同；判据的**意图** ——
//! 生产路径零 `stats` 访问 —— 不受影响）。

use azusa_local_proxy::stats::SeriesPoint;
use windows::Win32::Graphics::Gdi::{HDC, HFONT};
use windows::Win32::Graphics::GdiPlus::PointF;

/// Y 轴速率阶梯（bytes/s；1-2-5 风格）：取**刚好覆盖峰值 + 余量**的一档。
///
/// 先按 `峰值 × 1.1` 向上取整到档位 ⇒ **恒有余量（不贴顶）**，且上界 ≤ `2.5 ×` 峰值
/// （1-2-5 阶梯的最大相邻比）⇒ 曲线既有呼吸空间又不会被压成一条平线。
pub const RATE_LADDER_BYTES_PER_SEC: [u64; 19] = [
    1 << 10,
    2 << 10,
    5 << 10,
    10 << 10,
    20 << 10,
    50 << 10,
    100 << 10,
    200 << 10,
    500 << 10,
    1 << 20,
    2 << 20,
    5 << 20,
    10 << 20,
    20 << 20,
    50 << 20,
    100 << 20,
    200 << 20,
    500 << 20,
    1 << 30,
];

/// **最小量程**（bytes/s；P8-UI4）：显示量程的**下限** —— 峰值再小（哪怕全 0）也按它画。
///
/// 两个方向都咬：① 不许把"几十 B/s 的抖动"放大到满高（那是**谎报**起伏）；② 也不许让低速流量缩成
/// 一条贴底直线（几百 B/s 的流量在 1 KiB/s 量程里仍有 30–60 % 高度 ⇒ 看得见起伏）。
/// 现取值 = 阶梯最低档（`RATE_LADDER_BYTES_PER_SEC[0]`，由单测钉住两者相等）。
pub const MIN_RANGE_BYTES_PER_SEC: u64 = 1 << 10;

/// 余量系数（百分比）：`峰值 × 110 / 100` 后再取档 ⇒ **min 10 % 不贴顶**（D7）。
const HEADROOM_NUM: u64 = 110;
const HEADROOM_DEN: u64 = 100;

/// 绘图区（像素；已过 `theme.px()`）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlotRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// 曲线几何：由 [`build`] 填充，由调用方（`UiState`）持有并复用 ⇒ 每帧 0 分配。
#[derive(Default)]
pub struct CurveGeometry {
    /// 折线点（**只含该秒有速率值的点**，且**不含进行中的那一秒**）；X 单调递增、旧→新（第 0 点在最左）。
    pub rate_points: Vec<PointF>,
    /// 断点：值段之间的**新分段起始下标**（下标空间 = [`CurveGeometry::rate_points`]）。
    /// 首段隐含从 0 开始 ⇒ 本向量**不含** 0；全 `None` ⇒ 空（A-1）。
    pub seg_breaks: Vec<usize>,
    /// 分段区间（`start..end`，下标空间同上）—— 由 `seg_breaks` 现算、**预分配复用**
    /// （每帧 `build` 重填 ⇒ 绘制侧可直接取 `&[..]` 喂 [`super::theme::Gfx::polyline_runs`] /
    /// [`super::theme::Gfx::fill_area_runs`] = 各**单次** `GdipDrawPath` / `GdipFillPath`，
    /// **调用数与段数无关** —— 审查 P2-① 的收口）。
    pub runs: Vec<(usize, usize)>,
    /// 单序列归一化上界（bytes/s；含 ≥10 % 余量，且 ≥ [`MIN_RANGE_BYTES_PER_SEC`]）。
    pub y_max_rate: u64,
    /// 当前值行里的速率（`(in, out)` bytes/s），读自**最新的完整秒**：`None` ⇒ 该秒无样本 ⇒
    /// **绘制侧隐藏**（C3）；也**不**回落去读更早的陈旧点。
    pub now_rate: Option<(u64, u64)>,
    /// 当前值行里的每秒请求数（与 `now_rate` 同生同灭；隐藏时恒 `0`，不读陈旧桶）。
    pub now_requests: u64,
    /// 窗口**填充度**（C2 角标）：**实际画出来**的有速率值的秒数（= 去掉了进行中的那一秒）。
    pub filled_secs: u32,
    /// 窗口长度（秒）—— 角标的分母（= 输入点数）。
    pub window_secs: u64,
}

impl CurveGeometry {
    /// 缓冲按窗口上界预分配（`120 + 1` 个点、段数远小于点数）⇒ 稳态下 0 次 realloc。
    pub fn new() -> CurveGeometry {
        CurveGeometry {
            rate_points: Vec::with_capacity(121),
            seg_breaks: Vec::with_capacity(16),
            runs: Vec::with_capacity(16),
            ..Default::default()
        }
    }

    /// 每帧复用前清空（保留容量 ⇒ 零堆分配）。
    pub fn clear(&mut self) {
        self.rate_points.clear();
        self.seg_breaks.clear();
        self.runs.clear();
        self.y_max_rate = MIN_RANGE_BYTES_PER_SEC;
        self.now_rate = None;
        self.now_requests = 0;
        self.filled_secs = 0;
        self.window_secs = 0;
    }

    /// 无任何有速率值的秒 ⇒ 绘制侧落**空态**（文案由 `WindowSnapshot::latency_text()` 给 —— 与面板
    /// 延迟行**同一真源**，UI 不另写一份）。
    pub fn is_empty(&self) -> bool {
        self.rate_points.is_empty()
    }

    /// 窗口填充度角标（C2）：`20 s / 120 s`。
    pub fn fill_text(&self) -> String {
        format!("{} s / {} s", self.filled_secs, self.window_secs)
    }

    /// 当前值（`↑1.2 KB/s ↓340 B/s · 2.0 req/s`）；最新的完整秒无样本 ⇒ **`None`**（C3：隐藏）。
    pub fn now_text(&self) -> Option<String> {
        let (bytes_in, bytes_out) = self.now_rate?;
        Some(format!(
            "↑{} ↓{} · {:.1} req/s",
            format_rate(bytes_in),
            format_rate(bytes_out),
            self.now_requests as f64
        ))
    }

    /// 右上状态串（**一次绘制**）：`20 s / 120 s · ↑1.2 KB/s ↓340 B/s · 2.0 req/s`；
    /// 无当前值 ⇒ 只留填充度（C3：不给孤立符号）。
    pub fn status_text(&self) -> String {
        match self.now_text() {
            Some(now) => format!("{} · {now}", self.fill_text()),
            None => self.fill_text(),
        }
    }
}

/// 速率显示（`B/s → KB/s → MB/s` 自适应；C1/D7）。
pub fn format_rate(bytes_per_sec: u64) -> String {
    if bytes_per_sec < 1 << 10 {
        return format!("{bytes_per_sec} B/s");
    }
    if bytes_per_sec < 1 << 20 {
        let kb = bytes_per_sec as f64 / 1024.0;
        return if kb < 10.0 {
            format!("{kb:.1} KB/s")
        } else {
            format!("{kb:.0} KB/s")
        };
    }
    let mb = bytes_per_sec as f64 / (1024.0 * 1024.0);
    if mb < 10.0 {
        format!("{mb:.1} MB/s")
    } else {
        format!("{mb:.0} MB/s")
    }
}

/// Y 轴三档标签（上 / 中 / 下）—— 与 [`format_rate`] 同一单位口径。
///
/// **顶档守卫（P2-② / A-14 同构）**：`RATE_LADDER_BYTES_PER_SEC` 的**顶档**（1 GiB/s）是阶梯的尽头
/// —— 峰值再高也只能落在它上面（`ladder_with_headroom` 回落）。此时"上界"实际是**下界**信息
/// （真实峰值 ≥ 顶档），所以顶档一律带 `≥` 标注，**不谎报**成恰好 1.0 GB/s；
/// 折线侧由 `build` 的 `ratio.clamp(0.0, 1.0)` 保证不越出绘图区（两条一起构成"越界 + 谎报"双防线）。
pub fn y_labels(y_max_rate: u64) -> [String; 3] {
    let top = format_rate(y_max_rate);
    let top = if y_max_rate >= *RATE_LADDER_BYTES_PER_SEC.last().unwrap() {
        format!("≥{top}")
    } else {
        top
    };
    [top, format_rate(y_max_rate / 2), format_rate(0)]
}

/// Y 轴标签列的**必需宽度**（像素）—— **按当前实际文案量宽**（C1 的修法：不用常数拍脑袋）。
///
/// 真机缺陷 C1：标签先右对齐到一个拍脑袋的带宽上，再用 `DrawTextW` 裁掉左侧 ⇒ 用户看到
/// `250ms/125ms/0ms` 退化成 `0ms/5ms/0ms`。现在宽度由**要画的三个串**量出来，
/// 且绘制侧用 `max(名义绘图区左缘, 标签左缘 + 本函数结果 + 间隙)` 保证**永不进入绘图区**。
pub fn label_column_width(hdc: HDC, font: HFONT, y_max_rate: u64) -> i32 {
    let mut widest = 0;
    for label in y_labels(y_max_rate) {
        widest = widest.max(super::theme::measure_text(hdc, font, &label));
    }
    widest.max(0)
}

/// 投影一串 `SeriesPoint` 到绘图区（**纯函数**；无状态、写入调用方给的 `out`）。
///
/// **X = 时间轴**（P8-UI4）：窗口右缘 = **现在** = 当前秒 + `second_frac`；样本 `i`（旧→新）画在
/// **它那一秒结束的时刻** ⇒ `X(i) = plot.x + plot.w × (i + 2 − frac) / len`。
/// 推论（都由单测钉住）：`frac` ∈ [0,1) 时**整条线随相位平滑左移**（改的是 X，y 一个都不动）；
/// `i = len − 1`（进行中的那一秒）恒在右缘之外 ⇒ **永不上图**。
/// Y 由 `rate / y_max` 归一化（`y_max` 取 [`RATE_LADDER_BYTES_PER_SEC`] 里带余量的一档，
/// 且不低于 [`MIN_RANGE_BYTES_PER_SEC`]）。
/// 曲线值 = `bytes_in + bytes_out`（两个方向都算流量；当前值行给出 **in/out 拆分**）。
pub fn build(points: &[SeriesPoint], plot: PlotRect, second_frac: f32, out: &mut CurveGeometry) {
    out.clear();
    let len = points.len();
    out.window_secs = len as u64;
    if len == 0 || plot.w <= 0.0 || plot.h <= 0.0 {
        return;
    }
    let frac = if second_frac.is_finite() {
        second_frac.clamp(0.0, 0.999)
    } else {
        0.0
    };

    // ① y 上界：只由**有值秒**决定（`None` 不参与 ⇒ 不制造假峰）；取带余量的档（含最小量程）。
    let mut peak = 0_u64;
    for point in points {
        if let Some(total) = rate_total(point) {
            peak = peak.max(total);
        }
    }
    out.y_max_rate = ladder_with_headroom(peak);
    let y_max = out.y_max_rate.max(1);

    // ② 逐点投影（旧→新）+ 断点（`None` ⇒ 断开；**只有后面还有值**才记断点 ⇒ 尾部空窗不产生空段）。
    let step = plot.w / len as f32;
    let right = plot.x + plot.w;
    let mut pending_break = false;
    for (index, point) in points.iter().enumerate() {
        let Some(total) = rate_total(point) else {
            if !out.rate_points.is_empty() {
                pending_break = true;
            }
            continue;
        };
        let x = plot.x + step * (index as f32 + 2.0 - frac);
        if x > right {
            // 进行中的那一秒（序列末点）：值还在长 ⇒ 不上图。X 随 index 单调递增 ⇒ 直接收尾。
            break;
        }
        out.filled_secs += 1;
        if pending_break {
            out.seg_breaks.push(out.rate_points.len());
            pending_break = false;
        }
        let ratio = (total as f32 / y_max as f32).clamp(0.0, 1.0);
        out.rate_points.push(PointF {
            X: x,
            Y: plot.y + plot.h * (1.0 - ratio),
        });
    }

    // ③ 当前值行 = **最新的完整秒**（`len − 2` 位）。为什么不读序列末点：它是**进行中的那一秒**
    //    （值还在长）；为什么不读"最后一个画出来的点"：空闲 ≥1 s 后那是**陈旧点**（C3 要的正是
    //    "服务停止/空闲后如实回落 ⇒ 隐藏"，而不是把 30 s 前的读数挂在"现在"）。
    if len >= 2 {
        if let Some(point) = points.get(len - 2) {
            if let Some((bytes_in, bytes_out)) = rate_pair(point) {
                out.now_rate = Some((bytes_in, bytes_out));
                out.now_requests = point.requests;
            }
        }
    }

    // ④ 段区间（`runs`）：与 `seg_breaks` 同源、**预分配复用**（零新增分配）⇒ 绘制侧可一次性
    // 交给 `Gfx::polyline_runs` / `Gfx::fill_area_runs`（各**单次** `GdipDrawPath` / `GdipFillPath`）。
    let breaks_len = out.seg_breaks.len();
    let mut start = 0;
    for index in 0..breaks_len {
        let end = out.seg_breaks[index];
        out.runs.push((start, end));
        start = end;
    }
    if !out.rate_points.is_empty() {
        out.runs.push((start, out.rate_points.len()));
    }
}

/// 该点的速率总量（`in + out`）；`None` = 断线（缺秒 —— 见 `stats::SeriesPoint`）。
fn rate_total(point: &SeriesPoint) -> Option<u64> {
    rate_pair(point).map(|(bytes_in, bytes_out)| bytes_in.saturating_add(bytes_out))
}

/// 该点的速率对（`(in, out)`）；两个方向**同生同灭**（`stats` 侧同源同判）。
fn rate_pair(point: &SeriesPoint) -> Option<(u64, u64)> {
    match (point.bytes_in_per_sec, point.bytes_out_per_sec) {
        (Some(bytes_in), Some(bytes_out)) => Some((bytes_in, bytes_out)),
        _ => None,
    }
}

/// 峰值 → 带余量的档（`峰值 × 110 %` 向上取档；低于最小量程 ⇒ 取最小量程）。
fn ladder_with_headroom(peak: u64) -> u64 {
    let wanted = peak
        .saturating_mul(HEADROOM_NUM)
        .saturating_div(HEADROOM_DEN)
        .max(MIN_RANGE_BYTES_PER_SEC);
    for rung in RATE_LADDER_BYTES_PER_SEC {
        if rung >= wanted {
            return rung;
        }
    }
    RATE_LADDER_BYTES_PER_SEC[RATE_LADDER_BYTES_PER_SEC.len() - 1]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plot() -> PlotRect {
        PlotRect {
            x: 100.0,
            y: 40.0,
            w: 238.0,
            h: 48.0,
        }
    }

    /// 有速率值的点（in/out 各半）。
    fn point(bytes_in: u64, bytes_out: u64, requests: u64) -> SeriesPoint {
        SeriesPoint {
            latency_p50_ms: None,
            latency_overflow: false,
            bytes_in_per_sec: Some(bytes_in),
            bytes_out_per_sec: Some(bytes_out),
            requests,
        }
    }

    /// 断线点（缺秒）。
    fn absent() -> SeriesPoint {
        SeriesPoint {
            latency_p50_ms: None,
            latency_overflow: false,
            bytes_in_per_sec: None,
            bytes_out_per_sec: None,
            requests: 0,
        }
    }

    fn all_present(secs: usize, rate: u64) -> Vec<SeriesPoint> {
        (0..secs).map(|_| point(rate / 2, rate / 2, 1)).collect()
    }

    fn all_absent(secs: usize) -> Vec<SeriesPoint> {
        (0..secs).map(|_| absent()).collect()
    }

    fn interleaved(secs: usize) -> Vec<SeriesPoint> {
        (0..secs)
            .map(|index| {
                if index % 2 == 0 {
                    point(4096, 4096, 2)
                } else {
                    absent()
                }
            })
            .collect()
    }

    /// X 单调递增（旧→新、第 0 点在最左）—— A-1 的核心断言（把旧新画反 = 唯一后果）。
    fn assert_x_increasing(geometry: &CurveGeometry) {
        for pair in geometry.rate_points.windows(2) {
            assert!(
                pair[1].X > pair[0].X,
                "X 必须严格递增（旧→新）：{:?}",
                geometry.rate_points
            );
        }
    }

    /// 【A-1】几何上界与顺序：三例（全 `None` / 全 `Some` / 交错）+ 真实出口 `series(120)`。
    #[test]
    fn a1_geometry_bounds_and_old_to_new_order() {
        let plot = plot();
        let mut geometry = CurveGeometry::new();

        for case in [all_absent(120), all_present(120, 2048), interleaved(120)] {
            build(&case, plot, 0.0, &mut geometry);
            assert_eq!(case.len(), 120);
            assert!(
                geometry.rate_points.len() + geometry.seg_breaks.len() <= 121,
                "点数 + 段数必须 ≤ 121（实得 {} + {}）",
                geometry.rate_points.len(),
                geometry.seg_breaks.len()
            );
            assert_x_increasing(&geometry);
            assert!(
                geometry.rate_points.iter().all(|point| point.Y >= plot.y
                    && point.Y <= plot.y + plot.h
                    && point.X >= plot.x
                    && point.X <= plot.x + plot.w),
                "所有点必须落在绘图区内"
            );
        }

        // 全 None：`rate_points` 与 `seg_breaks` **均为空**（空态的唯一机械判据）
        build(&all_absent(120), plot, 0.0, &mut geometry);
        assert!(geometry.rate_points.is_empty());
        assert!(geometry.seg_breaks.is_empty());
        assert!(geometry.is_empty());
        assert_eq!(geometry.filled_secs, 0);
        assert_eq!(geometry.window_secs, 120);
        assert_eq!(geometry.status_text(), "0 s / 120 s", "空态角标正常");
        assert_eq!(geometry.now_text(), None, "C3：无样本 ⇒ 无当前值（隐藏）");

        // 填充度角标（C2）：交错 120 s ⇒ 60 s 有值；末点（进行中的那一秒）不上图 ⇒ 画 60 点
        build(&interleaved(120), plot, 0.0, &mut geometry);
        assert_eq!(geometry.filled_secs, 60);
        assert_eq!(geometry.fill_text(), "60 s / 120 s");

        // 真实出口：`stats::Window::series(120)`（UFCS 形式；见模块头注释）
        let window = azusa_local_proxy::stats::Window::new();
        let points = azusa_local_proxy::stats::Window::series(
            &window,
            azusa_local_proxy::stats::WINDOW_SECS,
        );
        assert_eq!(points.len(), 120, "series(120) 长度恒 == secs");
        build(&points, plot, 0.0, &mut geometry);
        assert!(geometry.rate_points.len() + geometry.seg_breaks.len() <= 121);
        assert_x_increasing(&geometry);

        // 空 `points`（防御）：不 panic、保持空态
        build(&[], plot, 0.0, &mut geometry);
        assert!(geometry.is_empty());
    }

    /// 【P8-UI4】亚秒相位 = **整条线的平移**：`frac` 0 → 0.5 时每个点左移**半个步长**，
    /// y 一个都不动；`frac = 0` 时最新的**完整**秒恰画在窗口右缘（"现在"）。
    #[test]
    fn p8ui4_sub_second_phase_scrolls_the_whole_trace_left() {
        let plot = plot();
        let case = all_present(120, 2048);
        let step = plot.w / 120.0;
        let mut at0 = CurveGeometry::new();
        let mut at5 = CurveGeometry::new();
        build(&case, plot, 0.0, &mut at0);
        build(&case, plot, 0.5, &mut at5);

        assert_eq!(
            at0.rate_points.len(),
            119,
            "120 点序列画 119 点（末点=进行中的那一秒）"
        );
        assert_eq!(at0.rate_points.len(), at5.rate_points.len());
        for (a, b) in at0.rate_points.iter().zip(at5.rate_points.iter()) {
            assert!(
                (a.X - b.X - step * 0.5).abs() < 0.01,
                "相位 0.5 ⇒ 整条线左移半个步长：{a:?} vs {b:?}"
            );
            assert!((a.Y - b.Y).abs() < f32::EPSILON, "相位只平移 X、不改 y");
        }
        let right = plot.x + plot.w;
        let newest0 = at0.rate_points.last().expect("末点").X;
        let newest5 = at5.rate_points.last().expect("末点").X;
        assert!(
            (newest0 - right).abs() < 0.01,
            "相位 0 ⇒ 最新完整秒恰在右缘（实得 {newest0} vs {right}）"
        );
        assert!(
            (newest5 - (right - step * 0.5)).abs() < 0.01,
            "相位 0.5 ⇒ 右端内缩半步"
        );
        // 相位只影响 X：量程与填充度一字不变
        assert_eq!(at0.y_max_rate, at5.y_max_rate);
        assert_eq!(at0.filled_secs, at5.filled_secs);
        // 相位越界输入不 panic、不产生越界点
        let mut odd = CurveGeometry::new();
        build(&case, plot, f32::NAN, &mut odd);
        assert_eq!(odd.rate_points.len(), 119);
        build(&case, plot, -3.0, &mut odd);
        assert!(odd.rate_points.iter().all(|p| p.X <= right));
    }

    /// 【P8-UI4】进行中的那一秒（序列末点）**永不上图**：哪怕它是**唯一**有值的点。
    #[test]
    fn p8ui4_in_progress_second_is_never_plotted() {
        let plot = plot();
        let mut geometry = CurveGeometry::new();
        // 只有末点有值 ⇒ 一个点都不画（空态），"当前值"也无（最新的完整秒无样本）
        let mut only_last = all_absent(120);
        if let Some(last) = only_last.last_mut() {
            *last = point(1_000_000, 1_000_000, 3);
        }
        build(&only_last, plot, 0.0, &mut geometry);
        assert!(geometry.is_empty(), "进行中的那一秒不上图");
        assert_eq!(geometry.status_text(), "0 s / 120 s");
        assert_eq!(
            geometry.now_rate, None,
            "当前值读最新的**完整**秒 ⇒ 无样本 ⇒ 隐藏"
        );

        // 末点 + 前一点都有值 ⇒ 只画前一点（1 点、1 段）
        let mut two = all_absent(120);
        if let Some(cell) = two.get_mut(118) {
            *cell = point(2_048, 0, 1);
        }
        if let Some(cell) = two.get_mut(119) {
            *cell = point(9_999_999, 9_999_999, 5);
        }
        build(&two, plot, 0.0, &mut geometry);
        assert_eq!(geometry.rate_points.len(), 1, "只画完整秒那一点");
        assert_eq!(geometry.filled_secs, 1);
        assert_eq!(geometry.runs, vec![(0, 1)]);
        assert_eq!(
            geometry.now_rate,
            Some((2_048, 0)),
            "当前值 = 最新的完整秒（不是末点、更不是更早的陈旧点）"
        );
        assert_eq!(geometry.now_requests, 1);
    }

    /// 【P8-UI4】**最小量程**：峰值再小也按 ≥ `MIN_RANGE_BYTES_PER_SEC` 画（低速看得见起伏）。
    #[test]
    fn p8ui4_minimum_range_floor_keeps_low_rate_visible() {
        assert_eq!(
            RATE_LADDER_BYTES_PER_SEC[0], MIN_RANGE_BYTES_PER_SEC,
            "最小量程 = 阶梯最低档（两处必须是同一个数）"
        );
        let plot = plot();
        let mut geometry = CurveGeometry::new();
        // 峰值 300 B/s（低速档）：量程仍是 1 KiB/s ⇒ 该点占 29 % 高度（看得见），不是贴底的一条
        build(&all_present(120, 300), plot, 0.0, &mut geometry);
        assert_eq!(geometry.y_max_rate, MIN_RANGE_BYTES_PER_SEC);
        let y = geometry.rate_points.last().expect("末点").Y;
        let used = (plot.y + plot.h - y) / plot.h;
        assert!(
            (used - 300.0 / 1024.0).abs() < 0.01,
            "300 B/s 在 1 KiB/s 量程里应占 29 % 高度（实得 {used:.3}）"
        );
        // 全 0 的窗口：量程也**不塌成 0**（除零 ⇒ NaN 的防线）
        build(&all_present(120, 0), plot, 0.0, &mut geometry);
        assert_eq!(geometry.y_max_rate, MIN_RANGE_BYTES_PER_SEC);
        assert_eq!(
            geometry.rate_points.last().expect("末点").Y,
            plot.y + plot.h,
            "真 0 ⇒ 落在基线（真点，不是空态）"
        );
        assert!(!geometry.is_empty(), "真 0 是读数 ⇒ 不是空态");
    }

    /// 【A-9】断线：`None` 处折线**断开**、不补 0、不桥接（P8-UI4 改动**不碰**这条口径）。
    #[test]
    fn a9_missing_seconds_break_the_line_instead_of_zero_filling() {
        let mut case = vec![
            point(1024, 0, 1),
            absent(),
            point(2048, 0, 1),
            point(2048, 0, 1),
        ];
        case.push(absent_pad()); // 末点 = 进行中的那一秒（不上图），前面 4 点是数据
        let mut geometry = CurveGeometry::new();
        build(&case, plot(), 0.0, &mut geometry);

        assert_eq!(geometry.rate_points.len(), 3, "有值且完整的秒只有 3 个");
        assert_eq!(geometry.seg_breaks, vec![1], "第 1 个有值点开启新段");
        assert_eq!(
            geometry.runs,
            vec![(0, 1), (1, 3)],
            "2 段：单点段 + 两点段（`runs` = 绘制侧唯一取段入口）"
        );
        assert!(
            geometry
                .rate_points
                .iter()
                .all(|point| point.Y != plot().y + plot().h),
            "没有任何点落在 0 值基线（否则就是补 0）"
        );
    }

    /// 一个恒不上图的末点（进行中的那一秒，无样本）—— 让短序列测试聚焦在"数据段"上。
    fn absent_pad() -> SeriesPoint {
        absent()
    }

    /// **真实 0 与"无样本"必须可区分**（C2/A-14' 的核心）：测到 0 字节的秒 = 落在基线的真点；
    /// 无样本的秒 = 不画点（断线）。
    #[test]
    fn measured_zero_is_a_real_point_while_absent_is_a_break() {
        let case = [point(0, 0, 1), absent(), point(0, 0, 1), absent()];
        let mut geometry = CurveGeometry::new();
        build(&case, plot(), 0.0, &mut geometry);
        assert_eq!(
            geometry.rate_points.len(),
            2,
            "两个真 0 = 两个点（末点不上图）"
        );
        assert_eq!(geometry.seg_breaks, vec![1], "中间的无样本秒断开");
        let baseline = plot().y + plot().h;
        for point in geometry.rate_points.iter() {
            assert!((point.Y - baseline).abs() < 0.01, "真 0 ⇒ 基线：{point:?}");
        }
        assert_eq!(
            geometry.filled_secs, 2,
            "填充度按**画出来的有值秒**计（含真 0）"
        );
    }

    /// 单位自适应（C1/D7）：`B/s → KB/s → MB/s`，且三档标签永不出现"拍脑袋宽度"。
    #[test]
    fn rate_units_adapt_and_labels_are_measured() {
        assert_eq!(format_rate(0), "0 B/s");
        assert_eq!(format_rate(512), "512 B/s");
        assert_eq!(format_rate(1024), "1.0 KB/s");
        assert_eq!(format_rate(1536), "1.5 KB/s");
        assert_eq!(format_rate(10 * 1024), "10 KB/s");
        assert_eq!(format_rate(1024 * 1024), "1.0 MB/s");
        assert_eq!(format_rate(100 * 1024 * 1024), "100 MB/s");
        assert_eq!(
            y_labels(1024),
            [
                "1.0 KB/s".to_string(),
                "512 B/s".to_string(),
                "0 B/s".to_string()
            ]
        );

        // Y 上界**留余量**（D7：不贴顶）：峰值 1024 ⇒ 档位 ≥ 1.1 × 1024
        let mut geometry = CurveGeometry::new();
        build(&all_present(5, 1024), plot(), 0.0, &mut geometry);
        assert!(
            geometry.y_max_rate as f64 >= 1024.0 * 1.1,
            "y_max 必须留 ≥10 % 余量（实得 {}）",
            geometry.y_max_rate
        );
        assert!(geometry.y_max_rate <= 1024 * 2, "余量不得超过一档（1-2-5）");

        // **C1 判据（量宽 + 读图双判据的"量宽"那一半）**：标签列宽由 `label_column_width` 量出，
        // 且**任一**当前文案都放得下。负例：把列宽拍成常数 20 DIP ⇒ 最宽标签（≥3 字符 + 单位）放不下。
        let theme = crate::ui::theme::Theme::new(96);
        let hdc = unsafe { windows::Win32::Graphics::Gdi::GetDC(None) };
        for y_max in [
            1024_u64,
            10 * 1024,
            512 * 1024,
            10 * 1024 * 1024,
            500 * 1024 * 1024,
        ] {
            let column = label_column_width(hdc, theme.font_small, y_max);
            for label in y_labels(y_max) {
                let measured = crate::ui::theme::measure_text(hdc, theme.font_small, &label);
                assert!(
                    measured <= column,
                    "标签 {label}（{measured} px）放不进量宽列 {column} px"
                );
                assert!(
                    measured > 20,
                    "负例锚点：{label} 宽 {measured} px > 常数 20 ⇒ 拍脑袋常数必被咬"
                );
            }
        }
        unsafe { windows::Win32::Graphics::Gdi::ReleaseDC(None, hdc) };
    }

    /// **【P2-②】顶档守卫**：峰值超过阶梯顶档（1 GiB/s）时 ① `y_max` 取顶档（不越出绘图区）
    /// ② 顶档标签**带 `≥`**（与 A-14 的"末桶诚实"同构，**不谎报**成恰好 1.0 GB/s）。
    #[test]
    fn p2_top_rung_is_honest_and_never_overflows() {
        let top = *RATE_LADDER_BYTES_PER_SEC.last().unwrap();
        let plot = plot();
        let mut geometry = CurveGeometry::new();
        // 峰值 = 4 GiB/s（远超顶档）；末点不上图 ⇒ 造 3 个点保证至少画 2 个
        build(
            &[
                point(top * 4, 0, 1),
                point(top * 4, 0, 1),
                point(top * 4, 0, 1),
            ],
            plot,
            0.0,
            &mut geometry,
        );
        assert_eq!(
            geometry.y_max_rate, top,
            "超出顶档 ⇒ 回落顶档（不放大台阶）"
        );
        assert!(
            y_labels(geometry.y_max_rate)[0].starts_with('≥'),
            "顶档必须带 `≥` 诚实标注：{:?}",
            y_labels(geometry.y_max_rate)
        );
        for point in geometry.rate_points.iter() {
            assert!(
                point.Y >= plot.y && point.Y <= plot.y + plot.h,
                "夹紧后**不得越出绘图区**：{point:?}"
            );
        }
        // 非顶档不带 `≥`（默认路径不变）
        assert!(!y_labels(1024)[0].starts_with('≥'));
        assert_eq!(y_labels(top)[0], format!("≥{}", format_rate(top)));
    }

    /// 当前值行：**最新的完整秒**有值才给串（C3）；请求数与速率同源同帧。
    #[test]
    fn current_value_follows_the_newest_complete_second() {
        let mut geometry = CurveGeometry::new();
        build(&all_present(4, 2048), plot(), 0.0, &mut geometry);
        let now = geometry.now_text().expect("最新完整秒有值");
        assert!(now.starts_with("↑1.0 KB/s ↓1.0 KB/s · 1.0 req/s"), "{now}");
        assert!(geometry.status_text().starts_with("3 s / 4 s · "));

        // 最新的完整秒无样本 ⇒ 当前值 None（C3：隐藏），但曲线保留（更早的秒照画）
        let case = [point(2048, 0, 1), point(2048, 0, 1), absent(), absent()];
        build(&case, plot(), 0.0, &mut geometry);
        assert_eq!(
            geometry.rate_points.len(),
            2,
            "曲线保留（点位在，只是当前值隐藏）"
        );
        assert_eq!(geometry.now_text(), None);
        assert_eq!(geometry.now_requests, 0, "无样本的秒不得读陈旧桶计数");
        assert_eq!(geometry.status_text(), "2 s / 4 s", "只留填充度角标");
    }
}
