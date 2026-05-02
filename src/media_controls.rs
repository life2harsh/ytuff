use crate::core::track::Track;
use crate::playback::PlaybackCommand;
use anyhow::{anyhow, Context, Result};
use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
    SeekDirection,
};
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

pub struct MediaSession {
    controls: MediaControls,
    last_metadata: MetadataState,
    last_playback: PlaybackState,
    #[cfg(target_os = "linux")]
    last_volume_bucket: Option<u16>,
    #[cfg(target_os = "windows")]
    _window: HiddenWindow,
}

#[derive(Clone, Default, PartialEq, Eq)]
struct MetadataState {
    track_id: Option<String>,
    title: Option<String>,
    artist: Option<String>,
    cover_url: Option<String>,
    duration_secs: Option<u64>,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum PlaybackMode {
    #[default]
    Stopped,
    Paused,
    Playing,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct PlaybackState {
    mode: PlaybackMode,
    position_secs: Option<u64>,
}

impl MediaSession {
    pub fn new(tx: Sender<PlaybackCommand>) -> Result<Option<Self>> {
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            let _ = tx;
            Ok(None)
        }

        #[cfg(any(target_os = "linux", target_os = "windows"))]
        {
            #[cfg(target_os = "windows")]
            let window = HiddenWindow::new()?;

            let mut controls = MediaControls::new(PlatformConfig {
                dbus_name: "rustplayer",
                display_name: "RustPlayer",
                #[cfg(target_os = "windows")]
                hwnd: Some(window.hwnd()),
                #[cfg(not(target_os = "windows"))]
                hwnd: None,
            })
            .context("Could not initialize OS media controls")?;

            controls
                .attach(move |event| {
                    if let Some(command) = command_from_event(event) {
                        let _ = tx.send(command);
                    }
                })
                .context("Could not attach OS media control events")?;

            Ok(Some(Self {
                controls,
                last_metadata: MetadataState::default(),
                last_playback: PlaybackState::default(),
                #[cfg(target_os = "linux")]
                last_volume_bucket: None,
                #[cfg(target_os = "windows")]
                _window: window,
            }))
        }
    }

    pub fn sync(
        &mut self,
        track: Option<&Track>,
        position_secs: u64,
        is_playing: bool,
        volume: f32,
    ) {
        #[cfg(not(target_os = "linux"))]
        let _ = volume;

        let metadata = MetadataState::from_track(track);
        if metadata != self.last_metadata {
            let _ = self.controls.set_metadata(MediaMetadata {
                title: metadata.title.as_deref(),
                album: None,
                artist: metadata.artist.as_deref(),
                cover_url: metadata.cover_url.as_deref(),
                duration: metadata.duration_secs.map(Duration::from_secs),
            });
            self.last_metadata = metadata;
        }

        let playback = if track.is_none() {
            PlaybackState {
                mode: PlaybackMode::Stopped,
                position_secs: None,
            }
        } else if is_playing {
            PlaybackState {
                mode: PlaybackMode::Playing,
                position_secs: Some(position_secs),
            }
        } else {
            PlaybackState {
                mode: PlaybackMode::Paused,
                position_secs: Some(position_secs),
            }
        };

        if playback != self.last_playback {
            let progress = playback
                .position_secs
                .map(|secs| MediaPosition(Duration::from_secs(secs)));
            let _ = self.controls.set_playback(match playback.mode {
                PlaybackMode::Stopped => MediaPlayback::Stopped,
                PlaybackMode::Paused => MediaPlayback::Paused { progress },
                PlaybackMode::Playing => MediaPlayback::Playing { progress },
            });
            self.last_playback = playback;
        }

        #[cfg(target_os = "linux")]
        {
            let bucket = (volume.clamp(0.0, 1.0) * 1000.0).round() as u16;
            if self.last_volume_bucket != Some(bucket) {
                let _ = self.controls.set_volume(volume.clamp(0.0, 1.0) as f64);
                self.last_volume_bucket = Some(bucket);
            }
        }
    }
}

impl MetadataState {
    fn from_track(track: Option<&Track>) -> Self {
        let Some(track) = track else {
            return Self::default();
        };
        Self {
            track_id: Some(track.id.clone()),
            title: Some(track.title.clone()),
            artist: Some(track.who()),
            cover_url: track.art.clone(),
            duration_secs: track.dur,
        }
    }
}

fn command_from_event(event: MediaControlEvent) -> Option<PlaybackCommand> {
    match event {
        MediaControlEvent::Play => Some(PlaybackCommand::Resume),
        MediaControlEvent::Pause => Some(PlaybackCommand::Pause),
        MediaControlEvent::Toggle => Some(PlaybackCommand::TogglePause),
        MediaControlEvent::Next => Some(PlaybackCommand::Next),
        MediaControlEvent::Previous => Some(PlaybackCommand::Prev),
        MediaControlEvent::Stop => Some(PlaybackCommand::Stop),
        MediaControlEvent::Seek(direction) => Some(match direction {
            SeekDirection::Forward => PlaybackCommand::SkipForward(10),
            SeekDirection::Backward => PlaybackCommand::SkipBackward(10),
        }),
        MediaControlEvent::SeekBy(direction, amount) => {
            let seconds = amount.as_secs().max(1);
            Some(match direction {
                SeekDirection::Forward => PlaybackCommand::SkipForward(seconds),
                SeekDirection::Backward => PlaybackCommand::SkipBackward(seconds),
            })
        }
        MediaControlEvent::SetPosition(position) => {
            Some(PlaybackCommand::SeekTo(position.0.as_secs()))
        }
        MediaControlEvent::SetVolume(volume) => {
            Some(PlaybackCommand::SetVolume(volume.clamp(0.0, 1.0) as f32))
        }
        MediaControlEvent::OpenUri(_) | MediaControlEvent::Raise | MediaControlEvent::Quit => None,
    }
}

#[cfg(target_os = "windows")]
struct HiddenWindow {
    hwnd: usize,
    thread_id: u32,
    join: Option<std::thread::JoinHandle<()>>,
}

#[cfg(target_os = "windows")]
unsafe impl Send for HiddenWindow {}

#[cfg(target_os = "windows")]
impl HiddenWindow {
    fn new() -> Result<Self> {
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(usize, u32)>>();
        let join = std::thread::spawn(move || run_hidden_window(ready_tx));
        let (hwnd, thread_id): (usize, u32) = ready_rx
            .recv_timeout(Duration::from_secs(5))
            .context("Timed out creating the media-control window")??;
        Ok(Self {
            hwnd,
            thread_id,
            join: Some(join),
        })
    }

