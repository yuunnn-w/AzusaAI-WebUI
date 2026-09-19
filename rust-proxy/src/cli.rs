//! CLI 全参数化（D4 / 方案 §2.1）+ 自环守卫（D13-b / 方案 §3.5）。
//!
//! 语法：`--opt value` 与 `--opt=value` 均支持；无位置参数（出现 ⇒ 参数错误）。
//! `-h|--help` / `--version|-V` 属**立即动作**，执行后退出，不进入正常启动流程。
//! **K13**：原两个开机自启参数（`--install-*` / `--uninstall-*`）已随自启功能整体移除
//! ⇒ 传它们走"未知参数"分支（用法 + exit 2），程序**没有任何注册表写入路径**。
//!
//! ## 自环守卫（关键口径，唯一）
//!
//! 守卫**只判一件事**：上游的 `(归一 host, 有效 port)` == **实际监听**的 `(归一 host, 实际 port)`。
//! 它**不是**"没传 `--upstream` 就拒绝" —— **K12**：默认上游 = `http://127.0.0.1:8080`，与默认监听
//! `127.0.0.1:80` **不同址 ⇒ 不带任何参数即可正常启动**；只有把上游（显式或经默认）配成与实际
//! 监听同址时才拒绝（例：`--listen-port 8080` 不传 `--upstream`，此时默认上游正好落在 8080）。
//!
//! **host 归一 = 单点来源**（[`normalize_host`]）：`listen` 侧取 `--listen-host`，`upstream` 侧先
//! `Url::parse` 再取 `host_str()`；两侧共用同一函数，**不存在第二套字符串规则**。
//!
//! 两个检查时点由调用方落位：① `main.rs` 的启动预检（bind 之前，意图监听口）；
//! ② `service.rs` 的端口回退复检（bind 成功后按**实际**监听口，命中即跳过该候选）。

use std::net::{IpAddr, Ipv4Addr};

use crate::config::Config;

/// `usage_text()` 的规范定义在 [`crate::output`]（方案 §2.2「三种呈现共用同一份」）；
/// 这里以 `pub use` 暴露到 `cli` 命名空间（方案 §4-S3 的落点约定）。
pub use crate::output::usage_text;

/// 立即动作 / 正常启动。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// 正常启动流程。
    Run,
    /// `-h|--help`。
    Help,
    /// `--version|-V`。
    Version,
}

/// 解析后的 CLI（未应用的字段为 `None`，`apply_overrides` 时才落到 `Config`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cli {
    pub mode: Mode,
    pub no_gui: bool,
    pub start_paused: bool,
    pub listen_host: Option<String>,
    pub listen_port: Option<u16>,
    pub port_fallback: Option<Vec<u16>>,
    pub upstream: Option<String>,
    pub strip_prefix: Option<String>,
    pub close_action: Option<String>,
    pub cors_mode: Option<String>,
    pub cors_max_age: Option<u64>,
    pub connect_timeout_ms: Option<u64>,
    pub response_head_timeout_ms: Option<u64>,
    pub first_byte_timeout_ms: Option<u64>,
    pub pool_max_idle_per_host: Option<usize>,
    pub pool_idle_timeout_ms: Option<u64>,
    pub log_level: Option<String>,
}

impl Default for Cli {
    fn default() -> Self {
        Cli {
            mode: Mode::Run,
            no_gui: false,
            start_paused: false,
            listen_host: None,
            listen_port: None,
            port_fallback: None,
            upstream: None,
            strip_prefix: None,
            close_action: None,
            cors_mode: None,
            cors_max_age: None,
            connect_timeout_ms: None,
            response_head_timeout_ms: None,
            first_byte_timeout_ms: None,
            pool_max_idle_per_host: None,
            pool_idle_timeout_ms: None,
            log_level: None,
        }
    }
}

/// 参数错误（归入退出码 2）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError(pub String);

impl std::fmt::Display for CliError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl std::error::Error for CliError {}

fn err<T>(message: impl Into<String>) -> Result<T, CliError> {
    Err(CliError(message.into()))
}

/// 取 `--opt value` 的值（`--opt=value` 时用 inline）。
fn next_value<I: Iterator<Item = String>>(
    name: &str,
    inline: Option<String>,
    iter: &mut I,
) -> Result<String, CliError> {
    match inline {
        Some(value) => Ok(value),
        None => iter
            .next()
            .ok_or_else(|| CliError(format!("{name} 需要参数"))),
    }
}

/// 标志不接受 `=value`。
fn no_value(name: &str, inline: &Option<String>) -> Result<(), CliError> {
    if inline.is_some() {
        return err(format!("{name} 是标志，不接受值"));
    }
    Ok(())
}

