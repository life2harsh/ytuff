use crate::appdata::AppPaths;
use crate::core::track::Track;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct PlaylistStore {
    playlists: BTreeMap<String, Playlist>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Playlist {
    pub name: String,
    pub tracks: Vec<Track>,
    pub remote_url: Option<String>,
}

impl PlaylistStore {
    pub fn load(paths: &AppPaths) -> Result<Self> {
        paths.ensure()?;
        if !paths.playlists_file.exists() {
            let store = Self::default();
            store.save(paths)?;
            return Ok(store);
        }
        let txt = fs::read_to_string(&paths.playlists_file)
            .with_context(|| format!("Could not read {}", paths.playlists_file.display()))?;
        let store = serde_json::from_str(&txt)
            .with_context(|| format!("Could not parse {}", paths.playlists_file.display()))?;
        Ok(store)
    }

    pub fn save(&self, paths: &AppPaths) -> Result<()> {
        paths.ensure()?;
        fs::write(&paths.playlists_file, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("Could not write {}", paths.playlists_file.display()))
    }

    pub fn names(&self) -> Vec<String> {
        self.playlists
            .values()
            .map(|playlist| playlist.name.clone())
            .collect()
    }

    pub fn playlist(&self, name: &str) -> Option<&Playlist> {
        self.playlists.get(&playlist_key(name))
    }

    pub fn create(&mut self, name: &str) -> Result<()> {
        let name = clean_name(name)?;
        let key = playlist_key(&name);
        if self.playlists.contains_key(&key) {
            return Err(anyhow!("Playlist '{}' already exists", name));
        }
        self.playlists.insert(
            key,
            Playlist {
                name,
                tracks: Vec::new(),
                remote_url: None,
            },
        );
        Ok(())
    }

    pub fn add_track(&mut self, name: &str, track: Track) -> Result<usize> {
        let name = clean_name(name)?;
        let key = playlist_key(&name);
        let playlist = self
            .playlists
            .get_mut(&key)
            .ok_or_else(|| anyhow!("Playlist '{}' does not exist", name))?;
        if playlist.tracks.iter().any(|item| item.id == track.id) {
            return Ok(playlist.tracks.len());
        }
        playlist.tracks.push(track);
        Ok(playlist.tracks.len())
    }

    pub fn import_remote(
        &mut self,
        name: &str,
        tracks: Vec<Track>,
        remote_url: String,
    ) -> Result<usize> {
        let name = clean_name(name)?;
        let count = tracks.len();
        let key = playlist_key(&name);
        if self.playlists.contains_key(&key) {
            return Err(anyhow!(
                "Playlist '{}' already exists; use playlist sync to refresh it",
                name
            ));
        }
        self.playlists.insert(
            key,
            Playlist {
                name,
                tracks,
                remote_url: Some(remote_url),
            },
        );
        Ok(count)
    }

    pub fn sync_remote(
        &mut self,
        name: &str,
        tracks: Vec<Track>,
        remote_url: String,
    ) -> Result<usize> {
        let name = clean_name(name)?;
        let key = playlist_key(&name);
        let playlist = self
            .playlists
            .get_mut(&key)
            .ok_or_else(|| anyhow!("Playlist '{}' does not exist", name))?;
        playlist.tracks = tracks;
        playlist.remote_url = Some(remote_url);
        Ok(playlist.tracks.len())
    }
}

fn clean_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("Playlist name cannot be empty"));
    }
    Ok(trimmed.to_string())
}

fn playlist_key(name: &str) -> String {
    name.trim().to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::track::Track;
    use std::path::PathBuf;

    #[test]
    fn avoids_duplicate_tracks() {
        let mut store = PlaylistStore::default();
        store.create("mix").unwrap();
        let track = Track::new_local(
            "loc:test".into(),
            PathBuf::from("test.mp3"),
            "Test".into(),
            Some("Artist".into()),
            None,
        );
        assert_eq!(store.add_track("mix", track.clone()).unwrap(), 1);
        assert_eq!(store.add_track("mix", track).unwrap(), 1);
    }
}
