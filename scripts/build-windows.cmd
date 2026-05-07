@echo off
REM Batch script to build YTuff for Windows (run on Windows)
REM Includes wimg for image rendering

echo === YTuff Windows Builder ===
cargo pkgid | findstr /R "#.*"

REM Clean
echo [1/5] Cleaning...
cargo clean

REM Build release
echo [2/5] Building release for Windows...
cargo build --release

REM Strip binary (if strip available)
echo [3/5] Creating distribution...
if exist "C:\Program Files\Git\usr\bin\strip.exe" (
    "C:\Program Files\Git\usr\bin\strip.exe" target\release\ytuff.exe
)

REM Create dist folder
if not exist "dist" mkdir dist
if exist "dist\ytuff-windows-x86_64" rmdir /s /q "dist\ytuff-windows-x86_64"
mkdir "dist\ytuff-windows-x86_64"

REM Copy main binary
copy "target\release\ytuff.exe" "dist\ytuff-windows-x86_64\"

REM Copy wimg and its dependencies
echo [4/5] Adding wimg for image rendering...
if exist ".tmp_wimg\wimg\build_wimg\wimg.exe" (
    copy ".tmp_wimg\wimg\build_wimg\wimg.exe" "dist\ytuff-windows-x86_64\"
    copy ".tmp_wimg\wimg\build_wimg\*.dll" "dist\ytuff-windows-x86_64\" 2>nul
    echo wimg added successfully
) else (
    echo WARNING: wimg.exe not found, images will not render
)

REM Copy documentation
copy "README.md" "dist\ytuff-windows-x86_64\"
copy "LICENSE" "dist\ytuff-windows-x86_64\"

REM Create ZIP (requires PowerShell)
echo [5/5] Creating ZIP archive...
powershell -Command "Compress-Archive -Path 'dist\ytuff-windows-x86_64\*' -DestinationPath 'dist\ytuff-windows.zip' -Force"

echo.
echo === Build Complete ===
dir dist\
