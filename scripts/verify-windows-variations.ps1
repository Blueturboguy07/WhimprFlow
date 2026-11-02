# Blind-verifier variations for the Windows first run (fix/windows-first-run).
# Runs on the installed 0.2.3 build and tries what verify-windows.ps1 does not:
#
# V1. A 250 ms tap of Right Ctrl logs the hold but raises NO diagnostic.
# V2. The pill's Stop button ends a hands-free dictation (stop_dictation wired).
# V3. Settings -> Push-to-Talk Key -> "Left Alt" takes effect with no restart:
#     Left Alt raises the diagnostic, Right Ctrl no longer does, Home names Left Alt.
# V4. The saved key survives a relaunch (old settings file).
# V5. An unknown key string in settings.json ("Fn"): what the Hub says vs what
#     the hook listens to (NOTE only).
#
# Usage: scripts/verify-windows-variations.ps1 [-Installer <setup.exe>] [-OutDir <dir>]
param(
  [string]$Installer = "",
  [string]$OutDir = "verify-variations-out"
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class VerifyKeys {
  [DllImport("user32.dll")] public static extern void keybd_event(byte bVk, byte bScan, uint dwFlags, UIntPtr dwExtraInfo);
  [DllImport("user32.dll")] public static extern void mouse_event(uint dwFlags, uint dx, uint dy, uint dwData, UIntPtr dwExtraInfo);
}
public static class SelfHook {
  delegate IntPtr HookProc(int nCode, IntPtr wParam, IntPtr lParam);
  [DllImport("user32.dll")] static extern IntPtr SetWindowsHookEx(int id, HookProc cb, IntPtr hMod, uint tid);
  [DllImport("user32.dll")] static extern IntPtr CallNextHookEx(IntPtr hhk, int nCode, IntPtr wParam, IntPtr lParam);
  [DllImport("user32.dll")] static extern bool UnhookWindowsHookEx(IntPtr hhk);
  [DllImport("user32.dll")] static extern int GetMessage(out MSG msg, IntPtr hWnd, uint min, uint max);
  [DllImport("user32.dll")] static extern bool PostThreadMessage(uint tid, uint msg, IntPtr w, IntPtr l);
  [DllImport("kernel32.dll")] static extern uint GetCurrentThreadId();
  [DllImport("kernel32.dll", CharSet = CharSet.Auto)] static extern IntPtr GetModuleHandle(string name);
  [StructLayout(LayoutKind.Sequential)] struct MSG { public IntPtr hwnd; public uint message; public IntPtr wParam; public IntPtr lParam; public uint time; public int x; public int y; }
  [StructLayout(LayoutKind.Sequential)] struct KBDLLHOOKSTRUCT { public uint vkCode; public uint scanCode; public uint flags; public uint time; public UIntPtr dwExtraInfo; }
  static System.Collections.Generic.List<string> seen = new System.Collections.Generic.List<string>();
  static HookProc keep; static uint tid; static System.Threading.Thread t;
  public static void Start() {
    t = new System.Threading.Thread(() => {
      tid = GetCurrentThreadId(); keep = Cb;
      IntPtr h = SetWindowsHookEx(13, keep, GetModuleHandle(null), 0);
      lock (seen) seen.Add("hook handle=" + h + " lastError=" + Marshal.GetLastWin32Error());
      MSG m; while (GetMessage(out m, IntPtr.Zero, 0, 0) > 0) { }
      UnhookWindowsHookEx(h);
    });
    t.IsBackground = true; t.Start(); System.Threading.Thread.Sleep(800);
  }
  static IntPtr Cb(int code, IntPtr w, IntPtr l) {
    if (code >= 0) {
      var k = (KBDLLHOOKSTRUCT)Marshal.PtrToStructure(l, typeof(KBDLLHOOKSTRUCT));
      lock (seen) seen.Add("msg=0x" + w.ToInt64().ToString("X") + " vk=0x" + k.vkCode.ToString("X"));
    }
    return CallNextHookEx(IntPtr.Zero, code, w, l);
  }
  public static string[] Drain() { lock (seen) { var a = seen.ToArray(); seen.Clear(); return a; } }
  public static void Stop() { PostThreadMessage(tid, 0x12, IntPtr.Zero, IntPtr.Zero); }
}
"@

New-Item -ItemType Directory -Force $OutDir | Out-Null
$OutDir = (Resolve-Path $OutDir).Path
$script:failures = 0
function Pass($m) { Write-Host "  PASS  $m" }
function Fail($m) { Write-Host "  FAIL  $m"; $script:failures++ }
function Note($m) { Write-Host "  NOTE  $m" }
function Skip($m) { Write-Host "  SKIP  $m" }

function Shot($name) {
  $b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
  $bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($b.Location, [System.Drawing.Point]::Empty, $b.Size)
  $bmp.Save((Join-Path $OutDir "$name.png"))
  $g.Dispose(); $bmp.Dispose()
}

function LogText($file) {
  if (Test-Path $file) { $t = Get-Content -Raw $file -ErrorAction SilentlyContinue; if ($t) { return $t } }
  return ""
}
function CountInLog($file, $pattern) {
  $t = LogText $file
  if (-not $t) { return 0 }
  return ([regex]::Matches($t, $pattern)).Count
}
function WaitForCount($file, $pattern, $atLeast, $seconds) {
  $deadline = (Get-Date).AddSeconds($seconds)
  while ((Get-Date) -lt $deadline) {
    if ((CountInLog $file $pattern) -ge $atLeast) { return $true }
    Start-Sleep -Milliseconds 500
  }
  return $false
}
function WaitForLog($file, $pattern, $seconds) { return (WaitForCount $file $pattern 1 $seconds) }

function Launch($tag) {
  $out = Join-Path $OutDir "$tag.out.log"
  $err = Join-Path $OutDir "$tag.err.log"
  $proc = Start-Process -FilePath $script:Exe -RedirectStandardOutput $out -RedirectStandardError $err -PassThru
  return @{ Proc = $proc; Err = $err }
}
function StopApp($run) {
  if ($run -and $run.Proc -and -not $run.Proc.HasExited) { Stop-Process -Id $run.Proc.Id -Force }
  Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $script:Exe } | Stop-Process -Force
  Start-Sleep -Seconds 2
}

