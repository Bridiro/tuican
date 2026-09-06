//! Remembers the last session, so the second launch is `tuican` and two Enters.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::transport::TransportSpec;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub interface: Option<TransportSpec>,
    pub bitrate: u32,
    pub dbc: Option<PathBuf>,
    pub mouse: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self { interface: None, bitrate: 500_000, dbc: None, mouse: true }
    }
}

impl Config {
    pub fn path() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "tuican")
            .map(|d| d.config_dir().join("config.toml"))
    }

    /// A corrupt or old config must never stop the tool starting; fall back.
    pub fn load() -> Self {
        let Some(path) = Self::path() else { return Self::default() };
        let Ok(text) = std::fs::read_to_string(&path) else { return Self::default() };
        toml::from_str(&text).unwrap_or_else(|e| {
            tracing::warn!(error = %e, ?path, "ignoring unreadable config");
            Self::default()
        })
    }

    pub fn save(&self) {
        let Some(path) = Self::path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match toml::to_string_pretty(self) {
            Ok(text) => {
                if let Err(e) = std::fs::write(&path, text) {
                    tracing::warn!(error = %e, ?path, "could not save config");
                }
            }
            Err(e) => tracing::warn!(error = %e, "could not serialise config"),
        }
    }

    /// Only remember a DBC we can actually still find next time.
    pub fn remember_dbc(&mut self, path: &Path) {
        self.dbc = path.canonicalize().ok().or_else(|| Some(path.to_path_buf()));
    }
}
