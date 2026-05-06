use crate::core::track::{Acc, Track};
use crate::proxy::{append_ytdlp_proxy_args, apply_command_proxy, apply_reqwest_proxy};
use anyhow::{anyhow, Context, Result};
use percent_encoding::percent_decode_str;
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, CONTENT_RANGE, CONTENT_TYPE, RANGE, REFERER, USER_AGENT};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use url::Url;

const API_BASE: &str = "https://music.youtube.com/youtubei/v1";
const VIDEO_API_BASE: &str = "https://www.youtube.com/youtubei/v1";
const MUSIC_ORIGIN: &str = "https://music.youtube.com";
const MUSIC_REFERER: &str = "https://music.youtube.com/";
const VIDEO_ORIGIN: &str = "https://www.youtube.com";
const VIDEO_REFERER: &str = "https://www.youtube.com/";
const TV_REFERER: &str = "https://www.youtube.com/tv";
const WEB_REMIX_CLIENT_VERSION: &str = "1.20260502.01.00";
const SEARCH_SONGS_FILTER: &str = "EgWKAQIIAWoKEAkQBRAKEAMQBA%3D%3D";
const VISITOR_PREFIXES: [&str; 2] = ["Cgt", "Cgs"];
const STREAM_DOWNLOAD_CHUNK_BYTES: u64 = 1024 * 1024 * 2;
const YT_DLP_TIMEOUT_SECS: u64 = 8;
const HOME_BROWSE_ID: &str = "FEmusic_home";
const LIBRARY_PLAYLISTS_BROWSE_ID: &str = "FEmusic_liked_playlists";

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Ql {
    Low,
    Med,
    High,
}

impl Ql {
    pub fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "low" => Self::Low,
            "medium" | "med" => Self::Med,
            _ => Self::High,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Med => "medium",
            Self::High => "high",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct YtState {
    pub ready: bool,
    pub user: bool,
    pub name: Option<String>,
    pub msg: Option<String>,
    pub ql: String,
}

#[derive(Clone, Debug)]
pub struct YtStream {
    pub url: String,
    pub duration_secs: Option<u64>,
}

#[derive(Clone, Debug)]
struct AccountInfo {
    name: String,
    _email: Option<String>,
    _channel_handle: Option<String>,
}

#[derive(Clone)]
pub struct YouTubeClient {
    http: Client,
    media_http: Client,
    visitor_data: Option<String>,
    stream_cache: HashMap<String, CachedStream>,
    audio_cache: HashMap<String, Vec<u8>>,
    audio_cache_order: Vec<String>,
    cookie_header: Option<String>,
    auth_user: Option<String>,
    ql: Ql,
}

#[derive(Clone, Debug)]
struct CachedStream {
    url: String,
    expires_at: u64,
    duration_secs: Option<u64>,
}

#[derive(Clone, Copy, Debug)]
enum PlaybackClient {
    AndroidVr,
    WebSafari,
    TvDowngraded,
}

impl PlaybackClient {
    fn candidates(authenticated: bool) -> &'static [Self] {
        if authenticated {
            &[Self::TvDowngraded, Self::WebSafari]
        } else {
            &[Self::AndroidVr, Self::WebSafari]
        }
    }

    fn client_id(self) -> &'static str {
        match self {
            Self::AndroidVr => "28",
            Self::WebSafari => "1",
            Self::TvDowngraded => "7",
        }
    }

    fn client_name(self) -> &'static str {
        match self {
            Self::AndroidVr => "ANDROID_VR",
            Self::WebSafari => "WEB",
            Self::TvDowngraded => "TVHTML5",
        }
    }

    fn client_version(self) -> &'static str {
        match self {
            Self::AndroidVr => "1.70.10",
            Self::WebSafari => "2.20260114.08.00",
            Self::TvDowngraded => "5.20260114",
        }
    }

    fn user_agent(self) -> &'static str {
        match self {
            Self::AndroidVr => {
                "com.google.android.apps.youtube.vr.oculus/1.65.10 (Linux; U; Android 12L; eureka-user Build/SQ3A.220605.009.A1) gzip"
            }
            Self::WebSafari => {
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/15.5 Safari/605.1.15,gzip(gfe)"
            }
            Self::TvDowngraded => {
                "Mozilla/5.0 (ChromiumStylePlatform) Cobalt/Version"
            }
        }
    }

    fn api_base(self) -> &'static str {
        VIDEO_API_BASE
    }

    fn origin(self) -> &'static str {
        VIDEO_ORIGIN
    }

    fn referer(self) -> &'static str {
        match self {
            Self::TvDowngraded => TV_REFERER,
            Self::AndroidVr | Self::WebSafari => VIDEO_REFERER,
        }
    }

    fn supports_cookies(self) -> bool {
        matches!(self, Self::WebSafari | Self::TvDowngraded)
    }

    fn request_body(self, visitor_data: &str, video_id: &str) -> Value {
        let client = match self {
            Self::AndroidVr => json!({
                "clientName": "ANDROID_VR",
                "clientVersion": "1.65.10",
                "osName": "Android",
                "osVersion": "12L",
                "deviceMake": "Oculus",
                "deviceModel": "Quest 3",
                "androidSdkVersion": 32,
                "gl": "US",
                "hl": "en-US",
                "userAgent": self.user_agent(),
                "visitorData": visitor_data,
            }),
            Self::WebSafari => json!({
                "clientName": "WEB",
                "clientVersion": "2.20260114.08.00",
                "gl": "US",
                "hl": "en-US",
                "userAgent": self.user_agent(),
                "visitorData": visitor_data,
            }),
            Self::TvDowngraded => json!({
                "clientName": "TVHTML5",
                "clientVersion": "5.20260114",
                "gl": "US",
                "hl": "en-US",
                "userAgent": self.user_agent(),
                "visitorData": visitor_data,
            }),
        };

        let body = json!({
            "context": {
                "client": client,
                "user": {}
            },
            "videoId": video_id,
            "contentCheckOk": true,
            "racyCheckOk": true,
            "playbackContext": {
                "contentPlaybackContext": {
                    "html5Preference": "HTML5_PREF_WANTS"
                }
            }
        });

        body
    }
}

#[derive(Clone, Copy, Debug)]
enum LegacyStreamPlaybackClient {
    TvHtml5,
    TvEmbedded,
    AndroidVr143,
    AndroidVr161,
    AndroidMobile,
    Ios,
    AndroidCreator,
}

impl LegacyStreamPlaybackClient {
    fn all() -> [Self; 7] {
        [
            Self::TvEmbedded,
            Self::TvHtml5,
            Self::AndroidVr143,
            Self::AndroidVr161,
            Self::AndroidCreator,
            Self::AndroidMobile,
            Self::Ios,
        ]
    }

    fn client_id(self) -> &'static str {
        match self {
            Self::TvHtml5 => "7",
            Self::AndroidVr143 | Self::AndroidVr161 => "28",
            Self::AndroidMobile => "3",
            Self::Ios => "5",
            Self::AndroidCreator => "14",
            Self::TvEmbedded => "85",
        }
    }

    fn client_name(self) -> &'static str {
        match self {
            Self::TvHtml5 => "TVHTML5",
            Self::AndroidVr143 | Self::AndroidVr161 => "ANDROID_VR",
            Self::AndroidMobile => "ANDROID",
            Self::Ios => "IOS",
            Self::AndroidCreator => "ANDROID_CREATOR",
            Self::TvEmbedded => "TVHTML5_SIMPLY_EMBEDDED_PLAYER",
        }
    }

    fn client_version(self) -> &'static str {
        match self {
            Self::TvHtml5 => "7.20260213.00.00",
            Self::AndroidVr143 => "1.43.32",
            Self::AndroidVr161 => "1.61.48",
            Self::AndroidMobile => "21.03.38",
            Self::Ios => "21.03.1",
            Self::AndroidCreator => "25.03.101",
            Self::TvEmbedded => "2.0",
        }
    }

    fn user_agent(self) -> &'static str {
        match self {
            Self::TvHtml5 => {
                "Mozilla/5.0(SMART-TV; Linux; Tizen 4.0.0.2) AppleWebkit/605.1.15 (KHTML, like Gecko) SamsungBrowser/9.2 TV Safari/605.1.15"
            }
            Self::AndroidVr143 => {
                "com.google.android.apps.youtube.vr.oculus/1.43.32 (Linux; U; Android 12; en_US; Quest 3; Build/SQ3A.220605.009.A1; Cronet/107.0.5284.2)"
            }
            Self::AndroidVr161 => {
                "com.google.android.apps.youtube.vr.oculus/1.61.48 (Linux; U; Android 12; en_US; Quest 3; Build/SQ3A.220605.009.A1; Cronet/132.0.6808.3)"
            }
            Self::AndroidMobile => {
                "com.google.android.youtube/21.03.38 (Linux; U; Android 14) gzip"
            }
            Self::Ios => {
                "com.google.ios.youtube/21.03.1 (iPhone16,2; U; CPU iOS 18_2 like Mac OS X;)"
            }
            Self::AndroidCreator => {
                "com.google.android.apps.youtube.creator/25.03.101 (Linux; U; Android 15; en_US; Pixel 9 Pro Fold; Build/AP3A.241005.015.A2; Cronet/132.0.6779.0)"
            }
            Self::TvEmbedded => {
                "Mozilla/5.0 (PlayStation; PlayStation 4/12.02) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/15.4 Safari/605.1.15"
            }
        }
    }

    fn login_supported(self) -> bool {
        matches!(
            self,
            Self::TvHtml5 | Self::TvEmbedded | Self::AndroidCreator | Self::AndroidMobile
        )
    }

    fn request_body(self, visitor_data: &str, data_sync_id: Option<&str>, video_id: &str) -> Value {
        let client = match self {
            Self::TvHtml5 => json!({
                "clientName": "TVHTML5",
                "clientVersion": "7.20260213.00.00",
                "gl": "US",
                "hl": "en-US",
                "visitorData": visitor_data,
            }),
            Self::AndroidVr143 => json!({
                "clientName": "ANDROID_VR",
                "clientVersion": "1.43.32",
                "osName": "Android",
                "osVersion": "12",
                "deviceMake": "Oculus",
                "deviceModel": "Quest 3",
                "androidSdkVersion": "32",
                "gl": "US",
                "hl": "en-US",
                "visitorData": visitor_data,
            }),
            Self::AndroidVr161 => json!({
                "clientName": "ANDROID_VR",
                "clientVersion": "1.61.48",
                "osName": "Android",
                "osVersion": "12",
                "deviceMake": "Oculus",
                "deviceModel": "Quest 3",
                "androidSdkVersion": "32",
                "gl": "US",
                "hl": "en-US",
                "visitorData": visitor_data,
            }),
            Self::AndroidMobile => json!({
                "clientName": "ANDROID",
                "clientVersion": "21.03.38",
                "gl": "US",
                "hl": "en-US",
                "visitorData": visitor_data,
            }),
            Self::Ios => json!({
                "clientName": "IOS",
                "clientVersion": "21.03.1",
                "osName": "iOS",
                "osVersion": "18.2",
                "deviceMake": "Apple",
                "deviceModel": "iPhone16,2",
                "gl": "US",
                "hl": "en-US",
                "visitorData": visitor_data,
            }),
            Self::AndroidCreator => json!({
                "clientName": "ANDROID_CREATOR",
                "clientVersion": "25.03.101",
                "osName": "Android",
                "osVersion": "15",
                "deviceMake": "Google",
                "deviceModel": "Pixel 9 Pro Fold",
                "androidSdkVersion": "35",
                "gl": "US",
                "hl": "en-US",
                "visitorData": visitor_data,
            }),
            Self::TvEmbedded => json!({
                "clientName": "TVHTML5_SIMPLY_EMBEDDED_PLAYER",
                "clientVersion": "2.0",
                "gl": "US",
                "hl": "en-US",
                "visitorData": visitor_data,
            }),
        };

        let mut user = json!({});
        if self.login_supported() {
            if let Some(data_sync_id) = data_sync_id {
                user["onBehalfOfUser"] = Value::String(data_sync_id.to_string());
            }
        }

        let mut body = json!({
            "context": {
                "client": client,
                "user": user
            },
            "videoId": video_id,
        });

        if matches!(self, Self::TvEmbedded) {
            body["context"]["thirdParty"] =
                json!({ "embedUrl": format!("https://www.youtube.com/watch?v={video_id}") });
        }

        body
    }
}

