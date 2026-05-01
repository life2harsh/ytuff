use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Src {
    Local,
    Sc,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Acc {
    Play,
    Prev,
    Block,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub src: Src,
    pub title: String,
    pub artist: Option<String>,
    pub user: Option<String>,
    pub dur: Option<u64>,
    pub path: Option<PathBuf>,
    pub link: Option<String>,
    pub art: Option<String>,
    pub strm: Option<String>,
    pub acc: Option<Acc>,
}

impl Track {
    pub fn new_local(
        id: String,
        path: PathBuf,
        title: String,
        artist: Option<String>,
        dur: Option<u64>,
    ) -> Self {
        Self {
            id,
            src: Src::Local,
            title,
            artist,
            user: None,
            dur,
            path: Some(path),
            link: None,
            art: None,
            strm: None,
            acc: None,
        }
    }

    pub fn new_sc(
        id: String,
        title: String,
        artist: Option<String>,
        user: Option<String>,
        dur: Option<u64>,
        link: Option<String>,
        art: Option<String>,
        strm: Option<String>,
        acc: Option<Acc>,
    ) -> Self {
        Self {
            id,
            src: Src::Sc,
            title,
            artist,
            user,
            dur,
            path: None,
            link,
            art,
            strm,
            acc,
        }
    }

    pub fn who(&self) -> String {
        self.artist
            .clone()
            .or_else(|| self.user.clone())
            .unwrap_or_else(|| "Unknown".to_string())
    }

    pub fn tag(&self) -> &'static str {
        match self.src {
            Src::Local => "L",
            Src::Sc if self.is_artist_browse() => "A",
            Src::Sc if self.is_album_browse() => "B",
            Src::Sc if self.is_remote_browse() => "P",
            Src::Sc => "Y",
        }
    }

    pub fn acc_tag(&self) -> &'static str {
        match self.acc {
            Some(Acc::Play) => "play",
            Some(Acc::Prev) => "prev",
            Some(Acc::Block) => "lock",
            None => "",
        }
    }

    pub fn is_sc(&self) -> bool {
        self.src == Src::Sc
    }

    pub fn is_remote_browse(&self) -> bool {
        self.src == Src::Sc && self.id.starts_with("ytb:")
    }

    pub fn is_playable_remote(&self) -> bool {
        self.is_sc() && !self.is_remote_browse()
    }

    pub fn browse_id(&self) -> Option<&str> {
        self.id.strip_prefix("ytb:")
    }

    pub fn remote_video_id(&self) -> Option<&str> {
        self.id
            .strip_prefix("yt:")
            .or_else(|| self.id.strip_prefix("sc:"))
    }

    pub fn is_artist_browse(&self) -> bool {
        self.browse_id()
            .is_some_and(|id| id.starts_with("UC") || id.starts_with("MPLA"))
    }

    pub fn is_album_browse(&self) -> bool {
        self.browse_id().is_some_and(|id| id.starts_with("MPRE"))
    }
}
