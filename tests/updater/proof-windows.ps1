# Updater proof on a Windows GitHub runner (updater-proof.yml): installs WhimprFlow N with its NSIS
# installer, runs N's updater self-test against a good feed and three bad ones, and checks what
# is on disk after each. Uses the release tools for the feed (scripts/updater-latest-json.mjs).
# The self-test opens no window, shows no sign-in gate and never starts the microphone.
#
#   powershell -File tests/updater/proof-windows.ps1 -N <N setup.exe> -N1 <N+1 setup.exe> -Out <dir>
# Env: FEED_PORT (the port the test builds were compiled to use). N+1's .sig must sit next to it
# (written by `tauri build` with the update key, as in windows.yml).
param(
  [Parameter(Mandatory = $true)][string]$N,
  [Parameter(Mandatory = $true)][string]$N1,
  [Parameter(Mandatory = $true)][string]$Out
)
$ErrorActionPreference = "Stop"
$Root = (Resolve-Path "$PSScriptRoot\..\..").Path
$Port = $env:FEED_PORT
New-Item -ItemType Directory -Force $Out | Out-Null
$Out = (Resolve-Path $Out).Path
$Feed = Join-Path $env:RUNNER_TEMP "feed"
New-Item -ItemType Directory -Force $Feed | Out-Null
$script:pass = 0; $script:fails = 0
$results = Join-Path $Out "results.md"
"| Check | Result | Detail |`n|---|---|---|" | Set-Content $results
function Check([string]$name, [bool]$ok, [string]$detail = "") {
  if ($ok) { $script:pass++; Write-Host "PASS  $name ($detail)"; "| $name | PASS | $detail |" | Add-Content $results }
  else { $script:fails++; Write-Host "FAIL  $name ($detail)"; "| $name | **FAIL** | $detail |" | Add-Content $results }
}
function Last([string]$file) {
  if (-not (Test-Path $file)) { return $null }
  $lines = @(Get-Content $file | Where-Object { $_.Trim() })
  if ($lines.Count -eq 0) { return $null }
  return $lines[-1] | ConvertFrom-Json
}
function Shot([string]$name) {
  try {
    Add-Type -AssemblyName System.Windows.Forms, System.Drawing
    $b = [System.Windows.Forms.SystemInformation]::VirtualScreen
    $bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($b.Left, $b.Top, 0, 0, $bmp.Size)
    $bmp.Save((Join-Path $Out "$name.png"), [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
  } catch { Write-Host "screenshot $name failed: $_" }
}
$InstallDir = Join-Path $env:LOCALAPPDATA "WhimprFlow"
function Version-Of([string]$base) { return ($base -replace '^WhimprFlow_', '' -replace '_x64-setup\.exe$', '') }
function Installed-Exe { Get-ChildItem $InstallDir -Filter *.exe -ErrorAction SilentlyContinue | Where-Object { $_.Name -notlike "uninstall*" } | Select-Object -First 1 }
function Disk-Version { $e = Installed-Exe; if ($e) { return $e.VersionInfo.ProductVersion } else { return "" } }
function Reg-Version { $r = Get-ItemProperty "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*" -ErrorAction SilentlyContinue | Where-Object { $_.DisplayName -like "WhimprFlow*" } | Select-Object -First 1; if ($r) { return $r.DisplayVersion } else { return "" } }
# The updater writes the installer to %TEMP%\<App>-<version>-updater-*\<App>-<version>-installer.exe
# and runs it from there (names from tauri-plugin-updater).
function Updater-Installers { Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Name -like "*-$vN1-installer" } }
function Updater-TempDirs { Get-ChildItem $env:TEMP -Directory -Filter "*-$vN1-updater-*" -ErrorAction SilentlyContinue }
# WhimprFlow processes: anything started from the install folder (the exe name is not assumed).
function App-Processes { Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path -like "$InstallDir\*" -and $_.Name -notlike "uninstall*" } }
# Windows builds use the single-instance plugin: a second start would hand over to a running
# copy and exit, so every self-test starts with no copy running.
function Stop-App { App-Processes | Stop-Process -Force -ErrorAction SilentlyContinue; Start-Sleep 1 }
# Runs the installed app's self-test; returns the exit code (124 on a 180 s timeout).
function SelfTest([string]$mode, [string]$report) {
  Stop-App
  Remove-Item $report -ErrorAction SilentlyContinue
  $env:PUBLIK_UPDATE_SELF_TEST = $mode
  $env:PUBLIK_UPDATE_SELF_TEST_OUT = $report
  $env:RUST_LOG = "info,tauri_plugin_updater=debug"
  $exe = Installed-Exe
  $p = Start-Process -FilePath $exe.FullName -PassThru -RedirectStandardOutput "$report.stdout" -RedirectStandardError "$report.log"
  $null = $p.Handle
  $done = $p.WaitForExit(180000)
  Remove-Item Env:PUBLIK_UPDATE_SELF_TEST, Env:PUBLIK_UPDATE_SELF_TEST_OUT, Env:RUST_LOG -ErrorAction SilentlyContinue
  if (-not $done) { $p.Kill(); return 124 }
  return $p.ExitCode
}

