# Triage probe for the v0.2.2 Windows first run (lane whimprflow-windows-silent).
#
# Installs the NSIS build, launches it, then presses the keys a reader might
# press, with synthetic input, and records what the app says about each:
#
#   A. Right Ctrl held 1.5 s. The runner has no microphone, so the capture
#      cannot start. Question: does the app TELL the reader (pill error, Hub
#      banner, "[whimpr] !" log line), or does it fail silently?
#   B. Ctrl+Shift+Space (the Hub's "Hands-free shortcut"). Question: does
#      anything happen on Windows?
#   C. The Hub's visible text (UI Automation names) on Home, Settings and
#      Help. Question: which key does the Hub tell a Windows reader to hold?
#
# Nothing here fails the job; every observation lands in -OutDir and in the
# step log so a reader can judge. Exit code is always 0.
param(
  [string]$Installer = "",
  [string]$OutDir = "probe-windows-out"
)
$ErrorActionPreference = 'Continue'
$ProgressPreference = 'SilentlyContinue'

Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class ProbeKeys {
  [DllImport("user32.dll")] public static extern void keybd_event(byte bVk, byte bScan, uint dwFlags, UIntPtr dwExtraInfo);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
}
"@

New-Item -ItemType Directory -Force $OutDir | Out-Null
$OutDir = (Resolve-Path $OutDir).Path
function Note($m) { Write-Host "  NOTE  $m"; Add-Content -Path (Join-Path $OutDir 'observations.txt') -Value $m }

function Shot($name) {
  try {
    $b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    $bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($b.Location, [System.Drawing.Point]::Empty, $b.Size)
    $bmp.Save((Join-Path $OutDir "$name.png"))
    $g.Dispose(); $bmp.Dispose()
  } catch { Note "screenshot $name failed: $_" }
}

function ReadLog($file) {
  if (Test-Path $file) { return (Get-Content -Raw -Encoding UTF8 $file -ErrorAction SilentlyContinue) }
  return ""
}

function WaitForLog($file, $pattern, $seconds) {
  $deadline = (Get-Date).AddSeconds($seconds)
  while ((Get-Date) -lt $deadline) {
    $t = ReadLog $file
    if ($t -and ($t -match $pattern)) { return $true }
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

# Every UI Automation name under the WhimprFlow Hub window, one per line.
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
  return $names | Select-Object -Unique
}

function ClickByName($name) {
  $el = FindByName $name 10
  if (-not $el) { Note "no element named '$name' to click"; return $false }
  try {
    $pat = $null
    if ($el.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref]$pat)) { $pat.Invoke(); return $true }
  } catch {}
  try {
    $r = $el.Current.BoundingRectangle
    $x = [int]($r.X + $r.Width / 2); $y = [int]($r.Y + $r.Height / 2)
    [System.Windows.Forms.Cursor]::Position = New-Object System.Drawing.Point($x, $y)
    Add-Type @"
using System; using System.Runtime.InteropServices;
public static class ProbeMouse { [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint x, uint y, uint d, UIntPtr e); }
"@ -ErrorAction SilentlyContinue
    [ProbeMouse]::mouse_event(0x2, 0, 0, 0, [UIntPtr]::Zero); [ProbeMouse]::mouse_event(0x4, 0, 0, 0, [UIntPtr]::Zero)
    return $true
  } catch { Note "click '$name' failed: $_"; return $false }
}

function KeyDown($vk, $scan, $ext) { $f = 0; if ($ext) { $f = 1 }; [ProbeKeys]::keybd_event([byte]$vk, [byte]$scan, [uint32]$f, [UIntPtr]::Zero) }
function KeyUp($vk, $scan, $ext)   { $f = 2; if ($ext) { $f = 3 }; [ProbeKeys]::keybd_event([byte]$vk, [byte]$scan, [uint32]$f, [UIntPtr]::Zero) }

# -- Install -------------------------------------------------------------------
if (-not $Installer) {
  $Installer = (Get-ChildItem -Recurse -Path target -Filter '*-setup.exe' -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -match 'bundle\\nsis' } | Select-Object -First 1).FullName
}
if (-not $Installer -or -not (Test-Path $Installer)) { Note "No NSIS installer found."; exit 0 }
Note "Installer: $Installer ($([math]::Round((Get-Item $Installer).Length / 1MB)) MB)"
$p = Start-Process -FilePath $Installer -ArgumentList '/S' -Wait -PassThru
Note "silent install exit code $($p.ExitCode)"

$InstallDir = Join-Path $env:LOCALAPPDATA 'WhimprFlow'
$exeItem = Get-ChildItem $InstallDir -Filter '*.exe' -ErrorAction SilentlyContinue |
  Where-Object { $_.Name -notmatch 'uninstall' } | Select-Object -First 1
if (-not $exeItem) { Note "no app .exe in $InstallDir"; exit 0 }
$script:Exe = $exeItem.FullName
Note "app exe: $($script:Exe)"

# Audio devices on this runner (the point of probe A).
try {
  $devs = Get-CimInstance Win32_SoundDevice -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Name
  Note ("Win32_SoundDevice: " + ($(if ($devs) { $devs -join '; ' } else { '(none)' })))
} catch { Note "Win32_SoundDevice query failed: $_" }
try {
  $mic = Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone\NonPackaged' -ErrorAction SilentlyContinue
  Note ("microphone NonPackaged consent: " + $(if ($mic) { $mic.Value } else { '(key absent)' }))
} catch {}