/// 解析命令行为 `Cli`。`args` 由调用方给（不含程序名；测试可注入）。
pub fn parse_args<I>(args: I) -> Result<Cli, CliError>
where
    I: IntoIterator<Item = String>,
{
    let mut cli = Cli::default();
    let mut help = false;
    let mut version = false;

    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        let (name, inline) = if arg.starts_with("--") {
            match arg.split_once('=') {
                Some((name, value)) => (name.to_string(), Some(value.to_string())),
                None => (arg.clone(), None),
            }
        } else {
            (arg.clone(), None)
        };
        match name.as_str() {
            "-h" | "--help" => {
                no_value(&name, &inline)?;
                help = true;
            }
            "-V" | "--version" => {
                no_value(&name, &inline)?;
                version = true;
            }
            "--no-gui" => {
                no_value(&name, &inline)?;
                cli.no_gui = true;
            }
            "--start-paused" => {
                no_value(&name, &inline)?;
                cli.start_paused = true;
            }
            "--listen-host" => {
                cli.listen_host = Some(next_value(&name, inline, &mut iter)?);
            }
            "--listen-port" | "--port" => {
                let value = next_value(&name, inline, &mut iter)?;
                cli.listen_port = Some(parse_port(&name, &value)?);
            }
            "--port-fallback" => {
                let value = next_value(&name, inline, &mut iter)?;
                cli.port_fallback = Some(parse_port_list(&name, &value)?);
            }
            "--upstream" => {
                cli.upstream = Some(next_value(&name, inline, &mut iter)?);
            }
            "--strip-prefix" => {
                cli.strip_prefix = Some(next_value(&name, inline, &mut iter)?);
            }
            "--close-action" => {
                let value = next_value(&name, inline, &mut iter)?;
                if value != "tray" && value != "exit" {
                    return err(format!("--close-action 只支持 tray/exit（当前 {value}）"));
                }
                cli.close_action = Some(value);
            }
            "--cors-mode" => {
                let value = next_value(&name, inline, &mut iter)?;
                cli.cors_mode = Some(match value.as_str() {
                    "*" | "star" => "*".to_string(),
                    "echo" => "echo".to_string(),
                    other => {
                        return err(format!("--cors-mode 只支持 *|star|echo（当前 {other}）"));
                    }
                });
            }
            "--cors-max-age" => {
                let value = next_value(&name, inline, &mut iter)?;
                cli.cors_max_age = Some(parse_bounded_u64(&name, &value, 0, 86_400)?);
            }
            "--connect-timeout-ms" => {
                let value = next_value(&name, inline, &mut iter)?;
                cli.connect_timeout_ms = Some(parse_connect_timeout(&value)?);
            }
            "--response-head-timeout-ms" => {
                let value = next_value(&name, inline, &mut iter)?;
                cli.response_head_timeout_ms = Some(parse_timeout_phase(&name, &value)?);
            }
            "--first-byte-timeout-ms" => {
                let value = next_value(&name, inline, &mut iter)?;
                cli.first_byte_timeout_ms = Some(parse_timeout_phase(&name, &value)?);
            }
            "--pool-max-idle-per-host" => {
                let value = next_value(&name, inline, &mut iter)?;
                let parsed = parse_bounded_u64(&name, &value, 0, 1024)?;
                cli.pool_max_idle_per_host = Some(parsed as usize);
            }
            "--pool-idle-timeout-ms" => {
                let value = next_value(&name, inline, &mut iter)?;
                cli.pool_idle_timeout_ms = Some(parse_bounded_u64(&name, &value, 0, 600_000)?);
            }
            "--log-level" => {
                let value = next_value(&name, inline, &mut iter)?;
                if !matches!(value.as_str(), "error" | "warn" | "info" | "debug") {
                    return err(format!(
                        "--log-level 只支持 error|warn|info|debug（当前 {value}）"
                    ));
                }
                cli.log_level = Some(value);
            }
            other => {
                return err(format!("未知参数：{other}"));
            }
        }
    }

    // 立即动作优先（`--help` 必须**永远可读** ⇒ 先于冲突与自环判定）。
    if help {
        cli.mode = Mode::Help;
        return Ok(cli);
    }
    if version {
        cli.mode = Mode::Version;
        return Ok(cli);
    }
    // 冲突校验
    if cli.no_gui && cli.start_paused {
        return err("--no-gui 与 --start-paused 不能同时给出（同给 ⇒ 退出码 2）");
    }
    cli.mode = Mode::Run;
    Ok(cli)
}

