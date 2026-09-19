# bugfix-lab NEGATIVE CONTROL for cluster: whimprflow-windows-build-toolchain
#
# Runs the OLD Windows guide steps EXACTLY as rendered from publik commit
# b9e2b4136536f6ec46c163a176f583b85ca5ec2e (lib/guides/whimprflow.ts, guide
# version 3, sourceCommit 3a9c737d09280e400338a8b8f4056fef99258a42) -- the
# guide pin that was live from before both bug reports (Aug 1 and Aug 7 2026)
# until the Aug 10 2026 01:33 PDT repin (commit 1c48d31 in publik). Report
# b3fb3031's own text ("Step 14- Build the installer ... ui\node_modules\.bin\
# tauri.CMD build") matches this guide's step numbering exactly (this guide's
# step 14 is "Build the installer"; the CURRENT guide's build step is #15),
# confirming this is the commit+guide-text the reporters actually had.
#
# Key differences from the current oracle script (whimprflow-windows-build-toolchain.ps1):
#  - dependencies step has NO `approve-builds --all` remediation line (that
#    was added by the Aug 10 campaign's guide repin, commit 1c48d31).
#  - no check-build-prereqs.mjs step (that script did not exist in the repo yet).
#  - guide step 7 ("Install LLVM") carries no specific-version guidance (the
#    current guide's step 7 is "Install LLVM 18" with a link pinned to 18.1.8).
#
# Exit 1 = bug PRESENT. Exit 0 = bug ABSENT. Prints BUGFIX_LAB_PRESENT/ABSENT.

$ErrorActionPreference = 'Continue'
$ProgressPreference = 'SilentlyContinue'

function Section($s) {
  Write-Host ""
  Write-Host "=== $s ==="
}

Section "repo state"
git rev-parse HEAD
git log -1 --format="%H %s"

Section "stock toolchain versions (no manual install - this is what the runner ships)"
node --version
git --version
try { rustc --version } catch { Write-Host "rustc: not found" }
try { cargo --version } catch { Write-Host "cargo: not found" }
try { cmake --version } catch { Write-Host "cmake: not found" }
$clangPath = (Get-Command clang -ErrorAction SilentlyContinue)
if ($clangPath) {
  Write-Host "clang path: $($clangPath.Source)"
  clang --version
} else {
  Write-Host "clang: not found on PATH"
}

Section "step 3: install pnpm (guide command)"
npm.cmd install -g pnpm
Write-Host "STEP3_EXIT=$LASTEXITCODE"
pnpm --version

Section "step 11: dependencies (OLD guide command, verbatim -- NO approve-builds line)"
Push-Location ui
pnpm.cmd install
$depExit1 = $LASTEXITCODE
Write-Host "PNPM_INSTALL_EXIT=$depExit1"
Pop-Location

Section "step 12: models folder (OLD guide command, verbatim)"
mkdir -Force "$env:APPDATA\WhimprFlow\models"

# Step 13 (download model) skipped, same rationale as the current oracle.

Section "step 14: BUILD THE INSTALLER (OLD guide command, verbatim - this is exactly what report b3fb3031 ran)"
ui\node_modules\.bin\tauri.CMD build 2>&1 | Tee-Object -FilePath build-output.log
$buildExit = $LASTEXITCODE
Write-Host "TAURI_BUILD_EXIT=$buildExit"

Section "searching for a produced installer"
$installer = Get-ChildItem -Path . -Recurse -Filter "*-setup.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $installer) {
  $installer = Get-ChildItem -Path . -Recurse -Include "*.exe","*.msi" -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -match "bundle" } | Select-Object -First 1
}
if ($installer) {
  Write-Host "INSTALLER_FOUND=$($installer.FullName)"
} else {
  Write-Host "INSTALLER_FOUND=none"
}

Section "verdict"
$buildLog = Get-Content -Raw build-output.log -ErrorAction SilentlyContinue
if (-not $buildLog) { $buildLog = "" }

Write-Host "tauri build exit code               : $buildExit"
Write-Host "installer produced                  : $([bool]$installer)"
Write-Host "PNPM_INSTALL_EXIT (step 11)          : $depExit1"

if ($buildExit -ne 0 -and -not $installer) {
  Write-Host "BUGFIX_LAB_PRESENT"
  exit 1
} elseif ($buildExit -eq 0 -and $installer) {
  Write-Host "BUGFIX_LAB_ABSENT"
  exit 0
} else {
  Write-Host "AMBIGUOUS_OUTCOME buildExit=$buildExit installer=$([bool]$installer)"
  Write-Host "BUGFIX_LAB_PRESENT"
  exit 1
}
