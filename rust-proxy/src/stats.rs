#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
//! 指标（方案 §6.9 FR-29～FR-31）：累计计数 / 活跃数 / 字节进出 / 延迟分位 / 最近错误环形缓冲。
//!
//! ⚠ 本文件在「请求路径零 panic」硬规格作用域内（方案 §6.6 / §8.6 1a）：
//! 禁止 `unwrap()` / `expect()` / `panic!()` / 显式索引取值 —— Win7 配方强制
//! `panic = "abort"`（§5.4），任何 panic = 进程立即退出。
//! 落地手法（本文件）：`Mutex` 一律用 poison-tolerant 模式；数组/切片取值一律 `get()`；
//! 直方图求和/取分位用 `iter().enumerate()`（不索引）。
//!
//! **长生命周期**：Phase 2 起 `Stats` 由 `Arc` 共享（服务启停热切换不丢累计数与最近错误，
//! 见 `src/service.rs`）⇒ 只允许 `&self` 方法（原子/内部可变）。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Mutex;

#[derive(Default)]
pub struct Stats {
    total: AtomicU64,
    preflight: AtomicU64,
    forwarded: AtomicU64,
    upstream_errors: AtomicU64,
    upstream_timeouts: AtomicU64,
    stream_errors: AtomicU64,
    client_aborted: AtomicU64,
    /// FR-30：上游 4xx / 5xx（**上游真实返回**的状态码，不含代理自己合成的 502/504 ——
    /// 那两个走 `upstream_errors` / `upstream_timeouts`）。
    upstream_4xx: AtomicU64,
    upstream_5xx: AtomicU64,
    bytes_in: AtomicU64,
    bytes_out: AtomicU64,
    /// 本修复批（L2/L4）：连接池重置次数（"失败即弃池"的机制计数，**不是**错误 ⇒ 不进 `total_errors`）。
    pool_flushes: AtomicU64,
    /// 本修复批（L4）：安全重试次数（含成功与失败的重试；只对 GET/HEAD/OPTIONS ∧ 无体 ∧ 未发字节）。
    retries: AtomicU64,
    active: AtomicI64,
    peak_active: AtomicI64,
    /// FR-30「上游延迟 p50/p95」：请求 → 上游响应头到达（TTFB）的直方图。
    latency_buckets: Vec<AtomicU64>,
    /// FR-31：最近错误（环形，容量 `RECENT_ERRORS_CAPACITY`；同一条连续出现时合并计数）。
    errors: Mutex<VecDeque<ErrorEntry>>,
}

/// FR-30：延迟直方图上界（毫秒）。桶 `i` 覆盖 `(upper[i-1], upper[i]]`；
/// 超过最后一个上界的样本进末桶（> 10 s 归入"≥10 s"档）。12 桶 = 够看，又不至于让
/// 分位读数抖动（每 1 s 刷新的 UI 上"38ms / 210ms"这种量级就够用）。
const LATENCY_BUCKET_UPPER_MS: [u64; 12] =
    [5, 10, 25, 50, 100, 250, 500, 1000, 2500, 5000, 10000, 30000];

/// FR-31：最近错误保留条数（方案写死 10）。
pub const RECENT_ERRORS_CAPACITY: usize = 10;

/// 末桶（`> 10 s`）的显示口径（FR-30 数字诚实性；审查 P2-7）：**不**谎报桶上界 30000ms。
pub const LATENCY_OVERFLOW_LABEL: &str = "≥10s";

/// 一条错误摘要（时间 + 单行 + 连续次数）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErrorEntry {
    pub time: String,
    pub line: String,
    pub count: u32,
}

/// 计数器快照（访问日志行 + Phase 2 的 UI 指标卡共用）。
pub struct Snapshot {
    pub total: u64,
    pub preflight: u64,
    pub forwarded: u64,
    pub upstream_errors: u64,
    pub upstream_timeouts: u64,
    pub stream_errors: u64,
    pub client_aborted: u64,
    pub upstream_4xx: u64,
    pub upstream_5xx: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    /// L2：连接池重置次数（`池重置=` 读数；判据 I15/I16/I19/I20 读它）。
    pub pool_flushes: u64,
    /// L4：安全重试次数。
    pub retries: u64,
    pub active: i64,
    pub peak_active: i64,
}

impl Stats {
    pub fn new() -> Stats {
        Stats {
            latency_buckets: (0..LATENCY_BUCKET_UPPER_MS.len())
                .map(|_| AtomicU64::new(0))
                .collect(),
            ..Stats::default()
        }
    }

