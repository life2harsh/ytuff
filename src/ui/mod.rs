use crate::appdata::{AppConfig, AppPaths};
use crate::auth::{youtube_login_window, AuthSession};
use crate::core::track::{Acc, Track};
use crate::core::Core;
use crate::playback::{CollectionKind, PlaybackCommand, PlaybackHandle, RepeatMode};
use crate::sources::soundcloud::{build_auth_link, ScState, SoundCloudClient};
use crossterm::event::{self, Event as CEvent, KeyCode, KeyEventKind};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame, Terminal,
};
use std::cmp::min;
use std::fs;
use std::io::{self, Write};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub enum ScReq {
    Init,
    Home,
    Library,
    Browse(String, String, BrowseHint),
    ResolveCollection(String, String, CollectionKind, CollectionAction),
    Search(String),
    Suggest(String),
    Login,
    Logout,
    Art(String, String),
}

enum ScEvt {
    State(ScState),
    View(String, ViewKind, Result<Vec<Track>, String>),
    Collection(String, CollectionKind, CollectionAction, Result<Vec<Track>, String>),
    Suggest(String, Result<Vec<String>, String>),
    Art(String, Result<Vec<u8>, String>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BrowseHint {
    Artist,
    Playlist,
    Album,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ViewKind {
    Generic,
    Collection(CollectionKind),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CollectionAction {
    Play,
    Enqueue,
}

mod media;
use media::Media;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Local,
    Sc,
}

struct ScSrv {
    tx: Sender<ScReq>,
    rx: Receiver<ScEvt>,
}

pub struct App {
    core: Core,
    pb: PlaybackHandle,
    sc: ScSrv,
    img: Media,
    md: Mode,
    lib_st: ListState,
    sc_st: ListState,
    res_st: ListState,
    q_st: ListState,
    dev_st: ListState,
    fld_st: ListState,
    devs: Vec<(String, bool)>,
    show_help: bool,
    show_dev: bool,
    show_fld: bool,
    show_viz: bool,
    inp: bool,
    qry: String,
    sc_sugs: Vec<String>,
    sc_sug_st: ListState,
    sc_sug_due: Option<Instant>,
    sc_sug_last: String,
    lres: Vec<String>,
    lres_on: bool,
    sc_ids: Vec<String>,
    sc_title: String,
    sc_stack: Vec<(String, Vec<String>, ViewKind)>,
    sc_view_kind: ViewKind,
    sc_busy: bool,
    sc_info: ScState,
    fld: Option<usize>,
    msg: Option<(String, Instant)>,
    autoplay: bool,
    repeat_mode: RepeatMode,
    shuffle_on: bool,
    vol: f32,
    mute: bool,
    old_vol: f32,
    viz: Vec<f32>,
    art_box: Option<Rect>,
    logo_box: Option<Rect>,
    art_key: Option<String>,
}

impl App {
    fn new(core: Core, pb: PlaybackHandle, sc: ScSrv) -> Self {
        let mut lib_st = ListState::default();
        lib_st.select(Some(0));
        let mut sc_st = ListState::default();
        sc_st.select(Some(0));
        let mut res_st = ListState::default();
        res_st.select(Some(0));
        let mut sc_sug_st = ListState::default();
        sc_sug_st.select(Some(0));
        let mut q_st = ListState::default();
        q_st.select(Some(0));
        let mut dev_st = ListState::default();
        dev_st.select(Some(0));
        let mut fld_st = ListState::default();
        fld_st.select(Some(0));

        Self {
            md: if core.sc_on() { Mode::Sc } else { Mode::Local },
            core,
            pb,
            sc,
            img: Media::new(),
            lib_st,
            sc_st,
            res_st,
            q_st,
            dev_st,
            fld_st,
            devs: Vec::new(),
            show_help: false,
            show_dev: false,
            show_fld: false,
            show_viz: false,
            inp: false,
            qry: String::new(),
            sc_sugs: Vec::new(),
            sc_sug_st,
            sc_sug_due: None,
            sc_sug_last: String::new(),
            lres: Vec::new(),
            lres_on: false,
            sc_ids: Vec::new(),
            sc_title: "YouTube".to_string(),
            sc_stack: Vec::new(),
            sc_view_kind: ViewKind::Generic,
            sc_busy: false,
            sc_info: ScState::default(),
            fld: None,
            msg: None,
            autoplay: false,
            repeat_mode: RepeatMode::Off,
            shuffle_on: false,
            vol: 1.0,
            mute: false,
            old_vol: 1.0,
            viz: vec![0.0; 32],
            art_box: None,
            logo_box: None,
            art_key: None,
        }
    }

    fn note(&mut self, msg: impl Into<String>) {
        self.msg = Some((msg.into(), Instant::now()));
    }

    fn clean_msg(&mut self) {
        if self
            .msg
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() > Duration::from_secs(4))
        {
            self.msg = None;
        }
    }

    fn local_ids(&self) -> Vec<String> {
        self.core.ids_local()
    }

    fn list_ids(&self) -> Vec<String> {
        match self.md {
            Mode::Local => {
                if self.lres_on {
                    self.lres.clone()
                } else {
                    self.local_ids()
                }
            }
            Mode::Sc => self.sc_ids.clone(),
        }
    }

    fn pick_id(&self) -> Option<String> {
        match self.md {
            Mode::Local => {
                let ids = if self.lres_on {
                    &self.lres
                } else {
                    return self
                        .local_ids()
                        .get(self.lib_st.selected().unwrap_or(0))
                        .cloned();
                };
                ids.get(self.res_st.selected().unwrap_or(0)).cloned()
            }
            Mode::Sc => self.sc_ids.get(self.sc_st.selected().unwrap_or(0)).cloned(),
        }
    }

    fn collection_track_ids(&self) -> Option<Vec<String>> {
        let ViewKind::Collection(_) = self.sc_view_kind else {
            return None;
        };
        let ids = self
            .sc_ids
            .iter()
            .filter(|id| {
                self.core
                    .track(id)
                    .is_some_and(|track| track.is_playable_remote() && track.acc != Some(Acc::Block))
            })
            .cloned()
            .collect::<Vec<_>>();
        (!ids.is_empty()).then_some(ids)
    }

    fn selected_collection_index(&self) -> Option<usize> {
        let ids = self.collection_track_ids()?;
        let selected_id = self.pick_id()?;
        ids.iter().position(|id| id == &selected_id)
    }

    fn play_collection_ids(&mut self, ids: Vec<String>, start_index: usize, kind: CollectionKind) {
        let _ = self.pb.tx.send(PlaybackCommand::PlayCollection {
            ids,
            start_index,
            kind,
        });
    }

    fn play_sel(&mut self) {
        if self.md == Mode::Sc {
            if let (Some(kind), Some(ids), Some(start_index)) = (
                match self.sc_view_kind {
                    ViewKind::Collection(kind) => Some(kind),
                    ViewKind::Generic => None,
                },
                self.collection_track_ids(),
                self.selected_collection_index(),
            ) {
                self.play_collection_ids(ids, start_index, kind);
                return;
            }
        }
        if let Some(id) = self.pick_id() {
            let _ = self.pb.tx.send(PlaybackCommand::PlayTrack(id));
        }
    }

    fn activate_sel(&mut self) {
        if self.md == Mode::Sc {
            if let Some(track) = self.pick_track() {
                if track.is_remote_browse() {
                    self.open_browse_track(&track);
                    return;
                }
            }
        }
        self.play_sel();
    }

    fn add_q(&mut self) {
        if let Some(track) = self.pick_track() {
            if track.is_remote_browse() {
                self.note("use P to play or Q to queue this playlist or album");
                return;
            }
            let id = track.id;
            self.core.enqueue(id);
            self.note("queued");
        }
    }

    fn play_collection_action(&mut self) {
        if let Some(track) = self.pick_track() {
            if track.is_remote_browse() {
                if let Some(kind) = collection_kind_from_browse(&track) {
                    self.sc_busy = true;
                    let _ = self.sc.tx.send(ScReq::ResolveCollection(
                        track.browse_id().unwrap_or_default().to_string(),
                        track.title.clone(),
                        kind,
                        CollectionAction::Play,
                    ));
                } else {
                    self.note("open the artist page first");
                }
                return;
            }
        }

        let Some(kind) = self.current_collection_kind() else {
            self.note("open a playlist or album first");
            return;
        };
        let Some(ids) = self.collection_track_ids() else {
            self.note("no playable tracks were found in this collection");
            return;
        };
        self.play_collection_ids(ids, 0, kind);
    }

    fn queue_collection_action(&mut self) {
        if let Some(track) = self.pick_track() {
            if track.is_remote_browse() {
                if let Some(kind) = collection_kind_from_browse(&track) {
                    self.sc_busy = true;
                    let _ = self.sc.tx.send(ScReq::ResolveCollection(
                        track.browse_id().unwrap_or_default().to_string(),
                        track.title.clone(),
                        kind,
                        CollectionAction::Enqueue,
                    ));
                } else {
                    self.note("artists can be opened, but not queued as a collection");
                }
                return;
            }
        }

        let Some(ids) = self.collection_track_ids() else {
            self.note("open a playlist or album first");
            return;
        };
        for id in ids {
            self.core.enqueue(id);
        }
        self.note("collection queued");
    }

    fn pick_track(&self) -> Option<Track> {
        self.pick_id().and_then(|id| self.core.track(&id))
    }

    fn cur_track(&self) -> Option<Track> {
        self.core.cur_id().and_then(|id| self.core.track(&id))
    }

    fn next_item(&mut self) {
        let len = self.list_ids().len();
        if len == 0 {
            return;
        }
        let st = match self.md {
            Mode::Local if self.lres_on => &mut self.res_st,
            Mode::Local => &mut self.lib_st,
            Mode::Sc => &mut self.sc_st,
        };
        let i = st.selected().unwrap_or(0);
        st.select(Some((i + 1) % len));
        self.img.mark();
    }

    fn prev_item(&mut self) {
        let len = self.list_ids().len();
        if len == 0 {
            return;
        }
        let st = match self.md {
            Mode::Local if self.lres_on => &mut self.res_st,
            Mode::Local => &mut self.lib_st,
            Mode::Sc => &mut self.sc_st,
        };
        let i = st.selected().unwrap_or(0);
        st.select(Some(if i == 0 { len.saturating_sub(1) } else { i - 1 }));
        self.img.mark();
    }

    fn next_suggestion(&mut self) {
        let len = self.sc_sugs.len();
        if len == 0 {
            return;
        }
        let i = self.sc_sug_st.selected().unwrap_or(0);
        self.sc_sug_st.select(Some((i + 1) % len));
    }

    fn prev_suggestion(&mut self) {
        let len = self.sc_sugs.len();
        if len == 0 {
            return;
        }
        let i = self.sc_sug_st.selected().unwrap_or(0);
        self.sc_sug_st
            .select(Some(if i == 0 { len.saturating_sub(1) } else { i - 1 }));
    }

    fn selected_suggestion(&self) -> Option<String> {
        self.sc_sug_st
            .selected()
            .and_then(|i| self.sc_sugs.get(i))
            .cloned()
    }

    fn clear_suggestions(&mut self) {
        self.sc_sugs.clear();
        self.sc_sug_st.select(Some(0));
        self.sc_sug_due = None;
        self.sc_sug_last.clear();
    }

    fn schedule_suggestions(&mut self) {
        if self.md != Mode::Sc {
            return;
        }
        let trimmed = self.qry.trim();
        if trimmed.len() < 2 {
            self.clear_suggestions();
            return;
        }
        self.sc_sug_due = Some(Instant::now() + Duration::from_millis(140));
    }

    fn poll_suggestions(&mut self) {
        if self.md != Mode::Sc || !self.inp {
            return;
        }
        let Some(due) = self.sc_sug_due else {
            return;
        };
        if Instant::now() < due {
            return;
        }
        let query = self.qry.trim().to_string();
        self.sc_sug_due = None;
        if query.len() < 2 || query == self.sc_sug_last {
            return;
        }
        self.sc_sug_last = query.clone();
        let _ = self.sc.tx.send(ScReq::Suggest(query));
    }

    fn accept_suggestion(&mut self) {
        if let Some(suggestion) = self.selected_suggestion() {
            self.qry = suggestion;
            self.schedule_suggestions();
        }
    }

    fn next_q(&mut self) {
        let len = self.core.q_ids().len();
        if len == 0 {
            return;
        }
        let i = self.q_st.selected().unwrap_or(0);
        self.q_st.select(Some((i + 1) % len));
    }

    fn prev_q(&mut self) {
        let len = self.core.q_ids().len();
        if len == 0 {
            return;
        }
        let i = self.q_st.selected().unwrap_or(0);
        self.q_st
            .select(Some(if i == 0 { len.saturating_sub(1) } else { i - 1 }));
    }

    fn next_dev(&mut self) {
        let len = self.devs.len();
        if len == 0 {
            return;
        }
        let i = self.dev_st.selected().unwrap_or(0);
        self.dev_st.select(Some((i + 1) % len));
    }

    fn prev_dev(&mut self) {
        let len = self.devs.len();
        if len == 0 {
            return;
        }
        let i = self.dev_st.selected().unwrap_or(0);
        self.dev_st
            .select(Some(if i == 0 { len.saturating_sub(1) } else { i - 1 }));
    }

    fn next_fld(&mut self) {
        let len = self.core.scan_paths.lock().unwrap().len();
        if len == 0 {
            return;
        }
        let i = self.fld_st.selected().unwrap_or(0);
        self.fld_st.select(Some((i + 1) % len));
    }

    fn prev_fld(&mut self) {
        let len = self.core.scan_paths.lock().unwrap().len();
        if len == 0 {
            return;
        }
        let i = self.fld_st.selected().unwrap_or(0);
        self.fld_st
            .select(Some(if i == 0 { len.saturating_sub(1) } else { i - 1 }));
    }

    fn toggle_mode(&mut self) {
        self.md = if self.md == Mode::Local {
            Mode::Sc
        } else {
            Mode::Local
        };
        self.inp = false;
        self.show_fld = false;
        self.lres_on = false;
        self.qry.clear();
        self.clear_suggestions();
        self.img.mark();
        if self.md == Mode::Sc && self.sc_ids.is_empty() && !self.sc_busy {
            self.load_home(false);
        }
    }

    fn search(&mut self) {
        let q = self.qry.trim().to_string();
        if q.is_empty() {
            self.inp = false;
            self.clear_suggestions();
            return;
        }
        match self.md {
            Mode::Local => {
                let fld = self
                    .fld
                    .and_then(|i| self.core.scan_paths.lock().unwrap().get(i).cloned());
                let ql = q.to_ascii_lowercase();
                let ids = self
                    .local_ids()
                    .into_iter()
                    .filter(|id| {
                        self.core.track(id).is_some_and(|tr| {
                            let name = tr.title.to_ascii_lowercase();
                            let who = tr.who().to_ascii_lowercase();
                            let ok = name.contains(&ql) || who.contains(&ql);
                            let in_fld = if let Some(fld) = fld.as_ref() {
                                tr.path.as_ref().is_some_and(|p| p.starts_with(fld))
                            } else {
                                true
                            };
                            ok && in_fld
                        })
                    })
                    .collect::<Vec<_>>();
                self.lres = ids;
                self.lres_on = true;
                self.res_st.select(Some(0));
                self.inp = false;
                self.note(format!("{} match(es)", self.lres.len()));
            }
            Mode::Sc => {
                self.push_sc_view();
                self.sc_busy = true;
                let _ = self.sc.tx.send(ScReq::Search(q.clone()));
                self.inp = false;
                self.sc_sug_last = q;
                self.clear_suggestions();
            }
        }
    }

    fn sel_art(&self) -> Option<Track> {
        if self.md == Mode::Sc {
            if let Some(track) = self.pick_track().filter(|tr| tr.is_sc()) {
                return Some(track);
            }
        }

        self.cur_track().filter(|tr| tr.is_sc())
    }

    fn sync_art(&mut self) {
        let art = self.sel_art();
        let next = art.as_ref().map(|tr| tr.id.clone());
        if self.art_key != next {
            self.img.mark();
            self.art_key = next;
        }
        if let Some(tr) = art {
            if let Some(url) = tr.art.clone() {
                self.img.want(&tr.id, &url, &self.sc.tx);
            }
        }
    }

    fn preview_art(&mut self) -> anyhow::Result<()> {
        let Some(key) = self.art_key.as_deref() else {
            anyhow::bail!("No artwork is selected yet");
        };
        let bytes = self
            .img
            .art_bytes(key)
            .ok_or_else(|| anyhow::anyhow!("Artwork is not downloaded yet"))?;
        preview_bytes_with_wimg(&bytes)
    }

    fn set_sc_view(&mut self, title: impl Into<String>, tracks: Vec<Track>) {
        let ids = tracks.iter().map(|track| track.id.clone()).collect::<Vec<_>>();
        self.core.put_tracks(tracks);
        self.sc_title = title.into();
        self.sc_ids = ids;
        self.sc_st.select(Some(0));
        self.img.mark();
    }

    fn current_collection_kind(&self) -> Option<CollectionKind> {
        match self.sc_view_kind {
            ViewKind::Collection(kind) => Some(kind),
            ViewKind::Generic => None,
        }
    }

    fn push_sc_view(&mut self) {
        if !self.sc_ids.is_empty() {
            self.sc_stack.push((
                self.sc_title.clone(),
                self.sc_ids.clone(),
                self.sc_view_kind,
            ));
        }
    }

    fn pop_sc_view(&mut self) {
        if let Some((title, ids, view_kind)) = self.sc_stack.pop() {
            self.sc_title = title;
            self.sc_ids = ids;
            self.sc_view_kind = view_kind;
            self.sc_st.select(Some(0));
            self.img.mark();
        }
    }

    fn load_home(&mut self, push_stack: bool) {
        if push_stack {
            self.push_sc_view();
        } else {
            self.sc_stack.clear();
        }
        self.sc_busy = true;
        let _ = self.sc.tx.send(ScReq::Home);
    }

    fn load_library(&mut self, push_stack: bool) {
        if push_stack {
            self.push_sc_view();
        } else {
            self.sc_stack.clear();
        }
        self.sc_busy = true;
        let _ = self.sc.tx.send(ScReq::Library);
    }

    fn open_browse_track(&mut self, track: &Track) {
        let Some(browse_id) = track.browse_id() else {
            return;
        };
        self.push_sc_view();
        self.sc_busy = true;
        let _ = self
            .sc
            .tx
            .send(ScReq::Browse(
                browse_id.to_string(),
                track.title.clone(),
                browse_hint_from_track(track),
            ));
    }
}

fn browse_hint_from_track(track: &Track) -> BrowseHint {
    if track.is_artist_browse() {
        BrowseHint::Artist
    } else if track.is_album_browse() {
        BrowseHint::Album
    } else {
        BrowseHint::Playlist
    }
}

fn collection_kind_from_browse(track: &Track) -> Option<CollectionKind> {
    match browse_hint_from_track(track) {
        BrowseHint::Artist => None,
        BrowseHint::Playlist => Some(CollectionKind::Playlist),
        BrowseHint::Album => Some(CollectionKind::Album),
    }
}

fn view_kind_from_browse_hint(hint: BrowseHint) -> ViewKind {
    match hint {
        BrowseHint::Playlist => ViewKind::Collection(CollectionKind::Playlist),
        BrowseHint::Album => ViewKind::Collection(CollectionKind::Album),
        BrowseHint::Artist => ViewKind::Generic,
    }
}

pub fn run_ui(
    core: Core,
    pb: PlaybackHandle,
    sc_cli: Arc<Mutex<SoundCloudClient>>,
    playback_sc_cli: Arc<Mutex<SoundCloudClient>>,
    paths: AppPaths,
    cfg: Arc<Mutex<AppConfig>>,
) -> anyhow::Result<()> {
    let out = io::stdout();
    crossterm::terminal::enable_raw_mode()?;
    let backend = CrosstermBackend::new(out);
    let mut term = Terminal::new(backend)?;
    term.clear()?;

    let (tx, rx): (Sender<CEvent>, Receiver<CEvent>) = mpsc::channel();
    let input_paused = Arc::new(AtomicBool::new(false));
    let input_paused_bg = input_paused.clone();
    thread::spawn(move || loop {
        if input_paused_bg.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(20));
            continue;
        }
        if event::poll(Duration::from_millis(50)).unwrap_or(false) {
            if let Ok(ev) = event::read() {
                if tx.send(ev).is_err() {
                    break;
                }
            }
        }
    });

