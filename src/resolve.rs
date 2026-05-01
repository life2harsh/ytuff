use crate::core::track::{Acc, Track};
use crate::core::Core;
use crate::sources::{local, soundcloud::SoundCloudClient};
use anyhow::{anyhow, Result};
use std::path::PathBuf;

pub fn resolve_input(core: &Core, client: &mut SoundCloudClient, input: &str) -> Result<Track> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("Input cannot be empty"));
    }

    let path = PathBuf::from(trimmed);
    if path.is_file() {
        return ensure_local_file(core, &path);
    }

    if let Some(track) = client.resolve(trimmed)? {
        core.put_tracks(vec![track.clone()]);
        return Ok(track);
    }

    if let Some(track) = local_search(core, trimmed, 1).into_iter().next() {
        return Ok(track);
    }

    let results = client.search(trimmed, 10)?;
    if results.is_empty() {
        return Err(anyhow!("No playable tracks matched '{}'", trimmed));
    }
    core.put_tracks(results.clone());
    results
        .into_iter()
        .find(|track| track.acc != Some(Acc::Block))
        .ok_or_else(|| anyhow!("No playable tracks matched '{}'", trimmed))
}

pub fn local_search(core: &Core, query: &str, limit: usize) -> Vec<Track> {
    let query = query.trim().to_ascii_lowercase();
    if query.is_empty() {
        return Vec::new();
    }

    core.ids_local()
        .into_iter()
        .filter_map(|id| core.track(&id))
        .filter(|track| {
            track.title.to_ascii_lowercase().contains(&query)
                || track.who().to_ascii_lowercase().contains(&query)
        })
        .take(limit)
        .collect()
}

fn ensure_local_file(core: &Core, path: &PathBuf) -> Result<Track> {
    let id = format!("loc:{}", path.display());
    if let Some(track) = core.track(&id) {
        return Ok(track);
    }
    let track = local::track_from_file(path)?;
    core.put_tracks(vec![track.clone()]);
    Ok(track)
}
