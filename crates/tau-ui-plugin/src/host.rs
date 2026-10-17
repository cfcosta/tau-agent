//! What a plugin reaches on the machine that runs agents: the run it is
//! built for ([`RunCtx`]), and the host's services ([`HostCx`]).

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
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

/// A repository's own id: given once, when tau lists it, and kept when
/// its folder moves. A plugin keys what it keeps for a repository on
/// it.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    Serialize,
    Deserialize,
)]
#[serde(transparent)]
pub struct RepoId(pub String);

impl std::fmt::Display for RepoId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A repository runs work in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoCtx {
    /// Its own id, which no path is made into.
    pub id: RepoId,
    /// The name the sidebar lists it under.
    pub name: String,
    /// Where it is fetched from and pushed to, such as
    /// `https://github.com/owner/name`, when it has a remote.
    pub remote: Option<String>,
    /// The person's own folder, for a repository tau works on in place
    /// (ADR 0043); `None` for a clone tau keeps.
    pub folder: Option<PathBuf>,
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

type ReadSettings =
    Arc<dyn Fn(&str, Option<&str>) -> Option<Value> + Send + Sync>;
type SaveSettings = Arc<
    dyn Fn(
            &str,
            Option<&str>,
            Value,
        ) -> crate::registry::HostFuture<'static, anyhow::Result<()>>
        + Send
        + Sync,
>;

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
    /// replaces them, by plugin name. With a repository, they are that
    /// repository's copy (ADR 0029): `read` falls back to the plugin's
    /// own when the repository keeps none.
    pub fn new(
        read: impl Fn(&str, Option<&str>) -> Option<Value> + Send + Sync + 'static,
        save: impl Fn(
            &str,
            Option<&str>,
            Value,
        )
            -> crate::registry::HostFuture<'static, anyhow::Result<()>>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            read: Arc::new(read),
            save: Arc::new(save),
        }
    }

    /// Settings kept in memory only: for tests, and hosts that save
    /// nothing.
    pub fn in_memory() -> Self {
        type Kept = std::collections::BTreeMap<(String, Option<String>), Value>;
        let saved: Arc<std::sync::Mutex<Kept>> = Arc::default();
        let read = saved.clone();
        Self::new(
            move |plugin, repo| {
                let kept = read.lock().expect("not poisoned");
                repo.and_then(|repo| {
                    kept.get(&(plugin.to_owned(), Some(repo.to_owned())))
                })
                .or_else(|| kept.get(&(plugin.to_owned(), None)))
                .cloned()
            },
            move |plugin, repo, value| {
                saved.lock().expect("not poisoned").insert(
                    (plugin.to_owned(), repo.map(str::to_owned)),
                    value,
                );
                Box::pin(std::future::ready(Ok(())))
            },
        )
    }

    /// `plugin`'s saved settings: `repo`'s copy when given and it keeps
    /// one, else the plugin's own.
    pub fn read(&self, plugin: &str, repo: Option<&str>) -> Option<Value> {
        (self.read)(plugin, repo)
    }

    /// Saves `plugin`'s settings, as `repo`'s copy when given; the host
    /// writes them on its runtime.
    pub async fn save(
        &self,
        plugin: &str,
        repo: Option<&str>,
        value: Value,
    ) -> anyhow::Result<()> {
        (self.save)(plugin, repo, value).await
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

/// Who a turn's message is from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum By {
    /// The person: what they asked for, said for them.
    Person,
    /// tau: shown as tau's, not the person's.
    Tau,
}

/// A turn a host half starts on a repository's main chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    pub repo: String,
    /// What the turn starts with.
    pub text: String,
    pub by: By,
    pub model: String,
    /// `auto`, `low` and so on.
    pub effort: String,
}

/// The model and effort a repository's conversations go to by default,
/// as a plugin sets it ([`crate::HostHalf::conversation_model`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelPick {
    pub model: String,
    /// `auto`, `low` and so on.
    pub effort: String,
}

/// What came of waking a chat ([`Conversations::wake`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waking {
    /// Its turn started on tau's message.
    Started,
    /// A turn of it is going: what woke it waits for that turn to end.
    Busy,
    /// It landed, was dropped, or is not there: it takes no message.
    Gone,
}

/// The host's conversations, which a plugin's host half may drive: an
/// agent's repository and its main chat, a conversation that never ends
/// (ADR 0037), and any chat a plugin wakes later (ADR 0039). The host
/// puts it in [`HostCx::services`] as an `Arc<dyn Conversations>`.
pub trait Conversations: Send + Sync {
    /// Makes the agent `name`'s repository, lists it as tau's own and
    /// makes its main chat.
    fn list_agent(
        &self,
        name: &str,
    ) -> crate::registry::HostFuture<'_, anyhow::Result<()>>;