function FindByName($name, $seconds) {
  $root = [System.Windows.Automation.AutomationElement]::RootElement
  $cond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::NameProperty, $name)
  $deadline = (Get-Date).AddSeconds($seconds)
  while ((Get-Date) -lt $deadline) {
    $el = $root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $cond)
    if ($el) { return $el }
    Start-Sleep -Seconds 1
  }
  return $null
}
function HubNames() {
  $root = [System.Windows.Automation.AutomationElement]::RootElement
  $cond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::NameProperty, 'WhimprFlow')
  $hub = $root.FindFirst([System.Windows.Automation.TreeScope]::Children, $cond)
  if (-not $hub) { return @() }
  $all = $hub.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
  $names = @()
  foreach ($e in $all) {
    try { $n = $e.Current.Name; if ($n -and $n.Trim() -ne '') { $names += $n.Trim() } } catch {}
  }
  return @($names | Select-Object -Unique)
}
function ClickElement($el) {
  try {
    $pat = $null
    if ($el.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref]$pat)) { $pat.Invoke(); return $true }
  } catch {}
  try {
    $r = $el.Current.BoundingRectangle
    $x = [int]($r.X + $r.Width / 2); $y = [int]($r.Y + $r.Height / 2)
    [System.Windows.Forms.Cursor]::Position = New-Object System.Drawing.Point($x, $y)
    [VerifyKeys]::mouse_event(0x2, 0, 0, 0, [UIntPtr]::Zero); [VerifyKeys]::mouse_event(0x4, 0, 0, 0, [UIntPtr]::Zero)
    return $true
  } catch { return $false }
}
function ClickByName($name) {
  $el = FindByName $name 10
  if (-not $el) { return $false }
  return (ClickElement $el)
}

