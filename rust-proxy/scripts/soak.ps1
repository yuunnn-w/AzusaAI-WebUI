#requires -Version 5.1
<#
soak.ps1 —— rust-proxy Phase 3 的 **24 h soak 驱动**（方案 §9 Phase 3 行 / §10.3 S1 / §4.4「长时间稳定」）。

一句话：**用 win7 目标的 release exe** 起代理（GUI 形态，与用户双击一致），本进程内托管 mock 上游，
拉起若干客户端负载子进程，按固定节拍采样时间序列并落盘；断网/重连、客户端中止、日志节流都在
负载里真实发生（不是模拟计数）。

判据（方案 §4.4）：
  · 无崩溃（进程存活 + 无 panic/ERROR 记录）
  · RSS 斜率 ≤ 5 MB/24 h（回归最小二乘；见 summary.json 的 rss_slope_mb_per_24h）
  · 句柄数/线程数稳定（斜率 + 末段平台判读）
  · 错误率不随时间上升（access 日志 5xx 计数的时间序列）
配套观察：**access 日志节流**（FR-39：同名 warn ≤ 1 条/小时，`Content-Type: text/plain` 负载触发）、
**上游断网/重连**（每 `-BlackoutEverySec` 秒关掉上游 `-BlackoutSec` 秒）、**客户端中止**（abort worker）、
**连接复用**（mock 上游的 TCP 连接数远小于请求数）。

