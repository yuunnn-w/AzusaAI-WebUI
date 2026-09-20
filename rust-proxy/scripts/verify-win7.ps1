#requires -Version 5.1
<#
verify-win7.ps1 —— Win7 四道验证一键化 + 归档（方案 §5.6 / §8.6 1d）。

① YY.Depends.Analyzer /Target:6.1.7600 /IgnoreReady（报告落点 = **分析器自己目录**
   `tools/depends/<exe 名>.md`，按实测，不是 exe 旁 —— 方案 §5.6.1 坑 2）
   三连断言：**存在 → mtime 晚于本次运行起点 → 为空**；报告缺失 ⇒ 立即 exit 1
   并打印搜索过的全部路径（绝不允许把"找不到报告"读成"空报告 = 通过"，RK21）。
② objdump 三查（w64devkit）：DLL Name 清单 / 无 VCRUNTIME140+api-ms-win-crt-* / 子系统 ≤ 6.1
③ 关键 API 自证（Config/x64/6.1.7600.txt；期望 **3 / 0 / 0**；DB 语义 = “该 OS 的导出集合”，
   格式 = `序号=API名`，**不得当缺口清单解析**）
④ 归档：报告 + 三查输出 + 自证读数 → win7-verification/（含 md5 与命令原文；
   `objdump | grep` 形态的输出一律**按字节封顶** —— 方案 §5.6.6 / E-10 第 2 条）

