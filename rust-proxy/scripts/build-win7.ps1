#requires -Version 5.1
<#
build-win7.ps1 —— Win7 发布链一键构建（方案 §9 Phase 4W / §5.5.4 / §5.6）

做什么（顺序固定，任一环节失败 ⇒ exit 1，不进下一步）：
  0. 前置检查：`nightly-2026-06-03` 工具链（`rust-toolchain.toml` 已钉）+ Win7 资产
     （assets/vc-ltl/x64 恰 4 个 release .lib、assets/yy-thunks 的 x64 obj —— DG14 按需取件，
     缺件提示先跑 scripts/fetch-assets.ps1；**绝不静默降级**）
  1. 构建：cargo +nightly-2026-06-03 build --release -Z build-std=std,panic_abort
     --target x86_64-win7-windows-msvc（铁律：每次 cargo 调用都同时带这两个标志，RK14）
  2. PE 资源自检：版本信息（FileVersion 与 Cargo.toml 的 version 对齐；产品名/版权等非空）+ 图标可提取
  2b. 产物变体自证：Assert-Variant 直接在 **exe 字节**里查"本次期望的默认上游" needle
     （不符 ⇒ 立即 exit 1：不写 dist/、不刷 win7-verification/**；`-SkipBuild` 同样经过）
  3. 四道验证：调用 scripts/verify-win7.ps1（Analyzer 三连 / objdump 三查 / API 自证 / 归档）
     → 开源版证据落 win7-verification/（入库）；**内网版落 target/win7-verification-internal/**（gitignored，D13）
  4. 归位 dist/：azusa-local-proxy-win7-x64[-internal].exe + SHA256SUMS.txt（重跑覆盖，不累积）

幂等：重建走 target/ 增量；验证同日覆盖同名归档；dist 只保留当前产物 + 汇总行重写。
用法：
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-win7.ps1
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-win7.ps1 -SkipBuild   # 只复验 + 归位
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-win7.ps1 -Internal \
    # ↑ 内网版（P6-K11）：默认上游由本地 gitignored 的 .internal-default.txt 注入
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-win7.ps1 -InternalDefault <url>
    # ↑ 内网版（显式值）：直接把 <url> 注入，不读文件
  # 不给上述两个开关 = **开源版**（默认）：显式关闭注入 ⇒ 即使本地树里存在
  #   .internal-default.txt 也绝不注入（源码 / 仓库零内网痕迹，D13）
退出码：0 = 全链通过；1 = 任一环节失败。
#>
[CmdletBinding()]
param(
    [switch]$SkipBuild,
    [string]$DistDir = "",
    [switch]$Internal,
    [string]$InternalDefault = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
try { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 } catch { }

$Root = Split-Path -Parent $PSScriptRoot
if ([string]::IsNullOrWhiteSpace($DistDir)) { $DistDir = Join-Path $Root "dist" }
$VerificationDir = Join-Path $Root "win7-verification"
$Target = "x86_64-win7-windows-msvc"
# 约定路径：正常构建时会被 §1 的 cargo 消息路径覆盖；`-SkipBuild`（只复验 + 归位）时直接用本条 ——
# **不校验产物新旧**（语义 = "复用现有产物"，供复验/归位场景）。
$ExePath = Join-Path $Root "target\$Target\release\azusa-local-proxy.exe"
$Toolchain = "nightly-2026-06-03"

# ── K11-a / K11-c：双产物（内网版 / 开源版）—— 同一份源码，唯一差异 = 默认上游字符串 ─────────
# 注入权**只在 build.rs**（`AZUSA_DEFAULT_UPSTREAM` 环境变量 → 否则读 gitignored 文件 → 否则不注入），
# 本脚本只负责"选哪一臂"，并把结果如实打印 + 在**产物字节**上自证（见 §1b 的 `Assert-Variant`）：
# ⚠ 构建日志**不是**变体见证 —— cargo 会把 build.rs 的 `cargo:warning` **重放**（实测：同状态连跑两次
#   `Compiling=0` 仍有那一行），且裸 cargo 对"注入源以**旧 mtime** 复原"不敏感（缺文件 → 原样移回 ⇒
#   不重编，产物仍是上一个变体）。故变体一律由 exe 上的 needle 断言钉住（P6-B4b 修正轮 P1-1）。
#   · 默认（无开关）      = 开源版：设 `AZUSA_DEFAULT_UPSTREAM_OFF=1` ⇒ build.rs 明确"不注入"
#   · -Internal           = 内网版：读 .internal-default.txt 第一行，作为环境变量交给 build.rs
#   · -InternalDefault u  = 内网版（显式值）：直接把它交给 build.rs
# ⚠ "不注入"必须用**独立变量名**（`..._OFF`）：cargo 的进程环境对 rustc 也可见 ⇒ 若把 `off`
#   塞进 `AZUSA_DEFAULT_UPSTREAM`，`option_env!` 会读到它、产出"默认上游 = off"的坏二进制
#   （P6-B4b 实测踩到过；改用独立变量名后开源版 `--help` = 4,336 B ✓）。
# ⚠ 保密红线（D13）：非开源变体的默认上游**绝不写进任何被 git 跟踪的文件** ⇒
#   内网版的四道门归档与汇总一律改落 `target/win7-verification-internal/`（gitignored），
#   `win7-verification/`（**入库**）只承载开源版读数。
$OpenDefaultUpstream = "http://127.0.0.1:8080"     # 开源回退值：与 build.rs::OPEN_DEFAULT_UPSTREAM / src/config.rs 同值
$envDefaultUpstream = $OpenDefaultUpstream         # 本次**期望的默认上游**（开源臂 = 回退值本身；内网臂 = 注入值）
$InjectionSourcePath = Join-Path $Root ".internal-default.txt"        # gitignored 注入源（-Internal 读它；构建前 touch 它）
$VariantStampPath = Join-Path $Root "target\.variant-stamp-$Target.txt"  # 上次"自证通过"的变体（target/ 已 gitignore）
$VariantLabel = "开源"
$VariantSource = "缺省回退（脚本显式关闭注入）"
$VariantSuffix = ""
$IsInternal = $false
if ($Internal -and -not [string]::IsNullOrWhiteSpace($InternalDefault)) {
    Write-Host "FAIL：-Internal 与 -InternalDefault 互斥（前者读本地 gitignored 文件，后者显式给值）" -ForegroundColor Red
    exit 1
}
if ($Internal) {
    $internalFile = $InjectionSourcePath
    if (-not (Test-Path -LiteralPath $internalFile)) {
        Write-Host "FAIL：-Internal 需要 $internalFile（本地 gitignored 注入源），但它不存在" -ForegroundColor Red
        Write-Host "      内网版请在仓库根的 rust-proxy/ 下放一行内网上游 URL；或用 -InternalDefault <url> 显式给值"
        exit 1
    }
    $envDefaultUpstream = ([System.IO.File]::ReadAllText($internalFile) -split "`r?`n")[0].Trim()
    if ([string]::IsNullOrWhiteSpace($envDefaultUpstream)) {
        Write-Host "FAIL：$internalFile 的第一行为空 ⇒ 拒绝产出'内网版'（会静默变成开源版）" -ForegroundColor Red
        exit 1
    }
    $VariantSource = ".internal-default.txt（本地 gitignored 注入源）"
    $IsInternal = $true
} elseif (-not [string]::IsNullOrWhiteSpace($InternalDefault)) {
    $envDefaultUpstream = $InternalDefault.Trim()
    $VariantSource = "-InternalDefault（命令行显式值）"
    $IsInternal = $true
}
if ($IsInternal) {
    $VariantLabel = "内网"
    $VariantSuffix = "-internal"
    # 注入值校验（P6-B4b 修正轮 P2-3）：只查 `http://` 前缀会放过 `http://` / `http://:8080` 这类
    # **host 为空**的值 ⇒ 产出"自己的默认值不可用"的二进制。三条同口径（含 build.rs 侧）：
    # ① 必须是 http://（本程序不支持 https 上游）② 不得含空白/控制字符 ③ host 段必须非空。
    # ⚠ 消息里**不回显注入值**（D13：内网值不落日志/档），只回显"值形态 + 长度"。
    $maskedValue = if ($envDefaultUpstream.Length -le 7) { "<masked>" } else { $envDefaultUpstream.Substring(0, 7) + "<masked>" }
    if (-not $envDefaultUpstream.StartsWith("http://")) {
        Write-Host "FAIL：注入值必须是 http:// 地址（本程序不支持 https 上游）：$maskedValue（长度 $($envDefaultUpstream.Length)）" -ForegroundColor Red
        exit 1
    }
    if ($envDefaultUpstream -match '\s') {
        Write-Host "FAIL：注入值不得含空白字符（含空格）：$maskedValue（长度 $($envDefaultUpstream.Length)）" -ForegroundColor Red
        exit 1
    }
    $injectHost = (($envDefaultUpstream.Substring(7) -split '[/?#]')[0] -split ':')[0]
    if ([string]::IsNullOrWhiteSpace($injectHost)) {
        Write-Host "FAIL：注入值缺 host 段（形如 http:// 或 http://:8080 的值不可用）：$maskedValue（长度 $($envDefaultUpstream.Length)）" -ForegroundColor Red
        exit 1
    }
    # 注入值经**环境变量**进入 build.rs（唯一注入通道；脚本读文件仅为打印这一行）。
    $env:AZUSA_DEFAULT_UPSTREAM = $envDefaultUpstream
    Remove-Item Env:AZUSA_DEFAULT_UPSTREAM_OFF -ErrorAction SilentlyContinue
} else {
    # 开源版：**不设** AZUSA_DEFAULT_UPSTREAM（设了就会被 rustc 的 option_env! 读到），只设关闭信号。
    Remove-Item Env:AZUSA_DEFAULT_UPSTREAM -ErrorAction SilentlyContinue
    $env:AZUSA_DEFAULT_UPSTREAM_OFF = "1"
}

$DistName = "azusa-local-proxy-win7-x64$VariantSuffix.exe"
$VerifyDirFinal = if ($IsInternal) {
    # 内网版读数**不入库**（D13）：落 target/ 下的 gitignored 目录
    Join-Path $Root "target\win7-verification-internal"
} else {
    $VerificationDir
}

$script:Failures = 0
function Fail([string]$m) { Write-Host "FAIL：$m" -ForegroundColor Red; $script:Failures++ }
function Pass([string]$m) { Write-Host "PASS：$m" -ForegroundColor Green }
function Note([string]$m) { Write-Host $m }
function Sha256([string]$path) { (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash.ToLowerInvariant() }
function Md5([string]$path) { (Get-FileHash -Algorithm MD5 -LiteralPath $path).Hash.ToLowerInvariant() }

# ── 产物变体自证（P6-B4b 修正轮 **P1-1**）──────────────────────────────────────
# 为什么必须钉在**产物字节**上（三条都已实测复现，任何一条都能让"变体标签"与"产物"脱钩）：
#   · cargo 会把 build.rs 的 `cargo:warning` **重放** ⇒ 同状态连跑两次（`Compiling=0`）照样有那一行；
#   · 裸 cargo 对"注入源缺失 → 以**旧 mtime** 原样复原"**不敏感**（cargo 把"缺失"记成时间基准）⇒
#     原样移回文件后不重编，产物仍是上一个变体；
#   · `-SkipBuild` 下开关**只影响命名与汇总文案**、根本不碰产物（实测：开源 exe 被贴成 `-internal` 名、exit 0）。
# 故在"产物就位之后、写 dist/ 与刷新归档之前"直接读 exe 字节查 needle，不符即**硬失败**：
# 不写 dist/、不刷 win7-verification/**（含内网版的 target/ 归档）、非 0 退出。`-SkipBuild` 同样经过（§1b）。
function Masked([string]$v) {
    if ([string]::IsNullOrEmpty($v)) { return "<empty>" }
    return "<masked:len=$($v.Length)>"
}
function Get-ExeText([string]$exe) {
    # 直读字节后按 ISO-8859-1 解码（byte ↔ char 一比一）⇒ ASCII needle 的 UTF-8 连续字节被逐字命中，
    # 且不经过任何编码嗅探/转换（PE 里那个值就是一段连续 UTF-8 字节）。
    return [System.Text.Encoding]::GetEncoding(28591).GetString([System.IO.File]::ReadAllBytes($exe))
}
function Get-ForeignIpHosts([string]$text) {
    # 点分十进制 host 且**非** 127.x ⇒ 视为外来上游（内网注入值实测就是这种形态）。
    # ⚠ 返回**空数组**时 PowerShell 会把它摊平成 `$null` ⇒ 调用点必须写 `@(Get-ForeignIpHosts …)`
    #   （否则 Set-StrictMode 下 `$null.Count` 直接终止脚本 —— 首次跑正例时就是这么红的）。
    $found = @()
    foreach ($mt in [regex]::Matches($text, 'http://(\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3})')) {
        $h = $mt.Groups[1].Value
        if (-not $h.StartsWith("127.")) { $found += $h }
    }
    return @($found | Sort-Object -Unique)
}
function Get-StampedInternalValue() {
    # 上一次"自证通过"的变体戳：仅当它是内网版时返回那个值（供本次开源臂交叉核对 ⇒ 堵住
    # "-SkipBuild 默认臂复用上一个**内网**产物"这条泄漏方向，与注入值是不是 IP 形态无关）。
    if (-not (Test-Path -LiteralPath $VariantStampPath)) { return "" }
    $lines = @([System.IO.File]::ReadAllText($VariantStampPath) -split "`r?`n")
    if ($lines.Count -lt 2) { return "" }
    if ($lines[0].Trim() -ne "内网") { return "" }
    return $lines[1].Trim()
}
function Assert-Variant([string]$exe, [bool]$expectInternal, [string]$expectedValue) {
    $text = Get-ExeText $exe
    if ($expectInternal) {
        if ($expectedValue -eq $OpenDefaultUpstream) {
            Fail "注入值与开源回退值同值（$OpenDefaultUpstream）⇒ 该产物无法自证变体，拒绝继续"
            exit 1
        }
        if (-not $text.Contains($expectedValue)) {
            Fail "产物**不含**本次注入值 $(Masked $expectedValue) ⇒ 它不是内网版（开关与产物脱钩：cargo 未重编 / 复用了旧产物？）"
            exit 1
        }
        Pass "产物变体自证：内网（exe 字节含本次注入值 needle $(Masked $expectedValue)）"
    } else {
        if (-not $text.Contains($OpenDefaultUpstream)) {
            Fail "产物不含开源回退值 $OpenDefaultUpstream ⇒ 它不是开源版"
            exit 1
        }
        $foreign = @()
        $stamped = Get-StampedInternalValue
        if (($stamped -ne "") -and ($stamped -ne $OpenDefaultUpstream) -and $text.Contains($stamped)) {
            $foreign += "上一次构建留下的注入值 $(Masked $stamped)（变体戳 $VariantStampPath）"
        }
        if (Test-Path -LiteralPath $InjectionSourcePath) {
            $localValue = ([System.IO.File]::ReadAllText($InjectionSourcePath) -split "`r?`n")[0].Trim()
            if (($localValue -ne "") -and ($localValue -ne $OpenDefaultUpstream) -and $text.Contains($localValue)) {
                $foreign += "本地注入源 .internal-default.txt 的值 $(Masked $localValue)"
            }
        }
        $ipHosts = @(Get-ForeignIpHosts $text)
        if ($ipHosts.Count -gt 0) { $foreign += "非回环点分十进制上游 host（$($ipHosts -join ', ')）" }
        if ($foreign.Count -gt 0) {
            Fail "产物含注入值（来源 = $($foreign -join '；')）⇒ 它是内网版，**不得按开源版命名/发布**（不写 dist、不刷归档）"
            exit 1
        }
        Pass "产物变体自证：开源（exe 含开源回退值，且无任何外来默认上游 needle）"
    }
    # 自证通过 ⇒ 落一枚变体戳（下次开源臂交叉核对用；落在 target/（已 gitignore））
    $stampLabel = if ($expectInternal) { "内网" } else { "开源" }
    $stampValue = if ($expectInternal) { $expectedValue } else { $OpenDefaultUpstream }
    [System.IO.File]::WriteAllText($VariantStampPath, ("$stampLabel`n$stampValue`n"), (New-Object System.Text.UTF8Encoding($false)))
}

Write-Host "== build-win7（目标 $Target / 工具链 $Toolchain）==" -ForegroundColor Cyan
Write-Host ("本次默认上游 = {0}（变体：{1}；来源：{2}）" -f $envDefaultUpstream, $VariantLabel, $VariantSource) -ForegroundColor Cyan
Write-Host ("产物名 = {0}；四道门归档 = {1}" -f $DistName, $VerifyDirFinal)

# ── 0. 前置检查 ────────────────────────────────────────────────────────────────
Write-Host ""
Write-Host "== 0. 前置检查 ==" -ForegroundColor Cyan
$tcList = (& rustup toolchain list 2>&1 | Out-String)
if ($tcList -match [regex]::Escape($Toolchain)) {
    Pass "工具链 $Toolchain 已安装"
} else {
    Fail "缺工具链 $Toolchain ⇒ 先装：rustup toolchain install $Toolchain --component rust-src"
}
$vcX64 = Join-Path $Root "assets\vc-ltl\x64"
$libs = @()
if (Test-Path -LiteralPath $vcX64) {
    $libs = @(Get-ChildItem -LiteralPath $vcX64 -Filter *.lib | ForEach-Object { $_.Name } | Sort-Object)
}
$expected = @("libucrt.lib", "libvcruntime.lib", "ucrt.lib", "vcruntime.lib")
$debugLibs = @($libs | Where-Object { $_ -like "*d.lib" })
if (($libs.Count -eq 4) -and ($debugLibs.Count -eq 0) -and (@(Compare-Object $libs $expected).Count -eq 0)) {
    Pass "VC-LTL5 x64 资产恰 4 个 release .lib（无 *d.lib）"
} else {
    Fail "VC-LTL5 x64 资产不齐（实际：$($libs -join ', ')）⇒ 先跑 scripts/fetch-assets.ps1"
}
$objPath = Join-Path $Root "assets\yy-thunks\YY_Thunks_for_Win7.obj"
if (Test-Path -LiteralPath $objPath) { Pass "YY-Thunks obj 在位（x64）" } else { Fail "缺 $objPath ⇒ 先跑 scripts/fetch-assets.ps1" }
if ($script:Failures -gt 0) {
    Write-Host "前置不齐，终止（绝不静默产出坏产物）" -ForegroundColor Red
    exit 1
}

# ── 1. 构建 ────────────────────────────────────────────────────────────────────
Write-Host ""
Write-Host "== 1. 构建 ==" -ForegroundColor Cyan
if ($SkipBuild) {
    Note "（-SkipBuild：跳过构建，复用现有产物）"
} else {
    # 注入源的**显式重编触发**（P1-1(a)）：`rerun-if-changed` 对"注入源以旧 mtime 复原"不敏感 ⇒
    # 每次构建前把注入源 touch 一次（**只动 mtime，内容一字不改**），保证 build.rs 真的重跑、
    # 注入值取的是现读值。⚠ 裸 `cargo`（不经本脚本）没有这一步 ⇒ 复原后请手工 `touch`（README 已写）。
    if (Test-Path -LiteralPath $InjectionSourcePath) {
        (Get-Item -LiteralPath $InjectionSourcePath).LastWriteTime = Get-Date
        Note "注入源 .internal-default.txt 已 touch（强制 build.rs 重跑；内容未改）"
    }
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    Push-Location $Root
    try {
        # --message-format=json：产物路径以 **cargo 自己发出的消息**为准（`compiler-artifact` 的
        # executable 字段；no-op 重建也会发）——绝不再按约定猜路径（RK21 同款纪律）。
        # ⚠ 不写 `2>&1`：PS 5.1 下重定向会把他方 stderr 变 ErrorRecord，配合 EAP=Stop 直接终止脚本
        #   （cargo 的 cargo:warning 曾把本脚本打断）；stderr（进度/warning）直通即可。
        $raw = & cargo "+$Toolchain" build --release -Z build-std=std,panic_abort --target $Target --message-format=json
        $buildRc = $LASTEXITCODE
    } finally {
        Pop-Location
    }
    $sw.Stop()
    $exeFromCargo = $null
    foreach ($line in @($raw)) {
        $text = "$line"
        if (-not $text.TrimStart().StartsWith("{")) { Write-Host $text; continue }
        try { $obj = $text | ConvertFrom-Json } catch { continue }
        if ($obj.reason -eq "compiler-message" -and $obj.message.rendered) { Write-Host $obj.message.rendered }
        if ($obj.reason -eq "compiler-artifact" -and $obj.executable) { $exeFromCargo = $obj.executable }
    }
    if ($buildRc -ne 0) { Fail "cargo build 失败（exit $buildRc）"; exit 1 }
    Pass ("cargo build 完成（{0:N1} s）" -f $sw.Elapsed.TotalSeconds)
    if ($exeFromCargo) {
        if ($exeFromCargo -ne $ExePath) { Note "cargo 报告的产物路径与约定不同，以 cargo 消息为准：$exeFromCargo" }
        $ExePath = $exeFromCargo
    } else {
        Fail "cargo 消息中未解析到 executable（预期 product=bin）⇒ 拒绝继续"
        exit 1
    }
}
if (-not (Test-Path -LiteralPath $ExePath)) { Fail "找不到构建产物：$ExePath"; exit 1 }
$exeItem = Get-Item -LiteralPath $ExePath
Pass ("产物：{0}（{1:N0} B，mtime {2}）" -f $ExePath, $exeItem.Length, $exeItem.LastWriteTime.ToString("HH:mm:ss"))

# ── 1b. 产物变体自证（P6-B4b 修正轮 P1-1；`-SkipBuild` 同样经过）──────────────────
Write-Host ""
Write-Host "== 1b. 产物变体自证（exe 字节 needle；不依赖 cargo 是否重编）==" -ForegroundColor Cyan
Assert-Variant $ExePath $IsInternal $envDefaultUpstream

# ── 2. PE 资源自检 ─────────────────────────────────────────────────────────────
Write-Host ""
Write-Host "== 2. PE 资源自检（版本信息 / 图标）==" -ForegroundColor Cyan
$cargoToml = Get-Content -LiteralPath (Join-Path $Root "Cargo.toml") -Raw
$m = [regex]::Match($cargoToml, '(?m)^version\s*=\s*"([^"]+)"')
$cargoVersion = if ($m.Success) { $m.Groups[1].Value } else { "" }
$vi = $exeItem.VersionInfo
foreach ($field in @("FileVersion", "ProductVersion", "ProductName", "FileDescription", "LegalCopyright", "OriginalFilename")) {
    $val = $vi.$field
    if ([string]::IsNullOrWhiteSpace($val)) { Fail "版本信息 $field 为空" } else { Pass "版本信息 $field = $val" }
}
if (($cargoVersion -ne "") -and ($vi.FileVersion -eq $cargoVersion)) {
    Pass "FileVersion 与 Cargo.toml 对齐（$cargoVersion）"
} else {
    Fail "FileVersion（$($vi.FileVersion)）与 Cargo.toml（$cargoVersion）不符"
}
try {
    Add-Type -AssemblyName System.Drawing
    $icon = [System.Drawing.Icon]::ExtractAssociatedIcon($ExePath)
    if ($null -ne $icon) { Pass ("图标可提取（{0}x{1}）" -f $icon.Width, $icon.Height) } else { Fail "无法从 exe 提取图标（资源损坏？）" }
} catch {
    Fail "图标提取异常：$_"
}

# ── 3. 四道验证（verify-win7.ps1 一键跑）────────────────────────────────────────
Write-Host ""
Write-Host "== 3. 四道验证 ==" -ForegroundColor Cyan
& powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $Root "scripts\verify-win7.ps1") -ExePath $ExePath -OutDir $VerifyDirFinal
$verifyRc = $LASTEXITCODE
if ($verifyRc -eq 0) { Pass "四道验证通过（Analyzer 三连 + objdump 三查 + API 自证 + 归档）" } else { Fail "四道验证失败（exit $verifyRc）" }

# ── 4. 归位 dist/ ──────────────────────────────────────────────────────────────
Write-Host ""
Write-Host "== 4. 归位 dist/ ==" -ForegroundColor Cyan
if ($script:Failures -gt 0) {
    Write-Host "存在失败项 ⇒ 不写 dist/（只发布通过验证的产物）" -ForegroundColor Red
} else {
    New-Item -ItemType Directory -Force -Path $DistDir | Out-Null
    $distExe = Join-Path $DistDir $DistName
    Copy-Item -LiteralPath $ExePath -Destination $distExe -Force
    $distItem = Get-Item -LiteralPath $distExe
    $sha = Sha256 $distExe
    $md5 = Md5 $distExe
    Pass ("dist 产物：{0}（{1:N0} B / md5 {2} / sha256 {3}）" -f $DistName, $distItem.Length, $md5, $sha)

    # SHA256SUMS.txt：多产物各一行；仅重写"本产物行"、保留其它产物行（幂等；**排序 + 去重**；UTF-8 无 BOM、LF）
    $sumsPath = Join-Path $DistDir "SHA256SUMS.txt"
    $lines = @()
    if (Test-Path -LiteralPath $sumsPath) {
        $lines = @(Get-Content -LiteralPath $sumsPath | Where-Object { $_ -notmatch [regex]::Escape($DistName) })
    }
    $lines += ("{0}  {1}" -f $sha, $DistName)
    $lines = @($lines | Where-Object { $_.Trim() -ne "" } | Sort-Object -Unique)
    [System.IO.File]::WriteAllText($sumsPath, ($lines -join "`n") + "`n", (New-Object System.Text.UTF8Encoding($false)))
    Pass "SHA256SUMS.txt 已更新（$($lines.Count) 行；排序 + 去重 + 无 BOM）"
}

# ── 汇总（开源版 → win7-verification/（**入库**）；内网版 → target/ 下 gitignored 目录，D13）────
Write-Host ""
Write-Host "== 汇总 ==" -ForegroundColor Cyan
$dateTag = Get-Date -Format "yyyyMMdd"
$summaryLines = @(
    "build-win7 一键构建汇总（方案 §9 Phase 4W / P6-K11 双产物）"
    "时间：$(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')"
    "变体：$VariantLabel（默认上游来源：$VariantSource）"
    "本次默认上游：$envDefaultUpstream"
    "目标：$Target（工具链 $Toolchain）"
    "exe：$ExePath"
    "exe 字节：$($exeItem.Length)"
    "exe md5：$(Md5 $ExePath)"
    "exe sha256：$(Sha256 $ExePath)"
    "FileVersion：$($vi.FileVersion)（Cargo.toml = $cargoVersion）"
    "四道验证退出码：$verifyRc"
    "dist 产物：$DistName（若失败项 > 0 则不写 dist）"
    "失败计数：$($script:Failures)"
)
New-Item -ItemType Directory -Force -Path $VerifyDirFinal | Out-Null
$summaryPath = Join-Path $VerifyDirFinal "build-win7-summary-$dateTag$VariantSuffix.txt"
[System.IO.File]::WriteAllText($summaryPath, (($summaryLines -join "`n") + "`n"), (New-Object System.Text.UTF8Encoding($false)))
Write-Host "汇总 -> $summaryPath"
if ($IsInternal) {
    Write-Host "（内网版：四道门归档与汇总**不入库**（D13 红线）—— 落在 target/ 下；win7-verification/ 只承载开源版读数）" -ForegroundColor Yellow
}

if ($script:Failures -gt 0) {
    Write-Host ""
    Write-Host "build-win7：$($script:Failures) 项失败" -ForegroundColor Red
    exit 1
}
Write-Host ""
Write-Host "build-win7：全链通过（构建 + PE 自检 + 四道验证 + dist 归位）" -ForegroundColor Green
exit 0
