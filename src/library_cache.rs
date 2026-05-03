use crate::appdata::AppPaths;
use crate::core::track::Track;
use crate::sources::local;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const CACHE_VERSION: u32 = 1;
const CACHE_FILE_NAME: &str = "library-cache.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CachedTrack {
    modified_secs: u64,
    file_len: u64,
    track: Track,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct LibraryCache {
    version: u32,
    #[serde(default)]
    files: HashMap<String, CachedTrack>,
}

impl Default for LibraryCache {
    fn default() -> Self {
        Self {
            version: CACHE_VERSION,
            files: HashMap::new(),
        }
    }
}

pub fn scan_paths_cached(paths: &AppPaths, roots: &[PathBuf]) -> Result<Vec<Track>> {
    paths.ensure()?;

    let mut cache = load_cache(paths);
    let mut dirty = false;
    let mut tracks = Vec::new();

    for root in roots {
        if !root.exists() || !root.is_dir() {
            continue;
        }

        for entry in walkdir::WalkDir::new(root)
            .follow_links(true)
            .into_iter()
            .filter_map(|entry| entry.ok())
        {
            let path = entry.path();
            if !path.is_file() || !is_audio_file(path) {
                continue;
            }

            let (track, changed) = cached_or_read_track(&mut cache, path)?;
            dirty |= changed;
            tracks.push(track);
        }
    }

    if dirty {
        save_cache(paths, &cache)?;
    }

    Ok(tracks)
}

fn cached_or_read_track(cache: &mut LibraryCache, path: &Path) -> Result<(Track, bool)> {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let key = cache_key(&canonical);
    let metadata = fs::metadata(&canonical)
        .with_context(|| format!("Could not read metadata for {}", canonical.display()))?;

    let modified_secs = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or_default();

    let file_len = metadata.len();

    if let Some(cached) = cache.files.get(&key) {
        if cached.modified_secs == modified_secs && cached.file_len == file_len {
            return Ok((cached.track.clone(), false));
        }
    }

    let track = local::track_from_file(&canonical)?;

    cache.files.insert(
        key,
        CachedTrack {
            modified_secs,
            file_len,
            track: track.clone(),
        },
    );

    Ok((track, true))
}

fn load_cache(paths: &AppPaths) -> LibraryCache {
    let path = cache_path(paths);

    let Ok(txt) = fs::read_to_string(path) else {
        return LibraryCache::default();
    };

    let Ok(cache) = serde_json::from_str::<LibraryCache>(&txt) else {
        return LibraryCache::default();
    };

    if cache.version == CACHE_VERSION {
        cache
    } else {
        LibraryCache::default()
    }
}

fn save_cache(paths: &AppPaths, cache: &LibraryCache) -> Result<()> {
    paths.ensure()?;

    let path = cache_path(paths);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(cache)?)?;

    if path.exists() {
        let _ = fs::remove_file(&path);
    }

    fs::rename(&tmp, &path)
        .with_context(|| format!("Could not write library cache to {}", path.display()))
}

fn cache_path(paths: &AppPaths) -> PathBuf {
    paths.cache_dir.join(CACHE_FILE_NAME)
}

fn cache_key(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

fn is_audio_file(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    matches!(
        ext.as_str(),
        "mp3" | "flac" | "wav" | "m4a" | "ogg" | "aac" | "opus" | "wma"
    )
}
