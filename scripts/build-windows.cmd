@echo off
REM Batch script to build RustPlayer for Windows (run on Windows)

echo === RustPlayer Windows Builder ===
cargo pkgid | findstr /R "#.*"

REM Clean
echo [1/4] Cleaning...
cargo clean

REM Build release
echo [2/4] Building release for Windows...
cargo build --release

REM Strip binary (if strip available)
echo [3/4] Creating distribution...
if exist "C:\Program Files\Git\usr\bin\strip.exe" (
    "C:\Program Files\Git\usr\bin\strip.exe" target\release\rustplayer.exe
)

REM Create dist folder
if not exist "dist" mkdir dist
if exist "dist\rustplayer-windows-x86_64" rmdir /s /q "dist\rustplayer-windows-x86_64"
mkdir "dist\rustplayer-windows-x86_64"

REM Copy files
copy "target\release\rustplayer.exe" "dist\rustplayer-windows-x86_64\"
copy "README.md" "dist\rustplayer-windows-x86_64\"
copy "LICENSE" "dist\rustplayer-windows-x86_64\"

REM Create ZIP (requires PowerShell)
powershell -Command "Compress-Archive -Path 'dist\rustplayer-windows-x86_64\*' -DestinationPath 'dist\rustplayer-windows.zip' -Force"

echo.
echo === Build Complete ===
dir dist\
