#requires -Version 5.1
<#
build-modern.ps1 —— 现代 Windows 标准构建（方案 §5.4 第三行：x86_64-pc-windows-msvc + crt-static）

说明（与 build-win7.ps1 的分工）：
  · 两套构建共用同一份源码：Win7 两层接线（VC-LTL5 + YY-Thunks）由 build.rs 按 TARGET 收口，
    本目标不含 `win7` ⇒ 不注入；本目标走 `+crt-static`（静态 CRT，无 UCRT/VCRUNTIME DLL 依赖）。
  · 本目标**不跑** Win7 四道验证 —— 那是 win7 目标的发布门槛（子系统/导入面口径不同）；
    本脚本只做：构建 + PE 资源自检 + 导入面软提示（WARN 非门禁）+ 归位 dist/。
  · 正式发布主产物仍是 `azusa-local-proxy-win7-x64.exe`（Win7 SP1+ 与现代 Windows 通用）；
    本产物（`azusa-local-proxy-win10-x64.exe`）为可选的现代档。

用法：
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-modern.ps1
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-modern.ps1 -SkipBuild
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-modern.ps1 -Internal
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-modern.ps1 -InternalDefault <url>
（-Internal / -InternalDefault 的语义与 build-win7.ps1 完全一致：不给 = 开源版，显式关闭注入；
  内网版产物名 = azusa-local-proxy-win10-x64-internal.exe）
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
$Target = "x86_64-pc-windows-msvc"
# 约定路径（-SkipBuild 时的回退）；带 --target 的产物在 target\<triple>\release\ —— **不是** target\release\
# （踩坑回放：曾误写 target\release ⇒ 验证/打包了旧的非 --target 产物，与"陈旧 Analyzer 报告"同族；
#  现已改为**从 cargo 消息取产物路径**，见 §1 —— 本行为 -SkipBuild 回退值（**不校验产物新旧**，语义 = "复用现有产物"））
$ExePath = Join-Path $Root "target\$Target\release\azusa-local-proxy.exe"
$Toolchain = "nightly-2026-06-03"

# ── K11-a / K11-c：双产物开关（与 build-win7.ps1 逐字同源的口径）────────────────────────
# 默认 = 开源版（`AZUSA_DEFAULT_UPSTREAM_OFF=1` ⇒ build.rs 明确不注入，即使本地存在 .internal-default.txt）；
# -Internal = 内网版（读本地 gitignored 文件）；-InternalDefault <url> = 内网版（显式值）。
# ⚠ "不注入"必须用**独立变量名**（`..._OFF`）：cargo 的进程环境对 rustc 也可见 ⇒ 若把 `off`
#   塞进 `AZUSA_DEFAULT_UPSTREAM`，`option_env!` 会读到它、产出"默认上游 = off"的坏二进制
#   （P6-B4b 实测踩到过）。
$OpenDefaultUpstream = "http://127.0.0.1:8080"     # 开源回退值：与 build.rs::OPEN_DEFAULT_UPSTREAM / src/config.rs 同值
$envDefaultUpstream = $OpenDefaultUpstream         # 本次**期望的默认上游**（开源臂 = 回退值本身；内网臂 = 注入值）
$InjectionSourcePath = Join-Path $Root ".internal-default.txt"        # gitignored 注入源（-Internal 读它；构建前 touch 它）
$VariantStampPath = Join-Path $Root "target\.variant-stamp-$Target.txt"  # 上次"自证通过"的变体（target/ 已 gitignore）
$VariantLabel = "开源"
$VariantSource = "缺省回退（脚本显式关闭注入）"
$VariantSuffix = ""
if ($Internal -and -not [string]::IsNullOrWhiteSpace($InternalDefault)) {
    Write-Host "FAIL：-Internal 与 -InternalDefault 互斥（前者读本地 gitignored 文件，后者显式给值）" -ForegroundColor Red
    exit 1
}
if ($Internal) {
    $internalFile = $InjectionSourcePath
    if (-not (Test-Path -LiteralPath $internalFile)) {
        Write-Host "FAIL：-Internal 需要 $internalFile（本地 gitignored 注入源），但它不存在" -ForegroundColor Red
        exit 1
    }
    $envDefaultUpstream = ([System.IO.File]::ReadAllText($internalFile) -split "`r?`n")[0].Trim()
    if ([string]::IsNullOrWhiteSpace($envDefaultUpstream)) {
        Write-Host "FAIL：$internalFile 的第一行为空 ⇒ 拒绝产出'内网版'（会静默变成开源版）" -ForegroundColor Red
        exit 1
    }
    $VariantSource = ".internal-default.txt（本地 gitignored 注入源）"
    $VariantLabel = "内网"
    $VariantSuffix = "-internal"
} elseif (-not [string]::IsNullOrWhiteSpace($InternalDefault)) {
    $envDefaultUpstream = $InternalDefault.Trim()
    $VariantSource = "-InternalDefault（命令行显式值）"
    $VariantLabel = "内网"
    $VariantSuffix = "-internal"
}
if ($VariantSuffix -ne "") {
    # 注入值校验（P6-B4b 修正轮 P2-3）：只查 `http://` 前缀会放过 `http://` / `http://:8080` 这类
    # **host 为空**的值 ⇒ 产出"自己的默认值不可用"的二进制。三条同口径（含 build.rs 侧）：
    # ① 必须是 http:// ② 不得含空白/控制字符 ③ host 段必须非空。
    # ⚠ 消息里**不回显注入值**（D13），只回显"值形态 + 长度"。
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
    # 开源版：**不设** AZUSA_DEFAULT_UPSTREAM（设了会被 rustc 的 option_env! 读到），只设关闭信号。
    Remove-Item Env:AZUSA_DEFAULT_UPSTREAM -ErrorAction SilentlyContinue
    $env:AZUSA_DEFAULT_UPSTREAM_OFF = "1"
}