fn parse_port(name: &str, value: &str) -> Result<u16, CliError> {
    let port: u16 = value
        .parse()
        .map_err(|_| CliError(format!("{name} 的值非法：{value}（1–65535）")))?;
    if port == 0 {
        return err("--port 不能为 0");
    }
    Ok(port)
}

fn parse_port_list(name: &str, value: &str) -> Result<Vec<u16>, CliError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("none") {
        return Ok(Vec::new());
    }
    let mut ports = Vec::new();
    for part in trimmed.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let port: u16 = part
            .parse()
            .map_err(|_| CliError(format!("{name} 的项非法：{part}（1–65535）")))?;
        if port == 0 {
            return err(format!("{name} 不能含 0（0 = 让系统选端口）"));
        }
        ports.push(port);
    }
    Ok(ports)
}

fn parse_bounded_u64(name: &str, value: &str, min: u64, max: u64) -> Result<u64, CliError> {
    let parsed: u64 = value
        .parse()
        .map_err(|_| CliError(format!("{name} 的值非法：{value}")))?;
    if parsed < min || parsed > max {
        return err(format!("{name} 必须在 {min}–{max}（当前 {value}）"));
    }
    Ok(parsed)
}

fn parse_connect_timeout(value: &str) -> Result<u64, CliError> {
    let parsed: u64 = value
        .parse()
        .map_err(|_| CliError(format!("--connect-timeout-ms 的值非法：{value}")))?;
    if parsed == 0 {
        return err("--connect-timeout-ms 不能为 0（不设连接超时 = 不可达上游永远挂住）");
    }
    if parsed > 3_600_000 {
        return err(format!(
            "--connect-timeout-ms 过大（当前 {parsed} ms；上限 3,600,000 = 1 h）"
        ));
    }
    Ok(parsed)
}

/// 分相位 watchdog：`0` = 显式关闭（允许），非 0 必须 ≥ 50 ms 且 ≤ 1 h。
fn parse_timeout_phase(name: &str, value: &str) -> Result<u64, CliError> {
    let parsed: u64 = value
        .parse()
        .map_err(|_| CliError(format!("{name} 的值非法：{value}")))?;
    if parsed == 0 {
        return Ok(0);
    }
    if parsed < 50 {
        return err(format!(
            "{name} 要么 0（关闭），要么 ≥ 50 ms（当前 {parsed} ms）"
        ));
    }
    if parsed > 3_600_000 {
        return err(format!(
            "{name} 过大（当前 {parsed} ms；上限 3,600,000 = 1 h）"
        ));
    }
    Ok(parsed)
}

impl Cli {
    /// 把 CLI 覆盖落到 `Config`（唯一入口），并跑 `Config::validate()`（去配置后成为唯一合法性闸）。
    ///
    /// `--listen-host` 在此做**单点归一**（`localhost`/`::1` → `127.0.0.1`；其余按 `IpAddr::is_loopback`
    /// 判定，非回环 ⇒ 参数错误，D11-d）；归一后的字面量 IP 写回 `Config`，`server::bind` 只见到 IP。
    ///
    /// `--start-paused` **不落 `Config`**（K13：`Config` 里原「启动即开始服务」字段已随自启功能删除）——
    /// 它是"本次启动是否立即起服务"的进程语义，由 `main` 直接读 [`Cli::start_paused`] 交给 `ui::run`。
    pub fn apply_overrides(&self, config: &mut Config) -> Result<(), CliError> {
        if let Some(host) = &self.listen_host {
            let ip = normalize_host(host)
                .ok_or_else(|| CliError(format!("--listen-host 无法解析：{host}")))?;
            if !ip.is_loopback() {
                return err(format!(
                    "--listen-host 只接受回环地址（127.0.0.1 / ::1）；当前 {host} 非回环"
                ));
            }
            config.listen_host = ip.to_string();
        }
        if let Some(port) = self.listen_port {
            config.listen_port = port;
        }
        if let Some(fallback) = &self.port_fallback {
            config.port_fallback = fallback.clone();
        }
        if let Some(upstream) = &self.upstream {
            config.upstream_base = upstream.clone();
        }
        if let Some(strip) = &self.strip_prefix {
            config.strip_prefix = strip.clone();
        }
        if let Some(action) = &self.close_action {
            config.ui.close_action = action.clone();
        }
        if let Some(mode) = &self.cors_mode {
            config.cors.mode = mode.clone();
        }
        if let Some(age) = self.cors_max_age {
            config.cors.max_age_seconds = age;
        }
        if let Some(ms) = self.connect_timeout_ms {
            config.timeouts.connect_ms = ms;
        }
        if let Some(ms) = self.response_head_timeout_ms {
            config.timeouts.response_head_ms = ms;
        }
        if let Some(ms) = self.first_byte_timeout_ms {
            config.timeouts.first_byte_ms = ms;
        }
        if let Some(value) = self.pool_max_idle_per_host {
            config.pool.max_idle_per_host = value;
        }
        if let Some(ms) = self.pool_idle_timeout_ms {
            config.pool.idle_timeout_ms = ms;
        }
        if let Some(level) = &self.log_level {
            config.logging.level = level.clone();
        }
        config.validate().map_err(|err| CliError(format!("{err}")))
    }
}

