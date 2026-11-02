# Blind-verifier variations, round 3 (fix/windows-first-run @ 6b90ecf installer).
# The fixer's recipe step 8 says: Settings -> Push-to-Talk Key -> Left Alt, hold
# Left Alt, speak, release: text is typed. The hook never swallows the key, so
# a lone Alt press+release also reaches the focused app, which can enter menu
# mode. The runner has no microphone, so this script reproduces the app's own
# paste (win.rs paste_text: clipboard, 60 ms, SendInput Ctrl, V, V up, Ctrl up)
# right after the hold and reads what landed in Notepad.
#
# R-C  control: the paste with no key held lands in Notepad.
# R-B  control: Right Ctrl (default) hold, then the paste: lands.
# R-A  Left Alt hold, then the paste: does it land?
# R-D  Left Alt hold while the Hub is focused: does the Hub still answer?
#
# Usage: scripts/verify-windows-variations-3.ps1 -Installer <setup.exe> [-OutDir <dir>]
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
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class W32 {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [StructLayout(LayoutKind.Sequential)] public struct GUITHREADINFO {
    public int cbSize; public uint flags; public IntPtr hwndActive; public IntPtr hwndFocus; public IntPtr hwndCapture;
    public IntPtr hwndMenuOwner; public IntPtr hwndMoveSize; public IntPtr hwndCaret; public RECT rcCaret; }
  [DllImport("user32.dll")] public static extern bool GetGUIThreadInfo(uint tid, ref GUITHREADINFO info);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsHungAppWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindow(string cls, string title);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern IntPtr SendMessageTimeout(IntPtr h, uint msg, IntPtr w, StringBuilder l, uint flags, uint ms, out IntPtr result);
  [DllImport("user32.dll")] static extern IntPtr SendMessageTimeout(IntPtr h, uint msg, IntPtr w, IntPtr l, uint flags, uint ms, out IntPtr result);
  delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] static extern bool EnumChildWindows(IntPtr p, EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc cb, IntPtr l);
  public static string Cls(IntPtr h) { var s = new StringBuilder(256); GetClassName(h, s, 256); return s.ToString(); }
  public static string Title(IntPtr h) { var s = new StringBuilder(512); GetWindowText(h, s, 512); return s.ToString(); }
  public static IntPtr EditChild(IntPtr top) {
    IntPtr found = IntPtr.Zero;
    EnumChildWindows(top, (h, l) => { var c = Cls(h); if (c.IndexOf("Edit", StringComparison.OrdinalIgnoreCase) >= 0) { found = h; return false; } return true; }, IntPtr.Zero);
    return found;
  }
  public static string Text(IntPtr h) {
    IntPtr r; var sb = new StringBuilder(4096);
    if (SendMessageTimeout(h, 0x000D, (IntPtr)4096, sb, 0x2, 3000, out r) == IntPtr.Zero) return "<WM_GETTEXT timed out>";
    return sb.ToString();
  }
  public static bool Answers(IntPtr h, uint ms) { IntPtr r; return SendMessageTimeout(h, 0, IntPtr.Zero, IntPtr.Zero, 0x2, ms, out r) != IntPtr.Zero; }
  public static uint MenuFlags(IntPtr top) {
    uint pid; uint tid = GetWindowThreadProcessId(top, out pid);
    var gi = new GUITHREADINFO(); gi.cbSize = Marshal.SizeOf(typeof(GUITHREADINFO));
    if (!GetGUIThreadInfo(tid, ref gi)) return 0xFFFFFFFF;
    return gi.flags;
  }
  public static IntPtr TopByTitle(string title) {
    IntPtr found = IntPtr.Zero;
    EnumWindows((h, l) => { if (Title(h) == title && Cls(h) != "ConsoleWindowClass") { found = h; return false; } return true; }, IntPtr.Zero);
    return found;
  }
}
"@

