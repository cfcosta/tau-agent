//! The plugins the host has active, read from the plugins repository's
//! trunk (ADR 0027).
//!
//! The registry watches trunk. When it moves, each plugin's folder at
//! the new commit is loaded and tested, and activates by itself unless
//! its tests fail or it reaches further than the version the person
//! allowed; then the version before stays active, and the Plugins
//! screen says why. A run takes the plugins active before each of its
//! model requests, so a change reaches runs already going.
//!
//! A run in the plugins repository has its own workspace's versions
//! too ([`Registry::in_workspace`]): a plugin it wrote or changed works
//! for it as soon as that version's tests pass, before it lands, and for
//! every other run once it lands on trunk.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use serde::{Deserialize, Serialize};
pub use tau_luau_plugins::{
    REPO,
    repository::{ROOT, first_files},
};
use tau_ui_plugin::{HostCx, PluginHost};
use tau_vcs_host::{Identity, Project};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::{
    Declaration,
    Entry,
    Overview,
    Standing,
    agent::Active,
    runtime::{Files, Loaded, load},
};

/// How often trunk is read for a change.
const POLL: Duration = Duration::from_secs(2);

/// The version of a plugin the person allowed: what it declared.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Allowed {
    digest: String,
    declaration: Declaration,
}

/// The host's Luau plugins. Clones share them.
#[derive(Clone)]
pub struct Registry(Arc<Shared>);

struct Shared {
    root: PathBuf,
    /// Where what the person allowed is kept.
    allowed_path: PathBuf,
    state: RwLock<State>,
    /// Versions read from runs' workspaces, by their files' digest: the
    /// version when it may be active, else `None`. Each is loaded and
    /// tested once.
    tried: tokio::sync::Mutex<BTreeMap<String, Option<Loaded>>>,
    /// Asks the interface to draw the catalog again.
    refresh: Box<dyn Fn() + Send + Sync>,
}

#[derive(Default)]
struct State {
    overview: Overview,
    active: Vec<Active>,
    /// Versions waiting for the person, by plugin.
    waiting: BTreeMap<String, Loaded>,
}

impl PluginHost for Registry {
    async fn new(cx: &HostCx) -> anyhow::Result<Self> {
        // The skill runs read to write a plugin, as this build has it.
        let skills = cx.dir.join(tau_skills::BUILTIN_DIR);
        tokio::task::spawn_blocking(move || crate::skill::install(&skills))
            .await??;
        let refresher = cx.clone();
        let registry = Self::at(
            cx.dir.join(ROOT),
            cx.plugin_dir(crate::NAME).join("allowed.json"),
            move || refresher.refresh(),
        );
        let watcher = registry.clone();
        cx.runtime.spawn(async move { watcher.watch().await });
        Ok(registry)
    }
}

impl Registry {
    /// A registry of the plugins repository at `root`, keeping what the
    /// person allowed at `allowed`; `refresh` redraws the catalog. Empty
    /// until it reads trunk.
    pub fn at(
        root: PathBuf,
        allowed: PathBuf,
        refresh: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self(Arc::new(Shared {
            root,
            allowed_path: allowed,
            state: RwLock::default(),
            tried: tokio::sync::Mutex::default(),
            refresh: Box::new(refresh),
        }))
    }

    /// The plugins every new run gets.
    pub async fn active(&self) -> Vec<Active> {
        self.0.state.read().await.active.clone()
    }

    /// The plugins a run whose workspace is `dir`, in the plugins
    /// repository, has active: trunk's, each in its version in `dir`
    /// instead when that version loads, passes its tests and reaches no
    /// further than the person allowed; else trunk's stays. A plugin
    /// `dir` lacks keeps trunk's version: removing one takes effect when
    /// the removal lands.
    pub async fn in_workspace(&self, dir: &Path) -> Vec<Active> {
        let mut active = self.active().await;
        let dir = dir.to_owned();
        let folders = tokio::task::spawn_blocking(move || plugin_folders(&dir))
            .await
            .unwrap_or_default();
        if folders.is_empty() {
            return active;
        }
        let allowed = read_allowed(&self.0.allowed_path).await;
        for (name, files) in folders {
            let digest = files.digest();
            let at = active
                .iter()
                .position(|active| active.loaded.declaration.name == name);
            if at.is_some_and(|at| active[at].loaded.digest == digest) {
                continue;
            }
            let Some(loaded) = self.try_version(&name, files, &allowed).await
            else {
                continue;
            };
            let settings = loaded.declaration.default_settings();
            let version = Active { loaded, settings };
            match at {
                Some(at) => active[at] = version,
                None => active.push(version),
            }
        }
        active
    }

