//! 配置**取值与校验**（P6-S4：已彻底去配置文件 —— 无读取、无写入、无模板）。
//!
//! 取值分层（方案 §3.1）：`Default`（本模块内建）→ CLI 覆盖（`cli::apply_overrides`，唯一入口）
//! → 运行期 UI 改动（设置窗「应用」→ `Controller::apply`，**仅内存**）。`validate()` 是三者共用的
//! 唯一合法性闸。字段名与默认值对齐方案 §6.1/§6.3。

/// 开源版默认上游（K11-a / K12：编译期注入；源码永不含内网地址）。
///
/// 注入来源（`build.rs` 的落点在 S10）：`AZUSA_DEFAULT_UPSTREAM` 环境变量；缺省 = 开源版默认。
/// **K12**：默认上游带端口 `:8080`（与默认监听 `127.0.0.1:80` **不同址** ⇒ 不带参数可直接启动）。
/// `--help` 必须打印**本函数**的返回值（运行期取，不得硬编码）。
pub fn default_upstream() -> &'static str {
    option_env!("AZUSA_DEFAULT_UPSTREAM").unwrap_or("http://127.0.0.1:8080")
}

#[derive(Clone, Debug)]
pub struct Config {
    pub listen_host: String,
    pub listen_port: u16,
    /// FR-2：主端口绑定失败时按序尝试的候选端口（默认 `[8000, 18080]`；**避开 8080**）。
    pub port_fallback: Vec<u16>,
    pub upstream_base: String,
    /// FR-7：可选剥离监听侧路径前缀（空 = 不动）。转发时只作用于 path，不碰查询串。
    pub strip_prefix: String,
    pub cors: CorsConfig,
    pub timeouts: TimeoutsConfig,
    /// 连接池 knobs（P6-S3 新增；`--pool-max-idle-per-host` / `--pool-idle-timeout-ms`）。
    pub pool: PoolConfig,
    pub logging: LoggingConfig,
    pub ui: UiConfig,
}

#[derive(Clone, Debug)]
pub struct CorsConfig {
    pub mode: String,
    pub max_age_seconds: u64,
}

/// FR-21 超时口径 = **三个相位**，各管各的：
/// `connect_ms`（连接建立）+ `response_head_ms`（**请求体送完 → 响应头到达**，新增）+
/// `first_byte_ms`（响应头之后 → 首帧）。
/// `first_byte_ms = 0` = 不设（默认，语义未变）；`response_head_ms = 0` = 不限（= 旧行为，不推荐）。
/// **这里没有、也不允许有"请求总超时"** —— 见 §6.5。
#[derive(Clone, Debug)]
pub struct TimeoutsConfig {
    pub connect_ms: u64,
    pub response_head_ms: u64,
    pub first_byte_ms: u64,
}

/// 连接池 knobs（P6-S3 新增；对应 `--pool-max-idle-per-host` / `--pool-idle-timeout-ms`）。
/// 落到 `proxy::forward::build_client`（原先写死 16 / 10 s）。
#[derive(Clone, Debug)]
pub struct PoolConfig {
    /// 每个主机的空闲连接上限（0–1024；默认 16）。
    pub max_idle_per_host: usize,
    /// 空闲连接保留期（毫秒，0–600,000；默认 10,000）。
    pub idle_timeout_ms: u64,
}

#[derive(Clone, Debug)]
pub struct LoggingConfig {
    /// 级别（`error|warn|info|debug`）。**P6-S5 / D12**：`dir` / `retain_files` 已随磁盘日志删除
    /// —— 日志只剩内存形态，没有可配置的目标。
    pub level: String,
}

/// §7.4「启动」组 + §7.3 退出语义：UI 行为配置。
#[derive(Clone, Debug)]
pub struct UiConfig {
    /// `"tray"` = 点 × 隐藏到托盘（默认）；`"exit"` = 直接退出（真正退出另一路径 = 托盘菜单「退出」）。
    pub close_action: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            listen_host: "127.0.0.1".to_string(),
            listen_port: 80,
            port_fallback: vec![8000, 18080],
            upstream_base: default_upstream().to_string(),
            strip_prefix: String::new(),
            cors: CorsConfig::default(),
            timeouts: TimeoutsConfig::default(),
            pool: PoolConfig::default(),
            logging: LoggingConfig::default(),
            ui: UiConfig::default(),
        }
    }
}

