#requires -Version 5.1
<#
verify-no-write.ps1 —— D12「零文件 I/O」的**只读面**常驻回归自检（方案 §6.7 = D12-2 的细则）。

一句话：把 exe 放进**只读目录**跑一遍**全功能冒烟**；再在**不去权限的同一目录**跑一遍同一冒烟；
两臂都必须全绿 ⇒ 证明运行期**不存在写入需求**（而不是"写失败后静默降级"）。

## 装置取法（K4：取法必须"可行且可复现"，并写在本头注释里；含失败时的替代路径）
  1. `<OutDir>\ro-dir\` 清空重建 → 复制被测 exe 进去（**先复制、后去权限** —— 反了连复制都进不去）；
  2. `icacls <dir> /deny "*S-1-1-0:(W,D)"`（**Everyone，按 SID 写**，避免账户名本地化/解析差异）；
  3. 收尾 `icacls <dir> /remove:d "*S-1-1-0"` 还原 → 删目录。
  · **★ 实测更正（本次实施期现取，2026-09-19 20:3x）**：K4 推荐的**按用户**写法
    `icacls <dir> /deny "<用户>:(W,D)"` 在**本机不生效** —— ACL 里确实出现了 `(DENY)(W,D)` 项，
    但同一用户仍能 `[IO.File]::WriteAllText` / `New-Item` / `cmd >` 建文件（`acl-exitcode-probe.ps1` 留档
    `out/s9/50-acl-exitcode-probe.txt`）；改成**按 Everyone（`*S-1-1-0`）**后写操作**确实被拒**
    （`out/s9/51-acl2-exitcode-probe.txt`）⇒ 本脚本采用 Everyone 写法。
  · 替代路径（若将来 Everyone 写法也失效）：① 只读共享（共享权限 Read，NTFS 不动）；② 用低权限账户跑 exe。
    两条都须重跑下面的"装置自证"。
  · **装置自证（本脚本内置的负例）**：去权限之后，以**同一用户**在该目录里写文件**必须失败**
    —— 否则"只读"只是摆设、两臂 PASS 也就没有意义（§6.7 第 4 条的"判据能咬"；K3：测试脚本里的
    写尝试属测试专用注入，**交付二进制里不含任何注入路径**）。

## 两臂 × 全功能冒烟（§6.7 第 2/3 条）
  · 臂 = 只读臂（已 deny）/ 对照臂（未 deny，先跑）；同一目录、同一命令、同一判据；
  · 每臂两项：
    a) CLI 臂：`--no-gui --upstream http://127.0.0.1:<未监听口> --listen-port <p> --response-head-timeout-ms 3000` ⇒
       起服务 ✓ · 预检 `OPTIONS`+`Origin`+`ACRM` = **204 + ACAO** ✓ · **上游不可达 ⇒ 502 + 结构化原因 JSON** ✓ ·
       再起 mock 上游 ⇒ **转发一条真实请求 = 200 + mock 体** ✓（顺序有意：先测"不可达"再起 mock，
       避免"池里留着刚被关掉的连接"把 502 变成 120 s 等待 —— 那是**装置形状**问题，不是被测面）；
    b) GUI 臂：`--start-paused --close-action exit …`（经一个 `-Wait` 形态的小助手启动，见下）⇒
       主窗出现 ⇒ 点「启动/停止」（`WM_COMMAND` 1001）⇒ 服务起得来 ✓ / 停得掉 ✓；
       点「复制日志到剪贴板」（`WM_COMMAND` 1007）⇒ `Get-Clipboard` **非空**
       （**正例断言**：防"因为写不了文件就把内存日志也静默关掉"）⇒ `WM_CLOSE` ⇒ **退出码 0** ✓
       （`ExitCode` 只能从 `Start-Process -Wait` 形态读到 —— 实测 `-PassThru` 不 `-Wait` 时该属性为空，
       故 exe 由 `gui-exitcode-helper.ps1`（运行时生成在 OutDir，**不是交付脚本**）以 `-Wait` 启动并回填退出码）。
  · 覆盖范围：起服务 / 预检 / 转发 / 502 / UI 起停 / 退出六项；**不覆盖** Win7 实机上的只读卷
    （UNC 只读共享）行为与"低权限账户"跑法 —— 登记为 `?`（Phase 5 用户侧）。