function KeyDown($vk, $scan, $ext) { $f = 0; if ($ext) { $f = 1 }; [VerifyKeys]::keybd_event([byte]$vk, [byte]$scan, [uint32]$f, [UIntPtr]::Zero) }
function KeyUp($vk, $scan, $ext)   { $f = 2; if ($ext) { $f = 3 }; [VerifyKeys]::keybd_event([byte]$vk, [byte]$scan, [uint32]$f, [UIntPtr]::Zero) }
function Hold($vk, $scan, $ext, $ms) { KeyDown $vk $scan $ext; Start-Sleep -Milliseconds $ms; KeyUp $vk $scan $ext }
function HoldRightCtrl($ms) { Hold 0xA3 0x1D $true $ms }
function HoldLeftAlt($ms)   { Hold 0xA4 0x38 $false $ms }
function PressHandsFree() {
  KeyDown 0x11 0x1D $false; KeyDown 0x10 0x2A $false; KeyDown 0x20 0x39 $false
  Start-Sleep -Milliseconds 120
  KeyUp 0x20 0x39 $false; KeyUp 0x10 0x2A $false; KeyUp 0x11 0x1D $false
}
# Did the runner hand the last synthetic key to low-level hooks?
function Delivered($vkHex) {
  $seen = @([SelfHook]::Drain())
  Write-Host ("  self-hook saw: " + $(if ($seen.Count) { $seen -join ' | ' } else { '(nothing)' }))
  return (@($seen -match "vk=0x$vkHex").Count -ge 1)
}

$DIAG = 'No microphone found'
$NOAUDIO = 'no audio captured'

# ── Install ──────────────────────────────────────────────────────────────────
if (-not $Installer) {
  $Installer = (Get-ChildItem -Recurse -Path target -Filter '*-setup.exe' -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -match 'bundle\\nsis' } | Select-Object -First 1).FullName
}
if (-not $Installer -or -not (Test-Path $Installer)) { Write-Host "No NSIS installer found."; exit 1 }
Write-Host "Installer: $Installer ($([math]::Round((Get-Item $Installer).Length / 1MB)) MB)"
Write-Host ""
Write-Host "Install"
$p = Start-Process -FilePath $Installer -ArgumentList '/S' -Wait -PassThru
if ($p.ExitCode -eq 0) { Pass "silent install exited 0" } else { Fail "silent install exited $($p.ExitCode)" }
$InstallDir = Join-Path $env:LOCALAPPDATA 'WhimprFlow'
$exeItem = Get-ChildItem $InstallDir -Filter '*.exe' -ErrorAction SilentlyContinue |
  Where-Object { $_.Name -notmatch 'uninstall' } | Select-Object -First 1
if (-not $exeItem) { Fail "no app .exe in $InstallDir"; exit 1 }
$script:Exe = $exeItem.FullName
Pass "installed $($script:Exe)"

$Support = Join-Path $env:APPDATA 'WhimprFlow'
$SettingsFile = Join-Path $Support 'settings.json'
if (Test-Path $SettingsFile) { Remove-Item -Force $SettingsFile }
$devs = Get-CimInstance Win32_SoundDevice -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Name
Write-Host ("  Win32_SoundDevice: " + $(if ($devs) { $devs -join '; ' } else { '(none)' }))

# ── Launch A: fresh settings ─────────────────────────────────────────────────
Write-Host ""
Write-Host "Launch A: fresh install (no settings.json)"
$run = Launch 'A-fresh'
if (WaitForLog $run.Err 'keyboard hook installed \(push-to-talk: Right Ctrl\)' 60) { Pass "fresh launch hooks Right Ctrl" }
else { Fail "no 'keyboard hook installed (push-to-talk: Right Ctrl)' line within 60 s" }
if (WaitForLog $run.Err 'ASR model loaded' 90) { Pass "the bundled model loaded" } else { Fail "no 'ASR model loaded' within 90 s" }
Start-Sleep -Seconds 4
[SelfHook]::Start()
[SelfHook]::Drain() | Out-Null

Write-Host ""
Write-Host "V1: a 250 ms tap of Right Ctrl"
$diagBefore = CountInLog $run.Err $DIAG
$noaudioBefore = CountInLog $run.Err $NOAUDIO
HoldRightCtrl 250
Start-Sleep -Seconds 4
$delivered = Delivered 'A3'
$t = LogText $run.Err
$m = [regex]::Match($t, 'no audio captured \([^)]*, held (\d+) ms\)')
if (-not $delivered) { Skip "the runner did not hand Right Ctrl to low-level hooks" }
else {
  if ($m.Success -and [int]$m.Groups[1].Value -lt 600) { Pass "the tap was logged as a hold of $($m.Groups[1].Value) ms" }
  elseif ($m.Success) { Fail "the tap was logged as $($m.Groups[1].Value) ms (expected < 600)" }
  else { Fail "no 'no audio captured (..., held N ms)' line after the tap" }
  if ((CountInLog $run.Err $DIAG) -eq $diagBefore) { Pass "a tap under 0.6 s raises no '$DIAG' diagnostic" }
  else { Fail "a 250 ms tap raised the '$DIAG' diagnostic" }
}
Shot 'V1-after-tap'