impl Default for CorsConfig {
    fn default() -> Self {
        CorsConfig {
            mode: "*".to_string(),
            max_age_seconds: 600,
        }
    }
}

impl Default for TimeoutsConfig {
    fn default() -> Self {
        TimeoutsConfig {
            connect_ms: 10_000,
            response_head_ms: 120_000,
            first_byte_ms: 0,
        }
    }
}

impl Default for PoolConfig {
    fn default() -> Self {
        PoolConfig {
            max_idle_per_host: 16,
            idle_timeout_ms: 10_000,
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        LoggingConfig {
            level: "info".to_string(),
        }
    }
}

impl Default for UiConfig {
    fn default() -> Self {
        UiConfig {
            close_action: "tray".to_string(),
        }
    }
}

/// FR-2：主端口 + 候选端口的**去重展开**（保持顺序；主端口失败才是降级）。
/// 返回 `(主端口, 候选列表)`；候选里与主端口重复的项被剔除（防手改配置导致"降级到同一个端口"）。
pub fn port_candidates(primary: u16, fallback: &[u16]) -> (u16, Vec<u16>) {
    let mut candidates: Vec<u16> = Vec::new();
    for port in fallback {
        if *port != primary && *port != 0 && !candidates.contains(port) {
            candidates.push(*port);
        }
    }
    (primary, candidates)
}

impl TimeoutsConfig {
    /// 连接建立超时（FR-21："连接建立超时默认 10 s"）。`connect_ms = 0` 由 `validate` 拒绝
    /// ⇒ 这里恒为 `Some`，但类型上仍返回 `Option`（reqwest 侧"不设"是合法表达，防将来放开）。
    pub fn connect(&self) -> Option<std::time::Duration> {
        if self.connect_ms == 0 {
            None
        } else {
            Some(std::time::Duration::from_millis(self.connect_ms))
        }
    }

    /// 响应首字节 watchdog（FR-21：**默认 0 = 不设**）。
    /// ⚠ 它不是"请求总超时"：只在首帧之前计时，首帧一到即失效（判据 §10.2 I3②）。
    pub fn first_byte(&self) -> Option<std::time::Duration> {
        if self.first_byte_ms == 0 {
            None
        } else {
            Some(std::time::Duration::from_millis(self.first_byte_ms))
        }
    }

    /// **等待响应头**上限（本修复批新增；默认 120 s，`0` = 不限 = 旧行为）。
    /// 语义 = 从「请求体已全部交给上游」到「上游响应头到达」；响应头一到计时器即 drop
    /// ⇒ **物理上不可能约束响应体时长**（判据 §2.8 J2）。
    pub fn response_head(&self) -> Option<std::time::Duration> {
        if self.response_head_ms == 0 {
            None
        } else {
            Some(std::time::Duration::from_millis(self.response_head_ms))
        }
    }
}

impl Config {
    /// 上游 base（去掉尾斜杠；转发拼接见 `proxy::forward::build_upstream_url`）。
    pub fn upstream_base_trimmed(&self) -> &str {
        self.upstream_base.trim_end_matches('/')
    }

