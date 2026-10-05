//! What a plugin reaches on the machine that runs agents: the run it is
//! built for ([`RunCtx`]), and the host's services ([`HostCx`]).

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use tau_agent::tool::RunId;
use tau_store::{Entry, Store, TurnUsage};

use crate::services::Services;

/// Where a run sits (ADR 0016): a repository's main chat, a chat under
/// it, or a sub-agent of either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunKind {
    Main,
    Chat,
    SubAgent,
}

/// A repository runs work in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoCtx {
    /// The name the sidebar lists it under.
    pub name: String,
    /// Its checkout.
    pub checkout: PathBuf,
    /// tau's own directory for it, outside the repository: archives,
    /// memory notes.
    pub dir: PathBuf,
    /// Where its workspaces are, main's and every chat's.
    pub workspaces: PathBuf,
}

/// The directory a run (or sub-agent) works in: its workspace. The
/// host puts it in [`RunCtx::services`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceDir(pub PathBuf);

/// What the repository's own commands start through, besides a run's:
/// the launcher plugins give it, joined, and the directory its servers
/// start in, the main workspace. The host puts it in
/// [`RunCtx::services`] when a plugin gives one; tau-mcp starts the
/// repository's stdio servers through it.
#[derive(Clone)]
pub struct RepoLauncher {
    pub launcher: std::sync::Arc<dyn tau_agent::launch::Launcher>,
    pub dir: PathBuf,
}

impl std::fmt::Debug for RepoLauncher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RepoLauncher")
            .field("dir", &self.dir)
            .finish_non_exhaustive()
    }
}

/// The run a plugin is built for.
#[derive(Debug, Clone)]
pub struct RunCtx {
    pub kind: RunKind,
    pub repo: RepoCtx,
    /// The model the run (or sub-agent) runs on.
    pub model: String,
    /// The reasoning effort picked by hand; `None` is auto.
    pub effort: Option<String>,
    /// What the host has for this run: the metered Jev when there is a
    /// key, the run's workspace, and so on.
    pub services: Services,
}

/// What a run's turn left in its workspace: its commit, and the paths
/// it changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnCommit {
    pub change_id: String,
    pub paths: Vec<String>,
}

/// What hears each turn's commit in a run's workspace. The host puts one
/// in a run's services ([`RunCtx::services`]) when the run has a
/// workspace, and calls it after each turn.
#[derive(Clone, Default)]
pub struct TurnHooks(Arc<std::sync::Mutex<Vec<TurnHook>>>);

type TurnHook = Arc<dyn Fn(&TurnCommit) + Send + Sync>;

impl TurnHooks {
    /// Calls `hook` with each turn's commit from now on.
    pub fn on_turn(&self, hook: impl Fn(&TurnCommit) + Send + Sync + 'static) {
        self.0.lock().expect("not poisoned").push(Arc::new(hook));
    }

    /// Tells every hook that a turn ended with `commit`.
    pub fn turned(&self, commit: &TurnCommit) {
        let hooks = self.0.lock().expect("not poisoned").clone();
        for hook in hooks {
            hook(commit);
        }
    }
}

impl std::fmt::Debug for TurnHooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let hooks = self.0.lock().expect("not poisoned").len();
        write!(f, "TurnHooks({hooks})")
    }
}

/// tau's configuration directory, `~/.config/tau`: files the user
/// writes by hand, such as tau-mcp's `mcp.json`. A host service
/// ([`HostCx::config_dir`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigDir(pub PathBuf);

type ReadSettings = Arc<dyn Fn(&str) -> Option<Value> + Send + Sync>;
type SaveSettings =
    Arc<dyn Fn(&str, Value) -> anyhow::Result<()> + Send + Sync>;

/// The plugins' saved settings, as the host keeps them: what a plugin's
/// host half reads and saves its own settings through, when an action
/// changes them ([`HostCx::settings`], [`HostCx::save_settings`]). A
/// host service.
#[derive(Clone)]
pub struct SavedSettings {
    read: ReadSettings,
    save: SaveSettings,
}

impl SavedSettings {
    /// `read` gives a plugin's saved settings, if it has any; `save`
    /// replaces them, by plugin name.
    pub fn new(
        read: impl Fn(&str) -> Option<Value> + Send + Sync + 'static,
        save: impl Fn(&str, Value) -> anyhow::Result<()> + Send + Sync + 'static,
    ) -> Self {
        Self {
            read: Arc::new(read),
            save: Arc::new(save),
        }
    }

    /// Settings kept in memory only: for tests, and hosts that save
    /// nothing.
    pub fn in_memory() -> Self {
        let saved: Arc<
            std::sync::Mutex<std::collections::BTreeMap<String, Value>>,
        > = Arc::default();
        let read = saved.clone();
        Self::new(
            move |plugin| {
                read.lock().expect("not poisoned").get(plugin).cloned()
            },
            move |plugin, value| {
                saved
                    .lock()
                    .expect("not poisoned")
                    .insert(plugin.to_owned(), value);
                Ok(())
            },
        )
    }