用法（detached 启动，见 README.txt 里的「停止方式」）：
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts/soak.ps1 `
      -RunDir ..\..\shared\tmp\rust-proxy-p3\soak\run-20260919-0700 -DurationHours 24

产出（全部在 `-RunDir` 下）：
  proxy/azusa-local-proxy.exe    跑的是这份**副本**（D3/D12 后**程序读写零文件**：同目录不再有配置、
                                 也不再有日志文件 —— 日志证据链 = 本脚本 `-RedirectStandardOutput`
                                 捕获的 `out/proxy-stdout.log`，见下）
  out/proxy-stdout.log           **脚本侧捕获**的代理 stdout（程序只写继承句柄；文件由本脚本创建）
  out/soak-controller.log        控制器日志（每次采样的摘要行）
  out/timeseries.csv             **时间序列**（采样列见 CSV 表头）
  out/summary.json              收尾汇总（含斜率、峰值、崩溃标记）
  workers/worker-*.json          各负载子进程的累计计数
  workers/worker-*.events.log    子进程侧的异常/读数事件（有上限，防巨型文件）
  stop.flag                      出现该文件 ⇒ 本次 soak 收尾（正常停止方式）
#>
param(
    [string]$Exe = "",
    [string]$RunDir = "",
    [int]$ProxyPort = 8010,
    [int]$UpstreamPort = 8011,
    [double]$DurationHours = 24,
    [int]$SampleSec = 30,
    [int]$BlackoutEverySec = 600,
    [int]$BlackoutSec = 20,
    # 断网形态：`stream-only`（默认）= 停监听（新连接被拒）+ **只掐在途流**；
    #           `realistic`  = 停监听 + **断开全部连接**（含空闲 keep-alive）。
    # ⚠ 历史：Phase 3 起默认是 `stream-only`，因为当时 `realistic`（断开空闲 keep-alive）会触发已登记的
    #   P1「半开池/静默 ⇒ 转发永久挂死」。**该缺陷已在 B1 修复**（`response_head_ms` 上界 + 失败即弃池）
    #   ⇒ `realistic` 已可用；B3（带断网 2 h soak）按方案 §6.5 显式传 `-BlackoutMode realistic`。
    #   本参数默认值**保持 stream-only**（不改变既有行为），由调用方显式选择。
    [ValidateSet("stream-only", "realistic")][string]$BlackoutMode = "stream-only",
    [string]$WorkerSpec = "mixed:2,sse:1,abort:1",
    [int]$ReadyTimeoutSec = 40,
    [switch]$NoProxy,         # 只起 mock 上游 + 负载（自测用；正常 soak 不开）
    # ── B0（半开池修复批 · 装置扩展）：确定性故障复现（方案 §4 B0 / §6.2）
    #    none                = 正常 soak（采样循环）
    #    halfopen            = 预热填池 → mock "进程级消失"（停监听 + 关全部连接）→ 探针（缺陷形态）
    #    control-no-listener = 空池 + 上游从不监听 → 探针（对照：应有界 502）
    #    blackhole           = 预热填池 → mock 转静默（收下不回响应头）→ 探针（池内复用路径）
    #    empty-silent        = 空池 + 静默 → 探针 POST/GET（I20 前身：空池路径）
    #    跑完即退，不进入采样循环；读数落 <RunDir>/out/{readings.json,scenario.log}
    [ValidateSet("none", "halfopen", "control-no-listener", "blackhole", "empty-silent")][string]$FaultScenario = "none",
    [int]$ProbeMaxSec = 8,
    # B0：只当"mock 上游宿主"跑（给故障场景做**进程级消失**用：父进程按 PID 杀掉本子进程 = 上游进程消失）
    [switch]$MockOnly,
    [string]$MockStatsFile = "",
    [switch]$AllowNonWin7     # 显式放行非 win7 目标（默认硬失败 —— Phase 3 审查 P2 加固项②）
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
if ([string]::IsNullOrWhiteSpace($Exe)) { $Exe = Join-Path $repoRoot "target\x86_64-win7-windows-msvc\release\azusa-local-proxy.exe" }
$Exe = [IO.Path]::GetFullPath($Exe)
if ([string]::IsNullOrWhiteSpace($RunDir)) {
    $RunDir = Join-Path (Join-Path $repoRoot "..\..\shared\tmp\rust-proxy-p3\soak") ("run-" + (Get-Date -Format "yyyyMMdd-HHmmss"))
}
$RunDir = [IO.Path]::GetFullPath($RunDir)
$proxyDir = Join-Path $RunDir "proxy"
$outDir = Join-Path $RunDir "out"
$workerDir = Join-Path $RunDir "workers"
$flagPath = Join-Path $RunDir "stop.flag"
foreach ($d in @($RunDir, $proxyDir, $outDir, $workerDir)) { New-Item -ItemType Directory -Force -Path $d | Out-Null }
$ctrlLog = Join-Path $outDir "soak-controller.log"
$csvPath = Join-Path $outDir "timeseries.csv"
# 代理 stdout 的**脚本侧捕获**路径 —— 故障场景与采样循环**唯一共用**该变量（避免"读到另一场景的
# 捕获文件"这类静默错源）。⚠ 口径：文件由**本脚本**创建（`Start-Process -RedirectStandardOutput`），
# 程序只往继承来的句柄里写 —— **不是"程序写文件"**（方案 §2.3 末条 / §11-① 的必须写清楚的区分）。
$CapturePath = Join-Path $outDir "proxy-stdout.log"

function Log([string]$m, [switch]$Quiet) {
    $line = "[{0}] {1}" -f (Get-Date -Format "yyyy-MM-dd HH:mm:ss"), $m
    if (-not $Quiet) { Write-Host $line }
    Add-Content -LiteralPath $ctrlLog -Value $line -Encoding UTF8
}

# ── mock 上游（C#：TcpListener + 每连接线程；可控"断网"= StopListener/Start） ──────
Add-Type -TypeDefinition @"
using System;
using System.Collections.Generic;
using System.Globalization;
using System.Net;
using System.Net.Sockets;
using System.Text;
using System.Threading;

public class RZUpstream
{
    private TcpListener _listener;
    private volatile bool _accepting;
    private Thread[] _threads;
    private readonly object _sync = new object();
    private readonly List<TcpClient> _live = new List<TcpClient>();
    private readonly HashSet<TcpClient> _busy = new HashSet<TcpClient>();

    public long ConnCount;
    public long ReqCount;
    public long SseStreams;
    public long SseFrames;
    public long WriteFails;
    public long CorsConflict;
    public long KeepAliveReuse;   // 同一条 TCP 上的第 2+ 个请求 = 连接复用的直接证据
    public long SilentHits;       // 命中"静默"的请求数（收下但不回响应头）
    public volatile bool Silent;  // B0：静默模式（黑洞 / silent-hold）

    public void Start(int port, int threads)
    {
        // ⚠ 断网=Stop/重连=Start 时，先前被强制关闭的**已接受连接**会留在 TIME_WAIT（本机端口 = 上游端口）
        // ⇒ 不带 SO_REUSEADDR 的重绑会失败（实测："通常每个套接字地址只允许使用一次"），
        //    断网后上游再也起不来，soak 只会跑 502 路径。故：关独占 + 开 ReuseAddress + 重试 5 次。
        Exception last = null;
        for (int attempt = 1; attempt <= 5; attempt++)
        {
            try
            {
                TcpListener l = new TcpListener(IPAddress.Loopback, port);
                l.ExclusiveAddressUse = false;
                l.Server.SetSocketOption(SocketOptionLevel.Socket, SocketOptionName.ReuseAddress, true);
                l.Start();
                _listener = l;
                last = null;
                break;
            }
            catch (Exception ex)
            {
                last = ex;
                Thread.Sleep(1000);
            }
        }
        if (last != null) { throw last; }
        _accepting = true;
        _threads = new Thread[threads];
        for (int i = 0; i < threads; i++)
        {
            Thread t = new Thread(new ThreadStart(AcceptLoop));
            t.IsBackground = true;
            t.Start();
            _threads[i] = t;
        }
    }

    public bool IsAccepting { get { return _accepting; } }

    public void StopListener()
    {
        _accepting = false;
        TcpListener l = _listener;
        _listener = null;
        if (l != null) { try { l.Stop(); } catch (Exception) { } }
    }

    /// 只断开"正在写响应"的连接（SSE 长流）—— 模拟上游把在途流掐断（FR-23）。
    /// ⚠ 与 `AbortAll` 的区别见脚本注释：杀**空闲 keep-alive** 连接会让代理的连接池留下半开连接，
    /// 触发已登记的 P1 缺陷（后续请求永久挂死、上游恢复也不自愈）⇒ 默认断网用本方法。
    public void AbortBusy()
    {
        List<TcpClient> copy;
        lock (_sync)
        {
            copy = new List<TcpClient>(_busy);
            _busy.Clear();
        }
        foreach (TcpClient c in copy) { try { c.Close(); } catch (Exception) { } }
    }

    /// B0：**进程级消失**的等价形态 —— 停监听（新连接被拒）+ 关闭全部已接受连接
    /// （对端视角：没有任何 FIN/RST 可读，池里那条连接就此半开）。事故形态复刻用。
    public void KillAll()
    {
        // ⚠ 只调 StopListener() 在**已被接受连接存在**时实测不够狠：监听的 LISTENING 状态会挂到进程退出，
        // 于是新拨号落进 backlog（无 accept 线程）⇒ 症状也是"挂死"，但**不是**事故形态（事故里上游端口无监听）。
        // 故这里直接对底层 socket 做 linger=0 的强制关闭，保证"进程级消失"等价形态。
        _accepting = false;
        TcpListener l = _listener;
        _listener = null;
        if (l != null)
        {
            try { l.Server.LingerState = new LingerOption(true, 0); } catch (Exception) { }
            try { l.Server.Close(0); } catch (Exception) { }
            try { l.Stop(); } catch (Exception) { }
        }
        AbortAll();
    }

    /// 断开**全部**在途连接（含空闲 keep-alive）—— "真实断网"形态；会触发 P1（半开池复用挂死）。
    public void AbortAll()
    {
        List<TcpClient> copy;
        lock (_sync) { copy = new List<TcpClient>(_live); _live.Clear(); }
        foreach (TcpClient c in copy) { try { c.Close(); } catch (Exception) { } }
    }

    private void AcceptLoop()
    {
        while (_accepting)
        {
            TcpListener l = _listener;
            if (l == null) { return; }
            TcpClient client;
            try { client = l.AcceptTcpClient(); }
            catch (Exception) { return; }
            Interlocked.Increment(ref ConnCount);
            lock (_sync) { _live.Add(client); }
            try { Handle(client); }
            catch (Exception) { }
            finally
            {
                lock (_sync) { _live.Remove(client); }
                try { client.Close(); } catch (Exception) { }
            }
        }
    }

    private void Handle(TcpClient client)
    {
        client.NoDelay = true;
        client.ReceiveTimeout = 60000;
        client.SendTimeout = 60000;
        NetworkStream ns = client.GetStream();
        BufReader reader = new BufReader(ns);
        int served = 0;
        while (true)
        {
            string method, path, version;
            Dictionary<string, string> headers;
            byte[] body;
            if (!ReadRequest(reader, out method, out path, out version, out headers, out body)) { return; }
            long n = Interlocked.Increment(ref ReqCount);
            served++;
            if (served > 1) { Interlocked.Increment(ref KeepAliveReuse); }
            string bodyText = body == null ? "" : Encoding.UTF8.GetString(body);
            if (Silent)                       // B0：收下请求但不回响应头（连接保持 ESTABLISHED）
            {
                Interlocked.Increment(ref SilentHits);
                while (Silent) { Thread.Sleep(100); }
            }
            bool stream = bodyText.IndexOf("\"stream\":true", StringComparison.Ordinal) >= 0;
            string id = Extract(bodyText, "\"id\":\"");
            int frames = ExtractInt(bodyText, "\"n\":", 8);
            bool conflict = (n % 3 == 0);
            int status = path.IndexOf("/error400") >= 0 ? 400 : 200;
            if (stream) { ServeSse(ns, id, frames, conflict, client); }
            else { ServeJson(ns, path, method, status, id, conflict, bodyText); }
            if (!string.Equals(version, "HTTP/1.1", StringComparison.Ordinal)) { return; }
        }
    }

    private static bool ReadRequest(BufReader r, out string method, out string path, out string version,
                                    out Dictionary<string, string> headers, out byte[] body)
    {
        method = null; path = null; version = null; body = null;
        headers = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        string line = r.ReadLine();
        if (line == null) { return false; }
        string[] parts = line.Split(' ');
        if (parts.Length < 3) { return false; }
        method = parts[0]; path = parts[1]; version = parts[2];
        while (true)
        {
            string h = r.ReadLine();
            if (h == null) { return false; }
            if (h.Length == 0) { break; }
            int idx = h.IndexOf(':');
            if (idx > 0) { headers[h.Substring(0, idx).Trim()] = h.Substring(idx + 1).Trim(); }
        }
        int len = 0;
        string cl;
        if (headers.TryGetValue("Content-Length", out cl)) { int.TryParse(cl, NumberStyles.Integer, CultureInfo.InvariantCulture, out len); }
        if (len > 0) { body = r.ReadBytes(len); }
        return true;
    }

    private bool WriteAll(NetworkStream ns, byte[] data)
    {
        try { ns.Write(data, 0, data.Length); return true; }
        catch (Exception) { Interlocked.Increment(ref WriteFails); return false; }
    }

    private bool WriteChunk(NetworkStream ns, byte[] data)
    {
        byte[] head = Encoding.ASCII.GetBytes(data.Length.ToString("x", CultureInfo.InvariantCulture) + "\r\n");
        byte[] tail = Encoding.ASCII.GetBytes("\r\n");
        try
        {
            ns.Write(head, 0, head.Length);
            ns.Write(data, 0, data.Length);
            ns.Write(tail, 0, tail.Length);
            return true;
        }
        catch (Exception) { Interlocked.Increment(ref WriteFails); return false; }
    }

    private void ServeSse(NetworkStream ns, string id, int frames, bool conflict, TcpClient client)
    {
        Interlocked.Increment(ref SseStreams);
        lock (_sync) { _busy.Add(client); }     // 标记"在途流"：断网时只掐这些（见 AbortBusy 注释）
        try
        {
            if (frames <= 0 || frames > 5000) { frames = 8; }
            StringBuilder head = new StringBuilder();
            head.Append("HTTP/1.1 200 OK\r\n");
            head.Append("Content-Type: text/event-stream\r\n");
            head.Append("Cache-Control: no-cache\r\n");
            head.Append("Transfer-Encoding: chunked\r\n");
            head.Append("Connection: keep-alive\r\n");
            if (conflict)
            {
                head.Append("Access-Control-Allow-Origin: http://example.com\r\n");
                head.Append("Access-Control-Allow-Methods: GET\r\n");
                Interlocked.Increment(ref CorsConflict);
            }
            head.Append("\r\n");
            if (!WriteAll(ns, Encoding.ASCII.GetBytes(head.ToString()))) { return; }
            for (int i = 1; i <= frames; i++)
            {
                string frame = "data: {\"id\":\"" + id + "\",\"i\":" + i.ToString(CultureInfo.InvariantCulture)
                    + ",\"cf\":" + (conflict ? "1" : "0")
                    + ",\"choices\":[{\"delta\":{\"content\":\"tok" + i.ToString(CultureInfo.InvariantCulture) + "\"}}]}\n\n";
                if (!WriteChunk(ns, Encoding.UTF8.GetBytes(frame))) { return; }
                Interlocked.Increment(ref SseFrames);
                Thread.Sleep(200);
            }
            if (!WriteChunk(ns, Encoding.UTF8.GetBytes("data: [DONE]\n\n"))) { return; }
            WriteAll(ns, Encoding.ASCII.GetBytes("0\r\n\r\n"));
        }
        finally { lock (_sync) { _busy.Remove(client); } }
    }

    private void ServeJson(NetworkStream ns, string path, string method, int status, string id, bool conflict, string bodyText)
    {
        int echoBytes = bodyText == null ? 0 : Encoding.UTF8.GetByteCount(bodyText);
        string cf = conflict ? ",\"cf\":1" : ",\"cf\":0";
        string body = "{\"id\":\"" + id + "\",\"ok\":" + (status == 200 ? "true" : "false")
            + ",\"path\":\"" + path + "\",\"method\":\"" + method + "\",\"echoBytes\":" + echoBytes.ToString(CultureInfo.InvariantCulture)
            + cf
            + ",\"ts\":" + DateTimeOffset.UtcNow.ToUnixTimeMilliseconds().ToString(CultureInfo.InvariantCulture) + "}";
        byte[] payload = Encoding.UTF8.GetBytes(body);
        StringBuilder head = new StringBuilder();
        head.Append("HTTP/1.1 " + status.ToString(CultureInfo.InvariantCulture) + (status == 200 ? " OK" : " Bad Request") + "\r\n");
        head.Append("Content-Type: application/json\r\n");
        head.Append("Content-Length: " + payload.Length.ToString(CultureInfo.InvariantCulture) + "\r\n");
        head.Append("Connection: keep-alive\r\n");
        if (conflict)
        {
            head.Append("Access-Control-Allow-Origin: http://example.com\r\n");
            head.Append("Access-Control-Allow-Methods: GET\r\n");
            Interlocked.Increment(ref CorsConflict);
        }
        head.Append("\r\n");
        if (!WriteAll(ns, Encoding.ASCII.GetBytes(head.ToString()))) { return; }
        WriteAll(ns, payload);
    }

    private static string Extract(string text, string key)
    {
        int i = text.IndexOf(key, StringComparison.Ordinal);
        if (i < 0) { return ""; }
        int start = i + key.Length;
        int end = text.IndexOf('"', start);
        if (end < 0) { return ""; }
        return text.Substring(start, end - start);
    }

    private static int ExtractInt(string text, string key, int fallback)
    {
        int i = text.IndexOf(key, StringComparison.Ordinal);
        if (i < 0) { return fallback; }
        int start = i + key.Length;
        int end = start;
        while (end < text.Length && (text[end] >= '0' && text[end] <= '9')) { end++; }
        int value;
        if (end > start && int.TryParse(text.Substring(start, end - start), NumberStyles.Integer, CultureInfo.InvariantCulture, out value)) { return value; }
        return fallback;
    }

    private class BufReader
    {
        private readonly NetworkStream _ns;
        private readonly byte[] _buf = new byte[8192];
        private int _pos;
        private int _len;

        public BufReader(NetworkStream ns) { _ns = ns; }

        private int Next()
        {
            if (_pos >= _len)
            {
                _len = _ns.Read(_buf, 0, _buf.Length);
                _pos = 0;
                if (_len <= 0) { return -1; }
            }
            return _buf[_pos++];
        }

        public string ReadLine()
        {
            StringBuilder sb = new StringBuilder();
            while (true)
            {
                int b = Next();
                if (b < 0) { return sb.Length == 0 ? null : sb.ToString(); }
                if (b == 13) { continue; }
                if (b == 10) { return sb.ToString(); }
                sb.Append((char)b);
            }
        }

        public byte[] ReadBytes(int n)
        {
            byte[] outBytes = new byte[n];
            int got = 0;
            while (got < n)
            {
                if (_pos < _len)
                {
                    int take = Math.Min(_len - _pos, n - got);
                    Array.Copy(_buf, _pos, outBytes, got, take);
                    _pos += take;
                    got += take;
                }
                else
                {
                    _len = _ns.Read(_buf, 0, _buf.Length);
                    _pos = 0;
                    if (_len <= 0) { break; }
                }
            }
            if (got < n) { return null; }
            return outBytes;
        }
    }
}
"@

# ── B0：mock-only 宿主（供故障场景做"上游进程级消失"；被杀即等价于上游进程死掉） ──
if ($MockOnly) {
    $mock = New-Object RZUpstream
    $mock.Start($UpstreamPort, 8)
    Log "mock-only 宿主已就绪：127.0.0.1:$UpstreamPort（PID=$PID；父进程按 PID 杀我 = 上游进程消失）"
    while ($true) {
        if (-not [string]::IsNullOrWhiteSpace($MockStatsFile)) {
            try {
                $stats = [ordered]@{ pid = $PID; conns = $mock.ConnCount; reqs = $mock.ReqCount; reuse = $mock.KeepAliveReuse; silent_hits = $mock.SilentHits; sse_streams = $mock.SseStreams; sse_frames = $mock.SseFrames; write_fails = $mock.WriteFails; at = (Get-Date).ToString("HH:mm:ss.fff") }
                ($stats | ConvertTo-Json -Compress) | Set-Content -LiteralPath $MockStatsFile -Encoding UTF8
            } catch { }
        }
        Start-Sleep -Milliseconds 500
    }
}

# ── 检查 exe ────────────────────────────────────────────────────────────
if (-not $NoProxy) {
    if (-not (Test-Path -LiteralPath $Exe)) { Log "装置失败：exe 不存在 $Exe"; exit 2 }
    $exeHash = (Get-FileHash -LiteralPath $Exe -Algorithm SHA256).Hash.ToLower()
    $exeBytes = (Get-Item -LiteralPath $Exe).Length
    # 目标三元组目录 = exe 的**上两级**（exe → release → <triple>）——Phase 3 里多剥了一层，
    # 致使该检查恒判 "target" ⇒ 只打告警、永不生效（B0 加固时被硬失败模式当场抓到）
    $targetDir = Split-Path -Parent (Split-Path -Parent $Exe)
    Log "exe = $Exe"
    Log "exe 读数：$exeBytes B / sha256 $exeHash（目标目录名 = $(Split-Path -Leaf $targetDir)）"
    if ((Split-Path -Leaf $targetDir) -ne "x86_64-win7-windows-msvc") {
        if ($AllowNonWin7) {
            Log "⚠⚠ 已用 -AllowNonWin7 显式放行非 win7 目标（$(Split-Path -Leaf $targetDir)）—— 此读数**不得**当发布门槛证据"
        } else {
            Log "装置硬失败：方案 §9 Phase 3 / §6.5 要求 soak 用 win7 目标的 exe 跑；当前目标 = $(Split-Path -Leaf $targetDir)（确需非 win7 请显式加 -AllowNonWin7）"
            exit 2
        }
    }
}

# ── 现场准备 ────────────────────────────────────────────────────────────
if (-not $NoProxy) {
    $proxyExe = Join-Path $proxyDir "azusa-local-proxy.exe"
    Copy-Item -LiteralPath $Exe -Destination $proxyExe -Force
}

# 代理的 CLI 参数（**唯一来源**：故障场景与采样循环共用；D3 去配置文件后不再生成任何配置文件）。
# 逐项 = 旧配置生成段的等价 CLI 形式（见 §4-S8 / §11-⑥）。
$proxyArgs = @(
    "--listen-host", "127.0.0.1",
    "--listen-port", "$ProxyPort",
    "--port-fallback", "18010,18011",
    "--upstream", "http://127.0.0.1:$UpstreamPort",
    "--cors-mode", "*",
    "--cors-max-age", "600",
    "--connect-timeout-ms", "3000",
    # 本修复批（B1）：等待响应头上限 —— soak/场景档取 3 s（方案 §3/§6.1 的 V_soak；探针 --max-time 需 > 2V+ε）
    "--response-head-timeout-ms", "3000",
    "--first-byte-timeout-ms", "0",
    "--log-level", "info",
    "--close-action", "tray"
)


# ── B0：确定性故障复现（装置扩展；见 specs/rust-proxy-halfopen-fix-plan.md §4 B0 / §6.2） ──
# 每场景在**当前（未修）载荷**上的期望读数（= 红）：
#   halfopen            : 预热填池 → mock "进程级消失" ⇒ 探针 **000**（挂死）
#   control-no-listener : 空池 + 上游从不监听          ⇒ 探针 **502** 且 ≤ ~3 s（有界 · 对照臂）
#   blackhole           : 预热填池 → mock 静默（收下不回响应头）⇒ 探针 **000**
#   empty-silent        : 空池 + 静默（I20 前身）      ⇒ 探针 POST/GET 均 **000**
function Invoke-FaultScenario {
    param([string]$Scenario, [string]$ProxyExe, [string]$ProxyWorkDir, [string]$EvidenceDir)
    $script:scenSteps = New-Object System.Collections.Generic.List[object]
    $probeBody = Join-Path $EvidenceDir "body-nonstream.json"
    Set-Content -LiteralPath $probeBody -Value '{"id":"b0-probe","stream":false}' -Encoding ASCII
    $script:scenMock = New-Object RZUpstream

    function AddStep([string]$name, $data) {
        $script:scenSteps.Add([ordered]@{ step = $name; at = (Get-Date).ToString("HH:mm:ss.fff"); data = $data })
        Log ("  · {0}：{1}" -f $name, (($data | ConvertTo-Json -Compress -Depth 5)))
    }
    function Probe([string]$method, [int]$maxSec) {
        $url = "http://127.0.0.1:$ProxyPort/v1/chat/completions"
        if ($method -eq "POST") {
            $raw = & curl.exe -s -o NUL -w "%{http_code}|%{time_total}" --max-time $maxSec -X POST -H "Content-Type: application/json" --data "@$probeBody" $url
        } else {
            $raw = & curl.exe -s -o NUL -w "%{http_code}|%{time_total}" --max-time $maxSec $url
        }
        $parts = "$raw".Split("|")
        return [ordered]@{ method = $method; status = $parts[0]; seconds = [double]$parts[1]; raw = "$raw"; max_sec = $maxSec }
    }
    function NetstatUpstream {
        $lines = @(netstat -ano | Select-String -Pattern ":$UpstreamPort\s" | ForEach-Object { $_.Line.Trim() })
        return [ordered]@{ lines = @($lines | Select-Object -First 8); count = $lines.Count }
    }
    function ProxyReady($proc) {
        $deadline = (Get-Date).AddSeconds($ReadyTimeoutSec)
        while ((Get-Date) -lt $deadline) {
            if ($proc.HasExited) { return $false }
            try { $c = New-Object System.Net.Sockets.TcpClient; $c.Connect("127.0.0.1", $ProxyPort); $c.Close(); return $true }
            catch { Start-Sleep -Milliseconds 400 }
        }
        return $false
    }
    function StopProxyProcess($proc) {
        if ($proc) {
            try { Stop-Process -Id $proc.Id -Force -ErrorAction Stop; Log "  已按 PID 关闭代理（PID=$($proc.Id)）" } catch { Log "  代理已不在（PID=$($proc.Id)）" }
        }
    }
    function StartProxyProcess {
        return Start-Process -FilePath $ProxyExe -WorkingDirectory $ProxyWorkDir -PassThru `
            -ArgumentList $proxyArgs `
            -RedirectStandardOutput $CapturePath `
            -RedirectStandardError (Join-Path $EvidenceDir "proxy-stderr.log")
    }

    $proxyProc = $null
    $mockProc = $null
    $readings = [ordered]@{
        scenario       = $Scenario
        started        = (Get-Date).ToString("s")
        probe_max_sec  = $ProbeMaxSec
        proxy_port     = $ProxyPort
        upstream_port  = $UpstreamPort
        exe            = $ProxyExe
        exe_bytes      = $exeBytes
        exe_sha256     = $exeHash
    }
    Log "=== B0 故障场景：$Scenario（探针上限 ${ProbeMaxSec}s）==="

    try {
        switch ($Scenario) {
            "control-no-listener" {
                $readings["mock"] = [ordered]@{ started = $false; note = "上游端口从不监听（空池对照臂）" }
                $proxyProc = StartProxyProcess
                Log "  代理 PID=$($proxyProc.Id)（上游端口 $UpstreamPort 无监听）"
                if (-not (ProxyReady $proxyProc)) { throw "代理未就绪" }
                $p1 = Probe "POST" $ProbeMaxSec
                AddStep "probe_post_cold_empty_pool" $p1
                $readings["probe_post"] = $p1
            }
            "empty-silent" {
                $script:scenMock.Silent = $true
                $script:scenMock.Start($UpstreamPort, 8)
                AddStep "mock_started_silent" ([ordered]@{ silent = $true; conns = $script:scenMock.ConnCount })
                $proxyProc = StartProxyProcess
                Log "  代理 PID=$($proxyProc.Id)"
                if (-not (ProxyReady $proxyProc)) { throw "代理未就绪" }
                $p1 = Probe "POST" $ProbeMaxSec
                AddStep "probe_post_empty_pool_silent" $p1
                $p2 = Probe "GET" $ProbeMaxSec
                AddStep "probe_get_empty_pool_silent" $p2
                $readings["probe_post"] = $p1
                $readings["probe_get"] = $p2
                $readings["mock"] = [ordered]@{ conns = $script:scenMock.ConnCount; reqs = $script:scenMock.ReqCount; reuse = $script:scenMock.KeepAliveReuse; silent_hits = $script:scenMock.SilentHits }
                $script:scenMock.Silent = $false
                Start-Sleep -Milliseconds 400
                $p3 = Probe "POST" 5
                AddStep "probe_post_after_recover" $p3
                $readings["probe_recover"] = $p3
            }
            "blackhole" {
                $script:scenMock.Start($UpstreamPort, 8)
                $proxyProc = StartProxyProcess
                Log "  代理 PID=$($proxyProc.Id)"
                if (-not (ProxyReady $proxyProc)) { throw "代理未就绪" }
                $w1 = Probe "POST" 5
                $w2 = Probe "POST" 5
                AddStep "warmup_two_posts" ([ordered]@{ first = $w1; second = $w2; mock_conns = $script:scenMock.ConnCount; mock_reqs = $script:scenMock.ReqCount; keepalive_reuse = $script:scenMock.KeepAliveReuse })
                $script:scenMock.Silent = $true
                AddStep "mock_became_silent" ([ordered]@{ silent = $true })
                $p1 = Probe "POST" $ProbeMaxSec
                AddStep "probe_post_pool_reuse_silent" $p1
                $readings["mock"] = [ordered]@{ conns = $script:scenMock.ConnCount; reqs = $script:scenMock.ReqCount; keepalive_reuse = $script:scenMock.KeepAliveReuse; silent_hits = $script:scenMock.SilentHits }
                $readings["probe_post"] = $p1
                $script:scenMock.Silent = $false
                Start-Sleep -Milliseconds 400
                $p2 = Probe "POST" 5
                AddStep "probe_post_after_recover" $p2
                $readings["probe_recover"] = $p2
            }
            "halfopen" {
                # 事故形态复刻：mock 跑在**独立子进程**里，父进程按 PID 杀它 = 上游进程级消失
                # （比"进程内关 socket"更忠实：事故当天就是上游进程没了）
                $statsFile = Join-Path $EvidenceDir "mock-stats.json"
                $mockProc = Start-Process -FilePath "powershell" -WindowStyle Hidden -PassThru `
                    -ArgumentList @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $PSCommandPath,
                        "-MockOnly", "-UpstreamPort", "$UpstreamPort", "-ProxyPort", "$ProxyPort", "-MockStatsFile", "$statsFile") `
                    -RedirectStandardOutput (Join-Path $EvidenceDir "mock-stdout.log") `
                    -RedirectStandardError (Join-Path $EvidenceDir "mock-stderr.log")
                Log "  mock 子进程 PID=$($mockProc.Id)（-MockOnly）"
                Start-Sleep -Milliseconds 1200
                $script:scenMock = $null
                $proxyProc = StartProxyProcess
                Log "  代理 PID=$($proxyProc.Id)"
                if (-not (ProxyReady $proxyProc)) { throw "代理未就绪" }
                $w1 = Probe "POST" 5
                $w2 = Probe "POST" 5
                Start-Sleep -Milliseconds 400
                $readStats = {
                    if (Test-Path -LiteralPath $statsFile) { Get-Content -LiteralPath $statsFile -Raw -Encoding UTF8 | ConvertFrom-Json } else { $null }
                }
                $mockStats = & $readStats
                if ($null -eq $mockStats -or [int]$mockStats.conns -eq 0) {   # 写盘节拍竞态：再等一拍重读
                    Start-Sleep -Milliseconds 700
                    $mockStats = & $readStats
                }
                $nsBefore = NetstatUpstream
                AddStep "warmup_two_posts" ([ordered]@{ first = $w1; second = $w2; mock_stats = $mockStats; netstat = $nsBefore })
                Stop-Process -Id $mockProc.Id -Force
                AddStep "mock_process_killed" ([ordered]@{ pid = $mockProc.Id; note = "按 PID 杀 mock 子进程 = 上游进程级消失（E16：按 PID，未用 /F /IM）" })
                Start-Sleep -Milliseconds 500
                AddStep "mock_killed_all" ([ordered]@{ note = "停监听 + 关闭全部已接受连接（事故形态：池中连接半开）"; netstat = (NetstatUpstream) })
                $p1 = Probe "POST" $ProbeMaxSec
                AddStep "probe_post_after_kill" $p1
                $nsAfter = NetstatUpstream
                AddStep "netstat_after_kill" $nsAfter
                $p2 = Probe "POST" $ProbeMaxSec
                AddStep "probe_post_second" $p2
                $readings["mock"] = [ordered]@{ mode = "child-process"; killed_pid = $mockProc.Id; stats_before_kill = $mockStats }
                $readings["probe_post"] = $p1
                $readings["probe_post_second"] = $p2
                $readings["netstat_before_kill"] = $nsBefore
                $readings["netstat_after_kill"] = $nsAfter
            }
        }
        try {
            # 五项读数里与本场景收尾相关的三项 —— **换源**：改读脚本侧重定向捕获的 stdout
            # （`$CapturePath`），不再读"程序写出的日志文件"（D12 后程序不写任何文件）。
            # tail 口径（末 8 行）与正则口径逐字不变。
            if (Test-Path -LiteralPath $CapturePath) {
                # ⚠ 必须逐元素强制成**字符串**：`Get-Content` 返回的元素带 PSPath/PSDrive/Provider 等
                #   附加属性，`ConvertTo-Json` 会把它们展开成数十 MB（B3fix P2-3：readings.json 曾达 32.8 MB）。
                $tail = @(Get-Content -LiteralPath $CapturePath -Tail 8 -Encoding UTF8 | ForEach-Object { "$_" })
                $readings["proxy_log_tail"] = $tail
                $readings["proxy_log_bytes"] = (Get-Item -LiteralPath $CapturePath).Length
                $watchdog = @($tail | Where-Object { $_ -match "watchdog|响应头|池重置" })
                $readings["proxy_log_watchdog_or_poolreset_lines"] = $watchdog
            }
        } catch { }
        $readings["finished"] = (Get-Date).ToString("s")
        $readings["steps"] = $script:scenSteps
        ($readings | ConvertTo-Json -Depth 6) | Set-Content -LiteralPath (Join-Path $EvidenceDir "readings.json") -Encoding UTF8
        Log "读数已落盘：$(Join-Path $EvidenceDir 'readings.json')"
    } catch {
        $readings["error"] = $_.Exception.Message
        $readings["steps"] = $script:scenSteps
        ($readings | ConvertTo-Json -Depth 6) | Set-Content -LiteralPath (Join-Path $EvidenceDir "readings.json") -Encoding UTF8
        Log "场景异常：$($_.Exception.Message)"
        StopProxyProcess $proxyProc
        try { $script:scenMock.KillAll() } catch { }
        exit 2
    } finally {
        StopProxyProcess $proxyProc
        if ($mockProc) {
            try { Stop-Process -Id $mockProc.Id -Force -ErrorAction Stop; Log "  已按 PID 关闭 mock 子进程（PID=$($mockProc.Id)）" } catch { Log "  mock 子进程已不在（PID=$($mockProc.Id)）" }
        }
        if ($script:scenMock) { try { $script:scenMock.KillAll() } catch { } }
    }
}

if (-not $NoProxy -and $FaultScenario -ne "none") {
    Invoke-FaultScenario -Scenario $FaultScenario -ProxyExe $proxyExe -ProxyWorkDir $proxyDir -EvidenceDir $outDir
    exit 0
}

# mock 上游先起（代理启动即要连它）—— ⚠ **子进程形态**（B3 装置修复②）：
#   进程内 mock 的"停监听→重连"在实测里出现过**双监听 socket**（SO_REUSEADDR 允许重复绑定）
#   ⇒ 新连接落进**没有 accept 线程**的旧 socket 的 backlog ⇒ 现象 = 请求永远等不到响应头（与 P1 同形），
#   而装置自己看不出来。改用子进程后：**断网 = 按 PID 杀（真进程级消失、端口随之释放）**，
#   **重连 = 起一个全新子进程**（全新 socket）—— 既更忠实事故形态，也彻底避开该 socket 生命周期坑。
$mockStatsFile = Join-Path $outDir "mock-stats.json"
$upstream = New-Object RZUpstream          # 只当"读数容器"：字段由子进程的 stats 文件回填
$mockProc = $null
function Start-MockChild {
    if (Test-Path -LiteralPath $mockStatsFile) { Remove-Item -LiteralPath $mockStatsFile -Force }
    return Start-Process -FilePath "powershell" -WindowStyle Hidden -PassThru `
        -ArgumentList @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $PSCommandPath,
            "-MockOnly", "-UpstreamPort", "$UpstreamPort", "-MockStatsFile", "$mockStatsFile") `
        -RedirectStandardOutput (Join-Path $outDir "mock-stdout.log") `
        -RedirectStandardError (Join-Path $outDir "mock-stderr.log")
}
function Wait-UpstreamPort([int]$timeoutSec) {
    $deadline = (Get-Date).AddSeconds($timeoutSec)
    while ((Get-Date) -lt $deadline) {
        try { $c = New-Object System.Net.Sockets.TcpClient; $c.Connect("127.0.0.1", $UpstreamPort); $c.Close(); return $true }
        catch { Start-Sleep -Milliseconds 250 }
    }
    return $false
}
function Wait-UpstreamPortDown([int]$timeoutSec) {
    $deadline = (Get-Date).AddSeconds($timeoutSec)
    while ((Get-Date) -lt $deadline) {
        try { $c = New-Object System.Net.Sockets.TcpClient; $c.Connect("127.0.0.1", $UpstreamPort); $c.Close() }
        catch { return $true }
        Start-Sleep -Milliseconds 250
    }
    return $false
}
function Read-MockStats {
    if (-not (Test-Path -LiteralPath $mockStatsFile)) { return }
    try {
        $j = Get-Content -LiteralPath $mockStatsFile -Raw -Encoding UTF8 | ConvertFrom-Json
        $upstream.ConnCount = [long]$j.conns
        $upstream.ReqCount = [long]$j.reqs
        $upstream.KeepAliveReuse = [long]$j.reuse
        $upstream.SilentHits = [long]$j.silent_hits
        $upstream.SseStreams = [long]$j.sse_streams
        $upstream.SseFrames = [long]$j.sse_frames
        $upstream.WriteFails = [long]$j.write_fails
    } catch { }
}
if ($FaultScenario -eq "none") {
    $mockProc = Start-MockChild
    if (-not (Wait-UpstreamPort 15)) { Log "装置失败：mock 子进程未就绪（PID=$($mockProc.Id)）"; exit 2 }
    Log "mock 上游已起（**子进程** PID=$($mockProc.Id)）：127.0.0.1:$UpstreamPort；断网 = 按 PID 杀它"
}

$proxyProc = $null
$workerProcs = @()
$startedAt = Get-Date
$crash = $false
$crashNote = ""

# `-NoProxy`：只跑 mock 上游自测（不起代理、不采样），用来验证装置本身
if ($NoProxy) {
    Log "自测模式：mock 上游已起，直接探三条（预检 / 非流式 / SSE）"
    # ⚠ JSON 体一律走**文件**（`--data @file`）：PowerShell 5.1 向原生 exe 传参时会吃掉参数里的
    # 双引号（实测：`{"id":"x"}` 变成 `{id:x}`，长度 26 ≠ 32），走文件是唯一稳的写法。
    $bodyPlain = Join-Path $outDir "body-nonstream.json"
    $bodySse = Join-Path $outDir "body-sse.json"
    Set-Content -LiteralPath $bodyPlain -Value '{"id":"selftest","stream":false}' -Encoding ASCII
    Set-Content -LiteralPath $bodySse -Value '{"id":"selftest-sse","stream":true,"n":3}' -Encoding ASCII
    $r1 = & curl.exe -s -o NUL -w "%{http_code}" --max-time 5 -X OPTIONS -H "Origin: null" -H "Access-Control-Request-Method: POST" "http://127.0.0.1:$UpstreamPort/v1/chat/completions"
    $r2 = & curl.exe -s --max-time 5 -X POST -H "Content-Type: application/json" --data "@$bodyPlain" "http://127.0.0.1:$UpstreamPort/v1/models"
    $r3 = & curl.exe -s -N --max-time 6 -X POST -H "Content-Type: application/json" --data "@$bodySse" "http://127.0.0.1:$UpstreamPort/v1/chat/completions"
    $frames = @($r3 -split "`n" | Where-Object { $_ -like "data:*" }).Count
    Log "自测读数：preflight=$r1 / post=$r2 / sse_frames=$frames / up_conns=$($upstream.ConnCount) up_reqs=$($upstream.ReqCount) reuse=$($upstream.KeepAliveReuse)"
    $upstream.StopListener()
    exit 0
}

try {
    if (-not $NoProxy) {
        # 先确认真实上游端口没被占（防"上游其实活着"的假读数）
        $busy = New-Object System.Net.Sockets.TcpClient
        try { $busy.Connect("127.0.0.1", $ProxyPort); $busy.Close(); Log "装置失败：$ProxyPort 已被占用"; exit 2 } catch { }

        $proxyProc = Start-Process -FilePath $proxyExe -WorkingDirectory $proxyDir -PassThru `
            -ArgumentList $proxyArgs `
            -RedirectStandardOutput $CapturePath `
            -RedirectStandardError (Join-Path $outDir "proxy-stderr.log")
        Log "代理已启动：PID=$($proxyProc.Id)（工作目录 $proxyDir）"

        $ready = $false
        $deadline = (Get-Date).AddSeconds($ReadyTimeoutSec)
        while ((Get-Date) -lt $deadline) {
            if ($proxyProc.HasExited) { Log "装置失败：代理启动后立即退出（exit=$($proxyProc.ExitCode)）"; exit 2 }
            try {
                $probe = New-Object System.Net.Sockets.TcpClient
                $probe.Connect("127.0.0.1", $ProxyPort); $probe.Close(); $ready = $true; break
            } catch { Start-Sleep -Milliseconds 400 }
        }
        if (-not $ready) { Log "装置失败：$ReadyTimeoutSec s 内 $ProxyPort 未 LISTENING"; exit 2 }
        Log "代理已就绪：$ProxyPort LISTENING"

        # 启动冒烟（三条：预检 204 / 非流式 POST / SSE 首帧；JSON 体走文件，见上方 PowerShell 传参坑）
        $smoke = @()
        $bodyPlain = Join-Path $outDir "body-nonstream.json"
        $bodySse = Join-Path $outDir "body-sse.json"
        Set-Content -LiteralPath $bodyPlain -Value '{"id":"smoke","stream":false}' -Encoding ASCII
        Set-Content -LiteralPath $bodySse -Value '{"id":"smoke-sse","stream":true,"n":3}' -Encoding ASCII
        try {
            $r1 = & curl.exe -s -o NUL -w "%{http_code}" --max-time 5 -X OPTIONS -H "Origin: null" -H "Access-Control-Request-Method: POST" "http://127.0.0.1:$ProxyPort/v1/chat/completions"
            $smoke += "preflight=$r1"
            $r2 = & curl.exe -s -o NUL -w "%{http_code}" --max-time 5 -X POST -H "Content-Type: application/json" --data "@$bodyPlain" "http://127.0.0.1:$ProxyPort/v1/chat/completions"
            $smoke += "post=$r2"
            $r3 = & curl.exe -s -N --max-time 6 -X POST -H "Content-Type: application/json" --data "@$bodySse" "http://127.0.0.1:$ProxyPort/v1/chat/completions"
            $frames = @($r3 -split "`n" | Where-Object { $_ -like "data:*" }).Count
            $smoke += "sse_frames=$frames"
        } catch { $smoke += "smoke_exception=$($_.Exception.Message)" }
        Log ("启动冒烟：" + ($smoke -join " / "))
        ($smoke -join "`n") | Set-Content -LiteralPath (Join-Path $outDir "startup-smoke.txt") -Encoding UTF8
    }

    # 负载子进程
    $workerScript = Join-Path (Split-Path -Parent $MyInvocation.MyCommand.Path) "soak-worker.ps1"
    $lifetimeFlag = Join-Path $RunDir "controller.alive"
    Set-Content -LiteralPath $lifetimeFlag -Value (Get-Date -Format "s") -Encoding UTF8
    $idx = 0
    foreach ($spec in $WorkerSpec.Split(",")) {
        $parts = $spec.Split(":")
        $mode = $parts[0].Trim()
        $count = [int]$parts[1].Trim()
        for ($i = 1; $i -le $count; $i++) {
            $idx++
            $name = "$mode-$i"
            $argList = @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $workerScript,
                "-Mode", $mode, "-ProxyPort", "$ProxyPort", "-OutDir", $workerDir, "-Name", $name,
                "-LifetimeFlag", $lifetimeFlag)
            $wp = Start-Process -FilePath "powershell" -ArgumentList $argList -WindowStyle Hidden -PassThru `
                -RedirectStandardOutput (Join-Path $workerDir "worker-$name.out.log") `
                -RedirectStandardError (Join-Path $workerDir "worker-$name.err.log")
            $workerProcs += [pscustomobject]@{ mode = $mode; name = $name; pid = $wp.Id }
            Log "负载子进程：$name PID=$($wp.Id)"
        }
    }
    ($workerProcs | ConvertTo-Json -Compress) | Set-Content -LiteralPath (Join-Path $outDir "worker-pids.json") -Encoding UTF8

    # ── README.txt（启动命令 / PID / 停止方式 —— 要求逐字写明） ─────────────
    $readme = @"
