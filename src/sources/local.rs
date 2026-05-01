use crate::core::track::Track;
use lofty::{Accessor, AudioFile, TaggedFileExt};
use std::path::Path;

pub fn scan_dir(path: &Path) -> anyhow::Result<Vec<Track>> {
    let mut trs = Vec::new();
    let walk = walkdir::WalkDir::new(path).follow_links(true).into_iter();
    for ent in walk.filter_map(|e| e.ok()) {
        let p = ent.path();
        if !p.is_file() {
            continue;
        }
        let ext = p
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !matches!(
            ext.as_str(),
            "mp3" | "flac" | "wav" | "m4a" | "ogg" | "aac" | "opus" | "wma"
        ) {
            continue;
        }

        trs.push(track_from_file(p)?);
    }
    Ok(trs)
}

pub fn track_from_file(path: &Path) -> anyhow::Result<Track> {
    let mut title = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();
    let mut artist = None;
    let mut dur = None;

    if let Ok(tag) = lofty::read_from_path(path) {
        if let Some(t) = tag.primary_tag() {
            if let Some(v) = t.title() {
                title = v.to_string();
            }
            if let Some(v) = t.artist() {
                artist = Some(v.to_string());
            }
        }
        let secs = tag.properties().duration().as_secs();
        if secs > 0 {
            dur = Some(secs);
        }
    }

    let id = format!("loc:{}", path.display());
    Ok(Track::new_local(id, path.to_path_buf(), title, artist, dur))
}