pub type SoundCloudClient = YouTubeClient;
pub type ScState = YtState;
pub type ScStream = YtStream;

#[allow(dead_code)]
impl YouTubeClient {
    pub fn new(ql: Ql) -> Self {
        let http = apply_reqwest_proxy(
            Client::builder()
                .timeout(Duration::from_secs(30))
                .user_agent(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:140.0) Gecko/20100101 Firefox/140.0",
            ),
        )
        .build()
        .unwrap_or_else(|_| Client::new());
        let media_http = apply_reqwest_proxy(
            Client::builder()
                .connect_timeout(Duration::from_secs(20))
                .user_agent(LegacyStreamPlaybackClient::AndroidVr143.user_agent()),
        )
        .build()
        .unwrap_or_else(|_| Client::new());

        Self {
            http,
            media_http,
            visitor_data: None,
            stream_cache: HashMap::new(),
            audio_cache: HashMap::new(),
            audio_cache_order: Vec::new(),
            cookie_header: None,
            auth_user: None,
            ql,
        }
    }

    pub fn set_cookie_header(&mut self, cookie_header: Option<String>) {
        self.cookie_header = cookie_header.filter(|value| !value.trim().is_empty());
    }

    pub fn set_auth_user(&mut self, auth_user: Option<String>) {
        self.auth_user = auth_user.filter(|value| !value.trim().is_empty());
    }

    pub fn state(&mut self) -> ScState {
        match self.ensure_visitor_data() {
            Ok(_) => {
                let mut state = ScState {
                    ready: true,
                    user: false,
                    name: None,
                    msg: None,
                    ql: self.ql.as_str().to_string(),
                };

                if self.cookie_header.is_some() {
                    match self.account_info() {
                        Ok(info) => {
                            state.user = true;
                            state.name = Some(info.name);
                        }
                        Err(err) => {
                            state.msg = Some(format!(
                                "Cookie auth is configured but account check failed: {err}"
                            ));
                        }
                    }
                }

                state
            }
            Err(err) => ScState {
                ready: false,
                user: false,
                name: None,
                msg: Some(err.to_string()),
                ql: self.ql.as_str().to_string(),
            },
        }
    }

    pub fn login(&mut self) -> Result<ScState> {
        self.visitor_data = None;
        if self.cookie_header.is_some() {
            let _ = self.account_info()?;
        } else {
            let _ = self.ensure_visitor_data()?;
        }
        Ok(self.state())
    }

    pub fn logout(&mut self) -> Result<ScState> {
        self.visitor_data = None;
        self.stream_cache.clear();
        self.audio_cache.clear();
        self.audio_cache_order.clear();
        self.cookie_header = None;
        self.auth_user = None;
        Ok(self.state())
    }

    pub fn search(&mut self, q: &str, lim: usize) -> Result<Vec<Track>> {
        let q = q.trim();
        if q.is_empty() {
            return Ok(Vec::new());
        }

        if is_soundcloud_url(q) {
            return Ok(self.resolve(q)?.into_iter().collect());
        }

        let rsp = self.search_request(q, Some(SEARCH_SONGS_FILTER))?;
        Ok(parse_search_results(&rsp, lim))
    }

    pub fn search_catalog(&mut self, q: &str, lim: usize) -> Result<Vec<Track>> {
        let q = q.trim();
        if q.is_empty() {
            return Ok(Vec::new());
        }

        if is_soundcloud_url(q) {
            return Ok(self.resolve(q)?.into_iter().collect());
        }

        let rsp = self.search_request(q, None)?;
        Ok(parse_search_results(&rsp, lim))
    }

    pub fn search_suggestions(&mut self, q: &str, lim: usize) -> Result<Vec<String>> {
        let q = q.trim();
        if q.is_empty() {
            return Ok(Vec::new());
        }

        let rsp = self.web_remix_request(
            "music/get_search_suggestions",
            json!({
                "input": q,
            }),
        )?;
        Ok(parse_search_suggestions(&rsp, lim))
    }

    fn search_request(&mut self, q: &str, params: Option<&str>) -> Result<Value> {
        let mut body = json!({
            "query": q,
        });
        if let Some(params) = params {
            body["params"] = Value::String(params.to_string());
        }
        self.web_remix_request("search", body)
    }

    pub fn authenticated(&self) -> bool {
        self.cookie_header.is_some()
    }

    pub fn ffmpeg_headers(&self) -> Vec<(String, String)> {
        let mut headers = vec![
            (
                "User-Agent".to_string(),
                LegacyStreamPlaybackClient::AndroidVr143
                    .user_agent()
                    .to_string(),
            ),
            ("Referer".to_string(), VIDEO_REFERER.to_string()),
        ];

        if let Some(cookie) = self.cookie_header.as_ref() {
            headers.push(("Cookie".to_string(), cookie.clone()));
        }

        headers
    }

    pub fn account_feed(&mut self, home_limit: usize, playlist_limit: usize) -> Result<Vec<Track>> {
        let mut items = self.home_feed(home_limit)?;
        if self.authenticated() {
            if let Ok(playlists) = self.library_playlists(playlist_limit) {
                append_unique_tracks(
                    &mut items,
                    playlists,
                    home_limit.saturating_add(playlist_limit),
                );
            }
        }
        Ok(items)
    }

    pub fn home_feed(&mut self, limit: usize) -> Result<Vec<Track>> {
        let rsp = self.browse(json!({ "browseId": HOME_BROWSE_ID }))?;
        Ok(parse_home_feed(&rsp, limit))
    }

    pub fn library_playlists(&mut self, limit: usize) -> Result<Vec<Track>> {
        self.ensure_authenticated_action("library playlists")?;
        let rsp = self.browse(json!({ "browseId": LIBRARY_PLAYLISTS_BROWSE_ID }))?;
        Ok(parse_library_playlists(&rsp, limit))
    }

    pub fn like_song(&mut self, video_id: &str) -> Result<()> {
        self.ensure_authenticated_action("like songs")?;
        let video_id = video_id.trim();
        if video_id.is_empty() {
            return Err(anyhow!("Track is missing a YouTube video id"));
        }

        let _ = self.web_remix_request(
            "like/like",
            json!({
                "target": {
                    "videoId": video_id,
                }
            }),
        )?;
        Ok(())
    }

    pub fn browse_page(&mut self, browse_id: &str, limit: usize) -> Result<(String, Vec<Track>)> {
        let browse_id = normalize_browse_id(browse_id);
        let rsp = self.browse(json!({ "browseId": browse_id }))?;
        if is_artist_page(&rsp) {
            return Ok(parse_artist_page(&rsp, limit));
        }

        let title = browse_page_title(&rsp).unwrap_or_else(|| browse_id.clone());
        let tracks = parse_collection_tracks(&rsp, limit);
        if !tracks.is_empty() {
            return Ok((title, tracks));
        }
        Ok((title, parse_home_feed(&rsp, limit)))
    }

    pub fn watch_next(&mut self, seed: &Track, limit: usize) -> Result<Vec<Track>> {
        if !seed.is_playable_remote() {
            return Ok(Vec::new());
        }

        let Some(video_id) = seed.remote_video_id() else {
            return Ok(Vec::new());
        };

        let rsp = self.next(json!({
            "enablePersistentPlaylistPanel": true,
            "isAudioOnly": true,
            "tunerSettingValue": "AUTOMIX_SETTING_NORMAL",
            "videoId": video_id,
            "playlistId": format!("RDAMVM{video_id}"),
            "watchEndpointMusicSupportedConfigs": {
                "watchEndpointMusicConfig": {
                    "hasPersistentPlaylistPanel": true,
                    "musicVideoType": "MUSIC_VIDEO_TYPE_ATV"
                }
            }
        }))?;

        Ok(parse_watch_next_tracks(&rsp, &seed.id, limit))
    }

    pub fn resolve(&mut self, url: &str) -> Result<Option<Track>> {
        let video_id = match extract_video_id(url) {
            Some(id) => id,
            None => return Ok(None),
        };

        let player = self.fetch_player_metadata(&video_id)?;
        Ok(Some(track_from_player(&video_id, &player)))
    }

    pub fn stream(&mut self, tr: &Track) -> Result<ScStream> {
        if !tr.is_sc() {
            return Err(anyhow!("Track is not from YouTube"));
        }

        if tr.acc == Some(Acc::Block) {
            return Err(anyhow!("This YouTube track is blocked"));
        }

        if let Some(cached) = self.stream_cache.get(&tr.id) {
            if cached.expires_at > now() + 30 {
                return Ok(ScStream {
                    url: cached.url.clone(),
                    duration_secs: cached.duration_secs.or(tr.dur),
                });
            }
        }

        let video_id = tr
            .id
            .strip_prefix("yt:")
            .or_else(|| tr.id.strip_prefix("sc:"))
            .unwrap_or(tr.id.as_str());

        match self
            .resolve_stream_with_legacy_pipeline(video_id)
            .or_else(|legacy_err| {
                self.resolve_stream_with_ytdlp(video_id).context(format!(
                    "legacy stream resolution failed first: {legacy_err:#}"
                ))
            }) {
            Ok(cached) => {
                let duration_secs = cached.duration_secs.or(tr.dur);
                self.stream_cache.insert(tr.id.clone(), cached.clone());
                Ok(ScStream {
                    url: cached.url,
                    duration_secs,
                })
            }
            Err(err) => Err(err),
        }
    }

    pub fn invalidate_stream(&mut self, track_id: &str) {
        self.stream_cache.remove(track_id);
        self.audio_cache.remove(track_id);
        self.audio_cache_order.retain(|id| id != track_id);
    }

    pub fn take_cached_audio(&mut self, track_id: &str) -> Option<Vec<u8>> {
        self.audio_cache_order.retain(|id| id != track_id);
        self.audio_cache.remove(track_id)
    }

    pub fn download_stream(&self, url: &str) -> Result<Vec<u8>> {
        let range_err = match self.download_stream_by_range(url) {
            Ok(bytes) => return Ok(bytes),
            Err(err) => err,
        };

        self.media_request(url)
            .send()
            .and_then(|rsp| rsp.error_for_status())
            .context(format!(
                "Falling back to a full-body download after ranged fetch failed: {range_err:#}"
            ))?
            .bytes()
            .map(|bytes| bytes.to_vec())
            .context("Could not download YouTube audio stream")
    }

    pub fn art(&self, url: &str) -> Result<Vec<u8>> {
        Ok(self
            .with_auth(self.http.get(url), false)
            .header(ACCEPT, "*/*")
            .send()?
            .error_for_status()?
            .bytes()?
            .to_vec())
    }

    fn resolve_stream_with_legacy_pipeline(&mut self, video_id: &str) -> Result<CachedStream> {
        let visitor = self.ensure_visitor_data()?.to_string();
        let data_sync_id = self.stream_data_sync_id();
        let mut last_error: Option<anyhow::Error> = None;
        let mut attempts = Vec::new();

        for client in LegacyStreamPlaybackClient::all() {
            let body = client.request_body(&visitor, data_sync_id.as_deref(), video_id);
            let rsp = self
                .with_stream_player_auth(
                    self.http
                        .post(format!("{API_BASE}/player?prettyPrint=false")),
                    client.login_supported(),
                )
                .header(ACCEPT, "application/json")
                .header(CONTENT_TYPE, "application/json")
                .header("X-Goog-Api-Format-Version", "1")
                .header("X-YouTube-Client-Name", client.client_id())
                .header("X-YouTube-Client-Version", client.client_version())
                .header("X-Origin", MUSIC_ORIGIN)
                .header("X-Goog-Visitor-Id", &visitor)
                .header(REFERER, MUSIC_REFERER)
                .header(USER_AGENT, client.user_agent())
                .json(&body)
                .send();

            match rsp {
                Ok(resp) => {
                    let json = match resp.error_for_status() {
                        Ok(resp) => match resp.json::<Value>() {
                            Ok(json) => json,
                            Err(err) => {
                                attempts.push(format!(
                                    "{}: could not parse player response: {err}",
                                    client.client_name()
                                ));
                                last_error = Some(err.into());
                                continue;
                            }
                        },
                        Err(err) => {
                            attempts.push(format!(
                                "{}: player request failed: {err}",
                                client.client_name()
                            ));
                            last_error = Some(err.into());
                            continue;
                        }
                    };
                    if playability_status(&json) != Some("OK") {
                        let reason = playability_reason(&json).unwrap_or_else(|| {
                            format!(
                                "{} returned {}",
                                client.client_name(),
                                playability_status(&json).unwrap_or("UNKNOWN")
                            )
                        });
                        let explanation = explain_playability_reason(reason, self.authenticated());
                        attempts.push(format!("{}: {explanation}", client.client_name()));
                        last_error = Some(anyhow!(explanation));
                        continue;
                    }

                    if let Some(choice) = pick_audio_stream(&json, self.ql) {
                        return Ok(CachedStream {
                            url: choice.url,
                            expires_at: choice.expires_at,
                            duration_secs: video_duration_secs(&json),
                        });
                    }

                    let message = format!(
                        "{} returned no directly playable AAC/MP4 audio formats",
                        client.client_name()
                    );
                    attempts.push(message.clone());
                    last_error = Some(anyhow!(message));
                }
                Err(err) => {
                    attempts.push(format!("{}: {err}", client.client_name()));
                    last_error = Some(err.into());
                }
            }
        }

        let attempts = attempts.join(" | ");
        Err(last_error
            .unwrap_or_else(|| anyhow!("No playable YouTube stream was returned"))
            .context(format!("stream resolver attempts: {attempts}")))
    }

    fn resolve_stream_with_ytdlp(&self, video_id: &str) -> Result<CachedStream> {
        let watch_url = format!("https://www.youtube.com/watch?v={video_id}");
        let cookie_file = self.write_ytdlp_cookie_file()?;
        let format = self.ytdlp_format_selector();
        let output = self
            .run_ytdlp(&watch_url, format, cookie_file.as_deref())
            .context("The yt-dlp stream resolver could not obtain a playable audio URL");

        if let Some(path) = cookie_file.as_ref() {
            let _ = fs::remove_file(path);
        }

        let output = output?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let url = stdout
            .lines()
            .find(|line| line.trim_start().starts_with("http"))
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .context("yt-dlp returned no audio URL")?;

        Ok(CachedStream {
            url: url.to_string(),
            expires_at: stream_expiration(url),
            duration_secs: None,
        })
    }

    fn run_ytdlp(
        &self,
        watch_url: &str,
        format: &str,
        cookie_file: Option<&std::path::Path>,
    ) -> Result<std::process::Output> {
        let mut common_args = vec![
            "-m".to_string(),
            "yt_dlp".to_string(),
            "--js-runtimes".to_string(),
            "node".to_string(),
            "--remote-components".to_string(),
            "ejs:github".to_string(),
            "--no-playlist".to_string(),
            "-g".to_string(),
            "-f".to_string(),
            format.to_string(),
        ];

        if let Some(cookie_file) = cookie_file {
            common_args.push("--cookies".to_string());
            common_args.push(cookie_file.display().to_string());
        }

        append_ytdlp_proxy_args(&mut common_args);
        common_args.push(watch_url.to_string());

        let output = run_ytdlp_command("python", &common_args)
            .or_else(|_| {
                let mut py_args = vec!["-3".to_string()];
                py_args.extend(common_args.clone());
                run_ytdlp_command("py", &py_args)
            })
            .or_else(|_| run_ytdlp_command("yt-dlp", &common_args))
            .context(
                "Could not start yt-dlp. Install it with `python -m pip install --user yt-dlp` or `winget install yt-dlp`.",
            )?;

        if output.status.success() {
            return Ok(output);
        }

        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(anyhow!(stderr.trim().to_string()))
    }

    fn write_ytdlp_cookie_file(&self) -> Result<Option<std::path::PathBuf>> {
        let Some(cookie_header) = self.cookie_header.as_deref() else {
            return Ok(None);
        };

        let cookie_map = parse_cookie_header(cookie_header);
        if cookie_map.is_empty() {
            return Ok(None);
        }

        let path = std::env::temp_dir().join(format!("rustplayer-ytdlp-{}.cookies", now()));
        let mut out = String::from("# Netscape HTTP Cookie File\n");

        for (name, value) in cookie_map {
            out.push_str(&format!(
                ".youtube.com\tTRUE\t/\tTRUE\t2147483647\t{name}\t{value}\n"
            ));
        }

        fs::write(&path, out).with_context(|| {
            format!(
                "Could not write temporary yt-dlp cookies to {}",
                path.display()
            )
        })?;
        Ok(Some(path))
    }

    fn ytdlp_format_selector(&self) -> &'static str {
        match self.ql {
            Ql::Low => {
                "bestaudio[ext=m4a][abr<=96]/bestaudio[abr<=96]/bestaudio[ext=m4a]/bestaudio"
            }
            Ql::Med => {
                "bestaudio[ext=m4a][abr<=128]/bestaudio[abr<=128]/bestaudio[ext=m4a]/bestaudio"
            }
            Ql::High => "bestaudio[ext=m4a]/bestaudio",
        }
    }

    fn ensure_visitor_data(&mut self) -> Result<&str> {
        if self.visitor_data.is_none() {
            let txt = self
                .with_auth(self.http.get("https://music.youtube.com/sw.js_data"), false)
                .send()?
                .error_for_status()?
                .text()?;

            let data: Value =
                serde_json::from_str(txt.strip_prefix(")]}'").unwrap_or(txt.as_str()))
                    .context("Could not parse YouTube visitor data payload")?;

            let visitor = data
                .get(0)
                .and_then(|v| v.get(2))
                .and_then(Value::as_array)
                .and_then(|items| {
                    items.iter().find_map(|item| {
                        item.as_str().and_then(|candidate| {
                            VISITOR_PREFIXES
                                .iter()
                                .any(|prefix| candidate.starts_with(prefix))
                                .then(|| candidate.to_string())
                        })
                    })
                })
                .context("Could not extract YouTube visitor data")?;

            self.visitor_data = Some(visitor);
        }

        self.visitor_data
            .as_deref()
            .context("YouTube visitor data was not initialized")
    }

    fn fetch_player_metadata(&mut self, video_id: &str) -> Result<Value> {
        let visitor = self.ensure_visitor_data()?.to_string();
        let mut last_error: Option<anyhow::Error> = None;

        for client in PlaybackClient::candidates(self.authenticated()) {
            let body = client.request_body(&visitor, video_id);
            let rsp = self
                .with_auth_origin(
                    self.http
                        .post(format!("{}/player?prettyPrint=false", client.api_base())),
                    client.supports_cookies(),
                    client.origin(),
                )
                .header(ACCEPT, "application/json")
                .header(CONTENT_TYPE, "application/json")
                .header("X-Goog-Api-Format-Version", "1")
                .header("X-YouTube-Client-Name", client.client_id())
                .header("X-YouTube-Client-Version", client.client_version())
                .header("X-Origin", client.origin())
                .header("X-Goog-Visitor-Id", &visitor)
                .header(REFERER, client.referer())
                .header(USER_AGENT, client.user_agent())
                .json(&body)
                .send();

            match rsp {
                Ok(resp) => {
                    let json = match resp.error_for_status() {
                        Ok(resp) => match resp.json::<Value>() {
                            Ok(json) => json,
                            Err(err) => {
                                last_error = Some(err.into());
                                continue;
                            }
                        },
                        Err(err) => {
                            last_error = Some(err.into());
                            continue;
                        }
                    };
                    if playability_status(&json).is_some()
                        && playability_status(&json) != Some("OK")
                    {
                        let reason = playability_reason(&json).unwrap_or_else(|| {
                            format!(
                                "{} returned {}",
                                client.client_name(),
                                playability_status(&json).unwrap_or("UNKNOWN")
                            )
                        });
                        last_error = Some(anyhow!(explain_playability_reason(
                            reason,
                            self.authenticated()
                        )));
                        continue;
                    }
                    if json.get("videoDetails").is_some() {
                        return Ok(json);
                    }
                    last_error = Some(anyhow!(
                        "{} returned no videoDetails payload",
                        client.client_name()
                    ));
                }
                Err(err) => last_error = Some(err.into()),
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow!("Could not resolve YouTube track metadata")))
    }

    fn media_request(&self, url: &str) -> reqwest::blocking::RequestBuilder {
        self.with_stream_media_auth(self.media_http.get(url).header(ACCEPT, "*/*").header(
            USER_AGENT,
            LegacyStreamPlaybackClient::AndroidVr143.user_agent(),
        ))
    }

    fn download_stream_by_range(&self, url: &str) -> Result<Vec<u8>> {
        let first_end = STREAM_DOWNLOAD_CHUNK_BYTES.saturating_sub(1);
        let first = self
            .media_request(url)
            .header(RANGE, format!("bytes=0-{first_end}"))
            .send()?
            .error_for_status()?;

        match first.status() {
            StatusCode::OK => {
                return Ok(first.bytes()?.to_vec());
            }
            StatusCode::PARTIAL_CONTENT => {}
            status => {
                return Err(anyhow!(
                    "Unexpected status {status} when requesting an initial stream chunk"
                ));
            }
        }

        let total = first
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_total_len_from_content_range)
            .context("Missing total stream length in Content-Range header")?;

        let first_bytes = first.bytes()?.to_vec();
        if first_bytes.len() as u64 >= total {
            return Ok(first_bytes);
        }

        let mut out = Vec::with_capacity(total.min(usize::MAX as u64) as usize);
        out.extend_from_slice(&first_bytes);

        while (out.len() as u64) < total {
            let start = out.len() as u64;
            let end = (start + STREAM_DOWNLOAD_CHUNK_BYTES - 1).min(total - 1);
            let chunk = self
                .media_request(url)
                .header(RANGE, format!("bytes={start}-{end}"))
                .send()
                .with_context(|| format!("Could not request stream bytes {start}-{end}"))?
                .error_for_status()
                .with_context(|| format!("YouTube rejected stream bytes {start}-{end}"))?;

            if chunk.status() != StatusCode::PARTIAL_CONTENT {
                return Err(anyhow!(
                    "Expected partial content for stream bytes {start}-{end}, got {}",
                    chunk.status()
                ));
            }

            let bytes = chunk.bytes()?.to_vec();
            if bytes.is_empty() {
                return Err(anyhow!(
                    "YouTube returned an empty body for stream bytes {start}-{end}"
                ));
            }

            out.extend_from_slice(&bytes);
        }

        Ok(out)
    }

    fn account_info(&mut self) -> Result<AccountInfo> {
        let visitor = self.ensure_visitor_data()?.to_string();
        let body = json!({
            "context": {
                "client": {
                    "clientName": "WEB_REMIX",
                    "clientVersion": WEB_REMIX_CLIENT_VERSION,
                    "gl": "US",
                    "hl": "en-US",
                    "visitorData": visitor,
                },
                "user": {}
            }
        });

        let rsp = self
            .with_auth(
                self.http
                    .post(format!("{API_BASE}/account/account_menu?prettyPrint=false")),
                true,
            )
            .header(ACCEPT, "application/json")
            .header(CONTENT_TYPE, "application/json")
            .header("X-Goog-Api-Format-Version", "1")
            .header("X-YouTube-Client-Name", "67")
            .header("X-YouTube-Client-Version", WEB_REMIX_CLIENT_VERSION)
            .header("X-Origin", MUSIC_ORIGIN)
            .header("X-Goog-Visitor-Id", visitor)
            .header(REFERER, MUSIC_REFERER)
            .header(
                USER_AGENT,
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:140.0) Gecko/20100101 Firefox/140.0",
            )
            .json(&body)
            .send()?
            .error_for_status()?
            .json::<Value>()?;

        let header = rsp
            .pointer("/actions/0/openPopupAction/popup/multiPageMenuRenderer/header/activeAccountHeaderRenderer")
            .context("Authenticated account data was not returned")?;

        Ok(AccountInfo {
            name: run_text(header.pointer("/accountName/runs"))
                .context("Authenticated account name was not returned")?,
            _email: run_text(header.pointer("/email/runs")),
            _channel_handle: run_text(header.pointer("/channelHandle/runs")),
        })
    }

    fn browse(&mut self, body: Value) -> Result<Value> {
        self.web_remix_request("browse", body)
    }

    fn next(&mut self, body: Value) -> Result<Value> {
        self.web_remix_request("next", body)
    }

    fn web_remix_request(&mut self, endpoint: &str, mut body: Value) -> Result<Value> {
        let visitor = self.ensure_visitor_data()?.to_string();
        body["context"] = json!({
            "client": {
                "clientName": "WEB_REMIX",
                "clientVersion": WEB_REMIX_CLIENT_VERSION,
                "gl": "US",
                "hl": "en-US",
                "visitorData": visitor,
            },
            "user": {}
        });

        Ok(self
            .with_auth(
                self.http
                    .post(format!("{API_BASE}/{endpoint}?prettyPrint=false")),
                true,
            )
            .header(ACCEPT, "application/json")
            .header(CONTENT_TYPE, "application/json")
            .header("X-Goog-Api-Format-Version", "1")
            .header("X-YouTube-Client-Name", "67")
            .header("X-YouTube-Client-Version", WEB_REMIX_CLIENT_VERSION)
            .header("X-Origin", MUSIC_ORIGIN)
            .header("X-Goog-Visitor-Id", visitor)
            .header(REFERER, MUSIC_REFERER)
            .header(
                USER_AGENT,
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:140.0) Gecko/20100101 Firefox/140.0",
            )
            .json(&body)
            .send()?
            .error_for_status()?
            .json::<Value>()?)
    }

    fn ensure_authenticated_action(&self, action: &str) -> Result<()> {
        if self.authenticated() {
            Ok(())
        } else {
            Err(anyhow!("Sign in with YouTube cookies to load {action}"))
        }
    }

    fn with_auth(
        &self,
        builder: reqwest::blocking::RequestBuilder,
        login: bool,
    ) -> reqwest::blocking::RequestBuilder {
        self.with_auth_origin(builder, login, MUSIC_ORIGIN)
    }

    fn with_stream_player_auth(
        &self,
        builder: reqwest::blocking::RequestBuilder,
        login_supported: bool,
    ) -> reqwest::blocking::RequestBuilder {
        let mut builder = builder;

        if login_supported {
            if let Some(cookie) = self.cookie_header.as_ref() {
                builder = builder.header("Cookie", cookie);
            }
            let auth_user = self.auth_user.as_deref().unwrap_or("0");
            builder = builder.header("X-Goog-AuthUser", auth_user);
            if let Some(auth) = self.sapisid_hash(MUSIC_ORIGIN) {
                builder = builder.header("Authorization", auth);
            }
        }

        builder
    }

    fn with_stream_media_auth(
        &self,
        builder: reqwest::blocking::RequestBuilder,
    ) -> reqwest::blocking::RequestBuilder {
        let mut builder = builder;

        if let Some(cookie) = self.cookie_header.as_ref() {
            builder = builder.header("Cookie", cookie);
        }

        builder
    }

    fn stream_data_sync_id(&self) -> Option<String> {
        let cookie_map = parse_cookie_header(self.cookie_header.as_deref()?);
        let raw = cookie_map.get("DATASYNC_ID")?.trim();
        if raw.is_empty() {
            return None;
        }

        Some(if !raw.contains("||") {
            raw.to_string()
        } else if raw.ends_with("||") {
            raw.trim_end_matches('|').to_string()
        } else {
            raw.split("||").last().unwrap_or(raw).to_string()
        })
    }

    fn with_auth_origin(
        &self,
        builder: reqwest::blocking::RequestBuilder,
        login: bool,
        origin: &str,
    ) -> reqwest::blocking::RequestBuilder {
        let mut builder = builder;

        if let Some(cookie) = self.cookie_header.as_ref() {
            builder = builder.header("Cookie", cookie);
            let auth_user = self.auth_user.as_deref().unwrap_or("0");
            builder = builder.header("X-Goog-AuthUser", auth_user);
        }

        if login {
            if let Some(auth) = self.sapisid_hash(origin) {
                builder = builder.header("Authorization", auth);
            }
        }

        builder
    }

    fn sapisid_hash(&self, origin: &str) -> Option<String> {
        let cookie_map = parse_cookie_header(self.cookie_header.as_deref()?);
        let sapisid = cookie_map
            .get("SAPISID")
            .or_else(|| cookie_map.get("__Secure-3PAPISID"))?;
        let now = now();
        let hash = sha1_hex(&format!("{now} {sapisid} {origin}"));
        Some(format!("SAPISIDHASH {now}_{hash}"))
    }
}

