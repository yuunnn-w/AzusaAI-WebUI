//! 「关于」→ **使用教程**：8 章正文 + 章节偏移表 + 长参数名单（P7-I2；D2 文档窗教程模式）。
//!
//! 口径（方案 §2.7.3 / §2.7.4）：
//! - **写作口径 = 指向上手**（不是特性罗列）：每章回答"我现在该看哪一章 / 这一步要看什么读数"；
//! - **与 `--help` 单向一致**（防漂移，且**不改 `--help`**）：[`CLI_LONG_FLAGS`] 的每一项必须
//!   同时出现在 `output::usage_text()`（帮助全文，受 4,336 B 字节级判据保护）与本文件的教程正文里
//!   （判据 A-6）；`cli.rs` / `output.rs` 一个字都不动；
//! - **章节偏移 = UTF-16 码元下标**（不是 UTF-8 字节下标）：`EM_SETSEL` 吃的是 Unicode EDIT 的
//!   码元位置 ⇒ 用 [`utf16_len`] 现算（判据 A-7 断言严格递增 + 落在字符边界上 + 章节数 == 8）；
//! - 运行 OS / 版本两行（原「关于」弹窗的 Open-6 两行）**移进第八章末「环境」小段**。

/// 教程正文 + 章节表（`chapters` = 每章起点的 **UTF-16 码元偏移**，与正文同源一次性算出）。
pub struct AboutText {
    pub body: String,
    pub chapters: Vec<(&'static str, usize)>,
}

/// 章节条标签（8 段；短标签 —— 章节条每段宽 = 内容宽 / 8；渲染时前缀 `N ` 由绘制方加）。
///
/// **每个标签必须原样出现在对应章节的标题行里**（[`chapter_heading`]）⇒ 章节条点击后的落点
/// 与标签可见地一致（A-7 断言"偏移处那一行就是该章标题行"）。
pub const CHAPTER_LABELS: [&str; 8] = [
    "这是什么",
    "快速开始",
    "地址怎么填",
    "参数说明",
    "常见问题",
    "排错",
    "安全与保密",
    "许可与环境",
];

/// 章节序号（标题行用中文数字：`第一章 · 这是什么`）。
const CHAPTER_NUMERALS: [&str; 8] = ["一", "二", "三", "四", "五", "六", "七", "八"];

/// 第 `index` 章（0 基）的标题行首（正文每章的第一行必须以此开头）。
pub fn chapter_heading(index: usize) -> String {
    format!(
        "第{}章 · {}",
        CHAPTER_NUMERALS[index.min(CHAPTER_NUMERALS.len() - 1)],
        CHAPTER_LABELS[index.min(CHAPTER_LABELS.len() - 1)]
    )
}

/// **长参数名单**（`--` 开头的全部参数；A-6 的检查面 = 每一项同时在 `--help` 与教程正文里）。
///
/// 维护口径：`--help` 增删参数 ⇒ 这里同改（单向一致性单测 `about_mentions_every_cli_flag` 会红）。
///
/// `#[allow(dead_code)]`：本名单是**判据面**（单测 + 审查读它），产品路径不消费它 —— 不为消警告而删
/// （仓库先例：`theme.rs` 的 `font_section`）。
#[allow(dead_code)]
pub const CLI_LONG_FLAGS: &[&str] = &[
    "--listen-host",
    "--listen-port",
    "--port",
    "--port-fallback",
    "--upstream",
    "--strip-prefix",
    "--connect-timeout-ms",
    "--response-head-timeout-ms",
    "--first-byte-timeout-ms",
    "--pool-max-idle-per-host",
    "--pool-idle-timeout-ms",
    "--cors-mode",
    "--cors-max-age",
    "--log-level",
    "--start-paused",
    "--close-action",
    "--no-gui",
    "--version",
    "--help",
];

/// UTF-16 码元长度（`EM_SETSEL` 的偏移口径；与 `String::len()`（UTF-8 字节）**不同**）。
pub fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// 教程正文（运行期拼一次；版本/OS 两行来自 `CARGO_PKG_VERSION` 与 `osver`）。
pub fn tutorial_text() -> AboutText {
    let version = env!("CARGO_PKG_VERSION");
    let os = azusa_local_proxy::osver::current()
        .map(|value| {
            format!(
                "{} {}（深色标题栏：{}）",
                value.name(),
                value.display(),
                if value.supports_dark_title_bar() {
                    "启用"
                } else {
                    "按版本闸跳过"
                }
            )
        })
        .unwrap_or_else(|| "未知（RtlGetVersion 不可用）".to_string());
    tutorial_text_for(version, &os)
}

/// 正文构造的**唯一实现**（版本/OS 由调用方给 ⇒ 单测可注入固定值）。
pub fn tutorial_text_for(version: &str, os: &str) -> AboutText {
    let mut body = String::with_capacity(16 * 1024);
    let mut chapters: Vec<(&'static str, usize)> = Vec::with_capacity(8);

    push_chapter(&mut body, &mut chapters, 0, CH1);
    push_chapter(&mut body, &mut chapters, 1, CH2);
    push_chapter(&mut body, &mut chapters, 2, CH3);
    push_chapter(&mut body, &mut chapters, 3, CH4);
    push_chapter(&mut body, &mut chapters, 4, CH5);
    push_chapter(&mut body, &mut chapters, 5, CH6);
    push_chapter(&mut body, &mut chapters, 6, CH7);
    // 第八章 = 固定正文 + 运行期「环境」小段（版本 / OS / 构建目标）
    {
        chapters.push((CHAPTER_LABELS[7], utf16_len(&body)));
        body.push_str(CH8);
        body.push_str(&format!(
            "环境（本次运行）\n  版本：v{version}\n  运行 OS：{os}\n"
        ));
        body.push_str(
            "  构建目标：Windows 7 SP1+ x64（win7 目标 + VC-LTL5/YY-Thunks 两层垫片）\n\
             \n\
             这一页的正文可以整段选中复制（正文框右上角有「复制全文」）；参数全文以「--help」为准。\n",
        );
    }
    AboutText { body, chapters }
}

fn push_chapter(
    body: &mut String,
    chapters: &mut Vec<(&'static str, usize)>,
    index: usize,
    text: &str,
) {
    chapters.push((CHAPTER_LABELS[index], utf16_len(body)));
    body.push_str(text);
}

const CH1: &str = "\
第一章 · 这是什么

  本程序 = 一个本机反代：把你机器上的 127.0.0.1:<端口> 收到的请求，原样转发到你指定的
  模型服务（上游），并把响应流式回传。它是给「客户端只能填 127.0.0.1、而模型服务在别处」
  这种场景用的：客户端不用改代码、不用装证书，只把 Base URL 指向本机端口即可。

  你需要它吗：如果应用（或它的浏览器环境）只允许 http://127.0.0.1/... 作为 Base URL，
  而真实模型服务不在本机 —— 需要。如果应用本来就能直连模型服务地址 —— 不需要。

  你现在该看哪一章：
    · 第一次用          → 第二章（快速开始，3 步）
    · 连接报错 / 404    → 第三章（地址怎么填）+ 第五章（常见问题）
    · 想调超时 / 连接池 → 第四章（参数说明）
    · 要在内网说明它做什么 → 第七章（安全与保密）

  本程序没有配置文件：全部参数走命令行，界面里的改动只对本次运行生效，关掉即回到默认值。
  日志只在内存（最近 2000 条），退出即消失 —— 要留存请用「查看日志」→「复制全部」。

";

const CH2: &str = "\
第二章 · 快速开始

  第 1 步：双击 exe
    默认监听 127.0.0.1:80；80 被占用时按候选端口回退（默认 8000,18080），主面板会显示
    实际监听地址（不是请求的那个）。若两个回退口也被占 ⇒ 面板显示「未监听」并给出提示。

  第 2 步：把客户端的 Base URL 指向本机
    设置 → 模型 → Base URL 填 http://127.0.0.1/v1
    （非 80 端口时写 http://127.0.0.1:<实际端口>/v1；端口见主面板第一行）
    面板上的「复制 Base URL」按钮就是这一串，点一下进剪贴板。

  第 3 步：发一条真实对话
    先在应用里关闭「预检规避兼容模式」，然后发一条消息。
    成功判据 = 回复逐字流式出现；失败判据与三读法见第五章。

  这一步之后还没通，按顺序看：第五章（常见问题）→ 第六章（排错，含日志怎么取）。

";

const CH3: &str = "\
第三章 · 地址怎么填

  · 端口：默认 80 ⇒ Base URL 里不写端口（http://127.0.0.1/v1）。用了别的端口就必须写全
    （http://127.0.0.1:18080/v1）—— 端口以主面板显示的实际地址为准（回退后端口会变）。
  · 路径：本程序同时支持 OpenAI 兼容（/v1/chat/completions）与 Anthropic（/v1/messages）；
    客户端里 Base URL 填到 /v1 即可，具体端点由客户端自己拼。
  · 为什么不是 8080：8080 是上游常见的本机端口。Base URL 要填的是本程序的监听口
    （127.0.0.1:80），不是上游的 8080 —— 这两个数最容易填反。
  · 剥前缀：上游要求带路径前缀（例如只接受 /gateway/v1/...）时，用 --strip-prefix 把客户端
    多写的前缀剥掉；Base URL 侧不用改。
  · 「复制 Base URL」在服务没起时不可用（还没有实际地址）—— 先点「启动」。

";

const CH4: &str = "\
第四章 · 参数说明

  写法两种都行：--参数 值 或 --参数=值。下面每条给「默认值 + 什么时候改」。

  监听与转发
    --listen-host <ip>            只接受回环地址（默认 127.0.0.1；非回环一律拒绝）
                                  改它：基本不用改（本程序只监听本机）。
    --listen-port <n>             监听端口 1-65535（默认 80）；别名 --port
                                  改它：80 要留给别的服务，或没有管理员/端口权限时。
    --port-fallback <列表>         被占用时的候选端口，逗号分隔（默认 8000,18080；none = 不回退）
                                  改它：想让回退口固定（例如防火墙只放行某个口）时。
    --upstream <url>              上游 http:// 地址（默认指向本机 8080 的模型服务；不支持 https）
                                  改它：模型服务不在本机 8080 时——最常改的一条。
                                  内网部署写成 --upstream http://<内网模型主机>。
    --strip-prefix <路径>         剥掉监听侧路径前缀（默认空 = 不动；例：/proxy）
                                  改它：客户端会在路径里多带一层前缀时。

  超时（单位毫秒）
    --connect-timeout-ms <ms>           连接建立超时（默认 10000；不允许 0）
                                        改它：上游在内网、握手慢时报「连接超时」时放大。
    --response-head-timeout-ms <ms>     等上游响应头的上限（默认 120000；0 = 无限等，不推荐）
                                        改它：上游冷启动很慢 ⇒ 保持或放大；soak/压测 ⇒ 调小（如 3000）
                                        好让「卡住」尽早暴露。
    --first-byte-timeout-ms <ms>        响应头之后、首帧之前的看门狗（默认 0 = 不设）
                                        改它：上游会「先回头发包、再长时间不出字」时设一个值。

  连接池与 CORS
    --pool-max-idle-per-host <n>        每个主机的空闲连接上限（默认 16）
                                        改它：并发很高、上游连接数吃紧时收紧。
    --pool-idle-timeout-ms <ms>         空闲连接保留期（默认 10000）
                                        改它：上游会主动断开空闲连接 ⇒ 调小；想省握手 ⇒ 调大。
    --cors-mode <*|star|echo>           跨源头模式（默认 *；star 是 * 的别名，等价）
                                        改它：需要按请求回显 Origin（多域场景）时用 echo。
    --cors-max-age <秒>                 预检 Max-Age（默认 600）
                                        改它：想让浏览器少发预检请求时调大。

  日志（只在内存，不写磁盘）
    --log-level <error|warn|info|debug> 日志级别（默认 info）
                                        改它：排障时用 debug（记录更细，代价是日志长得快）。

  启动与退出
    --start-paused              启动后不自动开始服务（到界面点「启动」）；与 --no-gui 互斥
                                改它：想先看设置再放流量时。
    --close-action <tray|exit>  关闭按钮行为：tray = 最小化到托盘（默认）；exit = 直接退出
                                改它：脚本/无人值守场景想要「关掉就退出」时。

  其它
    --no-gui                    无界面运行（脚本 / 排障）；与 --start-paused 互斥
    --version, -V               显示版本
    -h, --help                  显示本帮助（全文；本文只是上手版）

";

const CH5: &str = "\
第五章 · 常见问题

  1) 应用仍报 Failed to fetch —— 三读法
     ① 面板上的实际监听地址：是不是「未监听」（端口全被占）？
     ② Base URL 的端口 / 路径是否与面板一致（见第三章）；
     ③ 点「查看日志」，看有没有「本地预检」计数 / 转发记录 —— 一条都没有 = 请求根本没进来。

  2) 上游 404（而不是报错）
     多半是路径拼接：客户端 Base URL 少了 /v1，或上游需要路径前缀而没配 --strip-prefix。
     先试把 Base URL 补成 http://127.0.0.1/v1，再看上游是否要求前缀。

