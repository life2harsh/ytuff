use crate::appdata::AppPaths;
use crate::core::track::Track;
use anyhow::{Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use reqwest::blocking::Client;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

const LRCLIB_GET: &str = "https://lrclib.net/api/get";
const LRCLIB_SEARCH: &str = "https://lrclib.net/api/search";

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct LyricsDoc {
    pub provider: String,
    pub track_name: Option<String>,
    pub artist_name: Option<String>,
    pub plain: Option<String>,
    pub synced: Option<String>,
    pub instrumental: bool,
}

#[derive(Clone)]
pub struct LyricsClient {
    http: Client,
    paths: AppPaths,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LrcLibItem {
    track_name: Option<String>,
    artist_name: Option<String>,
    plain_lyrics: Option<String>,
    synced_lyrics: Option<String>,
    instrumental: Option<bool>,
}

impl LyricsClient {
    pub fn new(paths: AppPaths) -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(20))
            .user_agent("rustplayer/0.1.0")
            .build()
            .unwrap_or_else(|_| Client::new());
        Self { http, paths }
    }

    pub fn lookup_track(&self, track: &Track) -> Result<Option<LyricsDoc>> {
        self.paths.ensure()?;
        let cache = self.cache_path(&track.id);
        if cache.exists() {
            let txt = fs::read_to_string(&cache)
                .with_context(|| format!("Could not read {}", cache.display()))?;
            let doc = serde_json::from_str::<LyricsDoc>(&txt)
                .with_context(|| format!("Could not parse {}", cache.display()))?;
            return Ok(Some(doc));
        }

        let fetched = self.fetch_track(track)?;
        if let Some(doc) = fetched.as_ref() {
            fs::write(&cache, serde_json::to_vec_pretty(doc)?)
                .with_context(|| format!("Could not write {}", cache.display()))?;
        }
        Ok(fetched)
    }

    pub fn cached_track(&self, track: &Track) -> Result<Option<LyricsDoc>> {
        let cache = self.cache_path(&track.id);
        if !cache.exists() {
            return Ok(None);
        }
        let txt = fs::read_to_string(&cache)
            .with_context(|| format!("Could not read {}", cache.display()))?;
        let doc = serde_json::from_str::<LyricsDoc>(&txt)
            .with_context(|| format!("Could not parse {}", cache.display()))?;
        Ok(Some(doc))
    }

    fn fetch_track(&self, track: &Track) -> Result<Option<LyricsDoc>> {
        let mut params = vec![
            ("track_name".to_string(), track.title.clone()),
            ("artist_name".to_string(), track.who()),
        ];
        if let Some(duration) = track.dur {
            params.push(("duration".to_string(), duration.to_string()));
        }

        let rsp = self.http.get(LRCLIB_GET).query(&params).send()?;
        match rsp.status() {
            StatusCode::OK => {
                let item = rsp.json::<LrcLibItem>()?;
                return Ok(Some(map_item(item)));
            }
            StatusCode::NOT_FOUND => {}
            _ => {
                rsp.error_for_status_ref()?;
            }
        }

        let rsp = self.http.get(LRCLIB_SEARCH).query(&params).send()?;
        if rsp.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let items = rsp.error_for_status()?.json::<Vec<LrcLibItem>>()?;
        Ok(items.into_iter().next().map(map_item))
    }

    fn cache_path(&self, track_id: &str) -> PathBuf {
        let mut path = self.paths.lyrics_dir.clone();
        path.push(format!(
            "{}.json",
            URL_SAFE_NO_PAD.encode(track_id.as_bytes())
        ));
        path
    }
}

fn map_item(item: LrcLibItem) -> LyricsDoc {
    LyricsDoc {
        provider: "lrclib".to_string(),
        track_name: item.track_name,
        artist_name: item.artist_name,
        plain: item.plain_lyrics.filter(|v| !v.trim().is_empty()),
        synced: item.synced_lyrics.filter(|v| !v.trim().is_empty()),
        instrumental: item.instrumental.unwrap_or(false),
    }
}
