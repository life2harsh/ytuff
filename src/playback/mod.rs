use crate::core::Core;
use rodio::cpal::traits::{DeviceTrait, HostTrait};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use std::fs::File;
use std::io::BufReader;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub enum PlaybackCommand {
    PlayIndex(usize),
    Pause,
    Resume,
    Stop,
    Next,
    Prev,
    Quit,
    VolumeUp,
    VolumeDown,
    ToggleMute,
    SetVolume(f32),
    ListDevices,
    SwitchDevice(String),
    SkipForward(u64),
    SkipBackward(u64),
}

pub struct PlaybackHandle {
    pub tx: Sender<PlaybackCommand>,
    pub position_rx: Arc<Mutex<Option<(u64, u64, bool)>>>,
    pub devices_rx: Receiver<Vec<(String, bool)>>,
    pub volume_rx: Receiver<f32>,
}

struct PlaybackState {
    current_track_idx: Option<usize>,
    current_track_duration: Option<u64>,
    current_track_path: Option<std::path::PathBuf>,
    playback_start: Option<Instant>,
    elapsed_before_pause: u64,
    is_paused: bool,
    volume: f32,
    muted_volume: Option<f32>,
    current_device_name: Option<String>,
}

impl PlaybackState {
    fn new() -> Self {
        PlaybackState {
            current_track_idx: None,
            current_track_duration: None,
            current_track_path: None,
            playback_start: None,
            elapsed_before_pause: 0,
            is_paused: true,
            volume: 1.0,
            muted_volume: None,
            current_device_name: None,
        }
    }

    fn get_elapsed(&self) -> u64 {
        if self.is_paused {
            self.elapsed_before_pause
        } else if let Some(start) = self.playback_start {
            self.elapsed_before_pause + start.elapsed().as_secs()
        } else {
            0
        }
    }

    fn update_position(&self) -> Option<(u64, u64, bool)> {
        self.current_track_duration.map(|total| {
            let elapsed = self.get_elapsed();
            let current = elapsed.min(total);
            (current, total, !self.is_paused)
        })
    }
}