    /// 取值校验（**唯一合法性闸**：CLI 与 UI 都必须过它；非法值 ⇒ 中文提示）。
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.listen_port == 0 {
            return Err(ConfigError::Invalid("listen_port 不能为 0".to_string()));
        }
        if self.listen_host.trim().is_empty() {
            return Err(ConfigError::Invalid("listen_host 不能为空".to_string()));
        }
        for port in &self.port_fallback {
            if *port == 0 {
                return Err(ConfigError::Invalid(
                    "port_fallback 里不能出现 0（0 = 让系统选端口，不适合降级序列）".to_string(),
                ));
            }
        }
        if !self.strip_prefix.is_empty() && !self.strip_prefix.starts_with('/') {
            return Err(ConfigError::Invalid(format!(
                "strip_prefix 必须以 / 开头（当前 {}）",
                self.strip_prefix
            )));
        }
        let base = self.upstream_base_trimmed();
        if base.is_empty() {
            return Err(ConfigError::Invalid("upstream_base 不能为空".to_string()));
        }
        if base.starts_with("https://") {
            return Err(ConfigError::Invalid(
                "upstream_base 不支持 https（v1 未启用 TLS：方案 N1/DG10；请填 http:// 内网地址）"
                    .to_string(),
            ));
        }
        if !base.starts_with("http://") {
            return Err(ConfigError::Invalid(format!(
                "upstream_base 必须以 http:// 开头（当前 {base}）"
            )));
        }
        match self.cors.mode.as_str() {
            "*" | "echo" => {}
            other => {
                return Err(ConfigError::Invalid(format!(
                    "cors.mode 只支持 \"*\" 或 \"echo\"（当前 {other}）"
                )))
            }
        }
        // FR-21：连接超时必须 > 0 —— 0 表示"不设"，会让不可达上游永远挂住（拒掉，不是默认）
        if self.timeouts.connect_ms == 0 {
            return Err(ConfigError::Invalid(
                "timeouts.connect_ms 必须 > 0（0 = 不设连接超时 ⇒ 不可达上游会永远挂住）"
                    .to_string(),
            ));
        }
        if self.timeouts.connect_ms > 3_600_000 {
            return Err(ConfigError::Invalid(format!(
                "timeouts.connect_ms 过大（当前 {} ms；上限 3,600,000 = 1 h，单位是**毫秒**）",
                self.timeouts.connect_ms
            )));
        }
        // FR-21：首字节 watchdog 0 = 不限（默认）；非 0 时给一个下限防手滑写成 1 ms 自伤
        if self.timeouts.first_byte_ms != 0 && self.timeouts.first_byte_ms < 50 {
            return Err(ConfigError::Invalid(format!(
                "timeouts.first_byte_ms 要么 0（不限），要么 ≥ 50 ms（当前 {} ms）",
                self.timeouts.first_byte_ms
            )));
        }
        // 本修复批：等待响应头上限 —— 0 = 不限（旧行为，不推荐）；非 0 时同样给下限/上限
        if self.timeouts.response_head_ms != 0 && self.timeouts.response_head_ms < 50 {
            return Err(ConfigError::Invalid(format!(
                "timeouts.response_head_ms 要么 0（不限 = 旧行为，**不推荐**：半开/静默上游会永久挂死），\
                 要么 ≥ 50 ms（当前 {} ms）",
                self.timeouts.response_head_ms
            )));
        }
        if self.timeouts.response_head_ms > 3_600_000 {
            return Err(ConfigError::Invalid(format!(
                "timeouts.response_head_ms 过大（当前 {} ms；上限 3,600,000 = 1 h，单位是**毫秒**）",
                self.timeouts.response_head_ms
            )));
        }
        if crate::logging::Level::parse(&self.logging.level).is_none() {
            return Err(ConfigError::Invalid(format!(
                "logging.level 只支持 error/warn/info/debug（当前 {}）",
                self.logging.level
            )));
        }
        match self.ui.close_action.as_str() {
            "tray" | "exit" => {}
            other => {
                return Err(ConfigError::Invalid(format!(
                    "ui.close_action 只支持 \"tray\" 或 \"exit\"（当前 {other}）"
                )))
            }
        }
        if self.pool.max_idle_per_host > 1024 {
            return Err(ConfigError::Invalid(format!(
                "pool.max_idle_per_host 上限 1024（当前 {}）",
                self.pool.max_idle_per_host
            )));
        }
        if self.pool.idle_timeout_ms > 600_000 {
            return Err(ConfigError::Invalid(format!(
                "pool.idle_timeout_ms 上限 600,000 ms（当前 {}）",
                self.pool.idle_timeout_ms
            )));
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum ConfigError {
    Invalid(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Invalid(detail) => write!(formatter, "配置值非法：{detail}"),
        }
    }
}

impl std::error::Error for ConfigError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// P6-S4（K12）+ P6-B4b（K11-a）：默认上游**按编译变体**如实 —— 两个变体同源同码，
    /// 唯一差异就是这个字符串，故这里按 `option_env!` 分臂断言（否则"内网版"会把开源版的
    /// 断言判红，而那条断言对注入值本就不成立）。
    #[test]
    fn default_upstream_matches_the_compiled_variant() {
        let upstream = default_upstream();
        assert_eq!(Config::default().upstream_base, upstream);
        assert!(
            upstream.starts_with("http://"),
            "默认上游必须是 http://：{upstream}"
        );
        match option_env!("AZUSA_DEFAULT_UPSTREAM") {
            // 开源版（未注入）：回环、无内网地址（D13），且与默认监听 `127.0.0.1:80` **不同址**（K12）
            None => {
                assert!(
                    upstream.starts_with("http://127.0.0.1:"),
                    "开源版默认上游必须是回环地址（D13）：{upstream}"
                );
                let check = crate::cli::check_self_loop(upstream, "127.0.0.1", 80)
                    .expect("默认上游必须是可解析的 http:// 地址");
                assert!(
                    !check.is_loop,
                    "默认上游 {upstream} 与默认监听 127.0.0.1:80 不得同址（K12）"
                );
            }
            // 内网版（K11-a 编译期注入）：默认值必须**逐字**等于注入串
            // （证明注入真的到达了二进制，而不是被回退值顶掉）
            Some(injected) => {
                assert_eq!(
                    injected, upstream,
                    "内网版默认上游必须等于编译期注入值（K11-a）"
                );
            }
        }
    }