fn run_ytdlp_command(program: &str, args: &[String]) -> Result<std::process::Output> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    apply_command_proxy(&mut cmd);
    let mut child = cmd.spawn()?;
    let deadline = std::time::Instant::now() + Duration::from_secs(YT_DLP_TIMEOUT_SECS);

    loop {
        if let Some(status) = child.try_wait()? {
            return child.wait_with_output().map_err(|err| {
                anyhow!(format!(
                    "Could not read yt-dlp output after {status}: {err}"
                ))
            });
        }

        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow!(format!(
                "yt-dlp timed out after {} seconds",
                YT_DLP_TIMEOUT_SECS
            )));
        }

        std::thread::sleep(Duration::from_millis(100));
    }
}

fn parse_search_results(rsp: &Value, lim: usize) -> Vec<Track> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    let Some(sections) = rsp
        .pointer("/contents/tabbedSearchResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents")
        .and_then(Value::as_array)
    else {
        return out;
    };

    for section in sections {
        collect_browse_items(section, &mut seen, &mut out, lim);
        if out.len() >= lim {
            return out;
        }
    }

    out
}

fn track_from_responsive_list_renderer(renderer: &Value) -> Option<Track> {
    let title = renderer
        .pointer("/flexColumns/0/musicResponsiveListItemFlexColumnRenderer/text/runs")
        .and_then(Value::as_array)
        .and_then(|runs| runs_text(runs))?;

    let art =
        best_thumbnail(renderer.pointer("/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails"));

    let subtitle = renderer
        .pointer("/flexColumns/1/musicResponsiveListItemFlexColumnRenderer/text/runs")
        .and_then(Value::as_array)
        .map(|runs| subtitle_text_from_runs(runs))
        .filter(|text| !text.is_empty());

    let video_id = first_str(renderer, &[
        "/playlistItemData/videoId",
        "/playButton/playNavigationEndpoint/watchEndpoint/videoId",
        "/navigationEndpoint/watchEndpoint/videoId",
        "/overlay/musicItemThumbnailOverlayRenderer/content/musicPlayButtonRenderer/playNavigationEndpoint/watchEndpoint/videoId",
        "/flexColumns/0/musicResponsiveListItemFlexColumnRenderer/text/runs/0/navigationEndpoint/watchEndpoint/videoId",
        "/flexColumns/1/musicResponsiveListItemFlexColumnRenderer/text/runs/0/navigationEndpoint/watchEndpoint/videoId",
    ])
    .map(|value| value.to_string());

    let runs = renderer
        .pointer("/flexColumns/1/musicResponsiveListItemFlexColumnRenderer/text/runs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let segments = split_runs(&runs);
    let artist = segments
        .first()
        .map(|s| s.join(" "))
        .filter(|s| !s.is_empty());
    let duration = segments
        .iter()
        .rev()
        .find_map(|segment| segment.first())
        .and_then(|text| parse_duration(text))
        .or_else(|| {
            renderer
                .pointer(
                    "/fixedColumns/0/musicResponsiveListItemFixedColumnRenderer/text/runs/0/text",
                )
                .and_then(Value::as_str)
                .and_then(parse_duration)
        });

    if let Some(video_id) = video_id {
        let acc = match renderer
            .get("musicItemRendererDisplayPolicy")
            .and_then(Value::as_str)
        {
            Some("MUSIC_ITEM_RENDERER_DISPLAY_POLICY_GREY_OUT") => Some(Acc::Block),
            _ => Some(Acc::Play),
        };

        return Some(Track::new_sc(
            format!("yt:{video_id}"),
            title,
            artist,
            None,
            duration,
            Some(format!("https://music.youtube.com/watch?v={video_id}")),
            art,
            None,
            acc,
        ));
    }

    let browse_id = first_str(renderer, &[
        "/playButton/playNavigationEndpoint/browseEndpoint/browseId",
        "/playButton/playNavigationEndpoint/watchEndpoint/playlistId",
        "/navigationEndpoint/browseEndpoint/browseId",
        "/navigationEndpoint/watchEndpoint/playlistId",
        "/flexColumns/0/musicResponsiveListItemFlexColumnRenderer/text/runs/0/navigationEndpoint/browseEndpoint/browseId",
        "/flexColumns/0/musicResponsiveListItemFlexColumnRenderer/text/runs/0/navigationEndpoint/watchEndpoint/playlistId",
        "/flexColumns/1/musicResponsiveListItemFlexColumnRenderer/text/runs/0/navigationEndpoint/browseEndpoint/browseId",
    ])?;

    Some(browse_track(browse_id, title, subtitle.or(artist), art))
}

fn track_from_player(video_id: &str, player: &Value) -> Track {
    let details = player.get("videoDetails").cloned().unwrap_or(Value::Null);
    let title = details
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Unknown title")
        .to_string();
    let artist = details
        .get("author")
        .and_then(Value::as_str)
        .map(|v| v.to_string());
    let dur = details
        .get("lengthSeconds")
        .and_then(Value::as_str)
        .and_then(|v| v.parse::<u64>().ok());
    let art = best_thumbnail(details.pointer("/thumbnail/thumbnails"));
    let acc = if playability_status(player) == Some("OK") {
        Some(Acc::Play)
    } else {
        Some(Acc::Block)
    };

    Track::new_sc(
        format!("yt:{video_id}"),
        title,
        artist,
        None,
        dur,
        Some(format!("https://music.youtube.com/watch?v={video_id}")),
        art,
        None,
        acc,
    )
}

fn parse_home_feed(rsp: &Value, lim: usize) -> Vec<Track> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    if let Some(sections) = browse_sections(rsp) {
        for section in sections {
            collect_browse_items(section, &mut seen, &mut out, lim);
            if out.len() >= lim {
                return out;
            }
        }
    }

    if out.is_empty() {
        collect_browse_items(rsp, &mut seen, &mut out, lim);
    }

    out
}