    pub fn inc_total(&self) {
        self.total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_preflight(&self) {
        self.preflight.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_forwarded(&self) {
        self.forwarded.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_upstream_error(&self) {
        self.upstream_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// FR-23：上游超时（504）与"不可达/其它失败"（502）分开计数。
    pub fn inc_upstream_timeout(&self) {
        self.upstream_timeouts.fetch_add(1, Ordering::Relaxed);
    }

    /// FR-23：响应流中途出错/首字节 watchdog 命中（已发头之后）。
    pub fn inc_stream_error(&self) {
        self.stream_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// FR-19：客户端断开（未读完就 drop 上游流 ⇒ 该请求被取消）。
    pub fn inc_client_aborted(&self) {
        self.client_aborted.fetch_add(1, Ordering::Relaxed);
    }

    /// FR-30：上游真实返回 4xx（不含代理合成的 502/504）。
    pub fn inc_upstream_4xx(&self) {
        self.upstream_4xx.fetch_add(1, Ordering::Relaxed);
    }

    /// FR-30：上游真实返回 5xx（不含代理合成的 502/504）。
    pub fn inc_upstream_5xx(&self) {
        self.upstream_5xx.fetch_add(1, Ordering::Relaxed);
    }

    /// 本修复批（L2）：连接池被重置一次（三条触发面：等待响应头超时 / `send()` 传输错误 / 响应流中断）。
    pub fn inc_pool_flush(&self) {
        self.pool_flushes.fetch_add(1, Ordering::Relaxed);
    }

    /// 本修复批（L4）：一次安全重试（白名单 = GET/HEAD/OPTIONS ∧ 无体 ∧ 已发字节 0）。
    pub fn inc_retry(&self) {
        self.retries.fetch_add(1, Ordering::Relaxed);
    }

    /// 请求体字节（发往上游的 ↑）。
    pub fn add_bytes_in(&self, bytes: u64) {
        self.bytes_in.fetch_add(bytes, Ordering::Relaxed);
    }

    /// 响应体字节（上游回给客户端的 ↓）。
    pub fn add_bytes_out(&self, bytes: u64) {
        self.bytes_out.fetch_add(bytes, Ordering::Relaxed);
    }

    /// FR-30：记录一次"请求 → 上游响应头"延迟（毫秒）。
    pub fn record_upstream_latency_ms(&self, millis: u64) {
        let index = latency_bucket_index(millis);
        if let Some(bucket) = self.latency_buckets.get(index) {
            bucket.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// FR-30：延迟分位（`p` = 0–100）。无样本 ⇒ `None`（UI 显示 `—`）。
    /// 返回值 = 命中桶的上界（保守读数；桶宽见 `LATENCY_BUCKET_UPPER_MS`）。
    pub fn latency_percentile_ms(&self, p: u32) -> Option<u64> {
        let mut total: u64 = 0;
        for bucket in self.latency_buckets.iter() {
            total += bucket.load(Ordering::Relaxed);
        }
        if total == 0 {
            return None;
        }
        let target = ((u64::from(p.clamp(1, 100)) * total).div_ceil(100)).max(1);
        let mut accumulated: u64 = 0;
        for (index, bucket) in self.latency_buckets.iter().enumerate() {
            accumulated += bucket.load(Ordering::Relaxed);
            if accumulated >= target {
                return LATENCY_BUCKET_UPPER_MS.get(index).copied();
            }
        }
        LATENCY_BUCKET_UPPER_MS.last().copied()
    }

    /// FR-30 的**显示口径**（审查 P2-7 修）：末桶（`> 10 s` 的全部样本都归它）**不能谎报成
    /// 桶上界 30000ms** —— 一个 10.2 s 的超时会被读成"30 秒"。末桶一律显示 `≥10s`。
    /// 普通桶照旧显示上界（`38ms` / `210ms` 这种量级就是它的用途）。
    pub fn latency_percentile_label(&self, p: u32) -> Option<String> {
        let value = self.latency_percentile_ms(p)?;
        if Some(value) == LATENCY_BUCKET_UPPER_MS.last().copied() {
            return Some(LATENCY_OVERFLOW_LABEL.to_string());
        }
        Some(format!("{value}ms"))
    }

    /// FR-31：记一条错误（时间由本函数取，`line` 不含时间）。
    /// 与上一条**完全同文本**时合并（`count += 1` 并刷新时间），否则入队；
    /// 超过容量丢最旧。
    pub fn record_error(&self, line: &str) {
        let time = chrono::Local::now().format("%H:%M:%S").to_string();
        let mut guard = match self.errors.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(last) = guard.back_mut() {
            if last.line == line {
                last.time = time;
                last.count = last.count.saturating_add(1);
                return;
            }
        }
        guard.push_back(ErrorEntry {
            time,
            line: line.to_string(),
            count: 1,
        });
        while guard.len() > RECENT_ERRORS_CAPACITY {
            guard.pop_front();
        }
    }

    /// FR-31：最近的错误（**最新在前**）。
    pub fn recent_errors(&self) -> Vec<ErrorEntry> {
        let guard = match self.errors.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.iter().rev().cloned().collect()
    }

    /// FR-31：单行摘要（UI 的"最近错误"行）。
    pub fn latest_error(&self) -> Option<ErrorEntry> {
        let guard = match self.errors.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.back().cloned()
    }

    /// FR-29「当前活跃连接数」的分子：在途请求 +1（配 `dec_active` 成对使用；见 `proxy::ActiveGuard`）。
    pub fn inc_active(&self) {
        let current = self.active.fetch_add(1, Ordering::Relaxed) + 1;
        // CAS 循环而非 fetch_max：win7 目标的 std 里 `fetch_max` 也在，但这里保持最小原子面
        let mut peak = self.peak_active.load(Ordering::Relaxed);
        while current > peak {
            match self.peak_active.compare_exchange_weak(
                peak,
                current,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(observed) => peak = observed,
            }
        }
    }

    pub fn dec_active(&self) {
        self.active.fetch_sub(1, Ordering::Relaxed);
    }

    /// 错误总数（UI「错误」指标卡 = 上游 4xx/5xx + 代理自身错误 + 流中断）。
    pub fn total_errors(&self) -> u64 {
        self.upstream_errors.load(Ordering::Relaxed)
            + self.upstream_timeouts.load(Ordering::Relaxed)
            + self.stream_errors.load(Ordering::Relaxed)
            + self.upstream_4xx.load(Ordering::Relaxed)
            + self.upstream_5xx.load(Ordering::Relaxed)
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            total: self.total.load(Ordering::Relaxed),
            preflight: self.preflight.load(Ordering::Relaxed),
            forwarded: self.forwarded.load(Ordering::Relaxed),
            upstream_errors: self.upstream_errors.load(Ordering::Relaxed),
            upstream_timeouts: self.upstream_timeouts.load(Ordering::Relaxed),
            stream_errors: self.stream_errors.load(Ordering::Relaxed),
            client_aborted: self.client_aborted.load(Ordering::Relaxed),
            upstream_4xx: self.upstream_4xx.load(Ordering::Relaxed),
            upstream_5xx: self.upstream_5xx.load(Ordering::Relaxed),
            bytes_in: self.bytes_in.load(Ordering::Relaxed),
            bytes_out: self.bytes_out.load(Ordering::Relaxed),
            pool_flushes: self.pool_flushes.load(Ordering::Relaxed),
            retries: self.retries.load(Ordering::Relaxed),
            active: self.active.load(Ordering::Relaxed),
            peak_active: self.peak_active.load(Ordering::Relaxed),
        }
    }
}

/// 延迟 → 桶下标（首个"上界 ≥ 样本"的桶；超过全部上界 ⇒ 末桶）。
fn latency_bucket_index(millis: u64) -> usize {
    for (index, upper) in LATENCY_BUCKET_UPPER_MS.iter().enumerate() {
        if millis <= *upper {
            return index;
        }
    }
    LATENCY_BUCKET_UPPER_MS.len().saturating_sub(1)
}

impl Snapshot {
    pub fn render(&self) -> String {
        format!(
            "累计 请求={} 预检={} 转发={} 上游错误={} 上游超时={} 流中断={} 客户端中止={} 字节入={} 字节出={} 活跃={} 峰值活跃={} 池重置={} 重试={}",
            self.total,
            self.preflight,
            self.forwarded,
            self.upstream_errors,
            self.upstream_timeouts,
            self.stream_errors,
            self.client_aborted,
            self.bytes_in,
            self.bytes_out,
            self.active,
            self.peak_active,
            self.pool_flushes,
            self.retries,
        )
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;

    /// 审查 P2-7：末桶（> 10 s）读数**不得**谎报为桶上界 30000ms ⇒ 显示 `≥10s`。
    #[test]
    fn latency_overflow_bucket_is_labelled_honestly() {
        let stats = Stats::new();
        stats.record_upstream_latency_ms(12); // 普通桶 ⇒ 25ms
        assert_eq!(stats.latency_percentile_label(50).as_deref(), Some("25ms"));

        let stats = Stats::new();
        stats.record_upstream_latency_ms(10_008); // 10 s 连接超时样本 ⇒ 末桶
        assert_eq!(
            stats.latency_percentile_label(50).as_deref(),
            Some(LATENCY_OVERFLOW_LABEL),
            "10.2 s 的样本必须读作 ≥10s，不是 30000ms"
        );
        assert_eq!(
            stats.latency_percentile_ms(50),
            Some(30_000),
            "底层仍返回桶上界（口径不变，只是显示层改）"
        );

        let stats = Stats::new();
        assert_eq!(
            stats.latency_percentile_label(50),
            None,
            "无样本 ⇒ None（UI 显示 —）"
        );
    }

    /// Phase 2：直方图分位（无样本 ⇒ None；单调；末桶兜底）。
    #[test]
    fn latency_percentiles_are_monotonic() {
        let stats = Stats::new();
        assert_eq!(stats.latency_percentile_ms(50), None);
        assert_eq!(stats.latency_percentile_ms(95), None);

        for millis in [3_u64, 4, 8, 30, 60, 90, 120, 240, 700, 1500] {
            stats.record_upstream_latency_ms(millis);
        }
        let p50 = stats.latency_percentile_ms(50).expect("有样本");
        let p95 = stats.latency_percentile_ms(95).expect("有样本");
        assert!(p50 <= p95, "p50 {p50} 必须 <= p95 {p95}");
        // 样本落桶：3,4→5ms 桶；8→10；30→50；60,90→100；120,240→250；700→1000；1500→2500
        // p50 target = 5 ⇒ 累计到 100ms 桶（6 个）才够 ⇒ p50 = 100
        assert_eq!(p50, 100);
        // p95 target = 10 ⇒ 最后一个样本的桶 ⇒ p95 = 2500
        assert_eq!(p95, 2500);

        // 超过全部上界 ⇒ 末桶（30000）
        let huge = Stats::new();
        huge.record_upstream_latency_ms(86_400_000);
        assert_eq!(huge.latency_percentile_ms(50), Some(30000));
    }

    /// Phase 2：最近错误环形缓冲（合并连续同文本、容量封顶、最新在前）。
    #[test]
    fn recent_errors_merge_and_stay_bounded() {
        let stats = Stats::new();
        assert!(stats.latest_error().is_none());

        stats.record_error("HTTP 502 上游连接失败");
        stats.record_error("HTTP 502 上游连接失败");
        let latest = stats.latest_error().expect("有一条");
        assert_eq!(latest.count, 2, "连续同文本必须合并计数");
        assert_eq!(latest.line, "HTTP 502 上游连接失败");

        stats.record_error("HTTP 504 上游超时");
        assert_eq!(stats.recent_errors().len(), 2);
        assert_eq!(stats.latest_error().unwrap().line, "HTTP 504 上游超时");

        // 灌满 + 溢出：容量封顶在 RECENT_ERRORS_CAPACITY，最旧的被挤掉
        for index in 0..(RECENT_ERRORS_CAPACITY + 5) {
            stats.record_error(&format!("错误 {index}"));
        }
        let recent = stats.recent_errors();
        assert_eq!(recent.len(), RECENT_ERRORS_CAPACITY);
        assert_eq!(
            recent[0].line,
            format!("错误 {}", RECENT_ERRORS_CAPACITY + 4)
        );

        // 错误总数 = 上游 4xx/5xx + 代理错误 + 流中断
        let counters = Stats::new();
        counters.inc_upstream_4xx();
        counters.inc_upstream_5xx();
        counters.inc_upstream_error();
        counters.inc_upstream_timeout();
        counters.inc_stream_error();
        assert_eq!(counters.total_errors(), 5);
    }

    /// Phase 2：字节进出分开计（↑ 请求体 / ↓ 响应体）。
    #[test]
    fn bytes_in_and_out_are_separate() {
        let stats = Stats::new();
        stats.add_bytes_in(1024);
        stats.add_bytes_out(2048);
        let snapshot = stats.snapshot();
        assert_eq!(snapshot.bytes_in, 1024);
        assert_eq!(snapshot.bytes_out, 2048);
        assert!(snapshot.render().contains("字节入=1024"));
    }
}
