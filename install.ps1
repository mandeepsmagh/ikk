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

$Asset = "ikk-windows-${Arch}.zip"

# ── resolve version ────────────────────────────────────────────────────────
if ($Version -eq "latest") {
    $release = Invoke-RestMethod "https://api.github.com/repos/mandeepsmagh/ikk/releases/latest"
    $Version = $release.tag_name
}

$Base = "https://github.com/mandeepsmagh/ikk/releases/download/${Version}"
$Url = "${Base}/${Asset}"

Write-Host "ikk ${Version} -> ${InstallDir}\ikk.exe"
Write-Host "downloading ${Url}..."

# ── download + verify + install ────────────────────────────────────────────
$tmp = New-TemporaryFile | ForEach-Object { Remove-Item $_; New-Item $_ -ItemType Directory }

$zip = Join-Path $tmp "ikk.zip"
# curl.exe ships with Windows 10+ (and PowerShell 7 on older versions).
curl.exe -fsSL --retry 3 -o $zip $Url
if ($LASTEXITCODE -ne 0) {
    Write-Error "download failed: $Url"
    exit 1
}

# verify against the published SHA256SUMS
$sums = (curl.exe -fsSL "${Base}/SHA256SUMS") -split "`n"
$line = $sums | Where-Object { $_ -match [regex]::Escape($Asset) } | Select-Object -First 1
if (-not $line) {
    Write-Error "asset ${Asset} not found in SHA256SUMS"
    exit 1
}
$expected = ($line -split '\s+')[0].ToLower()
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

# add to user PATH permanently
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($userPath -notlike "*$InstallDir*") {
    [Environment]::SetEnvironmentVariable('Path', "$InstallDir;$userPath", 'User')
}

Write-Host ""
Write-Host "ikk installed to ${InstallDir}\ikk.exe"
Write-Host ""
Write-Host "open a new terminal and initialise:"
Write-Host "  ikk init --remote github.com"
