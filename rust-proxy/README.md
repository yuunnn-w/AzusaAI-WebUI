# AzusaAI 本地反向代理（`rust-proxy/`）

AzusaAI WebUI 的**独立子项目**：一个便携单文件的本地反向代理，把 AzusaAI 页面（`file://`）到内网模型
服务的跨源调用在本机消化掉 —— **本地应答 CORS 预检 + 给所有响应注入跨源头**，向上游只做透明转发。
**不改内网服务端、不改 AzusaAI 主程序**（主程序零代码改动）。Windows 7 SP1 及以上与现代 Windows 通用
（仅依赖系统 `msvcrt.dll`，无运行库 / 安装器要求）。

> **当前状态：Phase 6（批 6）收口**。本批把程序改为**无配置文件 · 零文件读写 · 零持久化**形态：
> 配置只走命令行（运行期可在设置窗「应用」，仅本次运行、重启回默认）；日志只在内存（可复制到剪贴板）；
> **不写注册表、不提供开机自启**；关闭按钮 = 进托盘、最小化 = 进任务栏；并交付**两个变体**（开源版 / 内网版，见下）。
> Win7 **实机**冒烟仍未执行（本机无 Win7；覆盖声明见「覆盖范围声明」一节）。

---

## 两个变体（同一份源码，只差默认上游）

| 变体 | 产物名 | 默认上游 | 字节 | sha256 |
|---|---|---|---|---|
| **开源版**（不注入） | `azusa-local-proxy-win7-x64.exe` | `http://127.0.0.1:8080` | 1,412,096 | `7471f53381c6beea6cbb40730875d0223d734f72f19243b8e8f2d597076bf04a` |
| **内网版**（注入） | `azusa-local-proxy-win7-x64-internal.exe` | 构建时注入的内网地址 | 1,412,096 | `e007d93866fa2eb5e5dcb77e40689cf4a1b7cef8d441d6d48a4eed3110f25236` |

- **同源同码**：两个变体由**同一份源码**构建，唯一差异 = 编译期注入的默认上游字符串；`--help` 打印**生效的默认值**
  （两版帮助文本只在这几处取值上不同）。
- **构建方式**（`scripts/build-win7.ps1`，开关可组合）：
  - 开源版：**不带**注入开关（默认上游 = 回退字面量 `http://127.0.0.1:8080`）；
  - 内网版：`-Internal` ⇒ 读本机 **gitignored** 的 `rust-proxy/.internal-default.txt` 第一行；或 `-InternalDefault <url>` 显式给值。
  - `.internal-default.txt` **不入库**（`.gitignore` 已登记，见下「目录速览」末尾）—— 内网地址**不落进任何被 git 跟踪的文件**。
- **Release 只放开源版**；内网版**本地交付**用户本人（不入库、不入 Release）。
- `dist/` 现为 **两个 win7 exe + `SHA256SUMS.txt`**（win10 现代档可选，未随本批构建）。

---

## 使用（四步）

1. **启动代理**：把对应变体的 exe 放到任意目录，双击（或命令行）运行。
   - **开源版**：默认上游 `http://127.0.0.1:8080`，与默认监听 `127.0.0.1:80` **不同址 ⇒ 不带参数可直接启动**；
     若该地址没有模型服务，会看到**明确的 502**（而不是静默自环），请用 `--upstream` 指到你的模型服务地址。
   - **内网版**：默认上游 = 构建时注入的内网地址 ⇒ 在内网机器上**开箱即用**；也可命令行显式覆盖：
     `azusa-local-proxy-win7-x64-internal.exe --upstream http://<内网模型主机>`
     （服务挂在路径前缀下时，把前缀一并写进 `--upstream`）。
   - **本程序不写任何配置文件**：配置只在运行期存在（命令行 + 设置窗「应用」，**仅本次运行，重启回默认**）。
   - **自环保护**：若你把上游配成与**实际监听**（含端口回退后的实际口）**同址**，程序**拒绝启动**并提示怎么改
     （见下「运行 · 自环保护」）。
   - 无界面运行（脚本 / 排障）用 `--no-gui`（见下「运行」）。