    /// `name` in `files`, when it may be active: it loads, its tests
    /// pass, and it reaches no further than `allowed` lets it.
    async fn try_version(
        &self,
        name: &str,
        files: Files,
        allowed: &BTreeMap<String, Allowed>,
    ) -> Option<Loaded> {
        let digest = files.digest();
        let mut tried = self.0.tried.lock().await;
        if let Some(known) = tried.get(&digest) {
            return known.clone();
        }
        let version = match load(name, files).await {
            Ok(loaded) => {
                let tests = loaded.test(CancellationToken::new()).await;
                let grown = match allowed.get(name) {
                    Some(allowed) if allowed.digest == loaded.digest => {
                        Vec::new()
                    }
                    Some(allowed) => {
                        loaded.declaration.grown_from(&allowed.declaration)
                    }
                    None => {
                        loaded.declaration.grown_from(&Declaration::default())
                    }
                };
                (tests.iter().all(|test| test.passed) && grown.is_empty())
                    .then_some(loaded)
            }
            Err(_) => None,
        };
        tried.insert(digest, version.clone());
        version
    }

    /// The repository as last read, for the Plugins screen.
    pub async fn overview(&self) -> Overview {
        self.0.state.read().await.overview.clone()
    }

    /// Reads trunk every [`POLL`], and reloads when it moved. Runs for
    /// the host's life.
    async fn watch(&self) {
        let mut project: Option<Project> = None;
        loop {
            if project.is_none() {
                project =
                    Project::open(self.0.root.clone(), Identity::default())
                        .await
                        .ok();
            }
            if let Some(open) = &project {
                match open.run(|repo| repo.trunk()).await {
                    Ok(trunk) => {
                        let seen =
                            self.0.state.read().await.overview.commit.clone();
                        if seen.as_deref() != Some(trunk.as_str()) {
                            self.reload(open, &trunk).await;
                        }
                    }
                    Err(error) => {
                        self.0.state.write().await.overview.error =
                            Some(error.to_string());
                    }
                }
            }
            tokio::time::sleep(POLL).await;
        }
    }

    /// Loads and tests every plugin at `trunk`, activates what may
    /// activate, and redraws the catalog.
    pub async fn reload(&self, project: &Project, trunk: &str) {
        let at = trunk.to_owned();
        let read = project.run(move |repo| repo.files_under(&at, "")).await;
        let files = match read {
            Ok(files) => files,
            Err(error) => {
                self.0.state.write().await.overview.error =
                    Some(error.to_string());
                (self.0.refresh)();
                return;
            }
        };
        // Each top-level folder with files is a plugin.
        let mut folders: BTreeMap<String, Vec<(String, Vec<u8>)>> =
            BTreeMap::new();
        for (path, bytes) in files {
            if let Some((folder, rest)) = path.split_once('/') {
                folders
                    .entry(folder.to_owned())
                    .or_default()
                    .push((rest.to_owned(), bytes));
            }
        }
        let path = self.0.allowed_path.clone();
        let mut allowed = read_allowed(&path).await;
        let before: BTreeMap<String, Active> = self
            .0
            .state
            .read()
            .await
            .active
            .iter()
            .map(|active| {
                (active.loaded.declaration.name.clone(), active.clone())
            })
            .collect();
        let mut state = State {
            overview: Overview {
                commit: Some(trunk.to_owned()),
                plugins: Vec::new(),
                error: None,
            },
            ..State::default()
        };
        for (name, paths) in folders {
            let earlier = before.get(&name).cloned();
            let loaded = match Files::from_paths(&paths) {
                Ok(files) => load(&name, files).await,
                Err(error) => Err(error),
            };
            let loaded = match loaded {
                Ok(loaded) => loaded,
                Err(error) => {
                    state.overview.plugins.push(Entry {
                        name: name.clone(),
                        description: String::new(),
                        standing: Standing::Broken { error },
                        declaration: None,
                        tests: Vec::new(),
                        keeps_earlier: earlier.is_some(),
                    });
                    state.active.extend(earlier);
                    continue;
                }
            };
            let tests = loaded.test(CancellationToken::new()).await;
            let declaration = loaded.declaration.clone();
            let grown = match allowed.get(&name) {
                Some(allowed) if allowed.digest == loaded.digest => Vec::new(),
                Some(allowed) => declaration.grown_from(&allowed.declaration),
                None => declaration.grown_from(&Declaration::default()),
            };
            let standing = if tests.iter().any(|test| !test.passed) {
                Standing::Failing
            } else if !grown.is_empty() {
                Standing::Waiting { grown }
            } else {
                Standing::Active
            };
            let active = matches!(standing, Standing::Active);
            state.overview.plugins.push(Entry {
                name: name.clone(),
                description: declaration.description.clone(),
                standing,
                declaration: Some(declaration.clone()),
                tests,
                keeps_earlier: !active && earlier.is_some(),
            });
            if active {
                allowed.insert(
                    name,
                    Allowed {
                        digest: loaded.digest.clone(),
                        declaration,
                    },
                );
                let settings = loaded.declaration.default_settings();
                state.active.push(Active { loaded, settings });
            } else {
                state.waiting.insert(name, loaded);
                state.active.extend(earlier);
            }
        }
        write_allowed(&path, &allowed).await;
        *self.0.state.write().await = state;
        (self.0.refresh)();
    }

