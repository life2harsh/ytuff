# RustPlayer

RustPlayer is a terminal music player for local files and YouTube Music. it has a full TUI, a background playback daemon, playlists, downloads, lyrics, artwork, media controls, and local library scanning.

first of all, shoutout @Metrolist. they made the API part of the project pretty easy. i personally hate kotlin, but i love what they made out of it. all my best wishes and prayers to you guys. 

fun fact: this originally started out as a SoundCloud terminal client, almost an year ago, or atleast i started planning it at that time. because the SoundCloud API said "if your application streams or uses SoundCloud data, it has to have the powered by SoundCloud logo" . well, i can't pay 15usd a month for artist pro. i'm broke. so then, i shifted to YT and well, here we are.

## Features

* Terminal UI for local music and YouTube Music
* Local library scanning
* YouTube Music search
* YouTube playlist, album, artist, and home/library support
* Background daemon for playback
* Queue, history, repeat, shuffle, autoplay, seeking, volume control, and sleep timer
* Playlists stored locally
* Lyrics support
* Downloads as `m4a` or `mp3`
* Artwork rendering through `wimg`, Kitty graphics, Sixel, or block art
* Windows and Linux login window for YouTube Music auth
* OS media controls on Windows and Linux
* Tray support on Windows
* FFmpeg backed streaming and local playback fallback

## Screenshots
### result screen
<img width="1897" height="1005" alt="image" src="https://github.com/user-attachments/assets/9fb52cf8-2e11-4229-a159-fea32858f022" />

### search:
<img width="1906" height="986" alt="image" src="https://github.com/user-attachments/assets/09b3ed67-b280-472d-bda4-9262ed8f6231" />

### help section
<img width="1276" height="802" alt="image" src="https://github.com/user-attachments/assets/858f0b6f-583c-4fd5-b4dc-59b6007ab552" />


```text
RustPlayer TUI
Local library + YouTube Music + queue + artwork
```

## Install

### Windows

Download the latest Windows release zip from Releases.

Extract it and run:

```powershell
.\rustplayer.exe tui
```

The Windows release is portable. It should include:

