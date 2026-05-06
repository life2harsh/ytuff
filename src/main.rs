mod appdata;
mod attach;
mod auth;
mod core;
mod daemon;
mod discord_rpc;
mod downloads;
mod library_cache;
mod lyrics;
mod media_controls;
mod playback;
mod playlist;
mod proxy;
mod resolve;
mod sources;
mod tray;
mod ui;

use crate::appdata::{AppConfig, AppPaths};
use crate::auth::youtube_login_window;
use crate::core::track::{Acc, Track};
use crate::core::Core;
use crate::daemon::{send_request, RpcRequest};
use crate::downloads::{download_track, DownloadFormat};
use crate::lyrics::LyricsClient;
use crate::playlist::PlaylistStore;
use crate::sources::soundcloud::{Ql, SoundCloudClient};
use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
#[cfg(target_os = "linux")]
use libc::{close, dup, dup2, STDERR_FILENO};
use serde::Serialize;
use serde_json::Value;
use std::fs;
#[cfg(target_os = "linux")]
use std::fs::OpenOptions;
#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::process::Command as ProcessCommand;
use std::sync::{Arc, Mutex};
use url::Url;

#[derive(Parser, Debug)]
#[command(
    name = "rustplayer",
    version,
    about = "A high-performance terminal music player with YouTube streaming",
    after_help = "Auth note:\n  On Windows and Linux, RustPlayer can open a dedicated YouTube Music login window with\n  'rustplayer auth login'.\n  Manual cookie login is still supported through\n  'rustplayer auth cookie-file <cookies.txt>' or\n  'rustplayer auth cookie-header \"SID=...; SAPISID=...\"'.\n  You can also import ytmusicapi headers.json via\n  'rustplayer auth headers-file <headers.json>'.\n\nProxy note:\n  Set RUSTPLAYER_PROXY to a standard proxy URL when native requests need a tunnel,\n  for example 'socks5://127.0.0.1:1080' or 'http://127.0.0.1:8080'.\n\nExamples:\n  rustplayer auth login\n  rustplayer auth show\n  rustplayer auth headers-file headers.json\n  rustplayer status\n  rustplayer play \"never gonna give you up\"\n  rustplayer like\n  rustplayer download \"https://music.youtube.com/watch?v=lYBUbBu4W08\" --format mp3\n  rustplayer playlist create mix"
)]
struct Cli {
    #[arg(short = 'p', long = "path", global = true, value_name = "DIR")]
    paths: Vec<PathBuf>,
    #[arg(short = 'q', long = "quality", global = true, value_name = "QUALITY")]
    quality: Option<String>,
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    Tui,
    Daemon,
    #[command(hide = true)]
    Tray,
    Status,
    Search {
        query: String,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    Play {
        input: String,
    },
    Like {
        input: Option<String>,
    },
    Pause,
    Resume,
    Stop,
    Next,
    Prev,
    Queue {
        #[command(subcommand)]
        command: QueueCommand,
    },
    Playlist {
        #[command(subcommand)]
        command: PlaylistCommand,
    },
    Lyrics {
        input: Option<String>,
        #[arg(long)]
        cached: bool,
        #[arg(long)]
        json: bool,
    },
    Download {
        input: String,
        #[arg(long, value_enum, default_value_t = DownloadFormatArg::M4a)]
        format: DownloadFormatArg,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    Library {
        #[command(subcommand)]
        command: LibraryCommand,
    },
    Autoplay {
        state: ToggleState,
    },
    Sleep {
        minutes: Option<u64>,
        #[arg(long)]
        off: bool,
    },
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    Startup {
        state: ToggleState,
    },
    Config,
    Shutdown,
}

#[derive(Subcommand, Debug)]
enum QueueCommand {
    Add { input: String },
    Show,
    Clear,
}

#[derive(Subcommand, Debug)]
enum PlaylistCommand {
    Create {
        name: String,
    },
    List,
    Show {
        name: String,
    },
    Add {
        name: String,
        input: String,
    },
    Import {
        input: String,
        #[arg(long)]
        name: Option<String>,
    },
    Sync {
        name: String,
    },
    Play {
        name: String,
    },
    Enqueue {
        name: String,
    },
    Download {
        name: String,
        #[arg(long, value_enum, default_value_t = DownloadFormatArg::M4a)]
        format: DownloadFormatArg,
        #[arg(long)]
        output: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
enum LibraryCommand {
    AddPath { path: PathBuf },
    RemovePath { index: usize },
    ListPaths,
}

#[derive(Subcommand, Debug)]
enum AuthCommand {
    Login,
    CookieFile { path: PathBuf },
    CookieHeader { header: String },
    HeadersFile { path: PathBuf },
    Clear,
    Show,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ToggleState {
    On,
    Off,
}

impl ToggleState {
    fn is_on(self) -> bool {
        matches!(self, Self::On)
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum DownloadFormatArg {
    M4a,
    Mp3,
}

impl From<DownloadFormatArg> for DownloadFormat {
    fn from(value: DownloadFormatArg) -> Self {
        match value {
            DownloadFormatArg::M4a => Self::M4a,
            DownloadFormatArg::Mp3 => Self::Mp3,
        }
    }
}

#[derive(Serialize)]
struct SearchResults {
    local: Vec<Track>,
    youtube: Vec<Track>,
}

#[cfg(target_os = "linux")]
struct NativeStderrGuard {
    saved_fd: i32,
    _log_file: fs::File,
}

#[cfg(target_os = "linux")]
impl Drop for NativeStderrGuard {
    fn drop(&mut self) {
        unsafe {
            dup2(self.saved_fd, STDERR_FILENO);
            close(self.saved_fd);
        }
    }
}

#[cfg(target_os = "linux")]
fn redirect_native_stderr(paths: &AppPaths) -> Option<NativeStderrGuard> {
    fs::create_dir_all(&paths.cache_dir).ok()?;
    let log_path = paths.cache_dir.join("native-stderr.log");
    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .ok()?;

    let saved_fd = unsafe { dup(STDERR_FILENO) };
    if saved_fd < 0 {
        return None;
    }

    if unsafe { dup2(log_file.as_raw_fd(), STDERR_FILENO) } < 0 {
        unsafe {
            close(saved_fd);
        }
        return None;
    }

    Some(NativeStderrGuard {
        saved_fd,
        _log_file: log_file,
    })
}

#[cfg(not(target_os = "linux"))]
fn redirect_native_stderr(_paths: &AppPaths) -> Option<()> {
    None
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut runtime = Runtime::load(cli.paths.clone(), cli.quality.clone())?;

    match &cli.command {
        None | Some(Command::Tui) => run_tui(&runtime),
        Some(Command::Daemon) => daemon::run_daemon(runtime.paths.clone(), runtime.cfg.clone()),
        Some(Command::Tray) => tray::run_tray(runtime.paths.clone(), runtime.cfg.clone()),
        Some(Command::Status) => show_status(&runtime, cli.json),
        Some(Command::Search { query, limit }) => search_tracks(&runtime, query, *limit, cli.json),
        Some(Command::Play { input }) => play_input(&runtime, input),
        Some(Command::Like { input }) => like_command(&runtime, input.as_deref()),
        Some(Command::Pause) => simple_request(&runtime, &RpcRequest::Pause, "paused"),
        Some(Command::Resume) => simple_request(&runtime, &RpcRequest::Resume, "resumed"),
        Some(Command::Stop) => simple_request(&runtime, &RpcRequest::Stop, "stopped"),
        Some(Command::Next) => simple_request(&runtime, &RpcRequest::Next, "skipped"),
        Some(Command::Prev) => simple_request(&runtime, &RpcRequest::Prev, "returned"),
        Some(Command::Queue { command }) => queue_command(&runtime, command, cli.json),
        Some(Command::Playlist { command }) => playlist_command(&runtime, command, cli.json),
        Some(Command::Lyrics {
            input,
            cached,
            json,
        }) => lyrics_command(&runtime, input.as_deref(), *cached, *json),
        Some(Command::Download {
            input,
            format,
            output,
        }) => download_input(&runtime, input, (*format).into(), output.as_deref()),
        Some(Command::Library { command }) => {
            let changed = library_command(&mut runtime, command, cli.json)?;
            if changed {
                maybe_restart_daemon(&runtime)?;
            }
            Ok(())
        }
        Some(Command::Autoplay { state }) => simple_request(
            &runtime,
            &RpcRequest::SetAutoplay {
                enabled: state.is_on(),
            },
            if state.is_on() {
                "autoplay enabled"
            } else {
                "autoplay disabled"
            },
        ),
        Some(Command::Sleep { minutes, off }) => {
            let request = RpcRequest::SetSleep {
                minutes: if *off { None } else { *minutes },
            };
            let message = if *off {
                "sleep timer cleared"
            } else {
                "sleep timer updated"
            };
            simple_request(&runtime, &request, message)
        }
        Some(Command::Auth { command }) => {
            let changed = auth_command(&mut runtime, command, cli.json)?;
            if changed {
                maybe_restart_daemon(&runtime)?;
            }
            Ok(())
        }
        Some(Command::Startup { state }) => {
            runtime.cfg.start_background_on_boot = state.is_on();
            runtime.persist()?;
            println!("{}", configure_startup(state.is_on())?);
            Ok(())
        }
        Some(Command::Config) => print_json_or_text(&runtime.cfg, cli.json, || {
            let downloads = runtime.cfg.effective_downloads_dir(&runtime.paths);
            format!(
                "quality: {}\nscan paths: {}\ndownloads: {}\nautoplay: {}\nlyrics: {}\nbackground on boot: {}",
                runtime.cfg.quality,
                runtime.cfg.scan_paths.len(),
                downloads.display(),
                runtime.cfg.autoplay,
                runtime.cfg.lyrics_enabled,
                runtime.cfg.start_background_on_boot
            )
        }),
        Some(Command::Shutdown) => shutdown_daemon(&runtime),
    }
}

struct Runtime {
    paths: AppPaths,
    cfg: AppConfig,
}

struct RemoteCollection {
    title: String,
    tracks: Vec<Track>,
    source_url: String,
}

impl Runtime {
    fn load(extra_paths: Vec<PathBuf>, quality: Option<String>) -> Result<Self> {
        let paths = AppPaths::discover();
        let mut cfg = AppConfig::load(&paths)?;
        let mut dirty = false;

        if let Some(quality) = quality {
            let quality = quality.trim().to_ascii_lowercase();
            if !quality.is_empty() && cfg.quality != quality {
                cfg.quality = quality;
                dirty = true;
            }
        }

        for path in extra_paths {
            let canon = normalize_path(&path)?;
            if !cfg.scan_paths.iter().any(|item| same_path(item, &canon)) {
                cfg.scan_paths.push(canon);
                dirty = true;
            }
        }

        if dirty {
            cfg.save(&paths)?;
        }

        Ok(Self { paths, cfg })
    }

    fn persist(&self) -> Result<()> {
        self.cfg.save(&self.paths)
    }

    fn make_client(&self) -> Result<SoundCloudClient> {
        let mut client = SoundCloudClient::new(Ql::parse(&self.cfg.quality));
        client.set_cookie_header(self.cfg.cookie_header()?);
        client.set_auth_user(self.cfg.youtube_auth_user.clone());
        Ok(client)
    }

    fn build_core(&self) -> Core {
        let core = Core::new();
        let downloads = self.cfg.effective_downloads_dir(&self.paths);
        if downloads.exists() {
            if let Some(path_str) = downloads.to_str() {
                let _ = core.add_scan_path(path_str);
            }
        }
        let scan_paths = self.cfg.scan_paths.clone();

        match crate::library_cache::scan_paths_cached(&self.paths, &scan_paths) {
            Ok(tracks) => {
                core.put_tracks(tracks);
                *core.scan_paths.lock().unwrap() = scan_paths
                    .into_iter()
                    .filter(|path| path.exists() && path.is_dir())
                    .collect();
            }
            Err(_) => {
                for path in &scan_paths {
                    if let Some(path_str) = path.to_str() {
                        let _ = core.add_scan_path(path_str);
                    }
                }
            }
        }

        core
    }

    fn resolve_track(&self, input: &str) -> Result<Track> {
        let core = self.build_core();
        let mut client = self.make_client()?;
        resolve::resolve_input(&core, &mut client, input)
    }
}

fn resolve_remote_collection(runtime: &Runtime, input: &str) -> Result<RemoteCollection> {
    let browse_id = remote_browse_id(input).ok_or_else(|| anyhow!("not a remote collection"))?;
    let mut client = runtime.make_client()?;
    let (title, items) = client.browse_page(&browse_id, 200)?;
    let tracks = items
        .into_iter()
        .filter(|track| track.is_playable_remote() && track.acc != Some(Acc::Block))
        .collect::<Vec<_>>();
    if tracks.is_empty() {
        return Err(anyhow!("No playable tracks were found in that collection"));
    }

    let source_url = remote_collection_url(input, &browse_id);
    let title = if title.trim().is_empty() {
        source_url.clone()
    } else {
        title
    };

    Ok(RemoteCollection {
        title,
        tracks,
        source_url,
    })
}

fn remote_browse_id(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with("ytb:") || trimmed.starts_with("VL") || trimmed.starts_with("MPRE") {
        return Some(normalize_browse_id(trimmed));
    }

    let url = Url::parse(trimmed).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    if !host.contains("youtube.com") && !host.contains("youtu.be") {
        return None;
    }

    if let Some(list_id) = url
        .query_pairs()
        .find_map(|(key, value)| (key == "list").then(|| value.into_owned()))
        .filter(|value| !value.trim().is_empty())
    {
        return Some(normalize_browse_id(&list_id));
    }

    let mut segments = url.path_segments()?;
    match (segments.next(), segments.next()) {
        (Some("browse"), Some(id)) if !id.trim().is_empty() => Some(normalize_browse_id(id)),
        _ => None,
    }
}

fn normalize_browse_id(raw: &str) -> String {
    let raw = raw.trim().trim_start_matches("ytb:");
    if raw.starts_with("VL")
        || raw.starts_with("MPRE")
        || raw.starts_with("MPLA")
        || raw.starts_with("UC")
        || raw.starts_with("FEmusic_")
    {
        raw.to_string()
    } else {
        format!("VL{raw}")
    }
}

fn remote_collection_url(input: &str, browse_id: &str) -> String {
    let trimmed = input.trim();
    if Url::parse(trimmed).is_ok() {
        return trimmed.to_string();
    }
    if let Some(playlist_id) = browse_id.strip_prefix("VL") {
        format!("https://music.youtube.com/playlist?list={playlist_id}")
    } else {
        format!("https://music.youtube.com/browse/{browse_id}")
    }
}

fn run_tui(runtime: &Runtime) -> Result<()> {
    let _native_stderr = redirect_native_stderr(&runtime.paths);
    daemon::ensure_daemon(&runtime.paths)?;
    let core = runtime.build_core();
    let ui_client = Arc::new(Mutex::new(runtime.make_client()?));
    let playback_client = Arc::new(Mutex::new(runtime.make_client()?));
    let shared_cfg = Arc::new(Mutex::new(runtime.cfg.clone()));
    let playback =
        attach::start_daemon_playback_proxy(core.clone(), runtime.cfg.daemon_addr.clone());
    ui::run_ui(
        core,
        playback,
        ui_client,
        playback_client,
        runtime.paths.clone(),
        shared_cfg,
    )
}

fn show_status(runtime: &Runtime, json: bool) -> Result<()> {
    match send_request(&runtime.cfg.daemon_addr, &RpcRequest::Status) {
        Ok(response) => {
            let status = response
                .status
                .ok_or_else(|| anyhow!("Daemon returned no status payload"))?;
            print_json_or_text(&status, json, || format_status(&status))
        }
        Err(_) => print_json_or_text(
            &serde_json::json!({
                "running": false,
                "addr": runtime.cfg.daemon_addr,
            }),
            json,
            || format!("daemon not running on {}", runtime.cfg.daemon_addr),
        ),
    }
}

fn search_tracks(runtime: &Runtime, query: &str, limit: usize, json: bool) -> Result<()> {
    let limit = limit.max(1);
    let core = runtime.build_core();
    let local = resolve::local_search(&core, query, limit);
    let remote_limit = limit.saturating_sub(local.len()).max(1);
    let mut client = runtime.make_client()?;
    let youtube = client.search(query, remote_limit)?;
    let results = SearchResults { local, youtube };

    print_json_or_text(&results, json, || {
        let mut out = String::new();
        if !results.local.is_empty() {
            out.push_str("Local\n");
            for track in &results.local {
                out.push_str(&format_track_line(track));
                out.push('\n');
            }
        }
        if !results.youtube.is_empty() {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str("YouTube\n");
            for track in &results.youtube {
                out.push_str(&format_track_line(track));
                out.push('\n');
            }
        }
        if out.trim().is_empty() {
            "no results".to_string()
        } else {
            out.trim_end().to_string()
        }
    })
}

fn play_input(runtime: &Runtime, input: &str) -> Result<()> {
    let track = runtime.resolve_track(input)?;
    send_daemon(runtime, &RpcRequest::PlayTrack { track })?;
    println!("playing");
    Ok(())
}

fn like_command(runtime: &Runtime, input: Option<&str>) -> Result<()> {
    let track = match input {
        Some(input) => runtime.resolve_track(input)?,
        None => current_daemon_track(runtime)?,
    };
    let video_id = track
        .remote_video_id()
        .ok_or_else(|| anyhow!("Only YouTube tracks can be liked on YouTube Music"))?;

    let mut client = runtime.make_client()?;
    client.like_song(video_id)?;
    println!("liked '{}' on YouTube Music", track.title);
    Ok(())
}

fn queue_command(runtime: &Runtime, command: &QueueCommand, json: bool) -> Result<()> {
    match command {
        QueueCommand::Add { input } => {
            let track = runtime.resolve_track(input)?;
            send_daemon(runtime, &RpcRequest::EnqueueTrack { track })?;
            println!("queued");
            Ok(())
        }
        QueueCommand::Show => {
            let response = send_daemon(runtime, &RpcRequest::Status)?;
            let status = response
                .status
                .ok_or_else(|| anyhow!("Daemon returned no queue payload"))?;
            print_json_or_text(&status.queue, json, || {
                if status.queue.is_empty() {
                    return "queue is empty".to_string();
                }
                status
                    .queue
                    .iter()
                    .enumerate()
                    .map(|(index, track)| format!("{:>2}. {}", index + 1, format_track_line(track)))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
        }
        QueueCommand::Clear => simple_request(runtime, &RpcRequest::ClearQueue, "queue cleared"),
    }
}

fn playlist_command(runtime: &Runtime, command: &PlaylistCommand, json: bool) -> Result<()> {
    match command {
        PlaylistCommand::Create { name } => {
            let mut store = PlaylistStore::load(&runtime.paths)?;
            store.create(name)?;
            store.save(&runtime.paths)?;
            println!("created playlist '{}'", name);
            Ok(())
        }
        PlaylistCommand::List => {
            let store = PlaylistStore::load(&runtime.paths)?;
            let names = store.names();
            print_json_or_text(&names, json, || {
                if names.is_empty() {
                    "no playlists".to_string()
                } else {
                    names.join("\n")
                }
            })
        }
        PlaylistCommand::Show { name } => {
            let store = PlaylistStore::load(&runtime.paths)?;
            let playlist = store
                .playlist(name)
                .ok_or_else(|| anyhow!("Playlist '{}' does not exist", name))?;
            print_json_or_text(playlist, json, || {
                let mut out = format!("{}\n", playlist.name);
                if playlist.tracks.is_empty() {
                    out.push_str("empty");
                } else {
                    for (index, track) in playlist.tracks.iter().enumerate() {
                        out.push_str(&format!("{:>2}. {}\n", index + 1, format_track_line(track)));
                    }
                }
                out.trim_end().to_string()
            })
        }
        PlaylistCommand::Add { name, input } => {
            let mut store = PlaylistStore::load(&runtime.paths)?;
            let track = runtime.resolve_track(input)?;
            let count = store.add_track(name, track)?;
            store.save(&runtime.paths)?;
            println!("playlist '{}' now has {} track(s)", name, count);
            Ok(())
        }
        PlaylistCommand::Import { input, name } => {
            let remote = resolve_remote_collection(runtime, input)?;
            let playlist_name = name
                .clone()
                .unwrap_or_else(|| remote.title.clone())
                .trim()
                .to_string();
            let mut store = PlaylistStore::load(&runtime.paths)?;
            let count =
                store.import_remote(&playlist_name, remote.tracks, remote.source_url.clone())?;
            store.save(&runtime.paths)?;
            println!(
                "imported {} track(s) into '{}' from {}",
                count, playlist_name, remote.source_url
            );
            Ok(())
        }
        PlaylistCommand::Sync { name } => {
            let mut store = PlaylistStore::load(&runtime.paths)?;
            let remote_url = store
                .playlist(name)
                .and_then(|playlist| playlist.remote_url.clone())
                .ok_or_else(|| anyhow!("Playlist '{}' is not linked to a remote playlist", name))?;
            let remote = resolve_remote_collection(runtime, &remote_url)?;
            let count = store.sync_remote(name, remote.tracks, remote.source_url)?;
            store.save(&runtime.paths)?;
            println!("synced '{}' with {} track(s)", name, count);
            Ok(())
        }
        PlaylistCommand::Play { name } => {
            let tracks = playlist_tracks(runtime, name)?;
            send_daemon(runtime, &RpcRequest::PlayTracks { tracks })?;
            println!("playlist started");
            Ok(())
        }
        PlaylistCommand::Enqueue { name } => {
            let tracks = playlist_tracks(runtime, name)?;
            send_daemon(runtime, &RpcRequest::EnqueueTracks { tracks })?;
            println!("playlist queued");
            Ok(())
        }
        PlaylistCommand::Download {
            name,
            format,
            output,
        } => {
            let tracks = playlist_tracks(runtime, name)?;
            let out_dir = output
                .clone()
                .unwrap_or_else(|| runtime.cfg.effective_downloads_dir(&runtime.paths));
            let mut client = runtime.make_client()?;
            for track in &tracks {
                let path = download_track(track, &mut client, (*format).into(), &out_dir)?;
                println!("{}", path.display());
            }
            Ok(())
        }
    }
}

fn lyrics_command(runtime: &Runtime, input: Option<&str>, cached: bool, json: bool) -> Result<()> {
    if input.is_none() {
        if let Ok(response) = send_request(&runtime.cfg.daemon_addr, &RpcRequest::Lyrics { cached })
        {
            if let Some(doc) = response.lyrics {
                return print_json_or_text(&doc, json, || render_lyrics(&doc));
            }
        }
    }

    let lyrics = LyricsClient::new(runtime.paths.clone());
    let track = match input {
        Some(input) => runtime.resolve_track(input)?,
        None => current_daemon_track(runtime)?,
    };
    let doc = if cached {
        lyrics.cached_track(&track)?
    } else {
        lyrics.lookup_track(&track)?
    }
    .ok_or_else(|| anyhow!("No lyrics found for '{}'", track.title))?;

    print_json_or_text(&doc, json, || render_lyrics(&doc))
}

fn download_input(
    runtime: &Runtime,
    input: &str,
    format: DownloadFormat,
    output: Option<&Path>,
) -> Result<()> {
    let out_dir = output
        .map(Path::to_path_buf)
        .unwrap_or_else(|| runtime.cfg.effective_downloads_dir(&runtime.paths));

    if let Ok(collection) = resolve_remote_collection(runtime, input) {
        let mut client = runtime.make_client()?;
        for track in &collection.tracks {
            let path = download_track(track, &mut client, format, &out_dir)?;
            println!("{}", path.display());
        }
        return Ok(());
    }

    let track = runtime.resolve_track(input)?;
    let mut client = runtime.make_client()?;
    let path = download_track(&track, &mut client, format, &out_dir)?;
    println!("{}", path.display());
    Ok(())
}

fn library_command(runtime: &mut Runtime, command: &LibraryCommand, json: bool) -> Result<bool> {
    match command {
        LibraryCommand::AddPath { path } => {
            let path = normalize_path(path)?;
            let count =
                crate::library_cache::scan_paths_cached(&runtime.paths, &[path.clone()])?.len();
            if runtime
                .cfg
                .scan_paths
                .iter()
                .any(|item| same_path(item, &path))
            {
                println!("path already added");
                return Ok(false);
            }
            runtime.cfg.scan_paths.push(path.clone());
            runtime.persist()?;
            println!("added {} ({} track(s))", path.display(), count);
            Ok(true)
        }
        LibraryCommand::RemovePath { index } => {
            if *index == 0 || *index > runtime.cfg.scan_paths.len() {
                return Err(anyhow!(
                    "Library path index must be between 1 and {}",
                    runtime.cfg.scan_paths.len()
                ));
            }
            let removed = runtime.cfg.scan_paths.remove(*index - 1);
            runtime.persist()?;
            println!("removed {}", removed.display());
            Ok(true)
        }
        LibraryCommand::ListPaths => {
            let items = runtime
                .cfg
                .scan_paths
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>();
            print_json_or_text(&items, json, || {
                if items.is_empty() {
                    "no library paths configured".to_string()
                } else {
                    items
                        .iter()
                        .enumerate()
                        .map(|(index, item)| format!("{:>2}. {}", index + 1, item))
                        .collect::<Vec<_>>()
                        .join("\n")
                }
            })?;
            Ok(false)
        }
    }
}

fn auth_command(runtime: &mut Runtime, command: &AuthCommand, json: bool) -> Result<bool> {
    match command {
        AuthCommand::Login => {
            println!("opening the YouTube Music login window...");
            let session = youtube_login_window(&runtime.paths)?;

            let mut client = SoundCloudClient::new(Ql::parse(&runtime.cfg.quality));
            client.set_cookie_header(Some(session.cookie_header.clone()));
            client.set_auth_user(session.auth_user.clone());
            let state = client.login()?;

            runtime.cfg.youtube_cookie_header = Some(session.cookie_header);
            runtime.cfg.youtube_cookie_file = None;
            runtime.cfg.youtube_auth_user = session.auth_user;
            runtime.persist()?;

            println!(
                "signed in as {}",
                state.name.unwrap_or_else(|| "unknown".to_string())
            );
            Ok(true)
        }
        AuthCommand::CookieFile { path } => {
            runtime.cfg.youtube_cookie_file = Some(normalize_path(path)?);
            runtime.cfg.youtube_cookie_header = None;
            runtime.cfg.youtube_auth_user = None;
            runtime.persist()?;
            println!("cookie file saved");
            Ok(true)
        }
        AuthCommand::CookieHeader { header } => {
            runtime.cfg.youtube_cookie_header = Some(header.trim().to_string());
            runtime.cfg.youtube_cookie_file = None;
            runtime.cfg.youtube_auth_user = None;
            runtime.persist()?;
            println!("cookie header saved");
            Ok(true)
        }
        AuthCommand::HeadersFile { path } => {
            let path = normalize_path(path)?;
            let (cookie, auth_user) = parse_headers_json(&path)?;
            runtime.cfg.youtube_cookie_header = Some(cookie);
            runtime.cfg.youtube_cookie_file = None;
            runtime.cfg.youtube_auth_user = auth_user;
            runtime.persist()?;
            println!("headers file imported");
            Ok(true)
        }
        AuthCommand::Clear => {
            runtime.cfg.youtube_cookie_header = None;
            runtime.cfg.youtube_cookie_file = None;
            runtime.cfg.youtube_auth_user = None;
            runtime.persist()?;
            println!("youtube auth cleared");
            Ok(true)
        }
        AuthCommand::Show => {
            let configured_cookie_header = runtime
                .cfg
                .youtube_cookie_header
                .as_ref()
                .is_some_and(|value| !value.trim().is_empty());
            let effective_cookie_header = runtime.cfg.cookie_header()?;
            let browser_cookie_fallback = runtime.cfg.youtube_cookie_file.is_none()
                && !configured_cookie_header
                && effective_cookie_header.is_some();
            let effective_auth_user = effective_cookie_header.as_ref().map(|_| {
                runtime
                    .cfg
                    .youtube_auth_user
                    .as_deref()
                    .unwrap_or("0")
                    .to_string()
            });
            let state = runtime.make_client()?.state();
            let payload = serde_json::json!({
                "cookie_file": runtime.cfg.youtube_cookie_file,
                "cookie_header_configured": configured_cookie_header,
                "browser_cookie_fallback": browser_cookie_fallback,
                "auth_user": runtime.cfg.youtube_auth_user,
                "effective_auth_user": effective_auth_user,
                "ready": state.ready,
                "logged_in": state.user,
                "account_name": state.name,
                "message": state.msg,
            });
            print_json_or_text(&payload, json, || {
                let source = if let Some(path) = runtime.cfg.youtube_cookie_file.as_ref() {
                    format!("cookie file: {} (account not verified)", path.display())
                } else if configured_cookie_header {
                    let auth_user = runtime.cfg.youtube_auth_user.as_deref().unwrap_or("0");
                    format!("cookie header: configured (auth user {auth_user})")
                } else if browser_cookie_fallback {
                    let auth_user = runtime.cfg.youtube_auth_user.as_deref().unwrap_or("0");
                    format!(
                        "browser cookies: detected (auth user {auth_user}, browser/profile auto-picked)"
                    )
                } else {
                    "youtube auth: not configured".to_string()
                };

                if state.user {
                    return format!(
                        "signed in as {}\n{}",
                        state.name.unwrap_or_else(|| "unknown".to_string()),
                        source
                    );
                }

                match state.msg {
                    Some(msg) if !msg.trim().is_empty() => format!("{source}\n{msg}"),
                    _ => source,
                }
            })?;
            Ok(false)
        }
    }
}

fn parse_headers_json(path: &Path) -> Result<(String, Option<String>)> {
    let txt = fs::read_to_string(path)
        .with_context(|| format!("Could not read headers file {}", path.display()))?;
    let json: Value = serde_json::from_str(&txt)
        .with_context(|| format!("Could not parse headers file {}", path.display()))?;
    let obj = json
        .as_object()
        .or_else(|| json.get("headers").and_then(Value::as_object))
        .ok_or_else(|| anyhow!("Headers file must be a JSON object"))?;

    let mut cookie: Option<String> = None;
    let mut auth_user: Option<String> = None;
    for (key, value) in obj {
        let key = key.trim().to_ascii_lowercase();
        if key == "cookie" {
            cookie = value
                .as_str()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty());
        } else if key == "x-goog-authuser" {
            auth_user = match value {
                Value::String(v) => Some(v.trim().to_string()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            }
            .filter(|v| !v.is_empty());
        }
    }

    let cookie = cookie.ok_or_else(|| anyhow!("Headers file is missing a Cookie header"))?;
    Ok((cookie, auth_user))
}

fn configure_startup(enabled: bool) -> Result<String> {
    if enabled {
        install_startup()?;
        Ok("startup enabled".to_string())
    } else {
        uninstall_startup()?;
        Ok("startup disabled".to_string())
    }
}

#[cfg(target_os = "windows")]
fn install_startup() -> Result<()> {
    let exe = std::env::current_exe().context("Could not resolve current executable")?;
    let appdata = std::env::var_os("APPDATA").ok_or_else(|| anyhow!("APPDATA is not set"))?;
    let path = PathBuf::from(appdata)
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join("Startup")
        .join("rustplayer-daemon.cmd");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let script = format!(
        "@echo off\r\nstart \"RustPlayer\" /min \"{}\" daemon\r\n",
        exe.display()
    );
    fs::write(path, script)?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn uninstall_startup() -> Result<()> {
    let appdata = std::env::var_os("APPDATA").ok_or_else(|| anyhow!("APPDATA is not set"))?;
    let path = PathBuf::from(appdata)
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join("Startup")
        .join("rustplayer-daemon.cmd");
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn install_startup() -> Result<()> {
    let exe = std::env::current_exe().context("Could not resolve current executable")?;
    let config_dir = dirs::config_dir().ok_or_else(|| anyhow!("Could not locate config dir"))?;
    let unit_dir = config_dir.join("systemd").join("user");
    fs::create_dir_all(&unit_dir)?;
    let unit_path = unit_dir.join("rustplayer.service");
    let unit = format!(
        "[Unit]\nDescription=RustPlayer background daemon\n\n[Service]\nType=simple\nExecStart={} daemon\nRestart=on-failure\nRestartSec=3\n\n[Install]\nWantedBy=default.target\n",
        shell_escape_arg(&exe.display().to_string())
    );
    fs::write(&unit_path, unit)?;
    let _ = ProcessCommand::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status();
    let status = ProcessCommand::new("systemctl")
        .args(["--user", "enable", "--now", "rustplayer.service"])
        .status();
    if status.as_ref().is_err() || !status.unwrap().success() {
        return Err(anyhow!(
            "Wrote {} but could not enable it with systemctl --user",
            unit_path.display()
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn uninstall_startup() -> Result<()> {
    let _ = ProcessCommand::new("systemctl")
        .args(["--user", "disable", "--now", "rustplayer.service"])
        .status();
    if let Some(config_dir) = dirs::config_dir() {
        let unit_path = config_dir
            .join("systemd")
            .join("user")
            .join("rustplayer.service");
        if unit_path.exists() {
            fs::remove_file(unit_path)?;
        }
    }
    let _ = ProcessCommand::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status();
    Ok(())
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn install_startup() -> Result<()> {
    Err(anyhow!(
        "startup integration is not implemented on this platform yet"
    ))
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn uninstall_startup() -> Result<()> {
    Err(anyhow!(
        "startup integration is not implemented on this platform yet"
    ))
}

#[cfg(target_os = "linux")]
fn shell_escape_arg(value: &str) -> String {
    let escaped = value.replace('\'', "'\\''");
    format!("'{}'", escaped)
}

fn maybe_restart_daemon(runtime: &Runtime) -> Result<()> {
    if send_request(&runtime.cfg.daemon_addr, &RpcRequest::Ping).is_ok() {
        let _ = send_request(&runtime.cfg.daemon_addr, &RpcRequest::Shutdown);
    }
    Ok(())
}

fn shutdown_daemon(runtime: &Runtime) -> Result<()> {
    match send_request(&runtime.cfg.daemon_addr, &RpcRequest::Shutdown) {
        Ok(_) => {
            println!("daemon stopped");
            Ok(())
        }
        Err(err) => Err(anyhow!("Could not stop daemon: {err}")),
    }
}

fn current_daemon_track(runtime: &Runtime) -> Result<Track> {
    let response = send_request(&runtime.cfg.daemon_addr, &RpcRequest::Status)
        .context("No active daemon track is available")?;
    response
        .status
        .and_then(|status| status.current)
        .ok_or_else(|| anyhow!("No track is currently playing"))
}

fn playlist_tracks(runtime: &Runtime, name: &str) -> Result<Vec<Track>> {
    let store = PlaylistStore::load(&runtime.paths)?;
    let playlist = store
        .playlist(name)
        .ok_or_else(|| anyhow!("Playlist '{}' does not exist", name))?;
    if playlist.tracks.is_empty() {
        return Err(anyhow!("Playlist '{}' is empty", name));
    }
    Ok(playlist.tracks.clone())
}

fn send_daemon(runtime: &Runtime, request: &RpcRequest) -> Result<crate::daemon::RpcResponse> {
    daemon::ensure_daemon(&runtime.paths)?;
    send_request(&runtime.cfg.daemon_addr, request)
}

fn simple_request(runtime: &Runtime, request: &RpcRequest, message: &str) -> Result<()> {
    let _ = send_daemon(runtime, request)?;
    println!("{message}");
    Ok(())
}

fn print_json_or_text<T>(value: &T, json: bool, render_text: impl FnOnce() -> String) -> Result<()>
where
    T: Serialize,
{
    if json {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else {
        println!("{}", render_text());
    }
    Ok(())
}

fn format_status(status: &crate::daemon::PlayerStatus) -> String {
    let mut lines = Vec::new();
    if let Some(track) = status.current.as_ref() {
        lines.push(format!("current: {}", format_track_line(track)));
    } else {
        lines.push("current: idle".to_string());
    }
    lines.push(format!(
        "state: {}",
        if status.is_playing {
            "playing"
        } else {
            "paused/stopped"
        }
    ));
    let duration = if status.duration_secs == 0 {
        "--:--".to_string()
    } else {
        format!(
            "{:02}:{:02}",
            status.duration_secs / 60,
            status.duration_secs % 60
        )
    };

    lines.push(format!(
        "position: {:02}:{:02} / {}",
        status.position_secs / 60,
        status.position_secs % 60,
        duration
    ));
    lines.push(format!("queue: {}", status.queue.len()));
    lines.push(format!("autoplay: {}", status.autoplay));
    if let Some(remaining) = status.sleep_remaining_secs {
        lines.push(format!("sleep: {}s", remaining));
    }
    lines.join("\n")
}

fn format_track_line(track: &Track) -> String {
    let dur = track
        .dur
        .map(|value| format!("{:02}:{:02}", value / 60, value % 60))
        .unwrap_or_else(|| "--:--".to_string());
    format!(
        "[{}] {} - {} ({dur})",
        track.tag(),
        track.title,
        track.who()
    )
}

fn render_lyrics(doc: &crate::lyrics::LyricsDoc) -> String {
    if doc.instrumental {
        return "instrumental".to_string();
    }

    if let Some(synced) = doc.synced.as_deref() {
        return synced.to_string();
    }

    if let Some(plain) = doc.plain.as_deref() {
        return plain.to_string();
    }

    "lyrics unavailable".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_browse_id_accepts_playlist_urls() {
        assert_eq!(
            remote_browse_id("https://music.youtube.com/playlist?list=PL123"),
            Some("VLPL123".to_string())
        );
    }

    #[test]
    fn remote_browse_id_accepts_browse_urls() {
        assert_eq!(
            remote_browse_id("https://music.youtube.com/browse/MPREb_abc"),
            Some("MPREb_abc".to_string())
        );
    }
}

fn normalize_path(path: &Path) -> Result<PathBuf> {
    path.canonicalize()
        .with_context(|| format!("Could not resolve {}", path.display()))
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}
