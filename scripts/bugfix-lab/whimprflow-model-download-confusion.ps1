# Executes the guide's actual Windows speech-model steps (rendered v16, pin 44c6d39)
# and checks whether the reader ends up with exactly one correctly-named model file,
# the same acceptance criterion as $WORK/oracle.sh (macOS half), applied for real on
# Windows this time instead of being judged from source alone.
$ErrorActionPreference = "Stop"

# Step 13 (models-folder)
mkdir -Force "$env:APPDATA\WhimprFlow\models"

# Step 14 (download-model) -- verbatim guide command
if (!(Test-Path "$env:APPDATA\WhimprFlow\models\ggml-base.en.bin")) {
  curl.exe -f -L -o "$env:APPDATA\WhimprFlow\models\ggml-base.en.bin" https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin
}
$rc = $LASTEXITCODE
Write-Host "curl.exe exit code: $rc"

$dir = "$env:APPDATA\WhimprFlow\models"
$files = @()
if (Test-Path $dir) { $files = Get-ChildItem $dir -File }
Write-Host "models folder ($dir) holds $($files.Count) file(s):"
$files | ForEach-Object { Write-Host ("  {0}  {1} bytes" -f $_.Name, $_.Length) }

$accepted = @("ggml-large-v3-turbo.bin","ggml-medium.en.bin","ggml-small.en.bin","ggml-base.en.bin")
$ok = $false
if ($rc -eq 0 -and $files.Count -eq 1) {
  $f = $files[0]
  if ($accepted -contains $f.Name -and $f.Length -ge 100000000) {
    $bytes = [System.IO.File]::ReadAllBytes($f.FullName) | Select-Object -First 4
    $magic = -join ($bytes | ForEach-Object { [char]$_ })
    Write-Host "file=$($f.Name) size=$($f.Length) magic=$magic"
    if ($magic -eq "lmgg" -or $magic -eq "ggml") { $ok = $true }
  }
}

if ($ok) {
  Write-Host "BUGFIX_LAB_ABSENT"
  exit 0
} else {
  Write-Host "BUGFIX_LAB_PRESENT"
  exit 1
}
