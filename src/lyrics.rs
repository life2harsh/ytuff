use crate::appdata::AppPaths;
use crate::core::track::Track;
use anyhow::{Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use reqwest::blocking::Client;
use reqwest::StatusCode;
use serde::de::Deserializer;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
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
    id: Option<u64>,
    track_name: Option<String>,
    artist_name: Option<String>,
    album_name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_duration_opt")]
    duration: Option<u64>,
    plain_lyrics: Option<String>,
    synced_lyrics: Option<String>,
    instrumental: Option<bool>,
}

impl LyricsClient {
    pub fn new(paths: AppPaths) -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(20))
            .user_agent(concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION")))
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
        let title = track.title.trim();
        if title.is_empty() {
            return Ok(None);
        }

        let artist = track_artist(track);
        if artist.is_some() || track.dur.is_some() {
            if let Some(item) = self.get_track(title, artist.as_deref(), track.dur)? {
                return Ok(Some(map_item(item)));
            }
        }
        let items = self.search_track(title, artist.as_deref())?;
        Ok(pick_best_match(title, artist.as_deref(), track.dur, items))
    }

    fn get_track(
        &self,
        title: &str,
        artist: Option<&str>,
        duration: Option<u64>,
    ) -> Result<Option<LrcLibItem>> {
        let mut params = vec![("track_name".to_string(), title.to_string())];
        if let Some(artist) = artist {
            params.push(("artist_name".to_string(), artist.to_string()));
        }
        if let Some(duration) = duration {
            params.push(("duration".to_string(), duration.to_string()));
        }

        let rsp = self.http.get(LRCLIB_GET).query(&params).send()?;
        if matches!(
            rsp.status(),
            StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND | StatusCode::UNPROCESSABLE_ENTITY
        ) {
            return Ok(None);
        }
        Ok(Some(rsp.error_for_status()?.json::<LrcLibItem>()?))
    }

    fn search_track(&self, title: &str, artist: Option<&str>) -> Result<Vec<LrcLibItem>> {
        let mut queries = vec![{
            let mut params = vec![("track_name".to_string(), title.to_string())];
            if let Some(artist) = artist {
                params.push(("artist_name".to_string(), artist.to_string()));
            }
            params
        }];

        let combined = artist
            .map(|artist| format!("{title} {artist}"))
            .unwrap_or_else(|| title.to_string());
        if !combined.trim().is_empty() {
            queries.push(vec![("q".to_string(), combined)]);
        }

        let mut items = Vec::new();
        let mut seen = HashSet::new();
        for params in queries {
            let rsp = self.http.get(LRCLIB_SEARCH).query(&params).send()?;
            if rsp.status() == StatusCode::NOT_FOUND {
                continue;
            }
            let batch = rsp.error_for_status()?.json::<Vec<LrcLibItem>>()?;
            for item in batch {
                let key = item_key(&item);
                if seen.insert(key) {
                    items.push(item);
                }
            }
        }
        Ok(items)
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

fn pick_best_match(
    title: &str,
    artist: Option<&str>,
    duration: Option<u64>,
    items: Vec<LrcLibItem>,
) -> Option<LyricsDoc> {
    let mut best = None::<(i32, u16, u16, LrcLibItem)>;

    for item in items {
        let candidate_title = item.track_name.as_deref().unwrap_or_default();
        let title_score = compare_title(title, candidate_title);
        if title_score < 65 {
            continue;
        }
        let artist_score = match artist {
            Some(artist) => compare_artist(artist, item.artist_name.as_deref().unwrap_or_default()),
            None => 70,
        };
        if artist.is_some() && artist_score < 35 {
            continue;
        }
        let total = title_score as i32 * 7
            + artist_score as i32 * 3
            + compare_duration(duration, item.duration);
        let replace = match best.as_ref() {
            Some((best_total, best_title, best_artist, _)) => {
                total > *best_total
                    || (total == *best_total
                        && (title_score > *best_title
                            || (title_score == *best_title && artist_score > *best_artist)))
            }
            None => true,
        };
        if replace {
            best = Some((total, title_score, artist_score, item));
        }
    }

    let (total, title_score, artist_score, item) = best?;
    if total < 520 {
        return None;
    }
    if artist.is_none() && title_score < 78 {
        return None;
    }
    if artist.is_some() && artist_score < 45 && title_score < 90 {
        return None;
    }
    Some(map_item(item))
}

fn compare_title(expected: &str, candidate: &str) -> u16 {
    let expected_raw = normalize_phrase(expected, false);
    let candidate_raw = normalize_phrase(candidate, false);
    if expected_raw.is_empty() || candidate_raw.is_empty() {
        return 0;
    }
    if expected_raw == candidate_raw {
        return 100;
    }
    let expected_clean = normalize_phrase(expected, true);
    let candidate_clean = normalize_phrase(candidate, true);
    if !expected_clean.is_empty() && expected_clean == candidate_clean {
        return 97;
    }

    let expected_tokens = title_tokens(expected);
    let candidate_tokens = title_tokens(candidate);
    if !expected_tokens.is_empty() && expected_tokens == candidate_tokens {
        return 95;
    }

    let overlap = overlap_score(&expected_tokens, &candidate_tokens);
    let prefix = prefix_bonus(&expected_tokens, &candidate_tokens);
    overlap.saturating_add(prefix).min(93)
}

fn compare_artist(expected: &str, candidate: &str) -> u16 {
    let expected_raw = normalize_phrase(expected, false);
    let candidate_raw = normalize_phrase(candidate, false);
    if expected_raw.is_empty() || candidate_raw.is_empty() {
        return 0;
    }
    if expected_raw == candidate_raw {
        return 100;
    }
    let expected_aliases = artist_aliases(expected);
    let candidate_aliases = artist_aliases(candidate);
    if expected_aliases
        .iter()
        .any(|alias| candidate_aliases.iter().any(|other| other == alias))
    {
        return 94;
    }
    overlap_score(
        &tokens_from_aliases(&expected_aliases),
        &tokens_from_aliases(&candidate_aliases),
    )
}

fn compare_duration(expected: Option<u64>, candidate: Option<u64>) -> i32 {
    match (expected, candidate) {
        (Some(expected), Some(candidate)) => {
            let diff = expected.abs_diff(candidate);
            if diff <= 2 {
                18
            } else if diff <= 5 {
                10
            } else if diff <= 10 {
                4
            } else if diff <= 20 {
                -8
            } else {
                -18
            }
        }
        _ => 0,
    }
}

fn track_artist(track: &Track) -> Option<String> {
    track
        .artist
        .clone()
        .or_else(|| track.user.clone())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("unknown"))
}

