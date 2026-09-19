#requires -Version 5.1
<#
b3-soak-stopper.ps1 —— soak 的**到点收尾器**（WMI 建进程 ⇒ 脱离会话；人不在也能到点收尾 + 落读数）。

⚠ **教训（2026-09-19 `14:54:41` 的 provenance 事故，P2-b 硬化而来）**：
  旧版脚本里 `$RunDir`/`$OutDir` 都有**写死的默认值**（指向某个具体轮次），且 `$OutDir` 是**跨轮共享**的
  `out/b3-soak-final/` ⇒ 两个"该已停用"的旧守时器醒来后，把**已被取代的 b3b 轮**的 summary/timeseries
  拷进了共享目录并写了 `DONE.flag`；若不查身份，就会把旧轮当终读数（与审查 P1-1 同类）。
  ⇒ 本版三处硬化：① **`-RunDir` 必填**（无默认，杜绝"默认指向旧轮"）；② **`$OutDir` 默认按 `RunDir` 派生**
  （`<RunDir 父目录>/out/<RunDir 名>-final`，不再跨轮共享）；③ **落盘前 RunDir 身份自证**：
  `summary.json` 的 `run_dir`/`proxy_pid` 必须与 `<RunDir>/CONTROLLER-PID.txt` 的记录一致，
  不一致 ⇒ **拒绝落盘并 exit 4**（不写任何文件、不写 DONE.flag）。

动作：等 -Hours 小时（或 -StopAt）→ 写 stop.flag（可 -SkipStopFlag）→ 身份自证 → 复制 summary/timeseries/
控制器日志/worker JSON → 跑 analyzer（`-SkipWarmup 1800` 双窗口径）→ 跑判据脚本 → 写 DONE.flag。
⚠ 只按 PID/stop.flag 收尾（严禁 /F /IM）。
#>
param(
    [Parameter(Mandatory = $true)][string]$RunDir,
    [string]$OutDir = "",
    [string]$StopAt = "",
    [double]$Hours = 2.0,
    [switch]$SkipStopFlag,
    [string]$Analyzer = "C:\Azusa\Code\AzusaAI-WebUI\shared\tmp\rust-proxy-p3\analyze-soak-reading.js",
    [string]$CriteriaScript = "C:\Azusa\Code\AzusaAI-WebUI\shared\tmp\rust-proxy-halfopen\scenario\b3-criteria.js"
)

$ErrorActionPreference = "Continue"
if ([string]::IsNullOrWhiteSpace($RunDir)) { Write-Host "装置失败：-RunDir 必填（本版不再有默认轮次，防跨轮误收尾）" -ForegroundColor Red; exit 2 }
$RunDir = [IO.Path]::GetFullPath($RunDir)
if (-not (Test-Path -LiteralPath $RunDir)) { Write-Host "装置失败：RunDir 不存在 $RunDir" -ForegroundColor Red; exit 2 }
if ([string]::IsNullOrWhiteSpace($OutDir)) {
    # ① 默认按 RunDir 派生（唯一派生点；不再跨轮共享）
    $OutDir = Join-Path (Join-Path (Split-Path -Parent $RunDir) "out") ((Split-Path -Leaf $RunDir) + "-final")
}
$OutDir = [IO.Path]::GetFullPath($OutDir)
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$log = Join-Path $RunDir "stopper.log"     # ② 日志也按 RunDir 落（旧的共享日志会让两轮相互混淆）

function Log([string]$m) {
    $line = "[{0}] {1}" -f (Get-Date -Format "yyyy-MM-dd HH:mm:ss"), $m
    Write-Host $line
    Add-Content -LiteralPath $log -Value $line -Encoding UTF8
}

Log "soak 收尾器启动：RunDir=$RunDir · OutDir=$OutDir · Hours=$Hours · SkipStopFlag=$($SkipStopFlag.IsPresent)"
if (-not [string]::IsNullOrWhiteSpace($StopAt)) {
    $target = [datetime]::ParseExact($StopAt, "yyyy-MM-dd HH:mm:ss", $null)
} else {
    $target = (Get-Date).AddHours($Hours)
}
while ((Get-Date) -lt $target) { Start-Sleep -Seconds 20 }

if (-not $SkipStopFlag) {
    Log "到点：写 stop.flag（若控制器已按其自身时长退出，则此文件对本次收尾是 no-op）"
    New-Item -ItemType File -Path (Join-Path $RunDir "stop.flag") -Force | Out-Null
    Start-Sleep -Seconds 75
} else {
    Log "按 -SkipStopFlag 跳过 stop.flag 与等待（只做归档）"
}