2. **应用 → 设置 → 模型**：`Base URL` 改为 `http://127.0.0.1`（端口不是 80 时写 `http://127.0.0.1:<端口>`）。
3. **应用 → 设置 → 模型 → 高级(兼容性)**：**关闭**「预检规避兼容模式」（对反代是净损害）。
4. **发一条真实对话**：能逐字流式输出即成功。

### ⚠ 「CORS 分层」面板的预期读数（排错必读）

反代就位后，设置 → 环境 →「运行自检」的「CORS 分层」会显示 **`· 简单 POST HTTP 400(未预检)`**，
结论行是 **「服务端不解析 text/plain —— 请关闭该模式…」**。**这是预期**：面板里那条"简单 POST"是
**恒定发 `text/plain` 的探针**（用途 = 判断「预检规避兼容模式」在该服务端是否有效），反代**有意不修改**
`Content-Type`。判"修好了"请看两件事：① `P1/P2/P4` 三行可读；② **发一条真实对话能正常流式输出**。
只有当 **"真实 POST" 那行也不可读** 时，才是反代没生效。

---

## 安全与保密

> 本节回答"凭什么说它不会触发杀软 / 适合在保密内网机器上跑" —— 每条都给可核依据。

**本程序不做的事（逐条）**

- **不读取、不写入任何文件**：运行期**零文件 I/O** —— 无配置文件（所以不读）、日志只在内存（所以不写盘）、
  不写 exe 同目录、不写 `%TEMP%`、不建目录、不自更新、不下载。**唯一豁免四项**（如实列出）：
  ① 操作系统加载 exe 自身与其静态链接的垫片；② `LoadImageW(hInstance, MAKEINTRESOURCE(1))` 读**自身嵌入资源**里的
  图标（构建期由 `build.rs` 用 `winresource` 嵌入，运行期无文件打开）；③ **用户显式**的剪贴板写入（点「复制到剪贴板」）；
  ④ **控制台设备句柄** —— `CreateFileW("CONOUT$" | "CONERR$", …)` 打开的是**控制台设备**而非文件
  （`GetFileType` 返回 `FILE_TYPE_CHAR`），不产生任何文件读写；落点 = `src/output.rs` 的 `open_console_device()`
  （**函数体 `:424–443`，两处调用于 `:435` / `:439`**，是本程序**唯一**的 `CreateFileW`）。`--help` 与日志都不落盘。
  - **一个必须分清的区分**：**"脚本把 stdout 重定向到文件" ≠ "本程序写文件"** —— 此时文件由**脚本**创建，
    本程序只写继承来的句柄（soak / 验证链就是这么取证据的）。
- **不写注册表**：程序内**没有任何注册表读写代码**（**开机自启功能已整体移除**，两个 `--*-autostart` 参数不存在，
  传入即报"未知参数"）。
- **不注入进程 / 不挂钩 / 不截屏 / 不键盘记录**：静态 grep 无 `SetWindowsHookEx` / `GetAsyncKeyState` /
  `GetKeyState` / `OpenProcess` / `Toolhelp`。
