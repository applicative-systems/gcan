//! Persistent cache of immutable Nix facts.
//!
//! Store paths never change after creation, so a path's NAR size and a fixed
//! member set's union closure size are valid forever. A warm, unchanged run
//! reads every group size straight from `groups` and issues zero `nix-store`
//! calls.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
pub struct Cache {
    pub version: u32,
    /// store path -> own NAR size (bytes).
    pub sizes: HashMap<String, u64>,
    /// member-set key (sorted store paths joined by '\n') -> union closure size.
    pub groups: HashMap<String, u64>,
}

impl Cache {
    pub fn empty() -> Self {
        Cache {
            version: VERSION,
            sizes: HashMap::new(),
            groups: HashMap::new(),
        }
    }

    /// Load the cache, falling back to empty on any error or version mismatch —
    /// a corrupt or half-written cache is never fatal.
    pub fn load(path: &Path) -> Self {
        match std::fs::read(path) {
            Ok(bytes) => match serde_json::from_slice::<Cache>(&bytes) {
                Ok(c) if c.version == VERSION => c,
                _ => Cache::empty(),
            },
            Err(_) => Cache::empty(),
        }
    }

    /// Persist atomically: write a sibling temp file then rename over the target.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let tmp = path.with_file_name(format!("{name}.tmp"));
        std::fs::write(&tmp, serde_json::to_vec(self)?)?;
        std::fs::rename(&tmp, path)
    }
}

/// `${XDG_CACHE_HOME:-$HOME/.cache}/gcan/cache.json`.
pub fn default_path() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."));
            home.join(".cache")
        });
    base.join("gcan").join("cache.json")
}
