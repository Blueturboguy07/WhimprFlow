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
Write-Host "--- download.err.log ---"
Get-Content (Join-Path $OutDir 'download.err.log') -ErrorAction SilentlyContinue | Select-Object -First 40

Write-Host ""
if ($script:failures -eq 0) { Write-Host "ALL CHECKS PASSED"; exit 0 }
Write-Host "$($script:failures) CHECK(S) FAILED"
exit 1