soak 运行说明（由 rust-proxy/scripts/soak.ps1 生成）
=====================================================
RunDir        : $RunDir
开始时间      : $($startedAt.ToString("yyyy-MM-dd HH:mm:ss"))
时长          : $DurationHours h（采样间隔 ${SampleSec}s）
exe           : $proxyExe（副本来源 = $Exe）
exe 字节/sha  : $(if ($NoProxy) { "N/A" } else { "$exeBytes / $exeHash" })
代理 PID      : $(if ($proxyProc) { $proxyProc.Id } else { "N/A（-NoProxy）" })
代理监听      : 127.0.0.1:$ProxyPort
mock 上游     : 127.0.0.1:$UpstreamPort（本控制器进程内托管）
负载          : $WorkerSpec（mixed=非流式+预检+text/plain；sse=逐帧长流；abort=中途断开）
断网节奏      : $(if ($BlackoutEverySec -gt 0) { "每 ${BlackoutEverySec}s 关掉上游 ${BlackoutSec}s（形态 = $BlackoutMode）" } else { '**本次关闭**（-BlackoutEverySec 0；由调用方选择，非"缺陷未修"——P1 半开池缺陷已在 `batch-rust-proxy-halfopen` 修复）' })
日志节流观察  : mixed 负载里 10% 请求带 Content-Type: text/plain（FR-39：同名 warn ≤ 1 条/小时）

