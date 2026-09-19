#requires -Version 5.1
<#
window-icon-probe.ps1 —— 标题栏图标回归探针（主窗 + 设置窗 × 每个目标 exe 一份读数）。

用途（Phase 3 起每批回归；由来 = Phase 2 known issue NEW-1）：
  设置窗类的 `WNDCLASSW` 曾漏设 `hIcon` ⇒ 标题栏退化为 Windows 通用占位图标（数值面 =
  `GCLP_HICON=0` / `GCLP_HICONSM=0`）。本探针把"两个窗口的类图标都非零"变成可自动咬合的判据。

判据（全部必须成立；任一不成立 ⇒ exit 1）：
  A0 静态面 —— `src/ui/**` 里 `APP_ICON_RESOURCE_ID as *const u16` 只出现 **1** 次
     （资源 ID 真值只有一个来源；复制粘贴出第二份 = 失败，防"同一入口"被破坏）
  A1/A2 主窗 `GCLP_HICON` / `GCLP_HICONSM` 非零（WM_GETICON 仅记录，不作判据）
  A3/A4 设置窗 `GCLP_HICON` / `GCLP_HICONSM` 非零
  A5 视觉代理 —— 设置窗标题栏图标区（左上角 `32×20` DIP）"粉色像素"占比 ≥ `-MinPinkRatio`
     （应用图标 = 粉底白环；通用占位图标无色相 ⇒ 占比≈0。此判据只作**代理**：真图标是否美观
     仍由人读图；它咬的是"有没有退化成占位"。）