- **不 spawn 子进程**（含 `cmd` / `powershell`）：无 `Command::new` / `CreateProcess`；运行期子进程数恒为 0。
- **除用户指定的上游外零外联**：无遥测、无更新检查、不做外部 DNS；只向 `--upstream` 指定的地址做透明转发。
- **只监听回环**：默认且只能绑 `127.0.0.1`；`--listen-host` 传非回环地址一律拒绝。
- **不打包 / 不混淆 / 不加密 / 不加壳**：无 UPX 一类加壳特征（release 档的 `strip = true` 只剥离符号表，**不是**打包或加密）。
- **manifest 未声明 `requestedExecutionLevel`**（⇒ 系统默认 `asInvoker`）：**不触 UAC**（不请求管理员权限）。
- **不含任何内网地址**：源码 / 文档 / 脚本 / 示例一律用 `127.0.0.1`（或 `::1` / `0.0.0.0`）与占位符
  **`<内网模型主机>`**；示例路径前缀一律用中性词（`/gateway` / `/proxy`）。部署到内网时由命令行显式给出上游：
  `azusa-local-proxy.exe --upstream http://<内网模型主机>`。本仓库不含、也不会持久化任何私有网段地址。
- **不含内网服务标识**：源码 / 文档 / 脚本 / 示例不出现内网部署独有的服务标识与路径前缀。
- **如实列出静态链接与动态解析**（透明 = 降低误报、便于人工放行）：
  - 静态垫片 = **VC-LTL5（EPL-2.0）+ YY-Thunks（MIT）**，两者都静态进 exe，仅依赖系统 `msvcrt.dll`；
  - 动态解析**仅两处**，均为老系统兼容（现取行号）：

    | 动态解析点 | 落点 | 用途 |
    |---|---|---|
    | `ntdll!RtlGetVersion` | `src/osver.rs:72,77` | Win7 上取真实系统版本（比静态导入面更小） |
    | `dwmapi!DwmSetWindowAttribute` | `src/ui/mod.rs:615,616,625` | Win7 无此导出 ⇒ 动态取，失败自动跳过（深色标题栏） |

    判据 = `grep -rn "LoadLibrary|GetProcAddress" rust-proxy/src` = **7 行 / 2 文件**
    （`osver.rs:6,72,77` · `ui/mod.rs:603,615,616,625`；其中 `:6` 与 `:603` 为文档注释）；
    **新增任何第 3 处动态解析点即判缺陷**（构建期门禁与归档判据会咬）。

**供内网管理员人工放行参考**

- 默认行为清单即本节；两条可核验脚本：`scripts/verify-no-residue.ps1`（零残留自检：exe 同目录 / `%TEMP%` /
  注册表 / 子进程 / 外联）与 `scripts/verify-no-write.ps1`（**只读目录**全功能冒烟 + 对照臂，证明运行期无写入需求）。
- **自启功能已整体移除** ⇒ 无需"清除自启项"步骤。若曾在**旧版本**（P6 之前）装过自启
  （旧版写 `HKCU\...\Run`，值名 `AzusaAI-LocalProxy`），遗留项请**手动**清理 —— 这只是参考命令，程序不再提供该能力：
  ```bat
  reg delete "HKCU\Software\Microsoft\Windows\CurrentVersion\Run" /v AzusaAI-LocalProxy /f
  ```

---

## 构建前置

| 前置 | 说明 |
|---|---|
| **日期化 nightly 工具链** | `rust-toolchain.toml` 钉 **`nightly-2026-06-03`** + `rust-src`（tier-3 目标无上游 CI）。安装（经代理）：`rustup toolchain install nightly-2026-06-03 --component rust-src` |
| MSVC 工具链（`link.exe`） | 即 Rust 的 `*-windows-msvc` 默认工具链 |
| **7z 解包能力** | `VC-LTL-Binary.7z` 是真 7z（非 zip）：`python -m pip install py7zr --proxy http://127.0.0.1:7890`（或安装 7-Zip） |
| **资产取件（DG14 = 按需取件）** | `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/fetch-assets.ps1` —— 经代理取 VC-LTL5 v5.3.1 与 YY-Thunks v1.2.2，**SHA-256 硬核验**（不匹配即 `exit 1`）后解包归位到 `assets/vc-ltl/`、`assets/yy-thunks/`、`tools/depends/`。⚠ 这些目录**不入库**（`.gitignore`）；**离线构建需要 `.cache/` 已有归档**（脚本会复用，不再联网）。代理地址可用 `-Proxy` 参数改。 |
| `objdump`（验证用） | w64devkit 的 `objdump.exe`（指南 §4.2） |

