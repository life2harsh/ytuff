# YTuff Development

This file contains the build, packaging, release, repo layout, and ongoing development notes split out of the main user README.

## Build from source

Install Rust first.

Build:

```bash
cargo build --release
```

Run:

```bash
cargo run --release -- tui
```

Stop a stale daemon before testing playback changes:

```bash
cargo run --release -- shutdown
```

On Windows, if the daemon does not respond:

```powershell
taskkill /IM ytuff.exe /F
```

## Build dependencies

### Windows

For normal development:

```powershell
cargo build --release
```

The Windows release bundles a prebuilt `wimg.exe`, its DLLs, `ffmpeg.exe`, and `ffprobe.exe`. You do not need to rebuild `wimg` for a normal YTuff release.

### Arch / Manjaro

```bash
sudo pacman -Syu --needed base-devel pkgconf curl openssl alsa-lib systemd-libs gtk3 webkit2gtk-4.1 ffmpeg
```

### Ubuntu / Debian

```bash
sudo apt update
sudo apt install build-essential pkg-config curl ca-certificates libssl-dev libasound2-dev libudev-dev libgtk-3-dev libwebkit2gtk-4.1-dev ffmpeg
```

## Build a Windows release zip

Run this in PowerShell from the repository root.

It builds YTuff, creates a clean release folder, copies the bundled `wimg` runtime files, copies FFmpeg, creates PATH installer scripts, and zips the result.

```powershell
cd path\to\ytuff

$repo = (Get-Location).Path
$source = Join-Path $repo "target\release"
$dist = Join-Path $repo "dist\ytuff-windows-x64"
$zip = Join-Path $repo "dist\ytuff-windows-x64.zip"
$ffmpegDir = "C:\path\to\ffmpeg\bin"

cargo run --release -- shutdown 2>$null
taskkill /IM ytuff.exe /F 2>$null

cargo build --release
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

Remove-Item $dist -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $dist | Out-Null

$requiredSourceFiles = @(
  "ytuff.exe",
  "wimg.exe",
  "libgcc_s_seh-1.dll",
  "libjpeg-8.dll",
  "libpng16-16.dll",
  "libsixel-1.dll",
  "libwinpthread-1.dll",
  "zlib1.dll"
)

foreach ($file in $requiredSourceFiles) {
  $src = Join-Path $source $file
  if (-not (Test-Path $src)) {
    throw "Missing required file: $src"
  }
  Copy-Item $src $dist -Force
}

foreach ($file in @("ffmpeg.exe", "ffprobe.exe")) {
  $src = Join-Path $ffmpegDir $file
  if (-not (Test-Path $src)) {
    throw "Missing FFmpeg file: $src"
  }
  Copy-Item $src $dist -Force
}

@'
$AppDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$UserPath = [Environment]::GetEnvironmentVariable("Path", "User")

$parts = @()
if (-not [string]::IsNullOrWhiteSpace($UserPath)) {
    $parts = $UserPath -split ";" | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
}

$alreadyInstalled = $false
foreach ($part in $parts) {
    if ($part.TrimEnd("\") -ieq $AppDir.TrimEnd("\")) {
        $alreadyInstalled = $true
        break
    }
}

if ($alreadyInstalled) {
    Write-Host "YTuff is already in your user PATH."
} else {
    $newParts = @($parts + $AppDir)
    $newPath = ($newParts | Select-Object -Unique) -join ";"
    [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
    Write-Host "YTuff was added to your user PATH."
}

Write-Host ""
Write-Host "Restart your terminal, then run:"
Write-Host "  ytuff tui"
Write-Host ""
Read-Host "Press Enter to close"
'@ | Set-Content -Path "$dist\install-user.ps1" -Encoding UTF8

@'
@echo off
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0install-user.ps1"
'@ | Set-Content -Path "$dist\install-user.bat" -Encoding ASCII

@'
YTuff Windows x64

Run:
  ytuff.exe tui

To install ytuff into your user PATH:
  Double-click install-user.bat

After installing:
  Restart your terminal
  Run: ytuff tui

Do not delete these files:
  wimg.exe
  ffmpeg.exe
  ffprobe.exe
  *.dll
'@ | Set-Content -Path "$dist\README.txt" -Encoding UTF8

Remove-Item $zip -Force -ErrorAction SilentlyContinue
Compress-Archive -Path "$dist\*" -DestinationPath $zip -Force

Write-Host "Release folder: $dist"
Write-Host "Release zip: $zip"
Get-ChildItem $dist
```

Test the packaged Windows build:

```powershell
cd .\dist\ytuff-windows-x64
Remove-Item Env:YTUFF_WIMG -ErrorAction SilentlyContinue
$env:YTUFF_ART="wimg"
.\ytuff.exe tui
```

A clean Windows release folder should contain:

```text
ytuff.exe
wimg.exe
ffmpeg.exe
ffprobe.exe
libgcc_s_seh-1.dll
libjpeg-8.dll
libpng16-16.dll
libsixel-1.dll
libwinpthread-1.dll
zlib1.dll
install-user.bat
install-user.ps1
README.txt
```

