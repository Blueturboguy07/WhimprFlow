# Installs WhimprFlow on Windows from the latest GitHub Release, in one line:
#
#   irm https://raw.githubusercontent.com/Blueturboguy07/WhimprFlow/main/install.ps1 | iex
#
# 1. Asks the GitHub API for the release (latest, or -Version) and picks its
#    Windows setup.exe (arm64 if the release has one and this PC is ARM,
#    otherwise x64).
# 2. Downloads it to %TEMP%, checks the size against the API, prints the
#    SHA-256 and checks it against the digest GitHub publishes for the asset.
# 3. Runs it with /S, the same silent install CI uses in
#    scripts/verify-windows.ps1. It installs per-user into
#    %LOCALAPPDATA%\WhimprFlow, so it needs no admin rights.
#
# The installer is not code-signed, so Windows SmartScreen may warn.
# Nothing is sent anywhere except the GitHub requests above.
#
# Options (run from a file, or through a scriptblock):
#   .\install.ps1 -Version 0.2.2        install that release instead of the latest
#   .\install.ps1 -DownloadOnly         download and check it, but do not install
#   & ([scriptblock]::Create((irm https://raw.githubusercontent.com/Blueturboguy07/WhimprFlow/main/install.ps1))) -DownloadOnly
#
# Works on Windows PowerShell 5.1 and PowerShell 7.
param(
  [string]$Version = "",
  [switch]$DownloadOnly
)

# Runs in its own scope so `irm | iex` leaves the caller's preferences alone,
# and throws rather than exits so a failure does not close the caller's window.
& {
  $ErrorActionPreference = 'Stop'
  $ProgressPreference = 'SilentlyContinue'  # the 5.1 progress bar makes a 130 MB download crawl
  [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

  $Repo = 'Blueturboguy07/WhimprFlow'
  $ReleasesPage = "https://github.com/$Repo/releases"

  function HttpStatus($err) {
    $resp = $err.Exception.Response
    if ($resp) { return [int]$resp.StatusCode }
    return 0
  }

  # Resolve the release
  if ($Version) {
    $tag = if ($Version.StartsWith('v')) { $Version } else { "v$Version" }
    $api = "https://api.github.com/repos/$Repo/releases/tags/$tag"
  } else {
    $api = "https://api.github.com/repos/$Repo/releases/latest"
  }

  try {
    $release = Invoke-RestMethod -Uri $api -Headers @{ Accept = 'application/vnd.github+json' } -UseBasicParsing
  } catch {
    $status = HttpStatus $_
    if ($status -eq 404) {
      if ($Version) { throw "No WhimprFlow release '$tag'. See $ReleasesPage for the versions that exist." }
      throw "No published WhimprFlow release found. See $ReleasesPage."
    }
    if ($status -eq 403 -or $status -eq 429) {
      throw "GitHub API rate limit hit (60 requests an hour without a login). Wait and retry, or download the installer from $ReleasesPage."
    }
    throw "Could not reach the GitHub API ($api): $($_.Exception.Message)"
  }

  $arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
  $asset = $null
  if ($arch -eq 'ARM64') {
    $asset = $release.assets | Where-Object { $_.name -match '_arm64-setup\.exe$' } | Select-Object -First 1
    if (-not $asset) { Write-Host "No arm64 build in $($release.tag_name); using x64 (Windows 11 on ARM runs it under emulation)." }
  }
  if (-not $asset) {
    $asset = $release.assets | Where-Object { $_.name -match '_x64-setup\.exe$' } | Select-Object -First 1
  }
  if (-not $asset) {
    throw "Release $($release.tag_name) has no Windows installer (*_x64-setup.exe). See $ReleasesPage."
  }

  Write-Host "WhimprFlow $($release.tag_name)"
  Write-Host "  $($asset.browser_download_url)"

  # Download and check
  $file = Join-Path $env:TEMP $asset.name
  try {
    Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $file -UseBasicParsing
  } catch {
    throw "Download failed ($($asset.browser_download_url)): $($_.Exception.Message)"
  }

  $size = (Get-Item $file).Length
  if ($size -ne $asset.size) {
    Remove-Item -Force $file
    throw "Download is $size bytes, the release says $($asset.size). Try again."
  }
  $sha256 = (Get-FileHash $file -Algorithm SHA256).Hash.ToLower()
  Write-Host "  $size bytes (as the release says), SHA-256 $sha256"

  # GitHub publishes a sha256 digest for each release asset.
  if ($asset.digest -and $asset.digest.StartsWith('sha256:')) {
    if ($asset.digest.Substring(7).ToLower() -ne $sha256) {
      Remove-Item -Force $file
      throw "SHA-256 does not match the digest GitHub publishes for this asset ($($asset.digest)). Not installing."
    }
    Write-Host "  matches the digest on the release"
  }

  if ($DownloadOnly) {
    Write-Host "Downloaded to $file (not installed: -DownloadOnly)."
    return
  }

  # Install
  Write-Host "Installing to $env:LOCALAPPDATA\WhimprFlow ..."
  Write-Host "The installer is not code-signed. If Windows SmartScreen warns, click More info, then Run anyway."
  $p = Start-Process -FilePath $file -ArgumentList '/S' -Wait -PassThru
  Remove-Item -Force $file -ErrorAction SilentlyContinue
  if ($p.ExitCode -ne 0) {
    throw "The installer exited with code $($p.ExitCode). Close WhimprFlow if it is running and try again, or run the installer from $ReleasesPage."
  }
  Write-Host "Installed WhimprFlow $($release.tag_name). Open it from the Start menu, then hold Right Ctrl to dictate."
}
