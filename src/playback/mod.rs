use crate::core::Core;
use rodio::cpal::traits::{DeviceTrait, HostTrait};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use rustfft::{algorithm::Radix4, Fft, FftDirection};
use std::collections::VecDeque;
use std::fs::File;
use std::io::BufReader;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub enum PlaybackCommand {
    PlayIndex(usize),
    PlayFile(String),
    Pause,
    Resume,
    Stop,
    Next,
    Prev,
    Quit,
    VolumeUp,
    VolumeDown,
    ToggleMute,
    SkipForward(u64),
    SkipBackward(u64),
    ToggleVisualizer,
    ListDevices,
    SwitchDevice(String),
}

pub struct PlaybackHandle {
    pub tx: Sender<PlaybackCommand>,
    pub position_rx: Arc<Mutex<Option<(u64, u64, bool)>>>,
    pub devices_rx: Receiver<Vec<(String, bool)>>,
    pub volume_rx: Receiver<f32>,
    pub visualizer_rx: Receiver<Vec<f32>>,
}

struct VisualizerSource<S> {
    inner: S,
    fft_processor: Arc<Mutex<FftProcessor>>,
    visualizer_tx: Sender<Vec<f32>>,
    visualizer_enabled: Arc<Mutex<bool>>,
}

impl<S> VisualizerSource<S> {
    fn new(
        inner: S,
        fft_processor: Arc<Mutex<FftProcessor>>,
        visualizer_tx: Sender<Vec<f32>>,
        visualizer_enabled: Arc<Mutex<bool>>,
    ) -> Self {
        Self {
            inner,
            fft_processor,
            visualizer_tx,
            visualizer_enabled,
        }
    }
}

impl<S> Iterator for VisualizerSource<S>
where
    S: Source<Item = f32>,
{
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        let sample = self.inner.next()?;

        if *self.visualizer_enabled.lock().unwrap() {
            if let Ok(mut processor) = self.fft_processor.lock() {
                processor.add_sample(sample);
                if let Some(bands) = processor.get_bands() {
                    self.visualizer_tx.send(bands).ok();
                }
            }
        }

        Some(sample)
    }
}

impl<S> Source for VisualizerSource<S>
where
    S: Source<Item = f32>,
{
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
    }

    fn channels(&self) -> u16 {
        self.inner.channels()
    }

    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        self.inner.try_seek(pos)
    }
}

struct FftProcessor {
    fft: Radix4<f32>,
    buffer_size: usize,
    sample_buffer: VecDeque<f32>,
    sample_count: usize,
}

impl FftProcessor {
    fn new(buffer_size: usize) -> Self {
        FftProcessor {
            fft: Radix4::new(buffer_size, FftDirection::Forward),
            buffer_size,
            sample_buffer: VecDeque::with_capacity(buffer_size),
            sample_count: 0,
        }
    }

    fn add_sample(&mut self, sample: f32) {
        self.sample_buffer.push_back(sample);
        if self.sample_buffer.len() > self.buffer_size {
            self.sample_buffer.pop_front();
        }
        self.sample_count += 1;
    }

    fn get_bands(&mut self) -> Option<Vec<f32>> {
        if self.sample_count % 512 != 0 {
            return None;
        }

        if self.sample_buffer.len() < self.buffer_size {
            return Some(vec![0.0; 32]);
        }

        use num_complex::Complex;
        let mut input: Vec<Complex<f32>> = self
            .sample_buffer
            .iter()
            .map(|&s| Complex::new(s, 0.0))
            .collect();

        self.fft.process(&mut input);

        let num_bands = 32;
        let mut bands = vec![0.0f32; num_bands];
        let samples_per_band = self.buffer_size / num_bands;

        for (i, band) in bands.iter_mut().enumerate() {
            if i == 0 {
                let start = i * samples_per_band;
                let end = (i + 1) * samples_per_band;
                let magnitude: f32 = input[start..end].iter().map(|c| c.norm()).sum::<f32>()
                    / samples_per_band as f32;
                *band = if magnitude > 0.2 {
                    (magnitude * 1.5).min(0.85)
                } else {
                    0.0
                };
                continue;
            }
            let start = i * samples_per_band;
            let end = (i + 1) * samples_per_band;
            let magnitude: f32 =
                input[start..end].iter().map(|c| c.norm()).sum::<f32>() / samples_per_band as f32;
            let threshold = if i >= 15 && i <= 18 { 0.25 } else { 0.08 };
            let scale = if i >= 15 && i <= 18 { 1.2 } else { 2.0 };
            *band = if magnitude > threshold {
                (magnitude * scale).min(0.85)
            } else {
                0.0
            };
        }

        Some(bands)
    }
}

