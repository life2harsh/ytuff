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

## Build Windows release artifacts

Run this in PowerShell from the repository root.

The release scripts now build one verified Windows payload and fail if any required runtime file is missing, including `WebView2Loader.dll`, `wimg.exe`, the `wimg` DLLs, `ffmpeg.exe`, and `ffprobe.exe`.

```powershell
.\scripts\build-windows.ps1
.\scripts\build-windows-msi.ps1 -NoClean
```

Optional environment overrides:

```powershell
$env:YTUFF_WIMG_DIR="H:\path\to\wimg\build_wimg"
$env:YTUFF_FFMPEG_DIR="C:\path\to\ffmpeg\bin"
```

Outputs:

```text
dist\ytuff-windows-x64\
dist\ytuff-windows-x64.zip
dist\ytuff-windows-x64.msi
```

The staged Windows payload should contain:

```text
ytuff.exe
wimg.exe
ffmpeg.exe
ffprobe.exe
WebView2Loader.dll
libgcc_s_seh-1.dll
libjpeg-8.dll
libpng16-16.dll
libsixel-1.dll
libwinpthread-1.dll
zlib1.dll
install-user.bat
install-user.ps1
README.txt
LICENSE
```

Test the portable package:

```powershell
cd .\dist\ytuff-windows-x64
Remove-Item Env:YTUFF_WIMG -ErrorAction SilentlyContinue
$env:YTUFF_ART="wimg"
.\ytuff.exe tui
```

The MSI installs the same runtime payload under `Program Files\YTuff`, adds that folder to `PATH`, and registers uninstall support.

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