    let sc = start_sc(sc_cli, playback_sc_cli, paths, cfg);
    let mut app = App::new(core, pb, sc);
    let _ = app.sc.tx.send(ScReq::Init);
    let _ = app.pb.tx.send(PlaybackCommand::ListDevices);

    loop {
        app.clean_msg();
        while let Ok(list) = app.pb.devices_rx.try_recv() {
            app.devs = list;
        }
        while let Ok(autoplay) = app.pb.autoplay_rx.try_recv() {
            app.autoplay = autoplay;
        }
        while let Ok(repeat_mode) = app.pb.repeat_rx.try_recv() {
            app.repeat_mode = repeat_mode;
        }
        while let Ok(shuffle_on) = app.pb.shuffle_rx.try_recv() {
            app.shuffle_on = shuffle_on;
        }
        while let Ok(vol) = app.pb.volume_rx.try_recv() {
            app.vol = vol;
        }
        while let Ok(v) = app.pb.visualizer_rx.try_recv() {
            app.viz = v;
        }
        while let Ok(msg) = app.pb.msg_rx.try_recv() {
            app.note(msg);
        }
        while let Ok(ev) = app.sc.rx.try_recv() {
            match ev {
                ScEvt::State(st) => {
                    app.sc_busy = false;
                    if let Some(msg) = st.msg.clone() {
                        app.note(msg);
                    }
                    app.sc_info = st;
                }
                ScEvt::View(title, view_kind, res) => {
                    app.sc_busy = false;
                    match res {
                        Ok(list) => {
                            let count = list.len();
                            app.set_sc_view(title, list);
                            app.sc_view_kind = view_kind;
                            app.note(format!("{} YouTube item(s)", count));
                        }
                        Err(e) => {
                            app.sc_view_kind = ViewKind::Generic;
                            app.note(e);
                        }
                    }
                }
                ScEvt::Collection(title, kind, action, res) => {
                    app.sc_busy = false;
                    match res {
                        Ok(list) => {
                            let ids = list
                                .iter()
                                .filter(|track| {
                                    track.is_playable_remote() && track.acc != Some(Acc::Block)
                                })
                                .map(|track| track.id.clone())
                                .collect::<Vec<_>>();
                            app.core.put_tracks(list.clone());

                            if ids.is_empty() {
                                app.note("no playable tracks were found in that collection");
                                continue;
                            }

                            match action {
                                CollectionAction::Play => {
                                    app.set_sc_view(title, list);
                                    app.sc_view_kind = ViewKind::Collection(kind);
                                    app.play_collection_ids(ids, 0, kind);
                                    app.note("collection started");
                                }
                                CollectionAction::Enqueue => {
                                    for id in ids {
                                        app.core.enqueue(id);
                                    }
                                    app.note("collection queued");
                                }
                            }
                        }
                        Err(e) => app.note(e),
                    }
                }
                ScEvt::Suggest(query, res) => {
                    if app.inp && query == app.qry.trim() {
                        match res {
                            Ok(list) => {
                                app.sc_sugs = list;
                                app.sc_sug_st.select(Some(0));
                            }
                            Err(_) => app.clear_suggestions(),
                        }
                    }
                }
                ScEvt::Art(key, dat) => {
                    app.img.put(key, dat);
                }
            }
        }

        app.poll_suggestions();
        app.sync_art();
        term.draw(|f| draw_ui(f, &mut app))?;

        let hide_img = app.show_help || app.show_dev || app.show_fld || app.inp;
        let cov = match (app.art_box, app.art_key.as_deref()) {
            (Some(rect), Some(key)) => Some((key, rect)),
            (Some(rect), None) => Some(("", rect)),
            _ => None,
        };
        app.img.draw(cov, app.logo_box, false, hide_img)?;

        if let Ok(ev) = rx.recv_timeout(Duration::from_millis(50)) {
            match ev {
                CEvent::Key(key) if key.kind == KeyEventKind::Press => {
                    if app.show_help {
                        match key.code {
                            KeyCode::Esc
                            | KeyCode::Char('q')
                            | KeyCode::Char('?')
                            | KeyCode::Char('h') => {
                                app.show_help = false;
                                app.img.mark();
                            }
                            _ => {}
                        }
                        continue;
                    }

                    if app.show_dev {
                        match key.code {
                            KeyCode::Esc | KeyCode::Char('d') => {
                                app.show_dev = false;
                                app.img.mark();
                            }
                            KeyCode::Char('q') => {
                                let _ = app.pb.tx.send(PlaybackCommand::Quit);
                                break;
                            }
                            KeyCode::Char('j') | KeyCode::Down => app.next_dev(),
                            KeyCode::Char('k') | KeyCode::Up => app.prev_dev(),
                            KeyCode::Enter => {
                                if let Some(i) = app.dev_st.selected() {
                                    if let Some((name, _)) = app.devs.get(i) {
                                        let _ = app
                                            .pb
                                            .tx
                                            .send(PlaybackCommand::SwitchDevice(name.clone()));
                                        app.note(format!("device {}", name));
                                    }
                                }
                                app.show_dev = false;
                                app.img.mark();
                            }
                            _ => {}
                        }
                        continue;
                    }

                    if app.show_fld {
                        match key.code {
                            KeyCode::Esc | KeyCode::Char('F') => {
                                app.show_fld = false;
                                app.img.mark();
                            }
                            KeyCode::Char('q') => {
                                let _ = app.pb.tx.send(PlaybackCommand::Quit);
                                break;
                            }
                            KeyCode::Char('j') | KeyCode::Down => app.next_fld(),
                            KeyCode::Char('k') | KeyCode::Up => app.prev_fld(),
                            KeyCode::Char('d') => {
                                if let Some(i) = app.fld_st.selected() {
                                    let _ = app.core.remove_scan_path(i);
                                }
                            }
                            KeyCode::Enter => {
                                let ps = app.core.scan_paths.lock().unwrap();
                                app.fld = if ps.is_empty() {
                                    None
                                } else {
                                    app.fld_st.selected()
                                };
                                drop(ps);
                                app.show_fld = false;
                                app.inp = true;
                                app.img.mark();
                            }
                            KeyCode::Char('a') => {
                                app.fld = None;
                                app.show_fld = false;
                                app.inp = true;
                                app.img.mark();
                            }
                            _ => {}
                        }
                        continue;
                    }

                    if app.inp {
                        match key.code {
                            KeyCode::Esc => {
                                app.inp = false;
                                app.qry.clear();
                                app.clear_suggestions();
                                app.img.mark();
                            }
                            KeyCode::Enter => app.search(),
                            KeyCode::Tab | KeyCode::Right if app.md == Mode::Sc => {
                                app.accept_suggestion();
                            }
                            KeyCode::Down if app.md == Mode::Sc => app.next_suggestion(),
                            KeyCode::Up if app.md == Mode::Sc => app.prev_suggestion(),
                            KeyCode::Backspace => {
                                app.qry.pop();
                                app.schedule_suggestions();
                            }
                            KeyCode::Char(c) => {
                                app.qry.push(c);
                                app.schedule_suggestions();
                            }
                            _ => {}
                        }
                        continue;
                    }

                    match key.code {
                        KeyCode::Char('q') => {
                            let _ = app.pb.tx.send(PlaybackCommand::Quit);
                            break;
                        }
                        KeyCode::Char('s') => app.toggle_mode(),
                        KeyCode::Char('?') | KeyCode::Char('h') => {
                            app.show_help = true;
                            app.img.mark();
                        }
                        KeyCode::Char('d') => {
                            app.show_dev = true;
                            let _ = app.pb.tx.send(PlaybackCommand::ListDevices);
                            app.img.mark();
                        }
                        KeyCode::Char('F') => {
                            if app.md == Mode::Local {
                                app.show_fld = true;
                                app.img.mark();
                            }
                        }
                        KeyCode::Char('/') => {
                            if app.md == Mode::Local
                                && !app.core.scan_paths.lock().unwrap().is_empty()
                            {
                                app.show_fld = true;
                                app.img.mark();
                            } else {
                                app.inp = true;
                                app.qry.clear();
                                app.clear_suggestions();
                                app.img.mark();
                            }
                        }
                        KeyCode::Char('l') => {
                            if app.md == Mode::Sc {
                                if app.sc_info.user {
                                    app.note("signing out of YouTube");
                                } else {
                                    app.note("opening the YouTube login window");
                                }
                                app.sc_busy = true;
                                let _ = app.sc.tx.send(if app.sc_info.user {
                                    ScReq::Logout
                                } else {
                                    ScReq::Login
                                });
                            }
                        }
                        KeyCode::Char('g') => {
                            if app.md == Mode::Sc {
                                app.load_home(false);
                            }
                        }
                        KeyCode::Char('m') => {
                            if app.md == Mode::Sc {
                                app.load_library(false);
                            }
                        }
                        KeyCode::Char('u') | KeyCode::Backspace => {
                            if app.md == Mode::Sc {
                                app.pop_sc_view();
                            }
                        }
                        KeyCode::Char('o') => {
                            if let Some(tr) = app.sel_art() {
                                if let Some(link) = tr.link {
                                    let _ = webbrowser::open(&build_auth_link(&link));
                                    app.note("opened in browser");
                                }
                            }
                        }
                        KeyCode::Char('i') => {
                            input_paused.store(true, Ordering::Relaxed);
                            thread::sleep(Duration::from_millis(70));
                            let res = app.preview_art();
                            let _ = term.clear();
                            let _ = term.hide_cursor();
                            input_paused.store(false, Ordering::Relaxed);
                            match res {
                                Ok(()) => app.note("art preview closed"),
                                Err(err) => app.note(err.to_string()),
                            }
                            app.img.mark();
                        }
                        KeyCode::Char(' ') => {
                            let _ = app.pb.tx.send(PlaybackCommand::Pause);
                        }
                        KeyCode::Char('r') => {
                            let _ = app.pb.tx.send(PlaybackCommand::Resume);
                        }
                        KeyCode::Char('n') => {
                            let _ = app.pb.tx.send(PlaybackCommand::Next);
                        }
                        KeyCode::Char('p') => {
                            let _ = app.pb.tx.send(PlaybackCommand::Prev);
                        }
                        KeyCode::Char('v') => {
                            app.show_viz = !app.show_viz;
                            let _ = app.pb.tx.send(PlaybackCommand::ToggleVisualizer);
                        }
                        KeyCode::Char('A') => {
                            let _ = app.pb.tx.send(PlaybackCommand::ToggleAutoplay);
                        }
                        KeyCode::Char('R') => {
                            let _ = app.pb.tx.send(PlaybackCommand::ToggleRepeat);
                        }
                        KeyCode::Char('z') => {
                            let _ = app.pb.tx.send(PlaybackCommand::ToggleShuffle);
                        }
                        KeyCode::Char('a') => app.add_q(),
                        KeyCode::Char('P') => app.play_collection_action(),
                        KeyCode::Char('Q') => app.queue_collection_action(),
                        KeyCode::Enter => app.activate_sel(),
                        KeyCode::Char('j') | KeyCode::Down => app.next_item(),
                        KeyCode::Char('k') | KeyCode::Up => app.prev_item(),
                        KeyCode::Char('J') => app.next_q(),
                        KeyCode::Char('K') => app.prev_q(),
                        KeyCode::Char('+') | KeyCode::Char('=') => {
                            let _ = app.pb.tx.send(PlaybackCommand::VolumeUp);
                            app.note("volume up");
                        }
                        KeyCode::Char('-') => {
                            let _ = app.pb.tx.send(PlaybackCommand::VolumeDown);
                            app.note("volume down");
                        }
                        KeyCode::Char('0') => {
                            let _ = app.pb.tx.send(PlaybackCommand::ToggleMute);
                            if app.mute {
                                app.mute = false;
                                app.vol = app.old_vol;
                                app.note("unmuted");
                            } else {
                                app.old_vol = app.vol;
                                app.mute = true;
                                app.note("muted");
                            }
                        }
                        KeyCode::Right => {
                            let _ = app.pb.tx.send(PlaybackCommand::SkipForward(5));
                            app.note("+5s");
                        }
                        KeyCode::Left => {
                            let _ = app.pb.tx.send(PlaybackCommand::SkipBackward(5));
                            app.note("-5s");
                        }
                        KeyCode::Char('f') => {
                            let _ = app.pb.tx.send(PlaybackCommand::SkipForward(30));
                            app.note("+30s");
                        }
                        KeyCode::Char('b') => {
                            let _ = app.pb.tx.send(PlaybackCommand::SkipBackward(30));
                            app.note("-30s");
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    crossterm::terminal::disable_raw_mode()?;
    term.show_cursor()?;
    Ok(())
}

fn apply_auth_session(
    clients: &[Arc<Mutex<SoundCloudClient>>],
    cfg: &Arc<Mutex<AppConfig>>,
    paths: &AppPaths,
    session: AuthSession,
) -> Result<ScState, String> {
    let mut verified = clients
        .first()
        .ok_or_else(|| "No YouTube client is available".to_string())?
        .lock()
        .unwrap()
        .clone();
    verified.set_cookie_header(Some(session.cookie_header.clone()));
    verified.set_auth_user(session.auth_user.clone());
    let state = verified.login().map_err(|err| err.to_string())?;

    {
        let mut cfg = cfg.lock().unwrap();
        cfg.youtube_cookie_header = Some(session.cookie_header);
        cfg.youtube_cookie_file = None;
        cfg.youtube_auth_user = session.auth_user;
        cfg.save(paths).map_err(|err| err.to_string())?;
    }

    for client in clients {
        *client.lock().unwrap() = verified.clone();
    }
    Ok(state)
}

fn clear_auth_session(
    clients: &[Arc<Mutex<SoundCloudClient>>],
    cfg: &Arc<Mutex<AppConfig>>,
    paths: &AppPaths,
) -> Result<ScState, String> {
    {
        let mut cfg = cfg.lock().unwrap();
        cfg.youtube_cookie_header = None;
        cfg.youtube_cookie_file = None;
        cfg.youtube_auth_user = None;
        cfg.save(paths).map_err(|err| err.to_string())?;
    }

    let first = clients
        .first()
        .ok_or_else(|| "No YouTube client is available".to_string())?;
    let state = first.lock().unwrap().logout().map_err(|err| err.to_string())?;
    for client in clients.iter().skip(1) {
        let _ = client.lock().unwrap().logout();
    }
    Ok(state)
}

fn start_sc(
    sc: Arc<Mutex<SoundCloudClient>>,
    playback_sc: Arc<Mutex<SoundCloudClient>>,
    paths: AppPaths,
    cfg: Arc<Mutex<AppConfig>>,
) -> ScSrv {
    let (tx, rx) = mpsc::channel::<ScReq>();
    let (etx, erx) = mpsc::channel::<ScEvt>();
    thread::spawn(move || {
        while let Ok(cmd) = rx.recv() {
            match cmd {
                ScReq::Init => {
                    let mut client = sc.lock().unwrap();
                    let st = client.state();
                    let _ = etx.send(ScEvt::State(st));
                    let res = client.account_feed(28, 12).map_err(|e| e.to_string());
                    let _ = etx.send(ScEvt::View("Home".to_string(), ViewKind::Generic, res));
                }
                ScReq::Home => {
                    let res = sc
                        .lock()
                        .unwrap()
                        .account_feed(28, 12)
                        .map_err(|e| e.to_string());
                    let _ = etx.send(ScEvt::View("Home".to_string(), ViewKind::Generic, res));
                }
                ScReq::Library => {
                    let res = sc
                        .lock()
                        .unwrap()
                        .library_playlists(40)
                        .map_err(|e| e.to_string());
                    let _ = etx.send(ScEvt::View(
                        "My Playlists".to_string(),
                        ViewKind::Generic,
                        res,
                    ));
                }
                ScReq::Browse(id, title, hint) => {
                    let res = sc
                        .lock()
                        .unwrap()
                        .browse_page(&id, 120)
                        .map_err(|e| e.to_string());
                    match res {
                        Ok((resolved_title, items)) => {
                            if items.is_empty() {
                                let _ = etx.send(ScEvt::View(
                                    title,
                                    ViewKind::Generic,
                                    Err("No playable items found".to_string()),
                                ));
                                continue;
                            }
                            let view_title = if resolved_title.trim().is_empty() {
                                title
                            } else {
                                resolved_title
                            };
                            let _ = etx.send(ScEvt::View(
                                view_title,
                                view_kind_from_browse_hint(hint),
                                Ok(items),
                            ));
                        }
                        Err(err) => {
                            let _ = etx.send(ScEvt::View(title, ViewKind::Generic, Err(err)));
                        }
                    }
                }
                ScReq::ResolveCollection(id, title, kind, action) => {
                    let res = sc
                        .lock()
                        .unwrap()
                        .browse_page(&id, 120)
                        .map(|(resolved_title, items)| {
                            let view_title = if resolved_title.trim().is_empty() {
                                title.clone()
                            } else {
                                resolved_title
                            };
                            (view_title, items)
                        })
                        .map_err(|e| e.to_string());
                    match res {
                        Ok((view_title, items)) => {
                            if items.is_empty() {
                                let _ = etx.send(ScEvt::Collection(
                                    title,
                                    kind,
                                    action,
                                    Err("No playable items found".to_string()),
                                ));
                                continue;
                            }
                            let _ = etx.send(ScEvt::Collection(view_title, kind, action, Ok(items)));
                        }
                        Err(err) => {
                            let _ = etx.send(ScEvt::Collection(title, kind, action, Err(err)));
                        }
                    }
                }
                ScReq::Search(q) => {
                    let res = sc
                        .lock()
                        .unwrap()
                        .search_catalog(&q, 48)
                        .map_err(|e| e.to_string());
                    let st = sc.lock().unwrap().state();
                    let _ = etx.send(ScEvt::View(
                        format!("Search: {}", q),
                        ViewKind::Generic,
                        res,
                    ));
                    let _ = etx.send(ScEvt::State(st));
                }
                ScReq::Suggest(q) => {
                    let res = sc
                        .lock()
                        .unwrap()
                        .search_suggestions(&q, 8)
                        .map_err(|e| e.to_string());
                    let _ = etx.send(ScEvt::Suggest(q, res));
                }
                ScReq::Login => {
                    let clients = [sc.clone(), playback_sc.clone()];
                    let res = youtube_login_window(&paths)
                        .map_err(|e| e.to_string())
                        .and_then(|session| apply_auth_session(&clients, &cfg, &paths, session));
                    match res {
                        Ok(st) => {
                            let _ = etx.send(ScEvt::State(st));
                            let res = sc
                                .lock()
                                .unwrap()
                                .account_feed(28, 12)
                                .map_err(|e| e.to_string());
                            let _ = etx.send(ScEvt::View(
                                "Home".to_string(),
                                ViewKind::Generic,
                                res,
                            ));
                        }
                        Err(e) => {
                            let mut st = sc.lock().unwrap().state();
                            st.msg = Some(e);
                            let _ = etx.send(ScEvt::State(st));
                        }
                    }
                }
                ScReq::Logout => {
                    let clients = [sc.clone(), playback_sc.clone()];
                    let res = clear_auth_session(&clients, &cfg, &paths);
                    match res {
                        Ok(st) => {
                            let _ = etx.send(ScEvt::State(st));
                            let res = sc
                                .lock()
                                .unwrap()
                                .account_feed(28, 12)
                                .map_err(|e| e.to_string());
                            let _ = etx.send(ScEvt::View(
                                "Home".to_string(),
                                ViewKind::Generic,
                                res,
                            ));
                        }
                        Err(e) => {
                            let mut st = sc.lock().unwrap().state();
                            st.msg = Some(e);
                            let _ = etx.send(ScEvt::State(st));
                        }
                    }
                }
                ScReq::Art(key, url) => {
                    let dat = sc.lock().unwrap().art(&url).map_err(|e| e.to_string());
                    let _ = etx.send(ScEvt::Art(key, dat));
                }
            }
        }
    });
    ScSrv { tx, rx: erx }
}

fn draw_ui(f: &mut Frame, app: &mut App) {
    let size = f.area();
    f.render_widget(
        Block::default().style(
            Style::default()
                .bg(Color::Rgb(20, 25, 31))
                .fg(Color::Rgb(214, 220, 229)),
        ),
        size,
    );
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(2),
        ])
        .split(size);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
        .split(root[1]);

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(44),
            Constraint::Percentage(31),
            Constraint::Percentage(25),
        ])
        .split(body[1]);

    draw_head(f, app, root[0]);
    draw_list_box(f, app, body[0]);
    draw_art_box(f, app, right[0]);
    draw_info_box(f, app, right[1]);
    draw_tail_box(f, app, right[2]);
    draw_foot(f, app, root[2]);

    if app.show_help {
        draw_help(f, size);
    }
    if app.show_dev {
        draw_dev(f, app, size);
    }
    if app.show_fld {
        draw_fld(f, app, size);
    }
    if app.inp {
        draw_inp(f, app, size);
    }
}

fn draw_head(f: &mut Frame, app: &App, area: Rect) {
    let md = if app.md == Mode::Local {
        Span::styled(
            " LOCAL ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Rgb(117, 214, 255))
                .bold(),
        )
    } else {
        Span::styled(" LOCAL ", Style::default().fg(Color::Rgb(104, 113, 124)))
    };
    let sc = if app.md == Mode::Sc {
        Span::styled(
            " YOUTUBE ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Rgb(255, 131, 116))
                .bold(),
        )
    } else {
        Span::styled(" YOUTUBE ", Style::default().fg(Color::Rgb(104, 113, 124)))
    };
    let auth = if app.sc_info.ready {
        if app.sc_info.user {
            format!(
                "signed in as {}",
                app.sc_info.name.clone().unwrap_or_else(|| "user".into())
            )
        } else {
            "youtube ready".to_string()
        }
    } else {
        "youtube unavailable".to_string()
    };
    let autoplay = if app.autoplay {
        "autoplay on"
    } else {
        "autoplay off"
    };
    let q = if app.inp {
        format!("  / {}", app.qry)
    } else if app.sc_busy {
        "  loading...".to_string()
    } else {
        String::new()
    };
    let line = Line::from(vec![
        md,
        Span::raw(" "),
        sc,
        Span::raw("   "),
        Span::styled(
            format!("RustPlayer :: {}", app.sc_title),
            Style::default().fg(Color::Rgb(240, 244, 250)).bold(),
        ),
        Span::raw("   "),
        Span::styled(auth, Style::default().fg(Color::Rgb(170, 178, 190))),
        Span::raw("   "),
        Span::styled(
            autoplay,
            Style::default().fg(if app.autoplay {
                Color::Rgb(138, 219, 109)
            } else {
                Color::Rgb(104, 113, 124)
            }),
        ),
        Span::raw("   "),
        Span::styled(
            " ? HELP ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Rgb(238, 204, 100))
                .bold(),
        ),
        Span::raw(q),
    ]);
    let p = Paragraph::new(line)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .style(Style::default().bg(Color::Rgb(24, 31, 39)))
                .border_style(Style::default().fg(Color::Rgb(92, 104, 116))),
        )
        .alignment(Alignment::Left);
    f.render_widget(p, area);
}

fn draw_list_box(f: &mut Frame, app: &mut App, area: Rect) {
    let ids = app.list_ids();
    let trs = app.core.tracks_of(&ids);
    let cur = app.core.cur_id();
    let mut items = Vec::new();
    for (i, tr) in trs.iter().enumerate() {
        let mut txt = if tr.is_remote_browse() {
            format!("{:>3}. [{}] {} - {}", i + 1, tr.tag(), tr.title, tr.who())
        } else {
            let dur = tr
                .dur
                .map(|v| format!("{:02}:{:02}", v / 60, v % 60))
                .unwrap_or_else(|| "--:--".to_string());
            format!(
                "{:>3}. [{}] {} - {} [{}]",
                i + 1,
                tr.tag(),
                tr.title,
                tr.who(),
                dur
            )
        };
        if tr.is_playable_remote() && !tr.acc_tag().is_empty() {
            txt.push_str(&format!(" <{}>", tr.acc_tag()));
        } else if tr.is_remote_browse() {
            txt.push_str(if tr.is_artist_browse() {
                " <artist>"
            } else if tr.is_album_browse() {
                " <album>"
            } else {
                " <open>"
            });
        }
        let mut st = Style::default();
        if cur.as_ref() == Some(&tr.id) {
            st = st
                .fg(Color::Rgb(170, 255, 170))
                .add_modifier(Modifier::BOLD);
        }
        items.push(ListItem::new(txt).style(st));
    }
    if items.is_empty() {
        let empty = match app.md {
            Mode::Local => "No local tracks found. Add a folder with --path or press / to search.",
            Mode::Sc if app.sc_busy => "Searching YouTube Music...",
            Mode::Sc if app.sc_info.user => {
                "Press g for Home, m for My Playlists, / to search, Enter to open playlists."
            }
            Mode::Sc => "Press g for Home, / to search, or sign in with l for account playlists.",
        };
        items.push(ListItem::new(empty).style(Style::default().fg(Color::Rgb(104, 113, 124))));
    }

    let ttl = match app.md {
        Mode::Local if app.lres_on => format!(" Search ({}) ", ids.len()),
        Mode::Local => format!(" Library ({}) ", ids.len()),
        Mode::Sc => format!(" {} ({}) ", app.sc_title, ids.len()),
    };
    let br = if app.md == Mode::Sc {
        Color::LightRed
    } else {
        Color::Cyan
    };
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(ttl)
                .style(Style::default().bg(Color::Rgb(27, 34, 42)))
                .border_style(Style::default().fg(br)),
        )
        .highlight_style(
            Style::default()
                .bg(Color::Rgb(38, 46, 57))
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    let st = match app.md {
        Mode::Local if app.lres_on => &mut app.res_st,
        Mode::Local => &mut app.lib_st,
        Mode::Sc => &mut app.sc_st,
    };
    f.render_stateful_widget(list, area, st);
}

fn draw_art_box(f: &mut Frame, app: &mut App, area: Rect) {
    let ttl = " Artwork ";
    let br = if app.md == Mode::Sc {
        Color::LightRed
    } else {
        Color::Yellow
    };
    let box_w = Block::default()
        .borders(Borders::ALL)
        .title(ttl)
        .style(Style::default().bg(Color::Rgb(27, 34, 42)))
        .border_style(Style::default().fg(br));
    f.render_widget(box_w, area);

    let inner = inner(area);
    if !app.img.on() {
        draw_art_hint(
            f,
            inner,
            &[
                "Artwork disabled",
                "set RUSTPLAYER_ART=blocks",
                "or set RUSTPLAYER_ART=sixel",
            ],
            Alignment::Left,
        );
    } else if app.art_key.is_none() {
        let msg = if app.md == Mode::Sc {
            "Select a YouTube item to show artwork"
        } else {
            "Artwork will appear here"
        };
        let renderer = format!("renderer: {}", app.img.renderer_label());
        let lines = [msg, renderer.as_str()];
        draw_art_hint(f, inner, &lines, Alignment::Center);
    }

    app.art_box = Some(inner);
    let lw = min(18, inner.width.saturating_sub(1)).max(8);
    let lh = min(4, inner.height.saturating_sub(1)).max(2);
    app.logo_box = Some(Rect {
        x: inner.x.saturating_add(inner.width.saturating_sub(lw)),
        y: inner.y,
        width: lw,
        height: lh,
    });
}

fn draw_art_hint(f: &mut Frame, area: Rect, lines: &[&str], alignment: Alignment) {
    if area.width < 6 || area.height < 2 || lines.is_empty() {
        return;
    }

    let max_width = lines
        .iter()
        .map(|line| line.len() as u16)
        .max()
        .unwrap_or(0)
        .saturating_add(2)
        .min(area.width);
    let height = (lines.len() as u16).min(area.height);
    let y = if matches!(alignment, Alignment::Center) {
        area.y
            .saturating_add(area.height.saturating_sub(height) / 2)
    } else {
        area.y.saturating_add(area.height.saturating_sub(height))
    };
    let rect = Rect {
        x: area.x,
        y,
        width: max_width.max(6),
        height,
    };
    let text = Text::from(
        lines
            .iter()
            .map(|line| {
                Line::from(Span::styled(
                    (*line).to_string(),
                    Style::default().fg(Color::Rgb(104, 113, 124)),
                ))
            })
            .collect::<Vec<_>>(),
    );
    let p = Paragraph::new(text)
        .alignment(alignment)
        .wrap(Wrap { trim: true });
    f.render_widget(p, rect);
}

fn draw_info_box(f: &mut Frame, app: &App, area: Rect) {
    let mut lines = Vec::new();
    if let Some(tr) = app.cur_track() {
        let (state_label, state_color) = playback_state_label(
            app.pb
                .position_rx
                .lock()
                .unwrap()
                .is_some_and(|(_, _, on)| on),
        );
        lines.push(Line::from(Span::styled(
            format!(" {} ", state_label),
            Style::default()
                .fg(Color::Black)
                .bg(state_color)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            trim(&tr.title, area.width.saturating_sub(4) as usize),
            Style::default().fg(Color::Rgb(240, 244, 250)).bold(),
        )));
        lines.push(Line::from(Span::styled(
            tr.who(),
            Style::default().fg(Color::Rgb(170, 178, 190)),
        )));

        let src = if tr.is_sc() { "YTMusic" } else { "Local" };
        let vol = if app.vol == 0.0 {
            "Muted".to_string()
        } else {
            format!("{}%", (app.vol * 100.0) as i32)
        };
        let mut meta = vec![
            pill(src, Color::Magenta),
            Span::raw(" "),
            pill(&app.sc_info.ql.to_ascii_uppercase(), Color::Blue),
            Span::raw(" "),
            pill(
                &vol,
                if app.vol == 0.0 {
                    Color::Red
                } else {
                    Color::Green
                },
            ),
            Span::raw(" "),
            pill(
                if app.autoplay { "AUTOPLAY" } else { "MANUAL" },
                if app.autoplay {
                    Color::LightGreen
                } else {
                    Color::DarkGray
                },
            ),
            Span::raw(" "),
            pill(
                match app.repeat_mode {
                    RepeatMode::Off => "REPEAT OFF",
                    RepeatMode::All => "REPEAT ALL",
                    RepeatMode::One => "REPEAT ONE",
                },
                match app.repeat_mode {
                    RepeatMode::Off => Color::DarkGray,
                    RepeatMode::All => Color::LightBlue,
                    RepeatMode::One => Color::Yellow,
                },
            ),
            Span::raw(" "),
            pill(
                if app.shuffle_on { "SHUFFLE ON" } else { "SHUFFLE OFF" },
                if app.shuffle_on {
                    Color::LightCyan
                } else {
                    Color::DarkGray
                },
            ),
        ];
        if tr.is_sc() {
            let acc = match tr.acc {
                Some(Acc::Play) => ("PLAY", Color::LightGreen),
                Some(Acc::Prev) => ("PREVIEW", Color::Yellow),
                Some(Acc::Block) => ("BLOCKED", Color::Red),
                None => ("UNKNOWN", Color::DarkGray),
            };
            meta.push(Span::raw(" "));
            meta.push(pill(acc.0, acc.1));
        }
        lines.push(Line::from(meta));

        if let Some((cur, tot, on)) = *app.pb.position_rx.lock().unwrap() {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled(
                    if on { ">" } else { "||" },
                    Style::default()
                        .fg(if on { Color::LightGreen } else { Color::Yellow })
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(
                    format!("{} / {}", clock(cur), clock(tot)),
                    Style::default().fg(Color::Rgb(229, 234, 241)),
                ),
            ]));
            lines.push(Line::from(Span::styled(
                bar(cur, tot, area.width.saturating_sub(4) as usize),
                Style::default().fg(Color::Rgb(255, 131, 116)),
            )));
        }

