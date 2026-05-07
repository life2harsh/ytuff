# YTuff

YTuff is a terminal music player for local files and YouTube Music. it has a full TUI, a background playback daemon, playlists, downloads, lyrics, artwork, media controls, and local library scanning.

first of all, shoutout @Metrolist. they made the API part of the project pretty easy. i personally hate kotlin, but i love what they made out of it. all my best wishes and prayers to you guys. 

fun fact: this originally started out as a SoundCloud terminal client, almost an year ago, or atleast i started planning it at that time. because the SoundCloud API said "if your application streams or uses SoundCloud data, it has to have the powered by SoundCloud logo" . well, i can't pay 15usd a month for artist pro. i'm broke. so then, i shifted to YT and well, here we are.

previously named rustplayer, because made in rust and music player, but since the name was taken, ytuff it is.

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

## Development and contributing
Build, packaging, release, repo layout, and development notes live in [DEVELOPMENT.md](DEVELOPMENT.md).

## Screenshots
### result screen
<img width="1897" height="1005" alt="image" src="https://github.com/user-attachments/assets/9fb52cf8-2e11-4229-a159-fea32858f022" />

### search:
<img width="1906" height="986" alt="image" src="https://github.com/user-attachments/assets/09b3ed67-b280-472d-bda4-9262ed8f6231" />

### help section
<img width="1276" height="802" alt="image" src="https://github.com/user-attachments/assets/858f0b6f-583c-4fd5-b4dc-59b6007ab552" />


```text
YTuff TUI
Local library + YouTube Music + queue + artwork
```

## Install

### Windows

Winget: Coming soon

Download the latest Windows release zip from Releases.

Extract it and run:

```powershell
.\ytuff.exe tui
```

The Windows release is portable. It should include:

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
```

Do not delete the DLL files. They are required by the bundled `wimg.exe` renderer.

To make `ytuff` available from any terminal, run:

```powershell
.\install-user.bat
```

Restart your terminal, then run:

```powershell
ytuff tui
```

### Linux

Arch User Repository(AUR)
```bash
yay -S ytuff-bin
yay -S ytuff
```
links to the packages: 
<https://aur.archlinux.org/packages/ytuff-bin>
<https://aur.archlinux.org/packages/ytuff>

Download the Linux tarball, extract it, and run:

```bash
./ytuff tui
```

Install to your user PATH:

```bash
./install-user.sh
```

Then run:

```bash
ytuff tui
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
ytuff tui
```

Search YouTube Music:

```bash
ytuff search "daft punk" --limit 10
```

Play something:

```bash
ytuff play "never gonna give you up"
```

Pause, resume, skip, or stop:

```bash
ytuff pause
ytuff resume
ytuff next
ytuff stop
```

Check status:

```bash
ytuff status
```

Stop the background daemon:

```bash
ytuff shutdown
```

## Terminal artwork

YTuff supports multiple artwork renderers.

Set one manually:

Windows PowerShell:

```powershell
$env:YTUFF_ART="wimg"
.\ytuff.exe tui
```

Linux shell:

```bash
YTUFF_ART=kitty ./ytuff tui
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
$env:YTUFF_WIMG="C:\path\to\wimg.exe"
```

The packaged Windows release should not need this because `wimg.exe` is bundled beside `ytuff.exe`.

## TUI controls

Press `?` or `h` inside YTuff to open the built in help.

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
ytuff library add-path "/path/to/Music"
```

Windows example:

```powershell
ytuff library add-path "D:\Music"
```

List folders:

```bash
ytuff library list-paths
```

Remove a folder by index:

```bash
ytuff library remove-path 0
```

You can also pass scan paths when starting the app:

```bash
ytuff --path "/path/to/Music" tui
```

Supported local file extensions include:

```text
mp3, flac, wav, m4a, ogg, aac, opus, wma
```

YTuff uses FFmpeg as a fallback for formats that the native decoder does not handle cleanly.

## YouTube Music auth

Guest playback works for many tracks, but YouTube can block or limit some requests. Signing in gives YTuff access to personalized home, account playlists, and more reliable playback.

Open the login window:

```bash
ytuff auth login
```

Show current auth state:

```bash
ytuff auth show
```

Import a cookie file:

```bash
ytuff auth cookie-file cookies.txt
```

