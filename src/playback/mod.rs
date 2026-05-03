use crate::core::track::Track;
use crate::core::Core;
use crate::media_controls::MediaSession;
use crate::sources::soundcloud::SoundCloudClient;
use rand::seq::SliceRandom;
use rodio::buffer::SamplesBuffer;
use rodio::cpal::traits::{DeviceTrait, HostTrait};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use rustfft::{algorithm::Radix4, Fft, FftDirection};
use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};
use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek};
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::OnceLock;
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

const AUTOPLAY_FETCH_LIMIT: usize = 24;
const AUTOPLAY_TARGET_QUEUE_LEN: usize = 12;
const AUTOPLAY_REFILL_THRESHOLD: usize = 4;

const FFMPEG_CHANNELS: u16 = 2;
const FFMPEG_SAMPLE_RATE: u32 = 48_000;

#[allow(dead_code)]
pub enum PlaybackCommand {
    PlayIndex(usize),
    PlayNow(String),
    PlayTrack(String),
    PlayCollection {
        ids: Vec<String>,
        start_index: usize,
        kind: CollectionKind,
    },
    PrefetchTrack(String),
    PlayFile(String),
    Pause,
    Resume,
    Stop,
    Enqueue(String),
    EnqueueMany(Vec<String>),
    ClearQueue,
    TogglePause,
    Next,
    Prev,
    Quit,
    VolumeUp,
    VolumeDown,
    ToggleMute,
    SetVolume(f32),
    SkipForward(u64),
    SkipBackward(u64),
    SeekTo(u64),
    ToggleVisualizer,
    ToggleAutoplay,
    ToggleRepeat,
    ToggleShuffle,
    SetAutoplay(bool),
    ListDevices,
    SwitchDevice(String),
    Prepared(Result<(Sink, u64), String>, String, u64),
}

impl Clone for PlaybackCommand {
    fn clone(&self) -> Self {
        match self {
            Self::PlayIndex(i) => Self::PlayIndex(*i),
            Self::PlayNow(s) => Self::PlayNow(s.clone()),
            Self::PlayTrack(s) => Self::PlayTrack(s.clone()),
            Self::PlayCollection { ids, start_index, kind } => Self::PlayCollection {
                ids: ids.clone(),
                start_index: *start_index,
                kind: *kind,
            },
            Self::PrefetchTrack(s) => Self::PrefetchTrack(s.clone()),
            Self::PlayFile(s) => Self::PlayFile(s.clone()),
            Self::Pause => Self::Pause,
            Self::Resume => Self::Resume,
            Self::Stop => Self::Stop,
            Self::Enqueue(s) => Self::Enqueue(s.clone()),
            Self::EnqueueMany(v) => Self::EnqueueMany(v.clone()),
            Self::ClearQueue => Self::ClearQueue,
            Self::TogglePause => Self::TogglePause,
            Self::Next => Self::Next,
            Self::Prev => Self::Prev,
            Self::Quit => Self::Quit,
            Self::VolumeUp => Self::VolumeUp,
            Self::VolumeDown => Self::VolumeDown,
            Self::ToggleMute => Self::ToggleMute,
            Self::SetVolume(v) => Self::SetVolume(*v),
            Self::SkipForward(v) => Self::SkipForward(*v),
            Self::SkipBackward(v) => Self::SkipBackward(*v),
            Self::SeekTo(v) => Self::SeekTo(*v),
            Self::ToggleVisualizer => Self::ToggleVisualizer,
            Self::ToggleAutoplay => Self::ToggleAutoplay,
            Self::ToggleRepeat => Self::ToggleRepeat,
            Self::ToggleShuffle => Self::ToggleShuffle,
            Self::SetAutoplay(v) => Self::SetAutoplay(*v),
            Self::ListDevices => Self::ListDevices,
            Self::SwitchDevice(s) => Self::SwitchDevice(s.clone()),
            Self::Prepared(_, _, _) => panic!("PlaybackCommand::Prepared cannot be cloned"),
        }
    }
}
impl std::fmt::Debug for PlaybackCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PlayIndex(i) => f.debug_tuple("PlayIndex").field(i).finish(),
            Self::PlayNow(s) => f.debug_tuple("PlayNow").field(s).finish(),
            Self::PlayTrack(s) => f.debug_tuple("PlayTrack").field(s).finish(),
            Self::PlayCollection { ids, start_index, kind } => f.debug_struct("PlayCollection")
                .field("ids_len", &ids.len())
                .field("start_index", start_index)
                .field("kind", kind)
                .finish(),
            Self::PrefetchTrack(s) => f.debug_tuple("PrefetchTrack").field(s).finish(),
            Self::PlayFile(s) => f.debug_tuple("PlayFile").field(s).finish(),
            Self::Pause => write!(f, "Pause"),
            Self::Resume => write!(f, "Resume"),
            Self::Stop => write!(f, "Stop"),
            Self::Enqueue(s) => f.debug_tuple("Enqueue").field(s).finish(),
            Self::EnqueueMany(v) => f.debug_tuple("EnqueueMany").field(&v.len()).finish(),
            Self::ClearQueue => write!(f, "ClearQueue"),
            Self::TogglePause => write!(f, "TogglePause"),
            Self::Next => write!(f, "Next"),
            Self::Prev => write!(f, "Prev"),
            Self::Quit => write!(f, "Quit"),
            Self::VolumeUp => write!(f, "VolumeUp"),
            Self::VolumeDown => write!(f, "VolumeDown"),
            Self::ToggleMute => write!(f, "ToggleMute"),
            Self::SetVolume(v) => f.debug_tuple("SetVolume").field(v).finish(),
            Self::SkipForward(v) => f.debug_tuple("SkipForward").field(v).finish(),
            Self::SkipBackward(v) => f.debug_tuple("SkipBackward").field(v).finish(),
            Self::SeekTo(v) => f.debug_tuple("SeekTo").field(v).finish(),
            Self::ToggleVisualizer => write!(f, "ToggleVisualizer"),
            Self::ToggleAutoplay => write!(f, "ToggleAutoplay"),
            Self::ToggleRepeat => write!(f, "ToggleRepeat"),
            Self::ToggleShuffle => write!(f, "ToggleShuffle"),
            Self::SetAutoplay(v) => f.debug_tuple("SetAutoplay").field(v).finish(),
            Self::ListDevices => write!(f, "ListDevices"),
            Self::SwitchDevice(s) => f.debug_tuple("SwitchDevice").field(s).finish(),
            Self::Prepared(res, id, offset) => f.debug_tuple("Prepared")
                .field(&res.as_ref().map(|(_, d)| format!("Ok(Sink, {d})")).map_err(|e| e.clone()))
                .field(id)
                .field(offset)
                .finish(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionKind {
    Playlist,
    Album,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepeatMode {
    Off,
    All,
    One,
}

impl RepeatMode {
    fn cycle(self) -> Self {
        match self {
            Self::Off => Self::All,
            Self::All => Self::One,
            Self::One => Self::Off,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "repeat off",
            Self::All => "repeat all",
            Self::One => "repeat one",
        }
    }
}

pub struct PlaybackHandle {
    pub tx: Sender<PlaybackCommand>,
    pub position_rx: Arc<Mutex<Option<(u64, u64, bool)>>>,
    pub devices_rx: Receiver<Vec<(String, bool)>>,
    pub autoplay_rx: Receiver<bool>,
    pub repeat_rx: Receiver<RepeatMode>,
    pub shuffle_rx: Receiver<bool>,
    pub volume_rx: Receiver<f32>,
    pub visualizer_rx: Receiver<Vec<f32>>,
    pub msg_rx: Receiver<String>,
}

#[derive(Debug, Clone)]
struct ActiveCollection {
    all_ids: Vec<String>,
    kind: CollectionKind,
    cycle_scope: Vec<String>,
    cycle_order: Vec<String>,
}

impl ActiveCollection {
    fn new(
        ids: Vec<String>,
        start_index: usize,
        kind: CollectionKind,
        shuffle: bool,
    ) -> Option<Self> {
        if ids.is_empty() || start_index >= ids.len() {
            return None;
        }

        let cycle_scope = ids[start_index..].to_vec();
        let cycle_order = collection_order_from_scope(&cycle_scope, kind, shuffle);
        Some(Self {
            all_ids: ids,
            kind,
            cycle_scope,
            cycle_order,
        })
    }

    fn contains(&self, id: &str) -> bool {
        self.cycle_order.iter().any(|candidate| candidate == id)
    }

    fn is_playlist(&self) -> bool {
        self.kind == CollectionKind::Playlist
    }

    fn current_id(&self) -> Option<&str> {
        self.cycle_order.first().map(String::as_str)
    }

    fn set_current(&mut self, current_id: &str) -> bool {
        let order_pos = match self
            .cycle_order
            .iter()
            .position(|candidate| candidate == current_id)
        {
            Some(pos) => pos,
            None => return false,
        };
        if order_pos > 0 {
            self.cycle_order = self.cycle_order[order_pos..].to_vec();
        }

        if self.cycle_scope.first().map(String::as_str) == Some(current_id) {
            return true;
        }

        let mut next_scope = vec![current_id.to_string()];
        next_scope.extend(
            self.cycle_scope
                .iter()
                .skip(1)
                .filter(|candidate| candidate.as_str() != current_id)
                .cloned(),
        );
        self.cycle_scope = next_scope;
        true
    }

    fn rebuild_order(&mut self, shuffle: bool) {
        self.cycle_order = collection_order_from_scope(&self.cycle_scope, self.kind, shuffle);
    }

    fn restart_cycle(&mut self, shuffle: bool) {
        self.cycle_scope = self.all_ids.clone();
        self.rebuild_order(shuffle);
    }
}

fn collection_order_from_scope(
    scope: &[String],
    _kind: CollectionKind,
    shuffle: bool,
) -> Vec<String> {
    let Some((current, rest)) = scope.split_first() else {
        return Vec::new();
    };

    let mut order = vec![current.clone()];
    let mut remaining = rest.to_vec();
    if shuffle && remaining.len() > 1 {
        remaining.shuffle(&mut rand::thread_rng());
    }
    order.extend(remaining);
    order
}

fn sync_collection_queue(core: &Core, collection: &ActiveCollection, extras: &[String]) {
    core.clear_queue();
    for id in collection.cycle_order.iter().skip(1) {
        core.enqueue(id.clone());
    }
    for id in extras {
        core.enqueue(id.clone());
    }
}

fn shuffle_queue(core: &Core) -> bool {
    let mut ids = core.q_ids();
    if ids.len() < 2 {
        return false;
    }
    ids.shuffle(&mut rand::thread_rng());
    core.set_queue(ids);
    true
}

fn non_collection_queue_ids(core: &Core, collection: &ActiveCollection) -> Vec<String> {
    let collection_ids = collection
        .all_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    core.q_ids()
        .into_iter()
        .filter(|id| !collection_ids.contains(id.as_str()))
        .collect()
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

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(msg) = payload.downcast_ref::<&str>() {
        (*msg).to_string()
    } else if let Some(msg) = payload.downcast_ref::<String>() {
        msg.clone()
    } else {
        "unknown decoder panic".to_string()
    }
}

fn track_label(label: &str) -> String {
    Path::new(label)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(label)
        .to_string()
}

fn catch_decoder_init<T, F>(f: F) -> Result<T, Box<dyn std::any::Any + Send>>
where
    F: FnOnce() -> T,
{
    static DECODER_PANIC_GUARD: OnceLock<Mutex<()>> = OnceLock::new();
    let guard = DECODER_PANIC_GUARD.get_or_init(|| Mutex::new(())).lock();

    let previous_hook = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));
    let result = panic::catch_unwind(AssertUnwindSafe(f));
    panic::set_hook(previous_hook);

    if let Ok(guard) = guard {
        drop(guard);
    }

    result
}