  3) 端口被占用
     默认 80 被占 ⇒ 按 --port-fallback 列表（默认 8000,18080）依次回退，面板显示实际地址；
     全被占 ⇒ 面板提示「未监听」（Windows 报 10048 / 10013：10048 = 端口已用，
     10013 = 权限被拒 —— 例如被系统保留段占用）。处理：换 --listen-port，或释放占用端口的程序。

  4) 上游不可达
     会看到带原因的 502（含上游地址；不是 Failed to fetch）—— 说明代理这一层是通的、
     问题在上游。查上游进程 / 地址 / 防火墙；上游恢复后无需重启本程序。

";

const CH6: &str = "\
第六章 · 排错

  · 「CORS 分层」面板的预期读数（应用里的「运行自检」）：
    显示「· 简单 POST HTTP 400(未预检)」属预期（浏览器没发预检时的正常表现），不是故障。
    判「修好了」看两件事：① P1/P2/P4 三行可读；② 发一条真实对话能正常流式输出。

  · 日志怎么取：主面板「查看日志」（或设置窗的「查看日志」）→ 文档窗右上角「复制全部」
    → 剪贴板。日志只在内存，退出即消失 ⇒ 需要留证请当场复制。
    「复制日志」（不带「全部」）是主面板的快捷项：最近 100 行、每行不截断。

  · 上游「卡住不回」：先看是不是上游慢；确认后调 --response-head-timeout-ms（见第四章）。
    代理侧的超时不是「掐断正常长响应」——流式响应只要头到了就会一直转发。

  · 代理自己起不来：看主面板结果行 / 日志里的「监听」与「自环」两行。上游与本程序监听地址
    同址会被拒绝启动（退出码 2）——改 --upstream 或 --listen-port 之一即可。