Do not zip `target\release` directly. It contains Cargo build folders and temporary files.

## Build a Linux release tarball on Arch using fish

Run this in fish from your repository root.

```fish
sudo pacman -Syu --needed \
  base-devel \
  pkgconf \
  curl \
  openssl \
  alsa-lib \
  systemd-libs \
  gtk3 \
  webkit2gtk-4.1 \
  ffmpeg

if not command -q cargo
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    set -gx PATH $HOME/.cargo/bin $PATH
end

cargo clean; or exit 1
cargo build --release; or exit 1

set dist dist/ytuff-linux-x86_64
set tarball dist/ytuff-linux-x86_64-arch.tar.gz

rm -rf $dist
mkdir -p $dist

cp target/release/ytuff $dist/ytuff; or exit 1

printf '%s\n' \
'#!/usr/bin/env bash' \
'set -e' \
'' \
'APPDIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"' \
'mkdir -p "$HOME/.local/bin"' \
'ln -sf "$APPDIR/ytuff" "$HOME/.local/bin/ytuff"' \
'' \
'if [[ ":$PATH:" != *":$HOME/.local/bin:"* ]]; then' \
'  echo ""' \
'  echo "~/.local/bin is not currently in PATH."' \
'  echo "Add this to your shell config:"' \
'  echo '\''export PATH="$HOME/.local/bin:$PATH"'\''' \
'  echo ""' \
'fi' \
'' \
'echo "Installed ytuff to ~/.local/bin/ytuff"' \
'echo "Run: ytuff tui"' \
> $dist/install-user.sh

chmod +x $dist/install-user.sh

printf '%s\n' \
'YTuff Linux x86_64' \
'' \
'Run:' \
'  ./ytuff tui' \
'' \
'Install to user PATH:' \
'  ./install-user.sh' \
'' \
'Then run:' \
'  ytuff tui' \
'' \
'Dependencies:' \
'  Arch / Manjaro: sudo pacman -S ffmpeg webkit2gtk-4.1 gtk3 alsa-lib' \
'  Ubuntu / Debian: sudo apt install ffmpeg libwebkit2gtk-4.1-0 libgtk-3-0 libasound2' \
'' \
'Artwork options:' \
'  YTUFF_ART=kitty ./ytuff tui' \
'  YTUFF_ART=sixel ./ytuff tui' \
'  YTUFF_ART=blocks ./ytuff tui' \
'  YTUFF_ART=off ./ytuff tui' \
> $dist/README.txt

rm -f $tarball
tar -C dist -czf $tarball ytuff-linux-x86_64; or exit 1

echo "Built: $tarball"
ls -lh $tarball
```

Test the packaged Linux build:

```fish
cd dist/ytuff-linux-x86_64
YTUFF_ART=kitty ./ytuff tui
```

Fallback if your terminal graphics are not set up:

```fish
YTUFF_ART=blocks ./ytuff tui
```

## Release files

Recommended release assets:

```text
ytuff-windows-x64.zip
ytuff-linux-x86_64-arch.tar.gz
```

If you build Linux on Ubuntu or Debian for broader compatibility, name it:

```text
ytuff-linux-x86_64.tar.gz
```

A Linux binary built on Arch may depend on newer system libraries than older Ubuntu or Debian systems have. For public Linux releases, building on Ubuntu LTS or Debian stable is usually safer.

## Project layout

```text
src/
├─ appdata.rs
├─ attach.rs
├─ auth.rs
├─ daemon.rs
├─ discord_rpc.rs
├─ downloads.rs
├─ library_cache.rs
├─ lyrics.rs
├─ main.rs
├─ media_controls.rs
├─ playlist.rs
├─ resolve.rs
├─ tray.rs
├─ core/
├─ playback/
├─ sources/
└─ ui/
```

Main pieces:

* `main.rs`: CLI entry point and commands
* `ui/`: terminal UI
* `daemon.rs`: background playback process
* `attach.rs`: UI to daemon proxy
* `playback/`: audio output, FFmpeg streaming, local decode fallback
* `sources/`: local files and YouTube Music resolution
* `downloads.rs`: track and playlist downloads
* `lyrics.rs`: lyrics fetch and cache
* `playlist.rs`: local playlist store
* `appdata.rs`: config, data, cache, downloads paths


Next steps for development:
i know there will be some or even many errors when it reaches the masses, for any related problems, feel free to leave an Issue, I highly recommend opening an issue first rather than just sending me a PR, it would help me to keep track, and since it is an active project for me, I hope to learn by implementing new features and fixing my dumb logics.

My first few fixes would be re-engineering the lyrics api because it is a hit-or-miss. Then it would be to keep wimg clean to render, because right now it is pretty haywire. I barely managed to get it in a fixed dimension. 

one more thing, the visualiser doesn't work right now because of my streaming flow, but i'll be sure to atleast come up with a solution.

but don't you worry, i will put all my soul to make this perfect.