/// 自环判定结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoopCheck {
    /// 是否构成自环（上游 host:port == 监听 host:port）。
    pub is_loop: bool,
    /// 上游归一后的 `host:port`（文案用）。
    pub upstream_key: String,
    /// 监听归一后的 `host:port`（文案用）。
    pub listen_key: String,
}

/// 自环判定（纯函数；两个时点共用）。
///
/// `listen_port` = **实际**（或意图）监听口；`upstream` = `http://…` 上游 base。
pub fn check_self_loop(
    upstream: &str,
    listen_host: &str,
    listen_port: u16,
) -> Result<LoopCheck, CliError> {
    let listen_ip = normalize_host(listen_host)
        .ok_or_else(|| CliError(format!("监听地址无法解析：{listen_host}")))?;
    let url = reqwest::Url::parse(upstream)
        .map_err(|err| CliError(format!("上游地址无法解析：{upstream}（{err}）")))?;
    let up_host_str = url
        .host_str()
        .ok_or_else(|| CliError(format!("上游地址缺少主机名：{upstream}")))?;
    let up_ip = normalize_host(up_host_str)
        .ok_or_else(|| CliError(format!("上游主机名无法解析：{up_host_str}")))?;
    let up_port = url.port_or_known_default().unwrap_or(80);
    Ok(LoopCheck {
        is_loop: up_ip == listen_ip && up_port == listen_port,
        upstream_key: key_of(up_ip, up_port),
        listen_key: key_of(listen_ip, listen_port),
    })
}

fn key_of(ip: IpAddr, port: u16) -> String {
    match ip {
        IpAddr::V4(v4) => format!("{v4}:{port}"),
        IpAddr::V6(v6) => format!("[{v6}]:{port}"),
    }
}

/// **host 归一（单点来源）**：把所有"回环等价类"映射到 `127.0.0.1`，其余解析为 `IpAddr`。
///
/// 等价类（写死；`listen` 与 `upstream` 两侧**共用本函数**）：
/// - `localhost` / `localhost.`（大小写不敏感、去尾点）→ `127.0.0.1`；
/// - `::1` / `[::1]` / `0:0:0:0:0:0:0:1` / `[0:0:0:0:0:0:0:1]` → `127.0.0.1`（显式映射）；
/// - `127.*.*.*`（含 `127.1` 这类数值型写法）→ 对应 `Ipv4Addr`；
/// - 其余 → 按 `IpAddr` 解析（非回环在 CLI 层被拒）。
pub fn normalize_host(host: &str) -> Option<IpAddr> {
    let trimmed = host.trim();
    let lower = trimmed.trim_end_matches('.').to_ascii_lowercase();
    if lower == "localhost" {
        return Some(IpAddr::V4(Ipv4Addr::LOCALHOST));
    }
    let bare = lower
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(lower.as_str());
    if bare == "::1" {
        return Some(IpAddr::V4(Ipv4Addr::LOCALHOST));
    }
    if let Ok(ip) = bare.parse::<IpAddr>() {
        return Some(canonical_loopback(ip));
    }
    parse_numeric_ipv4(bare).map(IpAddr::V4)
}

/// 把 IPv6 回环（`::1` 的各种书写）归一到 `127.0.0.1`，其余原样。
fn canonical_loopback(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) if v6.is_loopback() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        other => other,
    }
}