        if tr.is_sc() {
            if let Some(link) = tr.link {
                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    Span::styled("Link ", Style::default().fg(Color::Rgb(104, 113, 124))),
                    Span::styled(
                        trim(&link, area.width.saturating_sub(9) as usize),
                        Style::default().fg(Color::Rgb(170, 178, 190)),
                    ),
                ]));
            }
        }
    } else {
        lines.push(Line::from(Span::styled(
            "Nothing Playing",
            Style::default().fg(Color::Rgb(104, 113, 124)).bold(),
        )));
        lines.push(Line::from(Span::styled(
            "Pick a track to light this up.",
            Style::default().fg(Color::Rgb(170, 178, 190)),
        )));
    }

    if let Some(sel) = app.sel_art() {
        if app.cur_track().as_ref().map(|track| track.id.as_str()) != Some(sel.id.as_str()) {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Selected",
                Style::default().fg(Color::Rgb(170, 178, 190)).bold(),
            )));
            lines.push(Line::from(Span::styled(
                trim(&sel.title, area.width.saturating_sub(4) as usize),
                Style::default().fg(Color::Rgb(240, 244, 250)),
            )));
            lines.push(Line::from(Span::styled(
                sel.who(),
                Style::default().fg(Color::Rgb(170, 178, 190)),
            )));
            if sel.is_remote_browse() {
                lines.push(Line::from(Span::styled(
                    if sel.is_artist_browse() {
                        "Press Enter to open this artist page"
                    } else {
                        "Press Enter to open it, P to play it, or Q to queue it"
                    },
                    Style::default().fg(Color::Rgb(104, 113, 124)),
                )));
            } else if matches!(app.sc_view_kind, ViewKind::Collection(_)) && sel.is_playable_remote() {
                lines.push(Line::from(Span::styled(
                    "Enter plays from here and queues the rest of this collection",
                    Style::default().fg(Color::Rgb(104, 113, 124)),
                )));
            }
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Recent",
        Style::default().fg(Color::Rgb(170, 178, 190)).bold(),
    )));
    for tr in app
        .core
        .tracks_of(&app.core.hist_ids().into_iter().take(3).collect::<Vec<_>>())
    {
        lines.push(Line::from(Span::raw(format!(
            "{} {}",
            tr.tag(),
            trim(&tr.title, 26)
        ))));
    }

    let p = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Player ")
                .style(Style::default().bg(Color::Rgb(27, 34, 42)))
                .border_style(Style::default().fg(Color::Rgb(220, 196, 110))),
        )
        .wrap(Wrap { trim: true });
    f.render_widget(p, area);
}

