mod auth;
mod appdata;
mod core;
mod daemon;
mod downloads;
mod lyrics;
mod playback;
mod playlist;
mod resolve;
mod sources;
mod ui;

use crate::appdata::{AppConfig, AppPaths};
use crate::auth::youtube_login_window;
use crate::core::track::Track;
use crate::core::Core;
use crate::daemon::{send_request, RpcRequest};
use crate::downloads::{download_track, DownloadFormat};
use crate::lyrics::LyricsClient;
use crate::playlist::PlaylistStore;
use crate::sources::soundcloud::{Ql, SoundCloudClient};
use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Parser, Debug)]
#[command(
    name = "rustplayer",
    version,
    about = "A high-performance terminal music player with YouTube streaming",
    after_help = "Auth note:\n  On Windows, RustPlayer can open a dedicated YouTube Music login window with\n  'rustplayer auth login'.\n  Manual cookie login is still supported through\n  'rustplayer auth cookie-file <cookies.txt>' or\n  'rustplayer auth cookie-header \"SID=...; SAPISID=...\"'.\n  You can also import ytmusicapi headers.json via\n  'rustplayer auth headers-file <headers.json>'.\n\nExamples:\n  rustplayer auth login\n  rustplayer auth show\n  rustplayer auth headers-file headers.json\n  rustplayer status\n  rustplayer play \"never gonna give you up\"\n  rustplayer download \"https://music.youtube.com/watch?v=lYBUbBu4W08\" --format mp3\n  rustplayer playlist create mix"
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
    Status,
    Search {
        query: String,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    Play {
        input: String,
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

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut runtime = Runtime::load(cli.paths.clone(), cli.quality.clone())?;

    match &cli.command {
        None | Some(Command::Tui) => run_tui(&runtime),
        Some(Command::Daemon) => daemon::run_daemon(runtime.paths.clone(), runtime.cfg.clone()),
        Some(Command::Status) => show_status(&runtime, cli.json),
        Some(Command::Search { query, limit }) => search_tracks(&runtime, query, *limit, cli.json),
        Some(Command::Play { input }) => play_input(&runtime, input),
        Some(Command::Pause) => simple_request(&runtime, &RpcRequest::Pause, "paused"),
        Some(Command::Resume) => simple_request(&runtime, &RpcRequest::Resume, "resumed"),
        Some(Command::Stop) => simple_request(&runtime, &RpcRequest::Stop, "stopped"),
        Some(Command::Next) => simple_request(&runtime, &RpcRequest::Next, "skipped"),
        Some(Command::Prev) => simple_request(&runtime, &RpcRequest::Prev, "returned"),
        Some(Command::Queue { command }) => queue_command(&runtime, command, cli.json),
        Some(Command::Playlist { command }) => playlist_command(&runtime, command, cli.json),
        Some(Command::Lyrics { input, cached }) => {
            lyrics_command(&runtime, input.as_deref(), *cached)
        }
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
            println!(
                "startup preference {}",
                if state.is_on() { "enabled" } else { "disabled" }
            );
            Ok(())
        }
        Some(Command::Config) => print_json_or_text(&runtime.cfg, cli.json, || {
            format!(
                "quality: {}\nscan paths: {}\nautoplay: {}\nlyrics: {}\nbackground on boot: {}",
                runtime.cfg.quality,
                runtime.cfg.scan_paths.len(),
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
        for path in &self.cfg.scan_paths {
            if let Some(path_str) = path.to_str() {
                let _ = core.add_scan_path(path_str);
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

fn run_tui(runtime: &Runtime) -> Result<()> {
    let core = runtime.build_core();
    let ui_client = Arc::new(Mutex::new(runtime.make_client()?));
    let playback_client = Arc::new(Mutex::new(runtime.make_client()?));
    let shared_cfg = Arc::new(Mutex::new(runtime.cfg.clone()));
    let playback =
        playback::start_audio_thread(core.clone(), playback_client.clone(), runtime.cfg.autoplay);
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

fn lyrics_command(runtime: &Runtime, input: Option<&str>, cached: bool) -> Result<()> {
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

    if doc.instrumental {
        println!("instrumental");
        return Ok(());
    }

    if let Some(synced) = doc.synced.as_deref() {
        println!("{synced}");
        return Ok(());
    }

    if let Some(plain) = doc.plain.as_deref() {
        println!("{plain}");
        return Ok(());
    }

    Err(anyhow!("Lyrics provider returned an empty document"))
}

fn download_input(
    runtime: &Runtime,
    input: &str,
    format: DownloadFormat,
    output: Option<&Path>,
) -> Result<()> {
    let track = runtime.resolve_track(input)?;
    let mut client = runtime.make_client()?;
    let out_dir = output
        .map(Path::to_path_buf)
        .unwrap_or_else(|| runtime.cfg.effective_downloads_dir(&runtime.paths));
    let path = download_track(&track, &mut client, format, &out_dir)?;
    println!("{}", path.display());
    Ok(())
}

fn library_command(runtime: &mut Runtime, command: &LibraryCommand, json: bool) -> Result<bool> {
    match command {
        LibraryCommand::AddPath { path } => {
            let path = normalize_path(path)?;
            let count = crate::sources::local::scan_dir(&path)?.len();
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
            let effective_auth_user = effective_cookie_header
                .as_ref()
                .map(|_| runtime.cfg.youtube_auth_user.as_deref().unwrap_or("0").to_string());
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
                    let auth_user = runtime
                        .cfg
                        .youtube_auth_user
                        .as_deref()
                        .unwrap_or("0");
                    format!("cookie header: configured (auth user {auth_user})")
                } else if browser_cookie_fallback {
                    let auth_user = runtime
                        .cfg
                        .youtube_auth_user
                        .as_deref()
                        .unwrap_or("0");
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
    lines.push(format!(
        "position: {:02}:{:02} / {:02}:{:02}",
        status.position_secs / 60,
        status.position_secs % 60,
        status.duration_secs / 60,
        status.duration_secs % 60
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