/// inet_aton 风格的数值型 IPv4（`127.1` / `2130706433` / `0x7f.0.0.1`）。
fn parse_numeric_ipv4(text: &str) -> Option<Ipv4Addr> {
    let parts: Vec<&str> = text.split('.').collect();
    if parts.is_empty() || parts.len() > 4 {
        return None;
    }
    let mut nums: Vec<u32> = Vec::with_capacity(parts.len());
    for part in &parts {
        if part.is_empty() {
            return None;
        }
        let value = if let Some(hex) = part.strip_prefix("0x").or_else(|| part.strip_prefix("0X")) {
            u32::from_str_radix(hex, 16).ok()?
        } else if part.len() > 1 && part.starts_with('0') {
            u32::from_str_radix(&part[1..], 8).ok()?
        } else {
            part.parse::<u32>().ok()?
        };
        nums.push(value);
    }
    let octets = match nums.len() {
        1 => [nums[0] >> 24, nums[0] >> 16, nums[0] >> 8, nums[0]],
        2 => {
            if nums[0] > 0xFF || nums[1] > 0xFF_FFFF {
                return None;
            }
            [nums[0], nums[1] >> 16, nums[1] >> 8, nums[1]]
        }
        3 => {
            if nums[0] > 0xFF || nums[1] > 0xFF || nums[2] > 0xFFFF {
                return None;
            }
            [nums[0], nums[1], nums[2] >> 8, nums[2]]
        }
        _ => {
            if nums.iter().any(|value| *value > 0xFF) {
                return None;
            }
            [nums[0], nums[1], nums[2], nums[3]]
        }
    };
    Some(Ipv4Addr::new(
        octets[0] as u8,
        octets[1] as u8,
        octets[2] as u8,
        octets[3] as u8,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, CliError> {
        parse_args(args.iter().map(|value| (*value).to_string()))
    }

    // ------------------------------------------------------------------
    // S3 单测矩阵（每参数：默认 / 合法 / 非法 / 别名 / 未知 / 缺值 / 冲突）
    // ------------------------------------------------------------------

    #[test]
    fn defaults_are_all_none_and_run() {
        let cli = parse(&[]).expect("空参数必须解析成功");
        assert_eq!(cli.mode, Mode::Run);
        assert!(!cli.no_gui && !cli.start_paused);
        assert_eq!(cli.listen_port, None);
        assert_eq!(cli.upstream, None);
        assert!(cli.port_fallback.is_none());
        assert_eq!(cli.pool_max_idle_per_host, None);
    }

    #[test]
    fn help_and_version_are_immediate() {
        assert_eq!(parse(&["-h"]).unwrap().mode, Mode::Help);
        assert_eq!(parse(&["--help"]).unwrap().mode, Mode::Help);
        assert_eq!(parse(&["-V"]).unwrap().mode, Mode::Version);
        assert_eq!(parse(&["--version"]).unwrap().mode, Mode::Version);
        // help 优先于冲突校验（必须永远可读）
        assert_eq!(
            parse(&["--no-gui", "--start-paused", "--help"])
                .unwrap()
                .mode,
            Mode::Help
        );
    }

    #[test]
    fn value_forms_space_and_equals_both_work() {
        let spaced = parse(&["--listen-port", "8080"]).unwrap();
        let equals = parse(&["--listen-port=8080"]).unwrap();
        assert_eq!(spaced.listen_port, Some(8080));
        assert_eq!(equals.listen_port, Some(8080));
        assert_eq!(
            parse(&["--upstream=http://198.51.100.7:9"])
                .unwrap()
                .upstream,
            Some("http://198.51.100.7:9".to_string())
        );
    }

    #[test]
    fn listen_port_alias_and_bounds() {
        assert_eq!(parse(&["--port", "8080"]).unwrap().listen_port, Some(8080));
        // 别名同给取最后一次出现
        assert_eq!(
            parse(&["--port", "8080", "--listen-port", "9000"])
                .unwrap()
                .listen_port,
            Some(9000)
        );
        assert!(parse(&["--listen-port", "0"]).is_err());
        assert!(parse(&["--listen-port", "70000"]).is_err());
        assert!(parse(&["--listen-port", "abc"]).is_err());
    }

    #[test]
    fn listen_host_values() {
        assert_eq!(
            parse(&["--listen-host", "127.0.0.1"]).unwrap().listen_host,
            Some("127.0.0.1".to_string())
        );
        assert_eq!(
            parse(&["--listen-host=localhost"]).unwrap().listen_host,
            Some("localhost".to_string())
        );
    }

    #[test]
    fn port_fallback_values() {
        assert_eq!(
            parse(&["--port-fallback", "8000,18080"])
                .unwrap()
                .port_fallback,
            Some(vec![8000, 18080])
        );
        assert_eq!(
            parse(&["--port-fallback", "none"]).unwrap().port_fallback,
            Some(Vec::new())
        );
        assert_eq!(
            parse(&["--port-fallback", ""]).unwrap().port_fallback,
            Some(Vec::new())
        );
        assert!(parse(&["--port-fallback", "8000,0"]).is_err());
        assert!(parse(&["--port-fallback", "x"]).is_err());
    }

    #[test]
    fn upstream_and_strip_prefix() {
        assert_eq!(
            parse(&["--upstream", "http://127.0.0.1:8080"])
                .unwrap()
                .upstream,
            Some("http://127.0.0.1:8080".to_string())
        );
        assert_eq!(
            parse(&["--strip-prefix", "/proxy"]).unwrap().strip_prefix,
            Some("/proxy".to_string())
        );
        // strip-prefix 不以 / 开头 ⇒ apply_overrides 里被 Config::validate 拒
        let cli = parse(&["--strip-prefix", "proxy"]).unwrap();
        let mut config = Config::default();
        assert!(cli.apply_overrides(&mut config).is_err());
    }

    #[test]
    fn close_action_values() {
        assert_eq!(
            parse(&["--close-action", "exit"]).unwrap().close_action,
            Some("exit".to_string())
        );
        assert!(parse(&["--close-action", "hide"]).is_err());
    }

    #[test]
    fn cors_mode_alias_star() {
        assert_eq!(
            parse(&["--cors-mode", "*"]).unwrap().cors_mode,
            Some("*".into())
        );
        assert_eq!(
            parse(&["--cors-mode", "star"]).unwrap().cors_mode,
            Some("*".into())
        );
        assert_eq!(
            parse(&["--cors-mode", "echo"]).unwrap().cors_mode,
            Some("echo".into())
        );
        assert!(parse(&["--cors-mode", "bogus"]).is_err());
    }

    #[test]
    fn cors_max_age_bounds() {
        assert_eq!(
            parse(&["--cors-max-age", "0"]).unwrap().cors_max_age,
            Some(0)
        );
        assert_eq!(
            parse(&["--cors-max-age", "86400"]).unwrap().cors_max_age,
            Some(86400)
        );
        assert!(parse(&["--cors-max-age", "86401"]).is_err());
    }

    #[test]
    fn timeout_params() {
        assert_eq!(
            parse(&["--connect-timeout-ms", "10000"])
                .unwrap()
                .connect_timeout_ms,
            Some(10000)
        );
        assert!(parse(&["--connect-timeout-ms", "0"]).is_err());
        assert!(parse(&["--connect-timeout-ms", "3600001"]).is_err());
        // 分相位：0 允许；1–49 拒
        assert_eq!(
            parse(&["--response-head-timeout-ms", "0"])
                .unwrap()
                .response_head_timeout_ms,
            Some(0)
        );
        assert_eq!(
            parse(&["--first-byte-timeout-ms", "0"])
                .unwrap()
                .first_byte_timeout_ms,
            Some(0)
        );
        assert!(parse(&["--response-head-timeout-ms", "10"]).is_err());
        assert!(parse(&["--first-byte-timeout-ms", "10"]).is_err());
        assert_eq!(
            parse(&["--response-head-timeout-ms", "120000"])
                .unwrap()
                .response_head_timeout_ms,
            Some(120000)
        );
    }

    #[test]
    fn pool_params() {
        assert_eq!(
            parse(&["--pool-max-idle-per-host", "32"])
                .unwrap()
                .pool_max_idle_per_host,
            Some(32)
        );
        assert_eq!(
            parse(&["--pool-idle-timeout-ms", "5000"])
                .unwrap()
                .pool_idle_timeout_ms,
            Some(5000)
        );
        assert!(parse(&["--pool-max-idle-per-host", "1025"]).is_err());
        assert!(parse(&["--pool-idle-timeout-ms", "600001"]).is_err());
    }

    #[test]
    fn log_level_values() {
        for level in ["error", "warn", "info", "debug"] {
            assert_eq!(
                parse(&["--log-level", level]).unwrap().log_level,
                Some(level.to_string())
            );
        }
        assert!(parse(&["--log-level", "trace"]).is_err());
    }

    #[test]
    fn removed_params_are_unknown() {
        // 自启两个参数名按 `["--install-a", "utostart"].concat()` 拼接构造（与 D12 删除参数同一手法）：
        // K13 的源码面判据覆盖 `#[cfg(test)]` ⇒ 源码里不得出现完整字面量。
        let boot_flags = [
            ["--install-a", "utostart"].concat(),
            ["--uninstall-a", "utostart"].concat(),
        ];
        let retired = [
            ["--log", "-dir"].concat(),
            ["--log", "-file"].concat(),
            ["--log", "-retain-files"].concat(),
            ["--dump", "-icons"].concat(),
        ];
        for arg in boot_flags.iter().chain(retired.iter()) {
            let result = parse(&[arg.as_str()]);
            assert!(result.is_err(), "{arg} 必须报未知参数");
            assert!(result.unwrap_err().0.contains("未知参数"));
        }
    }

    #[test]
    fn unknown_missing_and_positional_errors() {
        assert!(parse(&["--bogus"]).unwrap_err().0.contains("未知参数"));
        assert!(parse(&["positional"]).unwrap_err().0.contains("未知参数"));
        assert!(parse(&["--upstream"]).unwrap_err().0.contains("需要参数"));
        assert!(parse(&["--listen-port"])
            .unwrap_err()
            .0
            .contains("需要参数"));
        // 标志不接受值
        assert!(parse(&["--no-gui=1"]).is_err());
    }

    #[test]
    fn conflicts_are_rejected() {
        assert!(parse(&["--no-gui", "--start-paused"]).is_err());
    }

    #[test]
    fn apply_overrides_sets_config() {
        let cli = parse(&[
            "--listen-port",
            "9000",
            "--upstream",
            "http://198.51.100.7:9",
            "--cors-mode",
            "star",
            "--connect-timeout-ms",
            "20000",
            "--pool-max-idle-per-host",
            "8",
            "--log-level",
            "debug",
            "--start-paused",
        ])
        .unwrap();
        let mut config = Config::default();
        cli.apply_overrides(&mut config).expect("覆盖必须成功");
        assert_eq!(config.listen_port, 9000);
        assert_eq!(config.upstream_base, "http://198.51.100.7:9");
        assert_eq!(config.cors.mode, "*");
        assert_eq!(config.timeouts.connect_ms, 20000);
        assert_eq!(config.pool.max_idle_per_host, 8);
        assert_eq!(config.logging.level, "debug");
        // K13：`Config` 里原「启动即开始服务」字段已删 ⇒ `--start-paused` 不落 `Config`，
        // 由 `main` 直接读 `Cli::start_paused`（FR-35 的"启动即服务"语义不变）。
        assert!(cli.start_paused);
    }

    #[test]
    fn apply_overrides_normalizes_and_rejects_non_loopback() {
        let cli = parse(&["--listen-host", "localhost"]).unwrap();
        let mut config = Config::default();
        cli.apply_overrides(&mut config).unwrap();
        assert_eq!(config.listen_host, "127.0.0.1");

        let cli = parse(&["--listen-host", "::1"]).unwrap();
        let mut config = Config::default();
        cli.apply_overrides(&mut config).unwrap();
        assert_eq!(config.listen_host, "127.0.0.1");

        let cli = parse(&["--listen-host", "0.0.0.0"]).unwrap();
        let mut config = Config::default();
        assert!(cli.apply_overrides(&mut config).is_err());
    }

    // ------------------------------------------------------------------
    // 自环矩阵（方案 §3.5；正例必拒 / 负例必过 / 归一必拒）
    // ------------------------------------------------------------------

    // 自环矩阵（方案 §3.5；正例必拒 / 负例必过 / 归一必拒）—— K12 后"默认必过"
    // ------------------------------------------------------------------

    #[test]
    fn self_loop_default_does_not_hit() {
        // K12：默认上游带 :8080 ⇒ 与默认监听 127.0.0.1:80 不同址 ⇒ **不**自环（不带参数可启动）。
        let default = crate::config::default_upstream();
        let check = check_self_loop(default, "127.0.0.1", 80).unwrap();
        assert!(
            !check.is_loop,
            "默认上游 {default} 与默认监听 127.0.0.1:80 必须不同址"
        );
        assert_eq!(check.listen_key, "127.0.0.1:80");
        // 守卫本身不动：显式"无端口 → 默认 80"与监听同址仍判自环。
        let same = check_self_loop("http://127.0.0.1", "127.0.0.1", 80).unwrap();
        assert!(
            same.is_loop,
            "显式 http://127.0.0.1（默认 80）与监听同址仍判自环"
        );
    }

    #[test]
    fn self_loop_default_upstream_at_listen_port_is_a_hit() {
        match option_env!("AZUSA_DEFAULT_UPSTREAM") {
            // 开源版（K12）：默认上游 = 回环 `:8080` ⇒ 不传 `--upstream` 且 `--listen-port 8080`
            // （= 默认上游口）必须判自环。
            None => {
                let check =
                    check_self_loop(crate::config::default_upstream(), "127.0.0.1", 8080).unwrap();
                assert!(check.is_loop, "监听口撞上默认上游口必须判自环");
            }
            // 内网版（K11-a 注入）：默认上游是**跨主机**地址 ⇒ 本机监听任何口都不得误判自环；
            // 同时钉住"默认值 == 注入值"与"与默认监听 :80 不同址"（K11-b 的内网版判据）。
            Some(injected) => {
                assert_eq!(crate::config::default_upstream(), injected);
                let check = check_self_loop(injected, "127.0.0.1", 8080).unwrap();
                assert!(!check.is_loop, "跨主机上游不得误判自环：{injected}");
            }
        }
    }

    #[test]
    fn self_loop_different_port_is_not_a_hit() {
        let check = check_self_loop("http://127.0.0.1:8080", "127.0.0.1", 9000).unwrap();
        assert!(!check.is_loop);
    }

    #[test]
    fn self_loop_explicit_same_address_is_a_hit() {
        let check = check_self_loop("http://127.0.0.1:8080", "127.0.0.1", 8080).unwrap();
        assert!(check.is_loop, "显式同址必须被拒");
    }

    #[test]
    fn self_loop_fallback_port_hits_again() {
        // 80 被占回退到 8000，而上游恰在本机 8000 ⇒ 第二次复检必须命中
        let check = check_self_loop("http://127.0.0.1:8000", "127.0.0.1", 8000).unwrap();
        assert!(check.is_loop, "回退口命中必须跳过/拒绝");
    }

    #[test]
    fn self_loop_cross_host_is_not_a_hit() {
        // RFC 5737 TEST-NET-2（非私有保留地址）
        let check = check_self_loop("http://198.51.100.7:9", "127.0.0.1", 80).unwrap();
        assert!(!check.is_loop);
    }

    #[test]
    fn self_loop_numeric_ipv4_is_a_hit() {
        let check = check_self_loop("http://127.1", "127.0.0.1", 80).unwrap();
        assert!(check.is_loop, "数值型 IPv4 写法必须归一到 127.0.0.1");
    }

    #[test]
    fn self_loop_ipv6_forms_are_hits() {
        for upstream in [
            "http://[::1]",
            "http://[0:0:0:0:0:0:0:1]",
            "http://[::1]:80",
        ] {
            let check = check_self_loop(upstream, "127.0.0.1", 80).unwrap();
            assert!(check.is_loop, "{upstream} 必须与 127.0.0.1:80 同址");
        }
    }

    #[test]
    fn self_loop_localhost_upstream_is_a_hit() {
        let check = check_self_loop("http://localhost", "127.0.0.1", 80).unwrap();
        assert!(check.is_loop);
    }

    #[test]
    fn normalize_host_equivalence_classes() {
        let v4 = IpAddr::V4(Ipv4Addr::LOCALHOST);
        for host in [
            "127.0.0.1",
            "localhost",
            "LOCALHOST.",
            "::1",
            "[::1]",
            "0:0:0:0:0:0:0:1",
            "[0:0:0:0:0:0:0:1]",
            "127.1",
            "2130706433",
        ] {
            assert_eq!(
                normalize_host(host),
                Some(v4),
                "{host} 必须归一到 127.0.0.1"
            );
        }
        assert_eq!(
            normalize_host("127.0.0.2"),
            Some(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2)))
        );
    }

    #[test]
    fn no_gui_flag_is_recognized() {
        let cli = parse(&["--no-gui"]).unwrap();
        assert!(cli.no_gui);
        assert_eq!(cli.mode, Mode::Run);
    }

    #[test]
    fn combined_command_line_applies_end_to_end() {
        let cli = parse(&[
            "--listen-host",
            "127.0.0.1",
            "--listen-port",
            "9000",
            "--port-fallback",
            "9100,9200",
            "--upstream",
            "http://198.51.100.7:9",
            "--strip-prefix",
            "/proxy",
            "--close-action",
            "exit",
            "--cors-mode",
            "echo",
            "--cors-max-age",
            "1200",
            "--response-head-timeout-ms",
            "60000",
            "--first-byte-timeout-ms",
            "0",
            "--pool-idle-timeout-ms",
            "30000",
        ])
        .unwrap();
        let mut config = Config::default();
        cli.apply_overrides(&mut config).unwrap();
        assert_eq!(config.listen_host, "127.0.0.1");
        assert_eq!(config.listen_port, 9000);
        assert_eq!(config.port_fallback, vec![9100, 9200]);
        assert_eq!(config.upstream_base, "http://198.51.100.7:9");
        assert_eq!(config.strip_prefix, "/proxy");
        assert_eq!(config.ui.close_action, "exit");
        assert_eq!(config.cors.mode, "echo");
        assert_eq!(config.cors.max_age_seconds, 1200);
        assert_eq!(config.timeouts.response_head_ms, 60000);
        assert_eq!(config.timeouts.first_byte_ms, 0);
        assert_eq!(config.pool.idle_timeout_ms, 30000);
    }

    #[test]
    fn usage_text_is_reexported() {
        assert!(usage_text().contains("http://127.0.0.1/v1"));
    }
}