Import a raw cookie header:

```bash
ytuff auth cookie-header "SID=...; SAPISID=..."
```

Import `ytmusicapi` headers:

```bash
ytuff auth headers-file headers.json
```

Sign out from the TUI with `L`.

## Queue

Add something to the queue:

```bash
ytuff queue add "aphex twin xtal"
```

Show the queue:

```bash
ytuff queue show
```

Clear the queue:

```bash
ytuff queue clear
```

## Playlists

Create a playlist:

```bash
ytuff playlist create mix
```

List playlists:

```bash
ytuff playlist list
```

Show a playlist:

```bash
ytuff playlist show mix
```

Add a track:

```bash
ytuff playlist add mix "https://music.youtube.com/watch?v=lYBUbBu4W08"
```

Import a YouTube playlist or album:

```bash
ytuff playlist import "https://music.youtube.com/playlist?list=..." --name my-playlist
```

Play a playlist:

```bash
ytuff playlist play mix
```

Queue a playlist:

```bash
ytuff playlist enqueue mix
```

Download a playlist:

```bash
ytuff playlist download mix --format m4a
```

## Lyrics

Show lyrics for the current track:

```bash
ytuff lyrics
```

Use cached lyrics only:

```bash
ytuff lyrics --cached
```

Return JSON:

```bash
ytuff lyrics --json
```

In the TUI, press `y` to open lyrics for the current track.

## Downloads

Download a track as M4A:

```bash
ytuff download "https://music.youtube.com/watch?v=lYBUbBu4W08" --format m4a
```

Download as MP3:

```bash
ytuff download "https://music.youtube.com/watch?v=lYBUbBu4W08" --format mp3
```

Choose an output folder:

```bash
ytuff download "https://music.youtube.com/watch?v=lYBUbBu4W08" --format mp3 --output "/path/to/output"
```

Windows example:

```powershell
ytuff download "https://music.youtube.com/watch?v=lYBUbBu4W08" --format mp3 --output "D:\Music"
```

## Autoplay and sleep timer

Enable autoplay:

```bash
ytuff autoplay on
```

Disable autoplay:

```bash
ytuff autoplay off
```

Set a sleep timer:

```bash
ytuff sleep 30
```

Clear the sleep timer:

```bash
ytuff sleep --off
```

## JSON output

Some commands support JSON output through the global `--json` flag.

```bash
ytuff --json status
```

```bash
ytuff --json search "boards of canada" --limit 5
```

## Configuration

Print the current config:

```bash
ytuff config
```

YTuff stores config, playlist data, downloads, and cached lyrics in your OS app directories under `ytuff`.

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

## Development

Build, packaging, release, repo layout, and ongoing development notes live in [DEVELOPMENT.md](DEVELOPMENT.md).

## Troubleshooting

### Artwork does not show on Windows

Use Windows Terminal and make sure Sixel support is available. Then force `wimg`:

```powershell
$env:YTUFF_ART="wimg"
.\ytuff.exe tui
```

Make sure these files are beside `ytuff.exe`:

```text
wimg.exe
libgcc_s_seh-1.dll
libjpeg-8.dll
libpng16-16.dll
libsixel-1.dll
libwinpthread-1.dll
zlib1.dll
```

### YTuff is using the wrong `wimg.exe`

Check PATH:

```powershell
where.exe wimg
```

Force the exact renderer:

```powershell
$env:YTUFF_WIMG="C:\path\to\wimg.exe"
```

For packaged releases, keep `wimg.exe` beside `ytuff.exe`.

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
ytuff shutdown
```

On Windows:

```powershell
taskkill /IM ytuff.exe /F
```

Then start again:

```bash
ytuff tui
```

### Search works but playback does not

Search and playback are separate paths. Search can work while the daemon or FFmpeg playback path is broken.

Check:

```bash
ytuff status
ffmpeg -version
```

Restart the daemon before testing again.

### YouTube playback is blocked

Try logging in:

```bash
ytuff auth login
```

If needed, import headers:

```bash
ytuff auth headers-file headers.json
```

### The Windows zip works on your machine but not on another PC

You probably forgot the `wimg` DLLs, `WebView2Loader.dll`, or the FFmpeg files. Ship the full Windows file set listed above.

