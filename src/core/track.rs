use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub path: Option<PathBuf>, // None for SoundCloud tracks
    pub title: String,
    pub artist: Option<String>,
    pub duration_seconds: Option<u64>,
    pub url: Option<String>, // For SoundCloud streaming
    pub track_type: TrackType,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TrackType {
    Local,
    SoundCloud,
}

impl Track {
    pub fn new_local(id: String, path: PathBuf, title: String, artist: Option<String>, duration_seconds: Option<u64>) -> Self {
        Track { 
            id, 
            path: Some(path), 
            title, 
            artist, 
            duration_seconds,
            url: None,
            track_type: TrackType::Local,
        }
    }

    pub fn new_soundcloud(id: String, title: String, artist: Option<String>, duration_seconds: Option<u64>, url: String) -> Self {
        Track {
            id,
            path: None,
            title,
            artist,
            duration_seconds,
            url: Some(url),
            track_type: TrackType::SoundCloud,
        }
    }
}