> **没有 CI**：Win7 构建为**本地手工步骤**（一键构建脚本 = `scripts/build-win7.ps1` / `scripts/build-modern.ps1`，
> 见下节）。每次刷新资产后请对照指南 §8.1 重新核验版本与 SHA-256（`fetch-assets.ps1` 已内置）。

## 构建

```bash
cd rust-proxy

# Win7 x64（发布主产物）—— ⚠ 每一次 cargo 调用都必须同时带
#   -Z build-std=std,panic_abort 与 --target（否则 cargo test 会静默测试另一套 std）
cargo +nightly-2026-06-03 build --release -Z build-std=std,panic_abort --target x86_64-win7-windows-msvc
# 产物：target/x86_64-win7-windows-msvc/release/azusa-local-proxy.exe

# 现代 Windows（快，日常循环；同样源码，另一套 profile）
cargo build --release --target x86_64-pc-windows-msvc
```

> 在 `rust-proxy/` 目录内，普通 `cargo` 已自动使用 `rust-toolchain.toml` 钉住的工具链；
> 上面显式写 `+nightly-2026-06-03` 是为了在任何调用点都无歧义。
> Win7 两层接线（VC-LTL5 + YY-Thunks）由 `build.rs` 在 `TARGET` 含 `win7` 时注入；
> **资产缺失时构建直接失败**（不会静默产出缺件的 exe）。

### 一键构建（发布链固化）

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-win7.ps1     # Win7 开源版：构建 → PE 自检 → 四道验证 → dist/
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-win7.ps1 -Internal          # Win7 内网版（读 gitignored 的 .internal-default.txt）
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-win7.ps1 -InternalDefault <url>  # 或显式给注入值
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-modern.ps1   # 现代档（+crt-static）→ dist/
```

- `build-win7.ps1`：前置检查（工具链/资产）→ `cargo +nightly-2026-06-03 build --release -Z build-std=std,panic_abort --target x86_64-win7-windows-msvc`
  → PE 版本信息/图标自检 → `verify-win7.ps1` 四道验证 → 归位 **`dist/azusa-local-proxy-win7-x64[-internal].exe`** + `dist/SHA256SUMS.txt`（任一环节失败即 `exit 1`，**不写 dist**）。
  产物路径以 **cargo 消息**（`compiler-artifact`）为准，不按约定猜路径；`-SkipBuild` 可只复验 + 归位（**语义 = 复用现有产物、不校验新旧**）。
  `SHA256SUMS.txt`：**多产物各一行**（重跑只替换本产物行、其它行保留；排序；UTF-8 无 BOM、LF）。
  **注入变体与证据分流**：`-Internal` / `-InternalDefault` = 内网版 ⇒ 验证归档落 `target/win7-verification-internal/`（**gitignored、不入库**）；
  开源版归档落 `win7-verification/`（**入库**）。构建日志会打印"本次默认上游 = …（变体：开源 / 内网）"。
  **变体自证**：产物就位后脚本在 **exe 字节**里查 needle（`Assert-Variant`）——含注入值即不当开源版发布，`-SkipBuild` 同样经过；⚠ **改过注入源后请 `touch` 它**（`rust-proxy/.internal-default.txt`）：裸 `cargo`（不经脚本）对"注入源以旧 mtime 复原"不敏感，可能不重编而沿用上一次的注入值（脚本路径无此问题：构建前自动 touch）。
- `build-modern.ps1`：现代档（`+crt-static` 静态 CRT，无 UCRT/VCRUNTIME DLL 依赖）⇒ `dist/azusa-local-proxy-win10-x64.exe`（可选档；
  Win7 发布主产物仍是 `-win7-x64`）。导入面作软校验（出现 `VCRUNTIME140`/`api-ms-win-crt-*` 会 WARN）。同样支持 `-Internal` / `-InternalDefault <url>`。
- `dist/` **不入库**（`.gitignore`）；exe 只做发布附件（Release 只放开源版）。

## 验证（四道，一键）

```bash
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/verify-win7.ps1
```

1. **YY.Depends.Analyzer** `/Target:6.1.7600 /IgnoreReady` ⇒ 报告**存在 + mtime 晚于本次运行 + 为空**（三连；
   报告落点 = `tools/depends/<exe名>.md`，脚本会把它复制进 `win7-verification/` 归档）；
2. **objdump 三查**：无 `VCRUNTIME140` / `api-ms-win-crt-*`；子系统版本 ≤ 6.1；
3. **关键 API 自证**：`Config/x64/6.1.7600.txt` 三条读数（期望 **3 / 0 / 0**）；
4. **归档**：以上证据 + 命令原文 + md5 → `win7-verification/`（**入库**；内网变体落 `target/win7-verification-internal/`）。

另有两套测试（现代目标快；Win7 目标是发布门槛）：

```bash
cargo test                                                                  # 现代目标
cargo +nightly-2026-06-03 test -Z build-std=std,panic_abort --target x86_64-win7-windows-msvc
cargo fmt --check && cargo clippy --all-targets -- -D warnings
```

### 覆盖范围声明（诚实边界）

- **已验证（本机 Win11 10.0.26220）**：Win7 目标构建可复现（`/Brepro` ⇒ 同输入逐字节相同，两个变体各 ×2 连跑一致）；
  **静态四道全绿**（Analyzer「存在 + mtime + 空」三连 / objdump 三查 / 关键 API 自证 3/0/0 / 归档）；
  两个变体的 `--help` 默认上游正确（开源 = `http://127.0.0.1:8080`；内网 = 注入值）、**不带参数均能正常起服务**、
  预检 204 + CORS 全套；**零文件 I/O 判据**（非测试面 `std::fs` / `File::open|create` / `OpenOptions` = 0；
  `CreateFileW` 仅 `open_console_device()` 内两处，见「安全与保密」）；**零持久化**（源码内注册表 API = 0）。
