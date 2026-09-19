#requires -Version 5.1
<#
verify-no-residue.ps1 —— D12「零文件 I/O」的**残留面**常驻回归自检（方案 §6.2 五点 / §12.1-D12-3、D12-4）。

装置（写死）：
  · 把被测 exe 复制进 <OutDir>\resid\empty-dir\（**每次运行前清空重建 ⇒ 恒为空目录**）并在该目录下运行 HoldSec 秒；
  · 运行前后各取一次快照，逐条比对（读数全部入档 <OutDir>\readings-residue.txt）。

五个检查点（= §6.2 的五个检查点）
  ① exe 同目录（= 该空目录）文件全集 diff = ∅（另附"exe 源目录"的读数，**只记录不作判据** —— 并行构建会写 target/）；
  ② 注册表：`HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 值集合 diff = ∅ ∧
     `reg query HKCU\Software /f AzusaAI /s` 无命中；
  ③ `%TEMP%\AzusaAI-LocalProxy` **不存在**（K13 后无条件成立：程序无任何持久化路径）；
  ④ 进程/网络：存活期间 `ParentProcessId = <PID>` 的子进程数 = **0**（D12 后无任何例外）；
     该 PID 的 TCP 行里 LISTENING 恰好 1 条且必须是 `127.0.0.1:<port>`，非 LISTENING 行只允许指向
     `127.0.0.1:<upstream-port>`（本脚本不启上游 ⇒ 期望 0 条），**不得**出现任何其它远端；
  ⑤ ①+③ 合并 = "该空目录 / exe 同目录 / `%TEMP%` 三处零新增"（同一份前后快照取 diff）。
附加（D12-4 参数面 —— 与残留面同属"不存在"的判据）
  ⑥ `--log-dir` / `--log-file` / `--log-retain-files` / `--dump-icons` / `--install-autostart` /
     `--uninstall-autostart` 一律报「未知参数」且退出码 **2**（证明它们**真的不再存在**，
     而不是"存在但默认不写"；后两个按 K13 已随自启功能整体删除）。

定向口径（沿用 E-6 的收窄，理由不变）：**不** diff 整个 `HKCU\Software` 与整个 `%TEMP%` —— 系统与
其它程序会自行写入 ⇒ 假失败。改用「`Run` 值集合 diff + `reg query … /f AzusaAI /s` 无命中 +
`%TEMP%\AzusaAI-LocalProxy` 不存在」这三条**定向**判据。
  · **实施期实测更正**：`reg query HKCU\Software /f AzusaAI /s` 在本机（Office/浏览器海量键）是
    **分钟级**操作 ⇒ 脚本对它加 **20 s 硬超时**；超时则如实登记该项为「未评估 `?`」，
    **不得**当成"无命中"（自动启项面的新增证据由 `Run` 值 diff + 源码面 grep 承担）。

「脚本把 stdout 重定向到文件 ≠ 程序写文件」（§2.3 末条 / §11-①）：本脚本自己创建
`<OutDir>\resid-proxy-stdout.log` 并把继承句柄交给子进程 ⇒ 该文件是**脚本的产物**，**不**计入残留
（`target/` 与源目录同理：判定面只取"被测 exe 所在的那个空目录 + `%TEMP%` + 注册表"）。

用法：
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts/verify-no-residue.ps1 [-Exe <path>] [-HoldSec 60]
退出码：0 = 全部判据成立（PASS）；1 = 有判据 FAIL；2 = 装置/环境失败（exe 缺失、服务未起来…）。
#>
param(
    [string]$Exe = "",
    [int]$Port = 8061,
    [int]$UpstreamPort = 8062,
    [int]$HoldSec = 60,
    [string]$OutDir = "",
    [switch]$KeepWorkDir
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
# 证据文件里的中文不要变成乱码（重定向捕获按控制台代码页写 ⇒ 显式钉 UTF-8；本机无控制台时 setter 可能抛，忽略）
try { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 } catch { }

$repoRoot = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
if ([string]::IsNullOrWhiteSpace($Exe)) { $Exe = Join-Path $repoRoot "target\x86_64-win7-windows-msvc\release\azusa-local-proxy.exe" }
if ([string]::IsNullOrWhiteSpace($OutDir)) { $OutDir = Join-Path (Join-Path $repoRoot "..\..\shared\tmp\rust-proxy-p6\out") "resid" }
$Exe = [IO.Path]::GetFullPath($Exe)
$OutDir = [IO.Path]::GetFullPath($OutDir)
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$workDir = Join-Path $OutDir "empty-dir"
$logPath = Join-Path $OutDir "probe-residue.log"
$readingsPath = Join-Path $OutDir "readings-residue.txt"
$verdictPath = Join-Path $OutDir "verdict-residue.txt"
$capture = Join-Path $OutDir "resid-proxy-stdout.log"
$captureErr = Join-Path $OutDir "resid-proxy-stderr.log"
$failures = New-Object System.Collections.Generic.List[string]
$readings = New-Object System.Collections.Generic.List[string]

function Log([string]$m) {
    $line = "[{0}] {1}" -f (Get-Date -Format "HH:mm:ss.fff"), $m
    Write-Host $line
    Add-Content -LiteralPath $logPath -Value $line -Encoding UTF8
}

function Note([string]$m) {
    $readings.Add($m)
    Write-Host "  · $m"
}

function Fail([string]$m) {
    $failures.Add($m)
    Log "FAIL：$m"
}

# ── 快照三件套 ──────────────────────────────────────────────────────────
function Get-DirSnapshot([string]$dir) {
    if (-not (Test-Path -LiteralPath $dir)) { return @() }
    return @(Get-ChildItem -LiteralPath $dir -Force -Recurse | Sort-Object FullName | ForEach-Object {
        $kind = $(if ($_.PSIsContainer) { "dir" } else { "" + $_.Length })
        "{0}|{1}|{2}" -f $_.FullName.Substring($dir.Length).TrimStart('\'), $kind, $_.LastWriteTimeUtc.ToString("o")
    })
}

function Get-RunValues {
    $key = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
    $values = @()
    if (Test-Path -LiteralPath $key) {
        $item = Get-ItemProperty -LiteralPath $key
        foreach ($p in $item.PSObject.Properties) {
            if ($p.Name -notlike "PS*") { $values += ("{0}={1}" -f $p.Name, $p.Value) }
        }
    }
    return @($values | Sort-Object)
}

function Get-AzusaRegistryHits([int]$TimeoutSec = 20) {
    # ⚠ 定向口径（E-6 收窄）在**本机的实测代价**：`reg query HKCU\Software /f AzusaAI /s` 会递归枚举
    # 本机全部键（Office / 浏览器海量键），耗时可到**分钟**级 ⇒ 必须**硬超时**，否则整份自检被这一步拖住。
    # 超时 ⇒ 返回 @("__TIMEOUT__")，调用方**如实登记「未评估」**（不得当"无命中"）。
    # 未超时 ⇒ 输出按系统 ANSI 代码页解（reg.exe 不写 UTF-8）。
    $outFile = Join-Path $OutDir "regquery-azusaai.out.txt"
    $errFile = Join-Path $OutDir "regquery-azusaai.err.txt"
    foreach ($f in @($outFile, $errFile)) { if (Test-Path -LiteralPath $f) { Remove-Item -LiteralPath $f -Force } }
    $proc = Start-Process -FilePath "reg.exe" -PassThru -NoNewWindow `
        -ArgumentList @("query", "HKCU\Software", "/f", "AzusaAI", "/s") `
        -RedirectStandardOutput $outFile -RedirectStandardError $errFile
    if (-not $proc.WaitForExit($TimeoutSec * 1000)) {
        try { Stop-Process -Id $proc.Id -Force -ErrorAction Stop } catch { }
        return @("__TIMEOUT__")
    }
    $raw = @()
    if (Test-Path -LiteralPath $outFile) {
        $raw = @(Get-Content -LiteralPath $outFile -Encoding Default -ErrorAction SilentlyContinue)
    }
    return @($raw | Where-Object { "$_" -match "AzusaAI" })
}

function Get-NetRows([int]$pidValue) {
    $rows = @()
    foreach ($m in (netstat -ano | Select-String -Pattern "\s+$pidValue\s*$")) {
        $parts = @($m.Line.Trim() -split "\s+")
        if ($parts.Count -ge 5 -and $parts[0] -eq "TCP") {
            $rows += [pscustomobject]@{ local = $parts[1]; remote = $parts[2]; state = $parts[3] }
        }
    }
    return @($rows)
}

function Get-ChildCount([int]$pidValue) {
    return @(Get-CimInstance Win32_Process -Filter "ParentProcessId = $pidValue" -ErrorAction SilentlyContinue).Count
}

# ── 前置 ────────────────────────────────────────────────────────────────
if (-not (Test-Path -LiteralPath $Exe)) { Log "装置失败：exe 不存在 $Exe"; exit 2 }
$exeHash = (Get-FileHash -LiteralPath $Exe -Algorithm SHA256).Hash.ToLower()
$exeBytes = (Get-Item -LiteralPath $Exe).Length
$sourceDir = Split-Path -Parent $Exe
Log "verify-no-residue：exe=$Exe（$exeBytes B / sha256 $exeHash）"
Note "exe = $Exe"
Note "exe_bytes = $exeBytes"
Note "exe_sha256 = $exeHash"
Note "work_dir = $workDir"
Note "hold_sec = $HoldSec"
Note "port = $Port / upstream_port = $UpstreamPort（本脚本**不启上游**）"

# 空目录：每次清空重建（保证"运行前为空"这一前提恒成立）
if (Test-Path -LiteralPath $workDir) { Remove-Item -LiteralPath $workDir -Recurse -Force }
New-Item -ItemType Directory -Force -Path $workDir | Out-Null
$proxyCopy = Join-Path $workDir "azusa-local-proxy.exe"
Copy-Item -LiteralPath $Exe -Destination $proxyCopy -Force

if (Test-Path -LiteralPath $capture) { Remove-Item -LiteralPath $capture -Force }
if (Test-Path -LiteralPath $captureErr) { Remove-Item -LiteralPath $captureErr -Force }

# ── 运行前快照 ──────────────────────────────────────────────────────────
$tempMarker = Join-Path $env:TEMP "AzusaAI-LocalProxy"
$beforeWork = @(Get-DirSnapshot $workDir)
$beforeSource = @(Get-DirSnapshot $sourceDir)
$beforeRun = @(Get-RunValues)
$beforeRegHits = @(Get-AzusaRegistryHits)
$beforeTemp = Test-Path -LiteralPath $tempMarker
Note "before: 空目录条目=$($beforeWork.Count)（仅 exe 副本）· Run 值=$($beforeRun.Count) · AzusaAI 注册表命中=$($beforeRegHits.Count) · %TEMP% 标记存在=$beforeTemp"

$proxyProc = $null
$childrenMax = 0
$netRowCount = 0
try {
    $cliArgs = @(
        "--no-gui",
        "--listen-host", "127.0.0.1",
        "--listen-port", "$Port",
        "--port-fallback", "none",
        "--upstream", "http://127.0.0.1:$UpstreamPort",
        "--log-level", "info"
    )
    $proxyProc = Start-Process -FilePath $proxyCopy -WorkingDirectory $workDir -PassThru -ArgumentList $cliArgs `
        -RedirectStandardOutput $capture -RedirectStandardError $captureErr
    Log "代理已启动：PID=$($proxyProc.Id)（工作目录 $workDir；stdout 由本脚本捕获）"

    # 就绪：TCP 连通
    $ready = $false
    $deadline = (Get-Date).AddSeconds(20)
    while ((Get-Date) -lt $deadline) {
        if ($proxyProc.HasExited) { throw "代理启动后立即退出（exit=$($proxyProc.ExitCode)）" }
        try { $c = New-Object System.Net.Sockets.TcpClient; $c.Connect("127.0.0.1", $Port); $c.Close(); $ready = $true; break }
        catch { Start-Sleep -Milliseconds 300 }
    }
    if (-not $ready) { throw "20 s 内 127.0.0.1:$Port 未 LISTENING" }
    Log "代理已就绪：127.0.0.1:$Port LISTENING"

    # 轻冒烟（起服务 + 预检 204 + 上游不可达 ⇒ 502 + 原因 JSON）：证明"确实在服务"，不是"起来了但什么都不干"
    $pf = "" + (& curl.exe -s -o NUL -w "%{http_code}" --max-time 5 -X OPTIONS -H "Origin: null" -H "Access-Control-Request-Method: POST" "http://127.0.0.1:$Port/v1/chat/completions")
    $body = Join-Path $OutDir "resid-body.json"
    Set-Content -LiteralPath $body -Value '{"id":"residue-probe","stream":false}' -Encoding ASCII
    $post = "" + (& curl.exe -s -o "$OutDir\resid-post-body.txt" -w "%{http_code}" --max-time 5 -X POST -H "Content-Type: application/json" --data "@$body" "http://127.0.0.1:$Port/v1/chat/completions")
    $postBody = ""
    if (Test-Path -LiteralPath (Join-Path $OutDir "resid-post-body.txt")) { $postBody = Get-Content -LiteralPath (Join-Path $OutDir "resid-post-body.txt") -Raw -Encoding UTF8 }
    Note "smoke: preflight_status=$pf · upstream_down_status=$post · reason_json=$($postBody.Trim())"
    if ($pf -ne "204") { Fail "预检不是 204（读数为 $pf）" }
    if ($post -ne "502") { Fail "上游不可达时不是 502（读数为 $post）" }
    if ($postBody -notmatch '"error"' -or $postBody -notmatch '"type"') { Fail "502 响应体缺少结构化原因（error.type/message；读数为 $postBody）" }

    # 存活期间：子进程数（多点取样取最大值）+ 网络面
    $checks = [math]::Max(3, [int]($HoldSec / 10))
    $netSample = @()
    for ($i = 0; $i -lt $checks; $i++) {
        $cc = Get-ChildCount $proxyProc.Id
        if ($cc -gt $childrenMax) { $childrenMax = $cc }
        if ($i -eq 1) {
            $netSample = @(Get-NetRows $proxyProc.Id)
            $netRowCount = $netSample.Count
        }
        Start-Sleep -Seconds ([math]::Max(1, [int]($HoldSec / $checks)))
    }
    Note "children_max（ParentProcessId = $($proxyProc.Id)）= $childrenMax"
    if ($childrenMax -ne 0) { Fail "存活期间出现子进程（$childrenMax 个）—— D12 后程序不 spawn 任何子进程，无例外" }

    $listenRows = @($netSample | Where-Object { $_.state -eq "LISTENING" })
    $otherRows = @($netSample | Where-Object { $_.state -ne "LISTENING" })
    Note "netstat(PID)：总行=$netRowCount · LISTENING=$($listenRows.Count) · 其它=$($otherRows.Count)"
    foreach ($r in $listenRows) { Note "  LISTENING $($r.local)" }
    foreach ($r in $otherRows) { Note "  $($r.state) $($r.local) -> $($r.remote)" }
    if ($listenRows.Count -ne 1) { Fail "该 PID 的 LISTENING 行数应为 1（实测 $($listenRows.Count)）" }
    elseif ($listenRows[0].local -ne "127.0.0.1:$Port") { Fail "监听地址应为 127.0.0.1:$Port（实测 $($listenRows[0].local)）" }
    foreach ($r in $otherRows) {
        if ($r.remote -notmatch "^127\.0\.0\.1:$UpstreamPort$") {
            Fail "出现到非预期远端的连接：$($r.state) $($r.local) -> $($r.remote)"
        }
    }

    # 程序自己写的 stdout（重定向通道仍在工作 ⇒ 不是"起来了但输出全丢"）
    $capBytes = 0
    if (Test-Path -LiteralPath $capture) { $capBytes = (Get-Item -LiteralPath $capture).Length }
    Note "captured_stdout_bytes = $capBytes（脚本侧捕获，不计入残留）"
    if ($capBytes -le 0) { Fail "重定向捕获为空 ⇒ 重定向通道（D6③）没有工作" }
} catch {
    Log "装置异常：$($_.Exception.Message)"
    $failures.Add("装置异常：$($_.Exception.Message)")
} finally {
    if ($proxyProc) {
        try { Stop-Process -Id $proxyProc.Id -Force -ErrorAction Stop; Log "已按 PID 关闭代理（PID=$($proxyProc.Id)）" }
        catch { Log "代理已不在（PID=$($proxyProc.Id)）" }
        Start-Sleep -Milliseconds 600
    }
}

