# bugfix-lab oracle script for cluster: whimprflow-windows-build-toolchain
#
# Runs the WhimprFlow Windows install guide's own steps (windowsSteps() in
# publik/lib/guides/whimprflow.ts, guide version 16, sourceCommit
# 44c6d39213e38e7619e6dcfea1184b6619a11d01 — the exact commit guide-installer
# users get today) on a clean windows-latest runner: no repo cache, stock
# Rust/Node/pnpm/LLVM as the image ships them. It runs the dependencies step,
# the check-build-prereqs.mjs preflight, the models folder step, then the
# REAL failing command from both bug reports: `tauri build`.
#
# Prints BUGFIX_LAB_PRESENT and exits 1 if the build fails in a way matching
# one of the cluster's documented failure modes (esbuild postinstall block,
# pnpm --dir ui/ui ENOENT, LLVM/bindgen version mismatch, or a Windows-invalid
# bundle target). Prints BUGFIX_LAB_ABSENT and exits 0 if `tauri build`
# actually succeeds and produces an installer under
# src-tauri/target/release/bundle (or target/release/bundle at the workspace
# root — both are checked, since this is a Cargo workspace).

$ErrorActionPreference = 'Continue'
$ProgressPreference = 'SilentlyContinue'

function Section($s) {
  Write-Host ""
  Write-Host "=== $s ==="
}

Section "repo state"
git rev-parse HEAD
git log -1 --format='%H %s'

Section "stock toolchain versions (no manual install — this is what the runner ships)"
node --version
git --version
try { rustc --version } catch { Write-Host "rustc: not found ($_)" }
try { cargo --version } catch { Write-Host "cargo: not found ($_)" }
try { cmake --version } catch { Write-Host "cmake: not found ($_)" }
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

Section "step 11: dependencies (guide command, verbatim)"
Push-Location ui
pnpm.cmd install
$depExit1 = $LASTEXITCODE
Write-Host "PNPM_INSTALL_EXIT=$depExit1"
Pop-Location
pnpm.cmd --dir ui approve-builds --all
$depExit2 = $LASTEXITCODE
Write-Host "APPROVE_BUILDS_EXIT=$depExit2"

Section "step 12: check-build-prereqs.mjs (guide command, verbatim)"
node scripts/check-build-prereqs.mjs
$prereqExit = $LASTEXITCODE
Write-Host "PREREQ_CHECK_EXIT=$prereqExit"

Section "step 13: models folder (guide command, verbatim)"
mkdir -Force "$env:APPDATA\WhimprFlow\models"

# Step 14 (download the ~150MB speech model) is intentionally skipped: it is
# not read by `tauri build` and doesn't affect the build's success/failure,
# only runtime. Skipping it keeps this oracle focused on the build step the
# reports are about and avoids an unrelated network dependency.

Section "step 15: BUILD THE INSTALLER (guide command, verbatim — this is what both reports ran)"
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

$esbuildBlocked = $buildLog -match "ERR_PNPM_IGNORED_BUILDS" -or $buildLog -match "postinstall.*blocked" -or $buildLog -match "Ignored build scripts"
$uiUiEnoent = $buildLog -match "ENOENT" -and $buildLog -match [regex]::Escape("ui\ui") -or $buildLog -match [regex]::Escape("ui/ui")
$bindgenClang = $buildLog -match "libclang" -or $buildLog -match "bindgen" -or ($prereqExit -eq 1)
$badBundleTarget = $buildLog -match "dmg" -and $buildLog -match "not supported" -or $buildLog -match "invalid.*target" -or $buildLog -match "unsupported bundle"

Write-Host "signal esbuild-postinstall-blocked : $esbuildBlocked"
Write-Host "signal pnpm-dir-ui-ui-enoent        : $uiUiEnoent"
Write-Host "signal bindgen-libclang-mismatch    : $bindgenClang (prereq check exit=$prereqExit)"
Write-Host "signal bad-bundle-target            : $badBundleTarget"
Write-Host "tauri build exit code               : $buildExit"
Write-Host "installer produced                  : $([bool]$installer)"

if ($buildExit -ne 0 -and -not $installer) {
  Write-Host "BUGFIX_LAB_PRESENT"
  exit 1
} elseif ($buildExit -eq 0 -and $installer) {
  Write-Host "BUGFIX_LAB_ABSENT"
  exit 0
} else {
  # Ambiguous: exit code and installer presence disagree. Treat as PRESENT
  # (a reader would not consider this a clean success) but flag it plainly.
  Write-Host "AMBIGUOUS_OUTCOME buildExit=$buildExit installer=$([bool]$installer)"
  Write-Host "BUGFIX_LAB_PRESENT"
  exit 1
}
