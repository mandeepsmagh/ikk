# ikk install script for Windows (PowerShell)
# Usage: irm https://raw.githubusercontent.com/mandeepsmagh/ikk/main/install.ps1 | iex

param(
    [string]$Version = "latest",
    [string]$InstallDir = "$env:USERPROFILE\.ikk\bin"
)

$ErrorActionPreference = "Stop"

# ── detect architecture ────────────────────────────────────────────────────
$Arch = if ([Environment]::Is64BitOperatingSystem) { "x86_64" } else { "x86" }
if ($Arch -eq "x86") {
    Write-Error "32-bit Windows is not supported"
    exit 1
}

$Target = "${Arch}-pc-windows-msvc"
$Ext = ".zip"

# ── resolve version ────────────────────────────────────────────────────────
if ($Version -eq "latest") {
    $release = Invoke-RestMethod "https://api.github.com/repos/mandeepsmagh/ikk/releases/latest"
    $Version = $release.tag_name
}

$Url = "https://github.com/mandeepsmagh/ikk/releases/download/${Version}/ikk-${Target}${Ext}"

Write-Host "ikk ${Version} -> ${InstallDir}\ikk.exe"
Write-Host "downloading ${Url}..."

# ── download + verify + install ────────────────────────────────────────────
$tmp = New-TemporaryFile | ForEach-Object { Remove-Item $_; New-Item $_ -ItemType Directory }

$zip = Join-Path $tmp "ikk.zip"
Invoke-WebRequest -Uri $Url -OutFile $zip

# verify checksum
$hashUrl = "${Url}.sha256"
$expected = (Invoke-RestMethod $hashUrl).Split()[0]
$actual = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
if ($expected -ne $actual) {
    Write-Error "checksum mismatch!`n  expected: $expected`n  got:      $actual"
    exit 1
}

# extract
Expand-Archive $zip -DestinationPath $tmp

# install
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
Copy-Item "$tmp\ikk.exe" "$InstallDir\ikk.exe" -Force

# add to PATH for current session
$env:Path = "$InstallDir;$env:Path"

Write-Host ""
Write-Host "ikk installed to ${InstallDir}\ikk.exe"
Write-Host ""
Write-Host "PATH is set for this session. To make it permanent, run:"
Write-Host "  ikk init --remote github.com"