fn decoder_from_reader<R>(reader: R, label: &str) -> anyhow::Result<Decoder<BufReader<R>>>
where
    R: Read + Seek + Send + Sync + 'static,
{
    let shown = track_label(label);
    let decoder =
        catch_decoder_init(|| Decoder::new(BufReader::new(reader))).map_err(|payload| {
            anyhow::anyhow!(
                "Decoder crashed while opening {}: {}",
                shown,
                panic_message(payload)
            )
        })?;

    decoder.map_err(|err| anyhow::anyhow!("Could not decode {}: {}", shown, err))
}

fn sink_from_reader<R>(
    handle: &OutputStreamHandle,
    reader: R,
    label: &str,
    fft_processor: Arc<Mutex<FftProcessor>>,
    visualizer_tx: Sender<Vec<f32>>,
    visualizer_enabled: Arc<Mutex<bool>>,
    volume: f32,
) -> anyhow::Result<(Sink, u64)>
where
    R: Read + Seek + Send + Sync + 'static,
{
    let source = decoder_from_reader(reader, label)?;
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
        path,
        fft_processor,
        visualizer_tx,
        visualizer_enabled,
        volume,
    )
}

#[allow(dead_code)]
fn sink_from_remote_track(
    handle: &OutputStreamHandle,
    track: &Track,
    sc_client: &Arc<Mutex<SoundCloudClient>>,
    fft_processor: Arc<Mutex<FftProcessor>>,
    visualizer_tx: Sender<Vec<f32>>,
    visualizer_enabled: Arc<Mutex<bool>>,
    volume: f32,
) -> anyhow::Result<(Sink, u64)> {
    sink_from_remote_track_at(
        handle,
        track,
        sc_client,
        fft_processor,
        visualizer_tx,
        visualizer_enabled,
        volume,
        0,
    )
}