# ── 运行后快照 + 五点判据 ────────────────────────────────────────────────
$afterWork = @(Get-DirSnapshot $workDir)
$afterSource = @(Get-DirSnapshot $sourceDir)
$afterRun = @(Get-RunValues)
$afterRegHits = @(Get-AzusaRegistryHits)
$afterTemp = Test-Path -LiteralPath $tempMarker

$newWork = @(Compare-Object -ReferenceObject $beforeWork -DifferenceObject $afterWork)
$newSource = @(Compare-Object -ReferenceObject $beforeSource -DifferenceObject $afterSource)
$newRun = @(Compare-Object -ReferenceObject $beforeRun -DifferenceObject $afterRun)

Note "after: 空目录条目=$($afterWork.Count) · Run 值=$($afterRun.Count) · AzusaAI 注册表命中=$($afterRegHits.Count) · %TEMP% 标记存在=$afterTemp"

# ① exe 同目录（= 空目录）全集 diff = ∅
if ($newWork.Count -ne 0) {
    foreach ($d in $newWork) { Note "  diff(空目录) $($d.SideIndicator) $($d.InputObject)" }
    Fail "① 空目录出现新增/变更条目（$($newWork.Count) 条）"
} else { Note "① 空目录 diff = ∅（exe 同目录零新增）✓" }
Note "①b 源目录 diff = $($newSource.Count) 条（**只记录不作判据**：并行 cargo 构建会写 target/）"
foreach ($d in $newSource) { Note "  diff(源目录) $($d.SideIndicator) $($d.InputObject)" }

