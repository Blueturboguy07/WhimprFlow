# Blind-verifier variations, round 2, for the Windows first run (fix/windows-first-run).
# Uses the installer that windows.yml built for 6b90ecf (run 41380472340) and tries:
#
# V9. UPGRADE: install the real 0.2.2 release, give it the settings a linked
#     0.2.2 user has (cleanup_mode publik, disclosure accepted, install id),
#     then install 0.2.3 over it. The settings survive, the Hub names Right
#     Ctrl, and Right Ctrl raises the diagnostic. publik_base_url points at
#     127.0.0.1:9 so nothing reaches the real publik API.
# V6. The hands-free chord typed with RIGHT Ctrl (the push-to-talk key) +
#     Shift + Space, against the same chord with LEFT Ctrl (control).
# V7. Twelve 800 ms holds in a row: the keyboard hook stays alive.
# V8. Relaunch and hold Right Ctrl the moment the hook is installed, then
#     hold again 20 s later: the second hold is still seen.
#
# Usage: scripts/verify-windows-variations-2.ps1 -Installer <setup.exe> [-OutDir <dir>]
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

function WriteJsonNoBom($path, $text) {
  New-Item -ItemType Directory -Force (Split-Path $path) | Out-Null
  [System.IO.File]::WriteAllText($path, $text, (New-Object System.Text.UTF8Encoding $false))
}
function FindExe() {
  $dir = Join-Path $env:LOCALAPPDATA 'WhimprFlow'
  $e = Get-ChildItem $dir -Filter '*.exe' -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -notmatch 'uninstall' } | Select-Object -First 1
  if ($e) { return $e.FullName } else { return $null }
}
function SetProp($o, $k, $v) {
  if ($o.PSObject.Properties[$k]) { $o.$k = $v } else { $o | Add-Member -NotePropertyName $k -NotePropertyValue $v }
}
function NewText($file, $fromLen) {
  $t = LogText $file
  if ($t.Length -gt $fromLen) { return $t.Substring($fromLen) } else { return "" }
}
function DownsSeen($vkHex) {
  $seen = @([SelfHook]::Drain())
  return @($seen -match "msg=0x100 vk=0x$vkHex").Count
}
function PressHandsFreeRightCtrl() {
  KeyDown 0xA3 0x1D $true; Start-Sleep -Milliseconds 150
  KeyDown 0x10 0x2A $false; KeyDown 0x20 0x39 $false
  Start-Sleep -Milliseconds 120
  KeyUp 0x20 0x39 $false; KeyUp 0x10 0x2A $false; KeyUp 0xA3 0x1D $true
}
$START = 'hands-free: start'
$STOP = 'hands-free: stop'
$TESTID = '9a9a9a9a-0000-4000-8000-000000000009'

$Support = Join-Path $env:APPDATA 'WhimprFlow'
$SettingsFile = Join-Path $Support 'settings.json'
if (Test-Path $SettingsFile) { Remove-Item -Force $SettingsFile }
$devs = Get-CimInstance Win32_SoundDevice -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Name
Write-Host ("  Win32_SoundDevice: " + $(if ($devs) { $devs -join '; ' } else { '(none)' }))
if (-not $Installer -or -not (Test-Path $Installer)) { Write-Host "No 0.2.3 installer given."; exit 1 }
Write-Host "0.2.3 installer: $Installer ($((Get-Item $Installer).Length) bytes)"