# ── ③ RunDir 身份自证（**落盘前**；不匹配 ⇒ 拒绝落盘） ──────────────────────
$pidFile = Join-Path $RunDir "CONTROLLER-PID.txt"
$summaryFile = Join-Path $RunDir "out\summary.json"
if (-not (Test-Path -LiteralPath $summaryFile)) {
    Log "⚠ 收尾中止：找不到 $summaryFile（控制器可能未走到收尾）⇒ 不写任何归档"
    exit 3
}
$recordedProxy = ""
$recordedController = ""
if (Test-Path -LiteralPath $pidFile) {
    $text = Get-Content -LiteralPath $pidFile -Raw -Encoding UTF8
    if ($text -match "proxy_pid=(\d+)") { $recordedProxy = $Matches[1] }
    if ($text -match "controller_pid=(\d+)") { $recordedController = $Matches[1] }
}
$summary = Get-Content -LiteralPath $summaryFile -Raw -Encoding UTF8 | ConvertFrom-Json
$summaryProxy = "$($summary.proxy_pid)"
$summaryRunDir = "$($summary.run_dir)"
$problems = New-Object System.Collections.Generic.List[string]
if ([string]::IsNullOrWhiteSpace($recordedProxy)) {
    $problems.Add("CONTROLLER-PID.txt 里读不到 proxy_pid（文件缺失或格式不符）")
} elseif ($summaryProxy -ne $recordedProxy) {
    $problems.Add("summary.proxy_pid=$summaryProxy 与 CONTROLLER-PID.txt 的 proxy_pid=$recordedProxy **不一致**")
}
if (-not [string]::IsNullOrWhiteSpace($summaryRunDir)) {
    $a = [IO.Path]::GetFullPath($summaryRunDir)
    $b = [IO.Path]::GetFullPath($RunDir)
    if ($a -ne $b) { $problems.Add("summary.run_dir=$a 与本次 RunDir=$b **不是同一轮**") }
}
if ($problems.Count -gt 0) {
    Log "❌ 身份自证失败 ⇒ **拒绝落盘**（不写 OutDir、不写 DONE.flag）；理由："
    foreach ($p in $problems) { Log "   · $p" }
    Log "   （这正是 2026-09-19 14:54 provenance 事故要防的情形：跨轮/跨 OutDir 的误收尾）"
    exit 4
}
Log "身份自证通过：run_dir=$summaryRunDir · proxy_pid=$summaryProxy（与 CONTROLLER-PID.txt 一致）"

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
foreach ($f in @("summary.json", "timeseries.csv", "soak-controller.log", "startup-smoke.txt", "proxy-stderr.log")) {
    $src = Join-Path $RunDir ("out\" + $f)
    if (Test-Path -LiteralPath $src) { Copy-Item -LiteralPath $src -Destination (Join-Path $OutDir $f) -Force }
}
if (Test-Path -LiteralPath $pidFile) { Copy-Item -LiteralPath $pidFile -Destination $OutDir -Force }
$workerOut = Join-Path $OutDir "workers"
New-Item -ItemType Directory -Force -Path $workerOut | Out-Null
Get-ChildItem -LiteralPath (Join-Path $RunDir "workers") -Filter "worker-*.json" -File -ErrorAction SilentlyContinue |
    ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination $workerOut -Force }

if (Test-Path -LiteralPath $Analyzer) {
    & node $Analyzer (Join-Path $OutDir "timeseries.csv") 0 -SkipWarmup 1800 -Out (Join-Path $OutDir "analysis-skip1800.txt") 2>&1 |
        Out-File -LiteralPath (Join-Path $OutDir "analysis-stdout.txt") -Encoding UTF8
    & node $Analyzer (Join-Path $OutDir "timeseries.csv") 0 -Out (Join-Path $OutDir "analysis-full.txt") 2>&1 | Out-Null
    Log "analysis（双窗）已落盘"
}
if (Test-Path -LiteralPath $CriteriaScript) {
    & node $CriteriaScript $RunDir $OutDir 2>&1 | Out-File -LiteralPath (Join-Path $OutDir "criteria-stdout.txt") -Encoding UTF8
    Log "criteria.txt 已落盘（十条判据）"
}
$left = Get-Process -Name "azusa-local-proxy", "python" -ErrorAction SilentlyContinue
Log ("收尾后残留进程（azusa/python）= " + @($left).Count)
"收尾完成：$(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')（RunDir=$RunDir；身份自证通过 proxy_pid=$summaryProxy）" |
    Set-Content -LiteralPath (Join-Path $OutDir "DONE.flag") -Encoding UTF8
Log "DONE"