$vN = Version-Of (Split-Path $N -Leaf)
$vN1 = Version-Of (Split-Path $N1 -Leaf)
Write-Host "N=$vN  N+1=$vN1"

Write-Host "==> The feeds, with the release tool"
$pkg = Split-Path $N1 -Leaf
Copy-Item $N1 (Join-Path $Feed $pkg)
Copy-Item "$N1.sig" (Join-Path $Feed "$pkg.sig")
& node "$Root\scripts\updater-latest-json.mjs" --version $vN1 --allow-http --base-url "http://127.0.0.1:$Port/" --platform "windows-x86_64=$Feed\$pkg" --notes "updater proof" --out "$Feed\latest-good.json"
if ($LASTEXITCODE -ne 0) { throw "updater-latest-json refused the N+1 installer" }
& node "$Root\scripts\updater-latest-json.mjs" --version $vN1 --base-url "https://github.com/$env:GITHUB_REPOSITORY/releases/download/v$vN1/" --platform "windows-x86_64=$Feed\$pkg" --notes "updater proof" --out "$Out\latest-release-form.json"
# A signature over the same installer from another key.
$other = Join-Path $env:RUNNER_TEMP "other"
New-Item -ItemType Directory -Force $other | Out-Null
$tauri = "$Root\ui\node_modules\.bin\tauri.cmd"
& $tauri signer generate --ci -w "$other\other.key" | Out-Null
Copy-Item $N1 "$other\$pkg"
$savedKey = $env:TAURI_SIGNING_PRIVATE_KEY; $savedPw = $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD
Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY, Env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD -ErrorAction SilentlyContinue
& $tauri signer sign -f "$other\other.key" --app-version $vN1 "$other\$pkg" | Out-Null
$env:TAURI_SIGNING_PRIVATE_KEY = $savedKey; $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = $savedPw
if (-not (Test-Path "$other\$pkg.sig")) { throw "no foreign signature was made" }
& node "$Root\tests\updater\make-feeds.mjs" $Feed windows-x86_64 $pkg "$other\$pkg.sig"
if ($LASTEXITCODE -ne 0) { throw "make-feeds failed" }
Copy-Item "$Feed\latest-*.json" $Out

Write-Host "==> Install N"
$inst = Start-Process -FilePath $N -ArgumentList "/S" -PassThru -Wait
Check "N installs silently" ($inst.ExitCode -eq 0) "exit $($inst.ExitCode)"
Stop-App
$exe = Installed-Exe
Check "N is on disk" ($null -ne $exe -and (Disk-Version) -like "$vN*") "$($exe.FullName) $(Disk-Version)"