# ── V9 part 1: the real 0.2.2 release, linked the way a 0.2.2 user is ─────────
Write-Host ""
Write-Host "V9: upgrade over the real 0.2.2 release"
$oldSetup = Join-Path $env:TEMP 'WhimprFlow_0.2.2_x64-setup.exe'
Invoke-WebRequest -UseBasicParsing -Uri 'https://github.com/Blueturboguy07/WhimprFlow/releases/download/v0.2.2/WhimprFlow_0.2.2_x64-setup.exe' -OutFile $oldSetup
$len = (Get-Item $oldSetup).Length
if ($len -eq 132654534) { Pass "downloaded the v0.2.2 release installer ($len bytes)" } else { Fail "v0.2.2 installer is $len bytes, expected 132654534" }
$p = Start-Process -FilePath $oldSetup -ArgumentList '/S' -Wait -PassThru
if ($p.ExitCode -eq 0) { Pass "0.2.2 silent install exited 0" } else { Fail "0.2.2 silent install exited $($p.ExitCode)" }
$script:Exe = FindExe
if (-not $script:Exe) { Fail "no app .exe after the 0.2.2 install"; exit 1 }
$oldExe = $script:Exe
Note ("0.2.2 exe: $oldExe, ProductVersion " + (Get-Item $oldExe).VersionInfo.ProductVersion)
$run = Launch 'V9a-0.2.2'
if (WaitForLog $run.Err 'keyboard hook installed' 60) { Pass "0.2.2 launched and installed its hook" } else { Fail "0.2.2: no hook line within 60 s" }
WaitForLog $run.Err 'ASR model loaded' 90 | Out-Null
Start-Sleep -Seconds 4
$names = HubNames
$names | Out-File -Encoding UTF8 (Join-Path $OutDir 'V9a-0.2.2-home-names.txt')
Note ("0.2.2 Home: names with 'Right Ctrl' = " + @($names -match 'Right Ctrl').Count + ", with 'Fn' = " + @($names -match '\bFn\b').Count)
Shot 'V9a-0.2.2-hub'
StopApp $run
$had = Test-Path $SettingsFile
Note "0.2.2 wrote settings.json by itself: $had"
if ($had) { $s = (Get-Content -Raw $SettingsFile | ConvertFrom-Json) }
else {
  # The exact field set 0.2.2 (22c8119) serializes.
  $s = [pscustomobject]@{ cleanup_mode = 'local'; cleanup_level = 'light'; openai_model = 'gpt-4o-mini';
    openai_base_url = ''; anthropic_model = 'claude-haiku-4-5'; sound_on_start = $true;
    hands_free_hotkey = 'CmdOrCtrl+Shift+Space' }
}
# What "Turn on publik API" leaves behind in 0.2.2 (publik_accept_disclosure).
SetProp $s 'cleanup_mode' 'publik'
SetProp $s 'publik_disclosure_version' 1
SetProp $s 'publik_install_id' $TESTID
SetProp $s 'publik_base_url' 'http://127.0.0.1:9'
SetProp $s 'publik_model' ''
SetProp $s 'publik_claim_url' ''
SetProp $s 'publik_starter_micros' 0
SetProp $s 'publik_cta_pending' $false
WriteJsonNoBom $SettingsFile ($s | ConvertTo-Json -Depth 5)
Copy-Item $SettingsFile (Join-Path $OutDir 'V9-settings-0.2.2.json')

# ── V9 part 2: install 0.2.3 over it ─────────────────────────────────────────
$p = Start-Process -FilePath $Installer -ArgumentList '/S' -Wait -PassThru
if ($p.ExitCode -eq 0) { Pass "0.2.3 silent install over 0.2.2 exited 0" } else { Fail "0.2.3 install over 0.2.2 exited $($p.ExitCode)" }
$script:Exe = FindExe
$newVer = (Get-Item $script:Exe).VersionInfo.ProductVersion
if ($script:Exe -eq $oldExe) { Pass "0.2.3 replaced 0.2.2 at the same path" } else { Fail "0.2.3 is at $($script:Exe), 0.2.2 was at $oldExe" }
if ($newVer -match '^0\.2\.3') { Pass "the installed exe is $newVer" } else { Fail "the installed exe reports $newVer, not 0.2.3" }
Note ("exes now in the install dir: " + ((Get-ChildItem (Split-Path $script:Exe) -Filter '*.exe' | ForEach-Object { $_.Name }) -join ', '))
$sj = (Get-Content -Raw $SettingsFile | ConvertFrom-Json)
if ($sj.publik_install_id -eq $TESTID) { Pass "the installer left the 0.2.2 settings.json alone" } else { Fail "settings.json changed during the install" }