function WriteJsonNoBom($path, $text) {
  New-Item -ItemType Directory -Force (Split-Path $path) | Out-Null
  [System.IO.File]::WriteAllText($path, $text, (New-Object System.Text.UTF8Encoding $false))
}
function SettingsWith($key) {
  return (@{ cleanup_mode = 'local'; cleanup_level = 'light'; openai_model = 'gpt-4o-mini'; anthropic_model = 'claude-haiku-4-5';
    sound_on_start = $true; push_to_talk_key = $key } | ConvertTo-Json)
}
function ClickCenter($hwnd, $dy) {
  $r = New-Object W32+RECT
  [W32]::GetWindowRect($hwnd, [ref]$r) | Out-Null
  $x = [int](($r.L + $r.R) / 2); $y = [int](($r.T + $r.B) / 2) + $dy
  [System.Windows.Forms.Cursor]::Position = New-Object System.Drawing.Point($x, $y)
  [VerifyKeys]::mouse_event(0x2, 0, 0, 0, [UIntPtr]::Zero); [VerifyKeys]::mouse_event(0x4, 0, 0, 0, [UIntPtr]::Zero)
  Start-Sleep -Milliseconds 400
}
function FocusNp() {
  try { [System.Windows.Automation.AutomationElement]::FromHandle($script:Np).SetFocus() } catch { Write-Host "  UIA SetFocus on Notepad failed: $_" }
  [W32]::SetForegroundWindow($script:Np) | Out-Null
  Start-Sleep -Milliseconds 400
  ClickCenter $script:Np 40
}
function FgDesc() { $h = [W32]::GetForegroundWindow(); return "$([W32]::Cls($h)) '$([W32]::Title($h))'" }
# The app's paste, step for step (win.rs paste_text).
function AppPaste($text) {
  Set-Clipboard -Value $text
  Start-Sleep -Milliseconds 60
  KeyDown 0x11 0x1D $false; KeyDown 0x56 0x2F $false; KeyUp 0x56 0x2F $false; KeyUp 0x11 0x1D $false
  Start-Sleep -Milliseconds 600
}
function ResetNotepad() {
  KeyDown 0x1B 0x01 $false; KeyUp 0x1B 0x01 $false; Start-Sleep -Milliseconds 200
  KeyDown 0x1B 0x01 $false; KeyUp 0x1B 0x01 $false; Start-Sleep -Milliseconds 200
  FocusNp
  KeyDown 0x11 0x1D $false; KeyDown 0x41 0x1E $false; KeyUp 0x41 0x1E $false; KeyUp 0x11 0x1D $false
  KeyDown 0x2E 0x53 $true; KeyUp 0x2E 0x53 $true
  Start-Sleep -Milliseconds 300
}
function Trial($label, $holdFn, $marker) {
  ResetNotepad
  Write-Host "  [$label] foreground before: $(FgDesc); notepad text before: '$([W32]::Text($script:NpEdit))'"
  [SelfHook]::Drain() | Out-Null
  $b = CountInLog $script:Run.Err $NOAUDIO
  if ($holdFn) { & $holdFn }
  Start-Sleep -Milliseconds 800
  $seen = @([SelfHook]::Drain()) -join ' | '
  $flags = [W32]::MenuFlags($script:Np)
  Write-Host ("  [$label] self-hook: " + $(if ($seen) { $seen } else { '(nothing)' }))
  Write-Host ("  [$label] app '$NOAUDIO' +" + ((CountInLog $script:Run.Err $NOAUDIO) - $b) + "; notepad GUI flags 0x{0:X} (0x4 = in menu mode, 0x8 = system menu); foreground: $(FgDesc)" -f $flags)
  AppPaste $marker
  $t = [W32]::Text($script:NpEdit)
  Write-Host "  [$label] notepad text after the app's paste: '$t'"
  Shot "R-$label"
  return ($t -match [regex]::Escape($marker))
}

$Support = Join-Path $env:APPDATA 'WhimprFlow'
$SettingsFile = Join-Path $Support 'settings.json'
if (-not $Installer -or -not (Test-Path $Installer)) { Write-Host "No 0.2.3 installer given."; exit 1 }
$p = Start-Process -FilePath $Installer -ArgumentList '/S' -Wait -PassThru
if ($p.ExitCode -eq 0) { Pass "0.2.3 silent install exited 0" } else { Fail "install exited $($p.ExitCode)" }
$exeItem = Get-ChildItem (Join-Path $env:LOCALAPPDATA 'WhimprFlow') -Filter '*.exe' | Where-Object { $_.Name -notmatch 'uninstall' } | Select-Object -First 1
$script:Exe = $exeItem.FullName

