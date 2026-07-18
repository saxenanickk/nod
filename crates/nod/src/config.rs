//! App configuration, persisted at
//! `~/Library/Application Support/nod/config.json`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// GitHub login to act as (gh multi-account). `None` = gh's active account.
    pub gh_user: Option<String>,
    /// Extra search scope for the PR list, e.g. `org:foo` or `repo:foo/bar`.
    /// Empty searches all of GitHub.
    pub scope: String,
    /// `owner/repo` → local clone path. Used for checkout from M3.
    pub clones: HashMap<String, PathBuf>,
    /// Directories scanned (3 levels deep) to auto-discover clones.
    pub clone_roots: Vec<PathBuf>,
    /// Poll interval for the PR list, in seconds.
    pub poll_seconds: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            // `None` → whichever account is active in `gh` (gh auth status).
            gh_user: None,
            scope: String::new(),
            clones: HashMap::new(),
            clone_roots: std::env::home_dir().into_iter().collect(),
            poll_seconds: 60,
        }
    }
}

pub fn config_path() -> Option<PathBuf> {
    let home = std::env::home_dir()?;
    Some(home.join("Library/Application Support/nod/config.json"))
}

/// The pre-rename config location, kept so existing settings migrate forward.
fn legacy_config_path() -> Option<PathBuf> {
    let home = std::env::home_dir()?;
    Some(home.join("Library/Application Support/prdesk/config.json"))
}

/// Remembers a discovered clone path (load-modify-save, so concurrent
/// sessions can't clobber unrelated edits wholesale).
pub fn record_clone(slug: &str, path: &std::path::Path) {
    let Some(config_file) = config_path() else { return };
    let mut config = load_or_init();
    config.clones.insert(slug.to_string(), path.to_path_buf());
    if let Ok(raw) = serde_json::to_string_pretty(&config) {
        let _ = std::fs::write(&config_file, raw);
    }
}

/// Loads the config, writing the default on first run so users have a file
/// to edit.
pub fn load_or_init() -> Config {
    let Some(path) = config_path() else {
        return Config::default();
    };
    // One-time migration from the pre-rename location.
    if !path.exists() {
        if let Some(legacy) = legacy_config_path() {
            if legacy.exists() {
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::copy(&legacy, &path);
            }
        }
    }
    match std::fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|err| {
            eprintln!("nod: ignoring malformed {}: {err}", path.display());
            Config::default()
        }),
        Err(_) => {
            let config = Config::default();
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(raw) = serde_json::to_string_pretty(&config) {
                let _ = std::fs::write(&path, raw);
            }
            config
        }
    }
}
