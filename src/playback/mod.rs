use crate::core::track::Track;
use crate::core::Core;
use crate::sources::soundcloud::SoundCloudClient;
use rodio::buffer::SamplesBuffer;
use rodio::cpal::traits::{DeviceTrait, HostTrait};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use rustfft::{algorithm::Radix4, Fft, FftDirection};
use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::default::{get_codecs, get_probe};

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum PlaybackCommand {
    PlayIndex(usize),
    PlayTrack(String),
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
    ToggleAutoplay,
    SetAutoplay(bool),
    ListDevices,
    SwitchDevice(String),
}

pub struct PlaybackHandle {
    pub tx: Sender<PlaybackCommand>,
    pub position_rx: Arc<Mutex<Option<(u64, u64, bool)>>>,
    pub devices_rx: Receiver<Vec<(String, bool)>>,
    pub autoplay_rx: Receiver<bool>,
    pub volume_rx: Receiver<f32>,
    pub visualizer_rx: Receiver<Vec<f32>>,
    pub msg_rx: Receiver<String>,
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

fn sink_from_reader<R>(
    handle: &OutputStreamHandle,
    reader: R,
    fft_processor: Arc<Mutex<FftProcessor>>,
    visualizer_tx: Sender<Vec<f32>>,
    visualizer_enabled: Arc<Mutex<bool>>,
    volume: f32,
) -> anyhow::Result<(Sink, u64)>
where
    R: Read + Seek + Send + Sync + 'static,
{
    let source = Decoder::new(BufReader::new(reader))?;
    let duration = source.total_duration().map(|d| d.as_secs()).unwrap_or(0);
    sink_from_source(
        handle,
        source.convert_samples(),
        duration,
        fft_processor,
        visualizer_tx,
        visualizer_enabled,
        volume,
    )
}

fn sink_from_source<S>(
    handle: &OutputStreamHandle,
    source: S,
    duration: u64,
    fft_processor: Arc<Mutex<FftProcessor>>,
    visualizer_tx: Sender<Vec<f32>>,
    visualizer_enabled: Arc<Mutex<bool>>,
    volume: f32,
) -> anyhow::Result<(Sink, u64)>
where
    S: Source<Item = f32> + Send + 'static,
{
    let tapped = VisualizerSource::new(source, fft_processor, visualizer_tx, visualizer_enabled);

    let sink = Sink::try_new(handle)?;
    sink.append(tapped);
    sink.set_volume(volume);
    Ok((sink, duration))
}

fn sink_from_local_file(
    handle: &OutputStreamHandle,
    path: &str,
    fft_processor: Arc<Mutex<FftProcessor>>,
    visualizer_tx: Sender<Vec<f32>>,
    visualizer_enabled: Arc<Mutex<bool>>,
    volume: f32,
) -> anyhow::Result<(Sink, u64)> {
    let file = File::open(path)?;
    sink_from_reader(
        handle,
        file,
        fft_processor,
        visualizer_tx,
        visualizer_enabled,
        volume,
    )
}

fn sink_from_remote_track(
    handle: &OutputStreamHandle,
    track: &Track,
    sc_client: &Arc<Mutex<SoundCloudClient>>,
    fft_processor: Arc<Mutex<FftProcessor>>,
    visualizer_tx: Sender<Vec<f32>>,
    visualizer_enabled: Arc<Mutex<bool>>,
    volume: f32,
) -> anyhow::Result<(Sink, u64)> {
    let bytes = {
        let mut client = sc_client.lock().unwrap();
        if let Some(bytes) = client.take_cached_audio(&track.id) {
            bytes
        } else {
            let first = client.stream(track)?;

            match client.download_stream(&first.url) {
                Ok(bytes) => bytes,
                Err(_) => {
                    client.invalidate_stream(&track.id);
                    let retry = client.stream(track)?;
                    client.download_stream(&retry.url)?
                }
            }
        }
    };

    let fallback_duration = track.dur.unwrap_or(0);
    let (sink, duration) = if bytes.get(4..8) == Some(b"ftyp") {
        sink_from_m4a_bytes(
            handle,
            bytes,
            fft_processor,
            visualizer_tx,
            visualizer_enabled,
            volume,
        )?
    } else {
        sink_from_reader(
            handle,
            Cursor::new(bytes),
            fft_processor,
            visualizer_tx,
            visualizer_enabled,
            volume,
        )?
    };

    Ok((
        sink,
        if duration == 0 {
            fallback_duration
        } else {
            duration
        },
    ))
}

fn prefetch_next_remote_track(core: &Core, sc_client: &Arc<Mutex<SoundCloudClient>>) {
    let Some(next_id) = core.q_ids().into_iter().next() else {
        return;
    };
    let Some(track) = core.track(&next_id) else {
        return;
    };
    if !track.is_sc() {
        return;
    }

    let sc_client = Arc::clone(sc_client);
    thread::spawn(move || {
        if let Ok(mut client) = sc_client.lock() {
            let _ = client.prefetch_track(&track);
        }
    });
}

struct VecMediaSource {
    inner: Cursor<Vec<u8>>,
    len: u64,
}

impl VecMediaSource {
    fn new(bytes: Vec<u8>) -> Self {
        let len = bytes.len() as u64;
        Self {
            inner: Cursor::new(bytes),
            len,
        }
    }
}

impl Read for VecMediaSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buf)
    }
}