    /// The id of `repo`'s main chat, once it is made.
    fn main_of(&self, repo: &str) -> Option<String>;

    /// Whether `repo`'s main chat has a turn going.
    fn running(&self, repo: &str) -> bool;

    /// Starts a turn of `repo`'s main chat with `turn.text`. It refuses
    /// while a turn is going: what comes then waits for the next one.
    fn turn(
        &self,
        turn: Turn,
    ) -> crate::registry::HostFuture<'_, anyhow::Result<()>>;

    /// Starts a turn of the chat `run` on tau's message `text`, on the
    /// model it last ran on, unless a turn of it is going or it takes no
    /// more messages. A host that wakes no chats refuses.
    fn wake(
        &self,
        run: &str,
        text: String,
    ) -> crate::registry::HostFuture<'_, anyhow::Result<Waking>> {
        let _ = (run, text);
        Box::pin(async { anyhow::bail!("This host wakes no chats") })
    }
}

/// One plugin's tools that run outside any run: what an agent's polls
/// call (ADR 0037). A plugin whose tools need no run offers them in the
/// host's [`OutsideTools`].
pub trait ToolSource: Send + Sync {
    /// Calls `name` with `args` for the repository checked out at
    /// `checkout`, when `name` is one of this source's tools; `None`
    /// when it is not. The answer is what a script would get: the
    /// tool's structured value, else its text.
    fn call<'a>(
        &'a self,
        checkout: &'a Path,
        name: &'a str,
        args: Value,
    ) -> Option<crate::registry::HostFuture<'a, Result<Value, String>>>;
}

/// The tools a host half may call outside any run, from the plugins
/// that offer them. The host puts one in [`HostCx::services`]; clones
/// share it.
#[derive(Clone, Default)]
pub struct OutsideTools(Arc<std::sync::Mutex<Vec<Arc<dyn ToolSource>>>>);

impl OutsideTools {
    /// Adds `source`'s tools.
    pub fn offer(&self, source: Arc<dyn ToolSource>) {
        self.0.lock().expect("not poisoned").push(source);
    }

    /// Calls `name` with `args` for the repository at `checkout`,
    /// through the first source that has it.
    pub async fn call(
        &self,
        checkout: &Path,
        name: &str,
        args: Value,
    ) -> Result<Value, String> {
        let sources = self.0.lock().expect("not poisoned").clone();
        for source in &sources {
            if let Some(call) = source.call(checkout, name, args.clone()) {
                return call.await;
            }
        }
        Err(format!("no tool `{name}` can be called outside a run here"))
    }
}

/// A repository no person attends: an agent's (ADR 0037). Its MCP
/// servers connect without the person's approval, and its stdio servers
/// start only through its sandbox.
#[derive(Clone)]
pub struct UnattendedRepo {
    /// What its stdio servers start through, or why none can: they never
    /// start outside it.
    pub sandbox: Result<RepoLauncher, String>,
}

/// What says which repositories no person attends: a plugin that keeps
/// them offers one to the host's [`Unattended`].
pub trait UnattendedSource: Send + Sync {
    /// What it keeps of the repository checked out at `checkout`, when
    /// it is one of its own.
    fn unattended(&self, checkout: &Path) -> Option<UnattendedRepo>;
}

/// The repositories no person attends, from the plugins that keep them.
/// The host puts one in [`HostCx::services`]; clones share it.
#[derive(Clone, Default)]
pub struct Unattended(Arc<std::sync::Mutex<Vec<Arc<dyn UnattendedSource>>>>);

impl Unattended {
    /// Adds `source`'s repositories.
    pub fn offer(&self, source: Arc<dyn UnattendedSource>) {
        self.0.lock().expect("not poisoned").push(source);
    }

    /// The repository checked out at `checkout`, when a source keeps it.
    pub fn of(&self, checkout: &Path) -> Option<UnattendedRepo> {
        let sources = self.0.lock().expect("not poisoned").clone();
        sources
            .iter()
            .find_map(|source| source.unattended(checkout))
    }
}

impl std::fmt::Debug for Unattended {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let sources = self.0.lock().expect("not poisoned").len();
        write!(f, "Unattended({sources} sources)")
    }
}

/// A watch on one MCP resource, outside any run: what an agent's `mcp`
/// trigger follows (ADR 0037). It says each time the resource changed;
/// dropping it stops watching.
pub struct ResourceWatch {
    updates: tokio::sync::mpsc::UnboundedReceiver<()>,
    _held: Box<dyn Send + Sync>,
}

impl ResourceWatch {
    /// A watch that hears of each change on `updates`, keeping `held`,
    /// whatever stops the watching when it drops, until it ends.
    pub fn new(
        updates: tokio::sync::mpsc::UnboundedReceiver<()>,
        held: impl Send + Sync + 'static,
    ) -> Self {
        Self {
            updates,
            _held: Box::new(held),
        }
    }