Write-Host ""
Write-Host "V2: the pill's Stop button ends a hands-free dictation"
$diagBefore = CountInLog $run.Err $DIAG
PressHandsFree
if (WaitForLog $run.Err 'hands-free: start' 10) {
  Start-Sleep -Milliseconds 1500
  Shot 'V2-hands-free-recording'
  $stop = FindByName 'Stop' 5
  if ($stop) {
    ClickElement $stop | Out-Null
    if (WaitForCount $run.Err $DIAG ($diagBefore + 1) 15) { Pass "the pill's Stop button ended the dictation and reported the missing microphone" }
    else { Fail "Stop clicked through UI Automation, but no new '$DIAG' within 15 s"; PressHandsFree }
  } else {
    Skip "no element named 'Stop' through UI Automation (overlay not exposed); ending hands-free with the shortcut"
    PressHandsFree
    WaitForCount $run.Err $DIAG ($diagBefore + 1) 15 | Out-Null
  }
} else { Fail "Ctrl+Shift+Space did not start hands-free dictation" }
Start-Sleep -Seconds 5

Write-Host ""
Write-Host "V3: Settings -> Push-to-Talk Key -> Left Alt, no restart"
if (ClickByName 'Settings') {
  Start-Sleep -Seconds 3
  $btn = FindByName 'Left Alt' 10
  if ($btn) {
    ClickElement $btn | Out-Null
    Start-Sleep -Seconds 3
    Shot 'V3-settings-left-alt'
    $sj = $null
    if (Test-Path $SettingsFile) { try { $sj = (Get-Content -Raw $SettingsFile | ConvertFrom-Json) } catch {} }
    if ($sj -and $sj.push_to_talk_key -eq 'AltLeft') { Pass "settings.json now says push_to_talk_key = AltLeft" }
    else { Fail "settings.json does not say AltLeft (file: $(Test-Path $SettingsFile); value: $($sj.push_to_talk_key))" }
    [SelfHook]::Drain() | Out-Null
    $diagBefore = CountInLog $run.Err $DIAG
    HoldLeftAlt 1500
    $deliveredAlt = Delivered 'A4'
    if (WaitForCount $run.Err $DIAG ($diagBefore + 1) 15) { Pass "holding Left Alt 1.5 s raises the '$DIAG' diagnostic with no restart" }
    elseif (-not $deliveredAlt) { Skip "the runner did not hand Left Alt to low-level hooks" }
    else { Fail "Left Alt reached low-level hooks but raised no '$DIAG' within 15 s" }
    Start-Sleep -Seconds 5
    [SelfHook]::Drain() | Out-Null
    $diagBefore = CountInLog $run.Err $DIAG
    $noaudioBefore = CountInLog $run.Err $NOAUDIO
    HoldRightCtrl 1500
    $deliveredCtrl = Delivered 'A3'
    Start-Sleep -Seconds 6
    if (-not $deliveredCtrl) { Skip "the runner did not hand Right Ctrl to low-level hooks" }
    elseif (((CountInLog $run.Err $DIAG) -eq $diagBefore) -and ((CountInLog $run.Err $NOAUDIO) -eq $noaudioBefore)) { Pass "Right Ctrl no longer starts a dictation after the switch" }
    else { Fail "Right Ctrl still started a dictation after switching to Left Alt" }
    if (ClickByName 'Home') {
      Start-Sleep -Seconds 3
      $names = HubNames
      $names | Out-File -Encoding UTF8 (Join-Path $OutDir 'V3-home-names.txt')
      if (@($names -match 'Left Alt').Count -ge 1) { Pass "Home names Left Alt after the switch" } else { Fail "Home does not name Left Alt after the switch" }
      if (@($names -match 'Right Ctrl').Count -eq 0) { Pass "Home no longer names Right Ctrl" } else { Fail "Home still names Right Ctrl: $(@($names -match 'Right Ctrl') -join ' | ')" }
      Shot 'V3-home-after-switch'
    } else { Fail "could not open Home through UI Automation" }
  } else { Fail "no 'Left Alt' button in Settings through UI Automation" }
} else { Fail "could not open Settings through UI Automation" }
[SelfHook]::Stop()
StopApp $run