impl Seek for VecMediaSource {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

impl MediaSource for VecMediaSource {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.len)
    }
}

pub(crate) fn decode_m4a_bytes(bytes: Vec<u8>) -> anyhow::Result<(u16, u32, Vec<f32>)> {
    let mss = MediaSourceStream::new(Box::new(VecMediaSource::new(bytes)), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("m4a");

    let format_opts = FormatOptions {
        enable_gapless: true,
        ..Default::default()
    };
    let metadata_opts: MetadataOptions = Default::default();
    let probed = get_probe().format(&hint, mss, &format_opts, &metadata_opts)?;
    let mut format = probed.format;
    let track = format
        .default_track()
        .ok_or_else(|| anyhow::anyhow!("The YouTube stream did not include an audio track"))?;
    let track_id = track.id;
    let mut decoder = get_codecs().make(&track.codec_params, &DecoderOptions::default())?;
    let mut channels = None;
    let mut sample_rate = None;
    let mut samples = Vec::new();

    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(err))
                if err.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(err) => return Err(err.into()),
        };

        if packet.track_id() != track_id {
            continue;
        }

        match decoder.decode(&packet) {
            Ok(audio_buf) => {
                let spec = *audio_buf.spec();
                channels.get_or_insert(spec.channels.count() as u16);
                sample_rate.get_or_insert(spec.rate);
                let mut buf = SampleBuffer::<f32>::new(audio_buf.capacity() as u64, spec);
                buf.copy_interleaved_ref(audio_buf);
                samples.extend_from_slice(buf.samples());
            }
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(SymphoniaError::IoError(err))
                if err.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(SymphoniaError::ResetRequired) => {
                return Err(anyhow::anyhow!("The YouTube decoder requested a reset"));
            }
            Err(err) => return Err(err.into()),
        }
    }

    let channels = channels
        .ok_or_else(|| anyhow::anyhow!("The YouTube stream did not report any channels"))?;
    let sample_rate = sample_rate
        .ok_or_else(|| anyhow::anyhow!("The YouTube stream did not report a sample rate"))?;

    Ok((channels, sample_rate, samples))
}