$run = Launch 'V9b-upgraded'
if (WaitForLog $run.Err 'keyboard hook installed \(push-to-talk: Right Ctrl\)' 60) { Pass "the upgraded app hooks Right Ctrl" }
else { Fail "the upgraded app logged no 'keyboard hook installed (push-to-talk: Right Ctrl)' within 60 s" }
if (WaitForLog $run.Err 'ASR model loaded' 90) { Pass "the upgraded app loaded the bundled model" } else { Fail "upgraded: no 'ASR model loaded' within 90 s" }
Start-Sleep -Seconds 4
$names = HubNames
$names | Out-File -Encoding UTF8 (Join-Path $OutDir 'V9b-upgraded-home-names.txt')
if (@($names -match 'Right Ctrl').Count -ge 1) { Pass "upgraded Home names Right Ctrl" } else { Fail "upgraded Home does not name Right Ctrl ($($names.Count) names)" }
if (@($names -match '\bFn\b').Count -eq 0) { Pass "upgraded Home does not name Fn" } else { Fail "upgraded Home names Fn: $(@($names -match '\bFn\b') -join ' | ')" }
Shot 'V9b-upgraded-hub'
[SelfHook]::Start()
[SelfHook]::Drain() | Out-Null
$b = CountInLog $run.Err $DIAG
HoldRightCtrl 1500
$deliv = Delivered 'A3'
if (WaitForCount $run.Err $DIAG ($b + 1) 15) { Pass "upgraded: holding Right Ctrl 1.5 s raises '$DIAG'" }
elseif (-not $deliv) { Skip "upgraded: the runner did not hand Right Ctrl to low-level hooks" }
else { Fail "upgraded: hooks saw Right Ctrl but no '$DIAG' within 15 s" }
Shot 'V9b-upgraded-after-hold'
# Make the app save its in-memory settings: open Settings and click the
# already-selected Right Ctrl card. If 0.2.3 had failed to parse the 0.2.2
# file it would now write the defaults (local mode, empty install id).
if (ClickByName 'Settings') {
  Start-Sleep -Seconds 3
  $btn = FindByName 'Right Ctrl' 10
  if ($btn) {
    $before = (Get-Item $SettingsFile).LastWriteTimeUtc
    ClickElement $btn | Out-Null
    Start-Sleep -Seconds 3
    $after = (Get-Item $SettingsFile).LastWriteTimeUtc
    Copy-Item $SettingsFile (Join-Path $OutDir 'V9-settings-after-0.2.3-save.json')
    $sj = (Get-Content -Raw $SettingsFile | ConvertFrom-Json)
    Note "settings.json rewritten by 0.2.3: $($after -ne $before)"
    if ($after -ne $before -and $sj.publik_install_id -eq $TESTID -and $sj.cleanup_mode -eq 'publik' -and $sj.push_to_talk_key -eq 'ControlRight') {
      Pass "0.2.3 parsed the 0.2.2 settings (saved back: publik mode, same install id, push_to_talk_key ControlRight)"
    } elseif ($after -eq $before) { Note "the click did not make the app save; parse not proven this way" }
    else { Fail "0.2.3 saved different settings: mode=$($sj.cleanup_mode) id=$($sj.publik_install_id) key=$($sj.push_to_talk_key)" }
    Shot 'V9b-settings'
    ClickByName 'Home' | Out-Null
    Start-Sleep -Seconds 2
  } else { Note "no 'Right Ctrl' card through UI Automation" }
} else { Note "could not open Settings through UI Automation" }
$pl = ([regex]::Matches((LogText $run.Err), '(?m)^.*publik.*$') | ForEach-Object { $_.Value.Trim() }) -join ' || '
Note ("publik log lines: " + $(if ($pl) { $pl } else { '(none)' }))

# ── V6: the hands-free chord with RIGHT Ctrl vs LEFT Ctrl ────────────────────
Write-Host ""
Write-Host "V6: Ctrl+Shift+Space typed with RIGHT Ctrl (the push-to-talk key)"
Start-Sleep -Seconds 3
[SelfHook]::Drain() | Out-Null
$s0 = CountInLog $run.Err $START; $t0 = CountInLog $run.Err $STOP; $l0 = (LogText $run.Err).Length
PressHandsFreeRightCtrl
Start-Sleep -Seconds 4
$downs = DownsSeen 'A3'
$ds = (CountInLog $run.Err $START) - $s0; $dt = (CountInLog $run.Err $STOP) - $t0
Write-Host "  self-hook saw Right Ctrl down x$downs; log delta: start +$ds, stop +$dt"
Write-Host ("  new log: " + ((NewText $run.Err $l0) -replace "`r?`n", ' || '))
Shot 'V6-right-ctrl-chord'
if ($downs -eq 0) { Skip "the runner did not hand Right Ctrl to low-level hooks, so this says nothing (start +$ds, stop +$dt)" }
elseif ($ds -eq 1 -and $dt -eq 0) { Pass "the Right-Ctrl chord starts hands-free" }
elseif ($dt -ge 1 -and $ds -eq 0) { Fail "the Right-Ctrl chord logged '$STOP' and no '$START': Right Ctrl down began a push-to-talk hold, so the chord toggled it OFF; hands-free never starts" }
else { Fail "unexpected: start +$ds, stop +$dt" }
if ((CountInLog $run.Err $START) - (CountInLog $run.Err $STOP) -gt $s0 - $t0) { PressHandsFree; Start-Sleep -Seconds 3 }
# Second Right-Ctrl chord: a user who saw nothing happen presses it again.
$s1 = CountInLog $run.Err $START; $t1 = CountInLog $run.Err $STOP
PressHandsFreeRightCtrl
Start-Sleep -Seconds 4
$downs = DownsSeen 'A3'
Write-Host ("  second Right-Ctrl chord: hook downs x$downs; start +" + ((CountInLog $run.Err $START) - $s1) + ", stop +" + ((CountInLog $run.Err $STOP) - $t1))
if ((CountInLog $run.Err $START) - (CountInLog $run.Err $STOP) -gt $s0 - $t0) { PressHandsFree; Start-Sleep -Seconds 3 }
Write-Host "V6 control: Ctrl+Shift+Space typed with LEFT Ctrl"
$s2 = CountInLog $run.Err $START; $t2 = CountInLog $run.Err $STOP
PressHandsFree
Start-Sleep -Seconds 2
$a = (CountInLog $run.Err $START) - $s2
PressHandsFree
Start-Sleep -Seconds 4
$z = (CountInLog $run.Err $STOP) - $t2
if ($a -eq 1 -and $z -eq 1) { Pass "the Left-Ctrl chord starts and then stops hands-free" } else { Fail "Left-Ctrl chord: start +$a, stop +$z" }