    pub fn read(&self, plugin: &str) -> Option<Value> {
        (self.read)(plugin)
    }

    pub fn save(&self, plugin: &str, value: Value) -> anyhow::Result<()> {
        (self.save)(plugin, value)
    }
}

impl std::fmt::Debug for SavedSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SavedSettings")
    }
}

/// What the host tells the interface after a plugin acted.
#[derive(Debug, Clone, PartialEq)]
pub enum Push {
    /// A record stored with a run: the interface folds it like a
    /// published one.
    Record {
        run: RunId,
        plugin: String,
        body: Value,
    },
    /// The catalog changed (rules, notes, settings): draw it again.
    Catalog,
    Alert {
        title: String,
        message: String,
    },
}

/// The plugin name under which the host records how each run started
/// ([`HostRecord`]).
pub const HOST_RECORD: &str = "tau-host";

/// What the host records under [`HOST_RECORD`] as a run starts: the
/// repository it works in, and the plan a stored run shows as the live
/// one did.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize,
)]
pub struct HostRecord {
    pub repo: String,
    /// The reasoning effort picked, as the plan says it: `auto`, `high`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// What the run reached models through: `ChatGPT plan`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<String>,
    /// The directory of the workspace it worked in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
}

/// The host's services, the only part of the host a plugin reaches.
#[derive(Clone)]
pub struct HostCx {
    pub store: Store,
    /// The host's runtime, to spawn background work on. Nothing blocks
    /// on it (ADR 0028).
    pub runtime: tokio::runtime::Handle,
    /// Host-wide services, by type.
    pub services: Services,
    /// tau's directory for data shared by every repository.
    pub dir: PathBuf,
    /// The repositories the host lists.
    pub repos: Vec<RepoCtx>,
    push: Arc<dyn Fn(Push) + Send + Sync>,
}

impl HostCx {
    /// Where the plugin `plugin` keeps its own files, such as its
    /// database: a directory of its own under tau's.
    pub fn plugin_dir(&self, plugin: &str) -> PathBuf {
        self.dir.join("plugins").join(plugin)
    }

    pub fn new(
        store: Store,
        runtime: tokio::runtime::Handle,
        services: Services,
        dir: PathBuf,
        repos: Vec<RepoCtx>,
        push: Arc<dyn Fn(Push) + Send + Sync>,
    ) -> Self {
        Self {
            store,
            runtime,
            services,
            dir,
            repos,
            push,
        }
    }

    /// The repository listed as `name`.
    pub fn repo(&self, name: &str) -> Option<&RepoCtx> {
        self.repos.iter().find(|repo| repo.name == name)
    }

    /// `plugin`'s records along `run`'s fork chain, oldest first.
    pub async fn records(
        &self,
        run: &RunId,
        plugin: &str,
    ) -> anyhow::Result<Vec<Value>> {
        let bodies = self.store.records(&run.0, plugin).await?;
        Ok(bodies
            .iter()
            .filter_map(|body| serde_json::from_str(body).ok())
            .collect())
    }

    /// Every stored run that works in `repo`.
    pub async fn runs_in(
        &self,
        repo: &RepoCtx,
    ) -> anyhow::Result<std::collections::BTreeSet<RunId>> {
        Ok(self
            .records_everywhere(HOST_RECORD)
            .await?
            .into_iter()
            .filter(|(_, body)| {
                serde_json::from_value::<HostRecord>(body.clone())
                    .is_ok_and(|record| record.repo == repo.name)
            })
            .map(|(run, _)| run)
            .collect())
    }

    /// `plugin`'s records in every run, each with the run that stored it.
    pub async fn records_everywhere(
        &self,
        plugin: &str,
    ) -> anyhow::Result<Vec<(RunId, Value)>> {
        let rows = self.store.plugin_entries_everywhere(plugin).await?;
        Ok(rows
            .into_iter()
            .filter_map(|(run, body)| {
                Some((RunId(run.into()), serde_json::from_str(&body).ok()?))
            })
            .collect())
    }

    /// Stores `body` as `plugin`'s record with `run`, and hands it to the
    /// interface, which folds it as if the run had published it: an
    /// interface's own change to a plugin's state (pause a goal).
    pub async fn publish(
        &self,
        run: &RunId,
        plugin: &str,
        body: &Value,
    ) -> anyhow::Result<()> {
        let entry = Entry::Plugin {
            plugin: plugin.to_owned(),
            body: body.to_string(),
        };
        self.store
            .append_turn(&run.0, &[entry], TurnUsage::default())
            .await?;
        (self.push)(Push::Record {
            run: run.clone(),
            plugin: plugin.to_owned(),
            body: body.clone(),
        });
        Ok(())
    }