# ── Launch B: the saved key survives a relaunch ──────────────────────────────
Write-Host ""
Write-Host "V4: relaunch with the saved settings file (AltLeft)"
$run = Launch 'B-relaunch'
if (WaitForLog $run.Err 'keyboard hook installed \(push-to-talk: Left Alt\)' 60) { Pass "the relaunch hooks Left Alt from settings.json" }
else { Fail "no 'keyboard hook installed (push-to-talk: Left Alt)' line within 60 s" }
WaitForLog $run.Err 'ASR model loaded' 90 | Out-Null
Start-Sleep -Seconds 4
$names = HubNames
$names | Out-File -Encoding UTF8 (Join-Path $OutDir 'V4-home-names.txt')
if (@($names -match 'Left Alt').Count -ge 1) { Pass "Home names Left Alt after the relaunch" } else { Fail "Home does not name Left Alt after the relaunch ($(@($names).Count) names)" }
[SelfHook]::Start(); [SelfHook]::Drain() | Out-Null
$diagBefore = CountInLog $run.Err $DIAG
HoldLeftAlt 1500
$deliveredAlt = Delivered 'A4'
if (WaitForCount $run.Err $DIAG ($diagBefore + 1) 15) { Pass "holding Left Alt 1.5 s raises the '$DIAG' diagnostic after the relaunch" }
elseif (-not $deliveredAlt) { Skip "the runner did not hand Left Alt to low-level hooks" }
else { Fail "Left Alt reached low-level hooks but raised no '$DIAG' within 15 s after the relaunch" }
Shot 'V4-relaunch'
[SelfHook]::Stop()
StopApp $run

# ── Launch C: an unknown key string in settings.json ─────────────────────────
Write-Host ""
Write-Host "V5: settings.json push_to_talk_key = 'Fn' (unknown string; NOTE only)"
try {
  $sj = Get-Content -Raw $SettingsFile | ConvertFrom-Json
  $sj.push_to_talk_key = 'Fn'
  $sj | ConvertTo-Json -Depth 20 | Set-Content -Encoding UTF8 $SettingsFile
  $run = Launch 'C-unknown-key'
  if (WaitForLog $run.Err 'keyboard hook installed' 60) {
    $t = LogText $run.Err
    $m = [regex]::Match($t, 'keyboard hook installed \(push-to-talk: ([^)]*)\)')
    Note "log line names the key as: '$($m.Groups[1].Value)'"
  } else { Note "no 'keyboard hook installed' line within 60 s" }
  WaitForLog $run.Err 'ASR model loaded' 90 | Out-Null
  Start-Sleep -Seconds 4
  $names = HubNames
  $names | Out-File -Encoding UTF8 (Join-Path $OutDir 'V5-home-names.txt')
  Note ("Home mentions Fn=" + @($names -match '\bFn\b').Count + " Right Ctrl=" + @($names -match 'Right Ctrl').Count)
  foreach ($n in @($names -match '\bFn\b|Right Ctrl')) { Note "  Home text: $n" }
  [SelfHook]::Start(); [SelfHook]::Drain() | Out-Null
  $diagBefore = CountInLog $run.Err $DIAG
  HoldRightCtrl 1500
  $deliveredCtrl = Delivered 'A3'
  $raised = WaitForCount $run.Err $DIAG ($diagBefore + 1) 15
  Note ("Right Ctrl delivered=" + $deliveredCtrl + "; raised the diagnostic=" + $raised)
  Shot 'V5-unknown-key'
  [SelfHook]::Stop()
  StopApp $run
} catch { Note "V5 could not run: $_" }

if (Test-Path $SettingsFile) { Remove-Item -Force $SettingsFile }

Write-Host ""
foreach ($tag in @('A-fresh', 'B-relaunch', 'C-unknown-key')) {
  Write-Host "--- $tag.err.log (whimpr lines) ---"
  Get-Content (Join-Path $OutDir "$tag.err.log") -ErrorAction SilentlyContinue | Where-Object { $_ -match '^\[whimpr' } | Select-Object -First 60
}

Write-Host ""
if ($script:failures -eq 0) { Write-Host "ALL CHECKS PASSED"; exit 0 }
Write-Host "$($script:failures) CHECK(S) FAILED"
exit 1