$server = Start-Process -FilePath python -ArgumentList "-m", "http.server", $Port, "--bind", "127.0.0.1", "--directory", $Feed -PassThru -RedirectStandardError "$Out\feed-server.log"
for ($i = 0; $i -lt 20; $i++) { try { Invoke-WebRequest "http://127.0.0.1:$Port/latest-good.json" -UseBasicParsing | Out-Null; break } catch { Start-Sleep 1 } }

$rc = SelfTest "version" "$Out\r-version-N.jsonl"; $r = Last "$Out\r-version-N.jsonl"
Check "N reports its version" ($r.version -eq $vN) "$($r.version)"
Copy-Item "$Feed\latest-good.json" "$Feed\latest.json" -Force
$rc = SelfTest "check" "$Out\r-check-N.jsonl"; $r = Last "$Out\r-check-N.jsonl"
Check "N finds N+1 on the feed" ($r.result -eq "available" -and $r.new_version -eq $vN1) "result=$($r.result) new=$($r.new_version)"

$reasons = @{ badsig = "created with a different key"; tampered = "signature verification failed"; version = "signed for version $vN1 but the update endpoint announced" }
foreach ($case in @("badsig", "tampered", "version")) {
  Copy-Item "$Feed\latest-$case.json" "$Feed\latest.json" -Force
  $hashBefore = (Get-FileHash (Installed-Exe).FullName).Hash
  $rc = SelfTest "install" "$Out\r-$case.jsonl"; $r = Last "$Out\r-$case.jsonl"
  Start-Sleep 3
  Check "[$case] the update is refused (exit 2)" ($rc -eq 2 -and $r.result -eq "error") "exit=$rc; $($r.error)"
  Check "[$case] refused for the right reason" ("$($r.error)" -like "*$($reasons[$case])*") $reasons[$case]
  $dirs = @(Updater-TempDirs)
  Check "[$case] no installer was written or started" ($null -eq (Updater-Installers) -and $dirs.Count -eq 0) "temp dirs: $($dirs.Count)"
  Check "[$case] the app on disk is still N" ((Disk-Version) -like "$vN*" -and (Get-FileHash (Installed-Exe).FullName).Hash -eq $hashBefore) "$(Disk-Version)"
}

Copy-Item "$Feed\latest-good.json" "$Feed\latest.json" -Force
$rc = SelfTest "install" "$Out\r-install.jsonl"; $r = Last "$Out\r-install.jsonl"
$selfTestPid = $r.pid
Check "[good] N downloads and verifies N+1, then starts its installer and exits" ($rc -eq 0 -and $r.result -eq "installing" -and $r.new_version -eq $vN1) "exit=$rc result=$($r.result) new=$($r.new_version)"
# The installer (passive: a progress bar, no questions) replaces the app and opens it again.
$deadline = (Get-Date).AddSeconds(180)
while ((Get-Date) -lt $deadline) {
  $busy = Updater-Installers
  if ((Disk-Version) -like "$vN1*" -and -not $busy) { break }
  Start-Sleep 2
}
@(App-Processes) + @(Updater-Installers) | Select-Object Name, Id, Path | Format-Table -AutoSize | Out-String | Set-Content "$Out\processes-after-install.txt"
Start-Sleep 10
# The installer reopens the app with the same environment, so the reopened app runs the
# self-test too and appends its own line (evidence of the restart; informational).
$lines = @(Get-Content "$Out\r-install.jsonl" | Where-Object { $_.Trim() } | ForEach-Object { $_ | ConvertFrom-Json })
$reopened = $lines | Where-Object { $_.version -eq $vN1 } | Select-Object -First 1
Write-Host "reopened by the installer: $(if ($reopened) { "yes, reported $($reopened.version) result=$($reopened.result)" } else { 'no self-test line (the app may have opened normally)' })"
"reopened_report=$(if ($reopened) { $reopened | ConvertTo-Json -Compress } else { 'none' })" | Set-Content "$Out\reopen.txt"
# The installer ran with /R: WhimprFlow must be running again, as a new process, from the install folder.
$again = @(App-Processes | Where-Object { $_.Id -ne $selfTestPid })
Check "[good] the installer opened WhimprFlow again (restart after install)" ($again.Count -ge 1 -or $null -ne $reopened) "pids: $(($again | ForEach-Object Id) -join ',')"
Check "[good] the installer has finished" ($null -eq (Updater-Installers)) ""
Stop-App
Check "[good] the exe on disk is N+1" ((Disk-Version) -like "$vN1*") "$(Disk-Version)"
Check "[good] the uninstall entry says N+1" ((Reg-Version) -eq $vN1) "$(Reg-Version)"
$rc = SelfTest "version" "$Out\r-version-after.jsonl"; $r = Last "$Out\r-version-after.jsonl"
Check "[good] the app on disk reports N+1" ($r.version -eq $vN1) "$($r.version)"
$rc = SelfTest "check" "$Out\r-check-after.jsonl"; $r = Last "$Out\r-check-after.jsonl"
Check "[good] N+1 finds no newer version" ($r.result -eq "none") "$($r.result)"
Check "[good] one install of WhimprFlow, not two" (@(Get-ChildItem $InstallDir -Filter *.exe | Where-Object { $_.Name -notlike "uninstall*" }).Count -eq 1) ""