fn sink_from_remote_track_at(
    handle: &OutputStreamHandle,
    track: &Track,
    sc_client: &Arc<Mutex<SoundCloudClient>>,
    fft_processor: Arc<Mutex<FftProcessor>>,
    visualizer_tx: Sender<Vec<f32>>,
    visualizer_enabled: Arc<Mutex<bool>>,
    volume: f32,
    start_at_secs: u64,
) -> anyhow::Result<(Sink, u64)> {
    let mut fallback_duration = track.dur.unwrap_or(0);
    let cached = {
        let mut client = sc_client.lock().unwrap();
        client.take_cached_audio(&track.id)
    };

    if let Some(bytes) = cached {
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
                "cached streamed audio",
                fft_processor,
                visualizer_tx,
                visualizer_enabled,
                volume,
            )?
        };

        if start_at_secs > 0 {
            let _ = sink.try_seek(Duration::from_secs(start_at_secs));
        }

        return Ok((
            sink,
            if duration == 0 {
                fallback_duration
            } else {
                duration
            },
        ));
    }

    let stream_attempt = {
        let mut client = sc_client
            .lock()
            .map_err(|_| anyhow::anyhow!("The YouTube client is unavailable"))?;
        let stream = client.stream(track)?;
        fallback_duration = stream.duration_secs.unwrap_or(fallback_duration);
        let headers = client.ffmpeg_headers();
        (stream.url, headers)
    };

    match sink_from_ffmpeg_stream(
        handle,
        &stream_attempt.0,
        stream_attempt.1,
        fallback_duration,
        start_at_secs,
        fft_processor.clone(),
        visualizer_tx.clone(),
        visualizer_enabled.clone(),
        volume,
    ) {
        Ok((sink, duration)) => {
            return Ok((
                sink,
                if duration == 0 {
                    fallback_duration
                } else {
                    duration
                },
            ));
        }
        Err(err) => {
            eprintln!(
                "FFmpeg streaming failed; falling back to full download playback: {}",
                err
            );
        }
    }

    let bytes = {
        let stream = {
            let mut client = sc_client
                .lock()
                .map_err(|_| anyhow::anyhow!("The YouTube client is unavailable"))?;
            client.stream(track)?
        };

        let client = sc_client
            .lock()
            .map_err(|_| anyhow::anyhow!("The YouTube client is unavailable"))?
            .clone();

        match client.download_stream(&stream.url) {
            Ok(bytes) => bytes,
            Err(_) => {
                {
                    let mut shared = sc_client
                        .lock()
                        .map_err(|_| anyhow::anyhow!("The YouTube client is unavailable"))?;
                    shared.invalidate_stream(&track.id);
                }
                let retry = {
                    let mut shared = sc_client
                        .lock()
                        .map_err(|_| anyhow::anyhow!("The YouTube client is unavailable"))?;
                    shared.stream(track)?
                };
                let retry_client = sc_client
                    .lock()
                    .map_err(|_| anyhow::anyhow!("The YouTube client is unavailable"))?
                    .clone();
                retry_client.download_stream(&retry.url)?
            }
        }
    };

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
            &format!("downloaded stream {}", track.title),
            fft_processor,
            visualizer_tx,
            visualizer_enabled,
            volume,
        )?
    };

    if start_at_secs > 0 {
        let _ = sink.try_seek(Duration::from_secs(start_at_secs));
    }

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
    thread::spawn(move || prefetch_remote_track_bytes(&sc_client, track));
}

fn prefetch_remote_track_bytes(sc_client: &Arc<Mutex<SoundCloudClient>>, track: Track) {
    if let Ok(mut client) = sc_client.lock() {
        let _ = client.stream(&track);
    }
}

#[allow(clippy::too_many_arguments)]
fn seek_current_sink(
    sink: &mut Option<Sink>,
    stream_handle: Option<&OutputStreamHandle>,
    current_track_id: &Option<String>,
    core: &Core,
    sc_client: &Arc<Mutex<SoundCloudClient>>,
    fft_processor: Arc<Mutex<FftProcessor>>,
    visualizer_tx: Sender<Vec<f32>>,
    visualizer_enabled: Arc<Mutex<bool>>,
    volume: f32,
    target_secs: u64,
    current_duration: &mut u64,
    elapsed_before_pause: &mut u64,
    playback_start: &mut Option<Instant>,
    is_paused: bool,
) -> anyhow::Result<()> {
    let target_secs = if *current_duration > 0 {
        target_secs.min(*current_duration)
    } else {
        target_secs
    };

    let current_track = current_track_id.as_ref().and_then(|id| core.track(id));

    if let Some(track) = current_track.as_ref().filter(|track| track.is_sc()) {
        let handle = stream_handle
            .ok_or_else(|| anyhow::anyhow!("No audio output is available for seeking"))?;

        *sink = None;

        let (new_sink, duration) = prepare_track_sink_at(
            handle,
            track,
            sc_client,
            fft_processor,
            visualizer_tx,
            visualizer_enabled,
            volume,
            target_secs,
        )?;

        if is_paused {
            new_sink.pause();
            *playback_start = None;
        } else {
            new_sink.play();
            *playback_start = Some(Instant::now());
        }

        *current_duration = duration;
        *elapsed_before_pause = target_secs;
        *sink = Some(new_sink);

        return Ok(());
    }

    let Some(active_sink) = sink.as_ref() else {
        return Ok(());
    };

    active_sink
        .try_seek(Duration::from_secs(target_secs))
        .map_err(|err| anyhow::anyhow!("Seek failed: {:?}", err))?;

    *elapsed_before_pause = target_secs;
    *playback_start = if is_paused {
        None
    } else {
        Some(Instant::now())
    };

    Ok(())
}

fn restart_sink_in_place(
    sink: &Option<Sink>,
    elapsed_before_pause: &mut u64,
    playback_start: &mut Option<Instant>,
    is_paused: &mut bool,
) -> bool {
    let Some(sink) = sink.as_ref() else {
        return false;
    };

    if sink.try_seek(Duration::ZERO).is_err() {
        return false;
    }

    *elapsed_before_pause = 0;
    *playback_start = Some(Instant::now());
    *is_paused = false;
    sink.play();
    true
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

struct FfmpegPcmSource {
    child: Child,
    stdout: ChildStdout,
    prebuffer: VecDeque<f32>,
    duration: Option<Duration>,
}

impl FfmpegPcmSource {
    fn new(
        url: &str,
        headers: &[(String, String)],
        duration_secs: u64,
        start_at_secs: u64,
    ) -> anyhow::Result<Self> {
        let mut cmd = Command::new("ffmpeg");

        cmd.arg("-nostdin")
            .arg("-loglevel")
            .arg("error")
            .arg("-reconnect")
            .arg("1")
            .arg("-reconnect_streamed")
            .arg("1")
            .arg("-reconnect_delay_max")
            .arg("5");

        let header_arg = ffmpeg_header_arg(headers);
        if !header_arg.trim().is_empty() {
            cmd.arg("-headers").arg(header_arg);
        }

        if start_at_secs > 0 {
            cmd.arg("-ss").arg(start_at_secs.to_string());
        }

        cmd.arg("-i")
            .arg(url)
            .arg("-vn")
            .arg("-f")
            .arg("f32le")
            .arg("-acodec")
            .arg("pcm_f32le")
            .arg("-ac")
            .arg(FFMPEG_CHANNELS.to_string())
            .arg("-ar")
            .arg(FFMPEG_SAMPLE_RATE.to_string())
            .arg("pipe:1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = cmd.spawn().map_err(|err| {
            anyhow::anyhow!("Could not launch ffmpeg for streaming playback: {err}")
        })?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("ffmpeg stdout pipe was not available"))?;

        let mut source = Self {
            child,
            stdout,
            prebuffer: VecDeque::new(),
            duration: (duration_secs > 0).then(|| Duration::from_secs(duration_secs)),
        };

        let first_sample = Self::read_stdout_sample(&mut source.stdout).ok_or_else(|| {
            anyhow::anyhow!(
                "ffmpeg started, but did not produce audio. The stream URL may have expired or been rejected."
            )
        })?;

        source.prebuffer.push_back(first_sample);
        Ok(source)
    }

    fn read_stdout_sample(stdout: &mut ChildStdout) -> Option<f32> {
        let mut bytes = [0u8; 4];
        stdout.read_exact(&mut bytes).ok()?;
        Some(f32::from_le_bytes(bytes))
    }
}

