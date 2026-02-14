use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub mod track;
use track::Track;

#[derive(Clone, Default)]
pub struct Core {
    pub tracks: Arc<Mutex<Vec<Track>>>,
    pub queue: Arc<Mutex<Vec<usize>>>,
    pub current: Arc<Mutex<Option<usize>>>,
    pub recently_played: Arc<Mutex<Vec<usize>>>,
    pub soundcloud_enabled: Arc<Mutex<bool>>,
}

impl Core {
    pub fn new() -> Self {
        Core {
            tracks: Arc::new(Mutex::new(Vec::new())),
            queue: Arc::new(Mutex::new(Vec::new())),
            current: Arc::new(Mutex::new(None)),
            recently_played: Arc::new(Mutex::new(Vec::new())),
            soundcloud_enabled: Arc::new(Mutex::new(false)),
        }
    }

    pub async fn scan_path(&mut self, path: &str) -> anyhow::Result<()> {
        let p = PathBuf::from(path);
        let found = crate::sources::local::scan_dir(&p).await?;
        let mut tracks = self.tracks.lock().unwrap();
        *tracks = found;
        Ok(())
    }

    pub fn enqueue(&self, idx: usize) {
        let mut q = self.queue.lock().unwrap();
        q.push(idx);
    }

    pub fn dequeue(&self) -> Option<usize> {
        let mut q = self.queue.lock().unwrap();
        if !q.is_empty() {
            Some(q.remove(0))
        } else {
            None
        }
    }

    pub fn add_to_history(&self, idx: usize) {
        let mut history = self.recently_played.lock().unwrap();
        if !history.contains(&idx) {
            history.insert(0, idx);
        } else {
            if let Some(pos) = history.iter().position(|&x| x == idx) {
                history.remove(pos);
                history.insert(0, idx);
            }
        }
        if history.len() > 10 {
            history.truncate(10);
        }
    }
}