    fn hwnd(&self) -> *mut std::ffi::c_void {
        self.hwnd as *mut std::ffi::c_void
    }
}

#[cfg(target_os = "windows")]
impl Drop for HiddenWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = windows_sys::Win32::UI::WindowsAndMessaging::PostThreadMessageW(
                self.thread_id,
                windows_sys::Win32::UI::WindowsAndMessaging::WM_QUIT,
                0,
                0,
            );
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

#[cfg(target_os = "windows")]
fn run_hidden_window(ready_tx: Sender<Result<(usize, u32)>>) {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, DispatchMessageW, GetMessageW, TranslateMessage, MSG,
        WS_OVERLAPPEDWINDOW,
    };

    unsafe {
        let instance = GetModuleHandleW(null());
        if instance.is_null() {
            let _ = ready_tx.send(Err(anyhow!(
                "Could not get the current module handle for media controls"
            )));
            return;
        }

        let class_name = wide("STATIC");
        let title = wide("RustPlayer Media Controls");
        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            instance,
            null_mut(),
        );

        if hwnd.is_null() {
            let _ = ready_tx.send(Err(anyhow!(
                "Could not create the hidden media-control window"
            )));
            return;
        }

        let _ = ready_tx.send(Ok((hwnd as usize, GetCurrentThreadId())));

        let mut msg: MSG = std::mem::zeroed();
        loop {
            let code = GetMessageW(&mut msg, null_mut(), 0, 0);
            if code <= 0 {
                break;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        let _ = DestroyWindow(hwnd);
    }
}

#[cfg(target_os = "windows")]
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
