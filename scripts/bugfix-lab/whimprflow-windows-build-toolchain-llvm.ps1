# bugfix-lab oracle script (LLVM angle) for cluster whimprflow-windows-build-toolchain
#
# WHY THIS EXISTS, next to the earlier script in this folder.
# The earlier run executed the guide's TERMINAL steps on a stock windows-latest
# runner. But the GitHub runner image ships LLVM 20.1.8 pre-installed at
# C:\Program Files\LLVM and on PATH. No consumer Windows machine looks like
# that: the guide itself has a reader step ("Install LLVM 18") precisely
# because it assumes LLVM is NOT already there. A `kind: "open"` step renders
# to nothing in a script, so the earlier run silently substituted the runner's
# pre-installed LLVM for the step a real reader performs by hand -- and the
# LLVM/bindgen cause named in report f792b86d was therefore never exercised.
#
# This script restores the real starting state (removes the image's LLVM) and
# then performs guide step 7 the way a reader does, per variant:
#
#   no-llvm      the reader skipped the LLVM installer. This is literally what
#                report f792b86d says they did ("LLVM installer won't co-exist
#                with another LLVM -> skipped the installer entirely").
#   step7-default (PRIMARY, carries the oracle verdict) the reader ran
#                LLVM-18.1.8-win64.exe from the guide's linked release and took
#                the installer's default options. /S drives NSIS with exactly
#                the default page values a click-through Next/Next/Install
#                produces.
#   step7-path   the reader additionally put LLVM on PATH. Positive control:
#                if the build only works here, the defect is the guide's step-7
#                wording, not the app.
#
# Everything after that is the guide's own Windows steps, verbatim, at the
# pinned sourceCommit, ending in the exact command both bug reports ran.
#
# Exit 1 + BUGFIX_LAB_PRESENT  = the guide's build step fails.
# Exit 0 + BUGFIX_LAB_ABSENT   = tauri build succeeds and produces an installer.

$ErrorActionPreference = 'Continue'
$ProgressPreference = 'SilentlyContinue'

$VARIANT = $env:BUGFIX_VARIANT
if (-not $VARIANT) { $VARIANT = 'step7-default' }
$PIN = '44c6d39213e38e7619e6dcfea1184b6619a11d01'

function Section($s) {
  Write-Host ""
  Write-Host "=== $s ==="
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

# --------------------------------------------------------------------------
# Restore the real starting state of a consumer Windows machine: no LLVM.
# --------------------------------------------------------------------------
Section "removing the runner image's pre-installed LLVM (a real user's PC has none)"
if (Test-Path 'C:\Program Files\LLVM') {
  Rename-Item -Path 'C:\Program Files\LLVM' -NewName 'LLVM.gh-preinstalled' -Force
  Write-Host "renamed C:\Program Files\LLVM -> C:\Program Files\LLVM.gh-preinstalled"
} else {
  Write-Host "no C:\Program Files\LLVM present"
}
$env:PATH = (($env:PATH -split ';') | Where-Object { $_ -and ($_ -notmatch 'LLVM') }) -join ';'
Remove-Item Env:\LIBCLANG_PATH -ErrorAction SilentlyContinue

$afterClang = (Get-Command clang -ErrorAction SilentlyContinue)
if ($afterClang) {
  Write-Host "CLANG_AFTER_REMOVAL=$($afterClang.Source)"
  clang --version
} else {
  Write-Host "CLANG_AFTER_REMOVAL=none"
}
Write-Host "LIBCLANG_PATH_AFTER_REMOVAL=[$env:LIBCLANG_PATH]"

# --------------------------------------------------------------------------
# Guide step 7 (reader step: "Install LLVM 18"), performed per variant.
# --------------------------------------------------------------------------
Section "guide step 7 (reader): Install LLVM 18 -- variant $VARIANT"
$LLVM_URL = 'https://github.com/llvm/llvm-project/releases/download/llvmorg-18.1.8/LLVM-18.1.8-win64.exe'

if ($VARIANT -eq 'no-llvm') {
  Write-Host "variant no-llvm: reader skipped the LLVM installer (as report f792b86d describes). Nothing installed."
} else {
  $llvmExe = Join-Path $env:RUNNER_TEMP 'LLVM-18.1.8-win64.exe'
  Write-Host "downloading $LLVM_URL"
  curl.exe -f -L -o $llvmExe $LLVM_URL
  Write-Host "download exit: $LASTEXITCODE"
  Write-Host "size: $((Get-Item $llvmExe).Length)"
  Write-Host "installing with the installer's DEFAULT options (/S drives NSIS with the default page values)"
  $p = Start-Process -FilePath $llvmExe -ArgumentList '/S' -Wait -PassThru
  Write-Host "LLVM_INSTALLER_EXIT=$($p.ExitCode)"
  if (Test-Path 'C:\Program Files\LLVM\bin\clang.exe') {
    Write-Host "LLVM_INSTALLED_AT=C:\Program Files\LLVM"
    & 'C:\Program Files\LLVM\bin\clang.exe' --version
  } else {
    Write-Host "LLVM_INSTALLED_AT=none (clang.exe not found at the default location)"
  }
  if (Test-Path 'C:\Program Files\LLVM\bin\libclang.dll') {
    Write-Host "LIBCLANG_DLL=C:\Program Files\LLVM\bin\libclang.dll"
  } else {
    Write-Host "LIBCLANG_DLL=none"
  }

  if ($VARIANT -eq 'step7-path') {
    # The diligent reader who ticks "Add LLVM to the system PATH".
    $env:PATH = 'C:\Program Files\LLVM\bin;' + $env:PATH
    Write-Host "variant step7-path: prepended C:\Program Files\LLVM\bin to PATH"
  } else {
    Write-Host "variant step7-default: PATH left exactly as the installer's defaults left it"
  }
}

# Did the installer put clang where the toolchain can see it?
$postClang = (Get-Command clang -ErrorAction SilentlyContinue)
if ($postClang) {
  Write-Host "CLANG_ON_PATH_AFTER_STEP7=$($postClang.Source)"
} else {
  Write-Host "CLANG_ON_PATH_AFTER_STEP7=none"
}
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
if ($installer) {
  Write-Host "INSTALLER_FOUND=$($installer.FullName)"
} else {
  Write-Host "INSTALLER_FOUND=none"
}

# Surface the diagnostic lines a reporter would see, for the evidence record.
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