";

// 第七章用 raw 字符串：正文里有 Windows 路径反斜杠（reg 参考命令）。
const CH7: &str = r#"第七章 · 安全与保密

  「本程序不做什么」8 条（完整版与可核依据见 rust-proxy/README.md「安全与保密」）：
    ① 不读取、不写入任何文件：无配置文件，日志只在内存（唯四豁免：系统加载 exe 与垫片、
       读自身嵌入的图标资源、你显式点的复制到剪贴板、控制台设备句柄）；
    ② 不写注册表：程序内没有任何注册表读写代码（开机自启功能已整体移除，两个 --*-autostart
       参数不存在，传入即报「未知参数」）；
    ③ 不注入进程 / 不挂钩 / 不截屏 / 不键盘记录；
    ④ 不 spawn 子进程（含 cmd / powershell）：运行期子进程数恒为 0；
    ⑤ 除你指定的上游外零外联：无遥测、无更新检查、不做外部 DNS；
    ⑥ 只监听回环：默认且只能绑 127.0.0.1；
    ⑦ 不打包 / 不混淆 / 不加密 / 不加壳；manifest 未声明提权 ⇒ 不触 UAC；
    ⑧ 源码 / 文档 / 示例不含任何内网地址与服务标识（示例一律 127.0.0.1 与占位符）。

  旧版本自启遗留项的清理（参考）：P6 之前装过自启的机器上，遗留值请手动删 ——
  reg delete "HKCU\Software\Microsoft\Windows\CurrentVersion\Run" /v AzusaAI-LocalProxy /f
  这只是参考命令，程序不再提供该能力。

  运行期唯一的外部数据流向 = 你填的上游地址（日志、状态、读数全在本机内存里）。

