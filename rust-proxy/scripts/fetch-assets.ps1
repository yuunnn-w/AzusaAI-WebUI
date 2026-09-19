#requires -Version 5.1
<#
fetch-assets.ps1 —— Win7 构建资产按需取件（方案 §5.5.2 / DG14；指南 §3.2 / §8.1）。

链路（方案 §5.5.1 末注的执行顺序：取件 → 核验 → 解包 → 归位）：
  1) 经代理下载 VC-LTL-Binary.7z（v5.3.1）与 YY-Thunks-Objs.zip（v1.2.2）到 .cache/（幂等复用）
  2) SHA-256 硬核验（期望值逐字对照指南 §8.1；不匹配即 exit 1）
  3) 解包：7z 用 py7zr（方案 §5.5.1 第 1 步）；zip 用 Expand-Archive
  4) 归位（方案 §5.5.2 放置表）：
       assets/vc-ltl/x64| x86  ← 每架构恰 4 个 release .lib（libucrt / libvcruntime / ucrt / vcruntime）
       assets/yy-thunks        ← x64 源名=放置名；x86 源名无 _x86 后缀（改名只发生在放置端，方案 P1-4）
       tools/depends           ← YY.Depends.Analyzer.exe + Config/ + objs/（三者必须同目录，否则静默退化）

失败语义（写死）：URL 不可达 / 哈希不匹配 / 大小不符 / 解包失败 / 归位后文件数不符
                  ⇒ 一律 exit 1 并打印期望值与实际值（绝不"继续构建"）。

