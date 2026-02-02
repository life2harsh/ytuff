use crate::core::track::Track;
use anyhow::Result;
use reqwest;
use url::Url;

pub struct SoundCloudClient {
    client: reqwest::Client,
    client_id: Option<String>,
}

impl SoundCloudClient {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            client_id: None,
        }
    }

    pub fn with_client_id(client_id: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            client_id: Some(client_id),
        }
    }

    pub async fn search_tracks(&self, query: &str, limit: usize) -> Result<Vec<Track>> {
        println!("SoundCloud search for '{}' (limit: {})", query, limit);
        Ok(vec![])
    }

    pub async fn get_track_stream_url(&self, track_id: &str) -> Result<Option<String>> {
        println!("Getting stream URL for track: {}", track_id);
        Ok(None)
    }

    pub async fn resolve_url(&self, url: &str) -> Result<Option<Track>> {
        if url.contains("soundcloud.com") {
            println!("Resolving SoundCloud URL: {}", url);
        }
        Ok(None)
    }
}

pub fn is_soundcloud_url(url: &str) -> bool {
    if let Ok(parsed_url) = Url::parse(url) {
        if let Some(host) = parsed_url.host_str() {
            return host.contains("soundcloud.com") || host.contains("snd.sc");
        }
    }
    false
}