用法（两套目标各一次）：
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts/window-icon-probe.ps1 `
      -Tag win7 -OutDir ..\..\shared\tmp\rust-proxy-p3\out\r1\window-icon\win7
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts/window-icon-probe.ps1 `
      -Tag modern -Exe target\x86_64-pc-windows-msvc\release\azusa-local-proxy.exe `
      -OutDir ..\..\shared\tmp\rust-proxy-p3\out\r1\window-icon\modern
退出码：0 = PASS；1 = 判据 FAIL；2 = 装置/环境失败（exe 缺失、窗口未出现…）。
#>
param(
    [string]$Exe = "",
    [string]$OutDir = "",
    [string]$Tag = "probe",
    [string]$SourceDir = "",
    [double]$MinPinkRatio = 0.02,
    [int]$WaitWindowSec = 25,
    [switch]$KeepRunning
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing

$repoRoot = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
if ([string]::IsNullOrWhiteSpace($Exe)) { $Exe = Join-Path $repoRoot "target\x86_64-win7-windows-msvc\release\azusa-local-proxy.exe" }
if ([string]::IsNullOrWhiteSpace($SourceDir)) { $SourceDir = Join-Path $repoRoot "src\ui" }
if ([string]::IsNullOrWhiteSpace($OutDir)) { $OutDir = Join-Path $repoRoot "..\..\shared\tmp\rust-proxy-p3\out\window-icon\$Tag" }
$Exe = [IO.Path]::GetFullPath($Exe)
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$logPath = Join-Path $OutDir "probe-$Tag.log"
$failures = New-Object System.Collections.Generic.List[string]

function Log([string]$m) {
    $line = "[{0}] {1}" -f (Get-Date -Format "HH:mm:ss.fff"), $m
    Write-Host $line
    Add-Content -LiteralPath $logPath -Value $line -Encoding UTF8
}

Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class RZWI {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [DllImport("user32.dll", CharSet=CharSet.Unicode, SetLastError=true)] public static extern IntPtr FindWindowW(string cls, IntPtr win);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder sb, int max);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder sb, int max);
  [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [DllImport("user32.dll", SetLastError=true)] public static extern IntPtr SendMessageW(IntPtr hWnd, uint msg, IntPtr w, IntPtr l);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool PostMessageW(IntPtr hWnd, uint msg, IntPtr w, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr GetClassLongPtrW(IntPtr hWnd, int index);
  [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr ctx);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr hWnd);
  [DllImport("gdi32.dll")] public static extern IntPtr CreateCompatibleDC(IntPtr hdc);
  [DllImport("gdi32.dll")] public static extern IntPtr CreateCompatibleBitmap(IntPtr hdc, int w, int h);
  [DllImport("gdi32.dll")] public static extern IntPtr SelectObject(IntPtr hdc, IntPtr obj);
  [DllImport("gdi32.dll")] public static extern bool DeleteObject(IntPtr obj);
  [DllImport("gdi32.dll")] public static extern bool DeleteDC(IntPtr hdc);
  [DllImport("user32.dll")] public static extern IntPtr GetDC(IntPtr h);
  [DllImport("user32.dll")] public static extern int ReleaseDC(IntPtr h, IntPtr dc);
}
"@

try { [void][RZWI]::SetThreadDpiAwarenessContext([IntPtr](-4)) } catch { Log "SetThreadDpiAwarenessContext 不可用（继续）：$($_.Exception.Message)" }

# ── A0 静态面：资源 ID 真值来源唯一 ──────────────────────────────────────
function Test-SingleResourceIdSource {
    $hits = @()
    foreach ($f in Get-ChildItem -LiteralPath $SourceDir -Filter *.rs -File -Recurse) {
        $text = Get-Content -LiteralPath $f.FullName -Raw -Encoding UTF8
        if ($text -match 'APP_ICON_RESOURCE_ID as \*const u16') { $hits += $f.Name }
    }
    if ($hits.Count -eq 1) {
        Log ("A0 PASS  资源 ID 真值来源唯一（$($hits[0])）")
    } else {
        $failures.Add("A0 资源 ID 真值来源数 = $($hits.Count)（期望 1）：$($hits -join ', ')")
        Log ("A0 FAIL  出现 $($hits.Count) 处：$($hits -join ', ')")
    }
}

# ── 窗口读数 + 截图 ─────────────────────────────────────────────────────
function Save-Window([IntPtr]$hwnd, [string]$name) {
    $r = New-Object RZWI+RECT
    [void][RZWI]::GetWindowRect($hwnd, [ref]$r)
    $w = $r.Right - $r.Left; $h = $r.Bottom - $r.Top
    if ($w -le 0 -or $h -le 0) { throw "窗口矩形退化（$name）：$w x $h" }
    $screen = [RZWI]::GetDC([IntPtr]::Zero)
    $mem = [RZWI]::CreateCompatibleDC($screen)
    $bmp = [RZWI]::CreateCompatibleBitmap($screen, $w, $h)
    [void][RZWI]::SelectObject($mem, $bmp)
    $ok = [RZWI]::PrintWindow($hwnd, $mem, 2)
    $path = Join-Path $OutDir ("{0}.png" -f $name)
    if ($ok) {
        $img = [System.Drawing.Image]::FromHbitmap($bmp)
        $img.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
        $img.Dispose()
    }
    [void][RZWI]::SelectObject($mem, [IntPtr]::Zero)
    [void][RZWI]::DeleteObject($bmp)
    [void][RZWI]::DeleteDC($mem)
    [void][RZWI]::ReleaseDC([IntPtr]::Zero, $screen)
    if (-not $ok) { throw "PrintWindow 失败（$name）" }
    Log ("截图 {0}：{1}x{2} -> {3}" -f $name, $w, $h, $path)
    return $path
}

# 标题栏图标区「粉色像素」占比 = "真图标 vs 占位图标"的机械代理（A5）。
function Get-TitlebarPinkRatio([string]$pngPath, [double]$scale) {
    $img = [System.Drawing.Bitmap]::FromFile($pngPath)
    try {
        $rw = [Math]::Min($img.Width, [int](32 * $scale))
        $rh = [Math]::Min($img.Height, [int](20 * $scale))
        $pink = 0; $total = 0
        for ($y = 0; $y -lt $rh; $y++) {
            for ($x = 0; $x -lt $rw; $x++) {
                $p = $img.GetPixel($x, $y)
                $total++
                if (($p.R - $p.G) -gt 40 -and ($p.R - $p.B) -gt 20 -and $p.R -gt 150) { $pink++ }
            }
        }
        if ($total -eq 0) { return 0.0 }
        return [double]$pink / [double]$total
    } finally { $img.Dispose() }
}

function Get-IconReading([IntPtr]$hwnd, [string]$label) {
    # WM_GETICON: ICON_SMALL=0 / ICON_BIG=1；-14 = GCLP_HICON / -34 = GCLP_HICONSM
    $big = [RZWI]::SendMessageW($hwnd, 0x007F, [IntPtr]1, [IntPtr]::Zero)
    $small = [RZWI]::SendMessageW($hwnd, 0x007F, [IntPtr]0, [IntPtr]::Zero)
    $classBig = [IntPtr]::Zero; $classSmall = [IntPtr]::Zero
    $deadline = (Get-Date).AddSeconds(5)
    while ((Get-Date) -lt $deadline) {
        $classBig = [RZWI]::GetClassLongPtrW($hwnd, -14)
        $classSmall = [RZWI]::GetClassLongPtrW($hwnd, -34)
        if ($classBig -ne [IntPtr]::Zero -and $classSmall -ne [IntPtr]::Zero) { break }
        Start-Sleep -Milliseconds 250
    }
    $line = "{0}: GCLP_HICON=0x{1:X} GCLP_HICONSM=0x{2:X} WM_GETICON(big=0x{3:X} small=0x{4:X})" -f `
        $label, $classBig.ToInt64(), $classSmall.ToInt64(), $big.ToInt64(), $small.ToInt64()
    Log "读数 $line"
    return @{ Label = $label; ClassBig = $classBig; ClassSmall = $classSmall; Line = $line }
}