用法：
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts/verify-win7.ps1
退出码：任一检查失败 ⇒ exit 1。
#>
[CmdletBinding()]
param(
    [string]$ExePath = "",
    [string]$OutDir = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
try { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 } catch { }

$Root = Split-Path -Parent $PSScriptRoot
if ([string]::IsNullOrWhiteSpace($ExePath)) {
    $ExePath = Join-Path $Root "target\x86_64-win7-windows-msvc\release\azusa-local-proxy.exe"
}
if ([string]::IsNullOrWhiteSpace($OutDir)) {
    $OutDir = Join-Path $Root "win7-verification"
}
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$script:Failures = 0
function Record-Fail([string]$Message) {
    Write-Host "FAIL：$Message" -ForegroundColor Red
    $script:Failures++
}
function Record-Pass([string]$Message) {
    Write-Host "PASS：$Message" -ForegroundColor Green
}
function Write-Capped([string]$Text, [string]$Path, [int]$MaxBytes = 65536) {
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($Text)
    if ($bytes.Length -gt $MaxBytes) {
        $slice = New-Object byte[] $MaxBytes
        [System.Array]::Copy($bytes, $slice, $MaxBytes)
        $Text = [System.Text.Encoding]::UTF8.GetString($slice) + "`n...(已按 $MaxBytes 字节封顶)"
    }
    # 行尾纪律（审查 P2-④）：证据文件按仓库约定写「UTF-8 无 BOM + LF」——`Set-Content -Encoding UTF8`
    #   会加 BOM 并补 CRLF，而本仓库 `* -text`（禁任何自动行尾转换）⇒ 那些字节会原样入库（混合行尾）。
    if (-not $Text.EndsWith("`n")) { $Text += "`n" }
    [System.IO.File]::WriteAllText($Path, $Text, (New-Object System.Text.UTF8Encoding($false)))
}

$started = Get-Date
$summary = New-Object System.Collections.Generic.List[string]

if (-not (Test-Path -LiteralPath $ExePath)) {
    Write-Host "找不到 exe：$ExePath" -ForegroundColor Red
    Write-Host "先构建：cargo +nightly-2026-06-03 build --release -Z build-std=std,panic_abort --target x86_64-win7-windows-msvc"
    exit 1
}
$exeAbs = (Resolve-Path -LiteralPath $ExePath).Path
$fileVersion = (Get-Item -LiteralPath $exeAbs).VersionInfo.FileVersion
if (-not $fileVersion) { $fileVersion = "unknown" }
$dateTag = Get-Date -Format "yyyyMMdd"
$summary.Add("exe = $exeAbs")
$summary.Add("FileVersion = $fileVersion")
$summary.Add("运行起点 = $($started.ToString('yyyy-MM-dd HH:mm:ss'))")

# ── ① YY.Depends.Analyzer ───────────────────────────────────────────────────────
Write-Host ""
Write-Host "== ① YY.Depends.Analyzer（/Target:6.1.7600 /IgnoreReady）==" -ForegroundColor Cyan
$analyzer = Join-Path $Root "tools\depends\YY.Depends.Analyzer.exe"
$analyzerDir = Split-Path -Parent $analyzer
$configDbX64 = Join-Path $analyzerDir "Config\x64\6.1.7600.txt"
$objsDir = Join-Path $analyzerDir "objs"
$reportName = [System.IO.Path]::GetFileName($exeAbs) + ".md"
$reportPath = Join-Path $analyzerDir $reportName
# 实测落点 = `<exe 文件名（含 .exe）>.md`（Phase 0 现测：tools/depends/azusa-local-proxy.exe.md；
# 方案 §5.6.1 的 `<exe名>.md` 按此口径落地 —— 无扩展名变体已列入诊断路径）
$searched = @(
    $reportPath,
    (Join-Path $analyzerDir ([System.IO.Path]::GetFileNameWithoutExtension($exeAbs) + ".md")),
    (Join-Path (Split-Path -Parent $exeAbs) $reportName)
)

if (-not (Test-Path -LiteralPath $analyzer)) {
    Record-Fail "找不到分析器（先运行 scripts/fetch-assets.ps1）：$analyzer"
} else {
    if (-not (Test-Path -LiteralPath $configDbX64)) {
        Record-Fail "分析器旁缺 Config\x64\6.1.7600.txt ⇒ 本次运行会静默退化（指南 §4.1 坑 1）"
    }
    if (-not (Test-Path -LiteralPath $objsDir)) {
        Record-Fail "分析器旁缺 objs\ ⇒ /IgnoreReady 变成空操作（指南 §4.1 坑 1）"
    }

    $env:MSYS_NO_PATHCONV = "1"
    $cmdText = '"{0}" "{1}" /Target:6.1.7600 /IgnoreReady /ReportView:Table' -f $analyzer, $exeAbs
    Write-Host "命令：$cmdText"
    $oldEncoding = [Console]::OutputEncoding
    try { [Console]::OutputEncoding = [System.Text.Encoding]::GetEncoding(936) } catch { }
    $analyzerOut = (& $analyzer $exeAbs /Target:6.1.7600 /IgnoreReady /ReportView:Table 2>&1 | Out-String)
    if ($oldEncoding) { [Console]::OutputEncoding = $oldEncoding }
    $rc = $LASTEXITCODE
    Write-Host "分析器退出码 = $rc（0 = 正常；87 = /Target: 开关被改写 —— Git Bash 下需 MSYS_NO_PATHCONV=1）"
    Write-Capped ("命令：$cmdText`n退出码：$rc`n`n$analyzerOut") (Join-Path $OutDir "analyzer-stdout-旁证.txt")
    $summary.Add("analyzer 命令 = $cmdText")
    $summary.Add("analyzer rc = $rc")

    if (-not (Test-Path -LiteralPath $reportPath)) {
        Write-Host "报告不存在（主判据落点 = 分析器目录）。已搜索路径：" -ForegroundColor Red
        foreach ($candidate in $searched) { Write-Host "   - $candidate" }
        Write-Host "分析器 stdout（旁证，可能乱码）："
        Write-Host $analyzerOut
        exit 1
    }
    $reportItem = Get-Item -LiteralPath $reportPath
    if ($reportItem.LastWriteTime -lt $started) {
        Record-Fail ("报告陈旧：mtime {0} 早于本次运行起点 {1}" -f $reportItem.LastWriteTime, $started)
    } else {
        Record-Pass ("报告存在且为本次运行产出：{0}（mtime {1}）" -f $reportPath, $reportItem.LastWriteTime)
    }
    $raw = Get-Content -LiteralPath $reportPath -Raw
    if ($null -eq $raw) { $raw = "" }
    $raw = $raw.TrimStart([char]0xFEFF)
    $bodyLines = @($raw -split "`r?`n" | Where-Object { $_.Trim() -ne "" -and (-not $_.Trim().StartsWith("#")) })
    if ($bodyLines.Count -eq 0) {
        Record-Pass "报告为空（仅标题行）= 对 Win7 无静态导入缺口"
    } else {
        Record-Fail ("报告非空：{0} 条非标题行（存在 Win7 静态导入缺口）→ {1}" -f $bodyLines.Count, $reportPath)
    }
    $reportMd5 = (Get-FileHash -Algorithm MD5 -LiteralPath $reportPath).Hash.ToLowerInvariant()
    $archiveName = "analyzer-win7-x64-$fileVersion-$dateTag.md"
    Copy-Item -LiteralPath $reportPath -Destination (Join-Path $OutDir $archiveName) -Force
    $recordText = @(
        "Analyzer 报告归档记录（win7-verification/）"
        "命令：$cmdText"
        "退出码：$rc"
        "报告落点（主判据）：$reportPath"
        "报告 mtime：$($reportItem.LastWriteTime.ToString('yyyy-MM-dd HH:mm:ss'))（本次运行起点 $($started.ToString('yyyy-MM-dd HH:mm:ss'))）"
        "报告 md5：$reportMd5"
        "归档副本：$archiveName"
        "注意：输入的 exe 路径为绝对 Windows 路径（报告 Ref Module 列含输入路径原文 ⇒ 换写法会改变报告字节）。"
    ) -join "`n"
    Write-Capped $recordText (Join-Path $OutDir "analyzer-record.txt")
    $summary.Add("analyzer 报告 = $reportPath（md5 $reportMd5，已复制 $archiveName）")
    $summary.Add("analyzer 已搜索路径 = $($searched -join ' ; ')")
}

# ── ② objdump 三查 ─────────────────────────────────────────────────────────────
Write-Host ""
Write-Host "== ② objdump 三查 ==" -ForegroundColor Cyan
$objdump = $null
$objdumpCandidates = @("C:\Program Files\w64devkit\bin\objdump.exe")
if ($env:W64DEVKIT_HOME) { $objdumpCandidates += (Join-Path $env:W64DEVKIT_HOME "bin\objdump.exe") }
foreach ($candidate in $objdumpCandidates) {
    if ($candidate -and (Test-Path -LiteralPath $candidate)) { $objdump = $candidate; break }
}
if (-not $objdump) {
    $found = Get-Command objdump.exe -ErrorAction SilentlyContinue
    if ($found) { $objdump = $found.Source }
}
if (-not $objdump) {
    Record-Fail "找不到 objdump（指南 §4.2：w64devkit / MSYS2 binutils / llvm-objdump）"
} else {
    Write-Host "objdump = $objdump"
    $cmd2 = '"{0}" -p "{1}"' -f $objdump, $exeAbs
    $dump = (& $objdump -p $exeAbs | Out-String)
    Write-Capped ("命令：$cmd2`n`n$dump") (Join-Path $OutDir "objdump-full-字节封顶.txt")

    $dllLines = @($dump -split "`r?`n" | Where-Object { $_ -match "DLL Name" })
    Write-Capped ($dllLines -join "`n") (Join-Path $OutDir "objdump-1-dll-name.txt")
    # 查① 允许清单（v3.2 补强，审查 P2-3）：**只允许 msvcrt.dll + 系统 DLL**。
    # 原实现只归档 + 无条件 Record-Pass ⇒ 将来某依赖带入非 CRT 的第三方 DLL（如 zlib1.dll）时
    # 查②（只查 VCRUNTIME140|api-ms-win-crt-）不命中、查① 不判定 ⇒ **静默通过**（与 RK21 同族形状）。
    # 语义：`api-ms-win-*` 是 API Set（含 api-ms-win-core-*），Win7 正常 ⇒ 列入清单；
    #       其中的 `api-ms-win-crt-*` 由查② 单独判定（两查刻意正交、不重复）。
    # 匹配规则：等值，或 `清单项 + "."`（如 kernel32 ⇒ kernel32.dll），或以 `-` 结尾的清单项作前缀
    #           （api-ms-win-）—— 不做无边界前缀匹配（避免 `kernel32xxx.dll` 混过）。
    $dllAllow = @(
        "msvcrt.dll", "kernel32", "kernelbase", "ntdll", "ws2_32", "mswsock", "advapi32",
        "sechost", "user32", "shell32", "shlwapi", "comctl32", "comdlg32", "gdi32", "gdiplus",
        "ole32", "oleaut32", "uuid", "rpcrt4", "crypt32", "bcrypt", "ncrypt", "wintrust",
        "iphlpapi", "dnsapi", "netapi32", "userenv", "psapi", "powrprof", "setupapi",
        "cfgmgr32", "version", "imm32", "winmm", "uxtheme", "dwmapi", "api-ms-win-"
    )
    $dllNames = @()
    foreach ($line in $dllLines) {
        $match = [regex]::Match($line, "DLL Name:\s*(\S+)")
        if ($match.Success) { $dllNames += $match.Groups[1].Value }
    }
    $unknown = @()
    foreach ($name in $dllNames) {
        $lower = $name.ToLowerInvariant()
        $ok = $false
        foreach ($allowed in $dllAllow) {
            if ($lower -eq $allowed) { $ok = $true; break }
            if ($allowed.EndsWith("-")) { if ($lower.StartsWith($allowed)) { $ok = $true; break } }
            elseif ($lower.StartsWith($allowed + ".")) { $ok = $true; break }
        }
        if (-not $ok) { $unknown += $name }
    }
    if ($dllNames.Count -eq 0) {
        # 零条 = 解析失败（PE 必有导入表）⇒ 绝不能读成"清单干净"（防永真门禁，同 RK21）
        Record-Fail "查① 未能从 objdump 输出解析出任何 DLL Name（解析失败或输出异常）"
    } elseif ($unknown.Count -gt 0) {
        Record-Fail ("查① 出现允许清单外的 DLL（{0} 条）：{1} ⇒ 目标机需额外 DLL，Win7 免安装失效（P2-3）" -f $unknown.Count, (($unknown | ForEach-Object { $_.Trim() }) -join " ; "))
    } else {
        Record-Pass ("查① {0} 条 DLL 全在允许清单内（msvcrt.dll + 系统 DLL）：{1}" -f $dllNames.Count, ($dllNames -join " | "))
    }
    $summary.Add("objdump ① DLL Name = " + (($dllNames | ForEach-Object { $_.Trim() }) -join " | "))
    $unknownText = if ($unknown.Count -eq 0) { "（无）" } else { $unknown -join " ; " }
    $summary.Add("objdump ① 清单外 DLL = $unknownText")

    $crtLines = @($dump -split "`r?`n" | Where-Object { $_ -match "VCRUNTIME140|api-ms-win-crt-" })
    Write-Capped ($crtLines -join "`n") (Join-Path $OutDir "objdump-2-crt.txt")
    if ($crtLines.Count -eq 0) {
        Record-Pass "查② 无 VCRUNTIME140 / api-ms-win-crt-*（第 1 层已修）"
    } else {
        Record-Fail ("查② 命中 CRT 组 {0} 条（第 1 层未修）：{1}" -f $crtLines.Count, (($crtLines | ForEach-Object { $_.Trim() }) -join " ; "))
    }

    $majorMatch = [regex]::Match($dump, "MajorSubsystemVersion\s+(\d+)")
    $minorMatch = [regex]::Match($dump, "MinorSubsystemVersion\s+(\d+)")
    if ($majorMatch.Success -and $minorMatch.Success) {
        $major = [int]$majorMatch.Groups[1].Value
        $minor = [int]$minorMatch.Groups[1].Value
        $versionText = "$major.$minor"
        Write-Capped ("MajorSubsystemVersion = $major`nMinorSubsystemVersion = $minor`n（命令：$cmd2）") (Join-Path $OutDir "objdump-3-subsystem.txt")
        if (($major -lt 6) -or (($major -eq 6) -and ($minor -le 1))) {
            Record-Pass "查③ 子系统版本 $versionText <= 6.1"
        } else {
            Record-Fail "查③ 子系统版本 $versionText > 6.1（Win7 加载器会拒绝）"
        }
        $summary.Add("objdump ③ 子系统版本 = $versionText")
    } else {
        Record-Fail "查③ 无法从 objdump 输出解析子系统版本"
    }
}

# ── ③ 关键 API 离线自证 ────────────────────────────────────────────────────────
Write-Host ""
Write-Host "== ③ 关键 API 离线自证（Config/x64/6.1.7600.txt）==" -ForegroundColor Cyan
$configDb = Join-Path $Root "tools\depends\Config\x64\6.1.7600.txt"
if (-not (Test-Path -LiteralPath $configDb)) {
    Record-Fail "缺少 $configDb（先运行 scripts/fetch-assets.ps1）"
} else {
    $qcse = 0; $gstp = 0; $ghnw = 0; $pdh = 0; $woa = 0; $prng = 0
    foreach ($line in [System.IO.File]::ReadLines($configDb)) {
        if ($line -match "=GetQueuedCompletionStatusEx$") { $qcse++ }
        if ($line -match "=GetSystemTimePreciseAsFileTime$") { $gstp++ }
        if ($line -match "=GetHostNameW$") { $ghnw++ }
        if ($line -match "=PdhAddEnglishCounterW$") { $pdh++ }
        if ($line -match "=WaitOnAddress$") { $woa++ }
        if ($line -match "=ProcessPrng$") { $prng++ }
    }
    $readingText = @(
        ('数据库 = {0}（{1} B）' -f $configDb, (Get-Item -LiteralPath $configDb).Length)
        ('grep -c "=GetQueuedCompletionStatusEx$"        = {0}   （期望 > 0；tokio/mio IOCP 主循环）' -f $qcse)
        ('grep -c "=GetSystemTimePreciseAsFileTime$"     = {0}   （期望 = 0；Win8 新增 ⇒ 由 win7 std 分支/thunk 覆盖）' -f $gstp)
        ('grep -c "=GetHostNameW$"                       = {0}   （期望 = 0）' -f $ghnw)
        ('grep -c "=PdhAddEnglishCounterW$"              = {0}   （指南 §1.3 例；期望 > 0）' -f $pdh)
        ('grep -c "=WaitOnAddress$" / "=ProcessPrng$"    = {0} / {1}   （指南：Win8 新增；期望 0 / 0）' -f $woa, $prng)
        '说明：DB 格式 = “序号=API名”（同一名可多行），语义 = “该 OS 的导出集合”，不是缺口清单。'
    ) -join "`n"
    Write-Capped $readingText (Join-Path $OutDir "api-selfcheck.txt")
    $summary.Add("③ 自证读数（qcse / gstp / ghnw / pdh / woa / prng）= $qcse / $gstp / $ghnw / $pdh / $woa / $prng")
    if ($qcse -gt 0) { Record-Pass "GetQueuedCompletionStatusEx = $qcse > 0" } else { Record-Fail "GetQueuedCompletionStatusEx = $qcse，期望 > 0" }
    if ($gstp -eq 0) { Record-Pass "GetSystemTimePreciseAsFileTime = 0" } else { Record-Fail "GetSystemTimePreciseAsFileTime = $gstp，期望 0" }
    if ($ghnw -eq 0) { Record-Pass "GetHostNameW = 0" } else { Record-Fail "GetHostNameW = $ghnw，期望 0" }
}

# ── ④ 归档 ─────────────────────────────────────────────────────────────────────
Write-Host ""
Write-Host "== ④ 归档 ==" -ForegroundColor Cyan
$summary.Add("失败计数 = $script:Failures")
$summaryPath = Join-Path $OutDir "verify-summary-$dateTag.txt"
$summaryText = @("verify-win7 归档摘要（命令原文 + 读数；Analyzer 报告落点 = 分析器目录）", "") + $summary
Write-Capped ($summaryText -join "`n") $summaryPath 131072
Write-Host "摘要 -> $summaryPath"
Get-ChildItem -LiteralPath $OutDir -File | ForEach-Object { Write-Host ("  {0,10} B  {1}" -f $_.Length, $_.Name) }

if ($script:Failures -gt 0) {
    Write-Host ""
    Write-Host "四道验证存在 $script:Failures 项失败" -ForegroundColor Red
    exit 1
}
Write-Host ""
Write-Host "四道验证全部通过（Analyzer 三连 + objdump 三查 + API 自证 + 归档）" -ForegroundColor Green
exit 0