fn sink_from_m4a_bytes(
    handle: &OutputStreamHandle,
    bytes: Vec<u8>,
    fft_processor: Arc<Mutex<FftProcessor>>,
    visualizer_tx: Sender<Vec<f32>>,
    visualizer_enabled: Arc<Mutex<bool>>,
    volume: f32,
) -> anyhow::Result<(Sink, u64)> {
    let (channels, sample_rate, samples) = decode_m4a_bytes(bytes)?;
    let duration = if channels == 0 || sample_rate == 0 {
        0
    } else {
        samples.len() as u64 / channels as u64 / sample_rate as u64
    };
    sink_from_source(
        handle,
        SamplesBuffer::new(channels, sample_rate, samples),
        duration,
        fft_processor,
        visualizer_tx,
        visualizer_enabled,
        volume,
    )
}

fn prepare_track_sink(
    handle: &OutputStreamHandle,
    track: &Track,
    sc_client: &Arc<Mutex<SoundCloudClient>>,
    fft_processor: Arc<Mutex<FftProcessor>>,
    visualizer_tx: Sender<Vec<f32>>,
    visualizer_enabled: Arc<Mutex<bool>>,
    volume: f32,
) -> anyhow::Result<(Sink, u64)> {
    if let Some(path) = track.path.as_ref() {
        let path_str = path.to_string_lossy().to_string();
        sink_from_local_file(
            handle,
            &path_str,
            fft_processor,
            visualizer_tx,
            visualizer_enabled,
            volume,
        )
    } else if track.is_sc() {
        sink_from_remote_track(
            handle,
            track,
            sc_client,
            fft_processor,
            visualizer_tx,
            visualizer_enabled,
            volume,
        )
    } else {
        Err(anyhow::anyhow!("Playback is not available for this track"))
    }
}

fn fill_autoplay_queue(core: &Core, sc_client: &Arc<Mutex<SoundCloudClient>>, seed: &Track) -> bool {
    if !seed.is_playable_remote() {
        return false;
    }

    let existing = core
        .q_ids()
        .into_iter()
        .chain(core.hist_ids())
        .collect::<std::collections::HashSet<_>>();

    let mut client = match sc_client.lock() {
        Ok(client) => client,
        Err(_) => return false,
    };
    let Ok(results) = client.watch_next(seed, 8) else {
        return false;
    };

    let picks = results
        .into_iter()
        .filter(|track| track.id != seed.id)
        .filter(|track| !existing.contains(&track.id))
        .filter(|track| track.is_playable_remote())
        .take(5)
        .collect::<Vec<_>>();

    if picks.is_empty() {
        return false;
    }

    core.put_tracks(picks.clone());
    for track in picks {
        core.enqueue(track.id);
    }
    true
}

