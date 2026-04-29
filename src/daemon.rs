use crate::appdata::{AppConfig, AppPaths};
use crate::core::track::Track;
use crate::core::Core;
use crate::lyrics::{LyricsClient, LyricsDoc};
use crate::playback::{self, PlaybackCommand};
use crate::sources::soundcloud::{Ql, SoundCloudClient};
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RpcRequest {
    Ping,
    Status,
    PlayTrack { track: Track },
    PlayTracks { tracks: Vec<Track> },
    EnqueueTrack { track: Track },
    EnqueueTracks { tracks: Vec<Track> },
    Pause,
    Resume,
    Stop,
    Next,
    Prev,
    ClearQueue,
    SetAutoplay { enabled: bool },
    SetSleep { minutes: Option<u64> },
    Shutdown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RpcResponse {
    pub ok: bool,
    pub message: String,
    pub status: Option<PlayerStatus>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlayerStatus {
    pub current: Option<Track>,
    pub queue: Vec<Track>,
    pub autoplay: bool,
    pub position_secs: u64,
    pub duration_secs: u64,
    pub is_playing: bool,
    pub sleep_remaining_secs: Option<u64>,
    pub lyrics_cached: bool,
}

struct SharedState {
    core: Core,
    pb_tx: Sender<PlaybackCommand>,
    position: Arc<Mutex<Option<(u64, u64, bool)>>>,
    cfg: Arc<Mutex<AppConfig>>,
    lyrics: LyricsClient,
    current_lyrics: Arc<Mutex<Option<LyricsDoc>>>,
    sleep_deadline: Arc<Mutex<Option<Instant>>>,
    shutdown: Arc<AtomicBool>,
}

pub fn run_daemon(paths: AppPaths, config: AppConfig) -> Result<()> {
    let cfg = Arc::new(Mutex::new(config));
    let sc = Arc::new(Mutex::new(make_client(&cfg.lock().unwrap())?));
    let core = Core::new();
    core.set_sc(true);

    for path in cfg.lock().unwrap().scan_paths.clone() {
        if let Some(path_str) = path.to_str() {
            let _ = core.add_scan_path(path_str);
        }
    }

    let pb = playback::start_audio_thread(core.clone(), sc.clone(), cfg.lock().unwrap().autoplay);
    let state = SharedState {
        core,
        pb_tx: pb.tx.clone(),
        position: pb.position_rx.clone(),
        cfg: cfg.clone(),
        lyrics: LyricsClient::new(paths.clone()),
        current_lyrics: Arc::new(Mutex::new(None)),
        sleep_deadline: Arc::new(Mutex::new(None)),
        shutdown: Arc::new(AtomicBool::new(false)),
    };
    let state = Arc::new(state);

    start_runtime_monitor(state.clone());

    let addr = state.cfg.lock().unwrap().daemon_addr.clone();
    let listener =
        TcpListener::bind(&addr).with_context(|| format!("Could not bind daemon on {}", addr))?;
    listener
        .set_nonblocking(true)
        .context("Could not switch daemon socket to non-blocking mode")?;

    while !state.shutdown.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                if !handle_client(stream, &state)? {
                    break;
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(err) => return Err(err.into()),
        }
    }

    Ok(())
}

pub fn send_request(addr: &str, request: &RpcRequest) -> Result<RpcResponse> {
    let mut stream =
        TcpStream::connect(addr).with_context(|| format!("Could not connect to {}", addr))?;
    stream
        .set_nodelay(true)
        .context("Could not enable TCP_NODELAY")?;
    stream.write_all(serde_json::to_string(request)?.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    let response = serde_json::from_str::<RpcResponse>(&line)?;
    if response.ok {
        Ok(response)
    } else {
        Err(anyhow!(response.message))
    }
}

pub fn ensure_daemon(paths: &AppPaths) -> Result<()> {
    let cfg = AppConfig::load(paths)?;
    if send_request(&cfg.daemon_addr, &RpcRequest::Ping).is_ok() {
        return Ok(());
    }

    let exe = std::env::current_exe().context("Could not resolve current executable")?;
    let mut cmd = Command::new(exe);
    cmd.arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    cmd.spawn().context("Could not spawn daemon")?;

    for _ in 0..30 {
        thread::sleep(Duration::from_millis(150));
        if send_request(&cfg.daemon_addr, &RpcRequest::Ping).is_ok() {
            return Ok(());
        }
    }

    Err(anyhow!("Daemon did not become ready in time"))
}

fn handle_client(mut stream: TcpStream, state: &Arc<SharedState>) -> Result<bool> {
    let mut line = String::new();
    BufReader::new(stream.try_clone()?).read_line(&mut line)?;
    let request = serde_json::from_str::<RpcRequest>(&line)?;

    let keep_running = !matches!(request, RpcRequest::Shutdown);
    let response = match apply_request(request, state) {
        Ok(message) => RpcResponse {
            ok: true,
            message,
            status: Some(status_snapshot(state)),
        },
        Err(err) => RpcResponse {
            ok: false,
            message: err.to_string(),
            status: Some(status_snapshot(state)),
        },
    };

    stream.write_all(serde_json::to_string(&response)?.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    Ok(keep_running)
}

fn apply_request(request: RpcRequest, state: &Arc<SharedState>) -> Result<String> {
    match request {
        RpcRequest::Ping => Ok("pong".to_string()),
        RpcRequest::Status => Ok("status".to_string()),
        RpcRequest::PlayTrack { track } => {
            put_track(state, &track);
            state
                .pb_tx
                .send(PlaybackCommand::PlayTrack(track.id.clone()))?;
            Ok(format!("playing {}", track.title))
        }
        RpcRequest::PlayTracks { tracks } => {
            if tracks.is_empty() {
                return Err(anyhow!("Track list is empty"));
            }
            for track in &tracks {
                put_track(state, track);
            }
            state.core.clear_queue();
            for track in tracks.iter().skip(1) {
                state.core.enqueue(track.id.clone());
            }
            state
                .pb_tx
                .send(PlaybackCommand::PlayTrack(tracks[0].id.clone()))?;
            Ok(format!(
                "playing {} track(s) starting with {}",
                tracks.len(),
                tracks[0].title
            ))
        }
        RpcRequest::EnqueueTrack { track } => {
            put_track(state, &track);
            state.core.enqueue(track.id.clone());
            Ok(format!("queued {}", track.title))
        }
        RpcRequest::EnqueueTracks { tracks } => {
            if tracks.is_empty() {
                return Err(anyhow!("Track list is empty"));
            }
            for track in &tracks {
                put_track(state, track);
                state.core.enqueue(track.id.clone());
            }
            Ok(format!("queued {} track(s)", tracks.len()))
        }
        RpcRequest::Pause => {
            state.pb_tx.send(PlaybackCommand::Pause)?;
            Ok("paused".to_string())
        }
        RpcRequest::Resume => {
            state.pb_tx.send(PlaybackCommand::Resume)?;
            Ok("resumed".to_string())
        }
        RpcRequest::Stop => {
            state.pb_tx.send(PlaybackCommand::Stop)?;
            Ok("stopped".to_string())
        }
        RpcRequest::Next => {
            state.pb_tx.send(PlaybackCommand::Next)?;
            Ok("next".to_string())
        }
        RpcRequest::Prev => {
            state.pb_tx.send(PlaybackCommand::Prev)?;
            Ok("previous".to_string())
        }
        RpcRequest::ClearQueue => {
            state.core.clear_queue();
            Ok("queue cleared".to_string())
        }
        RpcRequest::SetAutoplay { enabled } => {
            let mut cfg = state.cfg.lock().unwrap();
            cfg.autoplay = enabled;
            cfg.save(&AppPaths::discover())?;
            drop(cfg);
            state.pb_tx.send(PlaybackCommand::SetAutoplay(enabled))?;
            Ok(format!(
                "autoplay {}",
                if enabled { "enabled" } else { "disabled" }
            ))
        }
        RpcRequest::SetSleep { minutes } => {
            let mut deadline = state.sleep_deadline.lock().unwrap();
            *deadline = minutes.map(|value| Instant::now() + Duration::from_secs(value * 60));
            Ok(match minutes {
                Some(value) => format!("sleep timer set for {} minute(s)", value),
                None => "sleep timer cleared".to_string(),
            })
        }
        RpcRequest::Shutdown => {
            state.shutdown.store(true, Ordering::Relaxed);
            state.pb_tx.send(PlaybackCommand::Quit)?;
            Ok("daemon shutting down".to_string())
        }
    }
}

fn put_track(state: &Arc<SharedState>, track: &Track) {
    state.core.put_tracks(vec![track.clone()]);
}

fn status_snapshot(state: &Arc<SharedState>) -> PlayerStatus {
    let (position_secs, duration_secs, is_playing) =
        (*state.position.lock().unwrap()).unwrap_or((0, 0, false));
    let current = state.core.cur_id().and_then(|id| state.core.track(&id));
    let queue = state.core.tracks_of(&state.core.q_ids());
    let autoplay = state.cfg.lock().unwrap().autoplay;
    let sleep_remaining_secs = state
        .sleep_deadline
        .lock()
        .unwrap()
        .map(|deadline| deadline.saturating_duration_since(Instant::now()).as_secs());
    let lyrics_cached = state.current_lyrics.lock().unwrap().is_some();

    PlayerStatus {
        current,
        queue,
        autoplay,
        position_secs,
        duration_secs,
        is_playing,
        sleep_remaining_secs,
        lyrics_cached,
    }
}

fn start_runtime_monitor(state: Arc<SharedState>) {
    thread::spawn(move || {
        let mut last_track_id = None::<String>;

        loop {
            if state.shutdown.load(Ordering::Relaxed) {
                break;
            }

            if let Some(deadline) = *state.sleep_deadline.lock().unwrap() {
                if Instant::now() >= deadline {
                    let _ = state.pb_tx.send(PlaybackCommand::Stop);
                    *state.sleep_deadline.lock().unwrap() = None;
                }
            }

            let current = state.core.cur_id().and_then(|id| state.core.track(&id));

            if current.as_ref().map(|track| track.id.as_str()) != last_track_id.as_deref() {
                last_track_id = current.as_ref().map(|track| track.id.clone());
                if state.cfg.lock().unwrap().lyrics_enabled
                    && state.cfg.lock().unwrap().auto_fetch_lyrics
                {
                    let lyrics = current
                        .as_ref()
                        .and_then(|track| state.lyrics.lookup_track(track).ok().flatten());
                    *state.current_lyrics.lock().unwrap() = lyrics;
                } else {
                    *state.current_lyrics.lock().unwrap() = None;
                }
            }

            thread::sleep(Duration::from_millis(250));
        }
    });
}

fn make_client(cfg: &AppConfig) -> Result<SoundCloudClient> {
    let mut client = SoundCloudClient::new(Ql::parse(&cfg.quality));
    client.set_cookie_header(cfg.cookie_header()?);
    Ok(client)
}