function Test-IconPair($reading, [string]$idBig, [string]$idSmall) {
    if ($reading.ClassBig -eq [IntPtr]::Zero) {
        $failures.Add("$idBig 失败 —— $($reading.Label) GCLP_HICON = 0（标题栏会显示系统占位图标）")
        Log "$idBig FAIL"
    } else { Log "$idBig PASS" }
    if ($reading.ClassSmall -eq [IntPtr]::Zero) {
        $failures.Add("$idSmall 失败 —— $($reading.Label) GCLP_HICONSM = 0")
        Log "$idSmall FAIL"
    } else { Log "$idSmall PASS" }
}

# ⚠ 窗口归属校验（Phase 3 审查 P2 加固项①）：`FindWindowW` 只看类名 —— 若机器上另有同类窗口
#    （前一次探针残留 / soak 实例 / 用户自己开的），会**读到别人的窗口**并给出假 PASS（实测踩过）。
#    ⇒ 每次拿到句柄，立即用 `GetWindowThreadProcessId` 校验它属于**本次刚启动的 PID**；不成立即 exit 2。
function Assert-WindowOwner([IntPtr]$hwnd, [int]$expectedPid, [string]$label) {
    $owner = 0
    [void][RZWI]::GetWindowThreadProcessId($hwnd, [ref]$owner)
    if ($owner -ne $expectedPid) {
        Log ("窗口归属校验失败：$label 句柄 0x{0:X} 属于 PID {1}，而本次启动的 PID = {2}（疑似读到别人的窗口 ⇒ 拒绝出结论）" -f $hwnd.ToInt64(), $owner, $expectedPid)
        Write-Host "❌ 窗口归属校验失败（exit 2）" -ForegroundColor Red
        exit 2
    }
    Log ("窗口归属校验通过：$label 属于 PID $owner（本次启动）")
}

function Wait-Class([string]$cls, [int]$timeoutSec) {
    $deadline = (Get-Date).AddSeconds($timeoutSec)
    while ((Get-Date) -lt $deadline) {
        $h = [RZWI]::FindWindowW($cls, [IntPtr]::Zero)
        if ($h -ne [IntPtr]::Zero) { return $h }
        Start-Sleep -Milliseconds 200
    }
    return [IntPtr]::Zero
}

# ── 主流程 ──────────────────────────────────────────────────────────────
if (-not (Test-Path -LiteralPath $Exe)) { Log "装置失败：exe 不存在 $Exe"; exit 2 }
$exeDir = Split-Path -Parent $Exe
# 探针不需要服务（避免占端口 / 干扰别的验证）：**不写任何配置文件**（D3 去配置文件 / D12 零文件 I/O
# —— 程序已不再读 exe 同目录的那份配置），改用 CLI 的 `--start-paused`（语义等价旧配置里那个
# 「启动即开始服务」开关，见 P6-S6/K13：它已随开机自启功能整体删除）。**脚本侧不生成任何文件到 exe
# 同目录**（旧版在这里写配置并把日志落到日志子目录 —— 两处习惯一并清除，逐字改法见方案 §11-⑥）。

Test-SingleResourceIdSource

$exeHash = (Get-FileHash -LiteralPath $Exe -Algorithm SHA256).Hash.ToLower()
$exeBytes = (Get-Item -LiteralPath $Exe).Length
Log "exe = $Exe（$exeBytes B / sha256 $exeHash）"