- **未覆盖（必读）**：
  - **Windows 7 SP1 实机 / VM 冒烟未执行** —— 本机无 Win7 环境。静态证据**不能替代**实机端到端；
    在完成实机冒烟前，**不应把本工具标为"已在 Win7 验证"**。
  - **`--no-gui` 的 Ctrl-C 可达性 = `?`（未定）**：实跑得"20 s 未停机"，但装置**无阳性对照**（同型 CUI 子进程同样不响应）
    ⇒ **不能**推出"不可达"，本文**不承诺** Ctrl-C 可用。要停无界面实例请按 PID：
    `Stop-Process -Id <PID>`（进程 id 见任务管理器 / `netstat -ano` 的监听 PID）。
  - **日志不留存**：日志只在内存、退出即消失 ⇒ 需要证据链时**现场**点「查看日志」→「复制到剪贴板」，事后无法找回。
  - **`--dump-icons` 已删除** ⇒ 四态图标的 exe 级取证入口消失；图标回归改由 `cargo test` 与进程内探针承担。
  - **只读目录臂不覆盖 Win7 实机只读卷**（只读冒烟在本机 Win11 上做，配对照组）。
  - **soak 只跑 3 min 短跑**（证明"日志改走 stdout 重定向后不产出空值"）；2 h 量级稳定性沿用
    `batch-rust-proxy-halfopen` 的旧读数，不在本批覆盖。
  - **内网版的真实可达性未验**：注入值是本机不可达的地址 ⇒ 只验证"起服务成功 + 默认值正确"，未验证真实转发。
  - **x86（32 位）未提供**：当前只发 x64。若确需 `-win7-x86.exe`：先安装 **NASM**，再
    `cargo +nightly-2026-06-03 build --release -Z build-std=std,panic_abort --target i686-win7-windows-msvc`
    （x86 资产已在位），**并必须重跑「验证（四道）」后才可发布**。
