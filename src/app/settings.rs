//! Persisted session preferences. Firmware and assets are never stored in the repository.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Settings {
    pub firmware: String,
    pub assets: String,
    pub offline: bool,
    pub hardware: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            firmware: String::new(),
            assets: String::new(),
            offline: true,
            hardware: false,
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        std::fs::read(settings_path())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = settings_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }

        let bytes = serde_json::to_vec_pretty(self).map_err(|error| error.to_string())?;
        std::fs::write(path, bytes).map_err(|error| error.to_string())
    }
}

fn settings_path() -> PathBuf {
    let directory = if cfg!(windows) {
        "LOCALAPPDATA"
    } else {
        "XDG_CONFIG_HOME"
    };
    let root = std::env::var_os(directory)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir)
                .join(".config")
        });

    root.join("DSMC-Emulator").join("settings.json")
}