$proc = $null
try {
    $proc = Start-Process -FilePath $Exe -WorkingDirectory $exeDir -PassThru -ArgumentList @(
        "--start-paused",              # 只建窗口，不起服务（探针不需要监听端口）
        "--listen-port", "8010",       # 旧探针配置里的监听口（本探针不会真的绑定：服务处于暂停态）
        "--port-fallback", "none",     # 不试回退口，避免任何意外占用
        "--upstream", "http://127.0.0.1:8011",
        "--log-level", "warn"
    )
    Log "启动 PID=$($proc.Id)（Tag=$Tag）"
    $main = Wait-Class "AzusaAI.LocalProxy.MainWindow" $WaitWindowSec
    if ($main -eq [IntPtr]::Zero) { throw "主窗未出现（$WaitWindowSec s 超时）" }
    Assert-WindowOwner $main $proc.Id "主窗"
    Start-Sleep -Milliseconds 1200

    $dpi = 96
    try { $dpi = [RZWI]::GetDpiForWindow($main) } catch { }
    $scale = [double]$dpi / 96.0
    Log "主窗 hwnd=0x$($main.ToInt64().ToString('X')) DPI=$dpi scale=$scale"

    $mainPath = Save-Window $main "main-window"
    $mainRead = Get-IconReading $main "主窗"
    Test-IconPair $mainRead "A1" "A2"

    # 打开设置窗：WM_COMMAND / 1002（主窗 [设置] 按钮）
    [void][RZWI]::PostMessageW($main, 0x0111, [IntPtr]1002, [IntPtr]::Zero)
    $settings = Wait-Class "AzusaAI.LocalProxy.SettingsWindow" 10
    if ($settings -eq [IntPtr]::Zero) { throw "设置窗未出现（10 s 超时）" }
    Assert-WindowOwner $settings $proc.Id "设置窗"
    Start-Sleep -Milliseconds 1200
    Log "设置窗 hwnd=0x$($settings.ToInt64().ToString('X')) 可见=$([RZWI]::IsWindowVisible($settings))"

    $setPath = Save-Window $settings "settings-window"
    $setRead = Get-IconReading $settings "设置窗"
    Test-IconPair $setRead "A3" "A4"

    $pinkMain = Get-TitlebarPinkRatio $mainPath $scale
    $pinkSet = Get-TitlebarPinkRatio $setPath $scale
    Log ("A5 读数：标题栏图标区粉色占比 主窗={0:P2} 设置窗={1:P2}（阈值 {2:P2}）" -f $pinkMain, $pinkSet, $MinPinkRatio)
    if ($pinkSet -ge $MinPinkRatio) { Log "A5 PASS" }
    else {
        $failures.Add("A5 失败 —— 设置窗标题栏图标区粉色占比 $("{0:P2}" -f $pinkSet) < 阈值 $("{0:P2}" -f $MinPinkRatio)（疑似退化回系统占位图标）")
        Log "A5 FAIL"
    }

    $readings = @(
        "tag=$Tag",
        "exe=$Exe",
        "exe_bytes=$exeBytes",
        "exe_sha256=$exeHash",
        "pid=$($proc.Id)",
        "dpi=$dpi scale=$scale",
        "main_hwnd=0x$($main.ToInt64().ToString('X'))",
        "settings_hwnd=0x$($settings.ToInt64().ToString('X'))",
        $mainRead.Line,
        $setRead.Line,
        "titlebar_pink_ratio_main=$("{0:F4}" -f $pinkMain)",
        "titlebar_pink_ratio_settings=$("{0:F4}" -f $pinkSet)",
        "threshold_min_pink_ratio=$("{0:F4}" -f $MinPinkRatio)",
        "assertions=" + $(if ($failures.Count -eq 0) { "A0-A5 PASS" } else { "FAIL x$($failures.Count)" })
    )
    $readings | Set-Content -LiteralPath (Join-Path $OutDir "readings-$Tag.txt") -Encoding UTF8

    # 关设置窗（WM_CLOSE），保持现场干净
    [void][RZWI]::PostMessageW($settings, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
    Start-Sleep -Milliseconds 400
} catch {
    $failures.Add("装置异常：$($_.Exception.Message)")
    Log "EXCEPTION $($_.Exception.Message)"
} finally {
    if ($proc -and -not $KeepRunning) {
        try { Stop-Process -Id $proc.Id -Force -ErrorAction Stop; Log "已按 PID 关闭 exe（PID=$($proc.Id)）" }
        catch { Log "关闭失败（可能已退出）：$($_.Exception.Message)" }
        Start-Sleep -Milliseconds 500
        $still = Get-Process -Id $proc.Id -ErrorAction SilentlyContinue
        if ($still) { Log "⚠ 进程仍在：PID=$($proc.Id)" } else { Log "确认进程已退出（PID=$($proc.Id)）" }
    }
}

$verdictPath = Join-Path $OutDir "verdict-$Tag.txt"
if ($failures.Count -eq 0) {
    "PASS  （A0-A5 全部成立）" | Set-Content -LiteralPath $verdictPath -Encoding UTF8
    Log "结论：PASS"
    exit 0
} else {
    ("FAIL  x{0}`n" -f $failures.Count) + ($failures -join "`n") | Set-Content -LiteralPath $verdictPath -Encoding UTF8
    Log "结论：FAIL（$($failures.Count) 条）"
    foreach ($f in $failures) { Log "  · $f" }
    exit 1
}
