# bugfix-lab oracle script (LLVM angle) for cluster whimprflow-windows-build-toolchain
#
# WHY THIS EXISTS, next to the earlier script in this folder.
# The earlier run executed the guide's TERMINAL steps on a stock windows-latest
# runner. But the GitHub runner image ships LLVM pre-installed at
# C:\Program Files\LLVM and on the machine PATH. No consumer Windows machine
# looks like that: the guide itself has a reader step ("Install LLVM 18")
# precisely because it assumes LLVM is NOT already there. A `kind: "open"` step
# renders to nothing in a script, so the earlier run silently substituted the
# runner's pre-installed LLVM for the step a real reader performs by hand -- and
# the LLVM/libclang cause named in report f792b86d was never exercised.
#
# This script restores a real starting state and then performs guide step 7 the
# way a reader does, per variant:
#
#   no-llvm            the reader never got LLVM onto the machine. Report
#                      f792b86d says literally this ("LLVM installer won't
#                      co-exist with another LLVM -> skipped the installer
#                      entirely").  <-- CARRIES THE ORACLE VERDICT
#   step7-default      clean the machine of LLVM properly (run the existing
#                      LLVM's own uninstaller so the registry is clean too),
#                      then run LLVM-18.1.8-win64.exe from the guide's linked
#                      release with the installer's default options, then
#                      rebuild the environment from the registry the way a
#                      FRESH PowerShell window does.
#   step7-path         as step7-default, plus explicitly putting LLVM's bin on
#                      PATH. Positive control.
#
# Every installer wait is bounded, and the script says so when it times out,
# rather than hanging until the job's timeout and producing nothing.
#
# Everything after step 7 is the guide's own Windows steps, verbatim, at the
# pinned sourceCommit, ending in the exact command both bug reports ran.
#
# Exit 1 + BUGFIX_LAB_PRESENT  = the guide's build step fails.
# Exit 0 + BUGFIX_LAB_ABSENT   = tauri build succeeds and produces an installer.

$ErrorActionPreference = 'Continue'
$ProgressPreference = 'SilentlyContinue'

$VARIANT = $env:BUGFIX_VARIANT
if (-not $VARIANT) { $VARIANT = 'no-llvm' }
$PIN = '44c6d39213e38e7619e6dcfea1184b6619a11d01'
$INSTALL_TIMEOUT_SECS = 1500

function Section($s) {
  Write-Host ""
  Write-Host "=== $s ==="
}

function Show-RegistryPath($label) {
  $m = [Environment]::GetEnvironmentVariable('Path','Machine')
  $u = [Environment]::GetEnvironmentVariable('Path','User')
  Write-Host ("REGPATH[$label] MACHINE_LLVM=" + (($m -split ';' | Where-Object { $_ -match 'LLVM' }) -join ' | '))
  Write-Host ("REGPATH[$label] USER_LLVM=" + (($u -split ';' | Where-Object { $_ -match 'LLVM' }) -join ' | '))
}

# Run an installer without ever hanging the job.
function Invoke-Installer($path, $args, $label) {
  Write-Host "running $label : $path $args"
  $p = Start-Process -FilePath $path -ArgumentList $args -PassThru
  $done = $p.WaitForExit($INSTALL_TIMEOUT_SECS * 1000)
  if (-not $done) {
    Write-Host "${label}_TIMED_OUT=true after $INSTALL_TIMEOUT_SECS s (the installer never returned)"
    try { $p.Kill() } catch { }
    return $false
  }
  Write-Host "${label}_EXIT=$($p.ExitCode)"
  return $true
}

Write-Host "BUGFIX_LAB_VARIANT=$VARIANT"
Write-Host "BUGFIX_LAB_PIN=$PIN"

Section "runner identity"
[System.Environment]::OSVersion.VersionString
(Get-CimInstance Win32_OperatingSystem).Caption

Section "stock toolchain as the image ships it (BEFORE any change)"
node --version
git --version
try { rustc --version } catch { Write-Host "rustc: not found" }
try { cargo --version } catch { Write-Host "cargo: not found" }
try { cmake --version } catch { Write-Host "cmake: not found" }
$stockClang = (Get-Command clang -ErrorAction SilentlyContinue)
if ($stockClang) {
  Write-Host "STOCK_CLANG_PATH=$($stockClang.Source)"
  clang --version
} else {
  Write-Host "STOCK_CLANG_PATH=none"
}
Write-Host "STOCK_LIBCLANG_PATH=[$env:LIBCLANG_PATH]"
Show-RegistryPath "stock"

# --------------------------------------------------------------------------
# Restore the real starting state of a consumer Windows machine: no LLVM.
# --------------------------------------------------------------------------
Section "removing the runner image's pre-installed LLVM (a real user's PC has none)"