fn draw_tail_box(f: &mut Frame, app: &mut App, area: Rect) {
    if app.show_viz && area.height > 8 {
        let sp = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(56), Constraint::Percentage(44)])
            .split(area);
        draw_viz(f, app, sp[0]);
        draw_q(f, app, sp[1]);
    } else if app.show_viz {
        draw_viz(f, app, area);
    } else {
        draw_q(f, app, area);
    }
}

fn draw_q(f: &mut Frame, app: &mut App, area: Rect) {
    let ids = app.core.q_ids();
    let trs = app.core.tracks_of(&ids);
    let mut items = Vec::new();
    for (i, tr) in trs.iter().enumerate() {
        items.push(ListItem::new(format!(
            "{:>2}. [{}] {} - {}",
            i + 1,
            tr.tag(),
            tr.title,
            tr.who()
        )));
    }
    if items.is_empty() {
        items.push(ListItem::new("Queue is empty").style(Style::default().fg(Color::DarkGray)));
    }
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Queue ({}) ", ids.len()))
                .style(Style::default().bg(Color::Rgb(27, 34, 42)))
                .border_style(Style::default().fg(Color::Rgb(116, 188, 255))),
        )
        .highlight_style(Style::default().bg(Color::Rgb(38, 46, 57)))
        .highlight_symbol("> ");
    f.render_stateful_widget(list, area, &mut app.q_st);
}