fn parse_library_playlists(rsp: &Value, lim: usize) -> Vec<Track> {
    parse_home_feed(rsp, lim)
        .into_iter()
        .filter(|track| track.is_remote_browse())
        .take(lim)
        .collect()
}

fn parse_collection_tracks(rsp: &Value, lim: usize) -> Vec<Track> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let collection_art = browse_page_art(rsp);

    if let Some(contents) = collection_track_contents(rsp) {
        for item in contents {
            if let Some(renderer) = item.get("musicResponsiveListItemRenderer") {
                let mut track = track_from_responsive_list_renderer(renderer);
                if let Some(track) = track.as_mut() {
                    if track.art.is_none() {
                        track.art = collection_art.clone();
                    }
                }
                push_unique_track(&mut out, &mut seen, track, lim);
                if out.len() >= lim {
                    return out;
                }
            }
        }
    }

    if out.is_empty() {
        collect_playable_tracks(rsp, &mut seen, &mut out, lim);
        if let Some(art) = collection_art {
            for track in &mut out {
                if track.art.is_none() {
                    track.art = Some(art.clone());
                }
            }
        }
    }

    out
}

fn collection_track_contents(rsp: &Value) -> Option<&Vec<Value>> {
    rsp.pointer("/contents/twoColumnBrowseResultsRenderer/secondaryContents/sectionListRenderer/contents/0/musicPlaylistShelfRenderer/contents")
        .and_then(Value::as_array)
        .or_else(|| {
            rsp.pointer("/contents/twoColumnBrowseResultsRenderer/secondaryContents/sectionListRenderer/contents/0/musicShelfRenderer/contents")
                .and_then(Value::as_array)
        })
}