"#;

const CH8: &str = "\
第八章 · 许可与环境

  许可证：GPL-3.0（全文见仓库根 LICENSE）。
  第三方：静态垫片 VC-LTL5（EPL-2.0）与 YY-Thunks（MIT），随 exe 一起静态链接，
  仅依赖系统 msvcrt.dll；声明见 rust-proxy/THIRD_PARTY_NOTICES.md。
  源码位置（纯文本，可复制）：
    https://github.com/yuunnn-w/AzusaAI-WebUI （目录 rust-proxy/）

";

#[cfg(test)]
mod tests {
    use super::*;

    /// 章节条 8 段（与正文一次性算出的章节表同长）。
    #[test]
    fn chapter_bar_has_eight_labels() {
        assert_eq!(CHAPTER_LABELS.len(), 8);
        assert_eq!(tutorial_text().chapters.len(), 8);
    }

    /// **A-7**：章节偏移严格递增、落在字符边界内、章节数 == 8，且偏移处正是该章标题行。
    #[test]
    fn chapter_offsets_are_monotonic_inside_the_text() {
        let text = tutorial_text();
        assert_eq!(text.chapters.len(), 8, "教程必须 8 章");
        let chars: Vec<char> = text.body.chars().collect();
        let mut previous = 0usize;
        for (index, (label, offset)) in text.chapters.iter().enumerate() {
            assert_eq!(
                *label,
                CHAPTER_LABELS[index],
                "章节表第 {} 项标签必须与章节条一致",
                index + 1
            );
            assert!(
                index == 0 || *offset > previous,
                "章节偏移必须严格递增：第 {} 章 offset={offset} 未超过上一章 {previous}",
                index + 1
            );
            assert!(
                *offset < chars.len(),
                "章节偏移必须落在正文内：第 {} 章 offset={offset} ≥ 正文字符数 {}",
                index + 1,
                chars.len()
            );
            // 偏移处那一行必须是该章标题行（逐字符还原 ⇒ 也证明偏移没劈开代理对）
            let first_line: String = chars[*offset..]
                .iter()
                .take_while(|ch| **ch != '\n')
                .collect();
            assert_eq!(
                first_line,
                chapter_heading(index),
                "第 {} 章偏移 {offset} 处不是该章标题行",
                index + 1
            );
            previous = *offset;
        }
        // 首章必须从 0 开始（否则正文开头的"前言"跳不过去）
        assert_eq!(text.chapters[0].1, 0);
        // 本正文全为 BMP 字符（无代理对）⇒ 码元数 == 字符数（偏移口径自证）
        assert_eq!(
            utf16_len(&text.body),
            text.body.chars().count(),
            "正文若引入非 BMP 字符（emoji），UTF-16 偏移与字符下标不再等价 ⇒ 单测要同步改"
        );
    }