fn draw_viz(f: &mut Frame, app: &App, area: Rect) {
    let cols = app.viz.len().min(area.width.saturating_sub(3) as usize);
    let h = area.height.saturating_sub(2) as usize;
    let mut lines = Vec::new();
    for row in 0..h {
        let top = h.saturating_sub(1 + row);
        let thr = top as f32 / h.max(1) as f32;
        let mut spans = Vec::new();
        for i in 0..cols {
            let v = app.viz[i];
            let ch = if v >= thr { "█" } else { " " };
            let col = if i < cols / 4 {
                Color::LightRed
            } else if i < cols / 2 {
                Color::Yellow
            } else if i < cols * 3 / 4 {
                Color::Green
            } else {
                Color::Cyan
            };
            spans.push(Span::styled(ch, Style::default().fg(col)));
        }
        lines.push(Line::from(spans));
    }
    let p = Paragraph::new(Text::from(lines)).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Visualizer ")
            .style(Style::default().bg(Color::Rgb(27, 34, 42)))
            .border_style(Style::default().fg(Color::Rgb(126, 221, 94))),
    );
    f.render_widget(p, area);
}

fn draw_foot(f: &mut Frame, app: &App, area: Rect) {
    let msg = app.msg.as_ref().map(|(v, _)| v.as_str()).unwrap_or("");
    let line = Line::from(vec![
        hot("?"),
        Span::raw(" help  "),
        hot("s"),
        Span::raw(" mode  "),
        hot("/"),
        Span::raw(" search  "),
        hot("Enter"),
        Span::raw(" play  "),
        hot("a"),
        Span::raw(" queue  "),
        hot("P/Q"),
        Span::raw(" play/queue collection  "),
        hot("A"),
        Span::raw(" autoplay  "),
        hot("R/z"),
        Span::raw(" repeat/shuffle  "),
        hot("g/m"),
        Span::raw(" home/library  "),
        hot("l"),
        Span::raw(" session  "),
        hot("o"),
        Span::raw(" open  "),
        hot("i"),
        Span::raw(" art  "),
        hot("q"),
        Span::raw(" quit  "),
        Span::styled(msg, Style::default().fg(Color::Green)),
    ]);
    let p = Paragraph::new(line).alignment(Alignment::Center).block(
        Block::default()
            .borders(Borders::ALL)
            .style(Style::default().bg(Color::Rgb(24, 31, 39)))
            .border_style(Style::default().fg(Color::Rgb(92, 104, 116))),
    );
    f.render_widget(p, area);
}