impl Iterator for FfmpegPcmSource {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(sample) = self.prebuffer.pop_front() {
            return Some(sample);
        }

        Self::read_stdout_sample(&mut self.stdout)
    }
}

impl Source for FfmpegPcmSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> u16 {
        FFMPEG_CHANNELS
    }

    fn sample_rate(&self) -> u32 {
        FFMPEG_SAMPLE_RATE
    }

    fn total_duration(&self) -> Option<Duration> {
        self.duration
    }
}

impl Drop for FfmpegPcmSource {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn ffmpeg_header_arg(headers: &[(String, String)]) -> String {
    let mut out = String::new();

    for (name, value) in headers {
        let name = name.trim();
        if name.is_empty() || name.contains(':') {
            continue;
        }

        let value = value.replace('\r', "").replace('\n', "");
        if value.trim().is_empty() {
            continue;
        }

        out.push_str(name);
        out.push_str(": ");
        out.push_str(&value);
        out.push_str("\r\n");
    }

    out
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

fn sink_from_ffmpeg_stream(
    handle: &OutputStreamHandle,
    url: &str,
    headers: Vec<(String, String)>,
    duration: u64,
    start_at_secs: u64,
    fft_processor: Arc<Mutex<FftProcessor>>,
    visualizer_tx: Sender<Vec<f32>>,
    visualizer_enabled: Arc<Mutex<bool>>,
    volume: f32,
) -> anyhow::Result<(Sink, u64)> {
    let source = FfmpegPcmSource::new(url, &headers, duration, start_at_secs)?;
    sink_from_source(
        handle,
        source,
        duration,
        fft_processor,
        visualizer_tx,
        visualizer_enabled,
        volume,
    )
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
    prepare_track_sink_at(
        handle,
        track,
        sc_client,
        fft_processor,
        visualizer_tx,
        visualizer_enabled,
        volume,
        0,
    )
}

#[allow(clippy::too_many_arguments)]
fn prepare_track_sink_at(
    handle: &OutputStreamHandle,
    track: &Track,
    sc_client: &Arc<Mutex<SoundCloudClient>>,
    fft_processor: Arc<Mutex<FftProcessor>>,
    visualizer_tx: Sender<Vec<f32>>,
    visualizer_enabled: Arc<Mutex<bool>>,
    volume: f32,
    start_at_secs: u64,
) -> anyhow::Result<(Sink, u64)> {
    if let Some(path) = track.path.as_ref() {
        let path_str = path.to_string_lossy().to_string();
        let (sink, duration) = sink_from_local_file(
            handle,
            &path_str,
            fft_processor,
            visualizer_tx,
            visualizer_enabled,
            volume,
        )?;

        if start_at_secs > 0 {
            sink.try_seek(Duration::from_secs(start_at_secs))
                .map_err(|err| {
                    anyhow::anyhow!("Seek failed after opening local file: {:?}", err)
                })?;
        }

        Ok((sink, duration))
    } else if track.is_sc() {
        sink_from_remote_track_at(
            handle,
            track,
            sc_client,
            fft_processor,
            visualizer_tx,
            visualizer_enabled,
            volume,
            start_at_secs,
        )
    } else {
        Err(anyhow::anyhow!("Playback is not available for this track"))
    }
}

fn audio_device_name(device: &rodio::cpal::Device) -> Option<String> {
    device
        .name()
        .ok()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
}

fn list_output_device_names(host: &rodio::cpal::Host) -> Vec<String> {
    let mut names = Vec::new();

    if let Ok(devices) = host.output_devices() {
        for device in devices {
            if let Some(name) = audio_device_name(&device) {
                names.push(name);
            }
        }
    }

    names
}

fn open_output_stream_for_device(
    device: &rodio::cpal::Device,
) -> anyhow::Result<(OutputStream, OutputStreamHandle, String)> {
    let name = audio_device_name(device)
        .ok_or_else(|| anyhow::anyhow!("Could not determine the audio device name"))?;
    let (stream, handle) = OutputStream::try_from_device(device)
        .map_err(|err| anyhow::anyhow!("Could not open audio device {}: {}", name, err))?;
    Ok((stream, handle, name))
}

fn open_named_output_stream(
    host: &rodio::cpal::Host,
    device_name: &str,
) -> anyhow::Result<(OutputStream, OutputStreamHandle, String)> {
    let devices = host
        .output_devices()
        .map_err(|err| anyhow::anyhow!("Could not enumerate audio devices: {}", err))?;

    for device in devices {
        if audio_device_name(&device).as_deref() == Some(device_name) {
            return open_output_stream_for_device(&device);
        }
    }

    Err(anyhow::anyhow!(
        "Audio device {} is no longer available",
        device_name
    ))
}

fn open_default_output_stream(
    host: &rodio::cpal::Host,
) -> anyhow::Result<(OutputStream, OutputStreamHandle, String)> {
    let default_name = host
        .default_output_device()
        .and_then(|device| audio_device_name(&device));
    let mut last_error = None::<String>;

    if let Some(device) = host.default_output_device() {
        match open_output_stream_for_device(&device) {
            Ok(stream) => return Ok(stream),
            Err(err) => last_error = Some(err.to_string()),
        }
    }

    if let Ok(devices) = host.output_devices() {
        for device in devices {
            let Some(name) = audio_device_name(&device) else {
                continue;
            };
            if default_name.as_ref() == Some(&name) {
                continue;
            }

            match OutputStream::try_from_device(&device) {
                Ok((stream, handle)) => return Ok((stream, handle, name)),
                Err(err) => {
                    last_error = Some(format!("Could not open audio device {}: {}", name, err));
                }
            }
        }
    }

    Err(anyhow::anyhow!(
        "{}",
        last_error.unwrap_or_else(|| "No audio output device is available".to_string())
    ))
}

fn restore_playback_on_handle(
    handle: &OutputStreamHandle,
    current_track_id: &Option<String>,
    current_file_path: &Option<String>,
    core: &Core,
    sc_client: &Arc<Mutex<SoundCloudClient>>,
    fft_processor: Arc<Mutex<FftProcessor>>,
    visualizer_tx: Sender<Vec<f32>>,
    visualizer_enabled: Arc<Mutex<bool>>,
    volume: f32,
    start_at_secs: u64,
) -> anyhow::Result<Option<(Sink, u64)>> {
    if let Some(track_id) = current_track_id.as_deref() {
        let track = core
            .track(track_id)
            .ok_or_else(|| anyhow::anyhow!("The selected track is no longer available"))?;

        return prepare_track_sink_at(
            handle,
            &track,
            sc_client,
            fft_processor,
            visualizer_tx,
            visualizer_enabled,
            volume,
            start_at_secs,
        )
        .map(Some);
    }

    if let Some(path) = current_file_path.as_deref() {
        let (sink, duration) = sink_from_local_file(
            handle,
            path,
            fft_processor,
            visualizer_tx,
            visualizer_enabled,
            volume,
        )?;

        if start_at_secs > 0 {
            let _ = sink.try_seek(Duration::from_secs(start_at_secs));
        }

        return Ok(Some((sink, duration)));
    }

    Ok(None)
}

struct RebuiltOutput {
    stream: OutputStream,
    handle: OutputStreamHandle,
    device_name: String,
    sink: Option<Sink>,
    duration: u64,
    restore_error: Option<String>,
}

#[allow(clippy::too_many_arguments)]
fn rebuild_output_stream(
    host: &rodio::cpal::Host,
    preferred_device_name: Option<&str>,
    allow_fallback_to_default: bool,
    core: &Core,
    sc_client: &Arc<Mutex<SoundCloudClient>>,
    current_track_id: &Option<String>,
    current_file_path: &Option<String>,
    saved_position: u64,
    was_playing: bool,
    fft_processor: Arc<Mutex<FftProcessor>>,
    visualizer_tx: Sender<Vec<f32>>,
    visualizer_enabled: Arc<Mutex<bool>>,
    volume: f32,
) -> anyhow::Result<RebuiltOutput> {
    let (stream, handle, device_name) = match preferred_device_name {
        Some(name) => match open_named_output_stream(host, name) {
            Ok(stream) => stream,
            Err(err) if allow_fallback_to_default => {
                let _ = err;
                open_default_output_stream(host)?
            }
            Err(err) => return Err(err),
        },
        None => open_default_output_stream(host)?,
    };

    let mut restore_error = None;
    let mut sink = None;
    let mut duration = 0;

    match restore_playback_on_handle(
        &handle,
        current_track_id,
        current_file_path,
        core,
        sc_client,
        fft_processor,
        visualizer_tx,
        visualizer_enabled,
        volume,
        saved_position,
    ) {
        Ok(Some((new_sink, new_duration))) => {
            if !was_playing {
                new_sink.pause();
            }

            duration = new_duration;
            sink = Some(new_sink);
        }
        Ok(None) => {}
        Err(err) => {
            restore_error = Some(err.to_string());
        }
    }

    Ok(RebuiltOutput {
        stream,
        handle,
        device_name,
        sink,
        duration,
        restore_error,
    })
}

fn fill_autoplay_queue(
    core: &Core,
    sc_client: &Arc<Mutex<SoundCloudClient>>,
    seed: &Track,
) -> bool {
    if !seed.is_playable_remote() {
        return false;
    }
    let queued = core.q_ids();
    if queued.len() >= AUTOPLAY_TARGET_QUEUE_LEN {
        return false;
    }

    let existing = core
        .q_ids()
        .into_iter()
        .chain(core.hist_ids())
        .chain(core.cur_id())
        .collect::<std::collections::HashSet<_>>();

    let mut client = match sc_client.lock() {
        Ok(client) => client.clone(),
        Err(_) => return false,
    };
    let Ok(results) = client.watch_next(seed, AUTOPLAY_FETCH_LIMIT) else {
        return false;
    };
    let needed = AUTOPLAY_TARGET_QUEUE_LEN.saturating_sub(queued.len());
    if needed == 0 {
        return false;
    }

    let picks = results
        .into_iter()
        .filter(|track| track.id != seed.id)
        .filter(|track| !existing.contains(&track.id))
        .filter(|track| track.is_playable_remote())
        .take(needed)
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

fn maybe_seed_autoplay_queue(
    core: &Core,
    sc_client: &Arc<Mutex<SoundCloudClient>>,
    track: &Track,
    autoplay: bool,
) {
    if !autoplay || !track.is_playable_remote() || core.q_ids().len() >= AUTOPLAY_REFILL_THRESHOLD {
        return;
    }

    let core = core.clone();
    let sc_client = Arc::clone(sc_client);
    let seed = track.clone();
    thread::spawn(move || {
        if fill_autoplay_queue(&core, &sc_client, &seed) {
            prefetch_next_remote_track(&core, &sc_client);
        }
    });
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
    let (repeat_tx, repeat_rx): (Sender<RepeatMode>, Receiver<RepeatMode>) = mpsc::channel();
    let (shuffle_tx, shuffle_rx): (Sender<bool>, Receiver<bool>) = mpsc::channel();
    let (volume_tx, volume_rx): (Sender<f32>, Receiver<f32>) = mpsc::channel();
    let (visualizer_tx, visualizer_rx): (Sender<Vec<f32>>, Receiver<Vec<f32>>) = mpsc::channel();
    let (devices_tx, devices_rx) = mpsc::channel::<Vec<(String, bool)>>();
    let (msg_tx, msg_rx) = mpsc::channel::<String>();

    thread::spawn(move || {
        let host = rodio::cpal::default_host();
        let mut _stream: Option<OutputStream> = None;
        let mut stream_handle: Option<OutputStreamHandle> = None;
        let mut current_device_name: Option<String> = None;

        if let Ok((stream, handle, device_name)) = open_default_output_stream(&host) {
            _stream = Some(stream);
            stream_handle = Some(handle);
            current_device_name = Some(device_name);
        }

        let mut sink: Option<Sink> = None;
        let mut current_track_id: Option<String> = None;
        let mut current_file_path: Option<String> = None;
        let mut current_duration: u64 = 0;
        let mut volume = 1.0f32;
        let mut autoplay = autoplay_initial;
        let mut repeat_mode = RepeatMode::Off;
        let mut shuffle_enabled = false;
        let mut active_collection = None::<ActiveCollection>;
        let mut queue_before_shuffle = None::<Vec<String>>;
        let mut elapsed_before_pause: u64 = 0;
        let mut playback_start: Option<Instant> = None;
        let mut is_paused = false;
        let mut last_finished_track_id: Option<String> = None;
        let mut preparing_track_id: Option<String> = None;
        let mut waiting_for_output_recovery = stream_handle.is_none();
        let mut last_device_check = Instant::now();

        let fft_processor = Arc::new(Mutex::new(FftProcessor::new(2048)));
        let visualizer_enabled = Arc::new(Mutex::new(false));

        let discord_rpc = crate::discord_rpc::init();
        let mut media_session = match MediaSession::new(tx_clone.clone()) {
            Ok(session) => session,
            Err(err) => {
                msg_tx
                    .send(format!("media controls unavailable: {}", err))
                    .ok();
                None
            }
        };

        autoplay_tx.send(autoplay).ok();
        repeat_tx.send(repeat_mode).ok();
        shuffle_tx.send(shuffle_enabled).ok();
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
            if let Some(session) = media_session.as_mut() {
                let track = current_track_id.as_ref().and_then(|id| core.track(id));
                session.sync(track.as_ref(), current_pos, is_playing, volume);
            }

            if last_device_check.elapsed() >= Duration::from_millis(750) {
                last_device_check = Instant::now();

                let available_devices = list_output_device_names(&host);
                let current_device_available = current_device_name
                    .as_ref()
                    .is_some_and(|name| available_devices.iter().any(|device| device == name));
                let needs_output_recovery = stream_handle.is_none()
                    || current_device_name.is_none()
                    || !current_device_available;

                if needs_output_recovery {
                    let previous_device_name = current_device_name.clone();
                    let saved_track_id = current_track_id.clone();
                    let saved_file_path = current_file_path.clone();
                    let saved_position = current_pos;
                    let was_playing = !is_paused
                        && (sink.is_some()
                            || saved_track_id.is_some()
                            || saved_file_path.is_some());

                    sink = None;
                    _stream = None;
                    stream_handle = None;
                    current_device_name = None;
                    elapsed_before_pause = saved_position;
                    playback_start = None;

                    match rebuild_output_stream(
                        &host,
                        None,
                        true,
                        &core,
                        &sc_client,
                        &saved_track_id,
                        &saved_file_path,
                        saved_position,
                        was_playing,
                        fft_processor.clone(),
                        visualizer_tx.clone(),
                        visualizer_enabled.clone(),
                        volume,
                    ) {
                        Ok(recovered) => {
                            _stream = Some(recovered.stream);
                            stream_handle = Some(recovered.handle);
                            current_device_name = Some(recovered.device_name.clone());
                            sink = recovered.sink;
                            if sink.is_some() {
                                current_duration = recovered.duration;
                                is_paused = !was_playing;
                                playback_start = Some(Instant::now());
                            }
                            waiting_for_output_recovery = false;

                            let recovery_msg = if let Some(old_name) = previous_device_name {
                                if old_name == recovered.device_name {
                                    format!("audio output restored on {}", recovered.device_name)
                                } else {
                                    format!(
                                        "audio device {} disconnected, switched to {}",
                                        old_name, recovered.device_name
                                    )
                                }
                            } else {
                                format!("audio output connected: {}", recovered.device_name)
                            };
                            msg_tx.send(recovery_msg).ok();

                            if let Some(err) = recovered.restore_error {
                                msg_tx.send(err).ok();
                            }
                        }
                        Err(err) => {
                            if !waiting_for_output_recovery {
                                msg_tx
                                    .send(format!(
                                        "audio output unavailable; waiting for another device ({})",
                                        err
                                    ))
                                    .ok();
                                waiting_for_output_recovery = true;
                            }
                        }
                    }
                } else {
                    waiting_for_output_recovery = false;
                }
            }

            if playback_finished {
                if let Some(current_id) = current_track_id.clone() {
                    if last_finished_track_id.as_deref() != Some(current_id.as_str()) {
                        if repeat_mode == RepeatMode::One {
                            last_finished_track_id = Some(current_id.clone());
                            tx_clone.send(PlaybackCommand::PlayTrack(current_id)).ok();
                            continue;
                        }
                        if core.q_ids().len() < AUTOPLAY_REFILL_THRESHOLD && autoplay {
                            if let Some(seed) = core.track(&current_id) {
                                fill_autoplay_queue(&core, &sc_client, &seed);
                            }
                        }
                        let mut next_id = core.dequeue();
                        if next_id.is_none() {
                            if let Some(collection) = active_collection.as_mut() {
                                if repeat_mode == RepeatMode::All {
                                    collection.restart_cycle(shuffle_enabled);
                                    sync_collection_queue(&core, collection, &[]);
                                    next_id = collection.current_id().map(ToOwned::to_owned);
                                }
                            } else if repeat_mode == RepeatMode::All {
                                next_id = Some(current_id.clone());
                            }
                        }
                        if let Some(next_id) = next_id {
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
                        let track = core.track_at(idx);
                        if let Some(track) = track {
                            active_collection = None;
                            if shuffle_enabled {
                                shuffle_enabled = false;
                                shuffle_tx.send(false).ok();
                            }
                            let Some(handle) = stream_handle.as_ref() else {
                                msg_tx
                                    .send(
                                        "No audio output available; waiting for another device"
                                            .to_string(),
                                    )
                                    .ok();
                                continue;
                            };

                            sink = None;
                            if track.is_sc() {
                                msg_tx.send("Buffering YouTube audio...".to_string()).ok();
                            }

                            preparing_track_id = Some(track.id.clone());
                            let tx = tx_clone.clone();
                            let sc_client = Arc::clone(&sc_client);
                            let handle = handle.clone();
                            let fft_processor = fft_processor.clone();
                            let visualizer_tx = visualizer_tx.clone();
                            let visualizer_enabled = visualizer_enabled.clone();
                            let track_clone = track.clone();

                            thread::spawn(move || {
                                let res = prepare_track_sink(
                                    &handle,
                                    &track_clone,
                                    &sc_client,
                                    fft_processor,
                                    visualizer_tx,
                                    visualizer_enabled,
                                    volume,
                                )
                                .map_err(|e| e.to_string());
                                let _ = tx.send(PlaybackCommand::Prepared(res, track_clone.id, 0));
                            });
                        }
                    }
                    PlaybackCommand::PlayTrack(id) => {
                        if let Some(track) = core.track(&id) {
                            let in_active_collection = active_collection
                                .as_ref()
                                .is_some_and(|collection| collection.contains(&id));
                            if let Some(collection) = active_collection.as_mut() {
                                if in_active_collection {
                                    collection.set_current(&id);
                                } else {
                                    active_collection = None;
                                }
                            }
                            if active_collection.is_none() && shuffle_enabled {
                                shuffle_enabled = false;
                                shuffle_tx.send(false).ok();
                            }
                            let Some(handle) = stream_handle.as_ref() else {
                                msg_tx
                                    .send(
                                        "No audio output available; waiting for another device"
                                            .to_string(),
                                    )
                                    .ok();
                                continue;
                            };

                            sink = None;
                            if track.is_sc() {
                                msg_tx.send("Buffering YouTube audio...".to_string()).ok();
                            }

                            preparing_track_id = Some(id.clone());
                            let tx = tx_clone.clone();
                            let sc_client = Arc::clone(&sc_client);
                            let handle = handle.clone();
                            let fft_processor = fft_processor.clone();
                            let visualizer_tx = visualizer_tx.clone();
                            let visualizer_enabled = visualizer_enabled.clone();
                            let track_clone = track.clone();

                            thread::spawn(move || {
                                let res = prepare_track_sink(
                                    &handle,
                                    &track_clone,
                                    &sc_client,
                                    fft_processor,
                                    visualizer_tx,
                                    visualizer_enabled,
                                    volume,
                                )
                                .map_err(|e| e.to_string());
                                let _ = tx.send(PlaybackCommand::Prepared(res, track_clone.id, 0));
                            });

                            if let Ok(mut rpc) = discord_rpc.lock() {
                                if let Some(rpc) = rpc.as_mut() {
                                    rpc.update(&track.title, track.artist.as_deref());
                                }
                            }
                        } else {
                            msg_tx
                                .send("The selected track is no longer available".to_string())
                                .ok();
                        }
                    }
                    PlaybackCommand::PlayNow(id) => {
                        core.clear_queue();
                        active_collection = None;
                        queue_before_shuffle = None;
                        if shuffle_enabled {
                            shuffle_enabled = false;
                            shuffle_tx.send(false).ok();
                        }
                        last_finished_track_id = None;
                        tx_clone.send(PlaybackCommand::PlayTrack(id)).ok();
                    }
                    PlaybackCommand::PrefetchTrack(id) => {
                        let Some(track) = core.track(&id) else {
                            continue;
                        };
                        if !track.is_sc() || current_track_id.as_deref() == Some(id.as_str()) {
                            continue;
                        }
                        let sc_client = Arc::clone(&sc_client);
                        thread::spawn(move || prefetch_remote_track_bytes(&sc_client, track));
                    }
                    PlaybackCommand::PlayCollection {
                        ids,
                        start_index,
                        kind,
                    } => {
                        let active =
                            match ActiveCollection::new(ids, start_index, kind, shuffle_enabled) {
                                Some(active) => active,
                                None => {
                                    msg_tx
                                        .send(
                                            "That collection does not contain any playable tracks"
                                                .to_string(),
                                        )
                                        .ok();
                                    continue;
                                }
                            };

                        if !active.is_playlist() && shuffle_enabled {
                            shuffle_enabled = false;
                            shuffle_tx.send(false).ok();
                        }

                        let Some(track_id) = active.current_id().map(ToOwned::to_owned) else {
                            msg_tx
                                .send(
                                    "That collection does not contain any playable tracks"
                                        .to_string(),
                                )
                                .ok();
                            continue;
                        };

                        sync_collection_queue(&core, &active, &[]);
                        active_collection = Some(active);
                        last_finished_track_id = None;
                        tx_clone.send(PlaybackCommand::PlayTrack(track_id)).ok();
                    }
                    PlaybackCommand::PlayFile(path) => {
                        sink = None;
                        current_track_id = None;
                        current_file_path = None;
                        core.set_cur(None);
                        last_finished_track_id = None;
                        active_collection = None;
                        if shuffle_enabled {
                            shuffle_enabled = false;
                            shuffle_tx.send(false).ok();
                        }

                        let Some(handle) = stream_handle.as_ref() else {
                            msg_tx
                                .send(
                                    "No audio output available; waiting for another device"
                                        .to_string(),
                                )
                                .ok();
                            continue;
                        };
                        match sink_from_local_file(
                            handle,
                            &path,
                            fft_processor.clone(),
                            visualizer_tx.clone(),
                            visualizer_enabled.clone(),
                            volume,
                        ) {
                            Ok((new_sink, duration)) => {
                                current_duration = duration;
                                current_file_path = Some(path.clone());
                                sink = Some(new_sink);
                                is_paused = false;
                                elapsed_before_pause = 0;
                                playback_start = Some(Instant::now());
                                last_finished_track_id = None;
                            }
                            Err(err) => {
                                current_duration = 0;
                                current_file_path = None;
                                elapsed_before_pause = 0;
                                playback_start = None;
                                msg_tx.send(err.to_string()).ok();
                            }
                        }
                    }
                    PlaybackCommand::Pause => {
                        preparing_track_id = None;
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
                    PlaybackCommand::TogglePause => {
                        if let Some(ref s) = sink {
                            if is_paused {
                                is_paused = false;
                                playback_start = Some(Instant::now());
                                s.play();
                            } else {
                                elapsed_before_pause = current_pos;
                                is_paused = true;
                                s.pause();
                            }
                        }
                    }
                    PlaybackCommand::Stop => {
                        preparing_track_id = None;
                        sink = None;
                        current_track_id = None;
                        current_file_path = None;
                        core.set_cur(None);
                        is_paused = false;
                        elapsed_before_pause = 0;
                        current_duration = 0;
                        last_finished_track_id = None;
                        active_collection = None;
                        if let Ok(mut rpc) = discord_rpc.lock() {
                            if let Some(rpc) = rpc.as_mut() {
                                rpc.clear();
                            }
                        }
                        if shuffle_enabled {
                            shuffle_enabled = false;
                            shuffle_tx.send(false).ok();
                        }
                    }
                    PlaybackCommand::Enqueue(id) => {
                        core.enqueue(id);
                    }
                    PlaybackCommand::EnqueueMany(ids) => {
                        for id in ids {
                            core.enqueue(id);
                        }
                    }
                    PlaybackCommand::ClearQueue => {
                        core.clear_queue();
                        queue_before_shuffle = None;
                        if shuffle_enabled && active_collection.is_none() {
                            shuffle_enabled = false;
                            shuffle_tx.send(false).ok();
                        }
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
                    PlaybackCommand::SetVolume(next_volume) => {
                        volume = next_volume.clamp(0.0, 1.0);
                        if let Some(ref s) = sink {
                            s.set_volume(volume);
                        }
                        volume_tx.send(volume).ok();
                    }
                    PlaybackCommand::SkipForward(secs) => {
                        let target = (current_pos + secs).min(current_duration);
                        tx_clone.send(PlaybackCommand::SeekTo(target)).ok();
                    }
                    PlaybackCommand::SeekTo(target) => {
                        let new_pos = if current_duration > 0 {
                            target.min(current_duration)
                        } else {
                            target
                        };

                        let current_track = current_track_id.as_ref().and_then(|id| core.track(id));
                        if let Some(track) = current_track.as_ref().filter(|track| track.is_sc()) {
                            let Some(handle) = stream_handle.as_ref() else {
                                msg_tx
                                    .send(
                                        "No audio output available; waiting for another device"
                                            .to_string(),
                                    )
                                    .ok();
                                continue;
                            };

                            sink = None;
                            msg_tx.send("Seeking YouTube audio...".to_string()).ok();

                            preparing_track_id = Some(track.id.clone());
                            let tx = tx_clone.clone();
                            let sc_client = Arc::clone(&sc_client);
                            let handle = handle.clone();
                            let fft_processor = fft_processor.clone();
                            let visualizer_tx = visualizer_tx.clone();
                            let visualizer_enabled = visualizer_enabled.clone();
                            let track_clone = track.clone();

                            thread::spawn(move || {
                                let res = prepare_track_sink_at(
                                    &handle,
                                    &track_clone,
                                    &sc_client,
                                    fft_processor,
                                    visualizer_tx,
                                    visualizer_enabled,
                                    volume,
                                    new_pos,
                                )
                                .map_err(|e| e.to_string());
                                let _ = tx.send(PlaybackCommand::Prepared(res, track_clone.id, new_pos));
                            });
                        } else {
                            if let Err(err) = seek_current_sink(
                                &mut sink,
                                stream_handle.as_ref(),
                                &current_track_id,
                                &core,
                                &sc_client,
                                fft_processor.clone(),
                                visualizer_tx.clone(),
                                visualizer_enabled.clone(),
                                volume,
                                new_pos,
                                &mut current_duration,
                                &mut elapsed_before_pause,
                                &mut playback_start,
                                is_paused,
                            ) {
                                msg_tx.send(err.to_string()).ok();
                            }
                        }
                    }
                    PlaybackCommand::SkipBackward(secs) => {
                        let target = current_pos.saturating_sub(secs);
                        tx_clone.send(PlaybackCommand::SeekTo(target)).ok();
                    }
                    PlaybackCommand::Next => {
                        if autoplay && core.q_ids().len() < AUTOPLAY_REFILL_THRESHOLD {
                            if let Some(seed) =
                                current_track_id.clone().and_then(|id| core.track(&id))
                            {
                                fill_autoplay_queue(&core, &sc_client, &seed);
                            }
                        }
                        let mut next_id = core.dequeue();
                        if next_id.is_none() {
                            if let Some(collection) = active_collection.as_mut() {
                                if repeat_mode == RepeatMode::All {
                                    collection.restart_cycle(shuffle_enabled);
                                    sync_collection_queue(&core, collection, &[]);
                                    next_id = collection.current_id().map(ToOwned::to_owned);
                                }
                            } else if repeat_mode == RepeatMode::All {
                                next_id = current_track_id.clone();
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
                                if !restart_sink_in_place(
                                    &sink,
                                    &mut elapsed_before_pause,
                                    &mut playback_start,
                                    &mut is_paused,
                                ) {
                                    tx_clone.send(PlaybackCommand::PlayTrack(current_id)).ok();
                                }
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
                    PlaybackCommand::ToggleRepeat => {
                        repeat_mode = repeat_mode.cycle();
                        repeat_tx.send(repeat_mode).ok();
                        msg_tx.send(repeat_mode.label().to_string()).ok();
                    }
                    PlaybackCommand::ToggleShuffle => {
                        if let Some(collection) = active_collection.as_mut() {
                            shuffle_enabled = !shuffle_enabled;
                            queue_before_shuffle = None;
                            let extras = non_collection_queue_ids(&core, collection);
                            collection.rebuild_order(shuffle_enabled);
                            sync_collection_queue(&core, collection, &extras);
                            shuffle_tx.send(shuffle_enabled).ok();
                            msg_tx
                                .send(format!(
                                    "collection shuffle {}",
                                    if shuffle_enabled { "on" } else { "off" }
                                ))
                                .ok();
                            continue;
                        }
                        if autoplay {
                            msg_tx
                                .send("shuffle is disabled for autoplay queue".to_string())
                                .ok();
                            continue;
                        }
                        if shuffle_enabled {
                            if let Some(original) = queue_before_shuffle.take() {
                                core.set_queue(original);
                            }
                            shuffle_enabled = false;
                            shuffle_tx.send(false).ok();
                            msg_tx.send("queue shuffle off".to_string()).ok();
                            continue;
                        }
                        let original = core.q_ids();
                        if original.len() < 2 {
                            msg_tx
                                .send("queue needs at least 2 tracks to shuffle".to_string())
                                .ok();
                            continue;
                        }
                        queue_before_shuffle = Some(original);
                        if shuffle_queue(&core) {
                            shuffle_enabled = true;
                            shuffle_tx.send(true).ok();
                            msg_tx.send("queue shuffle on".to_string()).ok();
                        }
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
                        let saved_file_path = current_file_path.clone();
                        let saved_position = current_pos;
                        let was_playing = !is_paused
                            && (sink.is_some()
                                || saved_track_id.is_some()
                                || saved_file_path.is_some());

                        sink = None;
                        stream_handle = None;
                        _stream = None;
                        current_device_name = None;
                        elapsed_before_pause = saved_position;
                        playback_start = None;

                        match rebuild_output_stream(
                            &host,
                            Some(device_name.as_str()),
                            false,
                            &core,
                            &sc_client,
                            &saved_track_id,
                            &saved_file_path,
                            saved_position,
                            was_playing,
                            fft_processor.clone(),
                            visualizer_tx.clone(),
                            visualizer_enabled.clone(),
                            volume,
                        ) {
                            Ok(recovered) => {
                                _stream = Some(recovered.stream);
                                stream_handle = Some(recovered.handle);
                                current_device_name = Some(recovered.device_name);
                                sink = recovered.sink;
                                if sink.is_some() {
                                    current_duration = recovered.duration;
                                    is_paused = !was_playing;
                                    playback_start = Some(Instant::now());
                                    last_finished_track_id = None;
                                }
                                waiting_for_output_recovery = false;

                                if let Some(err) = recovered.restore_error {
                                    msg_tx.send(err).ok();
                                }
                            }
                            Err(err) => {
                                msg_tx.send(err.to_string()).ok();
                            }
                        }
                    }
                    PlaybackCommand::Prepared(res, id, offset) => {
                        if preparing_track_id.as_deref() == Some(id.as_str()) {
                            preparing_track_id = None;
                            match res {
                                Ok((new_sink, duration)) => {
                                    last_finished_track_id = None;
                                    current_duration = duration;
                                    current_track_id = Some(id.clone());
                                    current_file_path = None;
                                    core.set_cur(Some(id.clone()));
                                    core.add_hist(id);
                                    sink = Some(new_sink);
                                    is_paused = false;
                                    elapsed_before_pause = offset;
                                    playback_start = Some(Instant::now());
                                    if let Some(track) = core.track(current_track_id.as_ref().unwrap()) {
                                        maybe_seed_autoplay_queue(&core, &sc_client, &track, autoplay);
                                        prefetch_next_remote_track(&core, &sc_client);
                                    }
                                }
                                Err(e) => {
                                    last_finished_track_id = None;
                                    current_track_id = None;
                                    current_file_path = None;
                                    core.set_cur(None);
                                    msg_tx.send(e).ok();
                                }
                            }
                        }
                    }
                    PlaybackCommand::Quit => {
                        if let Ok(mut rpc) = discord_rpc.lock() {
                            if let Some(rpc) = rpc.as_mut() {
                                rpc.clear();
                            }
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
        autoplay_rx,
        repeat_rx,
        shuffle_rx,
        volume_rx,
        visualizer_rx,
        msg_rx,
    }
}