fn parse_artist_page(rsp: &Value, lim: usize) -> (String, Vec<Track>) {
    let title = artist_page_title(rsp).unwrap_or_else(|| "Artist".to_string());
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    if let Some(sections) = browse_sections(rsp) {
        for section in sections {
            let section_title = section_title(section);
            let section_items = section_items(section);
            for item in section_items {
                let mut track = if let Some(renderer) = item.get("musicResponsiveListItemRenderer")
                {
                    track_from_responsive_list_renderer(renderer)
                } else if let Some(renderer) = item.get("musicTwoRowItemRenderer") {
                    item_from_two_row_renderer(renderer)
                } else {
                    None
                };

                if let Some(mut track_value) = track.take() {
                    decorate_artist_item(&mut track_value, &title, section_title.as_deref());
                    push_unique_track(&mut out, &mut seen, Some(track_value), lim);
                    if out.len() >= lim {
                        return (title, out);
                    }
                }
            }
        }
    }

    if out.is_empty() {
        collect_browse_items(rsp, &mut seen, &mut out, lim);
        for track in &mut out {
            decorate_artist_item(track, &title, None);
        }
    }

    (title, out)
}

fn parse_watch_next_tracks(rsp: &Value, current_id: &str, lim: usize) -> Vec<Track> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    seen.insert(current_id.to_string());
    let Some(contents) = rsp
        .pointer("/contents/singleColumnMusicWatchNextResultsRenderer/tabbedRenderer/watchNextTabbedResultsRenderer/tabs/0/tabRenderer/content/musicQueueRenderer/content/playlistPanelRenderer/contents")
        .and_then(Value::as_array)
    else {
        return out;
    };

    for item in contents {
        push_unique_track(
            &mut out,
            &mut seen,
            track_from_playlist_panel_item(item),
            lim,
        );
        if out.len() >= lim {
            break;
        }
    }

    out
}