pub fn start_audio_thread(core: Core) -> PlaybackHandle {
    let (tx, rx): (Sender<PlaybackCommand>, Receiver<PlaybackCommand>) = mpsc::channel();
    let tx_clone = tx.clone();
    let position = Arc::new(Mutex::new(None));
    let position_clone = position.clone();

    let (volume_tx, volume_rx): (Sender<f32>, Receiver<f32>) = mpsc::channel();
    let (visualizer_tx, visualizer_rx): (Sender<Vec<f32>>, Receiver<Vec<f32>>) = mpsc::channel();
    let (devices_tx, devices_rx) = mpsc::channel::<Vec<(String, bool)>>();

    thread::spawn(move || {
        let host = rodio::cpal::default_host();
        let mut _stream: Option<OutputStream> = None;
        let mut stream_handle: Option<OutputStreamHandle> = None;

        if let Ok((stream, handle)) = OutputStream::try_default() {
            _stream = Some(stream);
            stream_handle = Some(handle);
        }

        let mut sink: Option<Sink> = None;
        let mut current_track_idx: Option<usize> = None;
        let mut current_duration: u64 = 0;
        let mut volume = 1.0f32;
        let mut elapsed_before_pause: u64 = 0;
        let mut playback_start: Option<Instant> = None;
        let mut is_paused = false;
        let mut current_device_name: Option<String> = None;

        if let Some(ref device) = host.default_output_device() {
            if let Ok(name) = device.name() {
                current_device_name = Some(name);
            }
        }

        let fft_processor = Arc::new(Mutex::new(FftProcessor::new(2048)));
        let visualizer_enabled = Arc::new(Mutex::new(false));

        volume_tx.send(volume).ok();

        loop {
            let current_pos = if let Some(ref _s) = sink {
                if is_paused {
                    elapsed_before_pause
                } else if let Some(start) = playback_start {
                    elapsed_before_pause + start.elapsed().as_secs()
                } else {
                    elapsed_before_pause
                }
            } else {
                elapsed_before_pause
            };

            let is_playing = sink
                .as_ref()
                .map(|s| !s.is_paused() && !s.empty())
                .unwrap_or(false);
            *position_clone.lock().unwrap() = Some((current_pos, current_duration, is_playing));

            if let Ok(cmd) = rx.recv_timeout(Duration::from_millis(50)) {
                match cmd {
                    PlaybackCommand::PlayIndex(idx) => {
                        let tracks = core.tracks.lock().unwrap();
                        if let Some(track) = tracks.get(idx) {
                            if let Some(ref path) = track.path {
                                let path_str = path.to_string_lossy().to_string();
                                drop(tracks);

                                sink = None;

                                current_track_idx = Some(idx);
                                let mut current = core.current.lock().unwrap();
                                *current = Some(idx);

                                let handle =
                                    stream_handle.as_ref().expect("No audio output available");
                                let new_sink = Sink::try_new(handle).unwrap();

                                if let Ok(file) = File::open(&path_str) {
                                    if let Ok(source) = Decoder::new(BufReader::new(file)) {
                                        current_duration = source
                                            .total_duration()
                                            .map(|d| d.as_secs())
                                            .unwrap_or(0);

                                        let tapped = VisualizerSource::new(
                                            source.convert_samples(),
                                            fft_processor.clone(),
                                            visualizer_tx.clone(),
                                            visualizer_enabled.clone(),
                                        );

                                        new_sink.append(tapped);
                                        new_sink.set_volume(volume);
                                        sink = Some(new_sink);
                                        is_paused = false;
                                        elapsed_before_pause = 0;
                                        playback_start = Some(Instant::now());
                                    }
                                }
                            }
                        }
                    }
                    PlaybackCommand::PlayFile(path) => {
                        sink = None;

                        let handle = stream_handle.as_ref().expect("No audio output available");
                        let new_sink = Sink::try_new(handle).unwrap();

                        if let Ok(file) = File::open(&path) {
                            if let Ok(source) = Decoder::new(BufReader::new(file)) {
                                current_duration =
                                    source.total_duration().map(|d| d.as_secs()).unwrap_or(0);

                                let tapped = VisualizerSource::new(
                                    source.convert_samples(),
                                    fft_processor.clone(),
                                    visualizer_tx.clone(),
                                    visualizer_enabled.clone(),
                                );

                                new_sink.append(tapped);
                                new_sink.set_volume(volume);
                                sink = Some(new_sink);
                                is_paused = false;
                                elapsed_before_pause = 0;
                                playback_start = Some(Instant::now());
                            }
                        }
                    }
                    PlaybackCommand::Pause => {
                        if let Some(ref s) = sink {
                            if !is_paused {
                                elapsed_before_pause = current_pos;
                                is_paused = true;
                                s.pause();
                            }
                        }
                    }
                    PlaybackCommand::Resume => {
                        if let Some(ref s) = sink {
                            if is_paused {
                                is_paused = false;
                                playback_start = Some(Instant::now());
                                s.play();
                            }
                        }
                    }
                    PlaybackCommand::Stop => {
                        sink = None;
                        current_track_idx = None;
                        is_paused = false;
                        elapsed_before_pause = 0;
                        current_duration = 0;
                    }
                    PlaybackCommand::VolumeUp => {
                        volume = (volume + 0.1).min(1.0);
                        if let Some(ref s) = sink {
                            s.set_volume(volume);
                        }
                        volume_tx.send(volume).ok();
                    }
                    PlaybackCommand::VolumeDown => {
                        volume = (volume - 0.1).max(0.0);
                        if let Some(ref s) = sink {
                            s.set_volume(volume);
                        }
                        volume_tx.send(volume).ok();
                    }
                    PlaybackCommand::ToggleMute => {
                        if let Some(ref s) = sink {
                            if s.volume() > 0.0 {
                                s.set_volume(0.0);
                            } else {
                                s.set_volume(volume);
                            }
                        }
                    }
                    PlaybackCommand::SkipForward(secs) => {
                        let new_pos = (current_pos + secs).min(current_duration);
                        if let Some(ref s) = sink {
                            match s.try_seek(Duration::from_secs(new_pos)) {
                                Ok(_) => {
                                    elapsed_before_pause = new_pos;
                                    playback_start = Some(Instant::now());
                                }
                                Err(e) => eprintln!("Seek forward failed: {:?}", e),
                            }
                        }
                    }
                    PlaybackCommand::SkipBackward(secs) => {
                        let new_pos = current_pos.saturating_sub(secs);
                        if let Some(ref s) = sink {
                            match s.try_seek(Duration::from_secs(new_pos)) {
                                Ok(_) => {
                                    elapsed_before_pause = new_pos;
                                    playback_start = Some(Instant::now());
                                }
                                Err(e) => eprintln!("Seek backward failed: {:?}", e),
                            }
                        }
                    }
                    PlaybackCommand::Next => {
                        if let Some(next_idx) = core.dequeue() {
                            tx_clone.send(PlaybackCommand::PlayIndex(next_idx)).ok();
                        }
                    }
                    PlaybackCommand::Prev => {
                        if let Some(current_idx) = current_track_idx {
                            if current_pos > 3 {
                                tx_clone.send(PlaybackCommand::PlayIndex(current_idx)).ok();
                            } else if current_idx > 0 {
                                tx_clone
                                    .send(PlaybackCommand::PlayIndex(current_idx - 1))
                                    .ok();
                            }
                        }
                    }
                    PlaybackCommand::ToggleVisualizer => {
                        let mut enabled = visualizer_enabled.lock().unwrap();
                        *enabled = !*enabled;
                    }
                    PlaybackCommand::ListDevices => {
                        let mut devices = Vec::new();
                        if let Ok(output_devices) = host.output_devices() {
                            for device in output_devices {
                                if let Ok(name) = device.name() {
                                    let is_current = current_device_name.as_ref() == Some(&name);
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
                        devices_tx.send(devices).ok();
                    }
                    PlaybackCommand::SwitchDevice(device_name) => {
                        let saved_track_idx = current_track_idx;
                        let saved_position = current_pos;
                        let was_playing = !is_paused && sink.is_some();

                        sink = None;
                        stream_handle = None;
                        _stream = None;

                        if let Ok(devices) = host.output_devices() {
                            for device in devices {
                                if let Ok(name) = device.name() {
                                    if name == device_name {
                                        match OutputStream::try_from_device(&device) {
                                            Ok((stream, handle)) => {
                                                _stream = Some(stream);
                                                stream_handle = Some(handle);
                                                current_device_name = Some(name);

                                                if let Some(idx) = saved_track_idx {
                                                    if let Some(track) =
                                                        core.tracks.lock().unwrap().get(idx)
                                                    {
                                                        if let Some(ref path) = track.path {
                                                            if let Ok(file) = File::open(path) {
                                                                if let Ok(source) = Decoder::new(
                                                                    BufReader::new(file),
                                                                ) {
                                                                    let handle = stream_handle
                                                                        .as_ref()
                                                                        .unwrap();
                                                                    let new_sink =
                                                                        Sink::try_new(handle)
                                                                            .unwrap();

                                                                    let tapped =
                                                                        VisualizerSource::new(
                                                                            source
                                                                                .convert_samples(),
                                                                            fft_processor.clone(),
                                                                            visualizer_tx.clone(),
                                                                            visualizer_enabled
                                                                                .clone(),
                                                                        );

                                                                    new_sink.append(tapped);
                                                                    new_sink.set_volume(volume);

                                                                    if saved_position > 0 {
                                                                        new_sink
                                                                            .try_seek(
                                                                                Duration::from_secs(
                                                                                    saved_position,
                                                                                ),
                                                                            )
                                                                            .ok();
                                                                    }

                                                                    if !was_playing {
                                                                        new_sink.pause();
                                                                        is_paused = true;
                                                                    }

                                                                    sink = Some(new_sink);
                                                                    elapsed_before_pause =
                                                                        saved_position;
                                                                    playback_start =
                                                                        Some(Instant::now());
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                                break;
                                            }
                                            Err(e) => {
                                                eprintln!(
                                                    "Failed to switch to device {}: {:?}",
                                                    device_name, e
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    PlaybackCommand::Quit => break,
                }
            }
        }
    });

    PlaybackHandle {
        tx,
        position_rx: position,
        devices_rx,
        volume_rx,
        visualizer_rx,
    }
}
