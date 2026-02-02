use crate::core::Core;
use crate::playback::{PlaybackHandle, PlaybackCommand};
use crossterm::event::{self, Event as CEvent, KeyCode};
use ratatui::{backend::CrosstermBackend, Terminal};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Gauge};
use ratatui::layout::{Layout, Constraint, Direction, Alignment};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Span, Spans};
use std::io;
use std::time::Duration;
use std::thread;

pub async fn run_ui(core: Core, playback: PlaybackHandle) -> anyhow::Result<()> {
    let stdout = io::stdout();
    crossterm::terminal::enable_raw_mode()?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let (tx, rx) = std::sync::mpsc::channel::<CEvent>();
    thread::spawn(move || {
        loop {
            if event::poll(Duration::from_millis(100)).unwrap_or(false) {
                if let Ok(ev) = event::read() {
                    if tx.send(ev).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let mut selected: usize = 0;

    loop {
        terminal.draw(|f| {
            let size = f.size();
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(3),
                    Constraint::Length(7),
                    Constraint::Length(3),
                    Constraint::Length(3)
                ])
                .split(size);

            let tracks = core.tracks.lock().unwrap();
            let current_idx = *core.current.lock().unwrap();
            
            let items: Vec<ListItem> = tracks.iter().enumerate().map(|(idx, t)| {
                let duration_str = if let Some(dur) = t.duration_seconds {
                    let mins = dur / 60;
                    let secs = dur % 60;
                    format!("{:02}:{:02}", mins, secs)
                } else {
                    "--:--".to_string()
                };
                
                let artist = t.artist.clone().unwrap_or_else(|| "Unknown Artist".to_string());
                let track_indicator = match &t.track_type {
                    crate::core::track::TrackType::Local => "L",
                    crate::core::track::TrackType::SoundCloud => "C",
                };
                let line = format!("{} {:3}. {} - {} [{}]", track_indicator, idx + 1, t.title, artist, duration_str);
                
                let mut style = Style::default();
                if Some(idx) == current_idx {
                    style = style.fg(Color::Green).add_modifier(Modifier::BOLD);
                }
                if idx == selected {
                    style = style.bg(Color::DarkGray);
                }
                
                ListItem::new(line).style(style)
            }).collect();
            let list = List::new(items)
                .block(Block::default()
                    .borders(Borders::ALL)
                    .title(" Library ")
                    .border_style(Style::default().fg(Color::Cyan)));
            f.render_widget(list, chunks[0]);

            // now playing / queue info
            let mut now_playing_text = vec![
                Spans::from(Span::styled("Now Playing:", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
            ];
            
            if let Some(idx) = current_idx {
                if let Some(track) = tracks.get(idx) {
                    now_playing_text.push(Spans::from(vec![
                        Span::styled("Title: ", Style::default().fg(Color::Green)),
                        Span::raw(&track.title),
                    ]));
                    now_playing_text.push(Spans::from(vec![
                        Span::styled("Artist: ", Style::default().fg(Color::Cyan)),
                        Span::raw(track.artist.clone().unwrap_or_else(|| "Unknown Artist".to_string())),
                    ]));
                } else {
                    now_playing_text.push(Spans::from("  Nothing"));
                }
            } else {
                now_playing_text.push(Spans::from("  Nothing"));
            }
            
            let q = core.queue.lock().unwrap();
            now_playing_text.push(Spans::from(""));
            now_playing_text.push(Spans::from(vec![
                Span::styled("Queue: ", Style::default().fg(Color::Magenta)),
                Span::raw(format!("{} track(s)", q.len())),
            ]));
            
            let now_playing = Paragraph::new(now_playing_text)
                .block(Block::default()
                    .borders(Borders::ALL)
                    .title(" Now Playing ")
                    .border_style(Style::default().fg(Color::Yellow)));
            f.render_widget(now_playing, chunks[1]);

            // Progress bar
            let position = playback.position_rx.lock().unwrap();
            let (progress_ratio, time_display) = if let Some((current, total)) = *position {
                let ratio = if total > 0 { (current as f64 / total as f64) } else { 0.0 };
                let curr_min = current / 60;
                let curr_sec = current % 60;
                let tot_min = total / 60;
                let tot_sec = total % 60;
                (ratio, format!("{:02}:{:02} / {:02}:{:02}", curr_min, curr_sec, tot_min, tot_sec))
            } else {
                (0.0, "00:00 / 00:00".to_string())
            };
            
            let progress_bar = Gauge::default()
                .block(Block::default()
                    .borders(Borders::ALL)
                    .title(" Progress ")
                    .border_style(Style::default().fg(Color::Green)))
                .gauge_style(Style::default().fg(Color::Green).bg(Color::Black))
                .label(time_display)
                .ratio(progress_ratio);
            f.render_widget(progress_bar, chunks[2]);

            let help = Paragraph::new(Spans::from(vec![
                Span::styled("Enter", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                Span::raw(" Play  "),
                Span::styled("Space", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                Span::raw(" Pause  "),
                Span::styled("r", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                Span::raw(" Resume  "),
                Span::styled("n", Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD)),
                Span::raw(" Next  "),
                Span::styled("j/k", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                Span::raw(" Nav  "),
                Span::styled("q", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                Span::raw(" Quit  "),
                Span::styled("L", Style::default().fg(Color::Green)),
                Span::raw(" Local "),
                Span::styled("C", Style::default().fg(Color::Blue)),
                Span::raw(" Cloud"),
            ]))
            .alignment(Alignment::Center)
            .block(Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Gray)));
            f.render_widget(help, chunks[3]);
        })?;

        if let Ok(ev) = rx.recv_timeout(Duration::from_millis(100)) {
            match ev {
                CEvent::Key(key) => {
                    match key.code {
                        KeyCode::Char('q') => {
                            playback.tx.send(PlaybackCommand::Quit).ok();
                            break;
                        }
                        KeyCode::Char('j') | KeyCode::Down => {
                            let len = core.tracks.lock().unwrap().len();
                            if len > 0 { selected = (selected + 1).min(len - 1); }
                        }
                        KeyCode::Char('k') | KeyCode::Up => {
                            if selected > 0 { selected -= 1; }
                        }
                        KeyCode::Enter => {
                            core.enqueue(selected);
                            playback.tx.send(PlaybackCommand::PlayIndex(selected)).ok();
                        }
                        KeyCode::Char(' ') => {
                            playback.tx.send(PlaybackCommand::Pause).ok();
                        }
                        KeyCode::Char('r') => {
                            playback.tx.send(PlaybackCommand::Resume).ok();
                        }
                        KeyCode::Char('n') => {
                            playback.tx.send(PlaybackCommand::Next).ok();
                        }
                        _ => {}
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