启动命令（原样可复制；detached = Start-Process -WindowStyle Hidden）
---------------------------------------------------------------
powershell -NoProfile -ExecutionPolicy Bypass -File <rust-proxy>\scripts\soak.ps1 -RunDir "$RunDir" -DurationHours $DurationHours

停止方式
--------
① 正常收尾（推荐）：在 RunDir 下建一个 `stop.flag` 文件 —— 控制器下一次采样时收尾：
     New-Item -ItemType File -Path "$flagPath" -Force
   控制器会：写 summary.json → 关掉全部负载子进程 → 关掉代理 → 退出。
② 立即硬停（不写 summary）：按 PID 关（**严禁 /F /IM**）：
     Stop-Process -Id $(if ($proxyProc) { $proxyProc.Id } else { "<代理 PID>" }) -Force
     Stop-Process -Id <控制器 PID> -Force
   然后按 out/worker-pids.json 里的 PID 关掉各负载子进程。

读数
----
out/timeseries.csv      时间序列（每次采样一行；列名见文件首行）
out/summary.json        收尾汇总（RSS 峰值/斜率、句柄与线程斜率、崩溃标记）
out/soak-controller.log 控制器日志（含断网起止、启动冒烟、异常）
workers/worker-*.json   各负载子进程的累计计数
out/proxy-stdout.log    代理 stdout 的**脚本侧捕获**（本脚本 -RedirectStandardOutput 创建；程序只写继承句柄
                        ⇒ **不是程序写盘**。D12 后程序不写任何文件，这就是日志证据链的保留路径）
