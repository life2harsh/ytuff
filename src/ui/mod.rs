use crate::core::Core;
use crate::playback::{PlaybackCommand, PlaybackHandle};
use crossterm::event::{self, Event as CEvent, KeyCode, KeyEventKind};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{
        Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap,
    },
    Frame, Terminal,
};
use std::io;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

pub struct App {
    pub core: Core,
    pub playback: PlaybackHandle,
    pub library_state: ListState,
    pub queue_state: ListState,
    pub device_state: ListState,
    pub devices: Vec<(String, bool)>,
    pub show_help: bool,
    pub show_devices: bool,
    pub message: Option<(String, Instant, bool)>,
    pub current_volume: f32,
    pub volume_notification_time: Option<Instant>,
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
            core,
            playback,
            library_state,
            queue_state,
            device_state,
            devices: Vec::new(),
            show_help: false,
            show_devices: false,
            message: None,
            current_volume: 1.0,
            volume_notification_time: None,
        }
    }

    pub fn next_library_item(&mut self) {
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

    pub fn previous_library_item(&mut self) {
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

    pub fn next_queue_item(&mut self) {
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

    pub fn previous_queue_item(&mut self) {
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

    pub fn next_device_item(&mut self) {
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

    pub fn previous_device_item(&mut self) {
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

    pub fn play_selected(&mut self) {
        if let Some(idx) = self.library_state.selected() {
            self.core.enqueue(idx);
            self.playback.tx.send(PlaybackCommand::PlayIndex(idx)).ok();
        }
    }

    pub fn play_queue_item(&mut self) {
        if let Some(pos) = self.queue_state.selected() {
            let queue = self.core.queue.lock().unwrap();
            if let Some(&track_idx) = queue.get(pos) {
                drop(queue);
                self.playback.tx.send(PlaybackCommand::PlayIndex(track_idx)).ok();
            }
        }
    }

    pub fn add_to_queue(&mut self) {
        if let Some(idx) = self.library_state.selected() {
            self.core.enqueue(idx);
            self.set_message(format!("Added track #{} to queue", idx + 1), false);
        }
    }

    pub fn refresh_devices(&mut self) {
        self.playback.tx.send(PlaybackCommand::ListDevices).ok();
    }

    pub fn select_device(&mut self) {
        if let Some(idx) = self.device_state.selected() {
            if let Some((name, _)) = self.devices.get(idx) {
                self.playback
                    .tx
                    .send(PlaybackCommand::SwitchDevice(name.clone()))
                    .ok();
                self.set_message(format!("Device: {}", name), false);
            }
        }
    }

    pub fn set_message(&mut self, msg: String, is_volume: bool) {
        self.message = Some((msg, Instant::now(), is_volume));
        if is_volume {
            self.volume_notification_time = Some(Instant::now());
        }
    }

    pub fn clear_expired_message(&mut self) {
        if let Some((_, timestamp, is_volume)) = self.message {
            let delay = if is_volume {
                Duration::from_secs(4)
            } else {
                Duration::from_secs(3)
            };
            if timestamp.elapsed() > delay {
                self.message = None;
            }
        }
    }

    pub fn should_show_volume_notification(&self) -> bool {
        if let Some(time) = self.volume_notification_time {
            time.elapsed() >= Duration::from_secs(1)
        } else {
            false
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

        if let Ok(devices) = app.playback.devices_rx.try_recv() {
            app.devices = devices;
        }

        while let Ok(volume) = app.playback.volume_rx.try_recv() {
            app.current_volume = volume;
            if app.volume_notification_time.is_some() {
                app.volume_notification_time = Some(Instant::now());
            }
        }

        terminal.draw(|f| draw_ui(f, &mut app))?;

        if let Ok(ev) = rx.recv_timeout(Duration::from_millis(50)) {
            match ev {
                CEvent::Key(key) => {
                    if key.kind == KeyEventKind::Press {
                        match key.code {
                            KeyCode::Esc => {
                                if app.show_devices {
                                    app.show_devices = false;
                                } else if app.show_help {
                                    app.show_help = false;
                                }
                            }
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
                            KeyCode::Char('?') | KeyCode::Char('h') => {
                                app.show_help = !app.show_help;
                            }
                            KeyCode::Char('d') => {
                                app.show_devices = !app.show_devices;
                                if app.show_devices {
                                    app.refresh_devices();
                                }
                            }
                            KeyCode::Char('a') => app.add_to_queue(),
                            KeyCode::Enter => {
                                app.play_selected();
                            }
                            KeyCode::Char('j') | KeyCode::Down => {
                                app.next_library_item();
                            }
                            KeyCode::Char('k') | KeyCode::Up => {
                                app.previous_library_item();
                            }
                            KeyCode::Char('J') => {
                                if app.show_devices {
                                    app.next_device_item();
                                } else {
                                    app.next_queue_item();
                                }
                            }
                            KeyCode::Char('K') => {
                                if app.show_devices {
                                    app.previous_device_item();
                                } else {
                                    app.previous_queue_item();
                                }
                            }
                            KeyCode::Char('D') => {
                                app.select_device();
                            }
                            KeyCode::Char('+') | KeyCode::Char('=') => {
                                app.playback.tx.send(PlaybackCommand::VolumeUp).ok();
                                let new_vol = ((app.current_volume + 0.1).min(1.0) * 100.0) as i32;
                                app.set_message(format!("Volume: {}%", new_vol), true);
                            }
                            KeyCode::Char('-') => {
                                app.playback.tx.send(PlaybackCommand::VolumeDown).ok();
                                let new_vol = ((app.current_volume - 0.1).max(0.0) * 100.0) as i32;
                                app.set_message(format!("Volume: {}%", new_vol), true);
                            }
                            KeyCode::Char('0') => {
                                app.playback.tx.send(PlaybackCommand::ToggleMute).ok();
                                if app.current_volume > 0.0 {
                                    app.set_message("Volume: Muted".to_string(), true);
                                } else {
                                    let vol = (app.current_volume * 100.0) as i32;
                                    app.set_message(format!("Volume: {}%", vol), true);
                                }
                            }
                            KeyCode::Right => {
                                app.playback.tx.send(PlaybackCommand::SkipForward(5)).ok();
                                app.set_message("+5 seconds".to_string(), false);
                            }
                            KeyCode::Left => {
                                app.playback.tx.send(PlaybackCommand::SkipBackward(5)).ok();
                                app.set_message("-5 seconds".to_string(), false);
                            }
                            KeyCode::Char('f') => {
                                app.playback.tx.send(PlaybackCommand::SkipForward(30)).ok();
                                app.set_message("+30 seconds".to_string(), false);
                            }
                            KeyCode::Char('b') => {
                                app.playback.tx.send(PlaybackCommand::SkipBackward(30)).ok();
                                app.set_message("-30 seconds".to_string(), false);
                            }
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

    let main_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(60),
            Constraint::Percentage(40),
        ])
        .split(size);

    let left_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(main_chunks[0]);

    let right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(40),
            Constraint::Percentage(30),
            Constraint::Percentage(30),
        ])
        .split(main_chunks[1]);

    let title = Paragraph::new("RustPlayer v0.1.0")
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::Cyan).bold());
    f.render_widget(title, left_chunks[0]);

    draw_library(f, app, left_chunks[1]);
    draw_status_bar(f, app, left_chunks[2]);

    draw_info_panel(f, app, right_chunks[0]);
    draw_recently_played(f, app, right_chunks[1]);
    draw_queue(f, app, right_chunks[2]);

    if app.show_help {
        draw_help_popup(f, size);
    }

    if app.show_devices {
        draw_device_popup(f, app, size);
    }

    if let Some((msg, _, is_volume)) = &app.message {
        if !is_volume || app.should_show_volume_notification() {
            draw_notification(f, size, msg.clone());
        }
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
                .unwrap_or_else(|| "Unknown".to_string());
            let track_type_icon = match &t.track_type {
                crate::core::track::TrackType::Local => "L",
                crate::core::track::TrackType::SoundCloud => "C",
            };

            let content = format!(
                "{:3}. [{}] {} - {} [{}]",
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
        .highlight_symbol("> ");

    f.render_stateful_widget(library, area, &mut app.library_state);
}

fn draw_recently_played(f: &mut Frame, app: &App, area: Rect) {
    let history = app.core.recently_played.lock().unwrap();
    let tracks = app.core.tracks.lock().unwrap();

    let items: Vec<ListItem> = history
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

    let history_list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" History ({}) ", history.len()))
                .border_style(Style::default().fg(Color::Yellow)),
        );

    f.render_widget(history_list, area);
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
        .highlight_symbol("> ");

    f.render_stateful_widget(queue_list, area, &mut app.queue_state);
}

fn draw_info_panel(f: &mut Frame, app: &App, area: Rect) {
    let tracks = app.core.tracks.lock().unwrap();
    let current_idx = *app.core.current.lock().unwrap();

    let mut lines = vec![];

    if let Some(idx) = current_idx {
        if let Some(track) = tracks.get(idx) {
            lines.push(Line::from(vec![
                Span::styled("Now Playing", Style::default().fg(Color::Yellow).bold()),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Title:  ", Style::default().fg(Color::Green)),
                Span::raw(&track.title),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Artist: ", Style::default().fg(Color::Cyan)),
                Span::raw(track.artist.clone().unwrap_or_else(|| "Unknown".to_string())),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Type:   ", Style::default().fg(Color::Magenta)),
                Span::raw(format!("{:?}", track.track_type)),
            ]));
        } else {
            lines.push(Line::from("Nothing playing"));
        }
    } else {
        lines.push(Line::from("Nothing playing"));
    }

    lines.push(Line::from(""));

    let volume = app.current_volume;
    let vol_percent = (volume * 100.0) as i32;
    let vol_color = if volume > 0.8 {
        Color::Red
    } else if volume > 0.5 {
        Color::Yellow
    } else {
        Color::Green
    };
    let vol_text = if volume == 0.0 {
        "MUTED".to_string()
    } else {
        format!("{}%", vol_percent)
    };
    lines.push(Line::from(vec![
        Span::styled("Volume: ", Style::default().fg(Color::Blue)),
        Span::styled(vol_text, Style::default().fg(vol_color)),
    ]));

    if let Some((name, _)) = app.devices.iter().find(|(_, is_current)| *is_current) {
        lines.push(Line::from(vec![
            Span::styled("Device: ", Style::default().fg(Color::Blue)),
            Span::raw(name),
        ]));
    }

    lines.push(Line::from(""));

    let position = app.playback.position_rx.lock().unwrap();
    if let Some((current, total, is_playing)) = *position {
        let ratio = if total > 0 {
            (current as f64 / total as f64).min(1.0)
        } else {
            0.0
        };
        let curr_min = current / 60;
        let curr_sec = current % 60;
        let tot_min = total / 60;
        let tot_sec = total % 60;
        let time_str = format!(
            "{:02}:{:02} / {:02}:{:02}",
            curr_min, curr_sec, tot_min, tot_sec
        );
        let status = if is_playing { ">" } else { "||" };
        lines.push(Line::from(vec![
            Span::styled("Progress: ", Style::default().fg(Color::Blue)),
            Span::raw(format!("{} {}", status, time_str)),
        ]));

        let progress_bar = "█".repeat((ratio * 20.0) as usize)
            + &"░".repeat(20 - (ratio * 20.0) as usize);
        lines.push(Line::from(vec![
            Span::raw(progress_bar),
        ]));
    }

    let info_panel = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Info ")
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .wrap(Wrap { trim: true });

    f.render_widget(info_panel, area);
}

fn draw_status_bar(f: &mut Frame, _app: &App, area: Rect) {
    let help_text = vec![
        Span::styled("Enter", Style::default().fg(Color::Green).bold()),
        Span::raw("Play "),
        Span::styled("Space", Style::default().fg(Color::Cyan).bold()),
        Span::raw("Pause "),
        Span::styled("Arrows", Style::default().fg(Color::Yellow).bold()),
        Span::raw("Seek "),
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
        Line::from("  j/k or Up/Down  - Navigate library"),
        Line::from("  J/K             - Navigate queue"),
        Line::from(""),
        Line::from(vec![
            Span::styled("Layout", Style::default().fg(Color::Yellow)),
        ]),
        Line::from("  Left: Library | Right: Info/History/Queue"),
        Line::from(""),
        Line::from(vec![
            Span::styled("Playback", Style::default().fg(Color::Yellow)),
        ]),
        Line::from("  Enter           - Play selected track"),
        Line::from("  Space           - Pause playback"),
        Line::from("  r               - Resume playback"),
        Line::from("  n               - Next track"),
        Line::from("  p               - Previous track"),
        Line::from("  Left/Right      - Seek -5s/+5s"),
        Line::from("  b/f             - Seek -30s/+30s"),
        Line::from("  a               - Add to queue"),
        Line::from(""),
        Line::from(vec![
            Span::styled("Volume", Style::default().fg(Color::Yellow)),
        ]),
        Line::from("  +/=             - Volume up"),
        Line::from("  -               - Volume down"),
        Line::from("  0               - Toggle mute"),
        Line::from(""),
        Line::from(vec![
            Span::styled("Device", Style::default().fg(Color::Yellow)),
        ]),
        Line::from("  d               - Toggle device list"),
        Line::from("  J/K             - Navigate devices"),
        Line::from("  D               - Select device"),
        Line::from(""),
        Line::from(vec![
            Span::styled("Other", Style::default().fg(Color::Yellow)),
        ]),
        Line::from("  ?/h             - Toggle this help"),
        Line::from("  q               - Quit"),
        Line::from("  Esc             - Close help/device"),
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

fn draw_device_popup(f: &mut Frame, app: &mut App, area: Rect) {
    let popup_area = centered_rect(50, 60, area);

    let items: Vec<ListItem> = app
        .devices
        .iter()
        .enumerate()
        .map(|(_idx, (name, is_current))| {
            let icon = if *is_current { "* " } else { "  " };
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
        .highlight_symbol("> ");

    f.render_widget(Clear, popup_area);
    f.render_stateful_widget(device_list, popup_area, &mut app.device_state);
}

fn draw_notification(f: &mut Frame, area: Rect, message: String) {
    let notif_width = (message.len() as u16).max(20).min(40);
    let notif_area = top_right_rect(notif_width, 3, area);

    let is_volume = message.contains('%') || message.contains("Volume") || message.contains("volume");
    let border_color = if is_volume {
        Color::Cyan
    } else {
        Color::Green
    };

    let msg = Paragraph::new(message)
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color)),
        );

    f.render_widget(Clear, notif_area);
    f.render_widget(msg, notif_area);
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

fn top_right_rect(width: u16, height: u16, r: Rect) -> Rect {
    let x = r.width.saturating_sub(width);
    let y = 1;

    Rect {
        x,
        y,
        width: width.min(r.width),
        height: height.min(r.height),
    }
}