fn draw_help(f: &mut Frame, area: Rect) {
    let box_a = centered(area, 68, 80);
    let lines = vec![
        Line::from(Span::styled(
            "RustPlayer Keys",
            Style::default().fg(Color::Cyan).bold(),
        )),
        Line::from(""),
        Line::from("s switch local / youtube"),
        Line::from("/ search current mode"),
        Line::from("enter play track, or open a selected youtube playlist or album"),
        Line::from("tab accept selected live suggestion in youtube search"),
        Line::from("a add selected track to queue"),
        Line::from("P play a selected playlist/album, or restart the open collection"),
        Line::from("Q queue a selected playlist/album, or queue the open collection"),
        Line::from("space pause, r resume, n next, p prev"),
        Line::from("R cycle repeat off/all/one, z toggle playlist shuffle"),
        Line::from("left/right seek 5s, b/f seek 30s"),
        Line::from("o open selected youtube link"),
        Line::from("i preview selected artwork with wimg"),
        Line::from("g load youtube home, m load account playlists, u go back"),
        Line::from("A toggle autoplay, l open YouTube login or sign out"),
        Line::from("+/- volume, 0 mute"),
        Line::from("d audio devices, F local folders"),
        Line::from("v visualizer, j/k move, J/K queue"),
        Line::from(""),
        Line::from(Span::styled(
            "CLI / Auth",
            Style::default().fg(Color::Yellow).bold(),
        )),
        Line::from("l opens a dedicated YouTube Music login window on Windows"),
        Line::from("personalized home and library playlists use the captured session"),
        Line::from("use: rustplayer auth cookie-file <cookies.txt>"),
        Line::from("or : rustplayer auth cookie-header \"SID=...; SAPISID=...\""),
        Line::from("or : rustplayer auth headers-file <headers.json>"),
        Line::from("or : rustplayer auth login"),
        Line::from("download: rustplayer download <url> --format m4a|mp3"),
        Line::from("control : rustplayer status | play | pause | next"),
        Line::from("q or esc closes overlays, q quits app"),
    ];
    let p = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Help ")
                .style(Style::default().bg(Color::Rgb(24, 31, 39)))
                .border_style(Style::default().fg(Color::Rgb(117, 214, 255))),
        )
        .wrap(Wrap { trim: true });
    f.render_widget(Clear, box_a);
    f.render_widget(p, box_a);
}