⚠ 离线构建需要 .cache/ 已有归档（DG14 的已知代价，README 已写明）。
#>
[CmdletBinding()]
param(
    [switch]$Force,
    [string]$Proxy = "http://127.0.0.1:7890"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
try { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 } catch { }

$Root     = Split-Path -Parent $PSScriptRoot
$CacheDir = Join-Path $Root ".cache"
$VcLtlOut = Join-Path $Root "assets\vc-ltl"
$YyOut    = Join-Path $Root "assets\yy-thunks"
$Depends  = Join-Path $Root "tools\depends"

$VcLtlUrl  = "https://github.com/Chuyu-Team/VC-LTL5/releases/download/v5.3.1/VC-LTL-Binary.7z"
$VcLtlSha  = "7a18799ed3aa84a225610a5447a56bc534c5c98ccb8dec05caba0e3f633431ad"
$VcLtlSize = 18657123

$YyUrl     = "https://github.com/Chuyu-Team/YY-Thunks/releases/download/v1.2.2/YY-Thunks-Objs.zip"
$YySha     = "518ed7ef4825e8a41997fbccfa2c8090cf31a6038fd51520a2e49886f947f9fc"
$YySize    = 14108378

function Fail([string]$Message) {
    Write-Host ""
    Write-Host "失败：$Message" -ForegroundColor Red
    exit 1
}

function Get-Sha256([string]$Path) {
    return (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
}

function Ensure-Archive([string]$Path, [string]$Url, [string]$Sha, [long]$Size, [string]$Label) {
    if ((Test-Path -LiteralPath $Path) -and (-not $Force)) {
        $actual = Get-Sha256 $Path
        if ($actual -eq $Sha) {
            $length = (Get-Item -LiteralPath $Path).Length
            Write-Host ("[复用] {0}：{1} B，sha256 相符" -f (Split-Path -Leaf $Path), $length) -ForegroundColor Green
            return
        }
        Write-Host ("[缓存哈希不符] {0}：期望 {1} / 实际 {2} —— 重新下载" -f (Split-Path -Leaf $Path), $Sha, $actual) -ForegroundColor Yellow
        Remove-Item -LiteralPath $Path -Force
    }
    if (Test-Path -LiteralPath $Path) { Remove-Item -LiteralPath $Path -Force }
    Write-Host ("[下载] {0}（{1}）" -f $Url, $Label)
    Write-Host ("        经代理 {0}" -f $Proxy)
    & curl.exe -L --fail --silent --show-error --proxy $Proxy --connect-timeout 30 --max-time 900 -o $Path $Url
    if ($LASTEXITCODE -ne 0) { Fail ("下载失败（curl 退出码 {0}）：{1}" -f $LASTEXITCODE, $Url) }
    $length = (Get-Item -LiteralPath $Path).Length
    if ($length -ne $Size) { Fail ("大小不符：期望 {0} B，实际 {1} B（{2}）" -f $Size, $length, $Path) }
    $actual = Get-Sha256 $Path
    if ($actual -ne $Sha) {
        Fail ("SHA-256 不符（{0}）：`n  期望 {1}`n  实际 {2}`n  文件 {3}" -f (Split-Path -Leaf $Path), $Sha, $actual, $Path)
    }
    Write-Host ("[下载并核验通过] {0}：{1} B，sha256 {2}" -f (Split-Path -Leaf $Path), $length, $actual) -ForegroundColor Green
}

# ── 前置自检 ────────────────────────────────────────────────────────────────────
if (-not (Get-Command curl.exe -ErrorAction SilentlyContinue)) {
    Fail "找不到 curl.exe（Windows 10+ 自带；若没有请手工下载两个归档到 .cache/）"
}
if (-not (Get-Command python -ErrorAction SilentlyContinue)) {
    Fail ("找不到 python（7z 解包需要 py7zr）。请先运行：python -m pip install py7zr --proxy {0}" -f $Proxy)
}
$py7zrVersion = & python -c "import py7zr;print(py7zr.__version__)" 2>&1
if ($LASTEXITCODE -ne 0) {
    Fail ("缺少 py7zr（方案 §5.5.1 第 1 步）。请先运行：python -m pip install py7zr --proxy {0}" -f $Proxy)
}
Write-Host ("[前置] python {0}；py7zr {1}" -f (& python --version), ($py7zrVersion -join " "))

New-Item -ItemType Directory -Force -Path $CacheDir | Out-Null

# ── 1) 取件 + 2) 核验 ───────────────────────────────────────────────────────────
$vcLtlArchive = Join-Path $CacheDir "VC-LTL-Binary.7z"
$yyArchive    = Join-Path $CacheDir "YY-Thunks-Objs.zip"
Ensure-Archive $vcLtlArchive $VcLtlUrl $VcLtlSha $VcLtlSize "VC-LTL5 v5.3.1"
Ensure-Archive $yyArchive    $YyUrl    $YySha    $YySize    "YY-Thunks v1.2.2 Objs"

# ── 3) 解包（每次重建，保证归位结果与归档内容一致）─────────────────────────────
$extractRoot = Join-Path $CacheDir "extract"
if (Test-Path -LiteralPath $extractRoot) { Remove-Item -Recurse -Force -LiteralPath $extractRoot }
$vcLtlExtract = Join-Path $extractRoot "vc-ltl"
$yyExtract    = Join-Path $extractRoot "yy-thunks"
New-Item -ItemType Directory -Force -Path $vcLtlExtract | Out-Null
New-Item -ItemType Directory -Force -Path $yyExtract | Out-Null

Write-Host "[解包] VC-LTL-Binary.7z -> $vcLtlExtract（py7zr）"
& python -c "import py7zr,sys;z=py7zr.SevenZipFile(sys.argv[1],'r');z.extractall(sys.argv[2]);z.close()" $vcLtlArchive $vcLtlExtract
if ($LASTEXITCODE -ne 0) { Fail "7z 解包失败：$vcLtlArchive" }

Write-Host "[解包] YY-Thunks-Objs.zip -> $yyExtract（Expand-Archive）"
Expand-Archive -LiteralPath $yyArchive -DestinationPath $yyExtract -Force

# ── 4) 归位 ─────────────────────────────────────────────────────────────────────
# VC-LTL：TargetPlatform/6.0.6000.0/lib/{x64,Win32} → assets/vc-ltl/{x64,x86}
$targetPlatform = Get-ChildItem -Recurse -Directory -LiteralPath $vcLtlExtract -Filter "TargetPlatform" |
    Select-Object -First 1
if (-not $targetPlatform) { Fail "归档内找不到 TargetPlatform/（VC-LTL 结构变化？）：$vcLtlExtract" }
$libRoot = Join-Path $targetPlatform.FullName "6.0.6000.0\lib"
if (-not (Test-Path -LiteralPath $libRoot)) { Fail "归档内找不到 6.0.6000.0/lib/：$libRoot" }

$releaseLibs = @("libucrt.lib", "libvcruntime.lib", "ucrt.lib", "vcruntime.lib")
if (Test-Path -LiteralPath $VcLtlOut) { Remove-Item -Recurse -Force -LiteralPath $VcLtlOut }
foreach ($archMap in @(@("x64", "x64"), @("Win32", "x86"))) {
    $srcDir = Join-Path $libRoot $archMap[0]
    $dstDir = Join-Path $VcLtlOut $archMap[1]
    New-Item -ItemType Directory -Force -Path $dstDir | Out-Null
    foreach ($name in $releaseLibs) {
        $src = Join-Path $srcDir $name
        if (-not (Test-Path -LiteralPath $src)) { Fail "归档内缺少 $($archMap[0])/$name（期望四个 release .lib 齐备）" }
        Copy-Item -LiteralPath $src -Destination (Join-Path $dstDir $name)
    }
    Write-Host ("[归位] {0} -> {1}（4 个 release .lib；跳过 *d.lib / .pdb）" -f $archMap[0], $dstDir) -ForegroundColor Green
}

# YY-Thunks：objs/{x64,x86}/YY_Thunks_for_Win7.obj（x86 源名无 _x86 后缀，放置端改名）
$yyObjsDir = Get-ChildItem -Recurse -Directory -LiteralPath $yyExtract |
    Where-Object { $_.Name -eq "objs" } | Select-Object -First 1
if (-not $yyObjsDir) { Fail "归档内找不到 objs/：$yyExtract" }
if (Test-Path -LiteralPath $YyOut) { Remove-Item -Recurse -Force -LiteralPath $YyOut }
New-Item -ItemType Directory -Force -Path $YyOut | Out-Null
$objPlacements = @(
    @{ Src = (Join-Path $yyObjsDir.FullName "x64\YY_Thunks_for_Win7.obj"); Dst = "YY_Thunks_for_Win7.obj" },
    @{ Src = (Join-Path $yyObjsDir.FullName "x86\YY_Thunks_for_Win7.obj"); Dst = "YY_Thunks_for_Win7_x86.obj" }
)
foreach ($item in $objPlacements) {
    if (-not (Test-Path -LiteralPath $item.Src)) {
        Fail "归档内缺少源 obj：$($item.Src)（注意：x86 源名 = YY_Thunks_for_Win7.obj，无 _x86 后缀；方案 P1-4）"
    }
    Copy-Item -LiteralPath $item.Src -Destination (Join-Path $YyOut $item.Dst)
    Write-Host ("[归位] {0} -> {1}\{2}" -f $item.Src, $YyOut, $item.Dst) -ForegroundColor Green
}

# 分析器：YY.Depends.Analyzer.exe + Config/ + objs/（三者同目录）
$analyzer = Get-ChildItem -Recurse -File -LiteralPath $yyExtract -Filter "YY.Depends.Analyzer.exe" |
    Select-Object -First 1
if (-not $analyzer) { Fail "归档内找不到 YY.Depends.Analyzer.exe：$yyExtract" }
$analyzerRoot = Split-Path -Parent $analyzer.FullName
if (Test-Path -LiteralPath $Depends) { Remove-Item -Recurse -Force -LiteralPath $Depends }
New-Item -ItemType Directory -Force -Path $Depends | Out-Null
Copy-Item -LiteralPath $analyzer.FullName -Destination $Depends
foreach ($name in @("Config", "objs")) {
    $src = Join-Path $analyzerRoot $name
    if (-not (Test-Path -LiteralPath $src)) {
        Fail "分析器同目录缺少 $name/（否则分析器静默退化 —— 指南 §4.1 坑 1）：$src"
    }
    Copy-Item -Recurse -LiteralPath $src -Destination $Depends
}
Write-Host ("[归位] 分析器三件 -> {0}（YY.Depends.Analyzer.exe + Config/ + objs/）" -f $Depends) -ForegroundColor Green

# ── 归位断言（缺件即大声失败）──────────────────────────────────────────────────
foreach ($path in @(
    (Join-Path $VcLtlOut "x64"), (Join-Path $VcLtlOut "x86"),
    (Join-Path $YyOut "YY_Thunks_for_Win7.obj"), (Join-Path $YyOut "YY_Thunks_for_Win7_x86.obj"),
    (Join-Path $Depends "YY.Depends.Analyzer.exe"), (Join-Path $Depends "Config"), (Join-Path $Depends "objs")
)) {
    if (-not (Test-Path -LiteralPath $path)) { Fail "归位校验失败，缺少：$path" }
}
$libCountX64 = @(Get-ChildItem -LiteralPath (Join-Path $VcLtlOut "x64") -File).Count
$libCountX86 = @(Get-ChildItem -LiteralPath (Join-Path $VcLtlOut "x86") -File).Count
if ($libCountX64 -ne 4) { Fail "assets/vc-ltl/x64 文件数 = $libCountX64，期望 4" }
if ($libCountX86 -ne 4) { Fail "assets/vc-ltl/x86 文件数 = $libCountX86，期望 4" }
$debugLibs = @(Get-ChildItem -Recurse -LiteralPath $VcLtlOut -File -Filter "*d.lib")
if ($debugLibs.Count -ne 0) { Fail "assets/vc-ltl 下出现调试库（必须跳过）：$(($debugLibs | ForEach-Object { $_.Name }) -join ', ')" }

# ── 归位清单（size / sha256 —— 供 THIRD_PARTY_NOTICES 与审查对账）───────────────
Write-Host ""
Write-Host "== 归位清单（size / sha256）==" -ForegroundColor Cyan
$placedFiles = @(
    Get-ChildItem -LiteralPath (Join-Path $VcLtlOut "x64") -File
    Get-ChildItem -LiteralPath (Join-Path $VcLtlOut "x86") -File
    Get-Item -LiteralPath (Join-Path $YyOut "YY_Thunks_for_Win7.obj")
    Get-Item -LiteralPath (Join-Path $YyOut "YY_Thunks_for_Win7_x86.obj")
    Get-Item -LiteralPath (Join-Path $Depends "YY.Depends.Analyzer.exe")
)
foreach ($file in $placedFiles) {
    Write-Host ("  {0,12} B  {1}  {2}" -f $file.Length, (Get-Sha256 $file.FullName), $file.FullName.Substring($Root.Length + 1))
}
$configDb = Join-Path $Depends "Config\x64\6.1.7600.txt"
if (Test-Path -LiteralPath $configDb) {
    Write-Host ("  {0,12} B  (API 自证数据库) {1}" -f (Get-Item -LiteralPath $configDb).Length, $configDb.Substring($Root.Length + 1))
}
Write-Host ""
Write-Host "完成：取件 / SHA-256 核验 / 解包 / 归位全部通过（资产不入库 —— DG14）" -ForegroundColor Green
exit 0