    /// tau's configuration directory, when the host says where it is.
    pub fn config_dir(&self) -> Option<&Path> {
        self.services.get::<ConfigDir>().map(|dir| dir.0.as_path())
    }

    /// `plugin`'s saved settings, or their default when it has none, or
    /// they no longer read as `T`.
    pub fn settings<T: DeserializeOwned + Default>(&self, plugin: &str) -> T {
        self.services
            .get::<SavedSettings>()
            .and_then(|saved| saved.read(plugin))
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default()
    }

    /// Saves `settings` as `plugin`'s, as its page would, and asks the
    /// interface to draw the catalog again, which carries them. Runs
    /// started from now on take them.
    pub fn save_settings(
        &self,
        plugin: &str,
        settings: &impl Serialize,
    ) -> anyhow::Result<()> {
        let saved = self
            .services
            .get::<SavedSettings>()
            .ok_or_else(|| anyhow::anyhow!("This host saves no settings"))?;
        saved.save(plugin, serde_json::to_value(settings)?)?;
        self.refresh();
        Ok(())
    }

    /// Asks the interface to draw the catalog again.
    pub fn refresh(&self) {
        (self.push)(Push::Catalog);
    }

    pub fn alert(&self, title: impl Into<String>, message: impl Into<String>) {
        (self.push)(Push::Alert {
            title: title.into(),
            message: message.into(),
        });
    }
}

#[cfg(test)]
#[allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// A hook hears every commit after it was added, in order, and none
    /// before.
    #[hegel::test(test_cases = 100)]
    fn a_hook_hears_the_turns_after_it(tc: hegel::TestCase) {
        use hegel::generators as gs;
        // Each step adds a hook (`true`) or ends a turn (`false`).
        let steps: Vec<bool> = tc.draw(gs::vecs(gs::booleans()).max_size(12));
        let hooks = TurnHooks::default();
        let heard: Arc<Mutex<Vec<(usize, String)>>> = Arc::default();
        let mut expected = Vec::new();
        let mut added = 0;
        for (n, adds) in steps.iter().enumerate() {
            if *adds {
                let (heard, hook) = (heard.clone(), added);
                hooks.on_turn(move |commit| {
                    heard.lock().unwrap().push((hook, commit.change_id.clone()))
                });
                added += 1;
            } else {
                let change_id = format!("c{n}");
                expected
                    .extend((0..added).map(|hook| (hook, change_id.clone())));
                hooks.turned(&TurnCommit {
                    change_id,
                    paths: Vec::new(),
                });
            }
        }
        assert_eq!(*heard.lock().unwrap(), expected);
    }

    /// A host half's settings come back as it saved them, by plugin; one
    /// with none, or with settings of another shape, reads the default;
    /// and each save asks the interface for the catalog again.
    #[hegel::test(test_cases = 50)]
    fn saved_settings_come_back_by_plugin(tc: hegel::TestCase) {
        use hegel::generators as gs;
        let saves: Vec<(bool, Vec<u32>)> = tc.draw(
            gs::vecs(hegel::tuples!(
                gs::booleans(),
                gs::vecs(gs::integers::<u32>()).max_size(4),
            ))
            .max_size(6),
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let store = runtime.block_on(Store::memory()).unwrap();
        let pushes: Arc<Mutex<Vec<Push>>> = Arc::default();
        let heard = pushes.clone();
        let cx = HostCx::new(
            store,
            runtime.handle().clone(),
            Services::default()
                .with(SavedSettings::in_memory())
                .with(ConfigDir(PathBuf::from("/config/tau"))),
            PathBuf::from("/data/tau"),
            Vec::new(),
            Arc::new(move |push| heard.lock().unwrap().push(push)),
        );
        let mut expected: [Vec<u32>; 2] = Default::default();
        for (second, value) in &saves {
            let plugin = if *second { "b" } else { "a" };
            cx.save_settings(plugin, value).unwrap();
            expected[usize::from(*second)] = value.clone();
        }
        assert_eq!(cx.settings::<Vec<u32>>("a"), expected[0]);
        assert_eq!(cx.settings::<Vec<u32>>("b"), expected[1]);
        assert_eq!(cx.settings::<Vec<u32>>("c"), Vec::<u32>::new());
        // Another shape is the default, not an error.
        cx.save_settings("c", &"text").unwrap();
        assert_eq!(cx.settings::<Vec<u32>>("c"), Vec::<u32>::new());
        assert_eq!(pushes.lock().unwrap().len(), saves.len() + 1);
        assert_eq!(cx.config_dir(), Some(Path::new("/config/tau")));
    }
}