$DistName = "azusa-local-proxy-win10-x64$VariantSuffix.exe"

$script:Failures = 0
function Fail([string]$m) { Write-Host "FAIL：$m" -ForegroundColor Red; $script:Failures++ }
function Pass([string]$m) { Write-Host "PASS：$m" -ForegroundColor Green }
function Warn([string]$m) { Write-Host "WARN：$m" -ForegroundColor Yellow }
function Note([string]$m) { Write-Host $m }
function Sha256([string]$path) { (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash.ToLowerInvariant() }
function Md5([string]$path) { (Get-FileHash -Algorithm MD5 -LiteralPath $path).Hash.ToLowerInvariant() }

# ── 产物变体自证（P6-B4b 修正轮 **P1-1**；与 build-win7.ps1 同源口径）──────────────
# 为什么必须钉在**产物字节**上（三条都已实测复现）：cargo 会把 build.rs 的 `cargo:warning` **重放**
# （同状态连跑两次 `Compiling=0` 仍有那一行）⇒ 日志不是见证；裸 cargo 对"注入源以**旧 mtime** 复原"
# **不敏感** ⇒ 产物可能仍是上一个变体；`-SkipBuild` 下开关**只影响命名**、根本不碰产物。
# 故在"产物就位之后、写 dist/ 之前"直接读 exe 字节查 needle，不符即**硬失败**（不写 dist、非 0 退出）；
# `-SkipBuild` 同样经过（§1b）。
function Masked([string]$v) {
    if ([string]::IsNullOrEmpty($v)) { return "<empty>" }
    return "<masked:len=$($v.Length)>"
}
function Get-ExeText([string]$exe) {
    # 直读字节后按 ISO-8859-1 解码（byte ↔ char 一比一）⇒ ASCII needle 的 UTF-8 连续字节被逐字命中。
    return [System.Text.Encoding]::GetEncoding(28591).GetString([System.IO.File]::ReadAllBytes($exe))
}
function Get-ForeignIpHosts([string]$text) {
    # 点分十进制 host 且**非** 127.x ⇒ 视为外来上游（内网注入值实测就是这种形态）。
    # ⚠ 返回**空数组**时 PowerShell 会把它摊平成 `$null` ⇒ 调用点必须写 `@(Get-ForeignIpHosts …)`
    #   （否则 Set-StrictMode 下 `$null.Count` 直接终止脚本）。
    $found = @()
    foreach ($mt in [regex]::Matches($text, 'http://(\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3})')) {
        $h = $mt.Groups[1].Value
        if (-not $h.StartsWith("127.")) { $found += $h }
    }
    return @($found | Sort-Object -Unique)
}
function Get-StampedInternalValue() {
    # 上一次"自证通过"的变体戳：仅当它是内网版时返回那个值（供本次开源臂交叉核对）。
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
            Fail "产物含注入值（来源 = $($foreign -join '；')）⇒ 它是内网版，**不得按开源版命名/发布**（不写 dist）"
            exit 1
        }
        Pass "产物变体自证：开源（exe 含开源回退值，且无任何外来默认上游 needle）"
    }
    # 自证通过 ⇒ 落一枚变体戳（下次开源臂交叉核对用；落在 target/（已 gitignore））
    $stampLabel = if ($expectInternal) { "内网" } else { "开源" }
    $stampValue = if ($expectInternal) { $expectedValue } else { $OpenDefaultUpstream }
    [System.IO.File]::WriteAllText($VariantStampPath, ("$stampLabel`n$stampValue`n"), (New-Object System.Text.UTF8Encoding($false)))
}

Write-Host "== build-modern（目标 $Target / crt-static / 工具链 $Toolchain）==" -ForegroundColor Cyan
Write-Host ("本次默认上游 = {0}（变体：{1}；来源：{2}）" -f $envDefaultUpstream, $VariantLabel, $VariantSource) -ForegroundColor Cyan
Write-Host ("产物名 = {0}" -f $DistName)