    /// **A-6**：`CLI_LONG_FLAGS` 每一项同时出现在 `--help`（帮助全文）与教程正文里。
    #[test]
    fn about_mentions_every_cli_flag() {
        let help = azusa_local_proxy::output::usage_text();
        let tutorial = tutorial_text().body;
        assert!(!CLI_LONG_FLAGS.is_empty(), "名单不得为空（否则本判据空转）");
        for flag in CLI_LONG_FLAGS {
            assert!(
                flag.starts_with("--"),
                "名单里必须写全 `--` 前缀（便于直接 contains）：{flag}"
            );
            assert!(
                help.contains(flag),
                "帮助全文里找不到 {flag}（`--help` 改了名 ⇒ 教程与名单要同改）"
            );
            assert!(
                tutorial.contains(flag),
                "教程正文里找不到 {flag}（教程漏参数 = 用户按帮助抄不到）"
            );
        }
    }

    /// 名单与帮助全文的**双向**钉子：帮助里出现的每个长参数都必须被名单覆盖。
    ///
    /// 只做单向（名单 ⊆ 帮助）会漏掉"帮助新增了参数、名单没跟上" ⇒ 教程静默过时。
    #[test]
    fn cli_flag_list_covers_every_help_flag() {
        let help = azusa_local_proxy::output::usage_text();
        let mut missing: Vec<String> = Vec::new();
        for token in help.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-')) {
            if !token.starts_with("--") || token.len() < 3 {
                continue;
            }
            if !CLI_LONG_FLAGS.contains(&token) && !missing.iter().any(|seen| seen == token) {
                missing.push(token.to_string());
            }
        }
        assert!(
            missing.is_empty(),
            "帮助全文里有名单未覆盖的长参数（教程会漂移）：{missing:?}"
        );
    }

    /// 第八章末「环境」小段（Open-6 的两行迁移面）：版本与 OS 必须在正文里。
    #[test]
    fn environment_section_carries_version_and_os() {
        let text = tutorial_text_for("9.9.9", "Windows Test 1（深色标题栏：启用）").body;
        assert!(text.contains("环境（本次运行）"));
        assert!(text.contains("版本：v9.9.9"));
        assert!(text.contains("Windows Test 1（深色标题栏：启用）"));
        assert!(text.contains("构建目标：Windows 7 SP1+ x64"));
        // 第七章的参考命令必须带 README 原措辞（P3-6：不能只给命令不说"程序不提供"）
        assert!(text.contains("这只是参考命令，程序不再提供该能力"));
        // 教程里不得出现私有网段地址 / 内网服务标识。
        // ⚠ needle 一律**运行期分段拼接**（源码里不出现完整地址 —— 与本仓 D13-1 口径一致：
        //   否则"反例名单"自身就构成一条内网痕迹命中）。
        let c10 = ["1", "0"].join("");
        let c172 = ["1", "7", "2"].join("");
        let c192 = ["1", "9", "2"].join("");
        let id = ["k", "1", "0", "0", "a", "i"].join("");
        // **P2-③**：三个完整私有段全覆盖（`10.` 段 / `172.16…172.31` 段 / `192`+`168` 段），不只首段的第一格；
        // 断言消息只报序号（不打印 needle 真值）。
        let mut needles: Vec<String> = vec![format!("{c10}."), format!("{c192}.168."), id];
        for second in 16..=31 {
            needles.push(format!("{c172}.{second}."));
        }
        assert_eq!(needles.len(), 3 + 16, "私有段 needle 数必须齐（3 + 16）");
        for (index, forbidden) in needles.iter().enumerate() {
            assert!(
                !text.contains(forbidden.as_str()),
                "教程不得含内网痕迹（needle #{index}）"
            );
        }
    }

    /// `utf16_len` 的边界：BMP 字符按 1 计、非 BMP 按 2 计（`EM_SETSEL` 的口径）。
    #[test]
    fn utf16_len_counts_code_units() {
        assert_eq!(utf16_len(""), 0);
        assert_eq!(utf16_len("中"), 1);
        assert_eq!(utf16_len("a中"), 2);
        assert_eq!(utf16_len("\u{1F600}"), 2, "非 BMP 字符占两个 UTF-16 码元");
        assert_ne!(utf16_len("中"), "中".len(), "UTF-8 字节长度是另一回事");
    }
}
