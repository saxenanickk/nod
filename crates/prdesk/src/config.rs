//! App configuration, persisted at
//! `~/Library/Application Support/prdesk/config.json`.

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
    /// Poll interval for the PR list, in seconds.
    pub poll_seconds: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            gh_user: Some("saxenanickk".to_string()),
            scope: String::new(),
            clones: HashMap::new(),
            poll_seconds: 60,
        }
    }
}

pub fn config_path() -> Option<PathBuf> {
    let home = std::env::home_dir()?;
    Some(home.join("Library/Application Support/prdesk/config.json"))
}

/// Loads the config, writing the default on first run so users have a file
/// to edit.
pub fn load_or_init() -> Config {
    let Some(path) = config_path() else {
        return Config::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|err| {
            eprintln!("prdesk: ignoring malformed {}: {err}", path.display());
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