```text
rustplayer.exe
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

Do not delete the DLL files. They are required by the bundled `wimg.exe` renderer.

To make `rustplayer` available from any terminal, run:

```powershell
.\install-user.bat
```

Restart your terminal, then run:

```powershell
rustplayer tui
```

### Linux

Download the Linux tarball, extract it, and run:

```bash
./rustplayer tui
```

Install to your user PATH:

```bash
./install-user.sh
```

Then run:

```bash
rustplayer tui
```

Linux builds expect system dependencies to be installed through your distro package manager.

Arch / Manjaro:

```bash
sudo pacman -S ffmpeg webkit2gtk-4.1 gtk3 alsa-lib
```

Ubuntu / Debian:

```bash
sudo apt install ffmpeg libwebkit2gtk-4.1-0 libgtk-3-0 libasound2
```

If your distro does not package `libwebkit2gtk-4.1`, install the closest WebKitGTK 4.1 package available for your release.

## Quick start

Start the TUI:

```bash
rustplayer tui
```

Search YouTube Music:

```bash
rustplayer search "daft punk" --limit 10
```

Play something:

```bash
rustplayer play "never gonna give you up"
```

Pause, resume, skip, or stop:

```bash
rustplayer pause
rustplayer resume
rustplayer next
rustplayer stop
```

Check status:

```bash
rustplayer status
```

Stop the background daemon:

```bash
rustplayer shutdown
```

## Terminal artwork

RustPlayer supports multiple artwork renderers.

Set one manually:

Windows PowerShell:

```powershell
$env:RUSTPLAYER_ART="wimg"
.\rustplayer.exe tui
```

Linux shell:

```bash
RUSTPLAYER_ART=kitty ./rustplayer tui
```

Supported renderer values:

```text
wimg
kitty
sixel
blocks
off
```

Recommended values:

```text
Windows: wimg
Kitty terminal: kitty
Sixel terminal: sixel
Fallback: blocks
No artwork: off
```

The Windows release bundles `wimg.exe` and its DLLs. Linux builds do not need `wimg`; use `kitty`, `sixel`, or `blocks`.

To force a specific `wimg.exe` path on Windows:

```powershell
$env:RUSTPLAYER_WIMG="C:\path\to\wimg.exe"
```

The packaged Windows release should not need this because `wimg.exe` is bundled beside `rustplayer.exe`.

## TUI controls

Press `?` or `h` inside RustPlayer to open the built in help.

| Key              | Action                                                 |
| ---------------- | ------------------------------------------------------ |
| `s`              | Switch local / YouTube mode                            |
| `/`              | Search current mode                                    |
| `Enter`          | Play track, or open selected YouTube playlist or album |
| `Tab`            | Accept selected YouTube live suggestion                |
| `a`              | Add selected track to queue                            |
| `c`              | Clear queue                                            |
| `P`              | Play selected playlist or album                        |
| `Q`              | Queue selected playlist or album                       |
| `Space`          | Pause                                                  |
| `r`              | Resume                                                 |
| `n`              | Next track                                             |
| `p`              | Previous track                                         |
| `R`              | Cycle repeat off / all / one                           |
| `z`              | Toggle shuffle                                         |
| `Left` / `Right` | Seek 5 seconds                                         |
| `b` / `f`        | Seek 30 seconds                                        |
| `o`              | Open selected YouTube link                             |
| `i`              | Preview artwork with `wimg`                            |
| `S`              | Save the current playing song to Liked playlist        |
| `y`              | Open lyrics                                            |
| `D`              | Download selected track, playlist, or album            |
| `g`              | Load YouTube home                                      |
| `m`              | Load account playlists                                 |
| `u`              | Go back                                                |
| `A`              | Toggle autoplay                                        |
| `l`              | Open YouTube login window                              |
| `L`              | Sign out                                               |
| `+` / `-`        | Volume up / down                                       |
| `0`              | Mute                                                   |
| `d`              | Audio devices                                          |
| `F`              | Local folders                                          |
| `v`              | Visualizer                                             |
| `j` / `k`        | Move selection                                         |
| `J` / `K`        | Move inside queue                                      |
| `M`              | Minimize to tray                                       |
| `q`              | Quit or close overlay                                  |
| `Esc`            | Close overlay or cancel input                          |

## Local library

Add a folder:

```bash
rustplayer library add-path "/path/to/Music"
```

Windows example:

```powershell
rustplayer library add-path "D:\Music"
```

List folders:

```bash
rustplayer library list-paths
```

Remove a folder by index:

```bash
rustplayer library remove-path 0
```

You can also pass scan paths when starting the app:

```bash
rustplayer --path "/path/to/Music" tui
```

Supported local file extensions include:

```text
mp3, flac, wav, m4a, ogg, aac, opus, wma
```

RustPlayer uses FFmpeg as a fallback for formats that the native decoder does not handle cleanly.

## YouTube Music auth

Guest playback works for many tracks, but YouTube can block or limit some requests. Signing in gives RustPlayer access to personalized home, account playlists, and more reliable playback.

Open the login window:

```bash
rustplayer auth login
```

Show current auth state:

```bash
rustplayer auth show
```

Import a cookie file:

```bash
rustplayer auth cookie-file cookies.txt
```

Import a raw cookie header:

```bash
rustplayer auth cookie-header "SID=...; SAPISID=..."
```

Import `ytmusicapi` headers:

```bash
rustplayer auth headers-file headers.json
```

Sign out from the TUI with `L`.

## Queue

Add something to the queue:

```bash
rustplayer queue add "aphex twin xtal"
```

Show the queue:

```bash
rustplayer queue show
```

Clear the queue:

```bash
rustplayer queue clear
```

## Playlists

Create a playlist:

```bash
rustplayer playlist create mix
```

List playlists:

```bash
rustplayer playlist list
```

Show a playlist:

```bash
rustplayer playlist show mix
```

Add a track:

```bash
rustplayer playlist add mix "https://music.youtube.com/watch?v=lYBUbBu4W08"
```

Import a YouTube playlist or album:

```bash
rustplayer playlist import "https://music.youtube.com/playlist?list=..." --name my-playlist
```

Play a playlist:

```bash
rustplayer playlist play mix
```

Queue a playlist:

```bash
rustplayer playlist enqueue mix
```

Download a playlist:

```bash
rustplayer playlist download mix --format m4a
```

## Lyrics

Show lyrics for the current track:

```bash
rustplayer lyrics
```

Use cached lyrics only:

```bash
rustplayer lyrics --cached
```

Return JSON:

```bash
rustplayer lyrics --json
```

In the TUI, press `y` to open lyrics for the current track.

## Downloads

Download a track as M4A:

```bash
rustplayer download "https://music.youtube.com/watch?v=lYBUbBu4W08" --format m4a
```

Download as MP3:

```bash
rustplayer download "https://music.youtube.com/watch?v=lYBUbBu4W08" --format mp3
```

Choose an output folder:

```bash
rustplayer download "https://music.youtube.com/watch?v=lYBUbBu4W08" --format mp3 --output "/path/to/output"
```

Windows example:

```powershell
rustplayer download "https://music.youtube.com/watch?v=lYBUbBu4W08" --format mp3 --output "D:\Music"
```

## Autoplay and sleep timer

Enable autoplay:

```bash
rustplayer autoplay on
```

Disable autoplay:

```bash
rustplayer autoplay off
```

Set a sleep timer:

```bash
rustplayer sleep 30
```

Clear the sleep timer:

```bash
rustplayer sleep --off
```

## JSON output

Some commands support JSON output through the global `--json` flag.

```bash
rustplayer --json status
```

```bash
rustplayer --json search "boards of canada" --limit 5
```

## Configuration

Print the current config:

```bash
rustplayer config
```

RustPlayer stores config, playlist data, downloads, and cached lyrics in your OS app directories under `rustplayer`.

Important config values include:

```text
quality
scan_paths
autoplay
lyrics_enabled
auto_fetch_lyrics
daemon_addr
downloads_dir
youtube_cookie_header
youtube_cookie_file
youtube_auth_user
start_background_on_boot
```

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
taskkill /IM rustplayer.exe /F
```