# For the step7-* variants the removal must be a REAL uninstall, or the LLVM
# installer sees a registered LLVM and blocks on its co-existence dialog (which
# is exactly what happened in the previous round, and is its own finding, but
# confounds the question "what does a clean PC get?").
if ($VARIANT -ne 'no-llvm') {
  foreach ($un in @('C:\Program Files\LLVM\Uninstall.exe','C:\Program Files (x86)\LLVM\Uninstall.exe')) {
    if (Test-Path $un) { Invoke-Installer $un '/S' 'LLVM_UNINSTALL' | Out-Null }
  }
  Start-Sleep -Seconds 20
}

foreach ($dir in @('C:\Program Files\LLVM','C:\Program Files (x86)\LLVM')) {
  if (Test-Path $dir) {
    Remove-Item -Recurse -Force $dir -ErrorAction SilentlyContinue
    Write-Host "removed leftover $dir"
  }
}
foreach ($scope in @('Machine','User')) {
  $v = [Environment]::GetEnvironmentVariable('Path',$scope)
  if ($v) {
    $cleaned = (($v -split ';') | Where-Object { $_ -and ($_ -notmatch 'LLVM') }) -join ';'
    [Environment]::SetEnvironmentVariable('Path',$cleaned,$scope)
  }
}
$env:PATH = (($env:PATH -split ';') | Where-Object { $_ -and ($_ -notmatch 'LLVM') }) -join ';'
Remove-Item Env:\LIBCLANG_PATH -ErrorAction SilentlyContinue

Show-RegistryPath "after-removal"
$afterClang = (Get-Command clang -ErrorAction SilentlyContinue)
if ($afterClang) { Write-Host "CLANG_AFTER_REMOVAL=$($afterClang.Source)" } else { Write-Host "CLANG_AFTER_REMOVAL=none" }
Write-Host ("LLVM_DIR_AFTER_REMOVAL_EXISTS=" + (Test-Path 'C:\Program Files\LLVM'))
Write-Host "LIBCLANG_PATH_AFTER_REMOVAL=[$env:LIBCLANG_PATH]"

# --------------------------------------------------------------------------
# Guide step 7 (reader step: "Install LLVM 18"), performed per variant.
# --------------------------------------------------------------------------
Section "guide step 7 (reader): Install LLVM 18 -- variant $VARIANT"
$LLVM_URL = 'https://github.com/llvm/llvm-project/releases/download/llvmorg-18.1.8/LLVM-18.1.8-win64.exe'

if ($VARIANT -eq 'no-llvm') {
  Write-Host "variant no-llvm: reader never got LLVM onto the machine (report f792b86d). Nothing installed."
} else {
  $llvmExe = Join-Path $env:RUNNER_TEMP 'LLVM-18.1.8-win64.exe'
  Write-Host "downloading $LLVM_URL"
  curl.exe -f -L -o $llvmExe $LLVM_URL
  Write-Host "DOWNLOAD_EXIT=$LASTEXITCODE SIZE=$((Get-Item $llvmExe).Length)"
  $ok = Invoke-Installer $llvmExe '/S' 'LLVM_INSTALL'
  Write-Host "LLVM_INSTALL_COMPLETED=$ok"
  Write-Host ("CLANG_EXE_PRESENT=" + (Test-Path 'C:\Program Files\LLVM\bin\clang.exe'))
  Write-Host ("LIBCLANG_DLL_PRESENT=" + (Test-Path 'C:\Program Files\LLVM\bin\libclang.dll'))
  if (Test-Path 'C:\Program Files\LLVM\bin\clang.exe') { & 'C:\Program Files\LLVM\bin\clang.exe' --version }
  Show-RegistryPath "after-install"

  # A reader opens a NEW PowerShell window after installing (the guide says so
  # for other installers). Rebuild this process's PATH from the registry the
  # way a fresh shell does, so the test is not biased by our stale process env.
  $env:PATH = ([Environment]::GetEnvironmentVariable('Path','Machine') + ';' + [Environment]::GetEnvironmentVariable('Path','User'))
  Write-Host "rebuilt PATH from the registry, as a fresh PowerShell window would"

  if ($VARIANT -eq 'step7-path') {
    $env:PATH = 'C:\Program Files\LLVM\bin;' + $env:PATH
    Write-Host "variant step7-path: additionally prepended C:\Program Files\LLVM\bin"
  }
}

$postClang = (Get-Command clang -ErrorAction SilentlyContinue)
if ($postClang) { Write-Host "CLANG_ON_PATH_AFTER_STEP7=$($postClang.Source)" } else { Write-Host "CLANG_ON_PATH_AFTER_STEP7=none" }
Write-Host "LIBCLANG_PATH_AFTER_STEP7=[$env:LIBCLANG_PATH]"

