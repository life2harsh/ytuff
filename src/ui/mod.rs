use crate::core::Core;
use crate::playback::{PlaybackCommand, PlaybackHandle};
use crossterm::event::{self, Event as CEvent, KeyCode, KeyEventKind};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{
        Block, Borders, Clear, Gauge, List, ListItem, ListState, Paragraph, Wrap,
    },
    Frame, Terminal,
};
use std::io;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AppMode {
    Library,
    Queue,
    Devices,
    Help,
}

pub struct App {
    pub mode: AppMode,
    pub core: Core,
    pub playback: PlaybackHandle,
    pub library_state: ListState,
    pub queue_state: ListState,
    pub device_state: ListState,
    pub devices: Vec<(String, bool)>, // (name, is_current)
    pub selected_device: Option<usize>,
    pub show_help: bool,
    pub message: Option<(String, std::time::Instant)>, // (message, timestamp)
    pub current_volume: f32,
}

impl App {
    pub fn new(core: Core, playback: PlaybackHandle) -> Self {
        let mut library_state = ListState::default();
        library_state.select(Some(0));

        let mut queue_state = ListState::default();
        queue_state.select(Some(0));

        let mut device_state = ListState::default();
        device_state.select(Some(0));

        App {
            mode: AppMode::Library,
            core,
            playback,
            library_state,
            queue_state,
            device_state,
            devices: Vec::new(),
            selected_device: None,
            show_help: false,
            message: None,
            current_volume: 1.0,
        }
    }

    pub fn next_item(&mut self) {
        match self.mode {
            AppMode::Library => {
                let tracks = self.core.tracks.lock().unwrap();
                let i = match self.library_state.selected() {
                    Some(i) => {
                        if i >= tracks.len().saturating_sub(1) {
                            0
                        } else {
                            i + 1
                        }
                    }
                    None => 0,
                };
                self.library_state.select(Some(i));
            }
            AppMode::Queue => {
                let queue = self.core.queue.lock().unwrap();
                let i = match self.queue_state.selected() {
                    Some(i) => {
                        if i >= queue.len().saturating_sub(1) {
                            0
                        } else {
                            i + 1
                        }
                    }
                    None => 0,
                };
                self.queue_state.select(Some(i));
            }
            AppMode::Devices => {
                let i = match self.device_state.selected() {
                    Some(i) => {
                        if i >= self.devices.len().saturating_sub(1) {
                            0
                        } else {
                            i + 1
                        }
                    }
                    None => 0,
                };
                self.device_state.select(Some(i));
            }
            _ => {}
        }
    }

    pub fn previous_item(&mut self) {
        match self.mode {
            AppMode::Library => {
                let tracks = self.core.tracks.lock().unwrap();
                let i = match self.library_state.selected() {
                    Some(i) => {
                        if i == 0 {
                            tracks.len().saturating_sub(1)
                        } else {
                            i - 1
                        }
                    }
                    None => 0,
                };
                self.library_state.select(Some(i));
            }
            AppMode::Queue => {
                let queue = self.core.queue.lock().unwrap();
                let i = match self.queue_state.selected() {
                    Some(i) => {
                        if i == 0 {
                            queue.len().saturating_sub(1)
                        } else {
                            i - 1
                        }
                    }
                    None => 0,
                };
                self.queue_state.select(Some(i));
            }
            AppMode::Devices => {
                let i = match self.device_state.selected() {
                    Some(i) => {
                        if i == 0 {
                            self.devices.len().saturating_sub(1)
                        } else {
                            i - 1
                        }
                    }
                    None => 0,
                };
                self.device_state.select(Some(i));
            }
            _ => {}
        }
    }

    pub fn play_selected(&mut self) {
        if let AppMode::Library = self.mode {
            if let Some(idx) = self.library_state.selected() {
                self.core.enqueue(idx);
                self.playback.tx.send(PlaybackCommand::PlayIndex(idx)).ok();
            }
        }
    }

    pub fn add_to_queue(&mut self) {
        if let AppMode::Library = self.mode {
            if let Some(idx) = self.library_state.selected() {
                self.core.enqueue(idx);
                self.set_message(format!("Added track #{} to queue", idx + 1));
            }
        }
    }

