//! A plugin's `storage`: a small key/value store and its settings, in the
//! plugin's data directory (`<config>/plugin-data/<id>/` in the app).
//! `store.json` is written whole on each change, through a temporary file;
//! `settings.json` is the user's to edit and only ever read.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// Most bytes of keys and values one plugin may keep.
pub const QUOTA: usize = 4 << 20;

pub struct Storage {
    dir: Option<PathBuf>,
    /// Loaded on first use.
    map: Option<BTreeMap<String, String>>,
}

impl Storage {
    /// `dir` None: an in-memory store that's gone when the plugin unloads.
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self { dir, map: None }
    }

    fn map(&mut self) -> &mut BTreeMap<String, String> {
        let dir = &self.dir;
        self.map.get_or_insert_with(|| {
            dir.as_ref()
                .and_then(|d| std::fs::read(d.join("store.json")).ok())
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default()
        })
    }

    pub fn get(&mut self, key: &str) -> Option<String> {
        self.map().get(key).cloned()
    }

    pub fn keys(&mut self) -> Vec<String> {
        self.map().keys().cloned().collect()
    }

    pub fn set(&mut self, key: String, value: String) -> Result<(), String> {
        let map = self.map();
        let size: usize = map
            .iter()
            .filter(|(k, _)| **k != key)
            .map(|(k, v)| k.len() + v.len())
            .sum::<usize>()
            + key.len()
            + value.len();
        if size > QUOTA {
            return Err(format!(
                "storage is limited to {} MiB per plugin",
                QUOTA >> 20
            ));
        }
        map.insert(key, value);
        self.save()
    }

    pub fn delete(&mut self, key: &str) -> Result<(), String> {
        if self.map().remove(key).is_some() {
            self.save()
        } else {
            Ok(())
        }
    }

    /// The user's settings for the plugin, as JSON; "{}" without any.
    pub fn settings(&self) -> String {
        self.dir
            .as_ref()
            .and_then(|d| std::fs::read_to_string(d.join("settings.json")).ok())
            .filter(|s| serde_json::from_str::<serde_json::Value>(s).is_ok())
            .unwrap_or_else(|| "{}".into())
    }

    fn save(&mut self) -> Result<(), String> {
        let Some(dir) = self.dir.clone() else {
            return Ok(());
        };
        let json = serde_json::to_vec(self.map()).map_err(|e| e.to_string())?;
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(&dir)?;
            let tmp = dir.join(".store.json.tmp");
            std::fs::write(&tmp, json)?;
            std::fs::rename(&tmp, dir.join("store.json"))
        };
        write().map_err(|e| format!("saving storage: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_and_enforces_the_quota() {
        let dir = std::env::temp_dir().join(format!("sheaf-storage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Storage::new(Some(dir.clone()));
        assert_eq!(s.get("a"), None);
        s.set("a".into(), "1".into()).unwrap();
        s.set("b".into(), "2".into()).unwrap();
        s.delete("b").unwrap();
        let mut again = Storage::new(Some(dir.clone()));
        assert_eq!(again.get("a").as_deref(), Some("1"));
        assert_eq!(again.keys(), ["a"]);
        assert!(again.set("big".into(), "x".repeat(QUOTA)).is_err());
        // Replacing a value counts the new size, not both.
        again.set("a".into(), "x".repeat(QUOTA - 1)).unwrap();

        assert_eq!(again.settings(), "{}");
        std::fs::write(dir.join("settings.json"), r#"{"min": 5}"#).unwrap();
        assert_eq!(again.settings(), r#"{"min": 5}"#);
        std::fs::write(dir.join("settings.json"), "not json").unwrap();
        assert_eq!(again.settings(), "{}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