# ── 1. 构建 ────────────────────────────────────────────────────────────────────
Write-Host ""
Write-Host "== 1. 构建（+crt-static）==" -ForegroundColor Cyan
if ($SkipBuild) {
    Note "（-SkipBuild：跳过构建，复用现有产物）"
} else {
    # 注入源的**显式重编触发**（P1-1(a)）：`rerun-if-changed` 对"注入源以旧 mtime 复原"不敏感 ⇒
    # 每次构建前 touch 注入源（**只动 mtime，内容一字不改**），保证 build.rs 真的重跑。
    if (Test-Path -LiteralPath $InjectionSourcePath) {
        (Get-Item -LiteralPath $InjectionSourcePath).LastWriteTime = Get-Date
        Note "注入源 .internal-default.txt 已 touch（强制 build.rs 重跑；内容未改）"
    }
    $prevRustflags = $env:RUSTFLAGS
    if ([string]::IsNullOrWhiteSpace($prevRustflags)) {
        $env:RUSTFLAGS = "-C target-feature=+crt-static"
    } else {
        $env:RUSTFLAGS = "$prevRustflags -C target-feature=+crt-static"
    }
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    Push-Location $Root
    try {
        # --message-format=json：产物路径以 **cargo 自己发出的消息**为准（`compiler-artifact` 的
        # executable 字段；no-op 重建也会发）——绝不再按约定猜路径（RK21 同款纪律）。
        # ⚠ 不写 `2>&1`：PS 5.1 下重定向会把他方 stderr 变 ErrorRecord，配合 EAP=Stop 直接终止脚本。
        $raw = & cargo "+$Toolchain" build --release --target $Target --message-format=json
        $buildRc = $LASTEXITCODE
    } finally {
        Pop-Location
        $env:RUSTFLAGS = $prevRustflags
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
    Pass ("cargo build 完成（{0:N1} s；RUSTFLAGS 已含 -C target-feature=+crt-static）" -f $sw.Elapsed.TotalSeconds)
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

# ── 2. PE 资源自检（同 build-win7 口径：版本信息与图标）────────────────────────
Write-Host ""
Write-Host "== 2. PE 资源自检 ==" -ForegroundColor Cyan
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
    if ($null -ne $icon) { Pass ("图标可提取（{0}x{1}）" -f $icon.Width, $icon.Height) } else { Fail "无法从 exe 提取图标" }
} catch {
    Fail "图标提取异常：$_"
}

# ── 3. 导入面软提示（非门禁）───────────────────────────────────────────────────
Write-Host ""
Write-Host "== 3. 导入面软提示（crt-static 校验，非门禁）==" -ForegroundColor Cyan
$objdump = $null
$candidates = @("C:\Program Files\w64devkit\bin\objdump.exe")
if ($env:W64DEVKIT_HOME) { $candidates += (Join-Path $env:W64DEVKIT_HOME "bin\objdump.exe") }
foreach ($candidate in $candidates) { if (Test-Path -LiteralPath $candidate) { $objdump = $candidate; break } }
if (-not $objdump) { $found = Get-Command objdump.exe -ErrorAction SilentlyContinue; if ($found) { $objdump = $found.Source } }
if (-not $objdump) {
    Warn "找不到 objdump ⇒ 跳过导入面软提示（不影响本脚本门禁）"
} else {
    $dump = (& $objdump -p $ExePath | Out-String)
    $dllLines = @($dump -split "`r?`n" | Where-Object { $_ -match "DLL Name" })
    Note ("DLL Name（{0} 条）：{1}" -f $dllLines.Count, (($dllLines | ForEach-Object { $_.Trim() }) -join " | "))
    $crtLines = @($dump -split "`r?`n" | Where-Object { $_ -match "VCRUNTIME140|api-ms-win-crt-" })
    if ($crtLines.Count -eq 0) {
        Pass "导入面无 VCRUNTIME140 / api-ms-win-crt-*（crt-static 生效）"
    } else {
        Warn ("出现 CRT 组 {0} 条 ⇒ 说明 +crt-static 未生效，请检查 RUSTFLAGS（非本脚本门禁）" -f $crtLines.Count)
    }
}

# ── 4. 归位 dist/ ──────────────────────────────────────────────────────────────
Write-Host ""
Write-Host "== 4. 归位 dist/ ==" -ForegroundColor Cyan
if ($script:Failures -gt 0) {
    Write-Host "存在失败项 ⇒ 不写 dist/（只发布通过自检的产物）" -ForegroundColor Red
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

if ($script:Failures -gt 0) {
    Write-Host ""
    Write-Host "build-modern：$($script:Failures) 项失败" -ForegroundColor Red
    exit 1
}
Write-Host ""
Write-Host "build-modern：全链通过（构建 + PE 自检 + dist 归位）" -ForegroundColor Green
exit 0