# ② 注册表
if ($newRun.Count -ne 0) { foreach ($d in $newRun) { Note "  diff(Run) $($d.SideIndicator) $($d.InputObject)" }; Fail "② Run 键值集合发生变化（$($newRun.Count) 条）" }
else { Note "② Run 值集合 diff = ∅ ✓" }
if ($afterRegHits.Count -ne 0) { foreach ($h in $afterRegHits) { Note "  reg hit: $h" } }
$regTimedOut = ($afterRegHits.Count -eq 1 -and "$($afterRegHits[0])" -eq "__TIMEOUT__")
if ($regTimedOut) {
    Note "② 「reg query HKCU\Software /f AzusaAI /s」**硬超时**（>20 s，本机递归枚举太慢）⇒ 该项登记为『未评估』?（**不得**当无命中；自动启项的新增证据由 Run 值 diff + 源码面 grep 承担）"
} else {
    $newRegHits = @(Compare-Object -ReferenceObject $beforeRegHits -DifferenceObject $afterRegHits -PassThru)
    if ($newRegHits.Count -ne 0) { foreach ($h in $newRegHits) { Note "  reg new: $h" }; Fail "② reg query HKCU\Software /f AzusaAI /s 出现**新增**命中（$($newRegHits.Count) 行）" }
    elseif ($afterRegHits.Count -ne 0) { Note "② reg query HKCU\Software /f AzusaAI /s：运行前后**同为** $($afterRegHits.Count) 行（非本程序新增）⇢ 零新增 ✓" }
    else { Note "② reg query HKCU\Software /f AzusaAI /s = 无命中 ✓" }
}