## 口径
  · 捕获文件（`-RedirectStandardOutput` / 助手内的重定向）由**本脚本/助手**创建 ⇒ 不是"程序写文件"；
    全部落在 `<OutDir>`（可写），**不**落进只读目录（§2.3 末条 / §11-①）。
  · 收尾一律**按 PID** 关（`Stop-Process -Id`；**严禁 `/F /IM`**）。

用法：
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts/verify-no-write.ps1 [-Exe <path>]
退出码：0 = 两臂全绿 + 装置自证成立（PASS）；1 = 有判据 FAIL；2 = 装置/环境失败。
#>
param(
    [string]$Exe = "",
    [int]$Port = 8063,
    [int]$UpstreamPort = 8064,
    [string]$OutDir = "",
    [switch]$KeepWorkDir
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
# 证据文件里的中文不要变成乱码（重定向捕获按控制台代码页写 ⇒ 显式钉 UTF-8；本机无控制台时 setter 可能抛，忽略）
try { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 } catch { }

$repoRoot = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
if ([string]::IsNullOrWhiteSpace($Exe)) { $Exe = Join-Path $repoRoot "target\x86_64-win7-windows-msvc\release\azusa-local-proxy.exe" }
if ([string]::IsNullOrWhiteSpace($OutDir)) { $OutDir = Join-Path (Join-Path $repoRoot "..\..\shared\tmp\rust-proxy-p6\out") "no-write" }
$Exe = [IO.Path]::GetFullPath($Exe)
$OutDir = [IO.Path]::GetFullPath($OutDir)
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$workDir = Join-Path $OutDir "ro-dir"
$logPath = Join-Path $OutDir "probe-no-write.log"
$readingsPath = Join-Path $OutDir "readings-no-write.txt"
$verdictPath = Join-Path $OutDir "verdict-no-write.txt"
$failures = New-Object System.Collections.Generic.List[string]
$readings = New-Object System.Collections.Generic.List[string]
$DENY_SID = "*S-1-1-0"      # Everyone（按 SID，避免账户名本地化/解析差异）

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

# ── 进程内 mock 上游（单请求/连接，返回固定 200 JSON）────────────────────
Add-Type @"
using System;
using System.Net;
using System.Net.Sockets;
using System.Text;
using System.Threading;

public class RZNoWriteMock {
  private TcpListener _listener;
  private Thread _thread;
  private volatile bool _running;
  public int ConnCount = 0;
  public int ReqCount = 0;

  public bool Start(int port) {
    try {
      _listener = new TcpListener(IPAddress.Loopback, port);
      _listener.Start();
      _running = true;
      _thread = new Thread(AcceptLoop);
      _thread.IsBackground = true;
      _thread.Start();
      return true;
    } catch { return false; }
  }

  private void AcceptLoop() {
    while (_running) {
      TcpClient client = null;
      try { client = _listener.AcceptTcpClient(); } catch { return; }
      Interlocked.Increment(ref ConnCount);
      Thread worker = new Thread(delegate() { Serve(client); });
      worker.IsBackground = true;
      worker.Start();
    }
  }

  private void Serve(TcpClient client) {
    try {
      using (client) {
        NetworkStream stream = client.GetStream();
        byte[] buf = new byte[16384];
        int n = stream.Read(buf, 0, buf.Length);
        if (n <= 0) { return; }
        Interlocked.Increment(ref ReqCount);
        string body = "{\"ok\":true,\"from\":\"mock\"}";
        string head = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: "
          + Encoding.UTF8.GetByteCount(body)
          + "\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n";
        byte[] outb = Encoding.UTF8.GetBytes(head + body);
        stream.Write(outb, 0, outb.Length);
        stream.Flush();
      }
    } catch { }
  }

  public void Stop() {
    _running = false;
    try { if (_listener != null) { _listener.Stop(); } } catch { }
  }
}
"@

Add-Type @"
using System;
using System.Runtime.InteropServices;
public class RZNoWriteWin {
  [DllImport("user32.dll", CharSet=CharSet.Unicode, SetLastError=true)] public static extern IntPtr FindWindowW(string cls, IntPtr win);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool PostMessageW(IntPtr h, uint msg, IntPtr w, IntPtr l);
  [DllImport("user32.dll", SetLastError=true)] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
}
"@

$MAIN_CLASS = "AzusaAI.LocalProxy.MainWindow"
$WM_COMMAND = 0x0111
$WM_CLOSE = 0x0010
$IDC_TOGGLE_SERVICE = 1001
$IDC_COPY_LOGS = 1007

# ── 退出码助手（运行时生成；`-PassThru` 不 `-Wait` 读不到 ExitCode ⇒ 用它以 `-Wait` 形态代跑）──
$helperPath = Join-Path $OutDir "gui-exitcode-helper.ps1"
$helperBody = @'
#requires -Version 5.1
# 由 verify-no-write.ps1 在运行期生成（属**测试助手**，不是交付脚本）：以 `-Wait` 形态启动被测 exe，
# 并把退出码回填到文件（`Start-Process -PassThru` 不用 `-Wait` 时 `ExitCode` 读不到 —— 实测）。
param([Parameter(Mandatory = $true)][string]$Spec)
Set-StrictMode -Version Latest
$s = Get-Content -LiteralPath $Spec -Raw -Encoding UTF8 | ConvertFrom-Json
$p = Start-Process -FilePath $s.exe -WorkingDirectory $s.cwd -PassThru -Wait `
    -ArgumentList @($s.args) `
    -RedirectStandardOutput $s.stdout -RedirectStandardError $s.stderr
Set-Content -LiteralPath $s.exit_file -Value $p.ExitCode -Encoding ASCII
'@
Set-Content -LiteralPath $helperPath -Value $helperBody -Encoding UTF8

function Get-ParentPid([int]$pidValue) {
    $p = Get-CimInstance Win32_Process -Filter ("ProcessId = " + $pidValue) -ErrorAction SilentlyContinue
    if ($null -eq $p) { return -1 }
    return [int]$p.ParentProcessId
}

function Assert-WindowOwner([IntPtr]$hwnd, [int]$expectedPid, [string]$label) {
    $owner = 0
    [void][RZNoWriteWin]::GetWindowThreadProcessId($hwnd, [ref]$owner)
    if ($owner -ne $expectedPid) {
        throw ("窗口归属校验失败：$label 句柄属于 PID {0}，本次启动（助手子进程）PID = {1}（拒绝出结论）" -f $owner, $expectedPid)
    }
}

function Wait-MainWindow([int]$expectedPid, [int]$timeoutSec) {
    $deadline = (Get-Date).AddSeconds($timeoutSec)
    while ((Get-Date) -lt $deadline) {
        $h = [RZNoWriteWin]::FindWindowW($MAIN_CLASS, [IntPtr]::Zero)
        if ($h -ne [IntPtr]::Zero) {
            $owner = 0
            [void][RZNoWriteWin]::GetWindowThreadProcessId($h, [ref]$owner)
            if ($owner -eq $expectedPid) { return $h }
        }
        Start-Sleep -Milliseconds 300
    }
    return [IntPtr]::Zero
}

function Test-TcpPort([int]$port, [int]$timeoutMs) {
    try {
        $c = New-Object System.Net.Sockets.TcpClient
        $iar = $c.BeginConnect("127.0.0.1", $port, $null, $null)
        if (-not $iar.AsyncWaitHandle.WaitOne($timeoutMs)) { $c.Close(); return $false }
        $c.EndConnect($iar); $c.Close(); return $true
    } catch { return $false }
}

function Wait-Port([int]$port, [int]$timeoutSec, [bool]$expectedUp) {
    $deadline = (Get-Date).AddSeconds($timeoutSec)
    while ((Get-Date) -lt $deadline) {
        if ((Test-TcpPort $port 600) -eq $expectedUp) { return $true }
        Start-Sleep -Milliseconds 300
    }
    return $false
}

function Stop-ByPid([System.Diagnostics.Process]$proc, [string]$label) {
    if (-not $proc) { return }
    try { Stop-Process -Id $proc.Id -Force -ErrorAction Stop; Log "已按 PID 关闭 $label（PID=$($proc.Id)）" }
    catch { Log "$label 已不在（PID=$($proc.Id)）" }
}

function Get-HttpStatus([string]$headersText) {
    foreach ($line in ($headersText -split "`r?`n")) {
        if ($line -match "^HTTP/[0-9.]+ (\d{3})") { return $Matches[1] }
    }
    return ""
}

# ── 全功能冒烟（一臂 = 两项）─────────────────────────────────────────────
function Invoke-Smoke([string]$arm, [string]$exePath, [string]$runCwd, [int]$port, [int]$upstreamPort) {
    Log "=== 冒烟臂：$arm（exe=$exePath；cwd=$runCwd）==="
    $capture = Join-Path $OutDir "nowrite-$arm-stdout.log"
    $captureErr = Join-Path $OutDir "nowrite-$arm-stderr.log"
    foreach ($f in @($capture, $captureErr)) { if (Test-Path -LiteralPath $f) { Remove-Item -LiteralPath $f -Force } }
    $bodyPath = Join-Path $OutDir "nowrite-body.json"
    Set-Content -LiteralPath $bodyPath -Value '{"id":"no-write-smoke","stream":false}' -Encoding ASCII

    $cliArgs = @(
        "--no-gui",
        "--listen-host", "127.0.0.1",
        "--listen-port", "$port",
        "--port-fallback", "none",
        "--upstream", "http://127.0.0.1:$upstreamPort",
        "--response-head-timeout-ms", "3000",
        "--log-level", "info"
    )
    $proc = $null
    try {
        $proc = Start-Process -FilePath $exePath -WorkingDirectory $runCwd -PassThru -ArgumentList $cliArgs `
            -RedirectStandardOutput $capture -RedirectStandardError $captureErr
        Log "[$arm] CLI 臂 PID=$($proc.Id)"
        if (-not (Wait-Port $port 20 $true)) { throw "[$arm] 20 s 内 $port 未 LISTENING" }
        Note "$arm 起服务 ✓（127.0.0.1:$port LISTENING）"

        # 预检：必须 204 + ACAO（把 curl 的头输出按行拼回，`^` 锚才有效）
        $pfRaw = (@(& curl.exe -s -D - -o NUL --max-time 5 -X OPTIONS -H "Origin: null" -H "Access-Control-Request-Method: POST" "http://127.0.0.1:$port/v1/chat/completions") -join "`n")
        $pfCode = Get-HttpStatus $pfRaw
        $pfAcao = $pfRaw -match "(?im)^access-control-allow-origin:"
        Note "$arm 预检：status=$pfCode · ACAO=$pfAcao"
        if ($pfCode -ne "204") { Fail "[$arm] 预检不是 204（实测 $pfCode）" }
        if (-not $pfAcao) { Fail "[$arm] 预检响应缺 Access-Control-Allow-Origin" }

        # 上游不可达（此刻 mock **还没起**）⇒ 必须 502 + 结构化原因
        $downBody = Join-Path $OutDir "nowrite-down-body.txt"
        $downCode = "" + (& curl.exe -s -o $downBody -w "%{http_code}" --max-time 8 -X POST -H "Content-Type: application/json" --data "@$bodyPath" "http://127.0.0.1:$port/v1/chat/completions")
        $downText = ""
        if (Test-Path -LiteralPath $downBody) { $downText = (Get-Content -LiteralPath $downBody -Raw -Encoding UTF8) }
        Note "$arm 上游不可达：status=$downCode · body=$($downText.Trim())"
        if ($downCode -ne "502") { Fail "[$arm] 上游不可达不是 502（实测 $downCode）" }
        if ($downText -notmatch '"error"' -or $downText -notmatch '"type"') { Fail "[$arm] 502 响应体缺结构化原因（error.type/message；实测 $downText）" }

        # 起 mock ⇒ 转发一条真实请求 = 200 + mock 体
        $mock = New-Object RZNoWriteMock
        if (-not $mock.Start($upstreamPort)) { throw "[$arm] mock 上游无法监听 127.0.0.1:$upstreamPort" }
        Start-Sleep -Milliseconds 400
        $fwdBody = Join-Path $OutDir "nowrite-fwd-body.txt"
        $fwdCode = "" + (& curl.exe -s -o $fwdBody -w "%{http_code}" --max-time 8 -X POST -H "Content-Type: application/json" --data "@$bodyPath" "http://127.0.0.1:$port/v1/chat/completions")
        $fwdText = ""
        if (Test-Path -LiteralPath $fwdBody) { $fwdText = (Get-Content -LiteralPath $fwdBody -Raw -Encoding UTF8) }
        Note "$arm 转发：status=$fwdCode · body=$($fwdText.Trim())"
        if ($fwdCode -ne "200") { Fail "[$arm] 转发真实请求不是 200（实测 $fwdCode）" }
        if ($fwdText -notmatch "from" -or $fwdText -notmatch "mock") { Fail "[$arm] 转发响应体不是 mock 的体（实测 $fwdText）" }
        $mock.Stop()

        $capBytes = 0
        if (Test-Path -LiteralPath $capture) { $capBytes = (Get-Item -LiteralPath $capture).Length }
        Note "$arm CLI 臂 stdout 捕获字节 = $capBytes"
        if ($capBytes -le 0) { Fail "[$arm] CLI 臂重定向捕获为空（D6③ 重定向通道没工作）" }
    } catch {
        Fail "[$arm] CLI 臂异常：$($_.Exception.Message)"
    } finally {
        Stop-ByPid $proc "$arm CLI 臂"
        Start-Sleep -Milliseconds 500
    }

    # ── GUI 臂：UI 起停 + 内存日志可导出 + 正常退出码 0 ────────────────────
    $guiCapture = Join-Path $OutDir "nowrite-$arm-gui-stdout.log"
    $guiErr = Join-Path $OutDir "nowrite-$arm-gui-stderr.log"
    $exitFile = Join-Path $OutDir "nowrite-$arm-gui-exitcode.txt"
    $specPath = Join-Path $OutDir "nowrite-$arm-gui-spec.json"
    foreach ($f in @($guiCapture, $guiErr, $exitFile, $specPath)) { if (Test-Path -LiteralPath $f) { Remove-Item -LiteralPath $f -Force } }
    $mock2 = New-Object RZNoWriteMock
    [void]$mock2.Start($upstreamPort)
    $guiArgs = @(
        "--start-paused",
        "--listen-host", "127.0.0.1",
        "--listen-port", "$port",
        "--port-fallback", "none",
        "--upstream", "http://127.0.0.1:$upstreamPort",
        "--response-head-timeout-ms", "3000",
        "--close-action", "exit",
        "--log-level", "info"
    )
    $helper = $null
    try {
        [ordered]@{ exe = $exePath; cwd = $runCwd; args = $guiArgs; stdout = $guiCapture; stderr = $guiErr; exit_file = $exitFile } |
            ConvertTo-Json -Depth 3 | Set-Content -LiteralPath $specPath -Encoding UTF8
        $helper = Start-Process -FilePath "powershell" -PassThru -ArgumentList @(
            "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $helperPath, "-Spec", $specPath)
        Log "[$arm] GUI 臂助手 PID=$($helper.Id)（助手内以 -Wait 启动 exe）"
        Start-Sleep -Milliseconds 1500
        $guiPid = -1
        $deadline = (Get-Date).AddSeconds(25)
        while ((Get-Date) -lt $deadline -and $guiPid -lt 0) {
            $hwndProbe = [RZNoWriteWin]::FindWindowW($MAIN_CLASS, [IntPtr]::Zero)
            if ($hwndProbe -ne [IntPtr]::Zero) {
                $owner = 0
                [void][RZNoWriteWin]::GetWindowThreadProcessId($hwndProbe, [ref]$owner)
                if ((Get-ParentPid $owner) -eq $helper.Id) { $guiPid = $owner }
            }
            if ($guiPid -lt 0) { Start-Sleep -Milliseconds 300 }
        }
        if ($guiPid -lt 0) { throw "[$arm] 25 s 内主窗未出现（或不是本助手起的进程）" }
        $hwnd = [RZNoWriteWin]::FindWindowW($MAIN_CLASS, [IntPtr]::Zero)
        Assert-WindowOwner $hwnd $guiPid "$arm 主窗"
        Note "$arm 主窗句柄 = 0x$($hwnd.ToInt64().ToString('X')) · exe PID = $guiPid（父 = 助手 $($helper.Id)）"

        if ((Test-TcpPort $port 500)) { Fail "[$arm] --start-paused 下 $port 竟已在监听（服务没有被暂停）" }
        else { Note "$arm --start-paused 生效 ✓（$port 未监听）" }

        [void][RZNoWriteWin]::PostMessageW($hwnd, $WM_COMMAND, [IntPtr]$IDC_TOGGLE_SERVICE, [IntPtr]::Zero)
        if (-not (Wait-Port $port 20 $true)) { Fail "[$arm] 点「启动」后 20 s 内 $port 未 LISTENING" }
        else { Note "$arm UI 起服务 ✓（点「启动/停止」按钮 ⇒ $port LISTENING）" }

        try { Set-Clipboard -Value "before-copy-logs" } catch { }
        [void][RZNoWriteWin]::PostMessageW($hwnd, $WM_COMMAND, [IntPtr]$IDC_COPY_LOGS, [IntPtr]::Zero)
        Start-Sleep -Milliseconds 1200
        $clip = ""
        try { $clip = "" + (Get-Clipboard -Raw) } catch { $clip = "" }
        $clipHasLog = $clip -match "azusa|AzusaAI|启动|监听|服务"
        Note "$arm 「复制日志到剪贴板」：长度=$($clip.Length) · 含日志特征=$clipHasLog"
        if ($clip.Length -le 0 -or -not $clipHasLog) { Fail "[$arm] 剪贴板里没有内存日志文本 ⇒ 内存日志环可能被静默关闭（D12 判据 5）" }

        [void][RZNoWriteWin]::PostMessageW($hwnd, $WM_COMMAND, [IntPtr]$IDC_TOGGLE_SERVICE, [IntPtr]::Zero)
        if (-not (Wait-Port $port 15 $false)) { Fail "[$arm] 点「停止」后 15 s 内 $port 仍 LISTENING" }
        else { Note "$arm UI 停服务 ✓" }

        [void][RZNoWriteWin]::PostMessageW($hwnd, $WM_CLOSE, [IntPtr]::Zero, [IntPtr]::Zero)
        if (-not $helper.WaitForExit(25000)) { Fail "[$arm] WM_CLOSE 后 25 s 内助手（= exe）未退出" }
        else {
            $code = ""
            if (Test-Path -LiteralPath $exitFile) { $code = (Get-Content -LiteralPath $exitFile -Raw -Encoding ASCII).Trim() }
            Note "$arm WM_CLOSE 退出：exit_code=[$code]"
            if ($code -ne "0") { Fail "[$arm] 正常退出码应为 0（实测 [$code]）" }
        }
    } catch {
        Fail "[$arm] GUI 臂异常：$($_.Exception.Message)"
    } finally {
        if ($helper -and -not $helper.HasExited) { try { Stop-Process -Id $helper.Id -Force -ErrorAction Stop } catch { } }
        if ($guiPid -gt 0) { try { Stop-Process -Id $guiPid -Force -ErrorAction Stop } catch { } }
        try { $mock2.Stop() } catch { }
        Start-Sleep -Milliseconds 500
    }
}

# ── 前置 + 目录准备 ─────────────────────────────────────────────────────
if (-not (Test-Path -LiteralPath $Exe)) { Log "装置失败：exe 不存在 $Exe"; exit 2 }
$exeHash = (Get-FileHash -LiteralPath $Exe -Algorithm SHA256).Hash.ToLower()
$exeBytes = (Get-Item -LiteralPath $Exe).Length
Log "verify-no-write：exe=$Exe（$exeBytes B / sha256 $exeHash）"
Note "exe = $Exe"
Note "exe_bytes = $exeBytes"
Note "exe_sha256 = $exeHash"
Note "work_dir = $workDir"
Note "port = $Port / upstream_port = $UpstreamPort"
Note "装置取法 = icacls /deny ${DENY_SID}:(W,D)（Everyone 按 SID；K4 的按用户写法本机实测无效，见脚本头注释）"

if (Test-Path -LiteralPath $workDir) {
    & icacls.exe "$workDir" /remove:d $DENY_SID 2>&1 | Out-Null
    & icacls.exe "$workDir" /remove:d "$env:USERNAME" 2>&1 | Out-Null
    Remove-Item -LiteralPath $workDir -Recurse -Force
}
New-Item -ItemType Directory -Force -Path $workDir | Out-Null
$proxyCopy = Join-Path $workDir "azusa-local-proxy.exe"
Copy-Item -LiteralPath $Exe -Destination $proxyCopy -Force
Note "已复制 exe 进工作目录（复制**先于**去权限）"

try {
    # ── 对照臂（未去权限）──────────────────────────────────────────────
    Invoke-Smoke -arm "control" -exePath $proxyCopy -runCwd $workDir -port $Port -upstreamPort $UpstreamPort

    # ── 去权限（只读臂）───────────────────────────────────────────────
    $aclRaw = @(& icacls.exe "$workDir" /deny "$DENY_SID`:(W,D)" 2>&1)
    Note "icacls /deny $DENY_SID 输出：$($aclRaw -join ' | ')"
    $aclList = @(& icacls.exe "$workDir" 2>&1)
    foreach ($l in $aclList) { Note "ACL $l" }
    $denied = @($aclList | Where-Object { $_ -match "\(DENY\)" }).Count -gt 0
    if (-not $denied) { Fail "装置失败：icacls /deny 后 ACL 里看不到 (DENY) 项 ⇒ 只读臂不成立" }
    else { Note "只读臂装置生效：ACL 出现 DENY 项 ✓" }

    # 装置自证（负例）：同一用户在该目录里写文件必须失败
    $probeFile = Join-Path $workDir "probe-device-must-fail.txt"
    $probeBlocked = $false
    try {
        [IO.File]::WriteAllText($probeFile, "should-not-be-possible")
    } catch {
        $probeBlocked = $true
        Note "装置自证 ✓：写 $probeFile 被拒（$($_.Exception.GetType().Name)）"
    }
    if (-not $probeBlocked) {
        if (Test-Path -LiteralPath $probeFile) { Remove-Item -LiteralPath $probeFile -Force -ErrorAction SilentlyContinue }
        Fail "装置自证失败：去权限后仍能在该目录写文件 ⇒ 『只读』是摆设，只读臂的 PASS 无效"
    }

    Invoke-Smoke -arm "readonly" -exePath $proxyCopy -runCwd $workDir -port $Port -upstreamPort $UpstreamPort
} catch {
    Log "装置异常：$($_.Exception.Message)"
    $failures.Add("装置异常：$($_.Exception.Message)")
} finally {
    # 收尾：还原权限（否则目录删不掉），再按需清理
    $restore = @(& icacls.exe "$workDir" /remove:d $DENY_SID 2>&1)
    Log "已还原工作目录权限（remove:d $DENY_SID）：$($restore -join ' | ')"
    if (-not $KeepWorkDir) {
        if (Test-Path -LiteralPath $proxyCopy) { Remove-Item -LiteralPath $proxyCopy -Force -ErrorAction SilentlyContinue }
    }
}

$readings | Set-Content -LiteralPath $readingsPath -Encoding UTF8
if ($failures.Count -eq 0) {
    "PASS  （只读臂 + 对照臂全绿，且装置自证（负例）成立；exe $exeBytes B / sha256 $exeHash）" | Set-Content -LiteralPath $verdictPath -Encoding UTF8
    Log "结论：PASS（读数 $readingsPath）"
} else {
    ("FAIL  x{0}`n" -f $failures.Count) + ($failures -join "`n") | Set-Content -LiteralPath $verdictPath -Encoding UTF8
    Log ("结论：FAIL x{0}" -f $failures.Count)
    foreach ($f in $failures) { Log "  · $f" }
}
if ($failures.Count -eq 0) { exit 0 } else { exit 1 }