    pub fn refresh_devices(&mut self) {
        self.playback.tx.send(PlaybackCommand::ListDevices).ok();
    }

    pub fn select_device(&mut self) {
        if let AppMode::Devices = self.mode {
            if let Some(idx) = self.device_state.selected() {
                if let Some((name, _)) = self.devices.get(idx) {
                    self.playback
                        .tx
                        .send(PlaybackCommand::SwitchDevice(name.clone()))
                        .ok();
                    self.set_message(format!("Switching to device: {}", name));
                }
            }
        }
    }

    pub fn set_message(&mut self, msg: String) {
        self.message = Some((msg, std::time::Instant::now()));
    }

    pub fn clear_expired_message(&mut self) {
        if let Some((_, timestamp)) = self.message {
            if timestamp.elapsed() > Duration::from_secs(3) {
                self.message = None;
            }
        }
    }
}

pub async fn run_ui(core: Core, playback: PlaybackHandle) -> anyhow::Result<()> {
    let stdout = io::stdout();
    crossterm::terminal::enable_raw_mode()?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let (tx, rx): (Sender<CEvent>, Receiver<CEvent>) = mpsc::channel();
    thread::spawn(move || {
        loop {
            if event::poll(Duration::from_millis(50)).unwrap_or(false) {
                if let Ok(ev) = event::read() {
                    if tx.send(ev).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let mut app = App::new(core, playback);
    app.refresh_devices();

    loop {
        app.clear_expired_message();

        // Check for device updates from playback thread
        if let Ok(devices) = app.playback.devices_rx.try_recv() {
            let current_device = devices
                .iter()
                .position(|(_, is_current)| *is_current)
                .unwrap_or(0);
            app.devices = devices;
            app.selected_device = Some(current_device);
            app.device_state.select(Some(current_device));
        }

        // Check for volume updates
        while let Ok(volume) = app.playback.volume_rx.try_recv() {
            app.current_volume = volume;
        }

        terminal.draw(|f| draw_ui(f, &mut app))?;

        if let Ok(ev) = rx.recv_timeout(Duration::from_millis(50)) {
            match ev {
                CEvent::Key(key) => {
                    if key.kind == KeyEventKind::Press {
                        match app.mode {
                            AppMode::Library | AppMode::Queue => match key.code {
                                KeyCode::Char('q') => {
                                    app.playback.tx.send(PlaybackCommand::Quit).ok();
                                    break;
                                }
                                KeyCode::Char(' ') => {
                                    app.playback.tx.send(PlaybackCommand::Pause).ok();
                                }
                                KeyCode::Char('r') => {
                                    app.playback.tx.send(PlaybackCommand::Resume).ok();
                                }
                                KeyCode::Char('n') => {
                                    app.playback.tx.send(PlaybackCommand::Next).ok();
                                }
                                KeyCode::Char('p') => {
                                    app.playback.tx.send(PlaybackCommand::Prev).ok();
                                }
                                KeyCode::Char('l') => app.mode = AppMode::Library,
                                KeyCode::Char('Q') => app.mode = AppMode::Queue,
                                KeyCode::Char('d') | KeyCode::Char('D') => {
                                    app.mode = AppMode::Devices;
                                    app.refresh_devices();
                                }
                                KeyCode::Char('?') | KeyCode::Char('h') => {
                                    app.show_help = !app.show_help;
                                }
                                KeyCode::Char('a') => app.add_to_queue(),
                                KeyCode::Enter => app.play_selected(),
                                KeyCode::Char('j') | KeyCode::Down => app.next_item(),
                                KeyCode::Char('k') | KeyCode::Up => app.previous_item(),
                                KeyCode::Char('+') | KeyCode::Char('=') => {
                                    app.playback.tx.send(PlaybackCommand::VolumeUp).ok();
                                    app.set_message("Volume up".to_string());
                                }
                                KeyCode::Char('-') => {
                                    app.playback.tx.send(PlaybackCommand::VolumeDown).ok();
                                    app.set_message("Volume down".to_string());
                                }
                                KeyCode::Char('0') => {
                                    app.playback.tx.send(PlaybackCommand::ToggleMute).ok();
                                    if app.current_volume > 0.0 {
                                        app.set_message("Muted".to_string());
                                    } else {
                                        app.set_message("Unmuted".to_string());
                                    }
                                }
                                KeyCode::Right => {
                                    app.playback.tx.send(PlaybackCommand::SkipForward(5)).ok();
                                    app.set_message("+5 seconds".to_string());
                                }
                                KeyCode::Left => {
                                    app.playback.tx.send(PlaybackCommand::SkipBackward(5)).ok();
                                    app.set_message("-5 seconds".to_string());
                                }
                                KeyCode::Char('f') => {
                                    app.playback.tx.send(PlaybackCommand::SkipForward(30)).ok();
                                    app.set_message("+30 seconds".to_string());
                                }
                                KeyCode::Char('b') => {
                                    app.playback.tx.send(PlaybackCommand::SkipBackward(30)).ok();
                                    app.set_message("-30 seconds".to_string());
                                }
                                _ => {}
                            },
                            AppMode::Devices => match key.code {
                                KeyCode::Esc | KeyCode::Char('q') => app.mode = AppMode::Library,
                                KeyCode::Enter => app.select_device(),
                                KeyCode::Char('j') | KeyCode::Down => app.next_item(),
                                KeyCode::Char('k') | KeyCode::Up => app.previous_item(),
                                KeyCode::Char('r') => app.refresh_devices(),
                                _ => {}
                            },
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }

    crossterm::terminal::disable_raw_mode()?;
    terminal.show_cursor()?;
    Ok(())
}

fn draw_ui(f: &mut Frame, app: &mut App) {
    let size = f.area();

    // Create main layout
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Title bar
            Constraint::Min(10),   // Main content
            Constraint::Length(8), // Now playing
            Constraint::Length(3), // Progress
            Constraint::Length(3), // Status bar / Help
        ])
        .split(size);

    // Title bar
    let title_spans = vec![
        Span::styled("RustPlayer", Style::default().fg(Color::Cyan).bold()),
        Span::raw(" v0.1.0 "),
        Span::styled(
            match app.mode {
                AppMode::Library => "[Library]",
                AppMode::Queue => "[Queue]",
                AppMode::Devices => "[Devices]",
                AppMode::Help => "[Help]",
            },
            Style::default().fg(Color::Yellow),
        ),
    ];
    let title = Paragraph::new(Line::from(title_spans)).alignment(Alignment::Center);
    f.render_widget(title, main_chunks[0]);

    // Main content area
    match app.mode {
        AppMode::Library => draw_library(f, app, main_chunks[1]),
        AppMode::Queue => draw_queue(f, app, main_chunks[1]),
        AppMode::Devices => draw_devices(f, app, main_chunks[1]),
        _ => draw_library(f, app, main_chunks[1]),
    }

    // Now playing section
    draw_now_playing(f, app, main_chunks[2]);

    // Progress bar
    draw_progress_bar(f, app, main_chunks[3]);

    // Help / status bar
    draw_status_bar(f, app, main_chunks[4]);

    // Help popup
    if app.show_help {
        draw_help_popup(f, size);
    }

    // Message popup
    if let Some((msg, _)) = &app.message {
        draw_message_popup(f, size, msg.clone());
    }
}

fn draw_library(f: &mut Frame, app: &mut App, area: Rect) {
    let tracks = app.core.tracks.lock().unwrap();
    let current_idx = *app.core.current.lock().unwrap();

    let items: Vec<ListItem> = tracks
        .iter()
        .enumerate()
        .map(|(idx, t)| {
            let duration_str = if let Some(dur) = t.duration_seconds {
                format!("{:02}:{:02}", dur / 60, dur % 60)
            } else {
                "--:--".to_string()
            };

            let artist = t
                .artist
                .clone()
                .unwrap_or_else(|| "Unknown Artist".to_string());
            let track_type_icon = match &t.track_type {
                crate::core::track::TrackType::Local => "📁",
                crate::core::track::TrackType::SoundCloud => "☁️",
            };

            let content = format!(
                "{:3}. {} {} - {} [{}]",
                idx + 1,
                track_type_icon,
                t.title,
                artist,
                duration_str
            );

            let mut style = Style::default();
            if Some(idx) == current_idx {
                style = style.fg(Color::Green).add_modifier(Modifier::BOLD);
            }

            ListItem::new(content).style(style)
        })
        .collect();

    let library = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Library ({}) ", tracks.len()))
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    f.render_stateful_widget(library, area, &mut app.library_state);
}

fn draw_queue(f: &mut Frame, app: &mut App, area: Rect) {
    let queue = app.core.queue.lock().unwrap();
    let tracks = app.core.tracks.lock().unwrap();

    let items: Vec<ListItem> = queue
        .iter()
        .enumerate()
        .filter_map(|(pos, &idx)| {
            tracks.get(idx).map(|t| {
                let artist = t
                    .artist
                    .clone()
                    .unwrap_or_else(|| "Unknown".to_string());
                let content = format!("{:2}. {} - {}", pos + 1, t.title, artist);
                ListItem::new(content)
            })
        })
        .collect();

    let queue_list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Queue ({}) ", queue.len()))
                .border_style(Style::default().fg(Color::Magenta)),
        )
        .highlight_style(Style::default().bg(Color::DarkGray))
        .highlight_symbol("▶ ");

    f.render_stateful_widget(queue_list, area, &mut app.queue_state);
}

fn draw_devices(f: &mut Frame, app: &mut App, area: Rect) {
    let items: Vec<ListItem> = app
        .devices
        .iter()
        .enumerate()
        .map(|(_idx, (name, is_current))| {
            let icon = if *is_current { "● " } else { "○ " };
            let content = format!("{}{}", icon, name);
            let style = if *is_current {
                Style::default().fg(Color::Green).bold()
            } else {
                Style::default()
            };
            ListItem::new(content).style(style)
        })
        .collect();

    let device_list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Audio Devices ({}) ", app.devices.len()))
                .border_style(Style::default().fg(Color::Blue)),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    f.render_stateful_widget(device_list, area, &mut app.device_state);
}

fn draw_now_playing(f: &mut Frame, app: &App, area: Rect) {
    let tracks = app.core.tracks.lock().unwrap();
    let current_idx = *app.core.current.lock().unwrap();

    let mut lines = vec![Line::from(vec![
        Span::styled("Now Playing", Style::default().fg(Color::Yellow).bold()),
    ])];

    if let Some(idx) = current_idx {
        if let Some(track) = tracks.get(idx) {
            lines.push(Line::from(vec![
                Span::styled("Title:  ", Style::default().fg(Color::Green)),
                Span::raw(&track.title),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Artist: ", Style::default().fg(Color::Cyan)),
                Span::raw(track.artist.clone().unwrap_or_else(|| "Unknown".to_string())),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Source: ", Style::default().fg(Color::Magenta)),
                Span::raw(format!("{:?}", track.track_type)),
            ]));
        } else {
            lines.push(Line::from("  Nothing playing"));
        }
    } else {
        lines.push(Line::from("  Nothing playing"));
    }

    lines.push(Line::from(""));

    let queue = app.core.queue.lock().unwrap();
    lines.push(Line::from(vec![
        Span::styled("Queue: ", Style::default().fg(Color::Magenta)),
        Span::raw(format!("{} track(s)", queue.len())),
    ]));

    // Volume info - use stored volume
    let volume = app.current_volume;
    let vol_percent = (volume * 100.0) as i32;
    let vol_color = if volume > 0.8 {
        Color::Red
    } else if volume > 0.5 {
        Color::Yellow
    } else {
        Color::Green
    };
    let vol_icon = if volume == 0.0 {
        "🔇"
    } else if volume < 0.3 {
        "🔈"
    } else if volume < 0.7 {
        "🔉"
    } else {
        "🔊"
    };
    lines.push(Line::from(vec![
        Span::styled("Volume: ", Style::default().fg(Color::Blue)),
        Span::raw(format!("{} ", vol_icon)),
        Span::styled(format!("{}%", vol_percent), Style::default().fg(vol_color)),
    ]));

    let now_playing = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Now Playing ")
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .wrap(Wrap { trim: true });

    f.render_widget(now_playing, area);
}

fn draw_progress_bar(f: &mut Frame, app: &App, area: Rect) {
    let position = app.playback.position_rx.lock().unwrap();
    let (progress_ratio, time_display, is_playing) =
        if let Some((current, total, playing)) = *position {
            let ratio = if total > 0 {
                (current as f64 / total as f64).min(1.0)
            } else {
                0.0
            };
            let curr_min = current / 60;
            let curr_sec = current % 60;
            let tot_min = total / 60;
            let tot_sec = total % 60;
            (
                ratio,
                format!(
                    "{:02}:{:02} / {:02}:{:02}",
                    curr_min, curr_sec, tot_min, tot_sec
                ),
                playing,
            )
        } else {
            (0.0, "00:00 / 00:00".to_string(), false)
        };

    let label = format!("{} {}", if is_playing { "▶" } else { "⏸" }, time_display);
    let gauge_color = if is_playing {
        Color::Green
    } else {
        Color::Gray
    };

    let progress_bar = Gauge::default()
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Progress ")
                .border_style(Style::default().fg(gauge_color)),
        )
        .gauge_style(Style::default().fg(gauge_color).bg(Color::Black))
        .label(label)
        .ratio(progress_ratio);

    f.render_widget(progress_bar, area);
}

fn draw_status_bar(f: &mut Frame, _app: &App, area: Rect) {
    let help_text = vec![
        Span::styled("Enter", Style::default().fg(Color::Green).bold()),
        Span::raw("Play "),
        Span::styled("Space", Style::default().fg(Color::Cyan).bold()),
        Span::raw("Pause "),
        Span::styled("←/→", Style::default().fg(Color::Yellow).bold()),
        Span::raw("Seek "),
        Span::styled("n/p", Style::default().fg(Color::Blue).bold()),
        Span::raw("Track "),
        Span::styled("+/-", Style::default().fg(Color::Cyan).bold()),
        Span::raw("Vol "),
        Span::styled("?", Style::default().fg(Color::White).bold()),
        Span::raw("Help "),
        Span::styled("q", Style::default().fg(Color::Red).bold()),
        Span::raw("Quit"),
    ];

    let help = Paragraph::new(Line::from(help_text))
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Gray)),
        );

    f.render_widget(help, area);
}

fn draw_help_popup(f: &mut Frame, area: Rect) {
    let popup_area = centered_rect(60, 70, area);

    let help_text = vec![
        Line::from(vec![
            Span::styled("Keyboard Shortcuts", Style::default().fg(Color::Cyan).bold()),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Navigation", Style::default().fg(Color::Yellow)),
        ]),
        Line::from("  j/k or ↑/↓  - Navigate up/down"),
        Line::from("  l           - Library view"),
        Line::from("  Q           - Queue view"),
        Line::from("  d           - Device selection"),
        Line::from(""),
        Line::from(vec![
            Span::styled("Playback", Style::default().fg(Color::Yellow)),
        ]),
        Line::from("  Enter       - Play selected track"),
        Line::from("  Space       - Pause playback"),
        Line::from("  r           - Resume playback"),
        Line::from("  n           - Next track"),
        Line::from("  p           - Previous track"),
        Line::from("  ←/→         - Seek -5s/+5s"),
        Line::from("  b/f         - Seek -30s/+30s"),
        Line::from("  a           - Add to queue"),
        Line::from(""),
        Line::from(vec![
            Span::styled("Volume", Style::default().fg(Color::Yellow)),
        ]),
        Line::from("  +/=         - Volume up"),
        Line::from("  -           - Volume down"),
        Line::from("  0           - Toggle mute"),
        Line::from(""),
        Line::from(vec![
            Span::styled("Other", Style::default().fg(Color::Yellow)),
        ]),
        Line::from("  ?/h         - Toggle this help"),
        Line::from("  q           - Quit"),
        Line::from("  Esc         - Close popup/back"),
    ];

    let help_paragraph = Paragraph::new(Text::from(help_text))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Help ")
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .alignment(Alignment::Left);

    f.render_widget(Clear, popup_area);
    f.render_widget(help_paragraph, popup_area);
}

fn draw_message_popup(f: &mut Frame, area: Rect, message: String) {
    let popup_area = centered_rect(40, 20, area);

    let msg = Paragraph::new(message)
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Green)),
        );

    f.render_widget(Clear, popup_area);
    f.render_widget(msg, popup_area);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