fn draw_dev(f: &mut Frame, app: &mut App, area: Rect) {
    let box_a = centered(area, 52, 60);
    let items = app
        .devs
        .iter()
        .map(|(name, cur)| {
            let txt = if *cur {
                format!("* {}", name)
            } else {
                format!("  {}", name)
            };
            let st = if *cur {
                Style::default().fg(Color::Green).bold()
            } else {
                Style::default()
            };
            ListItem::new(txt).style(st)
        })
        .collect::<Vec<_>>();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Devices ")
                .border_style(Style::default().fg(Color::Magenta)),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    f.render_widget(Clear, box_a);
    f.render_stateful_widget(list, box_a, &mut app.dev_st);
}

fn draw_fld(f: &mut Frame, app: &mut App, area: Rect) {
    let box_a = centered(area, 62, 60);
    let ps = app.core.scan_paths.lock().unwrap();
    let mut items = ps
        .iter()
        .enumerate()
        .map(|(i, p)| ListItem::new(format!("{:>2}. {}", i + 1, p.to_string_lossy())))
        .collect::<Vec<_>>();
    if items.is_empty() {
        items.push(
            ListItem::new("No folders added yet").style(Style::default().fg(Color::DarkGray)),
        );
    } else {
        items.push(ListItem::new(""));
        items.push(ListItem::new("Enter search folder").style(Style::default().fg(Color::Green)));
        items.push(ListItem::new("a search all folders").style(Style::default().fg(Color::Green)));
        items.push(ListItem::new("d remove folder").style(Style::default().fg(Color::Yellow)));
    }
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Folders ({}) ", ps.len()))
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    drop(ps);
    f.render_widget(Clear, box_a);
    f.render_stateful_widget(list, box_a, &mut app.fld_st);
}