    /// Draws `plugin`'s settings page of `settings`: the version waiting
    /// for the person, else the active one.
    pub async fn settings_page(
        &self,
        plugin: &str,
        settings: serde_json::Value,
    ) -> crate::SettingsPage {
        let loaded = {
            let state = self.0.state.read().await;
            state.waiting.get(plugin).cloned().or_else(|| {
                state
                    .active
                    .iter()
                    .find(|active| active.loaded.declaration.name == plugin)
                    .map(|active| active.loaded.clone())
            })
        };
        let page = match loaded {
            None => Err(format!("{plugin} is not loaded")),
            Some(loaded) => {
                let context = crate::runtime::Context {
                    settings: settings.clone(),
                    ..Default::default()
                };
                let outcome = loaded
                    .call(
                        &crate::runtime::Hook::SettingsView,
                        settings.clone(),
                        &context,
                        std::sync::Arc::new(crate::runtime::NoReach),
                        CancellationToken::new(),
                    )
                    .await;
                match outcome.error {
                    Some(error) => Err(error),
                    None => Ok(outcome.value),
                }
            }
        };
        crate::SettingsPage {
            plugin: plugin.to_owned(),
            settings,
            page,
        }
    }

    /// The person allows `plugin`'s waiting version: it activates, and
    /// what it reaches becomes what that plugin may reach.
    pub async fn allow(&self, plugin: &str) -> anyhow::Result<()> {
        let mut state = self.0.state.write().await;
        let Some(loaded) = state.waiting.remove(plugin) else {
            anyhow::bail!("{plugin} has no version waiting");
        };
        let path = self.0.allowed_path.clone();
        let mut allowed = read_allowed(&path).await;
        allowed.insert(
            plugin.to_owned(),
            Allowed {
                digest: loaded.digest.clone(),
                declaration: loaded.declaration.clone(),
            },
        );
        write_allowed(&path, &allowed).await;
        state
            .active
            .retain(|active| active.loaded.declaration.name != plugin);
        let settings = loaded.declaration.default_settings();
        state.active.push(Active { loaded, settings });
        for entry in &mut state.overview.plugins {
            if entry.name == plugin {
                entry.standing = Standing::Active;
                entry.keeps_earlier = false;
            }
        }
        drop(state);
        (self.0.refresh)();
        Ok(())
    }
}

/// The plugin folders in a workspace `dir`: each top-level folder with a
/// `plugin.luau`, as its files.
fn plugin_folders(dir: &Path) -> Vec<(String, Files)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut folders: Vec<(String, Files)> = entries
        .flatten()
        .filter(|entry| {
            entry.path().join(crate::runtime::PLUGIN_FILE).is_file()
        })
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_owned();
            if name.starts_with('.') {
                return None;
            }
            Some((name, Files::read(&entry.path()).ok()?))
        })
        .collect();
    folders.sort_by(|a, b| a.0.cmp(&b.0));
    folders
}

async fn read_allowed(path: &Path) -> BTreeMap<String, Allowed> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    })
    .await
    .unwrap_or_default()
}

async fn write_allowed(path: &Path, allowed: &BTreeMap<String, Allowed>) {
    let (path, text) = (
        path.to_owned(),
        serde_json::to_string_pretty(allowed).unwrap_or_default(),
    );
    let written = tokio::task::spawn_blocking(move || {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, text)
    })
    .await;
    if let Ok(Err(error)) = written {
        eprintln!("{}: cannot keep what was allowed: {error}", crate::NAME);
    }
}
