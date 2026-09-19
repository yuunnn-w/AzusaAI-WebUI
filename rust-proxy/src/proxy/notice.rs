//! FR-39（误开提醒）的**去重闸门**：同名告警每小时 ≤ 1 条。
//!
//! ⚠ 本文件在「请求路径零 panic」硬规格作用域内（方案 §6.6 / §8.6 1a；
//! 由 `proxy/mod.rs` 的模块级内属性覆盖其子模块）。
//!
//! 语义边界（方案 §6.3 FR-39 写死）：
//! - 只**提示**，**绝不改包** —— 本模块只有"要不要记这一条"的判定，不碰请求头/请求体；
//! - Phase 1 只落 **日志**；主窗提示条（UI）属 Phase 2 ⇒ 本模块额外保留 `last_seen`
//!   供 Phase 2 的提示条读取，避免到时候重造状态。
//! - 告警会**周期出现**是预期（应用的"运行自检" P3 探针恒发 `text/plain`，方案 §3.3）⇒
//!   去重键按**告警名**分桶，而不是"全局只报一次"。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// FR-39：同名告警的最小间隔 = 1 小时。
pub const NOTICE_INTERVAL: Duration = Duration::from_secs(3600);

/// `Content-Type: text/plain` 告警的去重名。
pub const PLAIN_CONTENT_TYPE: &str = "content-type-text-plain";

#[derive(Default)]
pub struct WarnThrottle {
    last: Mutex<HashMap<&'static str, Instant>>,
}

impl WarnThrottle {
    pub fn new() -> WarnThrottle {
        WarnThrottle::default()
    }

    /// `true` = 本次允许记录（并记下时刻）；`false` = 该名义的告警还在间隔窗口内。
    pub fn allow(&self, name: &'static str, interval: Duration) -> bool {
        let mut guard = match self.last.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let now = Instant::now();
        match guard.get(name) {
            Some(previous) if now.duration_since(*previous) < interval => false,
            _ => {
                guard.insert(name, now);
                true
            }
        }
    }

    /// 最近一次"允许记录"的时刻（Phase 2 的提示条读它；未命中过 = `None`）。
    pub fn last_seen(&self, name: &str) -> Option<Instant> {
        let guard = match self.last.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.get(name).copied()
    }
}

/// 判定入站 `Content-Type` 是否为 `text/plain`（FR-39 的命中条件）。
/// 按 MIME essence 比较：忽略大小写、忽略 `; charset=…` 参数、忽略首尾空白。
pub fn is_plain_text(content_type: &str) -> bool {
    let essence = content_type.split(';').next().unwrap_or("").trim();
    essence.eq_ignore_ascii_case("text/plain")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_essence_matching() {
        assert!(is_plain_text("text/plain"));
        assert!(is_plain_text("TEXT/PLAIN"));
        assert!(is_plain_text("text/plain; charset=utf-8"));
        assert!(is_plain_text(" text/plain ;charset=UTF-8 "));
        assert!(!is_plain_text("application/json"));
        assert!(!is_plain_text("text/html"));
        assert!(!is_plain_text(""));
    }

    #[test]
    fn throttle_allows_once_per_window() {
        let throttle = WarnThrottle::new();
        let window = Duration::from_secs(3600);
        assert!(throttle.allow(PLAIN_CONTENT_TYPE, window));
        assert!(!throttle.allow(PLAIN_CONTENT_TYPE, window));
        assert!(!throttle.allow(PLAIN_CONTENT_TYPE, window));
        // 另一个名义独立计数（不互相压制）
        assert!(throttle.allow("another-warning", window));
        // 窗口 = 0 时每次都放行（负例：证明去重不是"永远只报一次"）
        assert!(throttle.allow("zero-window", Duration::ZERO));
        assert!(throttle.allow("zero-window", Duration::ZERO));
        assert!(throttle.last_seen(PLAIN_CONTENT_TYPE).is_some());
        assert!(throttle.last_seen("never-fired").is_none());
    }
}