Write-Host "==> The restart question, from N again (the background check as users get it)"
Stop-App
$un = Join-Path $InstallDir "uninstall.exe"
if (Test-Path $un) { Start-Process $un -ArgumentList "/S" -Wait }
Start-Sleep 2
Start-Process -FilePath $N -ArgumentList "/S" -Wait
Stop-App
$hashN = (Get-FileHash (Installed-Exe).FullName).Hash
$report = "$Out\r-prompt.jsonl"
Remove-Item $report -ErrorAction SilentlyContinue
$env:PUBLIK_UPDATE_SELF_TEST = "prompt"; $env:PUBLIK_UPDATE_SELF_TEST_OUT = $report
$p = Start-Process -FilePath (Installed-Exe).FullName -PassThru -RedirectStandardOutput "$report.stdout" -RedirectStandardError "$report.log"
Remove-Item Env:PUBLIK_UPDATE_SELF_TEST, Env:PUBLIK_UPDATE_SELF_TEST_OUT
$deadline = (Get-Date).AddSeconds(120)
while ((Get-Date) -lt $deadline -and -not $p.HasExited -and -not ((Test-Path $report) -and ((Get-Content $report -Raw) -match '"asking"'))) { Start-Sleep 1 }
Start-Sleep 5
$r = Last $report
$p.Refresh()
Check "[prompt] N asks and keeps running (no forced restart)" ($r.result -eq "asking" -and -not $p.HasExited -and $r.version -eq $vN) "result=$($r.result) running=$(-not $p.HasExited) version=$($r.version)"
Check "[prompt] nothing is installed before a yes" ((Disk-Version) -like "$vN*" -and (Get-FileHash (Installed-Exe).FullName).Hash -eq $hashN -and $r.install_waits_for_yes -eq $true) "$(Disk-Version), waits_for_yes=$($r.install_waits_for_yes)"
Check "[prompt] the question is on screen" ("$($p.MainWindowTitle)" -like "*update*") "window title '$($p.MainWindowTitle)'"
Shot "prompt-windows"
Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue

Stop-App
Stop-Process -Id $server.Id -Force -ErrorAction SilentlyContinue
if (Test-Path $un) { Start-Process $un -ArgumentList "/S" -Wait }
Write-Host ""
Write-Host "$($script:pass) passed, $($script:fails) failed"
if ($env:GITHUB_STEP_SUMMARY) { "### Windows updater proof: $($script:pass) passed, $($script:fails) failed`n" | Add-Content $env:GITHUB_STEP_SUMMARY; Get-Content $results | Add-Content $env:GITHUB_STEP_SUMMARY }
if ($script:fails -gt 0) { exit 1 }
exit 0