fn collect_browse_items(
    node: &Value,
    seen: &mut HashSet<String>,
    out: &mut Vec<Track>,
    lim: usize,
) {
    if out.len() >= lim {
        return;
    }

    match node {
        Value::Object(map) => {
            if let Some(renderer) = map.get("musicResponsiveListItemRenderer") {
                push_unique_track(
                    out,
                    seen,
                    track_from_responsive_list_renderer(renderer),
                    lim,
                );
            }
            if out.len() >= lim {
                return;
            }
            if let Some(renderer) = map.get("musicTwoRowItemRenderer") {
                push_unique_track(out, seen, item_from_two_row_renderer(renderer), lim);
            }
            if out.len() >= lim {
                return;
            }
            for value in map.values() {
                collect_browse_items(value, seen, out, lim);
                if out.len() >= lim {
                    return;
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_browse_items(item, seen, out, lim);
                if out.len() >= lim {
                    return;
                }
            }
        }
        _ => {}
    }
}

fn section_items(section: &Value) -> Vec<&Value> {
    section
        .pointer("/musicShelfRenderer/contents")
        .and_then(Value::as_array)
        .map(|items| items.iter().collect::<Vec<_>>())
        .or_else(|| {
            section
                .pointer("/musicCarouselShelfRenderer/contents")
                .and_then(Value::as_array)
                .map(|items| items.iter().collect::<Vec<_>>())
        })
        .unwrap_or_default()
}

fn section_title(section: &Value) -> Option<String> {
    first_str(section, &[
        "/musicShelfRenderer/title/runs/0/text",
        "/musicCarouselShelfRenderer/header/musicCarouselShelfBasicHeaderRenderer/title/runs/0/text",
        "/musicCarouselShelfRenderer/header/musicCarouselShelfBasicHeaderRenderer/title/text",
    ])
    .map(|value| value.to_string())
}

fn decorate_artist_item(track: &mut Track, artist_name: &str, section: Option<&str>) {
    if track.is_playable_remote() {
        if track.artist.is_none() || track.artist.as_deref() == Some("Unknown") {
            track.artist = Some(artist_name.to_string());
        }
        if let Some(section) = section {
            track.user = Some(section.to_string());
        }
    } else if let Some(section) = section {
        track.artist = Some(format!("{section} • {}", track.who()));
        track.user = Some(artist_name.to_string());
    } else {
        track.user = Some(artist_name.to_string());
    }
}

fn collect_playable_tracks(
    node: &Value,
    seen: &mut HashSet<String>,
    out: &mut Vec<Track>,
    lim: usize,
) {
    if out.len() >= lim {
        return;
    }

    match node {
        Value::Object(map) => {
            if let Some(renderer) = map.get("musicResponsiveListItemRenderer") {
                push_unique_track(
                    out,
                    seen,
                    track_from_responsive_list_renderer(renderer),
                    lim,
                );
            }
            if out.len() >= lim {
                return;
            }
            for value in map.values() {
                collect_playable_tracks(value, seen, out, lim);
                if out.len() >= lim {
                    return;
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_playable_tracks(item, seen, out, lim);
                if out.len() >= lim {
                    return;
                }
            }
        }
        _ => {}
    }
}

fn push_unique_track(
    out: &mut Vec<Track>,
    seen: &mut HashSet<String>,
    track: Option<Track>,
    lim: usize,
) {
    let Some(track) = track else {
        return;
    };
    if out.len() >= lim || !seen.insert(track.id.clone()) {
        return;
    }
    out.push(track);
}

fn append_unique_tracks(out: &mut Vec<Track>, tracks: Vec<Track>, lim: usize) {
    let mut seen = out
        .iter()
        .map(|track| track.id.clone())
        .collect::<HashSet<_>>();
    for track in tracks {
        if out.len() >= lim {
            break;
        }
        if seen.insert(track.id.clone()) {
            out.push(track);
        }
    }
}

fn browse_sections(rsp: &Value) -> Option<&Vec<Value>> {
    rsp.pointer("/contents/singleColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents")
        .and_then(Value::as_array)
        .or_else(|| {
            rsp.pointer("/contents/twoColumnBrowseResultsRenderer/primaryContents/sectionListRenderer/contents")
                .and_then(Value::as_array)
        })
}

fn item_from_two_row_renderer(renderer: &Value) -> Option<Track> {
    let title = renderer
        .pointer("/title/runs")
        .and_then(Value::as_array)
        .and_then(|runs| runs_text(runs))?;
    let art = best_thumbnail(
        renderer
            .pointer("/thumbnailRenderer/musicThumbnailRenderer/thumbnail/thumbnails")
            .or_else(|| renderer.pointer("/thumbnail/thumbnails")),
    );

    let video_id = first_str(renderer, &[
        "/navigationEndpoint/watchEndpoint/videoId",
        "/overlay/musicItemThumbnailOverlayRenderer/content/musicPlayButtonRenderer/playNavigationEndpoint/watchEndpoint/videoId",
        "/title/runs/0/navigationEndpoint/watchEndpoint/videoId",
        "/subtitle/runs/0/navigationEndpoint/watchEndpoint/videoId",
        "/thumbnailRenderer/musicThumbnailRenderer/thumbnailOverlay/musicItemThumbnailOverlayRenderer/content/musicPlayButtonRenderer/playNavigationEndpoint/watchEndpoint/videoId",
    ]);
    if let Some(video_id) = video_id {
        let subtitle = subtitle_text(renderer.pointer("/subtitle/runs"));
        let duration = renderer
            .pointer("/subtitle/runs")
            .and_then(Value::as_array)
            .and_then(|runs| {
                split_runs(runs)
                    .iter()
                    .flatten()
                    .find_map(|text| parse_duration(text))
            });
        return Some(Track::new_sc(
            format!("yt:{video_id}"),
            title,
            subtitle,
            None,
            duration,
            Some(format!("https://music.youtube.com/watch?v={video_id}")),
            art,
            None,
            Some(Acc::Play),
        ));
    }

    let browse_id = first_str(renderer, &[
        "/navigationEndpoint/browseEndpoint/browseId",
        "/overlay/musicItemThumbnailOverlayRenderer/content/musicPlayButtonRenderer/playNavigationEndpoint/browseEndpoint/browseId",
        "/navigationEndpoint/watchEndpoint/playlistId",
        "/overlay/musicItemThumbnailOverlayRenderer/content/musicPlayButtonRenderer/playNavigationEndpoint/watchEndpoint/playlistId",
        "/title/runs/0/navigationEndpoint/browseEndpoint/browseId",
    ])?;
    let browse_id = normalize_browse_id(browse_id);
    let subtitle = subtitle_text(renderer.pointer("/subtitle/runs"));

    Some(browse_track(&browse_id, title, subtitle, art))
}

fn track_from_playlist_panel_item(item: &Value) -> Option<Track> {
    let renderer = item
        .pointer("/playlistPanelVideoWrapperRenderer/primaryRenderer")
        .unwrap_or(item);
    let data = renderer.get("playlistPanelVideoRenderer")?;
    if data.get("unplayableText").is_some() {
        return None;
    }

    let video_id = data.get("videoId").and_then(Value::as_str)?.to_string();
    let title = data
        .pointer("/title/runs")
        .and_then(Value::as_array)
        .and_then(|runs| runs_text(runs))?;
    let artist = subtitle_text(data.pointer("/longBylineText/runs"));
    let duration = data
        .pointer("/lengthText/runs/0/text")
        .and_then(Value::as_str)
        .and_then(parse_duration);
    let art = best_thumbnail(data.pointer("/thumbnail/thumbnails"));

    Some(Track::new_sc(
        format!("yt:{video_id}"),
        title,
        artist,
        None,
        duration,
        Some(format!("https://music.youtube.com/watch?v={video_id}")),
        art,
        None,
        Some(Acc::Play),
    ))
}

#[derive(Clone, Debug)]
struct AudioChoice {
    url: String,
    bitrate: u64,
    expires_at: u64,
}

fn pick_audio_stream(player: &Value, ql: Ql) -> Option<AudioChoice> {
    let mut preferred_mp4 = Vec::new();
    let mut mp4_fallback = Vec::new();
    let mut fallback = Vec::new();

    let empty_vec = Vec::new();
    let formats = player
        .pointer("/streamingData/adaptiveFormats")
        .and_then(Value::as_array)
        .unwrap_or(&empty_vec)
        .iter()
        .chain(
            player
                .pointer("/streamingData/formats")
                .and_then(Value::as_array)
                .unwrap_or(&empty_vec)
                .iter(),
        );

    for fmt in formats {
        let Some(mime) = fmt.get("mimeType").and_then(Value::as_str) else {
            continue;
        };
        let mime = mime.to_string();
        if !mime.starts_with("audio/") {
            continue;
        }

        let url = extract_stream_url(fmt);
        if url.is_empty() {
            continue;
        }

        let bitrate = fmt
            .get("bitrate")
            .and_then(Value::as_u64)
            .unwrap_or_default();

        let choice = AudioChoice {
            expires_at: stream_expiration(&url),
            url,
            bitrate,
        };

        if mime.starts_with("audio/mp4")
            && (mime.contains("mp4a.40.2") || mime.contains("mp4a.40.5"))
        {
            preferred_mp4.push(choice);
        } else if mime.starts_with("audio/mp4") || mime.contains("codecs=\"opus\"") {
            mp4_fallback.push(choice);
        } else {
            fallback.push(choice);
        }
    }

    let mut candidates = if !preferred_mp4.is_empty() {
        preferred_mp4
    } else if !mp4_fallback.is_empty() {
        mp4_fallback
    } else {
        fallback
    };
    if candidates.is_empty() {
        return None;
    }

    candidates.sort_by_key(|choice| choice.bitrate);
    match ql {
        Ql::Low => candidates.first().cloned(),
        Ql::High => candidates.last().cloned(),
        Ql::Med => candidates
            .iter()
            .min_by_key(|choice| choice.bitrate.abs_diff(128_000))
            .cloned(),
    }
}

fn extract_stream_url(fmt: &Value) -> String {
    if let Some(url) = fmt.get("url").and_then(Value::as_str) {
        let url = url.to_string();
        if !url.is_empty() {
            return url;
        }
    }

    if let Some(cipher) = fmt.get("signatureCipher").and_then(Value::as_str) {
        if let Some(url) = decode_cipher_url(cipher) {
            return url;
        }
    }

    if let Some(cipher) = fmt.get("cipher").and_then(Value::as_str) {
        if let Some(url) = decode_cipher_url(cipher) {
            return url;
        }
    }

    String::new()
}

fn decode_cipher_url(cipher: &str) -> Option<String> {
    let mut base_url = String::new();
    let mut signature = None;
    let mut sp = "signature".to_string();

    for pair in cipher.split('&') {
        let decoded = percent_decode_str(pair).decode_utf8_lossy();
        let (key, value) = decoded.split_once('=')?;
        match key {
            "url" => base_url = value.to_string(),
            "s" => signature = Some(value.to_string()),
            "sp" => sp = value.to_string(),
            _ => {}
        }
    }

    if base_url.is_empty() || signature.is_none() {
        return None;
    }

    let signature = signature.unwrap();
    let decrypted = decrypt_signature(&signature)?;

    let separator = if base_url.contains('?') { "&" } else { "?" };
    Some(format!("{}{}{}={}", base_url, separator, sp, decrypted))
}

fn decrypt_signature(sig: &str) -> Option<String> {
    let sig_chars: Vec<char> = sig.chars().collect();
    let len = sig_chars.len();

    if len < 2 {
        return Some(sig.to_string());
    }

    let mut reversed: Vec<char> = sig_chars.clone();
    reversed.reverse();
    let reversed_str: String = reversed.iter().collect();

    if len >= 3 && len <= 4 {
        let mut result = String::new();
        for (i, c) in sig_chars.iter().enumerate() {
            if i % 2 == 0 {
                result.push(*c);
            }
        }
        return Some(result);
    }

    let mut result = sig_chars[2..].to_vec();
    result.push(sig_chars[0]);
    result.push(sig_chars[1]);
    let rotated: String = result.iter().collect();

    if len >= 10 {
        return Some(format!(
            "{}{}{}",
            &rotated[..1],
            &reversed_str[1..len - 1],
            &rotated[len - 1..]
        ));
    }

    Some(format!("{}.{}", reversed_str, &sig[..2]))
}

fn best_thumbnail(node: Option<&Value>) -> Option<String> {
    node.and_then(Value::as_array)
        .and_then(|thumbs| {
            thumbs
                .iter()
                .filter_map(|thumb| {
                    let url = thumb.get("url").and_then(Value::as_str)?;
                    let width = thumb.get("width").and_then(Value::as_u64).unwrap_or(0);
                    let height = thumb.get("height").and_then(Value::as_u64).unwrap_or(0);
                    Some(((width.saturating_mul(height), width.max(height)), url))
                })
                .max_by_key(|((area, edge), _)| (*area, *edge))
                .map(|(_, url)| url)
        })
        .map(upgrade_thumbnail_url)
}

fn browse_track(
    browse_id: &str,
    title: String,
    subtitle: Option<String>,
    art: Option<String>,
) -> Track {
    let browse_id = normalize_browse_id(browse_id);
    Track::new_sc(
        format!("ytb:{browse_id}"),
        title,
        subtitle,
        None,
        None,
        Some(browse_link(&browse_id)),
        art,
        None,
        None,
    )
}

fn runs_text(runs: &[Value]) -> Option<String> {
    let text = runs
        .iter()
        .filter_map(|run| run.get("text").and_then(Value::as_str))
        .collect::<String>()
        .trim()
        .to_string();
    (!text.is_empty()).then_some(text)
}

fn subtitle_text(node: Option<&Value>) -> Option<String> {
    let runs = node.and_then(Value::as_array)?;
    let text = subtitle_text_from_runs(runs);
    (!text.is_empty()).then_some(text)
}

fn subtitle_text_from_runs(runs: &[Value]) -> String {
    split_runs(runs)
        .into_iter()
        .filter_map(|segment| {
            let joined = segment.join(" ").trim().to_string();
            (!joined.is_empty()).then_some(joined)
        })
        .collect::<Vec<_>>()
        .join(" • ")
}

fn first_str<'a>(node: &'a Value, pointers: &[&str]) -> Option<&'a str> {
    pointers
        .iter()
        .find_map(|pointer| node.pointer(pointer).and_then(Value::as_str))
}

