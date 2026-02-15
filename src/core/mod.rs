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
    pub scan_paths: Arc<Mutex<Vec<PathBuf>>>,
}

impl Core {
    pub fn new() -> Self {
        Core {
            tracks: Arc::new(Mutex::new(Vec::new())),
            queue: Arc::new(Mutex::new(Vec::new())),
            current: Arc::new(Mutex::new(None)),
            recently_played: Arc::new(Mutex::new(Vec::new())),
            soundcloud_enabled: Arc::new(Mutex::new(false)),
            scan_paths: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub async fn add_scan_path(&mut self, path: &str) -> anyhow::Result<usize> {
        let p = PathBuf::from(path);
        if !p.exists() || !p.is_dir() {
            return Err(anyhow::anyhow!("Path does not exist or is not a directory"));
        }
        
        let mut paths = self.scan_paths.lock().unwrap();
        if paths.contains(&p) {
            return Err(anyhow::anyhow!("Path already added"));
        }
        
        let found = crate::sources::local::scan_dir(&p).await?;
        let count = found.len();
        
        let mut tracks = self.tracks.lock().unwrap();
        tracks.extend(found);
        paths.push(p);
        
        Ok(count)
    }
    
    pub fn remove_scan_path(&mut self, index: usize) -> anyhow::Result<()> {
        let mut paths = self.scan_paths.lock().unwrap();
        if index >= paths.len() {
            return Err(anyhow::anyhow!("Invalid index"));
        }
        
        let removed_path = paths.remove(index);
        drop(paths);
        
        let mut tracks = self.tracks.lock().unwrap();
        tracks.retain(|t| {
            if let Some(ref path) = t.path {
                !path.starts_with(&removed_path)
            } else {
                true
            }
        });
        
        Ok(())
    }
    
    pub async fn rescan_all(&mut self) -> anyhow::Result<usize> {
        let paths: Vec<PathBuf> = self.scan_paths.lock().unwrap().clone();
        let mut total = 0;
        
        let mut tracks = self.tracks.lock().unwrap();
        tracks.clear();
        
        for path in paths {
            let found = crate::sources::local::scan_dir(&path).await?;
            total += found.len();
            tracks.extend(found);
        }
        
        Ok(total)
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