fn draw_inp(f: &mut Frame, app: &App, area: Rect) {
    let sug_h = if app.md == Mode::Sc && !app.sc_sugs.is_empty() { 36 } else { 18 };
    let box_a = centered(area, 60, sug_h);
    let ttl = match app.md {
        Mode::Local => " Search Local ",
        Mode::Sc => " Search YouTube ",
    };
    let hint = match app.md {
        Mode::Local => "Find by title or artist",
        Mode::Sc => "Live suggestions from YouTube Music. Tab accepts a suggestion.",
    };
    let chunks = if app.md == Mode::Sc && !app.sc_sugs.is_empty() {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(5), Constraint::Min(4)])
            .split(box_a)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0)])
            .split(box_a)
    };

    let lines = vec![
        Line::from(Span::styled(hint, Style::default().fg(Color::Rgb(170, 178, 190)))),
        Line::from(""),
        Line::from(Span::styled(
            format!("> {}", app.qry),
            Style::default().fg(Color::Rgb(238, 204, 100)).bold(),
        )),
    ];
    let p = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(ttl)
                .style(Style::default().bg(Color::Rgb(24, 31, 39)))
                .border_style(Style::default().fg(Color::Rgb(220, 196, 110))),
        )
        .wrap(Wrap { trim: true });
    f.render_widget(Clear, box_a);
    f.render_widget(p, chunks[0]);

    if app.md == Mode::Sc && !app.sc_sugs.is_empty() && chunks.len() > 1 {
        let items = app
            .sc_sugs
            .iter()
            .enumerate()
            .map(|(index, item)| {
                ListItem::new(format!("{:>2}. {}", index + 1, item))
                    .style(Style::default().fg(Color::Rgb(229, 234, 241)))
            })
            .collect::<Vec<_>>();
        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Suggestions ")
                    .style(Style::default().bg(Color::Rgb(27, 34, 42)))
                    .border_style(Style::default().fg(Color::Rgb(117, 214, 255))),
            )
            .highlight_style(
                Style::default()
                    .bg(Color::Rgb(38, 46, 57))
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("> ");
        let mut state = app.sc_sug_st.clone();
        f.render_stateful_widget(list, chunks[1], &mut state);
    }
}

fn preview_bytes_with_wimg(bytes: &[u8]) -> anyhow::Result<()> {
    let ext = guessed_image_ext(bytes);
    let path = temp_preview_path(ext);
    fs::write(&path, bytes)?;

    let run = || -> anyhow::Result<()> {
        print!("\x1b[2J\x1b[H\x1b[?25h");
        io::stdout().flush()?;

        let status = Command::new("wimg").arg(&path).status()?;
        if !status.success() {
            anyhow::bail!("wimg exited with status {}", status);
        }

        println!();
        println!("Press any key to return...");
        io::stdout().flush()?;

        loop {
            if event::poll(Duration::from_millis(100))? {
                if matches!(
                    event::read()?,
                    CEvent::Key(key) if key.kind == KeyEventKind::Press
                ) {
                    break;
                }
            }
        }
        Ok(())
    };

    let res = run();
    let _ = fs::remove_file(&path);
    res
}

fn guessed_image_ext(bytes: &[u8]) -> &'static str {
    match image::guess_format(bytes).ok() {
        Some(image::ImageFormat::Png) => "png",
        Some(image::ImageFormat::Jpeg) => "jpg",
        Some(image::ImageFormat::Gif) => "gif",
        Some(image::ImageFormat::Bmp) => "bmp",
        Some(image::ImageFormat::WebP) => "webp",
        _ => "img",
    }
}

fn temp_preview_path(ext: &str) -> std::path::PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let mut path = std::env::temp_dir();
    path.push(format!("rustplayer-art-{stamp}.{ext}"));
    path
}

fn hot(v: &str) -> Span<'static> {
    Span::styled(v.to_string(), Style::default().fg(Color::White).bold())
}

fn pill(text: &str, color: Color) -> Span<'static> {
    Span::styled(
        format!(" {} ", text),
        Style::default().fg(Color::Black).bg(color),
    )
}

fn playback_state_label(is_playing: bool) -> (&'static str, Color) {
    if is_playing {
        ("PLAYING", Color::LightGreen)
    } else {
        ("PAUSED", Color::Yellow)
    }
}

fn clock(secs: u64) -> String {
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

fn trim(v: &str, n: usize) -> String {
    if v.len() <= n {
        v.to_string()
    } else {
        format!("{}...", &v[..n.saturating_sub(3)])
    }
}

fn bar(cur: u64, tot: u64, n: usize) -> String {
    let n = n.max(10);
    if tot == 0 {
        return "·".repeat(n);
    }
    let fill = ((cur as f64 / tot as f64).clamp(0.0, 1.0) * n as f64).round() as usize;
    let knob_at = fill.saturating_sub(1).min(n.saturating_sub(1));
    let mut out = String::with_capacity(n);
    for i in 0..n {
        if i == knob_at {
            out.push('●');
        } else if i < fill {
            out.push('█');
        } else {
            out.push('·');
        }
    }
    out
}

fn inner(r: Rect) -> Rect {
    Rect {
        x: r.x.saturating_add(1),
        y: r.y.saturating_add(1),
        width: r.width.saturating_sub(2),
        height: r.height.saturating_sub(2),
    }
}

fn centered(r: Rect, px: u16, py: u16) -> Rect {
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - py) / 2),
            Constraint::Percentage(py),
            Constraint::Percentage((100 - py) / 2),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - px) / 2),
            Constraint::Percentage(px),
            Constraint::Percentage((100 - px) / 2),
        ])
        .split(v[1])[1]
}