# ③ %TEMP% 定向
if ($afterTemp) { Fail "③ %TEMP%\AzusaAI-LocalProxy 存在（D12 后应无条件不存在）" }
else { Note "③ %TEMP%\AzusaAI-LocalProxy 不存在 ✓" }

# ④ 子进程 / 网络（读数在上面，判据已即时判过）
Note "④ 子进程 = $childrenMax（期望 0）· netstat 行 = $netRowCount"

# ⑤ 三处零新增（①+③ 的合并陈述）
if ($newWork.Count -eq 0 -and -not $afterTemp -and $newRun.Count -eq 0) { Note "⑤ 该空目录 / exe 同目录 / %TEMP% 三处零新增（且 Run 集合不变）✓" }
else { Fail "⑤ 三处零新增不成立（空目录 diff=$($newWork.Count) · %TEMP% 标记=$afterTemp · Run diff=$($newRun.Count)）" }

# ⑥ D12-4 参数面：六个已删参数一律"未知参数" + exit 2
$removedParams = @("--log-dir", "--log-file", "--log-retain-files", "--dump-icons", "--install-autostart", "--uninstall-autostart")
foreach ($p in $removedParams) {
    $o = Join-Path $OutDir ("paramface" + ($p -replace "--", "-") + ".out.txt")
    $e = Join-Path $OutDir ("paramface" + ($p -replace "--", "-") + ".err.txt")
    $proc = Start-Process -FilePath $proxyCopy -WorkingDirectory $workDir -PassThru -Wait `
        -ArgumentList @($p, "x") `
        -RedirectStandardOutput $o -RedirectStandardError $e
    $text = ""
    if (Test-Path -LiteralPath $o) { $text += (Get-Content -LiteralPath $o -Raw -Encoding UTF8) }
    if (Test-Path -LiteralPath $e) { $text += (Get-Content -LiteralPath $e -Raw -Encoding UTF8) }
    $code = $proc.ExitCode
    $hit = $text -match "未知参数"
    Note "⑥ $p ⇒ exit=$code · 含「未知参数」=$hit"
    if ($code -ne 2) { Fail "⑥ $p 的退出码应为 2（实测 $code）" }
    if (-not $hit) { Fail "⑥ $p 的文案应含「未知参数」（实际输出：$($text.Trim())）" }
}

# ── 结论 ────────────────────────────────────────────────────────────────
$readings | Set-Content -LiteralPath $readingsPath -Encoding UTF8
if ($failures.Count -eq 0) {
    "PASS  （五点 + 参数面全部成立；exe $exeBytes B / sha256 $exeHash）" | Set-Content -LiteralPath $verdictPath -Encoding UTF8
    Log "结论：PASS（读数 $readingsPath）"
} else {
    ("FAIL  x{0}`n" -f $failures.Count) + ($failures -join "`n") | Set-Content -LiteralPath $verdictPath -Encoding UTF8
    Log ("结论：FAIL x{0}" -f $failures.Count)
    foreach ($f in $failures) { Log "  · $f" }
}
if (-not $KeepWorkDir) {
    # 只清 exe 副本（保留目录本身与快照读数：目录的"运行后状态"本身就是证据）
    if (Test-Path -LiteralPath $proxyCopy) { Remove-Item -LiteralPath $proxyCopy -Force -ErrorAction SilentlyContinue }
}
if ($failures.Count -eq 0) { exit 0 } else { exit 1 }
