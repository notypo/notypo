//! On-disk cache of native completion answers.
//!
//! A cache file belongs to one backend fingerprint: the completer and app
//! files (path, size, modification time), the directories where extensions
//! and components are installed, and the locale. Upgrading the app or adding
//! an extension changes the fingerprint and starts a new file.
//!
//! A cached list is only trusted to say a word *is* valid. Before a word is
//! called invalid, or a level is taken to have no subcommands, the app is
//! asked again, so a stale entry can't hide a newly added command. Entries
//! expire after a week, files are bounded, and an unreadable file is
//! treated as empty. Only command and option lists are stored: never
//! command lines, output, history, or environment values.

use super::native::CompletionItem;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{env, fs};

const MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const MAX_ENTRIES: usize = 2000;
// Start a fresh namespace after restricting persistence to command paths,
// so old entries carrying argument values cannot be loaded and saved again.
const CACHE_FORMAT_VERSION: u8 = 2;

/// A stable digest of everything that can change an app's answers.
pub fn fingerprint(files: &[PathBuf], extra: &[&str]) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    env!("CARGO_PKG_VERSION").hash(&mut hasher);
    CACHE_FORMAT_VERSION.hash(&mut hasher);
    for file in files {
        file.hash(&mut hasher);
        if let Ok(meta) = fs::metadata(file) {
            meta.len().hash(&mut hasher);
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos());
            modified.hash(&mut hasher);
        }
    }
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        env::var(var).unwrap_or_default().hash(&mut hasher);
    }
    extra.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

pub struct CompletionCache {
    file: PathBuf,
    entries: HashMap<String, (u64, Vec<CompletionItem>)>,
    dirty: bool,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl CompletionCache {
    /// The cache for one backend; `None` when caching is turned off.
    pub fn open(tag: &str, fingerprint: &str) -> Option<CompletionCache> {
        if env::var_os("NOTYPO_NO_CACHE").is_some() {
            return None;
        }
        let file = crate::utils::cache_dir()
            .join("completion")
            .join(format!("{tag}-{fingerprint}.json"));
        Some(CompletionCache {
            entries: load(&file),
            file,
            dirty: false,
        })
    }

    pub fn get(&self, key: &str) -> Option<&[CompletionItem]> {
        let (saved, items) = self.entries.get(key)?;
        (now().saturating_sub(*saved) < MAX_AGE.as_secs()).then_some(items.as_slice())
    }

    pub fn put(&mut self, key: String, items: Vec<CompletionItem>) {
        self.entries.insert(key, (now(), items));
        self.dirty = true;
    }

    /// Writes changes atomically, keeping the newest entries.
    pub fn save(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let mut entries: Vec<_> = self.entries.iter().collect();
        entries.sort_by_key(|(_, (saved, _))| std::cmp::Reverse(*saved));
        entries.truncate(MAX_ENTRIES);
        let body: serde_json::Map<String, Value> = entries
            .into_iter()
            .map(|(key, (saved, items))| {
                let items: Vec<Value> = items
                    .iter()
                    .map(|i| json!([i.value, i.takes_value, i.description]))
                    .collect();
                (key.clone(), json!([saved, items]))
            })
            .collect();
        let Some(dir) = self.file.parent() else {
            return;
        };
        let _ = fs::create_dir_all(dir);
        let tmp = self
            .file
            .with_extension(format!("tmp{}", std::process::id()));
        if fs::write(&tmp, Value::Object(body).to_string()).is_ok() {
            let _ = fs::rename(&tmp, &self.file);
        }
    }
}

fn load(file: &Path) -> HashMap<String, (u64, Vec<CompletionItem>)> {
    let Ok(text) = fs::read_to_string(file) else {
        return HashMap::new();
    };
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&text) else {
        return HashMap::new();
    };
    map.into_iter()
        .filter_map(|(key, entry)| {
            let saved = entry.get(0)?.as_u64()?;
            let items = entry
                .get(1)?
                .as_array()?
                .iter()
                .map(|item| {
                    Some(CompletionItem {
                        value: item.get(0)?.as_str()?.to_owned(),
                        takes_value: item.get(1)?.as_bool(),
                        description: item
                            .get(2)
                            .and_then(Value::as_str)
                            .and_then(super::native::clean_description),
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            Some((key, (saved, items)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(value: &str) -> CompletionItem {
        CompletionItem {
            value: value.into(),
            takes_value: Some(false),
            description: None,
        }
    }

    #[test]
    fn round_trips_expires_and_survives_corruption() {
        let tag = format!("test-{}", std::process::id());
        let mut cache = CompletionCache::open(&tag, "round").unwrap();
        let described = CompletionItem {
            description: Some("Describes instances".into()),
            ..item("describe-instances")
        };
        cache.put("ec2|".into(), vec![described.clone()]);
        cache.entries.insert("old".into(), (0, vec![item("gone")]));
        cache.dirty = true;
        cache.save();
        let cache = CompletionCache::open(&tag, "round").unwrap();
        assert_eq!(cache.get("ec2|").unwrap(), [described]);
        assert!(cache.get("old").is_none(), "expired entries are ignored");
        fs::write(&cache.file, "{not json").unwrap();
        let reopened = CompletionCache::open(&tag, "round").unwrap();
        assert!(reopened.get("ec2|").is_none());
        let _ = fs::remove_file(&cache.file);
    }

    #[test]
    fn fingerprints_follow_file_changes() {
        let dir = env::temp_dir().join(format!("notypo-fp-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("completer");
        fs::write(&file, "v1").unwrap();
        let before = fingerprint(std::slice::from_ref(&file), &["x"]);
        assert_eq!(before, fingerprint(std::slice::from_ref(&file), &["x"]));
        fs::write(&file, "version 2").unwrap();
        assert_ne!(before, fingerprint(std::slice::from_ref(&file), &["x"]));
        assert_ne!(
            fingerprint(std::slice::from_ref(&file), &["x"]),
            fingerprint(std::slice::from_ref(&file), &["y"])
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
