//! build.rs —— ① 「默认上游」的编译期注入（K11-a 双产物，与 `TARGET` 无关，任何目标都做）；
//! ② 仅在 `TARGET` 含 `win7` 时注入 Win7 两层链接（VC-LTL5 + YY-Thunks）。
//!
//! 接线口径 = 方案 §5.5.3（对应指南 §3.3 的三条细节）；资产放置表 = 方案 §5.5.2。
//! 资产按 DG14「按需取件」（`scripts/fetch-assets.ps1`）：缺失时构建**大声失败**，绝不静默降级。

/// 注入源（K11-a ②）：本地**gitignored**文件 —— 内网版默认上游的常驻来源。
/// 内容 = 一行 URL。⚠ 绝不入库（开源红线 D13；`.gitignore` 已列）。
const INTERNAL_DEFAULT_FILE: &str = ".internal-default.txt";
/// 注入源（K11-a ①）：环境变量（`scripts/build-*.ps1 -Internal` / `-InternalDefault <url>` 与手工构建走它）。
const DEFAULT_UPSTREAM_ENV: &str = "AZUSA_DEFAULT_UPSTREAM";
/// **显式关闭信号**（本实现增补；见 [`inject_default_upstream`] 的说明）。
///
/// ⚠ 必须是**另一个**环境变量名，不能把"off"塞进 [`DEFAULT_UPSTREAM_ENV`]：
/// cargo 给 rustc 传的 `cargo:rustc-env` 之外，**进程环境变量本身对 rustc 也可见** ⇒
/// `option_env!("AZUSA_DEFAULT_UPSTREAM")` 会读到那个哨兵，产出一个"默认上游 = off"的坏二进制
/// （B4b 实测踩到：`--help` 打出 `默认上游 off`、exe 少 512 B；改用独立变量名后 `--help` = 4,336 B ✓）。
const NO_INJECT_ENV: &str = "AZUSA_DEFAULT_UPSTREAM_OFF";
/// 开源版回退值（K12）—— 与 `src/config.rs` 的 `option_env!(...).unwrap_or(...)` 字面量**必须一致**
/// （两处是同一个口径的两个落点；改一处必改另一处，`cargo test` 的
/// `default_upstream_matches_the_compiled_variant` 会抓住不一致）。
const OPEN_DEFAULT_UPSTREAM: &str = "http://127.0.0.1:8080";

fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    // `--version` 展示用：把编译目标三元组注入 crate（`src/output.rs` 的 `BUILD_TARGET`）。
    if !target.is_empty() {
        println!("cargo:rustc-env=AZUSA_BUILD_TARGET={target}");
    }
    inject_default_upstream();
    // 可复现构建（审查轮 P2-3）：MSVC 链接器默认把"构建时间"写进 PE 头 ⇒ 同源多次构建 sha256 三不同。
    // `/Brepro` 让时间戳由输入哈希导出；实测加该参数后同源两次构建逐字节一致（1,625,600 B / 同一 sha256）。
    if target.contains("msvc") {
        println!("cargo:rustc-link-arg=/Brepro");
    }
    embed_resources();

    if target.contains("win7") {
        inject_win7_link_layers(&target);
    }
}