- **Win7 实机验证步骤（待用户执行；有 SP1 虚拟机即可）**：
  1. 拷 `azusa-local-proxy-win7-x64.exe` 到 Win7 SP1 机器，**双击**运行：应出现主窗 + 托盘图标（无 "DLL missing" / "procedure entry point…" / "not a valid Win32 application" 对话框）；
  2. 事件查看器 → 应用程序日志：**不得**出现 `0xC0000135`（第 1 层）或 `0xC0000139`（第 2 层）；
  3. 按上文「使用（四步）」接上应用，发一条**真实对话**（流式逐字输出 = 主判据）；
  4. 断网/关上游再发一条 ⇒ 应看到**带原因的 502**（含上游地址），不是 `Failed to fetch`；恢复后无需重启即可用；
  5. 结果（含截图）记录进 `win7-verification/`。

## 运行

```bash
azusa-local-proxy.exe                        # 默认：主窗 + 托盘
azusa-local-proxy.exe --upstream http://<内网模型主机>   # 指定上游（内网部署）
azusa-local-proxy.exe --no-gui --listen-port 8010        # 无界面（脚本/排障）
```

**配置（只在运行期，不写文件）**

- **没有配置文件**：参数全走命令行；设置窗点「**应用**」只作用于**本次运行**，关掉程序即回到默认。
  两种写法都行：`--参数 值` / `--参数=值`。`--help` 有全文。

  ```
  监听与转发   --listen-host / --listen-port(别名 --port) / --port-fallback / --upstream / --strip-prefix
  超时(ms)     --connect-timeout-ms / --response-head-timeout-ms / --first-byte-timeout-ms
  连接池与 CORS --pool-max-idle-per-host / --pool-idle-timeout-ms / --cors-mode / --cors-max-age
  日志         --log-level <error|warn|info|debug>（只在内存）
  启动与退出   --start-paused / --close-action <tray|exit>
  其它         --no-gui / --version(-V) / -h|--help
  ```

- **默认值**：监听 `127.0.0.1:80`（被占则按 `--port-fallback` 回退，界面显示实际地址）；上游 = 见上「两个变体」；
  客户端 Base URL 填 `http://127.0.0.1/v1`（端口不是 80 时写 `http://127.0.0.1:<端口>`）。
- **日志只在内存**（最近 2,000 条，单行封顶）：**程序不写任何文件**；界面「查看日志」看最近 100 行 +
  总条数 / 已挤出条数，要留存就点「**复制到剪贴板**」（退出即消失）。
- **不写注册表、没有开机自启**。
- **窗口 / 托盘语义**：**关闭按钮 = 进托盘**（`--close-action tray`，默认）；**最小化 = 进任务栏**；
  两个状态都保留托盘图标，可从托盘唤回窗口。`--close-action exit` 可让关闭按钮直接退出。
- **`--start-paused`**：启动后不自动开始服务（到界面点「启动」）；与 `--no-gui` 互斥（同给 ⇒ 退出码 2）。
- `--help` 查看全部参数。绑定失败会**区分占用（10048）与权限（10013）**并给出提示。
- **退出码**：0 正常 / 1 运行期错误 / 2 参数错误 / 3 已有实例在运行（已尝试唤起其窗口）。

**自环保护**

- 判定**只看**"上游的 `host:port`"与"**实际监听**的 `host:port`"（含端口回退后的实际口）是否相同 ——
  **不是**"没传 `--upstream` 就拒"。默认值（开源版 `:8080` 上游 / `:80` 监听）不同址 ⇒ 默认可直接启动。