fn split_runs(runs: &[Value]) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    let mut cur = Vec::new();

    for run in runs {
        let text = run
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();

        if text.is_empty() {
            continue;
        }

        if text == "•" || text.contains("•") {
            if !cur.is_empty() {
                out.push(cur);
                cur = Vec::new();
            }
            continue;
        }

        cur.push(text);
    }

    if !cur.is_empty() {
        out.push(cur);
    }

    out
}

fn parse_duration(text: &str) -> Option<u64> {
    let parts = text
        .split(':')
        .map(|part| part.parse::<u64>().ok())
        .collect::<Option<Vec<_>>>()?;

    match parts.as_slice() {
        [minutes, seconds] => Some(minutes * 60 + seconds),
        [hours, minutes, seconds] => Some(hours * 3600 + minutes * 60 + seconds),
        _ => None,
    }
}

fn normalize_browse_id(raw: &str) -> String {
    let raw = raw.trim().trim_start_matches("ytb:");
    if raw.starts_with("VL")
        || raw.starts_with("MPRE")
        || raw.starts_with("MPLA")
        || raw.starts_with("UC")
        || raw.starts_with("FEmusic_")
    {
        raw.to_string()
    } else {
        format!("VL{raw}")
    }
}

fn browse_link(browse_id: &str) -> String {
    if let Some(playlist_id) = browse_id.strip_prefix("VL") {
        format!("https://music.youtube.com/playlist?list={playlist_id}")
    } else {
        format!("https://music.youtube.com/browse/{browse_id}")
    }
}

fn upgrade_thumbnail_url(url: &str) -> String {
    const GOOGLE_THUMB_EDGE: u32 = 720;
    const YTIMG_THUMB_VARIANT: &str = "sddefault.jpg";

    let mut out = url.to_string();

    if (out.contains("googleusercontent.com") || out.contains("ggpht.com"))
        && out.rsplit_once('=').is_some()
    {
        if let Some((base, _)) = out.rsplit_once('=') {
            return format!("{base}=w{GOOGLE_THUMB_EDGE}-h{GOOGLE_THUMB_EDGE}-l90-rj");
        }
    }

    if out.contains("ytimg.com/vi/") {
        out = out.replace("/default.jpg", &format!("/{YTIMG_THUMB_VARIANT}"));
        out = out.replace("/mqdefault.jpg", &format!("/{YTIMG_THUMB_VARIANT}"));
        out = out.replace("/hqdefault.jpg", &format!("/{YTIMG_THUMB_VARIANT}"));
    }

    out
}

fn parse_search_suggestions(rsp: &Value, lim: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    collect_search_suggestions(rsp, &mut seen, &mut out, lim);
    out
}

