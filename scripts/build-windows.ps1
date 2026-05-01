# PowerShell script to build RustPlayer for Windows
# Run this script on Windows with Rust installed

$ErrorActionPreference = "Stop"

Write-Host "=== RustPlayer Windows Builder ===" -ForegroundColor Cyan
Write-Host "Building version: $(cargo pkgid | Select-String -Pattern '#(.+)' | ForEach-Object { $_.Matches.Groups[1].Value })" -ForegroundColor Yellow

# Clean
Write-Host "[1/4] Cleaning..." -ForegroundColor Green
cargo clean

# Build for Windows (MSVC - native)
Write-Host "[2/4] Building release for Windows (x86_64-msvc)..." -ForegroundColor Green
cargo build --release --target x86_64-pc-windows-msvc

# Build for Windows (GNU - if needed)
# cargo build --release --target x86_64-pc-windows-gnu

# Strip binary
Write-Host "[3/4] Stripping binary..." -ForegroundColor Green
$stripExe = "C:\Program Files\Git\usr\bin\strip.exe"
if (Test-Path $stripExe) {
    & $stripExe "target\x86_64-pc-windows-msvc\release\rustplayer.exe"
}

# Create distribution
Write-Host "[4/4] Creating distribution..." -ForegroundColor Green
$distDir = "dist\rustplayer-windows-x86_64"
New-Item -ItemType Directory -Force -Path $distDir | Out-Null
Copy-Item "target\x86_64-pc-windows-msvc\release\rustplayer.exe" "$distDir\"
Copy-Item "README.md" "$distDir\"
Copy-Item "LICENSE" "$distDir\"

# Create ZIP
$zipFile = "dist\rustplayer-$(cargo pkgid | Select-String -Pattern '#(.+)' | ForEach-Object { $_.Matches.Groups[1].Value })-windows-x86_64.zip"
Compress-Archive -Path "$distDir\*" -DestinationPath $zipFile -Force

Write-Host "`n=== Build Complete ===" -ForegroundColor Cyan
Write-Host "Files created:" -ForegroundColor Yellow
Get-ChildItem "dist\" | Format-Table Name, Length, LastWriteTime

Write-Host "`nTo create installer, use NSIS or WiX Toolset" -ForegroundColor Yellow