/// K11-a / K12：「默认上游」的**编译期注入** —— 双产物（内网版 / 开源版）的实现落点。
///
/// 两个产物**同源同码**，唯一差异 = 这个字符串 ⇒ 没有"两套代码各修一次"的维护债；
/// 源码**永不**含内网地址（D13：tracked 文件 0 命中），内网默认值只在构建时进入二进制。
///
/// 来源优先级（K11-a）：
///   ① 环境变量 [`DEFAULT_UPSTREAM_ENV`]；
///   ② [`INTERNAL_DEFAULT_FILE`]（**gitignored**；存在则取第一行）；
///   ③ 缺省 ⇒ **不注入**（`src/config.rs` 的 `option_env!` 回退到 [`OPEN_DEFAULT_UPSTREAM`]）。
///
/// **本实现增补一级"显式关闭"**：`AZUSA_DEFAULT_UPSTREAM_OFF=1`（真值）⇒ **连 ② 也不读**、
/// 明确不注入。理由：本地树里 ② 是常驻的（内网版靠它开箱即用），而"② 存在"会污染**所有**
/// cargo 调用 —— 开源版构建（`scripts/build-{win7,modern}.ps1` 的默认臂）必须能**确定地**
/// 说"不要读它"，否则开源版会静默带上内网地址。
/// 哨兵是唯一确定性手段（临时改名 ② 的做法在构建中断时留下脏态，不作取）。
/// ⚠ 关闭信号**必须用独立变量名**（[`NO_INJECT_ENV`]）—— 见该常量的说明（`off` 塞进
/// [`DEFAULT_UPSTREAM_ENV`] 会被 rustc 的 `option_env!` 读到，产出一个默认上游 = `off` 的坏二进制）。
///
/// 注入方式 = `cargo:rustc-env`（`src/config.rs` 的 `option_env!` 消费）+ `rerun-if-*`。
/// ⚠ `rerun-if-changed` 只是**尽力而为**：它对"注入源缺失 → 以**旧 mtime** 原样复原"**不敏感**
/// （cargo 把"缺失"记成时间基准，早于基准的 mtime 视为未变）—— 实测：原样移回注入源后 `Compiling=0`、
/// 产物仍是上一个变体；且 `cargo:warning` 会被 cargo **重放**（同状态连跑两次仍有那一行）。
/// ⇒ **变体不由本脚本的告警行见证**：两个构建脚本会在构建前 `touch` 注入源（强制重跑），
/// 并在产物就位后于 **exe 字节**上跑 `Assert-Variant` 自证（P6-B4b 修正轮 P1-1）。
fn inject_default_upstream() {
    println!("cargo:rerun-if-env-changed={DEFAULT_UPSTREAM_ENV}");
    println!("cargo:rerun-if-env-changed={NO_INJECT_ENV}");
    println!("cargo:rerun-if-changed={INTERNAL_DEFAULT_FILE}");

    match resolve_default_upstream() {
        None => {
            println!(
                "cargo:warning=本次默认上游 = {OPEN_DEFAULT_UPSTREAM}（变体：开源；来源：缺省回退或显式关闭）"
            );
        }
        Some((value, source)) => {
            assert_injectable_default_upstream(&value, source);
            println!("cargo:rustc-env={DEFAULT_UPSTREAM_ENV}={value}");
            println!("cargo:warning=本次默认上游 = {value}（变体：内网；来源：{source}）");
        }
    }
}

/// K11-a 的三级来源（+本实现增补的"显式关闭"信号）。返回 `None` = 不注入。
fn resolve_default_upstream() -> Option<(String, &'static str)> {
    if std::env::var(NO_INJECT_ENV)
        .map(|raw| is_truthy(&raw))
        .unwrap_or(false)
    {
        return None;
    }
    if let Ok(raw) = std::env::var(DEFAULT_UPSTREAM_ENV) {
        let value = raw.trim().trim_start_matches('\u{feff}').trim();
        if !value.is_empty() {
            return Some((value.to_string(), "环境变量 AZUSA_DEFAULT_UPSTREAM"));
        }
    }
    let text = std::fs::read_to_string(INTERNAL_DEFAULT_FILE).ok()?;
    let first = text
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .trim_start_matches('\u{feff}')
        .trim();
    if first.is_empty() {
        println!(
            "cargo:warning={INTERNAL_DEFAULT_FILE} 存在但其第一行为空 ⇒ 不注入（按开源版处理）"
        );
        return None;
    }
    Some((first.to_string(), INTERNAL_DEFAULT_FILE))
}

/// [`NO_INJECT_ENV`] 的真值判定（`1` / `true` / `yes` / `on`，大小写不敏感）。
fn is_truthy(value: &str) -> bool {
    let value = value.trim();
    ["1", "true", "yes", "on"]
        .iter()
        .any(|t| value.eq_ignore_ascii_case(t))
}

/// 注入值必须能被 `--upstream` 接受（否则会产出一个"自己的默认值不可用"的二进制 ⇒ 大声失败）。
/// 三条同口径（与 `scripts/build-{win7,modern}.ps1` 的脚本侧校验一致）：
/// ① `http://` 前缀；② 无空白/控制字符；③ **host 段非空**（`http://` / `http://:8080` 这类值
/// 能通过前两条检查、却给出一个不可用的默认上游 —— P6-B4b 修正轮 P2-3）。
fn assert_injectable_default_upstream(value: &str, source: &str) {
    assert!(
        value.starts_with("http://"),
        "默认上游注入值必须是 http:// 地址（本程序不支持 https 上游）：source={source} value={value:?}"
    );
    assert!(
        !value.chars().any(|c| c.is_whitespace() || c.is_control()),
        "默认上游注入值不得含空白或控制字符：source={source} value={value:?}"
    );
    let host = value["http://".len()..]
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("");
    assert!(
        !host.is_empty(),
        "默认上游注入值缺 host 段（形如 http:// 或 http://:8080 的值不可用）：source={source} value={value:?}"
    );
}