# ── V7: twelve holds in a row ────────────────────────────────────────────────
Write-Host ""
Write-Host "V7: twelve 800 ms holds of Right Ctrl"
Start-Sleep -Seconds 3
[SelfHook]::Drain() | Out-Null
$b = CountInLog $run.Err $NOAUDIO
for ($i = 1; $i -le 12; $i++) { HoldRightCtrl 800; Start-Sleep -Milliseconds 1200 }
$downs = DownsSeen 'A3'
WaitForCount $run.Err $NOAUDIO ($b + 12) 20 | Out-Null
$d = (CountInLog $run.Err $NOAUDIO) - $b
Write-Host "  self-hook saw Right Ctrl down x$downs; app logged '$NOAUDIO' x$d"
if ($d -ge 12) { Pass "12 of 12 holds reached the app's hook" }
elseif ($downs -lt 12) { Skip "the runner handed only $downs of 12 holds to low-level hooks; the app saw $d" }
else { Fail "low-level hooks saw 12 holds, the app saw only $d (hook dropped?)" }
[SelfHook]::Stop()
StopApp $run

# ── V8: a hold the moment the hook is installed, then one 20 s later ─────────
Write-Host ""
Write-Host "V8: hold Right Ctrl right at launch, then again 20 s later"
[SelfHook]::Start()
[SelfHook]::Drain() | Out-Null
$run = Launch 'V8-early'
$deadline = (Get-Date).AddSeconds(60)
while ((Get-Date) -lt $deadline -and (CountInLog $run.Err 'keyboard hook installed') -lt 1) { Start-Sleep -Milliseconds 100 }
$b = CountInLog $run.Err $NOAUDIO
HoldRightCtrl 1000
$downs1 = DownsSeen 'A3'
Start-Sleep -Seconds 20
$mid = CountInLog $run.Err $NOAUDIO
HoldRightCtrl 1000
$downs2 = DownsSeen 'A3'
WaitForCount $run.Err $NOAUDIO ($mid + 1) 15 | Out-Null
$first = $mid - $b; $second = (CountInLog $run.Err $NOAUDIO) - $mid
Write-Host "  early hold: hook downs x$downs1, app '$NOAUDIO' x$first; later hold: hook downs x$downs2, app x$second"
if ($second -ge 1) { Pass "a hold 20 s after an at-launch hold still reaches the app's hook" }
elseif ($downs2 -eq 0) { Skip "the runner did not hand the later Right Ctrl to low-level hooks" }
else { Fail "the app's hook missed the later hold after an at-launch hold (hook removed by a timeout?)" }
Shot 'V8-after'
[SelfHook]::Stop()
StopApp $run

Write-Host ""
if ($script:failures -eq 0) { Write-Host "ALL VARIATION CHECKS PASSED" } else { Write-Host "$($script:failures) VARIATION CHECK(S) FAILED" }
# Exit 0 on purpose: the job's result must not hide the evidence upload; the
# PASS/FAIL lines are the verdict.
exit 0
