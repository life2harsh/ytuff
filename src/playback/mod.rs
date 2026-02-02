use crate::core::Core;
use rodio::{OutputStream, Sink, Decoder};
use std::sync::mpsc::{self, Sender, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::fs::File;
use std::io::BufReader;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub enum PlaybackCommand {
    PlayIndex(usize),
    Pause,
    Resume,
    Stop,
    Next,
    Prev,
    Quit,
}

pub struct PlaybackHandle {
    pub tx: Sender<PlaybackCommand>,
    pub position_rx: std::sync::Arc<std::sync::Mutex<Option<(u64, u64)>>>,
}

pub fn start_playback_thread(core: Core) -> PlaybackHandle {
    let (tx, rx): (Sender<PlaybackCommand>, Receiver<PlaybackCommand>) = mpsc::channel();
    let position = Arc::new(Mutex::new(None));
    let position_clone = position.clone();

    thread::spawn(move || {
        let (_stream, stream_handle) = OutputStream::try_default().expect("Failed to open output stream");
        let mut sink_opt: Option<Sink> = None;
        let mut current_track_duration: Option<u64> = None;
        let mut playback_start: Option<Instant> = None;
        let mut elapsed_before_pause: u64 = 0;

        loop {
            if let Some(ref sink) = sink_opt {
                if !sink.is_paused() && !sink.empty() {
                    if let (Some(start), Some(total)) = (playback_start, current_track_duration) {
                        let elapsed = start.elapsed().as_secs() + elapsed_before_pause;
                        let current = elapsed.min(total);
                        *position_clone.lock().unwrap() = Some((current, total));
                    }
                }
            }

            if let Ok(cmd) = rx.recv_timeout(Duration::from_millis(100)) {
                match cmd {
                    PlaybackCommand::PlayIndex(idx) => {
                        if let Some(s) = sink_opt.take() {
                            s.stop();
                        }
                        let tracks = core.tracks.lock().unwrap();
                        if let Some(track) = tracks.get(idx) {
                            current_track_duration = track.duration_seconds;
                            match &track.track_type {
                                crate::core::track::TrackType::Local => {
                                    if let Some(path) = &track.path {
                                        if let Ok(file) = File::open(path) {
                                            let source = Decoder::new(BufReader::new(file));
                                            match source {
                                                Ok(decoder) => {
                                                    let sink = Sink::try_new(&stream_handle).expect("Failed to create sink");
                                                    sink.append(decoder);
                                                    sink.play();
                                                    sink_opt = Some(sink);
                                                    playback_start = Some(Instant::now());
                                                    elapsed_before_pause = 0;
                                                    let mut cur = core.current.lock().unwrap();
                                                    *cur = Some(idx);
                                                    *position_clone.lock().unwrap() = Some((0, current_track_duration.unwrap_or(0)));
                                                }
                                                Err(e) => {
                                                    eprintln!("Decode error: {:?}", e);
                                                }
                                            }
                                        }
                                    }
                                }
                                crate::core::track::TrackType::SoundCloud => {
                                    eprintln!("SoundCloud streaming not yet implemented for: {}", track.title);
                                }
                            }
                        }
                    }
                    PlaybackCommand::Pause => {
                        if let Some(s) = &sink_opt {
                            if !s.is_paused() {
                                if let Some(start) = playback_start {
                                    elapsed_before_pause += start.elapsed().as_secs();
                                }
                                s.pause();
                            }
                        }
                    }
                    PlaybackCommand::Resume => {
                        if let Some(s) = &sink_opt {
                            if s.is_paused() {
                                s.play();
                                playback_start = Some(Instant::now());
                            }
                        }
                    }
                    PlaybackCommand::Stop => {
                        if let Some(s) = sink_opt.take() {
                            s.stop();
                        }
                        *position_clone.lock().unwrap() = None;
                    }
                    PlaybackCommand::Next => {
                        if let Some(next_idx) = core.dequeue() {
                            if let Some(s) = sink_opt.take() {
                                s.stop();
                            }
                            let tracks = core.tracks.lock().unwrap();
                            if let Some(track) = tracks.get(next_idx) {
                                current_track_duration = track.duration_seconds;
                                match &track.track_type {
                                    crate::core::track::TrackType::Local => {
                                        if let Some(path) = &track.path {
                                            if let Ok(file) = File::open(path) {
                                                if let Ok(decoder) = Decoder::new(BufReader::new(file)) {
                                                    let sink = Sink::try_new(&stream_handle).expect("Failed to create sink");
                                                    sink.append(decoder);
                                                    sink.play();
                                                    sink_opt = Some(sink);
                                                    playback_start = Some(Instant::now());
                                                    elapsed_before_pause = 0;
                                                    let mut cur = core.current.lock().unwrap();
                                                    *cur = Some(next_idx);
                                                    *position_clone.lock().unwrap() = Some((0, current_track_duration.unwrap_or(0)));
                                                }
                                            }
                                        }
                                    }
                                    crate::core::track::TrackType::SoundCloud => {
                                        eprintln!("SoundCloud streaming not yet implemented for: {}", track.title);
                                    }
                                }
                            }
                        }
                    }
                    PlaybackCommand::Prev => {
                    }
                    PlaybackCommand::Quit => {
                        if let Some(s) = sink_opt.take() {
                            s.stop();
                        }
                        break;
                    }
                }
            }
        }
    });

    PlaybackHandle { tx, position_rx: position }
}