    /// P6-S4：去配置文件后 `ConfigError` 只剩 `Invalid` 一个变体，且用户可见文案口径不变
    /// （`--help` / GUI / CLI 都把这句话直接给用户看）。
    #[test]
    fn config_error_display_keeps_the_invalid_wording() {
        let error = ConfigError::Invalid("listen_port 不能为 0".to_string());
        assert_eq!(error.to_string(), "配置值非法：listen_port 不能为 0");
        let boxed: Box<dyn std::error::Error> = Box::new(error);
        assert!(boxed.source().is_none());
    }

    #[test]
    fn validation_rejects_bad_values() {
        let with_upstream = |base: &str| Config {
            upstream_base: base.to_string(),
            ..Config::default()
        };
        assert!(with_upstream("https://example.com/gateway")
            .validate()
            .is_err());
        assert!(with_upstream("ftp://example.com").validate().is_err());
        assert!(with_upstream("").validate().is_err());

        let bad_cors = Config {
            cors: CorsConfig {
                mode: "bogus".to_string(),
                ..CorsConfig::default()
            },
            ..Config::default()
        };
        assert!(bad_cors.validate().is_err());

        let bad_level = Config {
            logging: LoggingConfig {
                level: "trace".to_string(),
            },
            ..Config::default()
        };
        assert!(bad_level.validate().is_err());
    }