## Build dependencies

### Windows

For normal development:

```powershell
cargo build --release
```

The Windows release bundles a prebuilt `wimg.exe`, its DLLs, `ffmpeg.exe`, and `ffprobe.exe`. You do not need to rebuild `wimg` for a normal RustPlayer release.

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

It builds RustPlayer, creates a clean release folder, copies the bundled `wimg` runtime files, copies FFmpeg, creates PATH installer scripts, and zips the result.

```powershell
cd H:\desktop\rustplayer

$source = "H:\desktop\rustplayer\target\release"
$dist = "H:\desktop\rustplayer\dist\rustplayer-windows-x64"
$zip = "H:\desktop\rustplayer\dist\rustplayer-windows-x64.zip"
$ffmpegDir = "C:\Users\jhaha\AppData\Local\Microsoft\WinGet\Packages\Gyan.FFmpeg_Microsoft.Winget.Source_8wekyb3d8bbwe\ffmpeg-8.0-full_build\bin"

cargo run --release -- shutdown 2>$null
taskkill /IM rustplayer.exe /F 2>$null

cargo build --release
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

Remove-Item $dist -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $dist | Out-Null

$requiredSourceFiles = @(
  "rustplayer.exe",
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
    Write-Host "RustPlayer is already in your user PATH."
} else {
    $newParts = @($parts + $AppDir)
    $newPath = ($newParts | Select-Object -Unique) -join ";"
    [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
    Write-Host "RustPlayer was added to your user PATH."
}

Write-Host ""
Write-Host "Restart your terminal, then run:"
Write-Host "  rustplayer tui"
Write-Host ""
Read-Host "Press Enter to close"
'@ | Set-Content -Path "$dist\install-user.ps1" -Encoding UTF8

@'
@echo off
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0install-user.ps1"
'@ | Set-Content -Path "$dist\install-user.bat" -Encoding ASCII

@'
RustPlayer Windows x64

Run:
  rustplayer.exe tui

To install rustplayer into your user PATH:
  Double-click install-user.bat

After installing:
  Restart your terminal
  Run: rustplayer tui

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
cd H:\desktop\rustplayer\dist\rustplayer-windows-x64
Remove-Item Env:RUSTPLAYER_WIMG -ErrorAction SilentlyContinue
$env:RUSTPLAYER_ART="wimg"
.\rustplayer.exe tui
```

A clean Windows release folder should contain:

```text
rustplayer.exe
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

set dist dist/rustplayer-linux-x86_64
set tarball dist/rustplayer-linux-x86_64-arch.tar.gz

rm -rf $dist
mkdir -p $dist

cp target/release/rustplayer $dist/rustplayer; or exit 1

printf '%s\n' \
'#!/usr/bin/env bash' \
'set -e' \
'' \
'APPDIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"' \
'mkdir -p "$HOME/.local/bin"' \
'ln -sf "$APPDIR/rustplayer" "$HOME/.local/bin/rustplayer"' \
'' \
'if [[ ":$PATH:" != *":$HOME/.local/bin:"* ]]; then' \
'  echo ""' \
'  echo "~/.local/bin is not currently in PATH."' \
'  echo "Add this to your shell config:"' \
'  echo '\''export PATH="$HOME/.local/bin:$PATH"'\''' \
'  echo ""' \
'fi' \
'' \
'echo "Installed rustplayer to ~/.local/bin/rustplayer"' \
'echo "Run: rustplayer tui"' \
> $dist/install-user.sh

chmod +x $dist/install-user.sh

printf '%s\n' \
'RustPlayer Linux x86_64' \
'' \
'Run:' \
'  ./rustplayer tui' \
'' \
'Install to user PATH:' \
'  ./install-user.sh' \
'' \
'Then run:' \
'  rustplayer tui' \
'' \
'Dependencies:' \
'  Arch / Manjaro: sudo pacman -S ffmpeg webkit2gtk-4.1 gtk3 alsa-lib' \
'  Ubuntu / Debian: sudo apt install ffmpeg libwebkit2gtk-4.1-0 libgtk-3-0 libasound2' \
'' \
'Artwork options:' \
'  RUSTPLAYER_ART=kitty ./rustplayer tui' \
'  RUSTPLAYER_ART=sixel ./rustplayer tui' \
'  RUSTPLAYER_ART=blocks ./rustplayer tui' \
'  RUSTPLAYER_ART=off ./rustplayer tui' \
> $dist/README.txt

rm -f $tarball
tar -C dist -czf $tarball rustplayer-linux-x86_64; or exit 1

echo "Built: $tarball"
ls -lh $tarball
```

Test the packaged Linux build:

```fish
cd dist/rustplayer-linux-x86_64
RUSTPLAYER_ART=kitty ./rustplayer tui
```

Fallback if your terminal graphics are not set up:

```fish
RUSTPLAYER_ART=blocks ./rustplayer tui
```

## Release files

Recommended release assets:

```text
rustplayer-windows-x64.zip
rustplayer-linux-x86_64-arch.tar.gz
```

If you build Linux on Ubuntu or Debian for broader compatibility, name it:

```text
rustplayer-linux-x86_64.tar.gz
```

A Linux binary built on Arch may depend on newer system libraries than older Ubuntu or Debian systems have. For public Linux releases, building on Ubuntu LTS or Debian stable is usually safer.

## Troubleshooting

### Artwork does not show on Windows

Use Windows Terminal and make sure Sixel support is available. Then force `wimg`:

```powershell
$env:RUSTPLAYER_ART="wimg"
.\rustplayer.exe tui
```

Make sure these files are beside `rustplayer.exe`:

```text
wimg.exe
libgcc_s_seh-1.dll
libjpeg-8.dll
libpng16-16.dll
libsixel-1.dll
libwinpthread-1.dll
zlib1.dll
```

### RustPlayer is using the wrong `wimg.exe`

Check PATH:

```powershell
where.exe wimg
```

Force the exact renderer:

```powershell
$env:RUSTPLAYER_WIMG="C:\path\to\wimg.exe"
```

For packaged releases, keep `wimg.exe` beside `rustplayer.exe`.

### Local files show up but do not play

Make sure FFmpeg is available.

Windows packaged release:

```text
ffmpeg.exe
ffprobe.exe
```

Linux:

```bash
ffmpeg -version
ffprobe -version
```

Test a local file directly:

```bash
ffmpeg -v error -i "/path/to/song.m4a" -f null -
```

On Windows:

```powershell
.\ffmpeg.exe -v error -i "D:\Music\song.m4a" -f null -
```

If FFmpeg prints nothing and exits, it can decode the file.

### Playback gets weird after rebuilding

Stop the old daemon:

```bash
rustplayer shutdown
```

On Windows:

```powershell
taskkill /IM rustplayer.exe /F
```

Then start again:

```bash
rustplayer tui
```

### Search works but playback does not

Search and playback are separate paths. Search can work while the daemon or FFmpeg playback path is broken.

Check:

```bash
rustplayer status
ffmpeg -version
```

Restart the daemon before testing again.

### YouTube playback is blocked

Try logging in:

```bash
rustplayer auth login
```

If needed, import headers:

```bash
rustplayer auth headers-file headers.json
```

### The Windows zip works on your machine but not on another PC

You probably forgot the `wimg` DLLs or FFmpeg files. Ship the full Windows file set listed above.

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