# --------------------------------------------------------------------------
# The guide's terminal steps, verbatim, from here on.
# --------------------------------------------------------------------------
Section "guide step 2: Check Git and Node"
git --version
node --version

Section "guide step 3: Install pnpm"
npm.cmd install -g pnpm
Write-Host "PNPM_GLOBAL_INSTALL_EXIT=$LASTEXITCODE"
pnpm.cmd --version

Section "guide step 8: Copy WhimprFlow to this PC"
Set-Location $env:USERPROFILE
if (Test-Path (Join-Path $env:USERPROFILE 'WhimprFlow')) {
  Remove-Item -Recurse -Force (Join-Path $env:USERPROFILE 'WhimprFlow')
}
git clone https://github.com/Blueturboguy07/WhimprFlow.git
Write-Host "CLONE_EXIT=$LASTEXITCODE"

Section "guide step 9: Open the WhimprFlow folder"
Set-Location (Join-Path $env:USERPROFILE 'WhimprFlow')
Get-Location

Section "guide step 10: Use the reviewed version"
git checkout $PIN
Write-Host "CHECKOUT_EXIT=$LASTEXITCODE"
git log -1 --format="%H %ci %s"

Section "guide step 11: Install the interface packages"
cd ui; pnpm.cmd install; cd ..; pnpm.cmd --dir ui approve-builds --all
Write-Host "DEPS_STEP_EXIT=$LASTEXITCODE"

Section "guide step 12: Check the build toolchain"
node scripts/check-build-prereqs.mjs
$prereqExit = $LASTEXITCODE
Write-Host "PREREQ_CHECK_EXIT=$prereqExit"

Section "guide step 13: Make the speech model folder"
mkdir -Force "$env:APPDATA\WhimprFlow\models" | Out-Null
Write-Host "MODELS_DIR_EXIT=$LASTEXITCODE"

Section "guide step 14: Download the speech model"
if (!(Test-Path "$env:APPDATA\WhimprFlow\models\ggml-base.en.bin")) { curl.exe -f -L -o "$env:APPDATA\WhimprFlow\models\ggml-base.en.bin" https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin }
Write-Host "MODEL_DOWNLOAD_EXIT=$LASTEXITCODE"

Section "guide step 15: Build the installer -- the exact command both bug reports ran"
Write-Host "COMMAND: ui\node_modules\.bin\tauri.CMD build"
$buildLog = Join-Path $env:RUNNER_TEMP 'tauri-build.log'
ui\node_modules\.bin\tauri.CMD build 2>&1 | Tee-Object -FilePath $buildLog
$tauriExit = $LASTEXITCODE
Write-Host "TAURI_BUILD_EXIT=$tauriExit"

Section "build outcome"
$installer = $null
if (Test-Path 'target\release\bundle') {
  $installer = Get-ChildItem -Recurse -Path 'target\release\bundle' -Include *.exe,*.msi -ErrorAction SilentlyContinue | Select-Object -First 1
}
if ($installer) { Write-Host "INSTALLER_FOUND=$($installer.FullName)" } else { Write-Host "INSTALLER_FOUND=none" }

if (Test-Path $buildLog) {
  Section "matched failure signatures in the build log"
  $pats = @(
    'Unable to find libclang',
    'libclang',
    'ERR_PNPM_IGNORED_BUILDS',
    'canonicalizing the ',
    'beforeBuildCommand',
    'is not a valid bundle target',
    'error: failed to run custom build command',
    'error: could not compile',
    'whisper-rs-sys'
  )
  foreach ($p in $pats) {
    $hits = Select-String -Path $buildLog -SimpleMatch -Pattern $p -ErrorAction SilentlyContinue
    if ($hits) {
      Write-Host ("--- pattern: " + $p)
      $hits | Select-Object -First 6 | ForEach-Object { Write-Host ("    " + $_.Line.Trim()) }
    }
  }
  Section "last 40 lines of the build log"
  Get-Content $buildLog -Tail 40 | ForEach-Object { Write-Host ("    " + $_) }
}

Section "verdict"
if ($tauriExit -eq 0 -and $installer) {
  Write-Host "BUGFIX_LAB_VARIANT_RESULT variant=$VARIANT tauri_exit=$tauriExit installer=$($installer.FullName) verdict=ABSENT"
  Write-Host "BUGFIX_LAB_ABSENT [variant=$VARIANT]"
  exit 0
} else {
  Write-Host "BUGFIX_LAB_VARIANT_RESULT variant=$VARIANT tauri_exit=$tauriExit installer=none verdict=PRESENT"
  Write-Host "BUGFIX_LAB_PRESENT [variant=$VARIANT]"
  exit 1
}