pub fn start_playback_thread(core: Core) -> PlaybackHandle {
    let (tx, rx): (Sender<PlaybackCommand>, Receiver<PlaybackCommand>) = mpsc::channel();
    let position = Arc::new(Mutex::new(None));
    let position_clone = position.clone();

    let (devices_tx, devices_rx): (Sender<Vec<(String, bool)>>, Receiver<Vec<(String, bool)>>) =
        mpsc::channel();

    let (volume_tx, volume_rx): (Sender<f32>, Receiver<f32>) = mpsc::channel();

    thread::spawn(move || {
        let host = rodio::cpal::default_host();
        let mut _current_stream: Option<OutputStream> = None;
        let mut stream_handle: Option<OutputStreamHandle> = None;
        let mut sink_opt: Option<Sink> = None;
        let mut state = PlaybackState::new();
        let mut device_error_count = 0;

        if let Ok((stream, handle)) = OutputStream::try_default() {
            _current_stream = Some(stream);
            stream_handle = Some(handle);
            if let Some(device) = host.default_output_device() {
                if let Ok(name) = device.name() {
                    state.current_device_name = Some(name);
                }
            }
        }

        volume_tx.send(state.volume).ok();

        loop {
            *position_clone.lock().unwrap() = state.update_position();

            if let Some(ref sink) = sink_opt {
                let _ = sink.is_paused();

                if sink.empty() && !state.is_paused && state.current_track_idx.is_some() {
                    thread::sleep(Duration::from_millis(100));
                    if sink.empty() && !state.is_paused {
                        if let Some(next_idx) = core.dequeue() {
                            play_track(
                                &core,
                                next_idx,
                                &mut sink_opt,
                                &stream_handle,
                                &mut state,
                                &position_clone,
                                0,
                            );
                        } else {
                            state.current_track_idx = None;
                            state.current_track_path = None;
                            state.is_paused = true;
                        }
                    }
                }
            }

            if device_error_count > 0 && stream_handle.is_none() {
                if let Some(device) = find_next_available_device(&host) {
                    if let Ok((stream, handle)) = OutputStream::try_from_device(&device) {
                        _current_stream = Some(stream);
                        stream_handle = Some(handle);
                        device_error_count = 0;

                        if let Some(idx) = state.current_track_idx {
                            let seek_pos = state.get_elapsed();
                            play_track(
                                &core,
                                idx,
                                &mut sink_opt,
                                &stream_handle,
                                &mut state,
                                &position_clone,
                                seek_pos,
                            );
                        }

                        if let Ok(name) = device.name() {
                            state.current_device_name = Some(name);
                        }
                    }
                }
            }

            if let Ok(cmd) = rx.recv_timeout(Duration::from_millis(50)) {
                match cmd {
                    PlaybackCommand::PlayIndex(idx) => {
                        play_track(
                            &core,
                            idx,
                            &mut sink_opt,
                            &stream_handle,
                            &mut state,
                            &position_clone,
                            0,
                        );
                    }
                    PlaybackCommand::Pause => {
                        if let Some(ref sink) = sink_opt {
                            if !sink.is_paused() {
                                state.elapsed_before_pause = state.get_elapsed();
                                state.is_paused = true;
                                sink.pause();
                            }
                        }
                    }
                    PlaybackCommand::Resume => {
                        if let Some(ref sink) = sink_opt {
                            if sink.is_paused() {
                                sink.play();
                                state.is_paused = false;
                                state.playback_start = Some(Instant::now());
                            }
                        }
                    }
                    PlaybackCommand::Stop => {
                        if let Some(sink) = sink_opt.take() {
                            sink.stop();
                        }
                        state.current_track_idx = None;
                        state.current_track_path = None;
                        state.is_paused = true;
                        *position_clone.lock().unwrap() = None;
                    }
                    PlaybackCommand::Next => {
                        if let Some(next_idx) = core.dequeue() {
                            play_track(
                                &core,
                                next_idx,
                                &mut sink_opt,
                                &stream_handle,
                                &mut state,
                                &position_clone,
                                0,
                            );
                        }
                    }
                    PlaybackCommand::Prev => {
                        if let Some(idx) = state.current_track_idx {
                            let prev_idx = if state.get_elapsed() < 3 {
                                idx.saturating_sub(1)
                            } else {
                                idx
                            };
                            play_track(
                                &core,
                                prev_idx,
                                &mut sink_opt,
                                &stream_handle,
                                &mut state,
                                &position_clone,
                                0,
                            );
                        }
                    }
                    PlaybackCommand::SkipForward(secs) => {
                        let current = state.get_elapsed();
                        let total = state.current_track_duration.unwrap_or(0);
                        let new_pos = (current + secs).min(total);

                        if new_pos >= total && total > 0 {
                            if let Some(next_idx) = core.dequeue() {
                                play_track(
                                    &core,
                                    next_idx,
                                    &mut sink_opt,
                                    &stream_handle,
                                    &mut state,
                                    &position_clone,
                                    0,
                                );
                            }
                        } else if let Some(idx) = state.current_track_idx {
                            if let Some(ref path) = state.current_track_path.clone() {
                                if let Some(sink) = sink_opt.take() {
                                    sink.stop();
                                }
                                play_track_from_path(
                                    &core,
                                    idx,
                                    path,
                                    &mut sink_opt,
                                    &stream_handle,
                                    &mut state,
                                    &position_clone,
                                    new_pos,
                                );
                            }
                        }
                    }
                    PlaybackCommand::SkipBackward(secs) => {
                        let current = state.get_elapsed();
                        let new_pos = current.saturating_sub(secs);

                        if let Some(idx) = state.current_track_idx {
                            if let Some(ref path) = state.current_track_path.clone() {
                                if let Some(sink) = sink_opt.take() {
                                    sink.stop();
                                }
                                play_track_from_path(
                                    &core,
                                    idx,
                                    path,
                                    &mut sink_opt,
                                    &stream_handle,
                                    &mut state,
                                    &position_clone,
                                    new_pos,
                                );
                            }
                        }
                    }
                    PlaybackCommand::VolumeUp => {
                        state.volume = (state.volume + 0.1).min(1.0);
                        if let Some(ref sink) = sink_opt {
                            sink.set_volume(state.volume);
                        }
                        volume_tx.send(state.volume).ok();
                    }
                    PlaybackCommand::VolumeDown => {
                        state.volume = (state.volume - 0.1).max(0.0);
                        if let Some(ref sink) = sink_opt {
                            sink.set_volume(state.volume);
                        }
                        volume_tx.send(state.volume).ok();
                    }
                    PlaybackCommand::ToggleMute => {
                        if state.muted_volume.is_some() {
                            state.volume = state.muted_volume.take().unwrap();
                        } else {
                            state.muted_volume = Some(state.volume);
                            state.volume = 0.0;
                        }
                        if let Some(ref sink) = sink_opt {
                            sink.set_volume(state.volume);
                        }
                        volume_tx.send(state.volume).ok();
                    }
                    PlaybackCommand::SetVolume(vol) => {
                        state.volume = vol.clamp(0.0, 1.0);
                        if let Some(ref sink) = sink_opt {
                            sink.set_volume(state.volume);
                        }
                        volume_tx.send(state.volume).ok();
                    }
                    PlaybackCommand::ListDevices => {
                        let devices = list_audio_devices(&host, &state.current_device_name);
                        devices_tx.send(devices).ok();
                    }
                    PlaybackCommand::SwitchDevice(device_name) => {
                        let current_position = state.get_elapsed();
                        let was_playing = !state.is_paused;

                        if let Ok(devices) = host.output_devices() {
                            for device in devices {
                                if let Ok(name) = device.name() {
                                    if name == device_name {
                                        if let Some(sink) = sink_opt.take() {
                                            sink.stop();
                                        }

                                        match OutputStream::try_from_device(&device) {
                                            Ok((stream, handle)) => {
                                                _current_stream = Some(stream);
                                                stream_handle = Some(handle);
                                                state.current_device_name = Some(name);
                                                device_error_count = 0;

                                                if let Some(ref path) =
                                                    state.current_track_path.clone()
                                                {
                                                    if let Some(idx) = state.current_track_idx {
                                                        play_track_from_path(
                                                            &core,
                                                            idx,
                                                            path,
                                                            &mut sink_opt,
                                                            &stream_handle,
                                                            &mut state,
                                                            &position_clone,
                                                            current_position,
                                                        );

                                                        if !was_playing {
                                                            if let Some(ref sink) = sink_opt {
                                                                sink.pause();
                                                                state.is_paused = true;
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                eprintln!(
                                                    "Failed to switch to device {}: {:?}",
                                                    device_name, e
                                                );
                                                device_error_count += 1;
                                            }
                                        }
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    PlaybackCommand::Quit => {
                        if let Some(sink) = sink_opt.take() {
                            sink.stop();
                        }
                        break;
                    }
                }
            }
        }
    });

    PlaybackHandle {
        tx,
        position_rx: position,
        devices_rx,
        volume_rx,
    }
}

fn find_next_available_device(host: &rodio::cpal::Host) -> Option<rodio::Device> {
    if let Ok(devices) = host.output_devices() {
        for device in devices {
            if let Ok(configs) = device.supported_output_configs() {
                if configs.count() > 0 {
                    return Some(device);
                }
            }
        }
    }
    host.default_output_device()
}

fn play_track(
    core: &Core,
    idx: usize,
    sink_opt: &mut Option<Sink>,
    stream_handle: &Option<OutputStreamHandle>,
    state: &mut PlaybackState,
    position_clone: &Arc<Mutex<Option<(u64, u64, bool)>>>,
    seek_position: u64,
) {
    if seek_position == 0 {
        core.add_to_history(idx);
    }

    let tracks = core.tracks.lock().unwrap();
    if let Some(track) = tracks.get(idx) {
        state.current_track_duration = track.duration_seconds;
        state.current_track_path = track.path.clone();
        drop(tracks);

        if let Some(ref path) = state.current_track_path.clone() {
            if let Some(sink) = sink_opt.take() {
                sink.stop();
            }
            play_track_from_path(
                core,
                idx,
                path,
                sink_opt,
                stream_handle,
                state,
                position_clone,
                seek_position,
            );
        }
    }
}

fn play_track_from_path(
    _core: &Core,
    idx: usize,
    path: &std::path::Path,
    sink_opt: &mut Option<Sink>,
    stream_handle: &Option<OutputStreamHandle>,
    state: &mut PlaybackState,
    position_clone: &Arc<Mutex<Option<(u64, u64, bool)>>>,
    seek_position: u64,
) {
    if let Ok(file) = File::open(path) {
        let total_duration = state.current_track_duration.unwrap_or(0);
        let clamped_seek = seek_position.min(total_duration);

        match Decoder::new(BufReader::new(file)) {
            Ok(decoder) => {
                if let Some(ref handle) = stream_handle {
                    match Sink::try_new(handle) {
                        Ok(sink) => {
                            sink.set_volume(state.volume);

                            let source: Box<dyn Source<Item = i16> + Send> = if clamped_seek > 0 {
                                let duration_to_skip = Duration::from_secs(clamped_seek);
                                Box::new(decoder.skip_duration(duration_to_skip))
                            } else {
                                Box::new(decoder)
                            };

                            sink.append(source);
                            sink.play();
                            *sink_opt = Some(sink);

                            state.current_track_idx = Some(idx);
                            state.elapsed_before_pause = clamped_seek;
                            state.playback_start = Some(Instant::now());
                            state.is_paused = false;

                            let mut cur = _core.current.lock().unwrap();
                            *cur = Some(idx);

                            *position_clone.lock().unwrap() =
                                Some((clamped_seek, total_duration, true));
                        }
                        Err(e) => {
                            eprintln!("Failed to create sink: {:?}", e);
                        }
                    }
                } else {
                    eprintln!("No audio output available");
                }
            }
            Err(e) => {
                eprintln!("Failed to decode audio file: {:?}", e);
            }
        }
    } else {
        eprintln!("Failed to open file: {:?}", path);
    }
}

fn list_audio_devices(
    host: &rodio::cpal::Host,
    current_device: &Option<String>,
) -> Vec<(String, bool)> {
    let mut devices = Vec::new();

    if let Ok(output_devices) = host.output_devices() {
        for device in output_devices {
            if let Ok(name) = device.name() {
                let is_current = current_device.as_ref() == Some(&name);
                devices.push((name, is_current));
            }
        }
    }

    devices.sort_by(|a, b| {
        if a.1 && !b.1 {
            std::cmp::Ordering::Less
        } else if !a.1 && b.1 {
            std::cmp::Ordering::Greater
        } else {
            a.0.cmp(&b.0)
        }
    });

    devices
}