    /// Phase 1：`timeouts.*` 的默认值、往返与校验（FR-21）。
    #[test]
    fn timeouts_default_and_validation() {
        let defaults = Config::default();
        assert_eq!(defaults.timeouts.connect_ms, 10_000);
        assert_eq!(defaults.timeouts.first_byte_ms, 0);
        // 本修复批新增键：默认 120 s（等待响应头上限）；`first_byte_ms` 语义与默认都**未变**
        assert_eq!(defaults.timeouts.response_head_ms, 120_000);
        assert_eq!(
            defaults.timeouts.response_head(),
            Some(std::time::Duration::from_millis(120_000))
        );
        assert_eq!(
            TimeoutsConfig {
                response_head_ms: 0,
                ..TimeoutsConfig::default()
            }
            .response_head(),
            None,
            "response_head_ms = 0 ⇒ 不限（= 旧行为，显式关闭）"
        );
        assert!(defaults.timeouts.connect().is_some());
        assert!(defaults.timeouts.first_byte().is_none());

        // 0 = 不设连接超时 ⇒ 拒绝（会挂住不可达上游）；单位是毫秒 ⇒ 上限 1 h
        let no_connect = Config {
            timeouts: TimeoutsConfig {
                connect_ms: 0,
                ..TimeoutsConfig::default()
            },
            ..Config::default()
        };
        assert!(no_connect.validate().is_err());
        let absurd = Config {
            timeouts: TimeoutsConfig {
                connect_ms: 3_600_001,
                ..TimeoutsConfig::default()
            },
            ..Config::default()
        };
        assert!(absurd.validate().is_err());

        // 首字节 watchdog：0 = 不限；非 0 必须 ≥ 50 ms
        let tiny = Config {
            timeouts: TimeoutsConfig {
                first_byte_ms: 10,
                ..TimeoutsConfig::default()
            },
            ..Config::default()
        };
        assert!(tiny.validate().is_err());
        let ok = Config {
            timeouts: TimeoutsConfig {
                first_byte_ms: 200,
                ..TimeoutsConfig::default()
            },
            ..Config::default()
        };
        assert!(ok.validate().is_ok());
        assert_eq!(
            ok.timeouts.first_byte(),
            Some(std::time::Duration::from_millis(200))
        );

        // 等待响应头上限（本修复批）：0 = 不限；非 0 必须 ≥ 50 ms 且 ≤ 1 h
        let head_tiny = Config {
            timeouts: TimeoutsConfig {
                response_head_ms: 10,
                ..TimeoutsConfig::default()
            },
            ..Config::default()
        };
        assert!(head_tiny.validate().is_err(), "10 ms 应被拒（防手滑自伤）");
        let head_absurd = Config {
            timeouts: TimeoutsConfig {
                response_head_ms: 3_600_001,
                ..TimeoutsConfig::default()
            },
            ..Config::default()
        };
        assert!(head_absurd.validate().is_err(), "> 1 h 应被拒");
        let head_off = Config {
            timeouts: TimeoutsConfig {
                response_head_ms: 0,
                ..TimeoutsConfig::default()
            },
            ..Config::default()
        };
        assert!(
            head_off.validate().is_ok(),
            "0 = 显式关闭（旧行为）必须合法"
        );
    }

    /// Phase 2：端口降级展开（FR-2）——去重、剔除主端口与 0、保持顺序。
    #[test]
    fn port_candidates_dedupes_and_keeps_order() {
        let (primary, candidates) = port_candidates(80, &[8000, 18080]);
        assert_eq!(primary, 80);
        assert_eq!(candidates, vec![8000, 18080]);

        let (_, candidates) = port_candidates(80, &[80, 8000, 8000, 0, 18080, 80]);
        assert_eq!(candidates, vec![8000, 18080]);

        let (_, candidates) = port_candidates(8000, &[8000]);
        assert!(candidates.is_empty());
    }

    /// Phase 2：新增字段的校验（FR-7 / FR-33 / FR-27）。
    #[test]
    fn phase2_fields_validation() {
        let bad_strip = Config {
            strip_prefix: "proxy".to_string(),
            ..Config::default()
        };
        assert!(bad_strip.validate().is_err());
        assert!(Config {
            strip_prefix: "/proxy".to_string(),
            ..Config::default()
        }
        .validate()
        .is_ok());

        let bad_close = Config {
            ui: UiConfig {
                close_action: "hide".to_string(),
            },
            ..Config::default()
        };
        assert!(bad_close.validate().is_err());

        let bad_log_level = Config {
            logging: LoggingConfig {
                level: "verbose".to_string(),
            },
            ..Config::default()
        };
        assert!(
            bad_log_level.validate().is_err(),
            "D12 后 LoggingConfig 只剩 level 一个字段 ⇒ 唯一可非法项就是级别"
        );

        let bad_fallback = Config {
            port_fallback: vec![8000, 0],
            ..Config::default()
        };
        assert!(bad_fallback.validate().is_err());
    }
}
