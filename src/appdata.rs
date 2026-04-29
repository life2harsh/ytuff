use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub config_file: PathBuf,
    pub playlists_file: PathBuf,
    pub lyrics_dir: PathBuf,
    pub downloads_dir: PathBuf,
}

impl AppPaths {
    pub fn discover() -> Self {
        let mut config_dir = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        config_dir.push("rustplayer");

        let mut data_dir = dirs::data_local_dir().unwrap_or_else(|| config_dir.clone());
        data_dir.push("rustplayer");

        let mut cache_dir = dirs::cache_dir().unwrap_or_else(|| data_dir.clone());
        cache_dir.push("rustplayer");

        let mut config_file = config_dir.clone();
        config_file.push("config.json");

        let mut playlists_file = data_dir.clone();
        playlists_file.push("playlists.json");

        let mut lyrics_dir = cache_dir.clone();
        lyrics_dir.push("lyrics");

        let mut downloads_dir = data_dir.clone();
        downloads_dir.push("downloads");

        Self {
            config_dir,
            data_dir,
            cache_dir,
            config_file,
            playlists_file,
            lyrics_dir,
            downloads_dir,
        }
    }

    pub fn ensure(&self) -> Result<()> {
        for dir in [
            &self.config_dir,
            &self.data_dir,
            &self.cache_dir,
            &self.lyrics_dir,
            &self.downloads_dir,
        ] {
            fs::create_dir_all(dir)
                .with_context(|| format!("Could not create {}", dir.display()))?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppConfig {
    pub quality: String,
    pub scan_paths: Vec<PathBuf>,
    pub autoplay: bool,
    pub lyrics_enabled: bool,
    pub auto_fetch_lyrics: bool,
    pub daemon_addr: String,
    pub downloads_dir: Option<PathBuf>,
    pub youtube_cookie_header: Option<String>,
    pub youtube_cookie_file: Option<PathBuf>,
    pub start_background_on_boot: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            quality: "high".to_string(),
            scan_paths: Vec::new(),
            autoplay: false,
            lyrics_enabled: true,
            auto_fetch_lyrics: true,
            daemon_addr: "127.0.0.1:38185".to_string(),
            downloads_dir: None,
            youtube_cookie_header: None,
            youtube_cookie_file: None,
            start_background_on_boot: false,
        }
    }
}

impl AppConfig {
    pub fn load(paths: &AppPaths) -> Result<Self> {
        paths.ensure()?;
        if !paths.config_file.exists() {
            let cfg = Self::default();
            cfg.save(paths)?;
            return Ok(cfg);
        }
        let txt = fs::read_to_string(&paths.config_file)
            .with_context(|| format!("Could not read {}", paths.config_file.display()))?;
        let cfg = serde_json::from_str(&txt)
            .with_context(|| format!("Could not parse {}", paths.config_file.display()))?;
        Ok(cfg)
    }

    pub fn save(&self, paths: &AppPaths) -> Result<()> {
        paths.ensure()?;
        fs::write(&paths.config_file, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("Could not write {}", paths.config_file.display()))
    }

    pub fn effective_downloads_dir(&self, paths: &AppPaths) -> PathBuf {
        self.downloads_dir
            .clone()
            .unwrap_or_else(|| paths.downloads_dir.clone())
    }

    pub fn cookie_header(&self) -> Result<Option<String>> {
        if let Some(header) = self
            .youtube_cookie_header
            .as_ref()
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
        {
            return Ok(Some(header.to_string()));
        }

        let Some(path) = self.youtube_cookie_file.as_ref() else {
            return Ok(None);
        };
        let txt = fs::read_to_string(path)
            .with_context(|| format!("Could not read cookie source {}", path.display()))?;
        Ok(parse_cookie_source(&txt))
    }
}

fn parse_cookie_source(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.contains('\t') {
        let pairs = trimmed
            .lines()
            .filter(|line| {
                let line = line.trim();
                !line.is_empty() && !line.starts_with('#')
            })
            .filter_map(|line| {
                let cols = line.split('\t').collect::<Vec<_>>();
                if cols.len() >= 7 {
                    Some(format!("{}={}", cols[5], cols[6]))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        if !pairs.is_empty() {
            return Some(pairs.join("; "));
        }
    }

    trimmed.contains('=').then(|| trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_netscape_cookie_file() {
        let raw = "# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t0\tVISITOR_INFO1_LIVE\tabc\n.youtube.com\tTRUE\t/\tTRUE\t0\tSID\tdef\n";
        assert_eq!(
            parse_cookie_source(raw).as_deref(),
            Some("VISITOR_INFO1_LIVE=abc; SID=def")
        );
    }
}
