# nmem installer for Windows (PowerShell)
# Usage: irm https://raw.githubusercontent.com/viablesys/nmem/main/scripts/install.ps1 | iex
# Options (pass as env vars before piping, or run the script directly):
#   -Cuda    Install the CUDA-accelerated build
#
# Examples:
#   irm https://raw.githubusercontent.com/viablesys/nmem/main/scripts/install.ps1 | iex
#   & { $Cuda = $true; irm https://raw.githubusercontent.com/viablesys/nmem/main/scripts/install.ps1 | iex }

param(
    [switch]$Cuda
)

$ErrorActionPreference = "Stop"

$Repo      = "viablesys/nmem"
$InstallDir = "$env:USERPROFILE\.local\bin"

# Only x86_64 Windows binaries are distributed
$Arch = (Get-WmiObject Win32_Processor | Select-Object -First 1).AddressWidth
if ($Arch -ne 64) {
    Write-Error "Only x86_64 Windows is supported."
    exit 1
}

# Build artifact name
if ($Cuda) {
    $Target = "nmem-windows-x86_64-cuda"
} else {
    $Target = "nmem-windows-x86_64"
}

# Get latest release tag
Write-Host "Detecting latest release..."
$Release = Invoke-RestMethod "https://api.github.com/repos/$Repo/releases/latest"
$Tag = $Release.tag_name

if (-not $Tag) {
    Write-Error "Failed to detect latest release. Check https://github.com/$Repo/releases"
    exit 1
}

$Url = "https://github.com/$Repo/releases/download/$Tag/$Target.exe"
Write-Host "Downloading $Target.exe ($Tag)..."

# Create install directory
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
New-Item -ItemType Directory -Force -Path "$env:USERPROFILE\.nmem" | Out-Null

# Download
$Dest = "$InstallDir\nmem.exe"
Invoke-WebRequest -Uri $Url -OutFile $Dest

Write-Host ""
Write-Host "Installed nmem to $Dest"

# Check PATH
$UserPath = [Environment]::GetEnvironmentVariable("PATH", "User")
if ($UserPath -notlike "*$InstallDir*") {
    Write-Host ""
    Write-Host "WARNING: $InstallDir is not in your PATH."
    Write-Host "Adding it now for the current user..."
    [Environment]::SetEnvironmentVariable("PATH", "$InstallDir;$UserPath", "User")
    $env:PATH = "$InstallDir;$env:PATH"
    Write-Host "PATH updated. Restart your shell for it to take effect in new sessions."
}

Write-Host ""
Write-Host "Next: install the Claude Code plugin:"
Write-Host "  claude plugin marketplace add viablesys/claude-plugins"
Write-Host "  claude plugin install nmem@viablesys"
Write-Host ""
Write-Host "Then restart Claude Code."
