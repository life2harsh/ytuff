use crate::core::track::Track;
use crate::core::Core;
use crate::daemon::{send_request, RpcRequest};
use crate::playback::{PlaybackCommand, PlaybackHandle, RepeatMode};
use anyhow::{anyhow, Result};
use std::cell::Cell;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const STATUS_POLL_INTERVAL: Duration = Duration::from_millis(250);

pub fn start_daemon_playback_proxy(core: Core, daemon_addr: String) -> PlaybackHandle {
    let (tx, rx): (Sender<PlaybackCommand>, Receiver<PlaybackCommand>) = mpsc::channel();
    let position = Arc::new(Mutex::new(None));

    let (autoplay_tx, autoplay_rx): (Sender<bool>, Receiver<bool>) = mpsc::channel();
    let (repeat_tx, repeat_rx): (Sender<RepeatMode>, Receiver<RepeatMode>) = mpsc::channel();
    let (shuffle_tx, shuffle_rx): (Sender<bool>, Receiver<bool>) = mpsc::channel();
    let (volume_tx, volume_rx): (Sender<f32>, Receiver<f32>) = mpsc::channel();
    let (visualizer_tx, visualizer_rx): (Sender<Vec<f32>>, Receiver<Vec<f32>>) = mpsc::channel();
    let (devices_tx, devices_rx): (Sender<Vec<(String, bool)>>, Receiver<Vec<(String, bool)>>) =
        mpsc::channel();
    let (msg_tx, msg_rx): (Sender<String>, Receiver<String>) = mpsc::channel();

    let daemon_addr_bg = daemon_addr.clone();
    let core_bg = core.clone();
    let position_bg = Arc::clone(&position);

    thread::spawn(move || {
        let last_autoplay = Cell::new(None::<bool>);
        let mut last_repeat: Option<RepeatMode> = None;
        let mut last_shuffle: Option<bool> = None;
        let mut last_volume: Option<f32> = None;
        let mut last_devices: Option<Vec<(String, bool)>> = None;

        let mut next_poll = Instant::now();

        let mut apply_status = |status: &crate::daemon::PlayerStatus| {
            sync_core_from_status(&core_bg, status);
            *position_bg.lock().unwrap() = Some((
                status.position_secs,
                status.duration_secs,
                status.is_playing,
            ));

            if last_autoplay.get() != Some(status.autoplay) {
                last_autoplay.set(Some(status.autoplay));
                let _ = autoplay_tx.send(status.autoplay);
            }

            if last_repeat != Some(status.repeat_mode) {
                last_repeat = Some(status.repeat_mode);
                let _ = repeat_tx.send(status.repeat_mode);
            }

            if last_shuffle != Some(status.shuffle_on) {
                last_shuffle = Some(status.shuffle_on);
                let _ = shuffle_tx.send(status.shuffle_on);
            }

            if last_volume != Some(status.volume) {
                last_volume = Some(status.volume);
                let _ = volume_tx.send(status.volume);
            }

            if last_devices.as_ref() != Some(&status.devices) {
                last_devices = Some(status.devices.clone());
                let _ = devices_tx.send(status.devices.clone());
            }
        };

        loop {
            let now = Instant::now();
            let timeout = if now >= next_poll {
                Duration::from_millis(0)
            } else {
                next_poll
                    .saturating_duration_since(now)
                    .min(Duration::from_millis(50))
            };

            match rx.recv_timeout(timeout) {
                Ok(cmd) => {
                    let autoplay = last_autoplay.get();
                    if let Err(err) = handle_command(
                        &core_bg,
                        &daemon_addr_bg,
                        cmd,
                        autoplay,
                        &msg_tx,
                        &mut apply_status,
                    ) {
                        let _ = msg_tx.send(err.to_string());
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }

            if Instant::now() >= next_poll {
                match send_request(&daemon_addr_bg, &RpcRequest::Status) {
                    Ok(resp) => {
                        if let Some(status) = resp.status.as_ref() {
                            apply_status(status);
                        }
                    }
                    Err(_) => {
                        // Daemon may be restarting; ignore transient failures.
                    }
                }
                next_poll = Instant::now() + STATUS_POLL_INTERVAL;
            }
        }

        // Avoid unused-channel warnings if compiler ever gets smarter.
        drop(visualizer_tx);
    });

    PlaybackHandle {
        tx,
        position_rx: position,
        devices_rx,
        autoplay_rx,
        repeat_rx,
        shuffle_rx,
        volume_rx,
        visualizer_rx,
        msg_rx,
    }
}

fn handle_command(
    core: &Core,
    daemon_addr: &str,
    cmd: PlaybackCommand,
    last_autoplay: Option<bool>,
    msg_tx: &Sender<String>,
    apply_status: &mut impl FnMut(&crate::daemon::PlayerStatus),
) -> Result<()> {
    let request = match cmd {
        PlaybackCommand::Pause => Some(RpcRequest::Pause),
        PlaybackCommand::Resume => Some(RpcRequest::Resume),
        PlaybackCommand::Stop => Some(RpcRequest::Stop),
        PlaybackCommand::Next => Some(RpcRequest::Next),
        PlaybackCommand::Prev => Some(RpcRequest::Prev),
        PlaybackCommand::VolumeUp => Some(RpcRequest::VolumeUp),
        PlaybackCommand::VolumeDown => Some(RpcRequest::VolumeDown),
        PlaybackCommand::ToggleMute => Some(RpcRequest::ToggleMute),
        PlaybackCommand::ToggleRepeat => Some(RpcRequest::ToggleRepeat),
        PlaybackCommand::ToggleShuffle => Some(RpcRequest::ToggleShuffle),
        PlaybackCommand::ToggleVisualizer => Some(RpcRequest::ToggleVisualizer),
        PlaybackCommand::SkipForward(secs) => Some(RpcRequest::SkipForward { secs }),
        PlaybackCommand::SkipBackward(secs) => Some(RpcRequest::SkipBackward { secs }),
        PlaybackCommand::ClearQueue => Some(RpcRequest::ClearQueue),
        PlaybackCommand::SetAutoplay(enabled) => Some(RpcRequest::SetAutoplay { enabled }),
        PlaybackCommand::ToggleAutoplay => Some(RpcRequest::SetAutoplay {
            enabled: !last_autoplay.unwrap_or(false),
        }),
        PlaybackCommand::ListDevices => Some(RpcRequest::ListDevices),
        PlaybackCommand::SwitchDevice(name) => Some(RpcRequest::SwitchDevice { name }),
        PlaybackCommand::PlayNow(id) | PlaybackCommand::PlayTrack(id) => {
            let track = core
                .track(&id)
                .ok_or_else(|| anyhow!("Track not found: {id}"))?;
            Some(RpcRequest::PlayTrack { track })
        }
        PlaybackCommand::PlayCollection {
            ids, start_index, ..
        } => {
            let tracks = ordered_tracks(core, &ids, start_index)?;
            Some(RpcRequest::PlayTracks { tracks })
        }
        PlaybackCommand::Enqueue(id) => {
            let track = core
                .track(&id)
                .ok_or_else(|| anyhow!("Track not found: {id}"))?;
            Some(RpcRequest::EnqueueTrack { track })
        }
        PlaybackCommand::EnqueueMany(ids) => {
            let tracks = ids
                .iter()
                .map(|id| {
                    core.track(id)
                        .ok_or_else(|| anyhow!("Track not found: {id}"))
                })
                .collect::<Result<Vec<_>>>()?;
            Some(RpcRequest::EnqueueTracks { tracks })
        }
        PlaybackCommand::Quit => Some(RpcRequest::Shutdown),
        PlaybackCommand::PrefetchTrack(_) => None,
        _ => None,
    };

    let Some(request) = request else {
        return Ok(());
    };

    let response = send_request(daemon_addr, &request)?;
    if !response.message.trim().is_empty() && response.message != "status" {
        let _ = msg_tx.send(response.message);
    }

    if let Some(status) = response.status.as_ref() {
        apply_status(status);
    }

    Ok(())
}

fn ordered_tracks(core: &Core, ids: &[String], start_index: usize) -> Result<Vec<Track>> {
    if ids.is_empty() {
        return Err(anyhow!("Track list is empty"));
    }
    if start_index >= ids.len() {
        return Err(anyhow!("Start index is out of range"));
    }

    let mut ordered = Vec::with_capacity(ids.len());
    ordered.extend(ids[start_index..].iter().cloned());
    ordered.extend(ids[..start_index].iter().cloned());

    ordered
        .into_iter()
        .map(|id| {
            core.track(&id)
                .ok_or_else(|| anyhow!("Track not found: {id}"))
        })
        .collect::<Result<Vec<_>>>()
}

fn sync_core_from_status(core: &Core, status: &crate::daemon::PlayerStatus) {
    core.clear_queue();

    if let Some(track) = status.current.as_ref() {
        core.put_tracks(vec![track.clone()]);
        core.set_cur(Some(track.id.clone()));
    } else {
        core.set_cur(None);
    }

    for track in &status.queue {
        core.put_tracks(vec![track.clone()]);
        core.enqueue(track.id.clone());
    }
}