"@
    Set-Content -LiteralPath (Join-Path $RunDir "README.txt") -Value $readme -Encoding UTF8
    Log "README.txt 已落盘（PID/停止方式/读数位置）"

    # ── 采样 ───────────────────────────────────────────────────────────
    $cols = @("ts", "elapsed_s", "pid_alive", "rss_kb", "handles", "threads", "cpu_s",
        "log_bytes", "access_total", "access_2xx", "access_4xx", "access_5xx", "warn_lines",
        "error_lines", "plain_warn", "up_conns", "up_reqs", "up_keepalive_reuse", "up_sse_frames",
        "up_write_fails", "cli_ok", "cli_err", "cli_exc", "cli_abort", "cli_badacao", "worst_nonstream_ms",
        "probe_status", "probe_ms", "blackout", "note")
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [IO.File]::WriteAllText($csvPath, (($cols -join ",") + "`r`n"), $utf8NoBom)

    $logOffset = 0
    $cum = @{ access = 0; s2 = 0; s4 = 0; s5 = 0; warn = 0; err = 0; plain = 0 }
    $rxAccess = [regex]' -> (\d{3}) 耗时='
    $rxWarn = [regex]'\] WARN '
    $rxError = [regex]'\] ERROR '
    $rxPlain = [regex]'Content-Type: text/plain'
    $samples = New-Object System.Collections.Generic.List[object]
    $rssPeak = 0
    $nextBlackout = $startedAt.AddSeconds($BlackoutEverySec)
    $blackoutUntil = $null
    $blackoutProbeAt = $null
    $blackoutProbed = $true
    $blackouts = 0
    $script:lastProbeStatus = ""
    $script:lastProbeMs = 0
    $script:probe000 = 0
    $script:worstNonstreamAll = 0.0
    $deadline = $startedAt.AddHours($DurationHours)

    if ($BlackoutEverySec -le 0) { Log "⚠ 断网注入已关闭（-BlackoutEverySec 0）：本次 soak 不注入断网（调用方选择；P1「半开池复用挂死」已在 batch-rust-proxy-halfopen 修复，本项并非因缺陷未修而关闭）" }
    while ($true) {
        if (Test-Path -LiteralPath $flagPath) { Log "检测到 stop.flag ⇒ 收尾"; break }
        if ((Get-Date) -ge $deadline) { Log "到达 $DurationHours h 时长 ⇒ 收尾"; break }

        # 断网/重连
        $now = Get-Date
        if ($BlackoutEverySec -gt 0 -and $null -eq $blackoutUntil -and $now -ge $nextBlackout) {
            if ($mockProc) {
                # 真"上游进程级消失"（事故形态）：按 PID 杀子进程 ⇒ 端口与全部连接随之释放
                try { Stop-Process -Id $mockProc.Id -Force -ErrorAction Stop; Log "上游进程已按 PID 杀掉（PID=$($mockProc.Id)；实为 $($BlackoutMode) 形态）" }
                catch { Log "⚠ 杀 mock 子进程失败：$($_.Exception.Message)" }
                $mockProc = $null
                if (-not (Wait-UpstreamPortDown 8)) { Log "⚠ 上游端口在 8 s 内仍未释放" }
            }
            $blackoutUntil = $now.AddSeconds($BlackoutSec)
            $blackoutProbeAt = $now.AddSeconds(5)
            $blackoutProbed = $false
            $blackouts++
            Log "上游断网开始（第 $blackouts 次；${BlackoutSec}s）"
        }
        # 断网期的**转发探针**：上游没了以后，"新请求"应当**快速**得到 502（FR-22 / RK6 无半死状态）。
        # 若这里读到 000（--max-time 打到），就是"池中半开连接把请求挂死"的现场证据 —— 必须记账，不能静默。
        # ⚠ 探针必须**在断网窗内**发（判据 §6.5-2）；采样节拍 30 s > 断网 20 s ⇒ 不能在"下一次采样"里补发
        #   （那样窗早已结束，探针永远不会跑 —— B3 首跑实测踩到：probe_status 列恒空）。
        #   做法：断网一开始就地睡到 +5 s（上限：窗长-1）→ 发探针 → 把剩余窗时睡完 → 在**同一轮**内重连。
        if ($null -ne $blackoutUntil) {
            $lead = [Math]::Max(1, [Math]::Min(5, $BlackoutSec - 2))
            Start-Sleep -Seconds $lead
            if ((Get-Date) -lt $blackoutUntil) {
                $pcode = & curl.exe -s -o NUL -w "%{http_code}|%{time_total}" --max-time $ProbeMaxSec -X POST -H "Content-Type: application/json" --data "@$(Join-Path $outDir 'body-nonstream.json')" "http://127.0.0.1:$ProxyPort/v1/chat/completions"
                $pparts = "$pcode".Split("|")
                $script:lastProbeStatus = $pparts[0]
                $script:lastProbeMs = [int]([double]$pparts[1] * 1000)
                if ($script:lastProbeStatus -eq "000") { $script:probe000++ }
                Log "断网期转发探针（判据 §6.5-2：禁止 000；期望 502/504/200/204）：$pcode"
            }
            $remain = [int](($blackoutUntil - (Get-Date)).TotalSeconds)
            if ($remain -gt 0) { Start-Sleep -Seconds $remain }
            $now = Get-Date
            # 重连 = 起一个**全新子进程**（全新 socket），并做连通性校验（不通过即重试一次）
            $ok = $false
            for ($re = 1; $re -le 2 -and -not $ok; $re++) {
                $mockProc = Start-MockChild
                $ok = Wait-UpstreamPort 12
                if (-not $ok) {
                    Log "⚠ 上游重连后端口未就绪（第 $re 次；PID=$($mockProc.Id)）⇒ 重试"
                    try { Stop-Process -Id $mockProc.Id -Force } catch { }
                    $mockProc = $null
                }
            }
            if ($ok) { Log "上游已重连并通过连通性校验（断网结束；新 mock 子进程 PID=$($mockProc.Id)）" } else { Log "⚠⚠ 上游重连后仍不可达（后续请求会持续 5xx；读数须按上游未恢复解读）" }
            $blackoutUntil = $null
            $nextBlackout = $now.AddSeconds($BlackoutEverySec)
        }

        # 日志增量计数（**换源**：读脚本侧捕获的 stdout `$CapturePath`，不再是"程序写出的日志文件"；
        # 正则与累计口径逐字不变 ⇒ 与 2 h soak 的旧读数同口径）。捕获文件被重建时 offset 归零。
        $logBytes = 0
        try {
            if (Test-Path -LiteralPath $CapturePath) {
                $fi = Get-Item -LiteralPath $CapturePath
                $logBytes = $fi.Length
                if ($logBytes -lt $logOffset) { $logOffset = 0 }
                if ($logBytes -gt $logOffset) {
                    $fs = [IO.File]::Open($CapturePath, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::ReadWrite)
                    try {
                        [void]$fs.Seek($logOffset, [IO.SeekOrigin]::Begin)
                        $buf = New-Object byte[] ($logBytes - $logOffset)
                        $read = $fs.Read($buf, 0, $buf.Length)
                        $chunk = [Text.Encoding]::UTF8.GetString($buf, 0, $read)
                    } finally { $fs.Dispose() }
                    $logOffset = $logBytes
                    foreach ($m in $rxAccess.Matches($chunk)) {
                        $cum.access++
                        $s = $m.Groups[1].Value
                        if ($s[0] -eq '2' -or $s[0] -eq '3') { $cum.s2++ } elseif ($s[0] -eq '4') { $cum.s4++ } elseif ($s[0] -eq '5') { $cum.s5++ }
                    }
                    $cum.warn += $rxWarn.Matches($chunk).Count
                    $cum.err += $rxError.Matches($chunk).Count
                    $cum.plain += $rxPlain.Matches($chunk).Count
                }
            }
        } catch { Log "日志计数异常：$($_.Exception.Message)" }

        # 进程读数
        $alive = 0; $rss = 0; $handles = 0; $threads = 0; $cpu = 0.0
        $note = ""
        $p = Get-Process -Id $proxyProc.Id -ErrorAction SilentlyContinue
        if ($p) {
            $alive = 1
            $rss = [math]::Round($p.WorkingSet64 / 1024)
            $handles = $p.HandleCount
            $threads = $p.Threads.Count
            $cpu = [math]::Round($p.TotalProcessorTime.TotalSeconds, 2)
            if ($rss -gt $rssPeak) { $rssPeak = $rss }
        } else {
            if ($proxyProc.HasExited) {
                $crash = $true
                $crashNote = "代理进程已退出（exit=$($proxyProc.ExitCode)）"
            } else {
                $crash = $true
                $crashNote = "Get-Process 取不到代理进程（非正常）"
            }
            $note = $crashNote
            Log "崩溃/异常：$crashNote"
        }
        if ($null -ne $blackoutUntil) { $note = ($note + " blackout").Trim() }

        Read-MockStats   # 子进程 mock 的统计回填（conns/reqs/reuse/sse/write_fails）

        # 客户端计数（各 worker 的 JSON 求和）
        $cOk = 0; $cErr = 0; $cExc = 0; $cAbort = 0; $cBad = 0; $cWorstNs = 0
        foreach ($wf in Get-ChildItem -LiteralPath $workerDir -Filter "worker-*.json" -File) {
            try {
                $j = Get-Content -LiteralPath $wf.FullName -Raw -Encoding UTF8 | ConvertFrom-Json
                $cOk += [int]$j.ok; $cErr += [int]$j.err; $cExc += [int]$j.exc; $cAbort += [int]$j.abort; $cBad += [int]$j.badacao
                if ($null -ne $j.worst_nonstream_ms -and [double]$j.worst_nonstream_ms -gt $cWorstNs) { $cWorstNs = [double]$j.worst_nonstream_ms }
            } catch { }
        }
        if ($cWorstNs -gt $script:worstNonstreamAll) { $script:worstNonstreamAll = $cWorstNs }

        $elapsed = [int]((Get-Date) - $startedAt).TotalSeconds
        $row = @(
            (Get-Date).ToString("s"), $elapsed, $alive, $rss, $handles, $threads, $cpu,
            $logBytes, $cum.access, $cum.s2, $cum.s4, $cum.s5, $cum.warn, $cum.err, $cum.plain,
            $upstream.ConnCount, $upstream.ReqCount, $upstream.KeepAliveReuse, $upstream.SseFrames,
            $upstream.WriteFails, $cOk, $cErr, $cExc, $cAbort, $cBad, [math]::Round($script:worstNonstreamAll),
            $script:lastProbeStatus, $script:lastProbeMs,
            $(if ($null -ne $blackoutUntil) { 1 } else { 0 }), $note
        ) -join ","
        [IO.File]::AppendAllText($csvPath, ($row + "`r`n"), $utf8NoBom)
        $samples.Add([pscustomobject]@{ elapsed = $elapsed; rss = $rss; handles = $handles; threads = $threads; s5 = $cum.s5; access = $cum.access })
        Log ("采样 t=${elapsed}s rss=${rss}KB handles=$handles threads=$threads access=$($cum.access) 5xx=$($cum.s5) warn=$($cum.warn) plain=$($cum.plain) up_conns=$($upstream.ConnCount) up_reqs=$($upstream.ReqCount) cli_ok=$cOk cli_err=$cErr cli_abort=$cAbort badacao=$cBad $note")

        if ($crash) { break }
        # 子进程存活检查（死了就重启一次并记账）
        foreach ($w in $workerProcs) {
            $wp = Get-Process -Id $w.pid -ErrorAction SilentlyContinue
            if (-not $wp) {
                Log "⚠ 负载子进程 $($w.name) 已退出（PID=$($w.pid)）—— 重启一次"
                $parts = $w.mode
                $argList = @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $workerScript,
                    "-Mode", $parts, "-ProxyPort", "$ProxyPort", "-OutDir", $workerDir, "-Name", $w.name,
                    "-LifetimeFlag", $lifetimeFlag)
                $np = Start-Process -FilePath "powershell" -ArgumentList $argList -WindowStyle Hidden -PassThru `
                    -RedirectStandardOutput (Join-Path $workerDir "worker-$($w.name).out.log") `
                    -RedirectStandardError (Join-Path $workerDir "worker-$($w.name).err.log")
                $w.pid = $np.Id
            }
        }
        Start-Sleep -Seconds $SampleSec
    }

    # ── 汇总 ───────────────────────────────────────────────────────────
    # [E13] 换源后的**证据链自检**（方案 §11-⑥ 的"任一项为空 / 缺失 ⇒ 不得 PASS"机器化）：
    # 五处读数里最关键的两位（捕获字节 / 访问行累计）任一为 0 ⇒ 捕获文件缺失或换源没接上，
    # 这种"静默降级"必须被显式喊出来（读数进 summary.json 的 evidence_chain_ok）。
    $evidenceChain = [ordered]@{
        log_bytes_now   = $logBytes
        log_bytes_ok    = ($logBytes -gt 0)
        access_total    = $cum.access
        access_total_ok = ($cum.access -gt 0)
        samples         = $samples.Count
        samples_ok      = ($samples.Count -ge 2)
    }
    $evidenceChainOk = ($evidenceChain["log_bytes_ok"] -and $evidenceChain["access_total_ok"] -and $evidenceChain["samples_ok"])
    if ($evidenceChainOk) {
        Log "证据链自检 PASS：捕获字节=$logBytes · access 累计=$($cum.access) · 采样点=$($samples.Count)"
    } else {
        Log "⚠⚠ 证据链自检 FAIL：捕获文件读数为空/0（log_bytes_now=$logBytes · access_total=$($cum.access) · samples=$($samples.Count)）—— 脚本侧捕获没接上 ⇒ 本批 soak 读数**不得**当证据（§11-⑥）"
    }

    function Get-Slope($points, [string]$field) {
        $n = $points.Count
        if ($n -lt 3) { return 0.0 }
        $sumX = 0.0; $sumY = 0.0; $sumXY = 0.0; $sumXX = 0.0
        foreach ($pt in $points) { $x = [double]$pt.elapsed; $y = [double]$pt.$field; $sumX += $x; $sumY += $y; $sumXY += $x * $y; $sumXX += $x * $x }
        $den = $n * $sumXX - $sumX * $sumX
        if ($den -eq 0) { return 0.0 }
        return ($n * $sumXY - $sumX * $sumY) / $den
    }
    $rssSlopePerSec = Get-Slope $samples "rss"
    $handleSlopePerSec = Get-Slope $samples "handles"
    $threadSlopePerSec = Get-Slope $samples "threads"
    $summary = [ordered]@{
        run_dir = $RunDir
        started = $startedAt.ToString("s")
        finished = (Get-Date).ToString("s")
        duration_hours_actual = [math]::Round(((Get-Date) - $startedAt).TotalHours, 3)
        exe = $(if ($NoProxy) { "" } else { $proxyExe })
        exe_bytes = $(if ($NoProxy) { 0 } else { $exeBytes })
        exe_sha256 = $(if ($NoProxy) { "" } else { $exeHash })
        proxy_pid = $(if ($proxyProc) { $proxyProc.Id } else { 0 })
        samples = $samples.Count
        crash = $crash
        crash_note = $crashNote
        rss_peak_kb = $rssPeak
        rss_slope_kb_per_sec = [math]::Round($rssSlopePerSec, 6)
        rss_slope_mb_per_24h = [math]::Round($rssSlopePerSec * 86400 / 1024, 3)
        handles_slope_per_24h = [math]::Round($handleSlopePerSec * 86400, 3)
        threads_slope_per_24h = [math]::Round($threadSlopePerSec * 86400, 3)
        access_total = $cum.access
        access_2xx_3xx = $cum.s2
        access_4xx = $cum.s4
        access_5xx = $cum.s5
        warn_lines = $cum.warn
        error_lines = $cum.err
        plain_content_type_warns = $cum.plain
        upstream_conns = $upstream.ConnCount
        upstream_requests = $upstream.ReqCount
        upstream_keepalive_reuse = $upstream.KeepAliveReuse
        upstream_sse_frames = $upstream.SseFrames
        upstream_write_fails = $upstream.WriteFails
        blackouts = $blackouts
        worst_nonstream_ms = [math]::Round($script:worstNonstreamAll)
        probe_000_count = $script:probe000
        last_probe = "$($script:lastProbeStatus)|$($script:lastProbeMs)ms"
        blackout_mode = $BlackoutMode
        blackout_every_sec = $BlackoutEverySec
        log_bytes_now = $logBytes
        evidence_chain_ok = $evidenceChainOk
        evidence_chain = $evidenceChain
        log_lines_rollover_dropped = "捕获文件被重建时的尾巴未计入（口径：增量读同一捕获文件；长度回退 ⇒ offset 归零）"
    }
    ($summary | ConvertTo-Json -Depth 4) | Set-Content -LiteralPath (Join-Path $outDir "summary.json") -Encoding UTF8
    Log "summary.json 已落盘：$($samples.Count) 个采样点"
} finally {
    # 收尾：先撤 lifetime flag（子进程自行退出），再按 PID 关（**绝不 /F /IM**）
    $lifetimeFlag = Join-Path $RunDir "controller.alive"
    if (Test-Path -LiteralPath $lifetimeFlag) { Remove-Item -LiteralPath $lifetimeFlag -Force }
    foreach ($w in $workerProcs) {
        try { Stop-Process -Id $w.pid -Force -ErrorAction Stop; Log "已关闭负载子进程 $($w.name)（PID=$($w.pid)）" } catch { Log "负载子进程 $($w.name) 已不在（PID=$($w.pid)）" }
    }
    if ($mockProc) {
        try { Stop-Process -Id $mockProc.Id -Force -ErrorAction Stop; Log "已关闭 mock 子进程（PID=$($mockProc.Id)）" } catch { Log "mock 子进程已不在（PID=$($mockProc.Id)）" }
    }
    try { $upstream.StopListener() } catch { }
    if ($proxyProc) {
        try { Stop-Process -Id $proxyProc.Id -Force -ErrorAction Stop; Log "已关闭代理（PID=$($proxyProc.Id)）" } catch { Log "代理已不在（PID=$($proxyProc.Id)）" }
    }
    Log "soak 控制器退出"
}
