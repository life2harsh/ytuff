use crate::core::track::Track;
use lofty::{Accessor, AudioFile, TaggedFileExt};
use std::path::Path;

pub async fn scan_dir(path: &Path) -> anyhow::Result<Vec<Track>> {
    let mut tracks = Vec::new();
    let walker = walkdir::WalkDir::new(path).follow_links(true).into_iter();
    for entry in walker.filter_map(|e| e.ok()) {
        let p = entry.path();
        if p.is_file() {
            if let Some(ext) = p.extension().and_then(|s| s.to_str()) {
                let ext = ext.to_lowercase();
                if matches!(ext.as_str(), "mp3" | "flac" | "wav" | "m4a" | "ogg" | "aac" | "opus" | "wma") {
                    let mut title = p.file_stem().and_then(|s| s.to_str()).unwrap_or("unknown").to_string();
                    let mut artist = None;
                    let mut duration = None;
                    if let Ok(tagged) = lofty::read_from_path(p) {
                        if let Some(tag) = tagged.primary_tag() {
                            if let Some(t) = tag.title() {
                                title = t.to_string();
                            }
                            if let Some(a) = tag.artist() {
                                artist = Some(a.to_string());
                            }
                        }
                        let props = tagged.properties();
                        let secs = props.duration().as_secs();
                        if secs > 0 {
                            duration = Some(secs);
                        }
                    }

                    let id = format!("{}", p.display());
                    tracks.push(Track::new_local(id, p.to_path_buf(), title, artist, duration));
                }
            }
        }
    }
    Ok(tracks)
}
