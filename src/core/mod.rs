use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub mod track;
use track::{Src, Track};

#[derive(Clone, Default)]
pub struct Core {
    pub tracks: Arc<Mutex<HashMap<String, Track>>>,
    pub track_order: Arc<Mutex<Vec<String>>>,
    pub q: Arc<Mutex<Vec<String>>>,
    pub cur: Arc<Mutex<Option<String>>>,
    pub hist: Arc<Mutex<Vec<String>>>,
    pub sc_on: Arc<Mutex<bool>>,
    pub scan_paths: Arc<Mutex<Vec<PathBuf>>>,
}

impl Core {
    pub fn new() -> Self {
        Self {
            tracks: Arc::new(Mutex::new(HashMap::new())),
            track_order: Arc::new(Mutex::new(Vec::new())),
            q: Arc::new(Mutex::new(Vec::new())),
            cur: Arc::new(Mutex::new(None)),
            hist: Arc::new(Mutex::new(Vec::new())),
            sc_on: Arc::new(Mutex::new(false)),
            scan_paths: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn set_sc(&self, on: bool) {
        *self.sc_on.lock().unwrap() = on;
    }

    pub fn sc_on(&self) -> bool {
        *self.sc_on.lock().unwrap()
    }

    pub fn add_scan_path(&self, path: &str) -> anyhow::Result<usize> {
        let p = PathBuf::from(path);
        if !p.exists() || !p.is_dir() {
            return Err(anyhow::anyhow!("Path does not exist or is not a directory"));
        }

        let ps = self.scan_paths.lock().unwrap();
        if ps.contains(&p) {
            return Ok(0);
        }
        let trs = crate::sources::local::scan_dir(&p)?;
        let n = trs.len();
        drop(ps);
        self.put_tracks(trs);
        self.scan_paths.lock().unwrap().push(p);
        Ok(n)
    }

    pub fn remove_scan_path(&self, idx: usize) -> anyhow::Result<()> {
        let mut ps = self.scan_paths.lock().unwrap();
        if idx >= ps.len() {
            return Err(anyhow::anyhow!("Invalid index"));
        }
        let rem = ps.remove(idx);
        drop(ps);
        let mut tracks = self.tracks.lock().unwrap();
        tracks.retain(|_, track| match (&track.src, &track.path) {
            (Src::Local, Some(path)) => !path.starts_with(&rem),
            _ => true,
        });
        let keep = tracks.keys().cloned().collect::<HashSet<_>>();
        drop(tracks);
        self.track_order
            .lock()
            .unwrap()
            .retain(|id| keep.contains(id));
        self.purge_dead();
        Ok(())
    }

    pub fn put_tracks(&self, list: Vec<Track>) {
        let mut tracks = self.tracks.lock().unwrap();
        let mut order = self.track_order.lock().unwrap();
        for track in list {
            if !tracks.contains_key(&track.id) {
                order.push(track.id.clone());
            }
            tracks.insert(track.id.clone(), track);
        }
    }

    pub fn ids_local(&self) -> Vec<String> {
        let tracks = self.tracks.lock().unwrap();
        let order = self.track_order.lock().unwrap();
        order
            .iter()
            .filter_map(|id| {
                tracks
                    .get(id)
                    .filter(|track| track.src == Src::Local)
                    .map(|track| track.id.clone())
            })
            .collect()
    }

    pub fn track(&self, id: &str) -> Option<Track> {
        self.tracks.lock().unwrap().get(id).cloned()
    }

    pub fn track_at(&self, idx: usize) -> Option<Track> {
        let id = self.track_order.lock().unwrap().get(idx).cloned()?;
        self.track(&id)
    }

    pub fn tracks_of(&self, ids: &[String]) -> Vec<Track> {
        let tracks = self.tracks.lock().unwrap();
        ids.iter()
            .filter_map(|id| tracks.get(id).cloned())
            .collect()
    }

    pub fn enqueue(&self, id: String) {
        self.q.lock().unwrap().push(id);
    }

    pub fn dequeue(&self) -> Option<String> {
        let mut q = self.q.lock().unwrap();
        if q.is_empty() {
            None
        } else {
            Some(q.remove(0))
        }
    }

    pub fn q_ids(&self) -> Vec<String> {
        self.q.lock().unwrap().clone()
    }

    pub fn clear_queue(&self) {
        self.q.lock().unwrap().clear();
    }

    pub fn set_queue(&self, ids: Vec<String>) {
        *self.q.lock().unwrap() = ids;
    }

    pub fn add_hist(&self, id: String) {
        let mut hs = self.hist.lock().unwrap();
        if let Some(i) = hs.iter().position(|x| x == &id) {
            hs.remove(i);
        }
        hs.insert(0, id);
        if hs.len() > 20 {
            hs.truncate(20);
        }
    }

    pub fn hist_ids(&self) -> Vec<String> {
        self.hist.lock().unwrap().clone()
    }

    pub fn prev_hist(&self, cur: Option<&str>) -> Option<String> {
        let hs = self.hist.lock().unwrap();
        if hs.len() < 2 {
            return None;
        }
        if let Some(cur) = cur {
            if hs.first().map(|x| x.as_str()) == Some(cur) {
                return hs.get(1).cloned();
            }
            if let Some(i) = hs.iter().position(|x| x == cur) {
                return hs.get(i + 1).cloned();
            }
        }
        hs.get(1).cloned()
    }

    pub fn set_cur(&self, id: Option<String>) {
        *self.cur.lock().unwrap() = id;
    }

    pub fn cur_id(&self) -> Option<String> {
        self.cur.lock().unwrap().clone()
    }

    pub fn purge_dead(&self) {
        let keep = self
            .tracks
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect::<HashSet<_>>();
        self.track_order
            .lock()
            .unwrap()
            .retain(|id| keep.contains(id));
        self.q.lock().unwrap().retain(|id| keep.contains(id));
        self.hist.lock().unwrap().retain(|id| keep.contains(id));
        let mut cur = self.cur.lock().unwrap();
        if cur.as_ref().is_some_and(|id| !keep.contains(id)) {
            *cur = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hist_moves_top() {
        let c = Core::new();
        c.add_hist("a".into());
        c.add_hist("b".into());
        c.add_hist("a".into());
        assert_eq!(c.hist_ids(), vec!["a".to_string(), "b".to_string()]);
    }
}
