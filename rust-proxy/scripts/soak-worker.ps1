#requires -Version 5.1
<#
soak-worker.ps1 —— soak 的**客户端负载**子进程（由 soak.ps1 拉起；不单独使用）。

三种模式（方案 §10.3 S1 的混合负载 + §6.5 FR-18/FR-19 的长连接与中止）：
  mixed —— 非流式 POST 为主 + 预检 OPTIONS（10%）+ `Content-Type: text/plain`（10%，FR-39 节流观察）
           + 每 3 个请求有 1 个由 mock 上游回**冲突 CORS 头**（验 FR-11 "替换而非追加"）
  sse   —— SSE 逐帧读数（帧间隔、总时长；每 5 轮一次 50 s 长流，覆盖 FR-18 "长流不被超时掐断"）
  abort —— 开流 → 读 2 帧 → 中途断开（FR-19：不得留孤儿上游连接）

计数口径：只统计**本进程**发出的请求与结果；`ok`/`err`/`exc` 三分，`abort`/`badacao`/`conflict`
单列。每 2 s 覆写 `$OutDir/worker-<name>.json`（小文件，供采样端读取），事件文本另存
`worker-<name>.events.log`（只记异常/异常读数，防止 24 h 写出几个 G）。
#>
param(
    [Parameter(Mandatory = $true)][ValidateSet("mixed", "sse", "abort")][string]$Mode,
    [int]$ProxyPort = 8010,
    [Parameter(Mandatory = $true)][string]$OutDir,
    [string]$Name = "",
    [int]$IntervalMs = 500,
    [string]$LifetimeFlag = "",
    [int]$MaxSeconds = 93600
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Net.Http   # PS 5.1 默认不加载 ⇒ HttpClient 系列要显式引
[System.Net.ServicePointManager]::DefaultConnectionLimit = 64
[System.Net.ServicePointManager]::Expect100Continue = $false

if ([string]::IsNullOrWhiteSpace($Name)) { $Name = "$Mode-$PID" }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$base = "http://127.0.0.1:$ProxyPort"
$eventsPath = Join-Path $OutDir "worker-$Name.events.log"
$counterPath = Join-Path $OutDir "worker-$Name.json"

$rand = New-Object System.Random
$counters = [ordered]@{
    mode = $Mode; name = $Name; pid = $PID; started = (Get-Date).ToString("s")
    sent = 0; ok = 0; err = 0; exc = 0; abort = 0; badacao = 0; conflict = 0
    preflight204 = 0; plain_ok = 0; idmismatch = 0
    sse_streams = 0; sse_frames = 0; sse_broken = 0; sse_stalled = 0; gap_min_ms = -1; gap_max_ms = -1; stream_secs_max = 0
    worst_nonstream_ms = 0; worst_nonstream_at = ""; count_over_8s = 0; count_nonstream = 0
    last_status = ""; last_error = ""
}

function Log-Event([string]$m) {
    $line = "[{0}] {1}" -f (Get-Date -Format "HH:mm:ss"), $m
    try {
        $size = 0
        if (Test-Path -LiteralPath $eventsPath) { $size = (Get-Item -LiteralPath $eventsPath).Length }
        if ($size -gt 4MB) { return }   # 事件日志上限：防 24 h 写出巨文件
        Add-Content -LiteralPath $eventsPath -Value $line -Encoding UTF8
    } catch { }
}

function Save-Counters {
    try {
        $counters.last_save = (Get-Date).ToString("s")
        ($counters | ConvertTo-Json -Compress) | Set-Content -LiteralPath $counterPath -Encoding UTF8
    } catch { }
}

$client = New-Object System.Net.Http.HttpClient
# ⚠ 客户端必须有**请求级兜底**：代理侧一旦出现"挂死的转发请求"（Phase 3 已登记 P1：半开池连接复用
# ⇒ 请求永久不回），无限超时会让整个 worker 卡死到轮末（实测踩过）⇒ 24 h soak 会退化成空转。
$client.Timeout = [System.TimeSpan]::FromSeconds(120)

function New-JsonContent([string]$json) {
    $content = New-Object System.Net.Http.StringContent($json, [System.Text.Encoding]::UTF8)
    $content.Headers.ContentType = New-Object System.Net.Http.Headers.MediaTypeHeaderValue("application/json")
    return $content
}

# 头取值：**必须**用 TryGetValues（`GetValues` 在头缺失时抛异常 —— 实测踩过，2026-09-19）
function Get-HeaderValues($resp, [string]$name) {
    $vals = $null
    # ⚠ 必须 `,` 前缀返回：PS 会把 `return @(...)` 单元素数组**解包**成标量 ⇒ StrictMode 下
    # 调用方的 `$x.Count` 直接报"找不到属性 Count"（实测踩过，2026-09-19）。
    if ($resp.Headers.TryGetValues($name, [ref]$vals)) { return , @($vals) }
    return , @()
}

# CORS 断言（FR-11）：预检响应 = 本地生成（带 Allow-Methods/Allow-Headers/Max-Age/PNA）；
# 实际响应 = 上游同名族必须**被替换/剥离**（恰好 1 个 ACAO 且值 = 代理计算值；不得残留 Allow-Methods）。
# `$kind` = "preflight" | "actual"（由调用方按请求形态给定 —— 判定式同 FR-9）。
function Check-Cors($resp, [string]$tag, [string]$kind = "actual") {
    $origins = Get-HeaderValues $resp "Access-Control-Allow-Origin"
    if ($origins.Count -ne 1) {
        $counters.badacao++
        Log-Event "$tag ACAO 头计数=$($origins.Count)（应恰好 1 个：追加就会产出两个 ⇒ 浏览器必拒，FR-11）"
    } elseif ($origins[0] -ne "*") {
        $counters.badacao++
        Log-Event "$tag ACAO=[$($origins[0])]（应为 * —— 上游值未被替换，FR-11）"
    }
    $methods = Get-HeaderValues $resp "Access-Control-Allow-Methods"
    if ($kind -eq "actual") {
        if ($methods.Count -gt 0) {
            $counters.badacao++
            Log-Event "$tag 实际响应仍带 Access-Control-Allow-Methods=[$($methods -join ',')]（同名族未剥离，FR-11）"
        }
    } else {
        if ($methods.Count -ne 1 -or $methods[0] -notmatch "POST") {
            $counters.badacao++
            Log-Event "$tag 预检响应缺 Allow-Methods（或值异常）：[$($methods -join ',')]"
        }
        $maxAge = Get-HeaderValues $resp "Access-Control-Max-Age"
        if ($maxAge.Count -eq 0) { $counters.badacao++; Log-Event "$tag 预检响应缺 Max-Age" }
    }
}

function Do-Preflight {
    $req = New-Object System.Net.Http.HttpRequestMessage([System.Net.Http.HttpMethod]::Options, "$base/v1/chat/completions")
    $req.Headers.Add("Origin", "http://localhost:9000")
    $req.Headers.Add("Access-Control-Request-Method", "POST")
    $req.Headers.Add("Access-Control-Request-Headers", "authorization, content-type")
    $req.Headers.Add("Access-Control-Request-Private-Network", "true")
    try {
        $resp = $client.SendAsync($req).Result
        $counters.sent++
        $status = [int]$resp.StatusCode
        $counters.last_status = "preflight:$status"
        if ($status -eq 204) { $counters.preflight204++; $counters.ok++ } else { $counters.err++ }
        Check-Cors $resp "preflight" "preflight"
        $pna = Get-HeaderValues $resp "Access-Control-Allow-Private-Network"
        if ($pna.Count -eq 0) { $counters.badacao++; Log-Event "预检缺 Access-Control-Allow-Private-Network（FR-13）" }
        $resp.Dispose()
    } catch {
        $counters.exc++; $counters.last_error = $_.Exception.Message; Log-Event "preflight 异常：$($_.Exception.GetType().Name)：$($_.Exception.Message)"
    } finally { $req.Dispose() }
}

function Do-Post([bool]$plain, [bool]$stream, [int]$nFrames = 8) {
    $id = [Guid]::NewGuid().ToString("N").Substring(0, 8)
    $ct = if ($plain) { "text/plain" } else { "application/json" }
    $payload = '{"model":"soak","id":"' + $id + '","stream":' + $(if ($stream) { "true" } else { "false" }) + ',"n":' + $nFrames + ',"messages":[{"role":"user","content":"soak"}]}'
    $req = New-Object System.Net.Http.HttpRequestMessage([System.Net.Http.HttpMethod]::Post, "$base/v1/chat/completions")
    $req.Content = New-Object System.Net.Http.StringContent($payload, [System.Text.Encoding]::UTF8)
    $req.Content.Headers.ContentType = New-Object System.Net.Http.Headers.MediaTypeHeaderValue($ct)
    $req.Headers.Add("X-Soak-Id", $id)
    $t0 = Get-Date
    try {
        $resp = $client.SendAsync($req, [System.Net.Http.HttpCompletionOption]::ResponseHeadersRead).Result
        $counters.sent++
        $status = [int]$resp.StatusCode
        $counters.last_status = "$status"
        if ($status -ge 200 -and $status -lt 400) { $counters.ok++; if ($plain) { $counters.plain_ok++ } } else { $counters.err++ }
        Check-Cors $resp "post" "actual"
        if ($stream -and $status -eq 200) {
            Read-Stream $resp $id
        } else {
            $text = $resp.Content.ReadAsStringAsync().Result
            # 非流式请求的**端到端耗时**（判据 C / §6.5-3：worst_nonstream_ms ≤ V_soak + 5000）
            if (-not $stream) {
                $ms = [int]((Get-Date) - $t0).TotalMilliseconds
                $counters.count_nonstream++
                if ($ms -gt 8000) { $counters.count_over_8s++ }
                if ($ms -gt $counters.worst_nonstream_ms) {
                    $counters.worst_nonstream_ms = $ms
                    $counters.worst_nonstream_at = (Get-Date).ToString("HH:mm:ss")
                    if ($ms -gt 8000) { Log-Event "非流式请求耗时 ${ms}ms（>8s ⇒ 疑似挂死；判据 C 阈值）" }
                }
            }
            if ($text -match '"cf":1') { $counters.conflict++ }
            if (-not $plain -and $status -eq 200 -and $text -notmatch [regex]::Escape($id)) {
                $counters.idmismatch++
                Log-Event "响应未回显请求 id=$id（疑似串包）：$($text.Substring(0, [Math]::Min(120, $text.Length)))"
            }
        }
        $resp.Dispose()
    } catch {
        $counters.exc++; $counters.last_error = $_.Exception.Message; Log-Event "post 异常：$($_.Exception.GetType().Name)：$($_.Exception.Message)"
    } finally { $req.Dispose() }
}

# SSE 读数：逐帧到达时刻 → 帧间隔；`$AbortAfterFrames > 0` 时读到该帧数即**中途断开**（FR-19）
function Read-Stream($resp, [string]$id, [int]$AbortAfterFrames = 0) {
    $counters.sse_streams++
    $stream = $resp.Content.ReadAsStreamAsync().Result   # .NET Framework 无同步 ReadAsStream（实测踩过）
    $stream.ReadTimeout = 25000                          # 帧级看门狗：上游死了但连接未关 ⇒ ReadLine 会永久阻塞
    $reader = New-Object System.IO.StreamReader($stream)
    $first = $null; $prev = $null; $frames = 0; $start = Get-Date
    $seenId = $false; $cfCounted = $false
    try {
        while ($true) {
            $line = $reader.ReadLine()
            if ($null -eq $line) { break }
            if ($line -notlike "data:*") { continue }
            $frames++
            $now = Get-Date
            if ($null -eq $first) { $first = $now } else {
                $gap = [int]($now - $prev).TotalMilliseconds
                if ($counters.gap_min_ms -lt 0 -or $gap -lt $counters.gap_min_ms) { $counters.gap_min_ms = $gap }
                if ($gap -gt $counters.gap_max_ms) { $counters.gap_max_ms = $gap }
            }
            $prev = $now
            if ($line -match [regex]::Escape($id)) { $seenId = $true }
            if (-not $cfCounted -and $line -match '"cf":1') { $cfCounted = $true; $counters.conflict++ }
            if ($AbortAfterFrames -gt 0 -and $frames -ge $AbortAfterFrames) {
                $counters.abort++
                Log-Event "abort：读满 $frames 帧即断开（id=$id）"
                $reader.Dispose(); $stream.Dispose()
                $counters.sse_frames += $frames
                return
            }
        }
    } catch {
        $counters.sse_broken++
        if ($_.Exception.Message -match "超时|timed out|Timeout") { $counters.sse_stalled++ }
        Log-Event "流中断：$($_.Exception.GetType().Name)：$($_.Exception.Message)"
    } finally {
        try { $reader.Dispose() } catch { }
        try { $stream.Dispose() } catch { }
    }
    $secs = [int]((Get-Date) - $start).TotalSeconds
    if ($secs -gt $counters.stream_secs_max) { $counters.stream_secs_max = $secs }
    $counters.sse_frames += $frames
    if (-not $seenId) { Log-Event "流内未出现 id=$id（frames=$frames）" }
}

$deadline = (Get-Date).AddSeconds($MaxSeconds)
$round = 0
Log-Event "worker 启动：mode=$Mode interval=${IntervalMs}ms"
Save-Counters

while ((Get-Date) -lt $deadline) {
    if (-not [string]::IsNullOrWhiteSpace($LifetimeFlag) -and -not (Test-Path -LiteralPath $LifetimeFlag)) {
        Log-Event "lifetime flag 消失 ⇒ worker 退出（父进程已收工）"
        break
    }
    $round++
    switch ($Mode) {
        "mixed" {
            $r = $rand.NextDouble()
            if ($r -lt 0.10) { Do-Preflight }
            elseif ($r -lt 0.20) { Do-Post $true $false }
            else { Do-Post $false $false }
            Start-Sleep -Milliseconds ([int]($IntervalMs * (0.6 + $rand.NextDouble() * 0.8)))
        }
        "sse" {
            $long = ($round % 5 -eq 0)
            $n = if ($long) { 250 } else { 8 }
            Do-Post $false $true $n
            Start-Sleep -Milliseconds 800
        }
        "abort" {
            $id = [Guid]::NewGuid().ToString("N").Substring(0, 8)
            $payload = '{"model":"soak","id":"' + $id + '","stream":true,"n":200,"messages":[]}'
            $req = New-Object System.Net.Http.HttpRequestMessage([System.Net.Http.HttpMethod]::Post, "$base/v1/chat/completions")
            $req.Content = New-Object System.Net.Http.StringContent($payload, [System.Text.Encoding]::UTF8)
            $req.Content.Headers.ContentType = New-Object System.Net.Http.Headers.MediaTypeHeaderValue("application/json")
            $req.Headers.Add("X-Soak-Id", $id)
            try {
                $resp = $client.SendAsync($req, [System.Net.Http.HttpCompletionOption]::ResponseHeadersRead).Result
                $counters.sent++
                if ([int]$resp.StatusCode -eq 200) {
                    $counters.ok++
                    Read-Stream $resp $id 2
                } else {
                    $counters.err++
                    $resp.Dispose()
                }
            } catch {
                $counters.exc++; Log-Event "abort 建流异常：$($_.Exception.GetType().Name)：$($_.Exception.Message)"
            } finally { $req.Dispose() }
            if ($round % 10 -eq 0) { Start-Sleep -Seconds 5 } else { Start-Sleep -Milliseconds 1200 }
        }
    }
    if ($round % 4 -eq 0) { Save-Counters }
}
Save-Counters
Log-Event "worker 收工：mode=$Mode rounds=$round"
$client.Dispose()
exit 0