fn item_key(item: &LrcLibItem) -> String {
    if let Some(id) = item.id {
        return format!("id:{id}");
    }
    format!(
        "{}|{}|{}|{}",
        normalize_phrase(item.track_name.as_deref().unwrap_or_default(), true),
        normalize_phrase(item.artist_name.as_deref().unwrap_or_default(), false),
        normalize_phrase(item.album_name.as_deref().unwrap_or_default(), false),
        item.duration.unwrap_or_default()
    )
}

fn normalize_phrase(value: &str, strip_brackets: bool) -> String {
    let mut out = String::with_capacity(value.len());
    let mut depth = 0u32;

    for ch in value.chars() {
        if strip_brackets {
            match ch {
                '(' | '[' | '{' => {
                    depth += 1;
                    continue;
                }
                ')' | ']' | '}' => {
                    depth = depth.saturating_sub(1);
                    continue;
                }
                _ if depth > 0 => continue,
                _ => {}
            }
        }

        if ch.is_alphanumeric() {
            for mapped in ch.to_lowercase() {
                out.push(mapped);
            }
        } else {
            out.push(' ');
        }
    }

    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn title_tokens(value: &str) -> Vec<String> {
    tokens(
        value,
        &[
            "a",
            "an",
            "and",
            "clean",
            "edit",
            "explicit",
            "feat",
            "featuring",
            "ft",
            "live",
            "mono",
            "official",
            "remaster",
            "remastered",
            "stereo",
            "the",
            "version",
        ],
    )
}

fn artist_aliases(value: &str) -> Vec<String> {
    let normalized = value
        .replace(" featuring ", "|")
        .replace(" Featuring ", "|")
        .replace(" feat. ", "|")
        .replace(" feat ", "|")
        .replace(" ft. ", "|")
        .replace(" ft ", "|")
        .replace(" & ", "|")
        .replace(" and ", "|")
        .replace(" x ", "|")
        .replace('/', "|")
        .replace(',', "|")
        .replace(';', "|");

    let mut aliases = vec![normalize_phrase(value, false)];
    for part in normalized.split('|') {
        let alias = normalize_phrase(part, false);
        if !alias.is_empty() && !aliases.iter().any(|existing| existing == &alias) {
            aliases.push(alias);
        }
    }
    aliases.retain(|alias| !alias.is_empty());
    aliases
}

fn tokens_from_aliases(aliases: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for alias in aliases {
        for token in alias.split_whitespace() {
            if seen.insert(token.to_string()) {
                out.push(token.to_string());
            }
        }
    }
    out
}

fn tokens(value: &str, stop_words: &[&str]) -> Vec<String> {
    let stop_words = stop_words.iter().copied().collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for token in normalize_phrase(value, true).split_whitespace() {
        if stop_words.contains(token) {
            continue;
        }
        let token = token.to_string();
        if seen.insert(token.clone()) {
            out.push(token);
        }
    }
    out
}

fn overlap_score(left: &[String], right: &[String]) -> u16 {
    if left.is_empty() || right.is_empty() {
        return 0;
    }
    let right = right.iter().map(String::as_str).collect::<HashSet<_>>();
    let hits = left
        .iter()
        .filter(|token| right.contains(token.as_str()))
        .count() as u16;
    hits * 100 / left.len().max(right.len()) as u16
}

fn prefix_bonus(left: &[String], right: &[String]) -> u16 {
    if left.len() < 2 || right.len() < 2 {
        return 0;
    }
    let left_joined = left.join(" ");
    let right_joined = right.join(" ");
    if left_joined.starts_with(&right_joined) || right_joined.starts_with(&left_joined) {
        8
    } else {
        0
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

fn deserialize_duration_opt<'de, D>(deserializer: D) -> std::result::Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    let Some(value) = value else {
        return Ok(None);
    };

    let duration = match value {
        serde_json::Value::Number(number) => number
            .as_u64()
            .or_else(|| number.as_f64().map(|value| value.round() as u64)),
        serde_json::Value::String(text) => text
            .parse::<u64>()
            .ok()
            .or_else(|| text.parse::<f64>().ok().map(|value| value.round() as u64)),
        _ => None,
    };

    Ok(duration)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(title: &str, artist: &str, duration: u64) -> LrcLibItem {
        LrcLibItem {
            id: Some(1),
            track_name: Some(title.to_string()),
            artist_name: Some(artist.to_string()),
            album_name: None,
            duration: Some(duration),
            plain_lyrics: Some("plain".to_string()),
            synced_lyrics: None,
            instrumental: Some(false),
        }
    }

    #[test]
    fn title_compare_rejects_loose_substring_match() {
        assert!(compare_title("ivy", "Poison Ivy") < 65);
    }

    #[test]
    fn artist_compare_accepts_featured_variants() {
        assert!(compare_artist("Taylor Swift", "Taylor Swift feat. Bon Iver") >= 90);
    }

    #[test]
    fn best_match_prefers_title_and_artist() {
        let picked = pick_best_match(
            "ivy",
            Some("Taylor Swift"),
            Some(260),
            vec![
                item("Poison Ivy", "The Cool Kids", 180),
                item("ivy", "Taylor Swift", 260),
            ],
        )
        .expect("should match");

        assert_eq!(picked.track_name.as_deref(), Some("ivy"));
        assert_eq!(picked.artist_name.as_deref(), Some("Taylor Swift"));
    }

    #[test]
    fn duration_deserializer_accepts_float_values() {
        let item = serde_json::from_str::<LrcLibItem>(
            r#"{
                "id": 1,
                "trackName": "ivy",
                "artistName": "Taylor Swift",
                "duration": 260.0
            }"#,
        )
        .expect("duration should deserialize");

        assert_eq!(item.duration, Some(260));
    }
}
