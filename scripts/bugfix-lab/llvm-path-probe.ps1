# Fidelity probe for cluster whimprflow-windows-build-toolchain.
#
# Question: when a reader performs the guide's step 7 (run
# LLVM-18.1.8-win64.exe and take the defaults), does LLVM end up on the PATH a
# FRESH PowerShell window would see? A process-local PATH cannot answer that,
# because an installer writes the machine/user PATH in the registry and only
# new shells pick it up. So this reads the registry values directly, before
# and after, and never relies on the current process environment.
$ErrorActionPreference = 'Continue'
$ProgressPreference = 'SilentlyContinue'

function Show($label) {
  Write-Host "--- $label"
  Write-Host ("MACHINE_PATH_HAS_LLVM=" + ([Environment]::GetEnvironmentVariable('Path','Machine') -match 'LLVM'))
  Write-Host ("USER_PATH_HAS_LLVM=" + ([Environment]::GetEnvironmentVariable('Path','User') -match 'LLVM'))
  Write-Host ("MACHINE_PATH_LLVM_ENTRIES=" + (([Environment]::GetEnvironmentVariable('Path','Machine') -split ';' | Where-Object { $_ -match 'LLVM' }) -join ' | '))
  Write-Host ("USER_PATH_LLVM_ENTRIES=" + (([Environment]::GetEnvironmentVariable('Path','User') -split ';' | Where-Object { $_ -match 'LLVM' }) -join ' | '))
}

Show "BEFORE anything (runner image state)"

if (Test-Path 'C:\Program Files\LLVM') {
  Rename-Item -Path 'C:\Program Files\LLVM' -NewName 'LLVM.gh-preinstalled' -Force
  Write-Host "renamed away the image's pre-installed LLVM"
}
# Take the image's own LLVM entries out of the registry PATHs too, so what we
# read afterwards is what the installer itself put there.
foreach ($scope in @('Machine','User')) {
  $v = [Environment]::GetEnvironmentVariable('Path',$scope)
  if ($v) {
    $cleaned = (($v -split ';') | Where-Object { $_ -and ($_ -notmatch 'LLVM') }) -join ';'
    [Environment]::SetEnvironmentVariable('Path',$cleaned,$scope)
  }
}
Show "AFTER removing the image's LLVM (a real consumer PC's starting state)"

$exe = Join-Path $env:RUNNER_TEMP 'LLVM-18.1.8-win64.exe'
curl.exe -f -L -o $exe https://github.com/llvm/llvm-project/releases/download/llvmorg-18.1.8/LLVM-18.1.8-win64.exe
Write-Host "DOWNLOAD_EXIT=$LASTEXITCODE SIZE=$((Get-Item $exe).Length)"
$p = Start-Process -FilePath $exe -ArgumentList '/S' -Wait -PassThru
Write-Host "LLVM_INSTALLER_EXIT=$($p.ExitCode)"
Write-Host ("CLANG_EXE_PRESENT=" + (Test-Path 'C:\Program Files\LLVM\bin\clang.exe'))
Write-Host ("LIBCLANG_DLL_PRESENT=" + (Test-Path 'C:\Program Files\LLVM\bin\libclang.dll'))
if (Test-Path 'C:\Program Files\LLVM\bin\clang.exe') { & 'C:\Program Files\LLVM\bin\clang.exe' --version }

Show "AFTER the guide's step 7 (installer defaults)"
Write-Host "PROBE_DONE"
