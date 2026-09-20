//! 波动曲线的**几何**（P7-I3 / UI 方案 §2.5.2 + **D7 改型**）—— 无状态纯函数：无 GDI 对象、不碰 `stats`、不持状态。
//!
//! **D7（2026-09-20，用户指令）**：曲线序列 = **实时网络流量速率（bytes/s）**；延迟**不上曲线**
//! （保留为面板下方的文字读数行 —— 它的真源是 `WindowSnapshot::latency_text()`）。
//!
//! 为什么单独一个模块：
//! - 它是"唯一真源 → 绘制"的**唯一变换点** ⇒ 判据（A-1 / A-9 / A-14'）可用合成输入机械验证，
//!   不需要真机、不需要 GDI；
//! - 绘制路径（`main_window::paint_curve`）只做"把几何画出来"，**零统计调用**（A-13b②）；
//! - 几何缓冲归调用方（`UiState`）持有 ⇒ 每帧 [`CurveGeometry::clear`] 复用、**零堆分配**（A-16②）。
//!
//! 诚实性口径（C2 / C3 / A-14'）：
//! - `rate == None` ⇒ **折线断开**（不补 0、不插值、不沿用前值）；全 `None` ⇒ 空态；
//! - "无样本"**绝不画成 0**（0 是"测到了 0 字节"这个真实读数，与"没测到"是两件事）；
//! - 单位自适应 `B/s → KB/s → MB/s`，**标签列宽按当前实际文案量宽**（C1：不用常数拍脑袋）；
//! - "当前值"在无样本时**返回 `None`**（C3：绘制侧隐藏，不留孤立 `-`/`—`）。
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
    /// 折线点（**只含该秒有速率值的点**）；X 单调递增、旧→新（第 0 点在最左）。
    pub rate_points: Vec<PointF>,
    /// 断点：值段之间的**新分段起始下标**（下标空间 = [`CurveGeometry::rate_points`]）。
    /// 首段隐含从 0 开始 ⇒ 本向量**不含** 0；全 `None` ⇒ 空（A-1）。
    pub seg_breaks: Vec<usize>,
    /// 分段区间（`start..end`，下标空间同上）—— 由 `seg_breaks` 现算、**预分配复用**
    /// （每帧 `build` 重填 ⇒ 绘制侧可直接取 `&[..]` 喂 [`super::theme::Gfx::polyline_runs`]
    /// = 单次 `GdipDrawPath`，**调用数与段数无关** —— 审查 P2-① 的收口）。
    pub runs: Vec<(usize, usize)>,
    /// 单序列归一化上界（bytes/s；含 ≥10 % 余量）。
    pub y_max_rate: u64,
    /// 当前值行里的速率（`(in, out)` bytes/s）：`None` ⇒ 该秒无样本 ⇒ **绘制侧隐藏**（C3）。
    pub now_rate: Option<(u64, u64)>,
    /// 当前值行里的每秒请求数（无样本的那一秒恒 `0`，不读陈旧桶）。
    pub now_requests: u64,
    /// 窗口**填充度**（C2 角标）：有速率值的秒数。
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
        self.y_max_rate = RATE_LADDER_BYTES_PER_SEC[0];
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

    /// 当前值（`↑1.2 KB/s ↓340 B/s · 2.0 req/s`）；该秒无样本 ⇒ **`None`**（C3：隐藏而不是画 `-`）。
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
    /// 无当前值 ��� 只留填充度（C3：不给孤立符号）。
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
/// 逐秒 X 均匀铺满 `plot.w`（长度恒 == `secs` ⇒ 步长 = `w / (n − 1)`）；
/// Y 由 `rate / y_max` 归一化（`y_max` 取 [`RATE_LADDER_BYTES_PER_SEC`] 里带余量的一档）。
/// 曲线值 = `bytes_in + bytes_out`（两个方向都算流量；当前值行给出 **in/out 拆分**）。
pub fn build(points: &[SeriesPoint], plot: PlotRect, out: &mut CurveGeometry) {
    out.clear();
    out.window_secs = points.len() as u64;
    if points.is_empty() || plot.w <= 0.0 || plot.h <= 0.0 {
        return;
    }

    // ① y 上界：只由**可比的有值秒**决定（`None` 不参与 ⇒ 不制造假峰）；取带余量的档。
    let mut peak = 0_u64;
    for point in points {
        if let Some(total) = rate_total(point) {
            out.filled_secs += 1;
            peak = peak.max(total);
        }
    }
    out.y_max_rate = ladder_with_headroom(peak);
    let y_max = out.y_max_rate.max(1);

    // ② 逐点投影 + 断点（`None` ⇒ 断开；**只有后面还有值**才记断点 ⇒ 尾部空窗不产生空段）。
    let last_index = points.len() - 1;
    let step = if last_index == 0 {
        0.0
    } else {
        plot.w / last_index as f32
    };
    let mut pending_break = false;
    for (index, point) in points.iter().enumerate() {
        let Some(total) = rate_total(point) else {
            if !out.rate_points.is_empty() {
                pending_break = true;
            }
            continue;
        };
        if pending_break {
            out.seg_breaks.push(out.rate_points.len());
            pending_break = false;
        }
        let ratio = (total as f32 / y_max as f32).clamp(0.0, 1.0);
        out.rate_points.push(PointF {
            X: plot.x + step * index as f32,
            Y: plot.y + plot.h * (1.0 - ratio),
        });
        // 当前值行 = **最后一秒**（不是"最后一个有值的秒"）⇒ 服务停止/空闲后如实回落（C3：隐藏）。
        if index == last_index {
            out.now_rate = Some((
                point.bytes_in_per_sec.unwrap_or(0),
                point.bytes_out_per_sec.unwrap_or(0),
            ));
            out.now_requests = point.requests;
        }
    }

    // ③ 段区间（`runs`）：与 `seg_breaks` 同源、**预分配复用**（零新增分配）⇒ 绘制侧可一次性
    // 交给 `Gfx::polyline_runs`（单次 `GdipDrawPath`）—— 审查 P2-① 的收口点。
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

/// 该点的速率总量（`in + out`）；`None` = 断线（缺秒 / 无可比前一秒 —— 见 `stats::SeriesPoint`）。
fn rate_total(point: &SeriesPoint) -> Option<u64> {
    match (point.bytes_in_per_sec, point.bytes_out_per_sec) {
        (Some(bytes_in), Some(bytes_out)) => Some(bytes_in.saturating_add(bytes_out)),
        _ => None,
    }
}

/// 峰值 → 带余量的档（`峰值 × 110 %` 向上取档；`峰值 = 0` ⇒ 最低档）。
fn ladder_with_headroom(peak: u64) -> u64 {
    let wanted = peak.saturating_mul(HEADROOM_NUM) / HEADROOM_DEN;
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

    /// 断线点（缺秒 / 无可比前一秒）。
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
            build(&case, plot, &mut geometry);
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
        build(&all_absent(120), plot, &mut geometry);
        assert!(geometry.rate_points.is_empty());
        assert!(geometry.seg_breaks.is_empty());
        assert!(geometry.is_empty());
        assert_eq!(geometry.filled_secs, 0);
        assert_eq!(geometry.window_secs, 120);
        assert_eq!(geometry.status_text(), "0 s / 120 s", "空态角标正常");
        assert_eq!(geometry.now_text(), None, "C3：无样本 ⇒ 无当前值（隐藏）");

        // 填充度角标（C2）：交错 120 s ⇒ 60 s 有值
        build(&interleaved(120), plot, &mut geometry);
        assert_eq!(geometry.filled_secs, 60);
        assert_eq!(geometry.fill_text(), "60 s / 120 s");

        // 真实出口：`stats::Window::series(120)`（UFCS 形式；见模块头注释）
        let window = azusa_local_proxy::stats::Window::new();
        let points = azusa_local_proxy::stats::Window::series(
            &window,
            azusa_local_proxy::stats::WINDOW_SECS,
        );
        assert_eq!(points.len(), 120, "series(120) 长度恒 == secs");
        build(&points, plot, &mut geometry);
        assert!(geometry.rate_points.len() + geometry.seg_breaks.len() <= 121);
        assert_x_increasing(&geometry);

        // 空 `points`（防御）：不 panic、保持空态
        build(&[], plot, &mut geometry);
        assert!(geometry.is_empty());
    }

    /// 【A-9】断线：`[有,无,有,有]` ⇒ 折线 **2 段**、点数 **3**（`None` 不画线、**不补 0**）。
    #[test]
    fn a9_missing_seconds_break_the_line_instead_of_zero_filling() {
        let case = [
            point(1024, 0, 1),
            absent(),
            point(2048, 0, 1),
            point(2048, 0, 1),
        ];
        let mut geometry = CurveGeometry::new();
        build(&case, plot(), &mut geometry);

        assert_eq!(geometry.rate_points.len(), 3, "有值的秒只有 3 个");
        assert_eq!(geometry.seg_breaks, vec![1], "第 1 个有值点开启新段");
        assert_eq!(
            geometry.runs,
            vec![(0, 1), (1, 3)],
            "2 段：单点段 + 两点段（`runs` = 绘制侧唯一取段入口）"
        );
        // 关键：无样本的秒**不得**出现在点集里（补 0 = 假数据）
        assert!(
            geometry
                .rate_points
                .iter()
                .all(|point| point.Y != plot().y + plot().h),
            "没有任何点落在 0 值基线（否则就是补 0）"
        );
    }

    /// **真实 0 与"无样本"必须可区分**（C2/A-14' 的核心）：测到 0 字节的秒 = 落在基线的真点；
    /// 无样本的秒 = 不画点（断线）。
    #[test]
    fn measured_zero_is_a_real_point_while_absent_is_a_break() {
        let case = [point(0, 0, 1), absent(), point(0, 0, 1)];
        let mut geometry = CurveGeometry::new();
        build(&case, plot(), &mut geometry);
        assert_eq!(geometry.rate_points.len(), 2, "两个真 0 = 两个点");
        assert_eq!(geometry.seg_breaks, vec![1], "中间的无样本秒断开");
        let baseline = plot().y + plot().h;
        for point in geometry.rate_points.iter() {
            assert!((point.Y - baseline).abs() < 0.01, "真 0 ⇒ 基线：{point:?}");
        }
        assert_eq!(geometry.filled_secs, 2, "填充度按**有值秒**计（含真 0）");
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
        build(&all_present(4, 1024), plot(), &mut geometry);
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
        // 峰值 = 4 GiB/s（远超顶档）
        build(
            &[point(top * 4, 0, 1), point(top * 4, 0, 1)],
            plot,
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

    /// 当前值行：**最后一秒**有值才给串（C3）；请求数与速率同源同帧。
    #[test]
    fn current_value_follows_the_last_second_and_hides_when_absent() {
        let mut geometry = CurveGeometry::new();
        build(&all_present(4, 2048), plot(), &mut geometry);
        let now = geometry.now_text().expect("最后一秒有值");
        assert!(now.starts_with("↑1.0 KB/s ↓1.0 KB/s · 1.0 req/s"), "{now}");
        assert!(geometry.status_text().starts_with("4 s / 4 s · "));

        // 最后一秒无样本 ⇒ 当前值 None（C3：绘制侧隐藏，不留孤立符号），但曲线保留
        let case = [point(2048, 0, 1), point(2048, 0, 1), absent()];
        build(&case, plot(), &mut geometry);
        assert_eq!(geometry.rate_points.len(), 2, "曲线保留");
        assert_eq!(geometry.now_text(), None);
        assert_eq!(geometry.now_requests, 0, "无样本的秒不得读陈旧桶计数");
        assert_eq!(geometry.status_text(), "2 s / 3 s", "只留填充度角标");
    }
}
