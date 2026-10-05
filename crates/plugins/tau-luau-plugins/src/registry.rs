//! The plugins the host has active, read from tau's plugins folder
//! (ADR 0027). Every run's agent gets them as they are when it starts.

use std::{path::PathBuf, sync::Arc};

use tau_ui_plugin::{HostCx, PluginHost};
use tokio::sync::RwLock;

use crate::{
    agent::Active,
    runtime::{Files, load},
};

/// The plugins folder under tau's data directory.
pub const FOLDER: &str = "plugins";

/// The host's Luau plugins: those that loaded, and those that did not,
/// with why.
pub struct Registry {
    dir: PathBuf,
    loaded: RwLock<Loaded>,
}

#[derive(Default)]
struct Loaded {
    active: Vec<Active>,
    broken: Vec<(String, String)>,
}

impl PluginHost for Registry {
    async fn new(cx: &HostCx) -> anyhow::Result<Self> {
        let registry = Self::at(cx.dir.join(FOLDER));
        registry.reload().await;
        Ok(registry)
    }
}

impl Registry {
    /// A registry of the plugins in `dir`, empty until reloaded.
    pub fn at(dir: PathBuf) -> Self {
        Self {
            dir,
            loaded: RwLock::default(),
        }
    }

    /// The plugins every new run gets.
    pub async fn active(&self) -> Vec<Active> {
        self.loaded.read().await.active.clone()
    }

    /// The plugins that did not load, by folder, and why.
    pub async fn broken(&self) -> Vec<(String, String)> {
        self.loaded.read().await.broken.clone()
    }

    /// Reads every folder in the plugins folder again and loads it.
    pub async fn reload(&self) {
        let dir = self.dir.clone();
        let folders = tokio::task::spawn_blocking(move || {
            let mut found: Vec<(String, Result<Files, String>)> = Vec::new();
            let Ok(entries) = std::fs::read_dir(&dir) else {
                return found;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.join(crate::runtime::PLUGIN_FILE).is_file() {
                    continue;
                }
                let Some(name) = path.file_name().and_then(|n| n.to_str())
                else {
                    continue;
                };
                found.push((name.to_owned(), Files::read(&path)));
            }
            found.sort_by(|a, b| a.0.cmp(&b.0));
            found
        })
        .await
        .unwrap_or_default();
        let mut loaded = Loaded::default();
        for (name, files) in folders {
            match files {
                Ok(files) => match load(&name, files).await {
                    Ok(plugin) => {
                        let settings = plugin.declaration.default_settings();
                        loaded.active.push(Active {
                            loaded: plugin,
                            settings,
                        });
                    }
                    Err(error) => loaded.broken.push((name, error)),
                },
                Err(error) => loaded.broken.push((name, error)),
            }
        }
        *self.loaded.write().await = loaded;
    }
}

/// A registry shared by the host and the code that reloads it.
pub type SharedRegistry = Arc<Registry>;
