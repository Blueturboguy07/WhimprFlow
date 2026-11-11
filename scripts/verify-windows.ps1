# Proves a Windows build of WhimprFlow installs and can transcribe, rather
# than merely built. The speech model is the point: v0.2.1 shipped with none,
# so a fresh install could not dictate at all.
#
# 1. Installs the NSIS setup.exe silently (per-user, %LOCALAPPDATA%\WhimprFlow).
# 2. Launches it and waits for the log line that says the BUNDLED model
#    (<install dir>\models\ggml-base.en.bin) loaded.
# 3. Removes that bundled model, relaunches, and waits for the "no usable
#    speech model" line and the Hub's download popup.
# 4. Presses "Download speech model (148 MB)" through UI Automation — the
#    same button a reader clicks — and waits for the model downloaded into
#    %APPDATA%\WhimprFlow\models to load, with no relaunch.
#
# Between 2 and 3, with the bundled build running, it also proves the first
# run that 0.2.2 got wrong on Windows (GitHub Actions run 41374831510):
#
# 5. Home, Settings and Help name the Windows push-to-talk key, Right Ctrl,
#    and never the macOS key (Fn), which no Windows hook can see.
# 6. Holding Right Ctrl on this runner, which has no input device, ends in a
#    loud "No microphone found" diagnostic (log line + Hub banner) instead of
#    the pill showing "recording", then idle, and nothing else. The runner
#    does not always hand a synthetic key to low-level hooks (seen in runs
#    41373827908 and 41376843945, not in 41374831510), so this script keeps a
#    WH_KEYBOARD_LL hook of its own: when that hook saw nothing either, the
#    check is skipped rather than failed, and 7 proves the diagnostic path.
# 7. Ctrl+Shift+Space, the Hub's hands-free shortcut, starts and then stops a
#    dictation instead of being swallowed by an empty handler; the stop, with
#    no microphone, raises the same "No microphone found" banner.
#
# Screenshots and logs land in -OutDir. Exit code 1 on any failure.
#
# Usage: scripts/verify-windows.ps1 [-Installer <setup.exe>] [-OutDir <dir>]
param(
  [string]$Installer = "",
  [string]$OutDir = "verify-windows-out"
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
# Synthetic key presses (keybd_event reaches RegisterHotKey and, on a good
# day, WH_KEYBOARD_LL hooks on the runner), plus a WH_KEYBOARD_LL hook in THIS
# process that records what the runner delivered, so "the app saw nothing"
# can be told apart from "the runner delivered nothing".
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
      lock (seen) seen.Add("msg=0x" + w.ToInt64().ToString("X") + " vk=0x" + k.vkCode.ToString("X") + " scan=0x" + k.scanCode.ToString("X") + " flags=0x" + k.flags.ToString("X"));
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

$ModelFile = 'ggml-base.en.bin'
$ModelSha256 = 'A03779C86DF3323075F5E796CB2CE5029F00EC8869EEE3FDFB897AFE36C6D002'
$ButtonName = 'Download speech model (148 MB)'

function Shot($name) {
  $b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
  $bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($b.Location, [System.Drawing.Point]::Empty, $b.Size)
  $bmp.Save((Join-Path $OutDir "$name.png"))
  $g.Dispose(); $bmp.Dispose()
}

function WaitForLog($file, $pattern, $seconds) {
  $deadline = (Get-Date).AddSeconds($seconds)
  while ((Get-Date) -lt $deadline) {
    if (Test-Path $file) {
      $t = Get-Content -Raw $file -ErrorAction SilentlyContinue
      if ($t -and ($t -match $pattern)) { return $true }
    }
    Start-Sleep -Milliseconds 500
  }
  return $false
}

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

# Every UI Automation name under the Hub window (titled "WhimprFlow"): the
# text a reader can see on the current screen, one entry per element.
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

# Click an element by name: the sidebar's "Settings" / "Help" entries.
function ClickByName($name) {
  $el = FindByName $name 10
  if (-not $el) { return $false }
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

function KeyDown($vk, $scan, $ext) { $f = 0; if ($ext) { $f = 1 }; [VerifyKeys]::keybd_event([byte]$vk, [byte]$scan, [uint32]$f, [UIntPtr]::Zero) }
function KeyUp($vk, $scan, $ext)   { $f = 2; if ($ext) { $f = 3 }; [VerifyKeys]::keybd_event([byte]$vk, [byte]$scan, [uint32]$f, [UIntPtr]::Zero) }

# Ctrl+Shift+Space, the default hands-free shortcut (Settings > Hands-free shortcut).
function PressHandsFree() {
  KeyDown 0x11 0x1D $false; KeyDown 0x10 0x2A $false; KeyDown 0x20 0x39 $false
  Start-Sleep -Milliseconds 120
  KeyUp 0x20 0x39 $false; KeyUp 0x10 0x2A $false; KeyUp 0x11 0x1D $false
}

# The names on one Hub screen must say Right Ctrl and never Fn.
function CheckKeyNamed($screen, $names) {
  $names | Out-File -Encoding UTF8 (Join-Path $OutDir "hub-$($screen.ToLower())-names.txt")
  if (@($names -match 'Right Ctrl').Count -ge 1) { Pass "$screen names Right Ctrl" }
  else { Fail "$screen does not name Right Ctrl ($(@($names).Count) names)" }
  $fn = @($names -match '\bFn\b')
  if ($fn.Count -eq 0) { Pass "$screen does not name Fn (the macOS key)" }
  else { Fail "$screen names Fn, the macOS key: $($fn -join ' | ')" }
}

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

# The sign-in gate: WhimprFlow opens with "Sign in with publik" and waits for GET /wallet to
# say "claimed". This runner has no publik account, so a fake publik answers on 127.0.0.1, and
# a fake signed-in credential (own key chosen) is written first. No real key or token is involved.
$FakePort = 47831
$script:FakePublik = Start-Job -ArgumentList $FakePort -ScriptBlock {
  param($p)
  $l = New-Object System.Net.HttpListener
  $l.Prefixes.Add("http://127.0.0.1:$p/")
  $l.Start()
  while ($true) {
    $c = $l.GetContext()
    $path = $c.Request.Url.AbsolutePath
    $body = ""
    if ($path -eq "/api/v1/wallet") {
      $c.Response.StatusCode = 200
      $body = '{"claim_state":"claimed","balance_micros":0,"plan":{"kind":"plan","label":"CI plan"},"account":{"handle":"ci"},"top_up_url":"https://publikhq.com/wallet"}'
    } elseif ($path -eq "/api/v1/installs") {
      $c.Response.StatusCode = 201
      $body = '{"key":"pk_ci_fake","base_url":"http://127.0.0.1:' + $p + '/api/v1","install_id":"00000000-0000-4000-8000-000000000000","claim_url":"https://publikhq.com/claim/CI"}'
    } else {
      $c.Response.StatusCode = 204
    }
    if ($body) {
      $bytes = [System.Text.Encoding]::UTF8.GetBytes($body)
      $c.Response.ContentType = "application/json"
      $c.Response.ContentLength64 = $bytes.Length
      $c.Response.OutputStream.Write($bytes, 0, $bytes.Length)
    }
    $c.Response.Close()
  }
}
$PublikApps = Join-Path $env:LOCALAPPDATA 'publik\apps'
New-Item -ItemType Directory -Force $PublikApps | Out-Null
'{"key":"pk_ci_fake","base_url":"http://127.0.0.1:' + $FakePort + '/api/v1","install_id":"00000000-0000-4000-8000-000000000000","claim_url":"https://publikhq.com/claim/CI","choice":"own_key"}' | Set-Content (Join-Path $PublikApps 'whimprflow.json') -Encoding ASCII
$env:PUBLIK_ORIGIN = "http://127.0.0.1:$FakePort"
Pass "fake publik on port $FakePort answers the sign-in gate (signed in, own key chosen)"

$Bundled = Join-Path $InstallDir "models\$ModelFile"
$UserModels = Join-Path $env:APPDATA 'WhimprFlow\models'
if (Test-Path $UserModels) { Remove-Item -Recurse -Force $UserModels }

Write-Host ""
Write-Host "Bundled speech model"
if ((Test-Path $Bundled) -and ((Get-FileHash $Bundled -Algorithm SHA256).Hash -eq $ModelSha256)) {
  Pass "$ModelFile is installed next to the app and its checksum matches"
} else {
  Fail "no valid bundled model at $Bundled"
}

$run = Launch 'bundled'
# Match on the path tail: Tauri may report the folder with a \\?\ prefix.
$bundledPattern = 'ASR model loaded \([^)]*' + [regex]::Escape("Local\WhimprFlow\models\$ModelFile") + '\)'
if (WaitForLog $run.Err $bundledPattern 90) {
  Pass "the app loaded the bundled model at launch"
} else {
  Fail "the bundled model did not load within 90 s"
}
Start-Sleep -Seconds 3
Shot '1-bundled-hub'
if (FindByName 'Speech model not installed' 3) { Fail "the 'not installed' banner shows even with the bundled model" }
else { Pass "no 'Speech model not installed' banner" }

Write-Host ""
Write-Host "First run: the key the Hub names"
CheckKeyNamed 'Home' (HubNames)

Write-Host ""
Write-Host "First run: a microphone that cannot open (this runner has no input device)"
$devs = Get-CimInstance Win32_SoundDevice -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Name
Write-Host ("  Win32_SoundDevice: " + $(if ($devs) { $devs -join '; ' } else { '(none)' }))
[SelfHook]::Start()
Write-Host ("  self-hook: " + ([SelfHook]::Drain() -join ' | '))
KeyDown 0xA3 0x1D $true
Start-Sleep -Milliseconds 700
Shot '2-right-ctrl-held'
Start-Sleep -Milliseconds 800
KeyUp 0xA3 0x1D $true
$seen = @([SelfHook]::Drain())
$delivered = @($seen -match 'vk=0xA3').Count -ge 1
Write-Host ("  self-hook saw: " + $(if ($seen.Count) { $seen -join ' | ' } else { '(nothing)' }))
if (WaitForLog $run.Err 'No microphone found' 15) { Pass "holding Right Ctrl 1.5 s logs the 'No microphone found' diagnostic" }
elseif (-not $delivered) { Write-Host "  SKIP  the runner did not hand the synthetic Right Ctrl to low-level hooks (this script's own hook saw nothing); the hands-free check below proves the diagnostic path" }
else { Fail "low-level hooks received Right Ctrl, but no 'No microphone found' diagnostic was logged within 15 s (0.2.2 failed silently here)" }
[SelfHook]::Stop()

Write-Host ""
Write-Host "First run: the hands-free shortcut (Ctrl+Shift+Space)"
PressHandsFree
if (WaitForLog $run.Err 'hands-free: start' 10) { Pass "Ctrl+Shift+Space starts hands-free dictation" }
else { Fail "no 'hands-free: start' line within 10 s (0.2.2's handler was empty)" }
Start-Sleep -Seconds 1
PressHandsFree
if (WaitForLog $run.Err 'hands-free: stop' 10) { Pass "Ctrl+Shift+Space again stops it" }
else { Fail "no 'hands-free: stop' line within 10 s" }
# With no microphone, that stop ends like the Right Ctrl release does: the
# diagnostic goes out (log line, pill error state, Hub banner).
if (WaitForLog $run.Err 'No microphone found' 15) { Pass "a hold with no microphone logs the 'No microphone found' diagnostic" }
else { Fail "no 'No microphone found' diagnostic in the log within 15 s (0.2.2 failed silently here)" }
Start-Sleep -Seconds 1
Shot '3-no-microphone-banner'
if (FindByName 'No microphone found' 10) { Pass "the Hub shows the 'No microphone found' banner" }
else { Fail "no 'No microphone found' banner through UI Automation" }

Write-Host ""
Write-Host "First run: Settings and Help name the key"
foreach ($page in @('Settings', 'Help')) {
  if (ClickByName $page) {
    Start-Sleep -Seconds 3
    Shot "4-$page"
    CheckKeyNamed $page (HubNames)
  } else {
    Fail "could not open $page through UI Automation"
  }
}
StopApp $run

Write-Host ""
Write-Host "No model: popup and download button"
$Aside = Join-Path $env:TEMP $ModelFile
Move-Item -Force $Bundled $Aside
$run = Launch 'download'
if (WaitForLog $run.Err 'no usable speech model' 60) { Pass "the app reports no usable speech model" }
else { Fail "no 'no usable speech model' line within 60 s" }

$button = FindByName $ButtonName 60
if ($button) {
  Pass "the popup shows the '$ButtonName' button"
  Shot '2-download-popup'
  $button.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke()
  Start-Sleep -Seconds 2
  Shot '3-downloading'
  $userModel = Join-Path $UserModels $ModelFile
  $loadedPattern = 'ASR model loaded \([^)]*' + [regex]::Escape("Roaming\WhimprFlow\models\$ModelFile") + '\)'
  if (WaitForLog $run.Err $loadedPattern 300) {
    Pass "the downloaded model loaded with no relaunch"
    if ((Get-FileHash $userModel -Algorithm SHA256).Hash -eq $ModelSha256) {
      Pass "the downloaded file's checksum matches"
    } else {
      Fail "the downloaded file's checksum does not match"
    }
    Start-Sleep -Seconds 2
    Shot '4-installed'
    if (FindByName 'Speech model installed' 10) { Pass "the popup says 'Speech model installed'" }
    else { Fail "no 'Speech model installed' confirmation in the popup" }
  } else {
    Fail "the downloaded model did not load within 300 s"
    Shot '4-timeout'
  }
} else {
  Fail "no '$ButtonName' button found through UI Automation within 60 s"
  Shot '2-no-popup'
}
StopApp $run
Move-Item -Force $Aside $Bundled

Write-Host ""
Write-Host "--- bundled.err.log ---"
Get-Content (Join-Path $OutDir 'bundled.err.log') -ErrorAction SilentlyContinue | Select-Object -First 60
Write-Host ""
Write-Host "--- download.err.log ---"
Get-Content (Join-Path $OutDir 'download.err.log') -ErrorAction SilentlyContinue | Select-Object -First 40

if ($script:FakePublik) {
  Stop-Job $script:FakePublik -ErrorAction SilentlyContinue
  Remove-Job $script:FakePublik -Force -ErrorAction SilentlyContinue
}

Write-Host ""
if ($script:failures -eq 0) { Write-Host "ALL CHECKS PASSED"; exit 0 }
Write-Host "$($script:failures) CHECK(S) FAILED"
exit 1