# -- Launch --------------------------------------------------------------------
Write-Host ""
Write-Host "Launch"
$run = Launch 'probe'
if (WaitForLog $run.Err 'keyboard hook installed' 90) { Note "log: keyboard hook line seen" } else { Note "log: no 'keyboard hook installed' line within 90 s" }
if (WaitForLog $run.Err 'ASR model loaded' 90) { Note "log: ASR model loaded" } else { Note "log: no 'ASR model loaded' line within 90 s" }
Start-Sleep -Seconds 4
Shot '0-launched'
$before = ReadLog $run.Err

# -- Probe C1: what the Home screen says ----------------------------------------
Write-Host ""
Write-Host "Probe C: Hub text"
$home = HubNames
$home | Out-File -Encoding UTF8 (Join-Path $OutDir 'hub-home-names.txt')
Note ("Home: " + $home.Count + " names; mentions Right Ctrl=" + (($home -match 'Right Ctrl').Count) + ", Fn=" + (($home -match '\bFn\b').Count) + ", Ctrl\+Shift\+Space=" + (($home -match 'Ctrl\+Shift\+Space').Count))
foreach ($n in ($home | Where-Object { $_ -match 'hold|Hold|key|Key|Fn|Ctrl' })) { Note "  Home text: $n" }

# Focus Notepad-free target: put the Hub in front so the paste (if any) is visible.
# -- Probe A: Right Ctrl with no microphone ------------------------------------
Write-Host ""
Write-Host "Probe A: Right Ctrl held 1.5 s (no microphone on this runner)"
KeyDown 0xA3 0x1D $true
Start-Sleep -Milliseconds 700
Shot '1-rctrl-held'
Start-Sleep -Milliseconds 800
KeyUp 0xA3 0x1D $true
Start-Sleep -Seconds 6
Shot '2-after-rctrl'
$afterA = ReadLog $run.Err
$deltaA = $afterA.Substring([Math]::Min($before.Length, $afterA.Length))
$deltaA | Out-File -Encoding UTF8 (Join-Path $OutDir 'delta-A-rctrl.log')
Note ("A: new log lines after Right Ctrl:`n" + $deltaA)
Note ("A: 'mic capture failed' logged = " + ($deltaA -match 'mic capture failed'))
Note ("A: 'no audio captured' logged = " + ($deltaA -match 'no audio captured'))
Note ("A: a user-facing diagnostic ('[whimpr] ' + warning sign, i.e. diag::report) logged = " + ($deltaA -match '\[whimpr\] \S+ (No audio came in|Microphone access blocked|Right Ctrl isn''t wired up|Couldn''t|Didn''t|Speech model)'))
$banner = $null
foreach ($h in @('No audio came in', 'Microphone access blocked', "Right Ctrl isn't wired up", 'Speech model not installed', "Couldn't transcribe that", "Didn't catch that")) {
  if (FindByName $h 1) { $banner = $h; break }
}
Note ("A: Hub banner/pill error visible through UI Automation = " + $(if ($banner) { $banner } else { 'NONE' }))

# -- Probe B: Ctrl+Shift+Space --------------------------------------------------
Write-Host ""
Write-Host "Probe B: Ctrl+Shift+Space (the Hub's hands-free shortcut)"
$beforeB = ReadLog $run.Err
KeyDown 0x11 0x1D $false; KeyDown 0x10 0x2A $false; KeyDown 0x20 0x39 $false
Start-Sleep -Milliseconds 120
KeyUp 0x20 0x39 $false; KeyUp 0x10 0x2A $false; KeyUp 0x11 0x1D $false
Start-Sleep -Seconds 4
Shot '3-after-ctrl-shift-space'
$afterB = ReadLog $run.Err
$deltaB = $afterB.Substring([Math]::Min($beforeB.Length, $afterB.Length))
$deltaB | Out-File -Encoding UTF8 (Join-Path $OutDir 'delta-B-handsfree.log')
Note ("B: new log lines after Ctrl+Shift+Space: " + $(if ($deltaB.Trim()) { "`n$deltaB" } else { '(none)' }))

# -- Probe C2/C3: Settings and Help text ----------------------------------------
Write-Host ""
Write-Host "Probe C: Settings and Help text"
foreach ($page in @('Settings', 'Help')) {
  if (ClickByName $page) {
    Start-Sleep -Seconds 3
    Shot "4-$page"
    $names = HubNames
    $names | Out-File -Encoding UTF8 (Join-Path $OutDir "hub-$($page.ToLower())-names.txt")
    Note ("$page`: " + $names.Count + " names; mentions Right Ctrl=" + (($names -match 'Right Ctrl').Count) + ", Fn=" + (($names -match '\bFn\b').Count) + ", Ctrl\+Shift\+Space=" + (($names -match 'Ctrl\+Shift\+Space').Count))
    foreach ($n in ($names | Where-Object { $_ -match 'Fn|Ctrl|hold|Hold|dictation key|Hands-free|push-to-talk|Push-to-talk' })) { Note "  $page text: $n" }
  }
}

StopApp $run
Write-Host ""
Write-Host "--- probe.err.log (first 60 lines) ---"
Get-Content (Join-Path $OutDir 'probe.err.log') -Encoding UTF8 -ErrorAction SilentlyContinue | Select-Object -First 60
Write-Host ""
Write-Host "--- observations.txt ---"
Get-Content (Join-Path $OutDir 'observations.txt') -Encoding UTF8
exit 0
