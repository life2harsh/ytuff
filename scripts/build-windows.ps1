# PowerShell script to build YTuff for Windows
# Run this script on Windows with Rust installed
# Includes wimg for image rendering

$ErrorActionPreference = "Stop"

Write-Host "=== YTuff Windows Builder ===" -ForegroundColor Cyan
Write-Host "Building version: $(cargo pkgid | Select-String -Pattern '#(.+)' | ForEach-Object { $_.Matches.Groups[1].Value })" -ForegroundColor Yellow

# Clean
Write-Host "[1/5] Cleaning..." -ForegroundColor Green
cargo clean

# Build for Windows (MSVC - native)
Write-Host "[2/5] Building release for Windows (x86_64-msvc)..." -ForegroundColor Green
cargo build --release --target x86_64-pc-windows-msvc

# Strip binary
Write-Host "[3/5] Stripping binary..." -ForegroundColor Green
$stripExe = "C:\Program Files\Git\usr\bin\strip.exe"
if (Test-Path $stripExe) {
    & $stripExe "target\x86_64-pc-windows-msvc\release\ytuff.exe"
}

# Create distribution
Write-Host "[4/5] Creating distribution..." -ForegroundColor Green
$distDir = "dist\ytuff-windows-x86_64"
New-Item -ItemType Directory -Force -Path $distDir | Out-Null
$releaseDir = "target\x86_64-pc-windows-msvc\release"

# Copy main binary
Copy-Item "$releaseDir\ytuff.exe" "$distDir\"

# Copy WebView2 loader for embedded YouTube login on Windows
$webview2Loader = Get-ChildItem -Path "target\x86_64-pc-windows-msvc\release\build" `
    -Recurse -Filter "WebView2Loader.dll" -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -like "*\out\x64\WebView2Loader.dll" } |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 1

if ($null -eq $webview2Loader) {
    Write-Host "WARNING: WebView2Loader.dll not found, YouTube login will fail on Windows" -ForegroundColor Red
} else {
    Copy-Item $webview2Loader.FullName "$distDir\" -Force
}

# Copy wimg and dependencies
Write-Host "Adding wimg for image rendering..." -ForegroundColor Yellow
$wimgSrc = ".tmp_wimg\wimg\build_wimg"
if (Test-Path "$wimgSrc\wimg.exe") {
    Copy-Item "$wimgSrc\wimg.exe" "$distDir\"
    Get-ChildItem "$wimgSrc\*.dll" | Copy-Item -Destination "$distDir\" -ErrorAction SilentlyContinue
    Write-Host "wimg added successfully" -ForegroundColor Green
} else {
    Write-Host "WARNING: wimg.exe not found, images will not render" -ForegroundColor Red
}

# Copy documentation
Copy-Item "README.md" "$distDir\"
Copy-Item "LICENSE" "$distDir\"

# Create ZIP
Write-Host "[5/5] Creating ZIP archive..." -ForegroundColor Green
$zipFile = "dist\ytuff-$(cargo pkgid | Select-String -Pattern '#(.+)' | ForEach-Object { $_.Matches.Groups[1].Value })-windows-x86_64.zip"
Compress-Archive -Path "$distDir\*" -DestinationPath $zipFile -Force

Write-Host "`n=== Build Complete ===" -ForegroundColor Cyan
Write-Host "Files created:" -ForegroundColor Yellow
Get-ChildItem "dist\" | Format-Table Name, Length, LastWriteTime

Write-Host "`nTo create installer, use NSIS with ytuff.nsi" -ForegroundColor Yellow