pub fn start_audio_thread(
    core: Core,
    sc_client: Arc<Mutex<SoundCloudClient>>,
    autoplay_initial: bool,
) -> PlaybackHandle {
    let (tx, rx): (Sender<PlaybackCommand>, Receiver<PlaybackCommand>) = mpsc::channel();
    let tx_clone = tx.clone();
    let position = Arc::new(Mutex::new(None));
    let position_clone = position.clone();

    let (autoplay_tx, autoplay_rx): (Sender<bool>, Receiver<bool>) = mpsc::channel();
    let (volume_tx, volume_rx): (Sender<f32>, Receiver<f32>) = mpsc::channel();
    let (visualizer_tx, visualizer_rx): (Sender<Vec<f32>>, Receiver<Vec<f32>>) = mpsc::channel();
    let (devices_tx, devices_rx) = mpsc::channel::<Vec<(String, bool)>>();
    let (msg_tx, msg_rx) = mpsc::channel::<String>();

    thread::spawn(move || {
        let host = rodio::cpal::default_host();
        let mut _stream: Option<OutputStream> = None;
        let mut stream_handle: Option<OutputStreamHandle> = None;

        if let Ok((stream, handle)) = OutputStream::try_default() {
            _stream = Some(stream);
            stream_handle = Some(handle);
        }

        let mut sink: Option<Sink> = None;
        let mut current_track_id: Option<String> = None;
        let mut current_duration: u64 = 0;
        let mut volume = 1.0f32;
        let mut autoplay = autoplay_initial;
        let mut elapsed_before_pause: u64 = 0;
        let mut playback_start: Option<Instant> = None;
        let mut is_paused = false;
        let mut current_device_name: Option<String> = None;
        let mut last_finished_track_id: Option<String> = None;

        if let Some(ref device) = host.default_output_device() {
            if let Ok(name) = device.name() {
                current_device_name = Some(name);
            }
        }

        let fft_processor = Arc::new(Mutex::new(FftProcessor::new(2048)));
        let visualizer_enabled = Arc::new(Mutex::new(false));

        autoplay_tx.send(autoplay).ok();
        volume_tx.send(volume).ok();

        loop {
            let mut current_pos = if let Some(ref _s) = sink {
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
            if current_duration > 0 {
                current_pos = current_pos.min(current_duration);
            }

            let playback_finished = sink
                .as_ref()
                .map(|s| !s.is_paused() && s.empty())
                .unwrap_or(false);
            let is_playing = sink
                .as_ref()
                .map(|s| !s.is_paused() && !s.empty())
                .unwrap_or(false);
            if playback_finished && current_duration > 0 {
                current_pos = current_duration;
                elapsed_before_pause = current_duration;
            }
            *position_clone.lock().unwrap() = Some((current_pos, current_duration, is_playing));

            if playback_finished {
                if let Some(current_id) = current_track_id.clone() {
                    if last_finished_track_id.as_deref() != Some(current_id.as_str()) {
                        if core.q_ids().is_empty() && autoplay {
                            if let Some(seed) = core.track(&current_id) {
                                fill_autoplay_queue(&core, &sc_client, &seed);
                            }
                        }
                        if let Some(next_id) = core.dequeue() {
                            last_finished_track_id = Some(current_id);
                            tx_clone.send(PlaybackCommand::PlayTrack(next_id)).ok();
                            continue;
                        }
                        last_finished_track_id = Some(current_id);
                    }
                }
            }

            if let Ok(cmd) = rx.recv_timeout(Duration::from_millis(50)) {
                match cmd {
                    PlaybackCommand::PlayIndex(idx) => {
                        let track = core.tracks.lock().unwrap().get(idx).cloned();
                        if let Some(track) = track {
                            let handle = stream_handle.as_ref().expect("No audio output available");

                            sink = None;
                            if track.is_sc() {
                                msg_tx.send("Buffering YouTube audio...".to_string()).ok();
                            }
                            match prepare_track_sink(
                                handle,
                                &track,
                                &sc_client,
                                fft_processor.clone(),
                                visualizer_tx.clone(),
                                visualizer_enabled.clone(),
                                volume,
                            ) {
                                Ok((new_sink, duration)) => {
                                    last_finished_track_id = None;
                                    current_duration = duration;
                                    current_track_id = Some(track.id.clone());
                                    core.set_cur(Some(track.id.clone()));
                                    core.add_hist(track.id.clone());
                                    sink = Some(new_sink);
                                    is_paused = false;
                                    elapsed_before_pause = 0;
                                    playback_start = Some(Instant::now());
                                    prefetch_next_remote_track(&core, &sc_client);
                                }
                                Err(e) => {
                                    last_finished_track_id = None;
                                    current_track_id = None;
                                    core.set_cur(None);
                                    msg_tx.send(e.to_string()).ok();
                                }
                            }
                        }
                    }
                    PlaybackCommand::PlayTrack(id) => {
                        if let Some(track) = core.track(&id) {
                            let handle = stream_handle.as_ref().expect("No audio output available");

                            sink = None;
                            if track.is_sc() {
                                msg_tx.send("Buffering YouTube audio...".to_string()).ok();
                            }
                            match prepare_track_sink(
                                handle,
                                &track,
                                &sc_client,
                                fft_processor.clone(),
                                visualizer_tx.clone(),
                                visualizer_enabled.clone(),
                                volume,
                            ) {
                                Ok((new_sink, duration)) => {
                                    last_finished_track_id = None;
                                    current_duration = duration;
                                    current_track_id = Some(id.clone());
                                    core.set_cur(Some(id.clone()));
                                    core.add_hist(id);
                                    sink = Some(new_sink);
                                    is_paused = false;
                                    elapsed_before_pause = 0;
                                    playback_start = Some(Instant::now());
                                    prefetch_next_remote_track(&core, &sc_client);
                                }
                                Err(e) => {
                                    last_finished_track_id = None;
                                    current_track_id = None;
                                    core.set_cur(None);
                                    msg_tx.send(e.to_string()).ok();
                                }
                            }
                        } else {
                            msg_tx
                                .send("The selected track is no longer available".to_string())
                                .ok();
                        }
                    }
                    PlaybackCommand::PlayFile(path) => {
                        sink = None;
                        current_track_id = None;
                        core.set_cur(None);
                        last_finished_track_id = None;

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
                                last_finished_track_id = None;
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
                        current_track_id = None;
                        core.set_cur(None);
                        is_paused = false;
                        elapsed_before_pause = 0;
                        current_duration = 0;
                        last_finished_track_id = None;
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
                        let mut next_id = core.dequeue();
                        if next_id.is_none() && autoplay {
                            if let Some(seed) = current_track_id.clone().and_then(|id| core.track(&id)) {
                                if fill_autoplay_queue(&core, &sc_client, &seed) {
                                    next_id = core.dequeue();
                                }
                            }
                        }
                        if let Some(next_id) = next_id {
                            last_finished_track_id = None;
                            tx_clone.send(PlaybackCommand::PlayTrack(next_id)).ok();
                        }
                    }
                    PlaybackCommand::Prev => {
                        if let Some(current_id) = current_track_id.clone() {
                            if current_pos > 3 {
                                tx_clone.send(PlaybackCommand::PlayTrack(current_id)).ok();
                            } else if let Some(prev_id) = core.prev_hist(Some(&current_id)) {
                                tx_clone.send(PlaybackCommand::PlayTrack(prev_id)).ok();
                            }
                        }
                    }
                    PlaybackCommand::ToggleVisualizer => {
                        let mut enabled = visualizer_enabled.lock().unwrap();
                        *enabled = !*enabled;
                    }
                    PlaybackCommand::ToggleAutoplay => {
                        autoplay = !autoplay;
                        autoplay_tx.send(autoplay).ok();
                        msg_tx
                            .send(format!(
                                "autoplay {}",
                                if autoplay { "enabled" } else { "disabled" }
                            ))
                            .ok();
                    }
                    PlaybackCommand::SetAutoplay(enabled) => {
                        autoplay = enabled;
                        autoplay_tx.send(autoplay).ok();
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
                        let saved_track_id = current_track_id.clone();
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

                                                if let Some(id) = saved_track_id.clone() {
                                                    if let Some(track) = core.track(&id) {
                                                        let handle =
                                                            stream_handle.as_ref().unwrap();
                                                        if let Ok((new_sink, duration)) =
                                                            prepare_track_sink(
                                                                handle,
                                                                &track,
                                                                &sc_client,
                                                                fft_processor.clone(),
                                                                visualizer_tx.clone(),
                                                                visualizer_enabled.clone(),
                                                                volume,
                                                            )
                                                        {
                                                            current_duration = duration;

                                                            if saved_position > 0 {
                                                                new_sink
                                                                    .try_seek(Duration::from_secs(
                                                                        saved_position,
                                                                    ))
                                                                    .ok();
                                                            }

                                                            if !was_playing {
                                                                new_sink.pause();
                                                                is_paused = true;
                                                            } else {
                                                                is_paused = false;
                                                            }

                                                            sink = Some(new_sink);
                                                            current_track_id = Some(id);
                                                            elapsed_before_pause = saved_position;
                                                            playback_start = Some(Instant::now());
                                                            last_finished_track_id = None;
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
        autoplay_rx,
        volume_rx,
        visualizer_rx,
        msg_rx,
    }
}