$np = Start-Process notepad -PassThru
Start-Sleep -Seconds 3
$script:Np = $np.MainWindowHandle
if ($script:Np -eq [IntPtr]::Zero) { $np.Refresh(); $script:Np = $np.MainWindowHandle }
$script:NpEdit = [W32]::EditChild($script:Np)
Write-Host "  notepad top=$([W32]::Cls($script:Np)) edit=$([W32]::Cls($script:NpEdit)) ($($script:NpEdit))"
if ($script:NpEdit -eq [IntPtr]::Zero) { Fail "no edit control in Notepad"; exit 0 }
[SelfHook]::Start()

function StartWith($key, $tag) {
  WriteJsonNoBom $SettingsFile (SettingsWith $key)
  $script:Run = Launch $tag
  if (-not (WaitForLog $script:Run.Err 'keyboard hook installed' 60)) { Fail "[$tag] no hook line" }
  WaitForLog $script:Run.Err 'ASR model loaded' 90 | Out-Null
  Start-Sleep -Seconds 4
  Write-Host ("  [$tag] " + ((LogText $script:Run.Err) -split "`n" | Where-Object { $_ -match 'keyboard hook installed' }))
  FocusNp
}

Write-Host ""
Write-Host "Launch with push_to_talk_key = ControlRight (default)"
StartWith 'ControlRight' 'R-ctrl'
if (Trial 'C-no-key' $null 'WHIMPR-NOKEY') { Pass "R-C: the app's paste with no key held lands in Notepad (method works)" } else { Fail "R-C: the paste with no key held did not land; the method is broken, R-A/R-B say nothing" }
if (Trial 'B-right-ctrl' { HoldRightCtrl 1500 } 'WHIMPR-RCTRL') { Pass "R-B: after a Right Ctrl hold the app's paste lands in Notepad" } else { Fail "R-B: after a Right Ctrl hold the app's paste did not land" }
StopApp $script:Run

Write-Host ""
Write-Host "Launch with push_to_talk_key = AltLeft (recipe step 8)"
StartWith 'AltLeft' 'R-alt'
if (Trial 'A-left-alt' { HoldLeftAlt 1500 } 'WHIMPR-LALT') { Pass "R-A: after a Left Alt hold the app's paste lands in Notepad" }
else { Fail "R-A: after a Left Alt hold the app's paste did NOT land in Notepad (the lone Alt reached Notepad: menu mode)" }
if (Trial 'A2-left-alt-again' { HoldLeftAlt 1500 } 'WHIMPR-LALT2') { Pass "R-A2: a second Left Alt try lands" } else { Fail "R-A2: a second Left Alt try did not land either" }

Write-Host ""
Write-Host "R-D: Left Alt hold while the Hub is focused"
$hub = [W32]::TopByTitle('WhimprFlow')
Write-Host "  hub hwnd=$hub class=$([W32]::Cls($hub))"
if ($hub -ne [IntPtr]::Zero) {
  ClickCenter $hub -200
  Write-Host "  foreground: $(FgDesc)"
  HoldLeftAlt 1500
  Start-Sleep -Seconds 2
  $ans = [W32]::Answers($hub, 3000); $hung = [W32]::IsHungAppWindow($hub); $flags = [W32]::MenuFlags($hub)
  Write-Host ("  hub answers WM_NULL in 3 s: $ans; IsHungAppWindow: $hung; GUI flags 0x{0:X}" -f $flags)
  $ok = $true
  try { $n = @(HubNames).Count; Write-Host "  UI Automation read $n names from the Hub" } catch { $ok = $false; Write-Host "  UI Automation failed: $_" }
  Shot 'R-D-hub-after-alt'
  if ($ans -and -not $hung -and $ok) { Pass "R-D: the Hub still answers after a Left Alt hold" } else { Fail "R-D: the Hub did not answer after a Left Alt hold" }
  KeyDown 0x1B 0x01 $false; KeyUp 0x1B 0x01 $false
} else { Note "R-D: no Hub window found" }
[SelfHook]::Stop()
StopApp $script:Run
Stop-Process -Id $np.Id -Force -ErrorAction SilentlyContinue

Write-Host ""
if ($script:failures -eq 0) { Write-Host "ALL ROUND-3 CHECKS PASSED" } else { Write-Host "$($script:failures) ROUND-3 CHECK(S) FAILED" }
exit 0
