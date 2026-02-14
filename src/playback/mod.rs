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
    SkipForward(u64),  // seconds
    SkipBackward(u64), // seconds
}

pub struct PlaybackHandle {
    pub tx: Sender<PlaybackCommand>,
    pub position_rx: Arc<Mutex<Option<(u64, u64, bool)>>>, // (current, total, is_playing)
    pub devices_rx: Receiver<Vec<(String, bool)>>,
    pub volume_rx: Receiver<f32>,
}

struct PlaybackState {
    current_track_idx: Option<usize>,
    current_track_duration: Option<u64>,
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

    fn set_elapsed(&mut self, elapsed: u64) {
        self.elapsed_before_pause = elapsed;
        if !self.is_paused {
            self.playback_start = Some(Instant::now());
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

        // Initialize with default device
        if let Ok((stream, handle)) = OutputStream::try_default() {
            _current_stream = Some(stream);
            stream_handle = Some(handle);
            // Get default device name
            if let Some(device) = host.default_output_device() {
                if let Ok(name) = device.name() {
                    state.current_device_name = Some(name);
                }
            }
        }

        // Send initial volume
        volume_tx.send(state.volume).ok();

        loop {
            // Update position
            *position_clone.lock().unwrap() = state.update_position();

            // Check if device is still connected by trying to get volume
            if let Some(ref sink) = sink_opt {
                // Try to check if sink is still valid by checking if it's paused
                // If the device was disconnected, this might fail or behave unexpectedly
                let _ = sink.is_paused(); // This is a no-op but keeps the sink alive

                // Check for device disconnection by polling sink state
                if sink.empty() && !state.is_paused && state.current_track_idx.is_some() {
                    // Track might have finished, or device disconnected
                    // Wait a bit and check again
                    thread::sleep(Duration::from_millis(100));
                    if sink.empty() && !state.is_paused {
                        // Track finished naturally
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
                            state.is_paused = true;
                        }
                    }
                }
            }

            // Check for stream errors by attempting to recreate if needed
            if device_error_count > 0 && stream_handle.is_none() {
                // Try to reconnect to any available device
                if let Some(device) = find_next_available_device(&host) {
                    if let Ok((stream, handle)) = OutputStream::try_from_device(&device) {
                        _current_stream = Some(stream);
                        stream_handle = Some(handle);
                        device_error_count = 0;

                        // Resume playback if there was a track
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
                        // Restart current track or go to previous
                        if let Some(idx) = state.current_track_idx {
                            let prev_idx = if state.get_elapsed() < 3 {
                                // If less than 3 seconds in, go to previous track
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
                        let new_pos = if let Some(total) = state.current_track_duration {
                            (current + secs).min(total)
                        } else {
                            current + secs
                        };

                        // To skip forward, we need to restart from new position
                        if let Some(idx) = state.current_track_idx {
                            play_track(
                                &core,
                                idx,
                                &mut sink_opt,
                                &stream_handle,
                                &mut state,
                                &position_clone,
                                new_pos,
                            );
                        }
                    }
                    PlaybackCommand::SkipBackward(secs) => {
                        let current = state.get_elapsed();
                        let new_pos = current.saturating_sub(secs);

                        // Restart from new position
                        if let Some(idx) = state.current_track_idx {
                            play_track(
                                &core,
                                idx,
                                &mut sink_opt,
                                &stream_handle,
                                &mut state,
                                &position_clone,
                                new_pos,
                            );
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
                        // Get current position before switching
                        let current_position = state.get_elapsed();
                        let was_playing = !state.is_paused;

                        if let Ok(devices) = host.output_devices() {
                            for device in devices {
                                if let Ok(name) = device.name() {
                                    if name == device_name {
                                        // Stop current playback but keep state
                                        if let Some(sink) = sink_opt.take() {
                                            sink.stop();
                                        }

                                        // Create new stream with selected device
                                        match OutputStream::try_from_device(&device) {
                                            Ok((stream, handle)) => {
                                                _current_stream = Some(stream);
                                                stream_handle = Some(handle);
                                                state.current_device_name = Some(name);
                                                device_error_count = 0;

                                                // Resume playback from same position if there was a track
                                                if let Some(idx) = state.current_track_idx {
                                                    play_track(
                                                        &core,
                                                        idx,
                                                        &mut sink_opt,
                                                        &stream_handle,
                                                        &mut state,
                                                        &position_clone,
                                                        current_position,
                                                    );

                                                    // Restore pause state
                                                    if !was_playing {
                                                        if let Some(ref sink) = sink_opt {
                                                            sink.pause();
                                                            state.is_paused = true;
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
            // Check if device supports output
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
    // Stop current playback
    if let Some(sink) = sink_opt.take() {
        sink.stop();
    }

    let tracks = core.tracks.lock().unwrap();
    if let Some(track) = tracks.get(idx) {
        state.current_track_duration = track.duration_seconds;

        match &track.track_type {
            crate::core::track::TrackType::Local => {
                if let Some(path) = &track.path {
                    if let Ok(file) = File::open(path) {
                        match Decoder::new(BufReader::new(file)) {
                            Ok(decoder) => {
                                if let Some(ref handle) = stream_handle {
                                    match Sink::try_new(handle) {
                                        Ok(sink) => {
                                            sink.set_volume(state.volume);

                                            // Apply seek if needed
                                            let source: Box<dyn Source<Item = i16> + Send> =
                                                if seek_position > 0 {
                                                    // Use skip_duration to seek
                                                    let duration_to_skip =
                                                        Duration::from_secs(seek_position);
                                                    Box::new(
                                                        decoder.skip_duration(duration_to_skip),
                                                    )
                                                } else {
                                                    Box::new(decoder)
                                                };

                                            sink.append(source);
                                            sink.play();
                                            *sink_opt = Some(sink);

                                            state.current_track_idx = Some(idx);
                                            state.elapsed_before_pause = seek_position;
                                            state.playback_start = Some(Instant::now());
                                            state.is_paused = false;

                                            let mut cur = core.current.lock().unwrap();
                                            *cur = Some(idx);

                                            *position_clone.lock().unwrap() = Some((
                                                seek_position,
                                                track.duration_seconds.unwrap_or(0),
                                                true,
                                            ));
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
            }
            crate::core::track::TrackType::SoundCloud => {
                eprintln!(
                    "SoundCloud streaming not yet implemented for: {}",
                    track.title
                );
            }
        }
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

    // Sort: current device first, then alphabetically
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