fn collect_search_suggestions(
    node: &Value,
    seen: &mut HashSet<String>,
    out: &mut Vec<String>,
    lim: usize,
) {
    if out.len() >= lim {
        return;
    }

    match node {
        Value::Object(map) => {
            if let Some(text) = map
                .get("searchSuggestionRenderer")
                .and_then(search_suggestion_text)
                .or_else(|| {
                    map.get("historySuggestionRenderer")
                        .and_then(search_suggestion_text)
                })
            {
                if seen.insert(text.clone()) {
                    out.push(text);
                    if out.len() >= lim {
                        return;
                    }
                }
            }
            for value in map.values() {
                collect_search_suggestions(value, seen, out, lim);
                if out.len() >= lim {
                    return;
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_search_suggestions(item, seen, out, lim);
                if out.len() >= lim {
                    return;
                }
            }
        }
        _ => {}
    }
}

fn search_suggestion_text(renderer: &Value) -> Option<String> {
    renderer
        .pointer("/suggestion/runs")
        .and_then(Value::as_array)
        .and_then(|runs| runs_text(runs))
        .or_else(|| {
            renderer
                .pointer("/suggestion/simpleText")
                .and_then(Value::as_str)
                .map(|value| value.trim().to_string())
        })
}

fn is_artist_page(rsp: &Value) -> bool {
    rsp.pointer("/header/musicImmersiveHeaderRenderer")
        .is_some()
}

fn artist_page_title(rsp: &Value) -> Option<String> {
    rsp.pointer("/header/musicImmersiveHeaderRenderer/title/runs")
        .and_then(Value::as_array)
        .and_then(|runs| runs_text(runs))
}

fn browse_page_art(rsp: &Value) -> Option<String> {
    best_thumbnail(
        rsp.pointer(
            "/header/musicResponsiveHeaderRenderer/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails",
        )
        .or_else(|| {
            rsp.pointer(
                "/header/musicDetailHeaderRenderer/thumbnail/croppedSquareThumbnailRenderer/thumbnail/thumbnails",
            )
        })
        .or_else(|| {
            rsp.pointer(
                "/header/musicDetailHeaderRenderer/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails",
            )
        })
        .or_else(|| {
            rsp.pointer(
                "/header/musicEditablePlaylistDetailHeaderRenderer/header/musicResponsiveHeaderRenderer/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails",
            )
        })
        .or_else(|| {
            rsp.pointer(
                "/contents/twoColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents/0/musicResponsiveHeaderRenderer/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails",
            )
        })
        .or_else(|| rsp.pointer("/microformat/microformatDataRenderer/thumbnail/thumbnails")),
    )
}

fn browse_page_title(rsp: &Value) -> Option<String> {
    first_str(rsp, &[
        "/header/musicResponsiveHeaderRenderer/title/runs/0/text",
        "/header/musicDetailHeaderRenderer/title/runs/0/text",
        "/header/musicEditablePlaylistDetailHeaderRenderer/header/musicResponsiveHeaderRenderer/title/runs/0/text",
        "/contents/twoColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents/0/musicResponsiveHeaderRenderer/title/runs/0/text",
    ])
    .map(|value| value.to_string())
}

fn video_duration_secs(player: &Value) -> Option<u64> {
    player
        .pointer("/videoDetails/lengthSeconds")
        .and_then(Value::as_str)
        .and_then(|value| value.parse::<u64>().ok())
        .or_else(|| {
            player
                .pointer("/videoDetails/lengthSeconds")
                .and_then(Value::as_u64)
        })
}

fn playability_status(player: &Value) -> Option<&str> {
    player
        .pointer("/playabilityStatus/status")
        .and_then(Value::as_str)
}

fn playability_reason(player: &Value) -> Option<String> {
    player
        .pointer("/playabilityStatus/reason")
        .and_then(Value::as_str)
        .map(|v| v.to_string())
}

fn explain_playability_reason(reason: String, authenticated: bool) -> String {
    if authenticated || !reason.to_ascii_lowercase().contains("not a bot") {
        return reason;
    }

    format!(
        "{reason}. YouTube is blocking guest playback. Import YouTube browser headers with \
         'rustplayer auth headers-file headers.json'. On Windows, Chromium browser cookies may \
         also require running RustPlayer as Administrator before they can be reused automatically."
    )
}

fn stream_expiration(url: &str) -> u64 {
    Url::parse(url)
        .ok()
        .and_then(|parsed| {
            parsed
                .query_pairs()
                .find(|(key, _)| key == "expire")
                .and_then(|(_, value)| value.parse::<u64>().ok())
        })
        .unwrap_or_else(|| now() + 1800)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn parse_total_len_from_content_range(value: &str) -> Option<u64> {
    value
        .split_once('/')
        .and_then(|(_, total)| total.parse::<u64>().ok())
}

fn parse_cookie_header(raw: &str) -> HashMap<String, String> {
    raw.split(';')
        .filter_map(|part| {
            let part = part.trim();
            let split = part.find('=')?;
            let key = part[..split].trim();
            let value = part[split + 1..].trim();
            (!key.is_empty()).then(|| (key.to_string(), value.to_string()))
        })
        .collect()
}

fn sha1_hex(input: &str) -> String {
    let mut sha1 = Sha1::new();
    sha1.update(input.as_bytes());
    format!("{:x}", sha1.finalize())
}

fn run_text(node: Option<&Value>) -> Option<String> {
    node.and_then(Value::as_array)
        .and_then(|runs| runs.first())
        .and_then(|run| run.get("text"))
        .and_then(Value::as_str)
        .map(|text| text.to_string())
}

pub fn is_soundcloud_url(url: &str) -> bool {
    is_youtube_url(url)
}

pub fn build_auth_link(url: &str) -> String {
    url.to_string()
}

fn is_youtube_url(url: &str) -> bool {
    extract_video_id(url).is_some()
}

fn extract_video_id(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.len() == 11
        && trimmed
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
    {
        return Some(trimmed.to_string());
    }

    let parsed = Url::parse(trimmed).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();

    if host.contains("youtu.be") {
        return parsed
            .path_segments()
            .and_then(|mut segments| segments.next())
            .map(|segment| segment.to_string());
    }

    if host.contains("youtube.com") || host.contains("music.youtube.com") {
        if let Some((_, value)) = parsed.query_pairs().find(|(key, _)| key == "v") {
            return Some(value.to_string());
        }

        let segments = parsed.path_segments()?.collect::<Vec<_>>();
        if segments.first().copied() == Some("watch") {
            return None;
        }
        if segments.first().copied() == Some("shorts") && segments.len() > 1 {
            return Some(segments[1].to_string());
        }
        if segments.first().copied() == Some("embed") && segments.len() > 1 {
            return Some(segments[1].to_string());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playback::decode_m4a_bytes;

    #[test]
    fn youtube_url_detection() {
        assert!(is_soundcloud_url(
            "https://music.youtube.com/watch?v=JhulBGMA7G4"
        ));
        assert!(is_soundcloud_url("https://youtu.be/JhulBGMA7G4"));
        assert!(is_soundcloud_url("JhulBGMA7G4"));
        assert!(!is_soundcloud_url("https://example.com"));
    }

    #[test]
    fn duration_parser() {
        assert_eq!(parse_duration("3:47"), Some(227));
        assert_eq!(parse_duration("1:02:03"), Some(3723));
        assert_eq!(parse_duration("abc"), None);
    }

    #[test]
    fn quality_prefers_expected_bitrate() {
        let player = json!({
            "streamingData": {
                "adaptiveFormats": [
                    { "mimeType": "audio/mp4; codecs=\"mp4a.40.5\"", "url": "https://example.com/a?expire=10", "bitrate": 50000 },
                    { "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"", "url": "https://example.com/b?expire=10", "bitrate": 128000 },
                    { "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"", "url": "https://example.com/c?expire=10", "bitrate": 192000 }
                ]
            }
        });

        assert_eq!(
            pick_audio_stream(&player, Ql::Low).unwrap().url,
            "https://example.com/b?expire=10"
        );
        assert_eq!(
            pick_audio_stream(&player, Ql::Med).unwrap().url,
            "https://example.com/b?expire=10"
        );
        assert_eq!(
            pick_audio_stream(&player, Ql::High).unwrap().url,
            "https://example.com/c?expire=10"
        );
    }

    #[test]
    fn parse_total_length_from_range_header() {
        assert_eq!(
            parse_total_len_from_content_range("bytes 0-1023/2048"),
            Some(2048)
        );
        assert_eq!(
            parse_total_len_from_content_range("bytes */2048"),
            Some(2048)
        );
        assert_eq!(parse_total_len_from_content_range("invalid"), None);
    }

    #[test]
    fn thumbnail_upgrade_prefers_moderate_google_images() {
        let url = "https://lh3.googleusercontent.com/abc=w120-h120-l90-rj";
        assert_eq!(
            upgrade_thumbnail_url(url),
            "https://lh3.googleusercontent.com/abc=w720-h720-l90-rj"
        );
    }

    #[test]
    fn thumbnail_upgrade_prefers_moderate_ytimg() {
        let url = "https://i.ytimg.com/vi/xyz/hqdefault.jpg";
        assert_eq!(
            upgrade_thumbnail_url(url),
            "https://i.ytimg.com/vi/xyz/sddefault.jpg"
        );
    }

    #[test]
    fn artist_browse_ids_are_not_rewritten() {
        assert_eq!(
            normalize_browse_id("UCmMUZbaYdNH0bEd1PAlAqsA"),
            "UCmMUZbaYdNH0bEd1PAlAqsA"
        );
    }

    #[test]
    fn parses_search_suggestions_from_renderer() {
        let rsp = json!({
            "contents": [
                {
                    "searchSuggestionRenderer": {
                        "suggestion": {
                            "runs": [
                                { "text": "fade" },
                                { "text": "d alan walker" }
                            ]
                        }
                    }
                },
                {
                    "historySuggestionRenderer": {
                        "suggestion": {
                            "simpleText": "faded remix"
                        }
                    }
                }
            ]
        });

        assert_eq!(
            parse_search_suggestions(&rsp, 8),
            vec!["faded alan walker".to_string(), "faded remix".to_string()]
        );
    }

    #[test]
    fn collection_tracks_use_album_cover_from_music_shelf() {
        let rsp = json!({
            "contents": {
                "twoColumnBrowseResultsRenderer": {
                    "secondaryContents": {
                        "sectionListRenderer": {
                            "contents": [
                                {
                                    "musicShelfRenderer": {
                                        "contents": [
                                            {
                                                "musicResponsiveListItemRenderer": {
                                                    "flexColumns": [
                                                        {
                                                            "musicResponsiveListItemFlexColumnRenderer": {
                                                                "text": {
                                                                    "runs": [
                                                                        {
                                                                            "text": "Welcome To New York",
                                                                            "navigationEndpoint": {
                                                                                "watchEndpoint": {
                                                                                    "videoId": "FsGdznlfE2U"
                                                                                }
                                                                            }
                                                                        }
                                                                    ]
                                                                }
                                                            }
                                                        },
                                                        {
                                                            "musicResponsiveListItemFlexColumnRenderer": {
                                                                "text": {
                                                                    "runs": [
                                                                        { "text": "Taylor Swift" }
                                                                    ]
                                                                }
                                                            }
                                                        }
                                                    ],
                                                    "fixedColumns": [
                                                        {
                                                            "musicResponsiveListItemFixedColumnRenderer": {
                                                                "text": {
                                                                    "runs": [
                                                                        { "text": "3:33" }
                                                                    ]
                                                                }
                                                            }
                                                        }
                                                    ]
                                                }
                                            }
                                        ]
                                    }
                                }
                            ]
                        }
                    },
                    "tabs": [
                        {
                            "tabRenderer": {
                                "content": {
                                    "sectionListRenderer": {
                                        "contents": [
                                            {
                                                "musicResponsiveHeaderRenderer": {
                                                    "thumbnail": {
                                                        "musicThumbnailRenderer": {
                                                            "thumbnail": {
                                                                "thumbnails": [
                                                                    {
                                                                        "url": "https://lh3.googleusercontent.com/abc=w120-h120-l90-rj",
                                                                        "width": 120,
                                                                        "height": 120
                                                                    }
                                                                ]
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        ]
                                    }
                                }
                            }
                        }
                    ]
                }
            }
        });

        let tracks = parse_collection_tracks(&rsp, 16);

        assert_eq!(tracks.len(), 1);
        assert_eq!(
            tracks[0].art.as_deref(),
            Some("https://lh3.googleusercontent.com/abc=w720-h720-l90-rj")
        );
    }

    #[test]
    fn collection_tracks_prefer_direct_thumbnail_when_present() {
        let rsp = json!({
            "contents": {
                "twoColumnBrowseResultsRenderer": {
                    "secondaryContents": {
                        "sectionListRenderer": {
                            "contents": [
                                {
                                    "musicPlaylistShelfRenderer": {
                                        "contents": [
                                            {
                                                "musicResponsiveListItemRenderer": {
                                                    "thumbnail": {
                                                        "musicThumbnailRenderer": {
                                                            "thumbnail": {
                                                                "thumbnails": [
                                                                    {
                                                                        "url": "https://lh3.googleusercontent.com/track=w120-h120-l90-rj",
                                                                        "width": 120,
                                                                        "height": 120
                                                                    }
                                                                ]
                                                            }
                                                        }
                                                    },
                                                    "flexColumns": [
                                                        {
                                                            "musicResponsiveListItemFlexColumnRenderer": {
                                                                "text": {
                                                                    "runs": [
                                                                        {
                                                                            "text": "Track",
                                                                            "navigationEndpoint": {
                                                                                "watchEndpoint": {
                                                                                    "videoId": "video1234567"
                                                                                }
                                                                            }
                                                                        }
                                                                    ]
                                                                }
                                                            }
                                                        },
                                                        {
                                                            "musicResponsiveListItemFlexColumnRenderer": {
                                                                "text": {
                                                                    "runs": [
                                                                        { "text": "Artist" }
                                                                    ]
                                                                }
                                                            }
                                                        }
                                                    ]
                                                }
                                            }
                                        ]
                                    }
                                }
                            ]
                        }
                    },
                    "tabs": [
                        {
                            "tabRenderer": {
                                "content": {
                                    "sectionListRenderer": {
                                        "contents": [
                                            {
                                                "musicResponsiveHeaderRenderer": {
                                                    "thumbnail": {
                                                        "musicThumbnailRenderer": {
                                                            "thumbnail": {
                                                                "thumbnails": [
                                                                    {
                                                                        "url": "https://lh3.googleusercontent.com/album=w120-h120-l90-rj",
                                                                        "width": 120,
                                                                        "height": 120
                                                                    }
                                                                ]
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        ]
                                    }
                                }
                            }
                        }
                    ]
                }
            }
        });

        let tracks = parse_collection_tracks(&rsp, 16);

        assert_eq!(tracks.len(), 1);
        assert_eq!(
            tracks[0].art.as_deref(),
            Some("https://lh3.googleusercontent.com/track=w720-h720-l90-rj")
        );
    }

    #[test]
    #[ignore = "live network smoke test for YouTube playback decoding"]
    fn live_mp4_decode_smoke() {
        let mut client = YouTubeClient::new(Ql::Low);
        let track = client
            .resolve("https://www.youtube.com/watch?v=dQw4w9WgXcQ")
            .expect("resolve should succeed")
            .expect("video should resolve");
        let stream = client.stream(&track).expect("stream should resolve");
        let bytes = client
            .download_stream(&stream.url)
            .expect("stream should download");
        let (_, _, samples) = decode_m4a_bytes(bytes).expect("returned bytes should decode");

        assert!(samples.len() >= 64, "decoder should yield samples");
    }
}