    /// Waits until the resource changes: `false` once the watch ended,
    /// as its connection did. Watching again connects again.
    pub async fn changed(&mut self) -> bool {
        self.updates.recv().await.is_some()
    }
}

impl std::fmt::Debug for ResourceWatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ResourceWatch")
    }
}

/// Why a resource cannot be watched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchError {
    /// Its server does not say when its resources change: only a poll
    /// can follow it.
    Unsupported(String),
    /// Anything else, such as no connection: watching again may work.
    Failed(String),
}

impl std::fmt::Display for WatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(why) | Self::Failed(why) => f.write_str(why),
        }
    }
}

/// One plugin's MCP resources outside any run: what an agent's `mcp`
/// triggers watch and read (ADR 0037).
pub trait ResourceSource: Send + Sync {
    /// Reads `uri` of the server `server` for the repository checked out
    /// at `checkout`: its `ReadResourceResult` as JSON. `None` when
    /// `server` is not one of this source's there.
    fn read<'a>(
        &'a self,
        checkout: &'a Path,
        server: &'a str,
        uri: &'a str,
    ) -> Option<crate::registry::HostFuture<'a, Result<Value, String>>>;

    /// Watches `uri` of `server` for the repository at `checkout`.
    /// `None` when `server` is not one of this source's there.
    fn watch<'a>(
        &'a self,
        checkout: &'a Path,
        server: &'a str,
        uri: &'a str,
    ) -> Option<
        crate::registry::HostFuture<'a, Result<ResourceWatch, WatchError>>,
    >;

    /// Waits until `server`'s connection for the repository at
    /// `checkout` changes: it connects again, or what it offers changes,
    /// so a watch it could not take may be tried again. `None` when
    /// `server` is not one of this source's there.
    fn changed<'a>(
        &'a self,
        _checkout: &'a Path,
        _server: &'a str,
    ) -> Option<crate::registry::HostFuture<'a, ()>> {
        None
    }
}

/// The MCP resources a host half may read and watch outside any run,
/// from the plugins that offer them. The host puts one in
/// [`HostCx::services`]; clones share it.
#[derive(Clone, Default)]
pub struct OutsideResources(
    Arc<std::sync::Mutex<Vec<Arc<dyn ResourceSource>>>>,
);

impl OutsideResources {
    /// Adds `source`'s resources.
    pub fn offer(&self, source: Arc<dyn ResourceSource>) {
        self.0.lock().expect("not poisoned").push(source);
    }

    fn sources(&self) -> Vec<Arc<dyn ResourceSource>> {
        self.0.lock().expect("not poisoned").clone()
    }

    /// Reads `uri` of `server` for the repository at `checkout`, through
    /// the first source that has the server.
    pub async fn read(
        &self,
        checkout: &Path,
        server: &str,
        uri: &str,
    ) -> Result<Value, String> {
        for source in &self.sources() {
            if let Some(read) = source.read(checkout, server, uri) {
                return read.await;
            }
        }
        Err(no_server(server))
    }

    /// Watches `uri` of `server` for the repository at `checkout`,
    /// through the first source that has the server.
    pub async fn watch(
        &self,
        checkout: &Path,
        server: &str,
        uri: &str,
    ) -> Result<ResourceWatch, WatchError> {
        for source in &self.sources() {
            if let Some(watch) = source.watch(checkout, server, uri) {
                return watch.await;
            }
        }
        Err(WatchError::Failed(no_server(server)))
    }

    /// Waits until `server`'s connection for the repository at
    /// `checkout` changes, through the first source that has the
    /// server; with none, it never does.
    pub async fn changed(&self, checkout: &Path, server: &str) {
        for source in &self.sources() {
            if let Some(changed) = source.changed(checkout, server) {
                return changed.await;
            }
        }
        std::future::pending().await
    }
}

fn no_server(server: &str) -> String {
    format!("no MCP server `{server}` is set up for this repository")
}

impl std::fmt::Debug for OutsideResources {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let sources = self.0.lock().map_or(0, |sources| sources.len());
        write!(f, "OutsideResources({sources} sources)")
    }
}