- 若**同址**（例：`--listen-port 8080 --upstream http://127.0.0.1:8080`）⇒ **拒绝启动**（退出码 2）并提示怎么改：
  用 `--upstream` 指到真实上游，或改 `--listen-port`。

## 排错

### 1) 「注入似乎没生效」三检查（指南 §5.2）

① `assets/vc-ltl/{x64,x86}/` 里应有那 4 个 `.lib`（`libucrt / libvcruntime / ucrt / vcruntime`），
且 `build.rs` 为**匹配的架构**发出搜索路径；
② 强制构建脚本重跑（`touch build.rs` / 删除 `target/` 中的 build 缓存），别复用陈旧的链接行；
③ `assets/yy-thunks/` 里的 `.obj` 正确，且链接参数包含它**以及** `/NODEFAULTLIB:kernel32.lib` + `kernel32.lib` 对。

### 2) 现象对照

| 现象 | 层 | 处置 |
|---|---|---|
| `LNK1181: cannot open input file 'YY_Thunks_for_Win7.obj'` | 链接 | 资产缺失/改名错/路径不对 ⇒ 重跑 `fetch-assets.ps1` |
| `LNK2001: unresolved external symbol` | 链接 | 6.0.6000.0 档真实缺口或 C++ 运行期部件 ⇒ 换等价 crate（记入风险） |
| 运行期缺 `api-ms-win-crt-*` | 第 1 层 | VC-LTL 搜索路径未生效 ⇒ 见三检查 ①② |
| 运行期 "procedure entry point …" | 第 2 层 | thunk 未接管 ⇒ 见三检查 ③ |
| 应用仍报 `Failed to fetch`，且「真实 POST」行不可读 | —— | 代理没起/端口不符/防火墙 ⇒ 查界面「查看日志」里的"本地预检"计数与监听地址 |
| 启动即被拒 + 提示"上游与监听同址" | —— | 自环保护命中 ⇒ 按提示改 `--upstream` 或 `--listen-port`（见「运行 · 自环保护」） |

### 3) 上游改动相关

- 上游不可达 ⇒ 应用会看到**带原因的 502**（JSON `proxy_upstream_unreachable`），不是 `Failed to fetch`；
- 上游要求 `Origin` 必填的网关不在本工具能力范围内（反代按设计剥离 `Origin`/`Referer`）；
- 上游 3xx 会被**如实透传**（不替客户端跟随）；浏览器跨源跟随 `Location` 失败属预期。

### 4) 上游"卡住不回"（半开/静默）—— `--response-head-timeout-ms`

**现象**：内网模型服务重启、被中间设备回收长连接、或对端进程"半死"时，
应用可能长时间**没有任何回显**（旧版本会**无限等下去**）。

**本版起的行为**：`--response-head-timeout-ms`（**默认 120000 = 120 s**）：
"请求体送完 → 上游响应头到达"的上限；`0` = 不限（= 旧行为，不推荐）。

- **命中时你会看到**：应用侧 **504 + 带原因的 JSON**（`proxy_upstream_timeout`，文案含上游 URL、上限毫秒数与已送达字节数），
  而不是浏览器级的 `Failed to fetch`；**同一进程内无需重启**，下一次请求（上游恢复后）通常 ≤1 s 成功。
- **两个相位别混**（常见误读）：`--response-head-timeout-ms` 管**"等响应头"**；`--first-byte-timeout-ms` 管
  **"响应头之后、首帧之前"**——后者默认仍是 0，**长思考/长流不受影响**（本键只在响应头还没到时计时，响应头一到计时器立刻失效）。
- **怎么调**：内网模型冷启动/排队很慢（TTFB 常见几十秒）⇒ 保持默认 120 s 或按实测放大（上限 1 h）；
  soak/压测等受控场景常设 `3000`（3 s）。调完在设置窗点「应用」即热生效（**仅本次运行**）。
