//! Bookmarks, stored as a JSON file in the data directory.
//!
//! Every bookmark carries a stable id and added/updated timestamps so the
//! file can later be merged with a copy on SquatchVault without conflicts.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Bookmark {
    pub id: String,
    pub url: String,
    pub title: String,
    #[serde(default)]
    pub folder: String,
    pub added: u64,
    pub updated: u64,
}

pub struct Bookmarks {
    path: PathBuf,
    items: Vec<Bookmark>,
    counter: u32,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Comparison key: ignore the fragment and a trailing slash.
fn key(url: &str) -> String {
    let u = url.split('#').next().unwrap_or(url);
    u.strip_suffix('/').unwrap_or(u).to_string()
}

impl Bookmarks {
    pub fn load(path: PathBuf) -> Self {
        let items = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        Bookmarks { path, items, counter: 0 }
    }

    fn save(&self) -> Result<()> {
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&self.items)?)?;
        std::fs::rename(&tmp, &self.path)?; // atomic on the same filesystem
        Ok(())
    }

    fn new_id(&mut self) -> String {
        self.counter = self.counter.wrapping_add(1);
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        format!("{:x}{:04x}", nanos, self.counter)
    }

    pub fn list(&self) -> &[Bookmark] {
        &self.items
    }

    pub fn contains(&self, url: &str) -> bool {
        let k = key(url);
        self.items.iter().any(|b| key(&b.url) == k)
    }

    /// Adds the page if it isn't bookmarked, removes it if it is. Returns the new state.
    pub fn toggle(&mut self, url: &str, title: &str) -> Result<bool> {
        let k = key(url);
        if let Some(pos) = self.items.iter().position(|b| key(&b.url) == k) {
            self.items.remove(pos);
            self.save()?;
            return Ok(false);
        }
        self.add(url, title, "")?;
        Ok(true)
    }

    pub fn add(&mut self, url: &str, title: &str, folder: &str) -> Result<()> {
        let t = now();
        let id = self.new_id();
        self.items.push(Bookmark {
            id,
            url: url.to_string(),
            title: if title.trim().is_empty() { url.to_string() } else { title.trim().to_string() },
            folder: folder.trim().to_string(),
            added: t,
            updated: t,
        });
        self.save()
    }

    pub fn remove(&mut self, id: &str) -> Result<()> {
        self.items.retain(|b| b.id != id);
        self.save()
    }

    pub fn update(&mut self, id: &str, title: &str, folder: &str) -> Result<()> {
        if let Some(b) = self.items.iter_mut().find(|b| b.id == id) {
            if !title.trim().is_empty() {
                b.title = title.trim().to_string();
            }
            b.folder = folder.trim().to_string();
            b.updated = now();
        }
        self.save()
    }

    /// Import (url, title, folder) triples, skipping URLs already present. Returns how many were added.
    pub fn import(&mut self, entries: Vec<(String, String, String)>) -> Result<usize> {
        let mut added = 0;
        for (url, title, folder) in entries {
            if !(url.starts_with("http://") || url.starts_with("https://")) || self.contains(&url) {
                continue;
            }
            let t = now();
            let id = self.new_id();
            self.items.push(Bookmark { id, url, title, folder, added: t, updated: t });
            added += 1;
        }
        if added > 0 {
            self.save()?;
        }
        Ok(added)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_and_import() {
        let dir = std::env::temp_dir().join(format!("sb-bm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut b = Bookmarks::load(dir.join("bookmarks.json"));
        assert!(b.toggle("https://a.example/story/", "Story").unwrap());
        assert!(b.contains("https://a.example/story#ch2"));
        let n = b
            .import(vec![
                ("https://a.example/story".into(), "dup".into(), "".into()),
                ("https://b.example/".into(), "B".into(), "Reading".into()),
                ("javascript:alert(1)".into(), "bad".into(), "".into()),
            ])
            .unwrap();
        assert_eq!(n, 1);
        let reloaded = Bookmarks::load(dir.join("bookmarks.json"));
        assert_eq!(reloaded.list().len(), 2);
        assert!(!b.toggle("https://a.example/story", "").unwrap());
        assert_eq!(b.list().len(), 1);
        std::fs::remove_dir_all(dir).ok();
    }
}