impl std::fmt::Debug for OutsideTools {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let sources = self.0.lock().map_or(0, |sources| sources.len());
        write!(f, "OutsideTools({sources} sources)")
    }
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
        self.read_settings(plugin, None)
    }

    /// `plugin`'s settings for the repository `repo`: its copy when it
    /// keeps one (ADR 0029), else the plugin's own, else their default.
    pub fn settings_in<T: DeserializeOwned + Default>(
        &self,
        plugin: &str,
        repo: &str,
    ) -> T {
        self.read_settings(plugin, Some(repo))
    }

    fn read_settings<T: DeserializeOwned + Default>(
        &self,
        plugin: &str,
        repo: Option<&str>,
    ) -> T {
        self.services
            .get::<SavedSettings>()
            .and_then(|saved| saved.read(plugin, repo))
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default()
    }

    /// Saves `settings` as `plugin`'s, as its page would, and asks the
    /// interface to draw the catalog again, which carries them. Runs
    /// started from now on take them.
    pub async fn save_settings(
        &self,
        plugin: &str,
        settings: &impl Serialize,
    ) -> anyhow::Result<()> {
        self.write_settings(plugin, None, settings).await
    }

    /// Saves `settings` as the repository `repo`'s copy of `plugin`'s
    /// (ADR 0029), and asks the interface to draw the catalog again.
    pub async fn save_settings_in(
        &self,
        plugin: &str,
        repo: &str,
        settings: &impl Serialize,
    ) -> anyhow::Result<()> {
        self.write_settings(plugin, Some(repo), settings).await
    }

    async fn write_settings(
        &self,
        plugin: &str,
        repo: Option<&str>,
        settings: &impl Serialize,
    ) -> anyhow::Result<()> {
        let saved = self
            .services
            .get::<SavedSettings>()
            .ok_or_else(|| anyhow::anyhow!("This host saves no settings"))?;
        saved
            .save(plugin, repo, serde_json::to_value(settings)?)
            .await?;
        self.refresh();
        Ok(())
    }

    /// The host's conversations, when it lets a host half drive them
    /// (ADR 0037).
    pub fn conversations(&self) -> Option<Arc<dyn Conversations>> {
        self.services.get::<Arc<dyn Conversations>>().cloned()
    }

    /// The tools a host half may call outside any run, when the host
    /// keeps them.
    pub fn outside_tools(&self) -> Option<&OutsideTools> {
        self.services.get::<OutsideTools>()
    }

    /// The MCP resources a host half may read and watch outside any run,
    /// when the host keeps them.
    pub fn outside_resources(&self) -> Option<&OutsideResources> {
        self.services.get::<OutsideResources>()
    }

    /// The repositories no person attends, when the host keeps them.
    pub fn unattended(&self) -> Option<&Unattended> {
        self.services.get::<Unattended>()
    }

    /// The person's model as `plugin` asks it outside any run (ADR
    /// 0044): while they are signed in, and the host offers it.
    pub fn model(&self, plugin: &str) -> Option<crate::PluginModel> {
        self.services
            .get::<crate::Model>()
            .filter(|model| model.signed_in())
            .map(|model| model.of(plugin))
    }

    /// The jobs the host keeps for plugins (ADR 0044), when it keeps
    /// them.
    pub fn jobs(&self) -> Option<&crate::Jobs> {
        self.services.get::<crate::Jobs>()
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
        let store = runtime.block_on(tau_store_sqlite::memory()).unwrap();
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
            cx.runtime
                .block_on(cx.save_settings(plugin, value))
                .unwrap();
            expected[usize::from(*second)] = value.clone();
        }
        assert_eq!(cx.settings::<Vec<u32>>("a"), expected[0]);
        assert_eq!(cx.settings::<Vec<u32>>("b"), expected[1]);
        assert_eq!(cx.settings::<Vec<u32>>("c"), Vec::<u32>::new());
        // Another shape is the default, not an error.
        cx.runtime.block_on(cx.save_settings("c", &"text")).unwrap();
        assert_eq!(cx.settings::<Vec<u32>>("c"), Vec::<u32>::new());
        assert_eq!(pushes.lock().unwrap().len(), saves.len() + 1);
        assert_eq!(cx.config_dir(), Some(Path::new("/config/tau")));
    }

    /// A repository's copy of a plugin's settings is its own; one that
    /// keeps none reads the plugin's.
    #[test]
    fn a_repositorys_settings_fall_back_to_the_plugins() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let store = runtime.block_on(tau_store_sqlite::memory()).unwrap();
        let cx = HostCx::new(
            store,
            runtime.handle().clone(),
            Services::default().with(SavedSettings::in_memory()),
            PathBuf::from("/data/tau"),
            Vec::new(),
            Arc::new(|_| {}),
        );
        runtime.block_on(cx.save_settings("a", &1u32)).unwrap();
        runtime
            .block_on(cx.save_settings_in("a", "x", &2u32))
            .unwrap();
        assert_eq!(cx.settings::<u32>("a"), 1);
        assert_eq!(cx.settings_in::<u32>("a", "x"), 2);
        assert_eq!(cx.settings_in::<u32>("a", "y"), 1);
        assert_eq!(cx.settings_in::<u32>("b", "x"), 0);
        assert!(cx.conversations().is_none());
    }
}