- **日志怎么读**：命中时内存日志有 `上游 … 等待响应头 watchdog 命中（…ms，已送达请求体 …B）`（同名 ≤1 条/60 s），
  访问行里出现 `-> 504` 且 `池重置=` 计数 +1（连接池已重置 ⇒ 陈旧连接不再被后续请求复用）。

## 许可

本子项目是 AzusaAI WebUI 的一部分，**许可证见仓库根 `LICENSE`（GPL-3.0）**。
编译产物包含/链接了第三方兼容库（VC-LTL5 = **EPL-2.0**，YY-Thunks = **MIT**）—— 声明与所用文件清单见
[`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md)。Win7 资产本身**不入库**（按需取件），但发布 exe
含由它们替换/桩接的代码 ⇒ **分发时保留许可证文本与署名**。

## 目录速览

```
rust-proxy/
├─ Cargo.toml / Cargo.lock        # 独立工程（可执行程序 ⇒ Cargo.lock 入库）
├─ rust-toolchain.toml            # 钉 nightly-2026-06-03 + rust-src
├─ build.rs                       # .ico/清单/版本信息嵌入；win7 目标注入 VC-LTL5 + YY-Thunks；默认上游注入
├─ .cargo/config.toml             # 目标声明（接线全在 build.rs）
├─ .internal-default.txt          # 内网版默认上游的本地注入源（**gitignored、不入库**）
├─ README.md / THIRD_PARTY_NOTICES.md  # 使用/构建/排错/安全与保密/覆盖范围；第三方声明（入库）
├─ licenses/                      # VC-LTL5 EPL-2.0 / YY-Thunks MIT 许可全文 + crate 许可清单（入库）
├─ src/
│  ├─ main.rs                     # 入口（装配输出通道 + CLI + 服务）
│  ├─ cli.rs                      # 命令行解析 + 覆盖内建默认值（唯一入口）
│  ├─ config.rs                   # 内建默认值 + 校验 + 自环守卫（内存储存，无配置文件）
│  ├─ output.rs                   # 三种输出通道（控制台/重定向/弹窗）+ 控制台设备句柄（唯一 CreateFileW）
│  ├─ logging.rs                  # 内存环形缓冲（2000 条）+ 凭据脱敏（不落盘）
│  ├─ server.rs / service.rs      # 监听/服务装配与生命周期
│  ├─ stats.rs                    # 计数（零 panic 作用域）
│  ├─ osver.rs                    # 系统版本探测（动态取 ntdll!RtlGetVersion）
│  ├─ ui/                         # 主窗 / 托盘 / 设置窗 / 主题 / 剪贴板
│  └─ proxy/{mod,forward,cors,notice}.rs  # 内核：catch-all 转发 / CORS 注入 / 预检本地应答
├─ scripts/
│  ├─ fetch-assets.ps1            # 取件 + SHA-256 核验 + 解包 + 归位（DG14）
│  ├─ verify-win7.ps1             # 四道验证一键跑 + 归档
│  ├─ verify-no-residue.ps1       # 零残留自检（exe 同目录 / %TEMP% / 注册表 / 子进程 / 外联）
│  ├─ verify-no-write.ps1         # 只读目录全功能冒烟 + 对照臂（证明无写入需求）
│  ├─ build-win7.ps1              # 一键：构建 + PE 自检 + 四道验证 + 归位 dist/（支持 -Internal / -InternalDefault）
│  └─ build-modern.ps1            # 一键：现代档（+crt-static）构建 + 归位 dist/
├─ assets/                        # app.ico / app.manifest（入库）；vc-ltl、yy-thunks（不入库）
├─ tools/depends/                 # 分析器三件（不入库，由 fetch 脚本解出）
├─ win7-verification/             # 验证报告与读数（入库；内网变体落 target/win7-verification-internal/）
├─ dist/                          # 发布包（两个 win7 exe ± win10 exe + SHA256SUMS；不入库）
└─ .cache/                        # 原始归档（不入库；离线构建依赖它已有件）
```