/// 本项目自有嵌入：`.ico` 四态图标 + comctl32 v6 / DPI 感知清单 + 版本信息（方案 §5.2）。
fn embed_resources() {
    println!("cargo:rerun-if-changed=assets/app.ico");
    println!("cargo:rerun-if-changed=assets/app.manifest");

    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set("FileDescription", "AzusaAI 本地反向代理");
        res.set("ProductName", "AzusaAI 本地反向代理");
        res.set("CompanyName", "AzusaAI WebUI");
        res.set("FileVersion", env!("CARGO_PKG_VERSION"));
        res.set("ProductVersion", env!("CARGO_PKG_VERSION"));
        res.set("OriginalFilename", "azusa-local-proxy.exe");
        res.set("InternalName", "azusa-local-proxy");
        res.set("LegalCopyright", "(c) AzusaAI WebUI · GPL-3.0");
        res.set_icon("assets/app.ico");
        res.set_manifest_file("assets/app.manifest");
        if let Err(err) = res.compile() {
            panic!(
                "资源嵌入失败（.ico / 清单 / 版本信息）：{err}。\
                 请确认装有 Windows SDK（rc.exe）与 MSVC 工具链；必要时用 RC_PATH 指定 rc.exe。"
            );
        }
    }
}

/// Win7 两层接线（指南 §3.3 的三条细节，逐字采用）：
/// ① 必须按 `TARGET` 收口 —— 否则普通 Windows 构建会静默链上 Vista 档 CRT
///    （`rustc-link-search` 变成 `-LIBPATH` 并排在 SDK 搜索顺序之前，同名替换库才赢得过 SDK）；
/// ② obj 必须出现在链接命令行上（`rustc-link-arg`），只"被引用"不算；
/// ③ `/NODEFAULTLIB:kernel32.lib` + `kernel32.lib` 是**顺序保险**，仅此一种作用域形态
///    —— 永远不要写裸的 `/NODEFAULTLIB`（会把 CRT 输入依赖的 `/DEFAULTLIB` 一并剥掉）。
fn inject_win7_link_layers(target: &str) {
    let (arch, obj) = if target.contains("x86_64") {
        ("x64", "YY_Thunks_for_Win7.obj")
    } else {
        ("x86", "YY_Thunks_for_Win7_x86.obj")
    };

    assert_win7_assets(arch, obj);

    println!("cargo:rustc-link-search=native=assets/vc-ltl/{arch}"); // 第 1 层：VC-LTL5
    println!("cargo:rustc-link-search=native=assets/yy-thunks"); // 第 2 层：YY-Thunks
    println!("cargo:rustc-link-arg={obj}");
    println!("cargo:rustc-link-arg=/NODEFAULTLIB:kernel32.lib");
    println!("cargo:rustc-link-arg=kernel32.lib");
    println!("cargo:warning=VC-LTL5 v5.3.1 + YY-Thunks v1.2.2 ({arch}) 已注入 {target}");
}

/// DG14 失败语义：资产缺失 / 不齐 ⇒ 构建直接失败（对应指南 §3.2「缺失必须大声失败」，
/// 与其等链接器的 LNK1181，不如在这里给出可操作的指引）。
fn assert_win7_assets(arch: &str, obj: &str) {
    let lib_dir = format!("assets/vc-ltl/{arch}");
    let expected = [
        "libucrt.lib",
        "libvcruntime.lib",
        "ucrt.lib",
        "vcruntime.lib",
    ];
    let mut found: Vec<String> = Vec::new();
    match std::fs::read_dir(&lib_dir) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.ends_with(".lib") {
                    found.push(name.to_string());
                }
            }
        }
        Err(err) => panic!(
            "Win7 资产缺失：目录 {lib_dir} 不存在或不可读（{err}）。\n\
             请先运行：powershell -NoProfile -ExecutionPolicy Bypass -File scripts/fetch-assets.ps1"
        ),
    }
    found.sort();
    let expected_sorted: Vec<String> = expected.iter().map(|name| name.to_string()).collect();
    if found != expected_sorted {
        panic!(
            "Win7 资产不齐：{lib_dir} 期望恰 4 个 release .lib {expected_sorted:?}，实际 {found:?}。\n\
             请重新运行：scripts/fetch-assets.ps1（或删除 assets/vc-ltl 后重跑）"
        );
    }

    let obj_path = format!("assets/yy-thunks/{obj}");
    if !std::path::Path::new(&obj_path).exists() {
        panic!(
            "Win7 资产缺失：{obj_path} 不存在。\n\
             请先运行：scripts/fetch-assets.ps1\
             （注意：x86 源归档内名为 YY_Thunks_for_Win7.obj（无 _x86 后缀），_x86 改名只发生在放置端）"
        );
    }
}
