//! Runs real agents behind the workspace: the seams from
//! [`crate::workspace`] wired to `tau-agent`.
//!
//! The host owns a tokio runtime beside GPUI's executor. A new run starts
//! on that runtime; a task reads its events and sends them over a channel
//! the workspace drains on the UI thread. Steering and cancelling go the
//! other way through each run's [`RunControl`].
//!
//! Repositories come from GitHub alone: the host clones each one and
//! makes it a [`Project`] under `$XDG_DATA_HOME/tau/repos`, and
//! each run gets a jj workspace there ([`RunWorkspace`]) with a commit
//! per turn, so a fork starts from a turn's conversation and code. The
//! repositories tau lists, and which the sidebar shows open, are kept in
//! `repos.json`; each run records its repository ([`RepoTag`]), so past
//! runs come back from the store under theirs.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use futures_util::StreamExt;
use gpui::{App, AppContext as _, Entity};
use serde::{Deserialize, Serialize};
use tau_agent::{
    agent::{Agent, Checkpoint, RunControl},
    error::PluginError,
    event::{RunEvent, StopReason},
    limits::Limits,
    plugin::{Plugin, PluginCtx, PluginRun, RunPlan},
    tool::RunId,
};
use tau_ai::{
    chatgpt::AccountId,
    client::OpenAi,
    message::Message,
    model::find,
    refusal::Refusal,
};
use tau_compaction::Compaction;
use tau_constitution::{Constitution, ConstitutionPlugin, Live, RuleError};
use tau_jev::TypeSafe;
use tau_store::{Entry, RunKind, Status, Store, TurnUsage};
use tau_tools::{path::Root, plugin::CodingTools};
use tau_vcs::{
    ChangeKind,
    DEFAULT_WORKSPACE,
    Delegate,
    FileDiff,
    Identity,
    Landing,
    Link,
    Project,
    RunWorkspace,
    VcsPlugin,
    delegate::ChildModel,
    run_workspace::{PLUGIN as WORKSPACE_PLUGIN, bookmark},
};
use tokio::{runtime::Runtime, sync::mpsc};

use crate::{
    accounts::{self, Credentials},
    catalog::{
        Catalog,
        Constitution as CatalogConstitution,
        PluginInfo,
        PluginScreen,
        ProjectStatus,
        Repo,
        Rule as CatalogRule,
        Seam,
        StoreInfo,
    },
    github,
    memory::{Memories, stale_on_turn},
    models::{
        AccessInfo,
        DEFAULT_MODEL,
        Effort,
        ModelChoice,
        ModelSettings,
        Models,
        plan_models,
    },
    pull_request::{PrCommit, PrState, PullRequest},
    route::Route,
    setup::{CloneState, ModelAccess, RepoClone, SetupStep, SetupUpdate},
    update::HostUpdate,
    view::{
        BranchCode,
        ChildKind,
        ChildRun,
        CodeState,
        ContextWindow,
        FileChange,
        FileKind,
        FileStat,
        LANDING_RECORD,
        LandingRecord,
        Limits as ViewLimits,
        Origin,
        PlanField,
        PluginStatus,
        RunView,
        Stored,
        Tone,
        parse_diff,
    },
    workspace::{Workspace, WorkspaceEvent},
};

/// What the host needs to start.
#[derive(Debug, Clone)]
pub struct HostConfig {
    /// The ChatGPT account whose plan runs use at first; see
    /// [`Host::set_account`].
    pub account: AccountId,
    /// Where sign-ins and keys are kept.
    pub credentials: Credentials,
    /// The model runs use when nothing else is chosen; `None` takes
    /// [`DEFAULT_MODEL`].
    pub model: Option<String>,
    /// The run store, usually `$XDG_DATA_HOME/tau/runs.db`.
    pub store: PathBuf,
    /// Where projects live, usually `$XDG_DATA_HOME/tau/repos`.
    pub repos: PathBuf,
    /// The user's model choices, usually `$XDG_CONFIG_HOME/tau/models.json`.
    pub settings: PathBuf,
    /// The repositories tau lists, usually `$XDG_DATA_HOME/tau/repos.json`.
    pub repo_list: PathBuf,
}

impl HostConfig {
    /// The model runs use when nothing else is chosen.
    pub fn default_model(&self) -> String {
        self.model
            .clone()
            .unwrap_or_else(|| DEFAULT_MODEL.to_owned())
    }

    /// `$XDG_DATA_HOME/tau`, or `~/.local/share/tau`.
    pub fn data_dir() -> PathBuf {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(|home| PathBuf::from(home).join(".local/share"))
            })
            .unwrap_or_else(|| PathBuf::from("."))
            .join("tau")
    }

    pub fn default_store() -> PathBuf {
        Self::data_dir().join("runs.db")
    }

    pub fn default_repos() -> PathBuf {
        Self::data_dir().join("repos")
    }

    pub fn default_repo_list() -> PathBuf {
        Self::data_dir().join("repos.json")
    }

    /// `models.json` in tau's config directory.
    pub fn default_settings() -> PathBuf {
        Credentials::default_dir().dir.join("models.json")
    }

    /// `interface.json` in tau's config directory: the interface's own
    /// settings, such as `reduce_motion`.
    pub fn default_interface_settings() -> PathBuf {
        Credentials::default_dir().dir.join("interface.json")
    }

    /// The project directory for the clone at `path`: its name and a
    /// hash of its full path, so two clones with one name get two
    /// projects.
    pub fn project_dir_of(&self, path: &Path) -> PathBuf {
        let full = canonical(path);
        self.repos.join(format!(
            "{}-{:08x}",
            dir_name(&full),
            fnv(&full.to_string_lossy())
        ))
    }
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned())
}

/// A directory's last component, or `project`.
fn dir_name(path: &Path) -> String {
    path.file_name()
        .map_or("project".into(), |name| name.to_string_lossy().into_owned())
}

/// The repositories tau lists, as `repos.json` keeps them.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct RepoList {
    repos: Vec<Listed>,
    /// The ones the sidebar shows open.
    #[serde(default)]
    open: Vec<String>,
    /// Conversations closed: History lists them, the sidebar does not.
    #[serde(default)]
    closed: Vec<String>,
    /// Flagged calls looked at, as `[run, call id]`.
    #[serde(default)]
    reviewed: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Listed {
    name: String,
    path: PathBuf,
    /// Removed from tau: not listed, but its runs keep its name.
    #[serde(default)]
    hidden: bool,
    /// Cloned from GitHub, as `owner/name`: updates fetch from there.
    #[serde(default)]
    github: Option<String>,
    /// The id of its main chat, once made; see [`Host::main_of`].
    #[serde(default)]
    main: Option<String>,
}

impl RepoList {
    /// The list `path` keeps, without the local checkouts listed before
    /// repositories came from GitHub alone: they stay out, open or not.
    fn load(path: &Path) -> Self {
        let list: Self = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        list.github_only()
    }

    /// The list without repositories that did not come from GitHub, and
    /// with only the listed ones open.
    fn github_only(mut self) -> Self {
        self.repos.retain(|listed| listed.github.is_some());
        let listed: Vec<String> = self
            .repos
            .iter()
            .map(|listed| listed.name.clone())
            .collect();
        self.open.retain(|name| listed.contains(name));
        self
    }

    fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    /// Lists the clone at `path`, or lists it again if it was removed,
    /// and returns its name: the directory's, made unique.
    fn list(&mut self, path: &Path) -> String {
        let path = canonical(path);
        if let Some(listed) =
            self.repos.iter_mut().find(|listed| listed.path == path)
        {
            listed.hidden = false;
            return listed.name.clone();
        }
        let base = dir_name(&path);
        let taken = |name: &str| self.repos.iter().any(|l| l.name == name);
        let name = (1..)
            .map(|n| {
                if n == 1 {
                    base.clone()
                } else {
                    format!("{base}-{n}")
                }
            })
            .find(|name| !taken(name))
            .expect("some suffix is free");
        self.repos.push(Listed {
            name: name.clone(),
            path,
            hidden: false,
            github: None,
            main: None,
        });
        name
    }
}

/// The plugin name under which a run records its repository.
pub const REPO_PLUGIN: &str = "repo";

/// A repository's main chat's title.
pub const MAIN_TITLE: &str = "main";

/// `base` on `choice`: its model and effort (auto leaves it to a
/// plugin, such as tau-reasoning), and compaction by the model's window,
/// pruning with Jev first when there is a key.
fn for_model(
    base: Agent,
    choice: &ModelChoice,
    jev: Option<Arc<dyn tau_jev::Jev>>,
    archive_dir: &Path,
    repo: &str,
) -> Agent {
    let mut agent = base.model(&choice.model);
    if let Some(effort) = choice.effort.reasoning() {
        agent = agent.reasoning(effort);
    }
    // Compaction steps in by the run's own model's window.
    let mut compaction = Compaction::default();
    if let Some(model) = find(&choice.model) {
        compaction = compaction.context_window(model.context_window);
    }
    // Pruning with Jev first, when there is a key: it is cheaper than
    // a summary, and summarizing follows when pruning cannot help.
    if let Some(jev) = &jev {
        let settings = tau_fast_compaction::Settings {
            context_window: find(&choice.model)
                .map(|model| model.context_window),
            archive_dir: archive_dir.to_owned(),
            ..tau_fast_compaction::Settings::default()
        };
        agent = agent.plugin(
            tau_fast_compaction::FastCompaction::shared(jev.clone())
                .settings(settings),
        );
    }
    agent.plugin(compaction).plugin(RepoTag(repo.to_owned()))
}

/// The model and effort a sub-agent runs on: what its call asked for,
/// else its caller's. An effort the model does not take is refused;
/// the caller's, on another model that does not take it, goes to auto.
fn child_choice(
    caller: &ModelChoice,
    asked: &ChildModel,
) -> Result<ModelChoice, tau_agent::tool::ToolError> {
    let model = asked.model.clone().unwrap_or_else(|| caller.model.clone());
    let Some(effort) = asked.effort else {
        return Ok(ModelChoice::new(model, caller.effort).fitted());
    };
    let effort = Effort::of(effort);
    let offered = Effort::offered(&model);
    if !offered.contains(&effort) {
        let offered: Vec<&str> = offered.iter().map(|e| e.label()).collect();
        return Err(format!(
            "{model} does not take effort {}; it takes {}",
            effort.label(),
            offered.join(", ")
        )
        .into());
    }
    Ok(ModelChoice::new(model, effort))
}

/// Records the repository a run works on, so history can list the run
/// under it.
struct RepoTag(String);

#[async_trait]
impl Plugin for RepoTag {
    fn name(&self) -> &str {
        REPO_PLUGIN
    }

    async fn start(
        &self,
        _plan: &mut RunPlan,
        _ctx: &PluginCtx,
    ) -> Result<Box<dyn PluginRun>, PluginError> {
        Ok(Box::new(TagOnce {
            repo: self.0.clone(),
            done: false,
        }))
    }
}

struct TagOnce {
    repo: String,
    done: bool,
}

#[async_trait]
impl PluginRun for TagOnce {
    async fn on_event(&mut self, event: &RunEvent, ctx: &PluginCtx) {
        // At its first turn: the run is stored by then.
        let RunEvent::TurnStart { run, .. } = event else {
            return;
        };
        if self.done || run != &ctx.run {
            return;
        }
        let body = serde_json::json!({ "repo": self.repo });
        self.done = ctx.record(&body).await.is_ok();
    }
}

/// The repository a stored run recorded, if it did.
async fn stored_repo(store: &Store, run: &str) -> Option<String> {
    let entries = store.plugin_entries(run, REPO_PLUGIN).await.ok()?;
    entries.iter().find_map(|(_, body)| {
        serde_json::from_str::<serde_json::Value>(body)
            .ok()?
            .get("repo")?
            .as_str()
            .map(str::to_owned)
    })
}

/// A stable 32-bit FNV-1a hash, for directory names.
fn fnv(text: &str) -> u32 {
    text.bytes().fold(0x811c_9dc5, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    })
}

/// The project runs work in, which may still be importing. Anything
/// that needs it waits in [`ProjectSlot::wait`], off the UI thread where
/// it can.
struct ProjectSlot {
    state: Mutex<ProjectState>,
    done: std::sync::Condvar,
}

#[derive(Clone)]
enum ProjectState {
    Importing,
    Ready(Project),
    /// The clone could not be made a project, for this reason; runs
    /// cannot start in it.
    Failed(String),
}

impl ProjectSlot {
    fn new(state: ProjectState) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(state),
            done: std::sync::Condvar::new(),
        })
    }

    fn set(&self, state: ProjectState) {
        *self.state.lock().expect("not poisoned") = state;
        self.done.notify_all();
    }

    fn peek(&self) -> ProjectState {
        self.state.lock().expect("not poisoned").clone()
    }

    /// The project, once the import is over; `None` if it failed.
    fn wait(&self) -> Option<Project> {
        let mut state = self.state.lock().expect("not poisoned");
        while matches!(*state, ProjectState::Importing) {
            state = self.done.wait(state).expect("not poisoned");
        }
        match &*state {
            ProjectState::Ready(project) => Some(project.clone()),
            _ => None,
        }
    }
}

/// A listed repository: its clone, and the project runs in it work in.
#[derive(Clone)]
struct RepoSlot {
    name: String,
    path: PathBuf,
    project: Arc<ProjectSlot>,
}

impl RepoSlot {
    /// The project runs in the repository work in, once imported.
    fn project(&self) -> anyhow::Result<Project> {
        self.project
            .wait()
            .ok_or_else(|| match self.project.peek() {
                ProjectState::Failed(why) => {
                    anyhow::anyhow!(
                        "{} could not be imported: {why}",
                        self.name
                    )
                }
                _ => anyhow::anyhow!("{} has no project", self.name),
            })
    }
}

pub struct Host {
    runtime: Runtime,
    /// The agent every run starts from; each run adds its own tools.
    base: Mutex<Agent>,
    /// The ChatGPT account runs reach models with; `None` after
    /// signing out, or on an account without plan use.
    account: Mutex<Option<AccountId>>,
    /// The client `base` reaches models with, for what it learns on the
    /// way: why the ChatGPT plan refused a run. `None` for an agent built
    /// elsewhere, as in tests.
    client: Mutex<Option<OpenAi>>,
    /// Why OpenAI refused the eligibility check at the last sign-in
    /// because plan use is not available to the account
    /// (`Recovery::Restricted`): the status, code and request id. `None`
    /// otherwise.
    not_eligible: Arc<Mutex<Option<String>>>,
    github: github::Api,
    /// Jev for tau-constitution, in place of TypeSafe's with the saved
    /// key: for tests.
    jev: Option<Arc<dyn tau_jev::Jev>>,
    /// What every plugin's Jev requests did this session.
    jev_meter: Arc<Mutex<crate::metered::Meter>>,
    /// Each repository's notes and the user's, shared by every run.
    memories: Arc<Memories>,
    /// Each repository's rules as runs check them, by
    /// [`Host::constitution_key`]: an edit replaces them here, and every
    /// run's next check reads the new ones.
    constitutions: Mutex<HashMap<String, Live>>,
    store: Store,
    config: HostConfig,
    /// The repositories of this session, in the list's order. Removed
    /// ones stay here, unlisted, for their runs.
    repos: Arc<Mutex<Vec<RepoSlot>>>,
    list: Arc<Mutex<RepoList>>,
    /// The repository each run of this session works in.
    run_repos: Arc<Mutex<HashMap<RunId, String>>>,
    /// Repositories taking in new commits now.
    updating: Arc<Mutex<Vec<String>>>,
    /// What the latest update found.
    last_update: Arc<Mutex<Option<String>>>,
    /// Pull request drafts written from runs, until they are opened.
    drafts: Mutex<HashMap<RunId, PullRequest>>,
    /// Pull requests opened from runs, for pushing their later turns.
    prs: Arc<Mutex<HashMap<RunId, OpenPr>>>,
    runs: Arc<Mutex<HashMap<RunId, RunControl>>>,
    /// The workspace each run of this session works in.
    workspaces: Arc<Mutex<HashMap<RunId, String>>>,
    /// The model each run of this session runs on.
    choices: Arc<Mutex<HashMap<RunId, ModelChoice>>>,
    /// Runs whose `RunEnd` went by while their outcome is still being
    /// stored: they stop in a moment.
    ending: Arc<Mutex<HashSet<RunId>>>,
    /// The user's model choices, as loaded and last saved.
    settings: Arc<Mutex<ModelSettings>>,
    events: mpsc::UnboundedSender<RunEvent>,
    /// The plugins with their UI, each with its state on this host (ADR
    /// 0017).
    hosted: Vec<hosted::Hosted>,
    /// What plugins' host halves tell the interface, and its receiving
    /// end until [`Self::attach`] takes it.
    pushes: mpsc::UnboundedSender<tau_ui_plugin::Push>,
    pushed: Mutex<Option<mpsc::UnboundedReceiver<tau_ui_plugin::Push>>>,
}

#[path = "host_plugins.rs"]
mod hosted;

const MAX_TURNS: u32 = 50;

/// How many past runs history shows.
const HISTORY: u32 = 50;

/// The agent runs start from, reaching models on `account`'s ChatGPT
/// plan, and the client it reaches them through.
fn coder(
    runtime: &Runtime,
    account: &AccountId,
    credentials: &Credentials,
    model: &str,
) -> anyhow::Result<(Agent, OpenAi)> {
    // Clients must be created inside the runtime.
    let _guard = runtime.enter();
    let client = OpenAi::chatgpt(credentials.chatgpt()?, account.clone());
    let agent = Agent::new(client.clone())
        .name("coder")
        .model(model)
        .instructions(INSTRUCTIONS)
        // Plugins that hold a stop cap themselves: the constitution by
        // its holds, a goal by its continuations.
        .limits(
            Limits::default()
                .max_turns(MAX_TURNS)
                .max_continuations(u32::MAX),
        );
    Ok((agent, client))
}

const INSTRUCTIONS: &str = "You are tau, a coding agent working in the \
    user's repository. Use the tools to read and change files and to run \
    commands, and the vcs tools for version control, never `git` or `jj` \
    in bash. Nothing is committed for you: your commits are how your work \
    is reviewed and landed, so commit with `vcs_commit` wherever a \
    reviewer would want a boundary, each with a Conventional Commits \
    message, and commit everything before you finish. Be concise, and say \
    which tests you ran.";

fn identity() -> Identity {
    Identity {
        name: "tau".into(),
        email: "tau@localhost".into(),
    }
}

impl Host {
    /// Opens the store and the project and builds the agent. Returns the
    /// receiving end of the event channel for [`Host::attach`].
    pub fn new(
        config: HostConfig,
    ) -> anyhow::Result<(Self, mpsc::UnboundedReceiver<RunEvent>)> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("tau-host")
            .build()?;
        if let Some(parent) = config.store.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let store = runtime.block_on(Store::open(&config.store))?;
        let (agent, client) = coder(
            &runtime,
            &config.account,
            &config.credentials,
            &config.default_model(),
        )?;
        let (mut host, events) =
            Self::with_agent(runtime, agent, store, config);
        host.client = Mutex::new(Some(client));
        // The app searches memory with docbert's model; hosts built
        // elsewhere, as in tests, by keywords alone.
        host.memories = Arc::new(Memories::semantic());
        // Importing clones can take a while; the window opens first.
        let listed: Vec<Listed> = host
            .list
            .lock()
            .expect("not poisoned")
            .repos
            .iter()
            .filter(|listed| !listed.hidden)
            .cloned()
            .collect();
        let mut slots = Vec::new();
        for listed in &listed {
            let slot = RepoSlot {
                name: listed.name.clone(),
                path: listed.path.clone(),
                project: ProjectSlot::new(ProjectState::Importing),
            };
            host.spawn_import(&slot)?;
            slots.push(slot);
        }
        *host.repos.lock().expect("not poisoned") = slots;
        for listed in &listed {
            host.main_of(&listed.name)?;
        }
        Ok((host, events))
    }

    /// Opens or copies a repository's project on a thread of its own.
    fn spawn_import(&self, slot: &RepoSlot) -> anyhow::Result<()> {
        let project = slot.project.clone();
        let source = slot.path.to_string_lossy().into_owned();
        let dir = self.config.project_dir_of(&slot.path);
        std::thread::Builder::new()
            .name("tau-import".into())
            .spawn(move || {
                project.set(
                    match Project::open_or_import(&source, dir, identity()) {
                        Ok(project) => ProjectState::Ready(project),
                        Err(error) => {
                            eprintln!(
                                "tau-ui: cannot import {source}: {error:#}"
                            );
                            ProjectState::Failed(format!("{error:#}"))
                        }
                    },
                );
            })?;
        Ok(())
    }

    /// A host over an agent built elsewhere: another model, other
    /// plugins, or a scripted model in tests. It lists no repository
    /// until one is cloned, or given with [`Self::with_repo`].
    pub fn with_agent(
        runtime: Runtime,
        agent: Agent,
        store: Store,
        config: HostConfig,
    ) -> (Self, mpsc::UnboundedReceiver<RunEvent>) {
        let (events, receiver) = mpsc::unbounded_channel();
        let (pushes, pushed) = mpsc::unbounded_channel();
        let settings = load_settings(&config.settings, &config.default_model());
        let list = RepoList::load(&config.repo_list);
        let mut host = Self {
            runtime,
            base: Mutex::new(agent),
            account: Mutex::new(Some(config.account.clone())),
            client: Mutex::new(None),
            not_eligible: Arc::default(),
            github: github::Api::default(),
            jev: None,
            jev_meter: Arc::default(),
            memories: Arc::new(Memories::keywords()),
            constitutions: Mutex::default(),
            store,
            choices: Arc::default(),
            settings: Arc::new(Mutex::new(settings)),
            config,
            repos: Arc::default(),
            list: Arc::new(Mutex::new(list)),
            run_repos: Arc::default(),
            updating: Arc::default(),
            last_update: Arc::default(),
            drafts: Mutex::default(),
            prs: Arc::default(),
            ending: Arc::default(),
            runs: Arc::default(),
            workspaces: Arc::default(),
            events,
            hosted: Vec::new(),
            pushes,
            pushed: Mutex::new(Some(pushed)),
        };
        host.host_plugins();
        (host, receiver)
    }

    /// Lists `project` as the repository `name`, in place of any listed
    /// under that name, for a host built elsewhere: runs in it get a
    /// workspace each there, as in a clone from GitHub.
    pub fn with_repo(self, name: &str, project: Project) -> Self {
        let path = project.root().to_owned();
        {
            let mut list = self.list.lock().expect("not poisoned");
            let main = list
                .repos
                .iter()
                .find(|listed| listed.name == name)
                .and_then(|listed| listed.main.clone());
            list.repos.retain(|listed| listed.name != name);
            list.repos.push(Listed {
                name: name.to_owned(),
                path: path.clone(),
                hidden: false,
                github: None,
                main,
            });
        }
        let mut repos = self.repos.lock().expect("not poisoned");
        repos.retain(|slot| slot.name != name);
        repos.push(RepoSlot {
            name: name.to_owned(),
            path,
            project: ProjectSlot::new(ProjectState::Ready(project)),
        });
        drop(repos);
        if let Err(error) = self.main_of(name) {
            eprintln!("tau-ui: cannot make {name}'s main chat: {error:#}");
        }
        self
    }

    /// The main chat of the listed repository `repo`, made the first
    /// time it is asked for: a run that starts empty and finished, which
    /// a message resumes. Every other chat in the repository is a fork
    /// of it, and it cannot be closed.
    pub fn main_of(&self, repo: &str) -> anyhow::Result<RunId> {
        let listed = {
            let list = self.list.lock().expect("not poisoned");
            let listed =
                list.repos
                    .iter()
                    .find(|listed| listed.name == repo)
                    .ok_or_else(|| anyhow::anyhow!("No repository {repo}"))?;
            listed.main.clone()
        };
        if let Some(id) = listed
            && self.runtime.block_on(self.store.run(&id))?.is_some()
        {
            return Ok(RunId(id.into()));
        }
        let id = uuid::Uuid::now_v7().to_string();
        self.runtime.block_on(async {
            self.store
                .create_run(&tau_store::NewRun {
                    id: &id,
                    workflow_id: None,
                    agent: "coder",
                    kind: RunKind::Root,
                    model: &self.config.default_model(),
                    turns: 0,
                })
                .await?;
            // Tagged with its repository, as a run's first turn would.
            let tag = Entry::Plugin {
                plugin: REPO_PLUGIN.to_owned(),
                body: serde_json::json!({ "repo": repo }).to_string(),
            };
            self.store
                .append_turn(&id, &[tag], TurnUsage::default())
                .await?;
            self.store.set_title(&id, MAIN_TITLE).await?;
            self.store.finish_run(&id, Status::Done, None, None).await
        })?;
        let mut list = self.list.lock().expect("not poisoned");
        if let Some(listed) =
            list.repos.iter_mut().find(|listed| listed.name == repo)
        {
            listed.main = Some(id.clone());
        }
        list.save(&self.config.repo_list)?;
        Ok(RunId(id.into()))
    }

    /// The main chats of the listed repositories.
    fn mains(&self) -> Vec<String> {
        self.list
            .lock()
            .expect("not poisoned")
            .repos
            .iter()
            .filter(|listed| !listed.hidden)
            .filter_map(|listed| listed.main.clone())
            .collect()
    }

    /// Whether `run` is a repository's main chat.
    fn is_main(&self, run: &RunId) -> bool {
        self.list
            .lock()
            .expect("not poisoned")
            .repos
            .iter()
            .any(|listed| listed.main.as_deref() == Some(&*run.0))
    }

    /// The bookmark `run`'s commits move: trunk's for a main chat, which
    /// commits on it, else `tau/<run>`.
    fn bookmark_of(
        &self,
        run: &RunId,
        project: &Project,
    ) -> anyhow::Result<String> {
        if self.is_main(run) {
            return Ok(project.trunk_name()?);
        }
        Ok(bookmark(run))
    }

    /// Brings a main chat's workspace, `name`, up to trunk, which moves
    /// without it on an update from GitHub: its work in `@` goes onto
    /// trunk's head, so its next commit moves trunk forward, not aside.
    fn catch_up(&self, project: &Project, name: &str) -> anyhow::Result<()> {
        let exists = name == DEFAULT_WORKSPACE
            || project.workspaces()?.iter().any(|known| known == name);
        if !exists {
            return Ok(());
        }
        let vcs = tau_vcs::Vcs::open(project.workspace_dir(name), identity())?;
        self.runtime.block_on(vcs.move_onto(
            project.trunk()?,
            project.trunk_name()?,
            true,
        ))?;
        Ok(())
    }

    fn slot(&self, name: &str) -> Option<RepoSlot> {
        self.repos
            .lock()
            .expect("not poisoned")
            .iter()
            .find(|slot| slot.name == name)
            .cloned()
    }

    /// The repository `run` works in: as this session started it, or as
    /// the store recorded it.
    fn slot_of_run(&self, run: &RunId) -> anyhow::Result<RepoSlot> {
        let known = self
            .run_repos
            .lock()
            .expect("not poisoned")
            .get(run)
            .cloned();
        let name = known.or_else(|| {
            self.runtime.block_on(stored_repo(&self.store, &run.0))
        });
        name.and_then(|name| self.slot(&name)).ok_or_else(|| {
            anyhow::anyhow!("{} works in no listed repository", run.0)
        })
    }

    /// A listed repository's project, waiting for its import.
    pub fn project_of(&self, repo: &str) -> Option<Project> {
        self.slot(repo)?.project.wait()
    }

    /// Where sign-ins and keys are kept.
    pub fn credentials(&self) -> &Credentials {
        &self.config.credentials
    }

    /// Whether a repository is still being imported.
    pub fn is_importing(&self) -> bool {
        self.repos
            .lock()
            .expect("not poisoned")
            .iter()
            .any(|slot| matches!(slot.project.peek(), ProjectState::Importing))
    }

    /// Lists the clone of `full_name` at `dir` and starts importing it.
    /// Returns it as the sidebar shows it.
    fn list_clone(&self, dir: &Path, full_name: &str) -> anyhow::Result<Repo> {
        let name = {
            let mut list = self.list.lock().expect("not poisoned");
            let name = list.list(dir);
            if let Some(listed) =
                list.repos.iter_mut().find(|listed| listed.name == name)
            {
                listed.github = Some(full_name.to_owned());
            }
            list.save(&self.config.repo_list)?;
            name
        };
        if self.slot(&name).is_none() {
            let slot = RepoSlot {
                name: name.clone(),
                path: canonical(dir),
                project: ProjectSlot::new(ProjectState::Importing),
            };
            self.spawn_import(&slot)?;
            self.repos.lock().expect("not poisoned").push(slot);
        }
        let mut repo = Repo::new(&name, canonical(dir).display().to_string());
        repo.main = Some(self.main_of(&name)?);
        Ok(repo)
    }

    /// Checks constitutions with `jev` instead of TypeSafe's, for tests.
    pub fn with_jev(mut self, jev: Arc<dyn tau_jev::Jev>) -> Self {
        self.jev = Some(jev);
        self
    }

    /// Reaches GitHub at `api`, for tests.
    pub fn with_github(mut self, api: github::Api) -> Self {
        self.github = api;
        self
    }

    /// Clones `full_name` (`owner/name`) from GitHub with the saved
    /// sign-in, unless it was cloned before, and lists it. Blocks for
    /// the clone; the import goes on in the background.
    pub fn clone_github(&self, full_name: &str) -> anyhow::Result<Repo> {
        let token = github::Token::load(&self.config.credentials)
            .ok_or_else(|| anyhow::anyhow!("Sign in to GitHub first"))?;
        let (owner, name) = full_name
            .split_once('/')
            .filter(|(owner, name)| {
                [owner, name].iter().all(|part| {
                    !part.is_empty()
                        && !part.starts_with('.')
                        && !part.contains('/')
                })
            })
            .ok_or_else(|| anyhow::anyhow!("{full_name} is not owner/name"))?;
        let dir = self.config.repos.join("github").join(owner).join(name);
        if !dir.exists() {
            tau_vcs::clone_bare(
                &self.github.clone_url(full_name),
                Some(&token.token),
                &dir,
            )?;
        }
        self.list_clone(&dir, full_name)
    }

    /// Brings new commits from GitHub into a repository's project. New
    /// runs start from the new trunk; runs going on keep their code.
    /// Blocks.
    pub fn update_repo(&self, name: &str) -> anyhow::Result<tau_vcs::Updated> {
        let slot = self
            .slot(name)
            .ok_or_else(|| anyhow::anyhow!("No repository {name}"))?;
        let project = slot.project()?;
        let full_name = self.github_of(name).ok_or_else(|| {
            anyhow::anyhow!("{name} was not cloned from GitHub")
        })?;
        let token = github::Token::load(&self.config.credentials);
        Ok(project.update(tau_vcs::UpdateFrom::Remote {
            url: &self.github.clone_url(&full_name),
            token: token.as_ref().map(|token| token.token.as_str()),
        })?)
    }

    /// The `owner/name` a repository was cloned from, if it came from
    /// GitHub.
    fn github_of(&self, name: &str) -> Option<String> {
        self.list
            .lock()
            .expect("not poisoned")
            .repos
            .iter()
            .find(|listed| listed.name == name)
            .and_then(|listed| listed.github.clone())
    }

    /// Stops listing a repository. Its project and runs stay.
    pub fn hide_repo(&self, name: &str) -> anyhow::Result<()> {
        let mut list = self.list.lock().expect("not poisoned");
        for listed in &mut list.repos {
            if listed.name == name {
                listed.hidden = true;
            }
        }
        list.open.retain(|open| open != name);
        list.save(&self.config.repo_list)
    }

    /// Closes a conversation, or opens it again, for the sidebar.
    pub fn set_closed(&self, run: &RunId, closed: bool) -> anyhow::Result<()> {
        if closed && self.is_main(run) {
            anyhow::bail!("A repository's main chat stays open");
        }
        let mut list = self.list.lock().expect("not poisoned");
        list.closed.retain(|id| **id != *run.0);
        if closed {
            list.closed.push(run.0.to_string());
        }
        list.save(&self.config.repo_list)
    }

    /// Remembers that a flagged call was looked at.
    pub fn set_reviewed(
        &self,
        run: &RunId,
        call_id: &str,
    ) -> anyhow::Result<()> {
        let mut list = self.list.lock().expect("not poisoned");
        let entry = (run.0.to_string(), call_id.to_owned());
        if !list.reviewed.contains(&entry) {
            list.reviewed.push(entry);
        }
        list.save(&self.config.repo_list)
    }

    /// Asks Jev what a rule being written makes of past `calls` (tool,
    /// arguments) and final `answers`, as a check would ask. Blocks.
    pub fn try_rule(
        &self,
        text: &str,
        on: &[String],
        review: f64,
        block: f64,
        calls: &[(String, serde_json::Value)],
        answers: &[String],
    ) -> Result<(Vec<tau_constitution::Trial>, f64), String> {
        let on = on
            .iter()
            .map(|place| tau_constitution::rules::Target::parse(place))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        let rule = tau_constitution::Rule {
            id: "new".into(),
            text: text.to_owned(),
            on,
            review,
            block,
        };
        let jev = self.jev().ok_or_else(|| {
            "Trying a rule asks Jev: add a TypeSafe key on the Models screen."
                .to_owned()
        })?;
        self.runtime
            .block_on(tau_constitution::try_rule(&*jev, &rule, calls, answers))
    }

    /// `plugin`'s records for `run`, along its fork chain, as stored.
    pub fn plugin_records(
        &self,
        run: &RunId,
        plugin: &str,
    ) -> Vec<serde_json::Value> {
        self.runtime
            .block_on(self.store.records(&run.0, plugin))
            .unwrap_or_default()
            .iter()
            .filter_map(|body| serde_json::from_str(body).ok())
            .collect()
    }

    /// Remembers which repositories the sidebar shows open.
    pub fn set_open_repos(&self, open: Vec<String>) -> anyhow::Result<()> {
        let mut list = self.list.lock().expect("not poisoned");
        list.open = open;
        list.save(&self.config.repo_list)
    }

    /// Whether `run` is still going.
    /// Waits for a run whose `RunEnd` went by to finish storing its
    /// outcome, a moment at most. A run still working is not waited on.
    fn settle(&self, run: &RunId) {
        for _ in 0..500 {
            if !self.ending.lock().expect("not poisoned").contains(run)
                || !self.is_running(run)
            {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    pub fn is_running(&self, run: &RunId) -> bool {
        self.runs.lock().expect("not poisoned").contains_key(run)
    }

    /// The workspace `run` works in, if it started in this session.
    pub fn workspace(&self, run: &RunId) -> Option<PathBuf> {
        let name = self
            .workspaces
            .lock()
            .expect("not poisoned")
            .get(run)?
            .clone();
        Some(
            self.slot_of_run(run)
                .ok()?
                .project
                .wait()?
                .workspace_dir(&name),
        )
    }

    /// What the workspace shows beyond runs: the agent's plugins and the
    /// store.
    pub fn catalog(&self) -> Catalog {
        let mut plugins = vec![
            PluginInfo {
                name: "tau-tools".into(),
                description: "read bash edit write grep find ls".into(),
                seams: vec![Seam::Tools],
                spend: 0.0,
                screen: None,
                page: None,
            },
            PluginInfo {
                name: "tau-vcs".into(),
                description: "status diff log show describe commit new \
                              restore undo, on the run's workspace"
                    .into(),
                seams: vec![Seam::Tools],
                spend: 0.0,
                screen: None,
                page: None,
            },
            PluginInfo {
                name: "workspace".into(),
                description: "A jj workspace per run, and a commit per \
                              turn to fork from"
                    .into(),
                seams: vec![Seam::Start],
                spend: 0.0,
                screen: None,
                page: None,
            },
        ];
        let jev = self.jev().is_some();
        plugins.push(PluginInfo {
            name: tau_fast_compaction::NAME.into(),
            description: needs_jev(
                jev,
                "Prunes large bash outputs as they arrive, and stale tool \
                 history",
            ),
            seams: vec![Seam::Start, Seam::Rewrite],
            spend: 0.0,
            screen: Some(PluginScreen::Ledger),
            page: None,
        });
        plugins.push(PluginInfo {
            name: "tau-compaction".into(),
            description: "Summarizes the context when it nears the window"
                .into(),
            seams: vec![Seam::Start, Seam::Rewrite],
            spend: 0.0,
            screen: None,
            page: None,
        });
        let source = self.access_label();
        let slots = self.repos.lock().expect("not poisoned").clone();
        let list = self.list.lock().expect("not poisoned").clone();
        // An import going on shows first, then an update, then a failed
        // import.
        let importing = slots.iter().find(|slot| {
            matches!(slot.project.peek(), ProjectState::Importing)
        });
        let updating =
            self.updating.lock().expect("not poisoned").first().cloned();
        let failed = slots.iter().find(|slot| {
            matches!(slot.project.peek(), ProjectState::Failed(_))
        });
        let project = match (importing, updating, failed) {
            (Some(slot), _, _) => ProjectStatus::Importing(slot.name.clone()),
            (None, Some(name), _) => ProjectStatus::Updating(name),
            (None, None, Some(slot)) => {
                ProjectStatus::Failed(slot.name.clone())
            }
            (None, None, None) => ProjectStatus::Unknown,
        };
        let mut history = self.constitution_history();
        // In the list's order, which adding a repository again keeps.
        let repos: Vec<Repo> = list
            .repos
            .iter()
            .filter(|listed| !listed.hidden)
            .filter_map(|listed| {
                let slot =
                    slots.iter().find(|slot| slot.name == listed.name)?;
                let mut repo =
                    Repo::new(&listed.name, listed.path.display().to_string());
                repo.main =
                    listed.main.as_deref().map(|main| RunId(main.into()));
                repo.constitution = self.repo_constitution(slot);
                repo.plugins = self.registered_repo_data(slot);
                repo.constitution.history =
                    history.remove(&listed.name).unwrap_or_default();
                repo.memory = self.memories.catalog(&self.memory_dir(slot));
                Some(repo)
            })
            .collect();
        let rules: usize =
            repos.iter().map(|repo| repo.constitution.rules.len()).sum();
        plugins.push(PluginInfo {
            name: tau_constitution::NAME.into(),
            description: if jev {
                format!(
                    "{rules} rules across your repositories, checked with Jev"
                )
            } else {
                needs_jev(false, "Checks calls against each repository's rules")
            },
            seams: vec![Seam::BeforeTool, Seam::BeforeStop],
            spend: 0.0,
            screen: Some(PluginScreen::Constitution),
            page: None,
        });
        plugins.push(PluginInfo {
            name: tau_memory::plugin::NAME.into(),
            description: "Linked notes each repository's runs keep, and \
                          yours across them; searched at the start of a run"
                .into(),
            seams: vec![
                Seam::Start,
                Seam::Tools,
                Seam::AfterTool,
                Seam::Rewrite,
            ],
            spend: 0.0,
            screen: Some(PluginScreen::Memory),
            page: None,
        });
        // The plugins with their UI, with their data and settings.
        let (registered, plugin_data, plugin_settings) =
            self.registered_catalog();
        plugins.extend(registered);
        // What each plugin cost over the runs of the last 30 days.
        let spend = self
            .runtime
            .block_on(self.store.plugin_spend(&days_ago(SPEND_DAYS)))
            .unwrap_or_default();
        for plugin in &mut plugins {
            plugin.spend = spend
                .iter()
                .find(|(name, _)| *name == plugin.name)
                .map_or(0.0, |(_, usd)| *usd);
        }
        Catalog {
            plugin_data,
            plugin_settings,
            agent: "coder".into(),
            agent_source: Some(source.to_owned()),
            plugins,
            jev: self.jev_stats(),
            repos,
            open_repos: list.open,
            closed_runs: list
                .closed
                .iter()
                .map(|id| RunId(id.as_str().into()))
                .collect(),
            reviewed: list
                .reviewed
                .iter()
                .map(|(run, call)| (RunId(run.as_str().into()), call.clone()))
                .collect(),
            store: StoreInfo {
                path: self.config.store.display().to_string(),
                size: std::fs::metadata(&self.config.store)
                    .map(|meta| format!("{:.1} MB", meta.len() as f64 / 1e6))
                    .unwrap_or_default(),
                sample_query:
                    "select agent, sum(cost_usd) from runs group by agent"
                        .into(),
            },
            pull_requests: github::Token::load(&self.config.credentials)
                .is_some(),
            project,
            update: self.last_update.lock().expect("not poisoned").clone(),
            models: self.models(),
        }
    }

    /// The agent for one run, with its tools on the run's workspace, and
    /// the workspace's name.
    ///
    /// `main`: the run is its repository's main chat, which commits on
    /// trunk and alone gets `delegate`, as runs nest one level (ADR
    /// 0016).
    fn agent_for_run(
        &self,
        choice: &ModelChoice,
        repo: &RepoSlot,
        name: String,
        main: bool,
    ) -> anyhow::Result<(Agent, String)> {
        if self.account().is_none() {
            anyhow::bail!(
                "tau has no ChatGPT plan to run on. Sign in with ChatGPT and \
                 enable plan use on the Models screen."
            );
        }
        let jev = self.jev();
        // What hangs on the model: its effort, and compaction by its
        // window. A sub-agent can run on another model than its caller.
        let for_model = {
            let base = self.base.lock().expect("not poisoned").clone();
            let jev = jev.clone();
            let archive_dir = self.archive_dir(repo);
            let repo = repo.name.clone();
            move |choice: &ModelChoice| {
                for_model(
                    base.clone(),
                    choice,
                    jev.clone(),
                    &archive_dir,
                    &repo,
                )
            }
        };
        // The plugins with their UI, after the rest: tau-goal's hold of a
        // stop comes after the constitution's.
        let registered = self.registered(repo);
        let agent = for_model(choice);
        // The repository's rules, checked with Jev when there is a key.
        // Rules that cannot be read fail the run: they are never skipped.
        let constitution = match jev {
            Some(jev) => {
                Some(ConstitutionPlugin::live(jev, self.constitution(repo)?))
            }
            None => None,
        };
        let memory = self.memory_plugin(repo);
        let project = repo.project()?;
        // A run and its sub-agents work the same way, each in its own
        // workspace: tools, memory and the repository's rules. Only the
        // run itself keeps the conversation's goal and can delegate, so
        // sub-agents do not nest.
        let on_workspace = {
            let memory = memory.clone();
            let constitution = constitution.clone();
            // `lands`: the run proposes its own landing with `vcs_land`
            // (ADR 0014). Sub-agents land as they return, without it.
            move |agent: Agent, workspace: RunWorkspace, lands: bool| {
                // Notes about the files a turn changed may be stale.
                let workspace = match &memory {
                    Some(memory) => {
                        workspace.on_turn(stale_on_turn(memory.clone()))
                    }
                    None => workspace,
                };
                let vcs = VcsPlugin::new(workspace.vcs().clone());
                let vcs = if lands { vcs.landing() } else { vcs };
                let agent = agent
                    .plugin(CodingTools::new(Root::new(workspace.dir())))
                    .plugin(vcs)
                    .plugin(workspace);
                let agent = with_plugin(agent, memory.clone());
                with_plugin(agent, constitution.clone())
            }
        };
        let workspace = RunWorkspace::new(project.clone(), &name, identity())?;
        // A main chat commits on trunk: it has nothing to land.
        let workspace = if main {
            workspace.commits_to(project.trunk_name()?)
        } else {
            workspace
        };
        let delegate = {
            let on_workspace = on_workspace.clone();
            let registered = registered.clone();
            let caller = choice.clone();
            let models: Vec<String> =
                plan_models().into_iter().map(|model| model.id).collect();
            Delegate::new(
                workspace.clone(),
                identity(),
                &models,
                move |child, asked| {
                    let choice = child_choice(&caller, asked)?;
                    Ok(registered(
                        on_workspace(for_model(&choice), child, false),
                        tau_ui_plugin::RunKind::SubAgent,
                        &choice,
                    ))
                },
            )
        };
        let agent = if main { agent.tool(delegate) } else { agent };
        let agent = on_workspace(agent, workspace, !main);
        let kind = if main {
            tau_ui_plugin::RunKind::Main
        } else {
            tau_ui_plugin::RunKind::Chat
        };
        Ok((registered(agent, kind, choice), name))
    }

    /// Jev, when there is a TypeSafe key (or one given for tests).
    fn jev(&self) -> Option<Arc<dyn tau_jev::Jev>> {
        let inner = self.jev.clone().or_else(|| {
            self.config.credentials.jev_key().map(|key| {
                Arc::new(TypeSafe::new(key)) as Arc<dyn tau_jev::Jev>
            })
        })?;
        Some(Arc::new(crate::metered::Metered::new(
            inner,
            self.jev_meter.clone(),
        )))
    }

    /// What Jev did this session, for the Plugins screen, when there is
    /// a key.
    fn jev_stats(&self) -> Option<crate::catalog::JevStats> {
        if self.jev.is_none() && self.config.credentials.jev_key().is_none() {
            return None;
        }
        let meter = self.jev_meter.lock().expect("not poisoned").clone();
        Some(crate::catalog::JevStats {
            latency_p50_ms: meter.latency_p50_ms(),
            model: meter.model.unwrap_or_else(|| tau_jev::DEFAULT_MODEL.into()),
            key_env: "typesafe-key, set on Models".into(),
            price: format!("${} / M input", tau_jev::PRICE_PER_MILLION_INPUT),
            requests: meter.requests,
            input_tokens: meter.input_tokens,
            spent: meter.spent,
            failed: meter.failed,
        })
    }

    /// Where fast compaction archives what it prunes, for the model to
    /// read back: in tau's directory for the repository, only the user
    /// can open it, so archives outlive the temporary directory's
    /// cleanups and stay private. Made on first use; one that cannot be
    /// made fails only the archiving, which leaves the output whole.
    fn archive_dir(&self, repo: &RepoSlot) -> PathBuf {
        use std::os::unix::fs::DirBuilderExt as _;
        let dir = self.config.project_dir_of(&repo.path).join("archive");
        let _ = std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir);
        dir
    }

    /// Where a repository's memory notes are kept: in tau's directory for
    /// it, beside its constitution, out of the repository's history.
    fn memory_dir(&self, repo: &RepoSlot) -> PathBuf {
        self.config.project_dir_of(&repo.path).join("memory")
    }

    /// Where the user's notes, shared by every repository, are kept.
    fn user_memory_dir(&self) -> PathBuf {
        self.config
            .repos
            .parent()
            .unwrap_or(&self.config.repos)
            .join("memory")
    }

    /// The memory plugin for a run in `repo`, or none when its notes
    /// cannot be opened: the run goes on without memory.
    fn memory_plugin(
        &self,
        repo: &RepoSlot,
    ) -> Option<tau_memory::MemoryPlugin> {
        self.memories
            .plugin(&self.memory_dir(repo), &self.user_memory_dir())
            .inspect_err(|error| {
                eprintln!("tau-ui: memory is off for this run: {error:#}");
            })
            .ok()
    }

    /// What a repository's constitution is stored under: its clone, as
    /// the repository list keeps it.
    fn constitution_key(repo: &RepoSlot) -> String {
        canonical(&repo.path).display().to_string()
    }

    /// Repository `repo`'s rules as runs check them, read from the store
    /// the first time.
    fn constitution(&self, repo: &RepoSlot) -> anyhow::Result<Live> {
        let key = Self::constitution_key(repo);
        let mut open = self.constitutions.lock().expect("not poisoned");
        if let Some(live) = open.get(&key) {
            return Ok(live.clone());
        }
        let loaded = self
            .runtime
            .block_on(Constitution::load(&self.store, &key))?;
        let live = Live::new(loaded);
        open.insert(key, live.clone());
        Ok(live)
    }

    /// The constitution of repository `name`, for its screen.
    fn repo_constitution(&self, slot: &RepoSlot) -> CatalogConstitution {
        let (loaded, error) = match self.constitution(slot) {
            Ok(live) => (live.get(), None),
            Err(error) => (
                Arc::new(Constitution::default()),
                Some(format!("{error:#}")),
            ),
        };
        CatalogConstitution {
            rules: loaded
                .rules
                .iter()
                .map(|rule| CatalogRule {
                    id: rule.id.clone(),
                    text: rule.text.clone(),
                    applies_to: rule.on.iter().map(|on| on.label()).collect(),
                    review: rule.review as f32,
                    block: rule.block as f32,
                })
                .collect(),
            max_holds: loaded.max_holds,
            blocks_unchecked: loaded.on_error
                == tau_constitution::OnError::Block,
            error,
            history: Vec::new(),
        }
    }

    /// What the constitution did in each stored run, by repository: its
    /// records, counted the way a run's view counts them.
    fn constitution_history(
        &self,
    ) -> HashMap<String, Vec<(RunId, crate::view::ConstitutionStats)>> {
        let (records, repos) = self.runtime.block_on(async {
            (
                self.store
                    .plugin_entries_everywhere(tau_constitution::NAME)
                    .await,
                self.store.plugin_entries_everywhere(REPO_PLUGIN).await,
            )
        });
        let (Ok(records), Ok(repos)) = (records, repos) else {
            return HashMap::new();
        };
        let repo_of: HashMap<String, String> = repos
            .into_iter()
            .filter_map(|(run, body)| {
                let body: serde_json::Value =
                    serde_json::from_str(&body).ok()?;
                Some((run, body.get("repo")?.as_str()?.to_owned()))
            })
            .collect();
        let mut history: HashMap<
            String,
            Vec<(RunId, crate::view::ConstitutionStats)>,
        > = HashMap::new();
        for (run, body) in records {
            let (Some(repo), Ok(body)) = (
                repo_of.get(&run),
                serde_json::from_str::<serde_json::Value>(&body),
            ) else {
                continue;
            };
            let runs = history.entry(repo.clone()).or_default();
            // Records come by run, so a run's are together.
            if runs.last().is_none_or(|(last, _)| *last.0 != *run) {
                runs.push((RunId(run.into()), Default::default()));
            }
            if let Some((_, stats)) = runs.last_mut() {
                stats.add(&body);
            }
        }
        history
    }

    /// Changes a repository's constitution with `edit`, which checks
    /// what it adds, and saves it. Runs going on check with the new rules
    /// from their next tool call.
    pub fn edit_rules(
        &self,
        repo: &str,
        edit: impl FnOnce(&mut Constitution) -> Result<(), RuleError>,
    ) -> anyhow::Result<()> {
        let slot = self
            .slot(repo)
            .ok_or_else(|| anyhow::anyhow!("No repository {repo}"))?;
        let live = self.constitution(&slot)?;
        let mut constitution = (*live.get()).clone();
        edit(&mut constitution)?;
        self.runtime.block_on(
            constitution.save(&self.store, &Self::constitution_key(&slot)),
        )?;
        live.set(constitution);
        Ok(())
    }

    /// Replaces a repository's stored constitution with an empty one:
    /// the way out when the stored one cannot be read, which no edit can
    /// fix. Runs going on check with no rules from their next tool call.
    pub fn reset_rules(&self, repo: &str) -> anyhow::Result<()> {
        let slot = self
            .slot(repo)
            .ok_or_else(|| anyhow::anyhow!("No repository {repo}"))?;
        let key = Self::constitution_key(&slot);
        let fresh = Constitution::default();
        self.runtime.block_on(fresh.save(&self.store, &key))?;
        let mut open = self.constitutions.lock().expect("not poisoned");
        match open.get(&key) {
            Some(live) => live.set(fresh),
            None => {
                open.insert(key, Live::new(fresh));
            }
        }
        Ok(())
    }

    /// The ChatGPT account runs reach models with, if any.
    pub fn account(&self) -> Option<AccountId> {
        self.account.lock().expect("not poisoned").clone()
    }

    /// Why the ChatGPT plan last refused a run, while runs use it: a
    /// usage limit, a sign-in OpenAI no longer takes, a restriction.
    pub fn refusal(&self) -> Option<Refusal> {
        self.account()?;
        self.client
            .lock()
            .expect("not poisoned")
            .as_ref()?
            .refusal()
    }

    /// Checks, once per sign-in, whether the active account may use
    /// its plan here: `GET /v1/models`, whose listing is not shown (the
    /// picker offers the model table's, [`plan_models`]); only a
    /// restricted refusal matters. The check runs on the host's runtime
    /// and the returned task ends when it is saved; `None` means no
    /// account is active.
    pub fn check_eligibility(&self) -> Option<tokio::task::JoinHandle<()>> {
        let account = self.account()?;
        let chatgpt = self.config.credentials.chatgpt();
        let refused = self.not_eligible.clone();
        Some(self.runtime.spawn(async move {
            let checked = match chatgpt {
                Ok(chatgpt) => chatgpt.models(&account).await.map(drop),
                Err(error) => Err(error),
            };
            *refused.lock().expect("not poisoned") =
                checked.err().as_ref().and_then(not_eligible);
        }))
    }

    /// Why the last eligibility check said plan use is not available to
    /// the account, if it did.
    pub fn not_eligible(&self) -> Option<String> {
        self.not_eligible.lock().expect("not poisoned").clone()
    }

    fn access_label(&self) -> &'static str {
        if self.account().is_some() {
            "ChatGPT plan"
        } else {
            "signed out"
        }
    }

    /// Runs started from now on reach models on `account`'s plan; runs
    /// going on keep theirs. `None` stops new runs until one is set.
    pub fn set_account(
        &self,
        account: Option<AccountId>,
    ) -> anyhow::Result<()> {
        if let Some(account) = &account {
            let (agent, client) = coder(
                &self.runtime,
                account,
                &self.config.credentials,
                &self.config.default_model(),
            )?;
            *self.base.lock().expect("not poisoned") = agent;
            *self.client.lock().expect("not poisoned") = Some(client);
        }
        *self.account.lock().expect("not poisoned") = account;
        Ok(())
    }

    /// The models the picker offers, with what this sign-in can run, and
    /// the user's choices.
    pub fn models(&self) -> Models {
        let account = self.account();
        let credentials = &self.config.credentials;
        Models {
            // The plan's models, from the model table, while signed in.
            options: if account.is_some() {
                plan_models()
            } else {
                Vec::new()
            },
            settings: self.settings.lock().expect("not poisoned").clone(),
            access: AccessInfo {
                label: self.access_label().into(),
                chatgpt: account.is_some(),
                jev: credentials.jev_key().is_some(),
                accounts: credentials.accounts(),
            },
            agents: vec![(
                "coder".into(),
                "Runs you start from the composer.".into(),
            )],
        }
    }

    /// Keeps and saves the user's model choices.
    pub fn save_settings(&self, settings: ModelSettings) -> anyhow::Result<()> {
        *self.settings.lock().expect("not poisoned") = settings.clone();
        if let Some(dir) = self.config.settings.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(
            &self.config.settings,
            serde_json::to_string_pretty(&settings)?,
        )?;
        Ok(())
    }

    /// Starts a chat in `repo`, which must be listed: a fork of the
    /// repository's main chat at its latest turn, on that turn's code,
    /// or at its start when it has none. Returns the chat's view, ready
    /// to be pushed into the workspace before its first event arrives.
    pub fn start(
        &self,
        prompt: &str,
        choice: &ModelChoice,
        repo: &str,
    ) -> anyhow::Result<RunView> {
        let repo = self
            .slot(repo)
            .ok_or_else(|| anyhow::anyhow!("No repository {repo}"))?;
        let main = self.main_of(&repo.name)?;
        let (source, seq, turn) = match self.fork_point(&main, None)? {
            Some((source, seq, link)) => (source, seq, link.turn),
            None => (main, -1, 0),
        };
        self.fork_at(&repo, source, seq, turn, prompt, choice)
    }

    /// Forks `run` after `turn` (its latest turn when `None`): a new run
    /// on `prompt` that has the run's conversation up to that turn and
    /// works on that turn's code, in a workspace of its own.
    pub fn fork(
        &self,
        run: &RunId,
        turn: Option<u32>,
        prompt: &str,
        choice: &ModelChoice,
    ) -> anyhow::Result<RunView> {
        let repo = self.slot_of_run(run)?;
        // Runs nest one level (ADR 0016): only a main chat has runs
        // under it.
        if !self.is_main(run) {
            anyhow::bail!(
                "Only a repository's main chat can be forked: a chat under \
                 it, like a sub-agent, has nothing under it"
            );
        }
        let (source, seq, link) =
            self.fork_point(run, turn)?.ok_or_else(|| match turn {
                Some(turn) => {
                    anyhow::anyhow!("Turn {turn} has no commit to fork from")
                }
                None => anyhow::anyhow!(
                    "The run has no finished turn to fork from yet"
                ),
            })?;
        self.fork_at(&repo, source, seq, link.turn, prompt, choice)
    }

    /// Starts a run on `prompt` that continues `source` from its entry
    /// `seq`, after its turn `turn`, in a workspace of its own in `repo`.
    fn fork_at(
        &self,
        repo: &RepoSlot,
        source: RunId,
        seq: i64,
        turn: u32,
        prompt: &str,
        choice: &ModelChoice,
    ) -> anyhow::Result<RunView> {
        // An effort the model does not take falls back to auto.
        let choice = &choice.clone().fitted();
        // Named after what it was asked, so the workspace says what it is
        // for.
        let name = workspace_name(&branch_slug(prompt));
        // A fork is a chat under the main chat: it delegates to none.
        let (agent, workspace) =
            self.agent_for_run(choice, repo, name, false)?;
        let _guard = self.runtime.enter();
        let forked = agent
            .fork(&Checkpoint::at(source.clone(), seq))
            .after_turn(turn)
            .start(prompt, &self.store);
        let id = self.track(forked, workspace, choice, &repo.name);
        // What each plugin's state is as the fork inherits it: a goal set
        // in the main chat, which tau-goal goes on checking; then what it
        // says as the fork starts.
        let starting = self.starting(&self.run_ctx(
            tau_ui_plugin::RunKind::Chat,
            repo,
            choice,
        ));
        let inherited: Vec<(String, Vec<serde_json::Value>)> = self
            .hosted
            .iter()
            .map(|hosted| {
                let name = hosted.plugin.name();
                let mut records = self.plugin_records_at(&source, seq, name);
                records.extend(
                    starting
                        .iter()
                        .filter(|(plugin, _)| plugin == name)
                        .map(|(_, body)| body.clone()),
                );
                (name.to_owned(), records)
            })
            .collect();
        let mut view = self
            .view(id, prompt, repo)
            .with_origin(Origin::Fork { from: source, turn });
        for (plugin, records) in inherited {
            view.restate(&plugin, &records);
        }
        Ok(view)
    }

    /// `plugin`'s records a fork of `source` at `seq` inherits: along
    /// `source`'s chain, without what `source` stored after `seq`.
    fn plugin_records_at(
        &self,
        source: &RunId,
        seq: i64,
        plugin: &str,
    ) -> Vec<serde_json::Value> {
        let all = self.plugin_records(source, plugin);
        let after = self
            .runtime
            .block_on(self.store.plugin_entries(&source.0, plugin))
            .unwrap_or_default()
            .iter()
            .filter(|(at, _)| *at > seq)
            .count();
        let keep = all.len().saturating_sub(after);
        all.into_iter().take(keep).collect()
    }

    /// Where forking `run` after `turn` starts: the run that took the
    /// turn, which for a turn a fork inherited is an ancestor, with the
    /// `seq` and link of that turn.
    fn fork_point(
        &self,
        run: &RunId,
        turn: Option<u32>,
    ) -> anyhow::Result<Option<(RunId, i64, Link)>> {
        let mut run = run.clone();
        loop {
            if let Some((seq, link)) = self.link(&run, turn)? {
                return Ok(Some((run, seq, link)));
            }
            if turn.is_none() {
                return Ok(None);
            }
            let record = self.runtime.block_on(self.store.run(&run.0))?;
            match record.map(|record| record.kind) {
                Some(RunKind::Fork { parent, .. }) => {
                    run = RunId(parent.into());
                }
                _ => return Ok(None),
            }
        }
    }

    /// Goes on with `run`, a finished chat, on `prompt`: the same run,
    /// in the same workspace, on `choice`. Its events carry on in the
    /// view the workspace already has.
    pub fn resume(
        &self,
        run: &RunId,
        prompt: &str,
        choice: &ModelChoice,
    ) -> anyhow::Result<()> {
        if self.is_running(run) {
            anyhow::bail!("The run is still going; steer it instead");
        }
        let repo = self.slot_of_run(run)?;
        // The workspace its last turn worked in, which still has its
        // files.
        let known = self
            .workspaces
            .lock()
            .expect("not poisoned")
            .get(run)
            .cloned();
        // A main chat works in the repository's own checkout, the
        // default workspace, not one of its own.
        let main = self.is_main(run);
        let workspace = match known {
            _ if main => Some(DEFAULT_WORKSPACE.to_owned()),
            Some(name) => Some(name),
            None => self.link(run, None)?.map(|(_, link)| link.workspace),
        };
        // A main chat catches up with trunk first: its commits move
        // trunk, which may have moved without it.
        if main && let Some(name) = &workspace {
            self.catch_up(&repo.project()?, name)?;
        }
        // An effort the model does not take falls back to auto.
        let choice = &choice.clone().fitted();
        // A run without a workspace yet gets one, named after the
        // message.
        let workspace =
            workspace.unwrap_or_else(|| workspace_name(&branch_slug(prompt)));
        let (agent, workspace) =
            self.agent_for_run(choice, &repo, workspace, main)?;
        let _guard = self.runtime.enter();
        let resumed = agent.resume(run).start(prompt, &self.store);
        self.track(resumed, workspace, choice, &repo.name);
        Ok(())
    }

    /// The link of `turn` in `run` (the latest when `None`), with the
    /// `seq` to fork at.
    fn link(
        &self,
        run: &RunId,
        turn: Option<u32>,
    ) -> anyhow::Result<Option<(i64, Link)>> {
        let entries = self
            .runtime
            .block_on(self.store.plugin_entries(&run.0, WORKSPACE_PLUGIN))?;
        let mut links = entries.iter().filter_map(|(seq, body)| {
            Link::parse(body).map(|link| (*seq, link))
        });
        Ok(match turn {
            Some(turn) => links.find(|(_, link)| link.turn == turn),
            None => links.next_back(),
        })
    }

    /// What landing `child` on its parent would do (ADR 0009): its
    /// changes as they would sit on the parent's stack, and the files
    /// that would conflict. Changes nothing.
    pub fn preview_landing(&self, child: &RunId) -> anyhow::Result<Landing> {
        let landing = self.landing(child)?;
        let into = self.bookmark_of(&landing.parent, &landing.project)?;
        Ok(self.runtime.block_on(landing.parent_vcs.land(
            &landing.child_head,
            into,
            false,
        ))?)
    }

    /// Lands `child` on its parent (ADR 0009): restacks its changes onto
    /// the parent's newest commit, records them as links in the parent,
    /// and closes the child: its workspace goes, and so does its
    /// bookmark. Both runs must be idle.
    pub fn land(&self, child: &RunId) -> anyhow::Result<Landing> {
        let plan = self.landing(child)?;
        let into = self.bookmark_of(&plan.parent, &plan.project)?;
        let landing = self.runtime.block_on(plan.parent_vcs.land(
            &plan.child_head,
            into,
            true,
        ))?;
        // The landed changes join the parent's links, oldest first, at
        // the parent's latest turn, so forks, the compare view and pull
        // requests see them as the parent's own.
        let turn = self
            .link(&plan.parent, None)?
            .map_or(0, |(_, link)| link.turn);
        let entries = landing
            .changes
            .iter()
            .rev()
            .map(|change| {
                let link = Link {
                    turn,
                    workspace: plan.parent_workspace.clone(),
                    commit_id: change.commit_id.clone(),
                    change_id: change.change_id.clone(),
                    changed: true,
                    from: Some(child.0.to_string()),
                    snapshot: false,
                };
                Ok(Entry::Plugin {
                    plugin: WORKSPACE_PLUGIN.to_owned(),
                    body: serde_json::to_string(&link)?,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        // And a record to draw the landing's card from, in history.
        let record = LandingRecord {
            from: child.0.to_string(),
            title: self.title_of(child)?,
            landing: landing.clone(),
        };
        let mut entries = entries;
        entries.push(Entry::Plugin {
            plugin: LANDING_RECORD.to_owned(),
            body: serde_json::to_string(&record)?,
        });
        self.runtime.block_on(self.store.append_turn(
            &plan.parent.0,
            &entries,
            TurnUsage::default(),
        ))?;
        // The child's changes live on the parent's stack now.
        plan.project.forget_workspace(&plan.child_workspace)?;
        plan.project.remove_bookmark(&bookmark(child))?;
        self.workspaces.lock().expect("not poisoned").remove(child);
        Ok(landing)
    }

    /// Starts `run`'s next turn itself, with `prompt`: the turn that
    /// resolves a landing's conflicts (ADR 0014), on the model the run
    /// was on.
    pub fn start_resolving(
        &self,
        run: &RunId,
        prompt: &str,
    ) -> anyhow::Result<()> {
        let choice = self
            .choices
            .lock()
            .expect("not poisoned")
            .get(run)
            .cloned()
            .unwrap_or_else(|| {
                ModelChoice::new(self.config.default_model(), Effort::Auto)
            });
        self.resume(run, prompt, &choice)
    }

    /// Drops `child`: abandons its own changes, the ones its parent does
    /// not have, and closes it like a landing does (ADR 0009). Both runs
    /// must be idle. The operation log keeps what was abandoned.
    pub fn drop_child(&self, child: &RunId) -> anyhow::Result<()> {
        let parent = self.parent_of(child)?;
        for run in [child, &parent] {
            if self.is_running(run) {
                anyhow::bail!(
                    "{} is still running; drop it once it stops",
                    run.0
                );
            }
        }
        let project = self.slot_of_run(child)?.project()?;
        if let Some(head) = project.bookmark(&bookmark(child))? {
            let keep = match project
                .bookmark(&self.bookmark_of(&parent, &project)?)?
            {
                Some(keep) => keep,
                None => project.trunk()?,
            };
            project.abandon_between(&keep, &head)?;
        }
        let workspace = self
            .workspaces
            .lock()
            .expect("not poisoned")
            .remove(child)
            .or(self.link(child, None)?.map(|(_, link)| link.workspace));
        if let Some(name) = workspace {
            project.forget_workspace(&name)?;
        }
        project.remove_bookmark(&bookmark(child))?;
        Ok(())
    }

    /// The run `child` was forked from or called by.
    fn parent_of(&self, child: &RunId) -> anyhow::Result<RunId> {
        let record = self
            .runtime
            .block_on(self.store.run(&child.0))?
            .ok_or_else(|| anyhow::anyhow!("No run {}", child.0))?;
        match record.kind {
            RunKind::Fork { parent, .. } | RunKind::Subagent { parent, .. } => {
                Ok(RunId(parent.into()))
            }
            RunKind::Root => anyhow::bail!("{} has no parent", child.0),
        }
    }

    /// `run`'s title: the one a model wrote, or until then the
    /// placeholder for the words it started with.
    pub(crate) fn title_of(&self, run: &RunId) -> anyhow::Result<String> {
        let written = self
            .runtime
            .block_on(self.store.run(&run.0))?
            .and_then(|record| record.title);
        match written {
            Some(title) => Ok(title),
            None => Ok(crate::titles::placeholder(&self.stored_prompt(run)?)),
        }
    }

    /// The words `run` was started with, from the store.
    fn stored_prompt(&self, run: &RunId) -> anyhow::Result<String> {
        self.runtime.block_on(first_prompt(&self.store, &run.0))
    }

    /// Everything landing `child` needs, once both runs are idle.
    fn landing(&self, child: &RunId) -> anyhow::Result<LandingPlan> {
        let parent = self.parent_of(child)?;
        for run in [child, &parent] {
            self.settle(run);
            if self.is_running(run) {
                anyhow::bail!("{} is still running; land once it stops", run.0);
            }
        }
        let project = self.slot_of_run(child)?.project()?;
        let workspace_of = |run: &RunId| -> anyhow::Result<String> {
            let known = self
                .workspaces
                .lock()
                .expect("not poisoned")
                .get(run)
                .cloned();
            match known {
                Some(name) => Ok(name),
                None => self
                    .link(run, None)?
                    .map(|(_, link)| link.workspace)
                    .ok_or_else(|| {
                        anyhow::anyhow!("{} has not finished a turn", run.0)
                    }),
            }
        };
        let child_workspace = workspace_of(child)?;
        let parent_workspace = match workspace_of(&parent) {
            // A main chat works in the repository's own checkout.
            _ if self.is_main(&parent) => {
                self.workspaces
                    .lock()
                    .expect("not poisoned")
                    .insert(parent.clone(), DEFAULT_WORKSPACE.to_owned());
                DEFAULT_WORKSPACE.to_owned()
            }
            Ok(name) => {
                // Opening a workspace that is gone would make a new one
                // on trunk; landing there would lose the parent's work.
                if !project.workspaces()?.contains(&name) {
                    anyhow::bail!("The parent's workspace is gone");
                }
                name
            }
            Err(error) => return Err(error),
        };
        let child_head =
            project.bookmark(&bookmark(child))?.ok_or_else(|| {
                anyhow::anyhow!("{} has no changes to land", child.0)
            })?;
        // A main chat takes landings on trunk as it is now.
        if self.is_main(&parent) {
            self.catch_up(&project, &parent_workspace)?;
        }
        let parent_vcs =
            project.add_workspace(&parent_workspace, &project.trunk()?)?;
        Ok(LandingPlan {
            parent,
            project,
            parent_workspace,
            child_workspace,
            child_head,
            parent_vcs,
        })
    }

    /// Keeps `run`, a branch of a fork, and removes the workspaces of the
    /// other branches that are not running: the run it was forked from,
    /// and its other forks. Their commits stay in the project.
    pub fn keep_branch(&self, run: &RunId) -> anyhow::Result<()> {
        let project = self.slot_of_run(run)?.project()?;
        let store = self.store.clone();
        let kept = run.0.to_string();
        let running: Vec<String> = self
            .runs
            .lock()
            .expect("not poisoned")
            .keys()
            .map(|run| run.0.to_string())
            .collect();
        self.runtime.block_on(async move {
            let record = store
                .run(&kept)
                .await?
                .ok_or_else(|| anyhow::anyhow!("No run {kept}"))?;
            let RunKind::Fork { parent, .. } = record.kind else {
                return Ok(());
            };
            let family: Vec<String> = store
                .recent_runs(1000)
                .await?
                .into_iter()
                .filter(|other| {
                    other.id == parent
                        || matches!(&other.kind, RunKind::Fork { parent: p, .. } if *p == parent)
                })
                .map(|other| other.id)
                .filter(|id| *id != kept && !running.contains(id))
                .collect();
            for other in family {
                let names: Vec<String> = store
                    .plugin_entries(&other, WORKSPACE_PLUGIN)
                    .await?
                    .iter()
                    .filter_map(|(_, body)| Link::parse(body))
                    .map(|link| link.workspace)
                    .collect();
                let project = project.clone();
                tokio::task::spawn_blocking(move || {
                    names.iter().try_for_each(|name| project.forget_workspace(name))
                })
                .await??;
            }
            Ok(())
        })
    }

    /// The code of `main` and its fork `fork`: what each changed after
    /// the fork point, and how the fork's code differs from the run's.
    pub fn branch_code(
        &self,
        main: &RunId,
        fork: &RunId,
    ) -> impl std::future::Future<Output = anyhow::Result<BranchCode>> + Send + 'static
    {
        branch_code(
            self.store.clone(),
            self.slot_of_run(main),
            main.clone(),
            fork.clone(),
        )
    }

    /// Runs `future` on the host's runtime and waits for it, for callers
    /// outside the runtime.
    pub fn block_on<F: std::future::Future>(&self, future: F) -> F::Output {
        self.runtime.block_on(future)
    }

    /// Runs from earlier sessions, newest first, rebuilt from the store.
    /// The view of `repo`'s main chat, as history shows it.
    pub fn main_view(&self, repo: &Repo) -> anyhow::Result<Option<RunView>> {
        let Some(main) = &repo.main else {
            return Ok(None);
        };
        self.runtime.block_on(async {
            match self.store.run(&main.0).await? {
                Some(record) => {
                    Ok(Some(stored_view(&self.store, &record).await?))
                }
                None => Ok(None),
            }
        })
    }

    pub fn history(&self) -> anyhow::Result<Vec<RunView>> {
        self.runtime.block_on(history(&self.store, &self.mains()))
    }

    /// Follows a started run: its control, its workspace, and a task
    /// that forwards its events.
    fn track(
        &self,
        mut run: tau_agent::agent::Run,
        workspace: String,
        choice: &ModelChoice,
        repo: &str,
    ) -> RunId {
        let id = run.id();
        self.run_repos
            .lock()
            .expect("not poisoned")
            .insert(id.clone(), repo.to_owned());
        self.choices
            .lock()
            .expect("not poisoned")
            .insert(id.clone(), choice.clone());
        self.runs
            .lock()
            .expect("not poisoned")
            .insert(id.clone(), run.control());
        self.workspaces
            .lock()
            .expect("not poisoned")
            .insert(id.clone(), workspace);
        let events = self.events.clone();
        let runs = self.runs.clone();
        let ending = self.ending.clone();
        self.runtime.spawn(async move {
            {
                let mut stream = run.events();
                while let Some(event) = stream.next().await {
                    if events.send(event).is_err() {
                        break;
                    }
                }
            }
            let id = run.id();
            // The outcome is stored by the run; the events said it all.
            let _ = run.outcome().await;
            runs.lock().expect("not poisoned").remove(&id);
            ending.lock().expect("not poisoned").remove(&id);
        });
        id
    }

    fn view(&self, id: RunId, prompt: &str, repo: &RepoSlot) -> RunView {
        let choice = self
            .choices
            .lock()
            .expect("not poisoned")
            .get(&id)
            .cloned()
            .unwrap_or_else(|| {
                ModelChoice::new(self.config.default_model(), Effort::Auto)
            });
        let mut view = RunView::new(
            id,
            crate::titles::placeholder(prompt),
            "coder",
            &choice.model,
        )
        .in_repo(repo.name.clone())
        .started("just now");
        view.push_user(prompt);
        // What plugins say as it starts.
        let run = self.run_ctx(tau_ui_plugin::RunKind::Chat, repo, &choice);
        for (plugin, body) in self.starting(&run) {
            view.fold(&plugin, &body);
        }
        view.limits = ViewLimits {
            max_turns: Some(MAX_TURNS),
            ..ViewLimits::default()
        };
        view.context = ContextWindow {
            window: find(&choice.model).map(|model| model.context_window),
            // Where fast-compaction steps in, when there is a key.
            trigger: self.jev().is_some().then(|| {
                (tau_fast_compaction::Settings::default().compact_at_percent
                    / 100.0) as f32
            }),
            ..ContextWindow::default()
        };
        let workspace = self
            .workspaces
            .lock()
            .expect("not poisoned")
            .get(&view.id)
            .cloned();
        view.plan = vec![
            PlanField {
                name: "model".into(),
                value: choice.model.clone(),
                set_by: None,
            },
            PlanField {
                name: "reasoning".into(),
                value: choice.effort.label().into(),
                set_by: None,
            },
            PlanField {
                name: "access".into(),
                value: self.access_label().into(),
                set_by: None,
            },
            PlanField {
                name: "workspace".into(),
                value: match (repo.project.wait(), &workspace) {
                    (Some(project), Some(name)) => {
                        project.workspace_dir(name).display().to_string()
                    }
                    _ => String::new(),
                },
                set_by: workspace.as_ref().map(|_| "workspace".to_owned()),
            },
        ];
        let jev = self.jev().is_some();
        let rules = self
            .constitution(repo)
            .map_or(0, |live| live.get().rules.len());
        let [pruning, constitution] = jev_statuses(jev, rules);
        view.plugins = vec![PluginStatus {
            name: "tau-tools".into(),
            state: "7 tools".into(),
            tone: Tone::Quiet,
        }];
        if workspace.is_some() {
            view.plugins.push(PluginStatus {
                name: "workspace".into(),
                state: "a commit per turn".into(),
                tone: Tone::Quiet,
            });
        }
        view.plugins.push(pruning);
        view.plugins.push(PluginStatus {
            name: "tau-compaction".into(),
            state: "watching the window".into(),
            tone: Tone::Quiet,
        });
        view.plugins.push(PluginStatus {
            name: tau_memory::plugin::NAME.into(),
            state: match self
                .memories
                .catalog(&self.memory_dir(repo))
                .notes
                .len()
            {
                0 => "no notes yet".into(),
                1 => "1 note".into(),
                n => format!("{n} notes"),
            },
            tone: Tone::Quiet,
        });
        view.plugins.push(constitution);
        view
    }

    pub fn steer(&self, run: &RunId, text: &str) {
        if let Some(control) = self.runs.lock().expect("not poisoned").get(run)
        {
            control.steer(text);
        }
    }

    pub fn cancel(&self, run: &RunId) {
        if let Some(control) = self.runs.lock().expect("not poisoned").get(run)
        {
            control.cancel();
        }
    }

    /// Wires the host to a workspace: its events drive the host, and the
    /// runs' events drive the workspace. Loads history first.
    pub fn attach(
        self,
        workspace: &Entity<Workspace>,
        mut events: mpsc::UnboundedReceiver<RunEvent>,
        cx: &mut App,
    ) {
        match self.history() {
            Ok(runs) => workspace
                .update(cx, |ws, cx| ws.apply(HostUpdate::History(runs), cx)),
            Err(error) => eprintln!("tau-ui: cannot read past runs: {error:#}"),
        }
        let host = Arc::new(self);
        // Phones reach this host once allowed.
        crate::phone_server::serve(
            host.runtime.handle().clone(),
            host.config.credentials.dir.join("phones"),
            workspace,
            cx,
        );
        // Once a repository is imported, the plugins and the status bar
        // change.
        let slots = host.repos.lock().expect("not poisoned").clone();
        for slot in slots {
            refresh_when_imported(&host, &slot, workspace, cx);
            // What changed while tau was closed comes in, quietly.
            update_in_background(&host, &slot.name, workspace, false, cx);
        }
        github::restore(workspace, &host.config.credentials, &host.github, cx);
        let handler = host.clone();
        // A sign-in, a switch of account or a sign-out from the Models
        // screen changes what new runs use.
        let connected: accounts::Connected = {
            let (host, entity) = (host.clone(), workspace.downgrade());
            std::rc::Rc::new(move |account, cx| {
                let applied = host.set_account(account);
                let catalog = host.catalog();
                let Some(workspace) = entity.upgrade() else {
                    return;
                };
                check_eligibility(&host, &workspace, cx);
                workspace.update(cx, |ws, cx| {
                    ws.apply(HostUpdate::catalog(catalog), cx);
                    if let Err(error) = applied {
                        ws.apply(
                            HostUpdate::alert(
                                "Could not use the new sign-in",
                                format!("{error:#}"),
                            ),
                            cx,
                        );
                    }
                });
            })
        };
        let sign_ins = accounts::SignIns::default();
        cx.subscribe(
            workspace,
            move |workspace, event: &WorkspaceEvent, cx| {
                if sign_ins.handle(
                    event,
                    &workspace,
                    &handler.config.credentials,
                    handler.config.model.as_deref(),
                    &connected,
                    cx,
                ) || github::handle(
                    event,
                    &workspace,
                    &handler.config.credentials,
                    &handler.github,
                    cx,
                ) {
                    return;
                }
                match event {
                WorkspaceEvent::UpdateRepo { repo } => {
                    update_in_background(&handler, repo, &workspace, true, cx)
                }
                WorkspaceEvent::CloneRepos { repos } => {
                    for name in repos {
                        clone_into_tau(&handler, name, &workspace, cx);
                    }
                }
                WorkspaceEvent::NewRun {
                    prompt,
                    model,
                    repo,
                } => match handler.start(prompt, model, repo) {
                    Ok(view) => {
                        let run = view.id.clone();
                        workspace.update(cx, |ws, cx| ws.apply(HostUpdate::Run(Box::new(view)), cx));
                        title_in_background(&handler, &run, prompt, &workspace, cx);
                    }
                    Err(error) => workspace.update(cx, |ws, cx| {
                        ws.apply(HostUpdate::alert("Could not start the run", format!("{error:#}")), cx)
                    }),
                },
                WorkspaceEvent::AddRule {
                    repo,
                    text,
                    on,
                    review,
                    block,
                } => {
                    let added = handler.edit_rules(repo, |rules| {
                        rules.add(text, on, *review, *block).map(drop)
                    });
                    let catalog = handler.catalog();
                    workspace.update(cx, |ws, cx| {
                        ws.apply(HostUpdate::catalog(catalog), cx);
                        if let Err(error) = added {
                            ws.apply(HostUpdate::alert("Could not add the rule", format!("{error:#}")), cx);
                        }
                    });
                }
                WorkspaceEvent::UpdateRule {
                    repo,
                    id,
                    text,
                    on,
                    review,
                    block,
                } => {
                    let saved = handler.edit_rules(repo, |rules| {
                        rules.replace(id, text, on, *review, *block)
                    });
                    let catalog = handler.catalog();
                    workspace.update(cx, |ws, cx| {
                        ws.apply(HostUpdate::catalog(catalog), cx);
                        if let Err(error) = saved {
                            ws.apply(HostUpdate::alert("Could not save the rule", format!("{error:#}")), cx);
                        }
                    });
                }
                WorkspaceEvent::TryRule {
                    text,
                    on,
                    review,
                    block,
                    calls,
                    answers,
                    ..
                } => {
                    let job = {
                        let host = handler.clone();
                        let (text, on) = (text.clone(), on.clone());
                        let (review, block) = (*review, *block);
                        let (calls, answers) = (calls.clone(), answers.clone());
                        handler.runtime.spawn_blocking(move || {
                            host.try_rule(&text, &on, review, block, &calls, &answers)
                        })
                    };
                    let workspace = workspace.downgrade();
                    cx.spawn(async move |cx| {
                        let result =
                            job.await.unwrap_or_else(|e| Err(e.to_string()));
                        let _ = workspace
                            .update(cx, |ws, cx| ws.apply(HostUpdate::RuleTrial(result), cx));
                    })
                    .detach();
                }
                WorkspaceEvent::PreparePullRequest { run } => {
                    let job = {
                        let (host, run) = (handler.clone(), run.clone());
                        handler
                            .runtime
                            .spawn_blocking(move || host.prepare_pull_request(&run))
                    };
                    let (run, workspace) = (run.clone(), workspace.downgrade());
                    cx.spawn(async move |cx| {
                        let prepared = match job.await {
                            Ok(result) => result.map_err(|error| format!("{error:#}")),
                            Err(error) => Err(error.to_string()),
                        };
                        let _ = workspace.update(cx, |ws, cx| match prepared {
                            Ok(draft) => ws.apply(HostUpdate::PullRequest { run: run.clone(), pr: Box::new(draft) }, cx),
                            Err(error) => {
                                ws.back(cx);
                                ws.apply(HostUpdate::alert("Could not write the pull request", error), cx)
                            }
                        });
                    })
                    .detach();
                }
                WorkspaceEvent::CreatePullRequest {
                    run,
                    title,
                    body,
                    draft,
                    keep_pushing,
                    reviewers,
                } => {
                    let Some(prepared) = handler.draft(run) else {
                        return;
                    };
                    let job = {
                        let host = handler.clone();
                        let (run, title, body, reviewers) =
                            (run.clone(), title.clone(), body.clone(), reviewers.clone());
                        let (draft, keep_pushing) = (*draft, *keep_pushing);
                        handler.runtime.spawn_blocking(move || {
                            host.create_pull_request(
                                &run,
                                &prepared,
                                &title,
                                &body,
                                draft,
                                keep_pushing,
                                &reviewers,
                            )
                        })
                    };
                    let (host, run, workspace) =
                        (handler.clone(), run.clone(), workspace.downgrade());
                    cx.spawn(async move |cx| {
                        let created = match job.await {
                            Ok(result) => result.map_err(|error| format!("{error:#}")),
                            Err(error) => Err(error.to_string()),
                        };
                        let opened = created.is_ok();
                        let state = match created {
                            Ok((opened, _)) => PrState::Opened {
                                number: opened.number,
                                url: opened.url,
                                checks: crate::pull_request::Checks::Running,
                            },
                            Err(error) => PrState::Failed(error),
                        };
                        let _ = workspace.update(cx, |ws, cx| {
                            ws.apply(HostUpdate::PullRequestState { run: run.clone(), state }, cx)
                        });
                        if opened {
                            watch_checks(host, run, workspace, cx).await;
                        }
                    })
                    .detach();
                }
                WorkspaceEvent::Query { sql } => {
                    let job = {
                        let (store, sql) = (handler.store.clone(), sql.clone());
                        handler.runtime.spawn(async move {
                            store.query(&sql, 200).await.map_err(|e| e.to_string())
                        })
                    };
                    let workspace = workspace.downgrade();
                    cx.spawn(async move |cx| {
                        let result = job.await.unwrap_or_else(|e| Err(e.to_string()));
                        let _ = workspace
                            .update(cx, |ws, cx| ws.apply(HostUpdate::QueryResult(result), cx));
                    })
                    .detach();
                }
                WorkspaceEvent::ConstitutionSettings {
                    repo,
                    blocks_unchecked,
                    max_holds,
                } => {
                    let saved = handler.edit_rules(repo, |rules| {
                        rules.on_error = if *blocks_unchecked {
                            tau_constitution::OnError::Block
                        } else {
                            tau_constitution::OnError::Allow
                        };
                        rules.max_holds = *max_holds;
                        Ok(())
                    });
                    let catalog = handler.catalog();
                    workspace.update(cx, |ws, cx| {
                        ws.apply(HostUpdate::catalog(catalog), cx);
                        if let Err(error) = saved {
                            ws.apply(HostUpdate::alert("Could not save the constitution", format!("{error:#}")), cx);
                        }
                    });
                }
                WorkspaceEvent::ResetRules { repo } => {
                    let reset = handler.reset_rules(repo);
                    let catalog = handler.catalog();
                    workspace.update(cx, |ws, cx| {
                        ws.apply(HostUpdate::catalog(catalog), cx);
                        if let Err(error) = reset {
                            ws.apply(HostUpdate::alert("Could not remove the rules", format!("{error:#}")), cx);
                        }
                    });
                }
                WorkspaceEvent::RemoveRule { repo, id } => {
                    let removed = handler.edit_rules(repo, |rules| {
                        rules.remove(id);
                        Ok(())
                    });
                    let catalog = handler.catalog();
                    workspace.update(cx, |ws, cx| {
                        ws.apply(HostUpdate::catalog(catalog), cx);
                        if let Err(error) = removed {
                            ws.apply(HostUpdate::alert("Could not remove the rule", format!("{error:#}")), cx);
                        }
                    });
                }
                WorkspaceEvent::JevKey { key } => {
                    let saved =
                        handler.config.credentials.set_jev_key(key.as_deref());
                    let catalog = handler.catalog();
                    workspace.update(cx, |ws, cx| {
                        ws.apply(HostUpdate::catalog(catalog), cx);
                        if let Err(error) = saved {
                            ws.apply(HostUpdate::alert("Could not save the TypeSafe key", error.to_string()), cx);
                        }
                    });
                }
                WorkspaceEvent::PluginAct { plugin, action } => {
                    // Off the UI thread: an action may ask Jev, or the
                    // store.
                    let (host, plugin, action) =
                        (handler.clone(), plugin.clone(), action.clone());
                    let task = cx.background_spawn({
                        let plugin = plugin.clone();
                        async move { host.plugin_act(&plugin, action) }
                    });
                    let workspace = workspace.downgrade();
                    cx.spawn(async move |cx| {
                        let done = task.await;
                        let _ = workspace.update(cx, |ws, cx| match done {
                            Ok(Some(reply)) => ws.apply(
                                HostUpdate::PluginReply { plugin, reply },
                                cx,
                            ),
                            Ok(None) => {}
                            Err(error) => ws.apply(
                                HostUpdate::alert(
                                    format!("{plugin} could not do that"),
                                    format!("{error:#}"),
                                ),
                                cx,
                            ),
                        });
                    })
                    .detach();
                }
                WorkspaceEvent::PluginSettings { plugin, settings } => {
                    let saved =
                        handler.save_plugin_settings(plugin, settings.clone());
                    let catalog = handler.catalog();
                    workspace.update(cx, |ws, cx| {
                        ws.apply(HostUpdate::catalog(catalog), cx);
                        if let Err(error) = saved {
                            ws.apply(HostUpdate::alert(format!("Could not save {plugin}'s settings"), format!("{error:#}")), cx);
                        }
                    });
                }
                WorkspaceEvent::PluginRecord { run, plugin, body } => {
                    if let Err(error) =
                        handler.store_plugin_record(run, plugin, body)
                    {
                        // The interface showed the change already: put
                        // back what the plugin will read, and say so.
                        let records = handler.plugin_records(run, plugin);
                        workspace.update(cx, |ws, cx| {
                            ws.apply(HostUpdate::PluginRestate { run: run.clone(), plugin: plugin.clone(), records }, cx);
                            ws.apply(HostUpdate::alert(format!("Could not save what {plugin} changed"), format!("{error:#}")), cx);
                        });
                    }
                }
                WorkspaceEvent::Reviewed { run, call_id } => {
                    if let Err(error) = handler.set_reviewed(run, call_id) {
                        eprintln!("tau-ui: cannot save the review: {error:#}");
                    }
                }
                // Opening a flagged call only shows it; nothing to keep.
                WorkspaceEvent::ReviewCall { .. } => {}
                WorkspaceEvent::CloseRun { run } => {
                    if let Err(error) = handler.set_closed(run, true) {
                        eprintln!("tau-ui: cannot save closed runs: {error:#}");
                    }
                }
                WorkspaceEvent::Resume { run, prompt, model } => {
                    // A message opens a closed conversation again.
                    let _ = handler.set_closed(run, false);
                    match handler.resume(run, prompt, model) {
                        Ok(()) => {
                            let starting = handler.starting_of(run, model);
                            workspace.update(cx, |ws, cx| {
                                for (plugin, body) in starting {
                                    ws.apply(HostUpdate::PluginFold { run: run.clone(), plugin, body }, cx);
                                }
                            });
                        }
                        Err(error) => workspace.update(cx, |ws, cx| {
                            ws.apply(HostUpdate::ResumeFailed(run.clone()), cx);
                            ws.apply(HostUpdate::alert("Could not go on with the run", format!("{error:#}")), cx)
                        }),
                    }
                }
                WorkspaceEvent::Fork {
                    run,
                    turn,
                    prompt,
                    model,
                } => match handler.fork(run, *turn, prompt, model) {
                    Ok(view) => {
                        let run = view.id.clone();
                        workspace.update(cx, |ws, cx| ws.apply(HostUpdate::Run(Box::new(view)), cx));
                        title_in_background(&handler, &run, prompt, &workspace, cx);
                    }
                    Err(error) => workspace.update(cx, |ws, cx| {
                        ws.apply(HostUpdate::alert("Could not fork the run", format!("{error:#}")), cx)
                    }),
                },
                WorkspaceEvent::CompareCode { main, fork } => {
                    let job =
                        handler.runtime.spawn(handler.branch_code(main, fork));
                    let (main, fork) = (main.clone(), fork.clone());
                    let workspace = workspace.downgrade();
                    cx.spawn(async move |cx| {
                        let code = match job.await {
                            Ok(Ok(code)) => CodeState::Ready(code),
                            Ok(Err(error)) => {
                                CodeState::Unavailable(format!("{error:#}"))
                            }
                            Err(error) => {
                                CodeState::Unavailable(error.to_string())
                            }
                        };
                        let _ = workspace.update(cx, |ws, cx| {
                            ws.apply(HostUpdate::BranchCode { main: main.clone(), fork: fork.clone(), code }, cx)
                        });
                    })
                    .detach();
                }
                WorkspaceEvent::SaveModelSettings(settings) => {
                    if let Err(error) = handler.save_settings(settings.clone())
                    {
                        workspace.update(cx, |ws, cx| {
                            ws.apply(HostUpdate::alert("Could not save the model settings", format!("{error:#}")), cx)
                        });
                    }
                }
                WorkspaceEvent::PreviewLanding { run } => {
                    let preview = handler
                        .preview_landing(run)
                        .map_err(|error| format!("{error:#}"));
                    workspace.update(cx, |ws, cx| {
                        ws.apply(HostUpdate::LandingPreview { run: run.clone(), preview }, cx)
                    });
                }
                WorkspaceEvent::Land { run } => {
                    let parent = handler.parent_of(run).ok();
                    let landed =
                        handler.land(run).map_err(|error| format!("{error:#}"));
                    let conflicts = landed
                        .as_ref()
                        .map(|landing| landing.conflicts.clone())
                        .unwrap_or_default();
                    workspace.update(cx, |ws, cx| ws.apply(HostUpdate::Landed { run: run.clone(), landing: landed }, cx));
                    // What conflicts, the parent resolves, in a turn tau
                    // starts (ADR 0014).
                    if let Some(parent) = parent.filter(|_| !conflicts.is_empty()) {
                        let title = handler.title_of(run).unwrap_or_else(|_| run.0.to_string());
                        let prompt = format!(
                            "Landing `{title}` left conflicts in {}. Resolve them, \
                             and commit the resolution.",
                            code_list(&conflicts)
                        );
                        resolve(&handler, &parent, prompt, &workspace, cx);
                    }
                }
                WorkspaceEvent::DropChild { run } => {
                    let dropped = handler
                        .drop_child(run)
                        .map_err(|error| format!("{error:#}"));
                    workspace.update(cx, |ws, cx| ws.apply(HostUpdate::Dropped { run: run.clone(), result: dropped }, cx));
                }
                WorkspaceEvent::KeepBranch { run } => {
                    if let Err(error) = handler.keep_branch(run) {
                        eprintln!(
                            "tau-ui: cannot drop the other branches: {error:#}"
                        );
                    }
                }
                WorkspaceEvent::HideRepo { repo } => {
                    if let Err(error) = handler.hide_repo(repo) {
                        eprintln!(
                            "tau-ui: cannot save the repository list: {error:#}"
                        );
                    }
                }
                WorkspaceEvent::OpenRepos(open) => {
                    if let Err(error) = handler.set_open_repos(open.clone()) {
                        eprintln!(
                            "tau-ui: cannot save the repository list: {error:#}"
                        );
                    }
                }
                // `phone_server::serve` handles these in its own
                // subscription.
                WorkspaceEvent::Phones(_) => {}
                WorkspaceEvent::Steer { run, text } => handler.steer(run, text),
                WorkspaceEvent::Cancel { run } => handler.cancel(run),
                other => eprintln!("tau-ui: not handled yet: {other:?}"),
                }
            },
        )
        .detach();
        // What plugins' host halves tell the interface.
        if let Some(mut pushed) =
            host.pushed.lock().expect("not poisoned").take()
        {
            let (host, workspace) = (host.clone(), workspace.downgrade());
            cx.spawn(async move |cx| {
                while let Some(push) = pushed.recv().await {
                    let Some(workspace) = workspace.upgrade() else {
                        return;
                    };
                    cx.update(|cx| {
                        hosted::apply_push(&host, push, &workspace, cx)
                    });
                }
            })
            .detach();
        }
        let workspace = workspace.downgrade();
        cx.spawn(async move |cx| {
            while let Some(event) = events.recv().await {
                // The run may have asked Jev: the Plugins screen's count
                // follows.
                if matches!(event, RunEvent::RunEnd { .. }) {
                    let catalog = host.catalog();
                    let _ = workspace.update(cx, |ws, cx| {
                        ws.apply(HostUpdate::catalog(catalog), cx)
                    });
                }
                // A pull request that keeps pushing takes the commits the
                // turn made.
                if let RunEvent::TurnEnd { run, .. } = &event
                    && host.keeps_pushing(run)
                {
                    let (pusher, run) = (host.clone(), run.clone());
                    host.runtime.spawn_blocking(move || {
                        if let Err(error) = pusher.push_later_commits(&run) {
                            eprintln!(
                                "tau-ui: cannot push the turn: {error:#}"
                            );
                        }
                    });
                }
                // A run the ChatGPT plan stopped says what to do next.
                let refusal = match &event {
                    RunEvent::RunEnd {
                        stop: StopReason::Error(_),
                        ..
                    } => host.refusal(),
                    _ => None,
                };
                if let RunEvent::RunEnd { run, .. } = &event {
                    host.ending
                        .lock()
                        .expect("not poisoned")
                        .insert(run.clone());
                }
                let applied = workspace.update(cx, |ws, cx| {
                    ws.apply(HostUpdate::Event(event.clone()), cx);
                    if let Some(refusal) = &refusal {
                        ws.apply(HostUpdate::PlanRefusal(refusal.clone()), cx);
                    }
                });
                if applied.is_err() {
                    break;
                }
            }
            // Keep the host, and its runtime, alive as long as events flow.
            drop(host);
        })
        .detach();
    }
}

/// Updates repository `name` off the UI thread, saying so in the status
/// bar. An update the user asked for says what went wrong in a dialog;
/// one at startup only in the status bar.
/// Has a model write `run`'s title from `prompt`, keeps it in the store
/// and shows it. Without a client, as in tests, or when the call fails,
/// the run keeps its placeholder. The run's record is written as it
/// starts, well before the model answers.
fn title_in_background(
    host: &Arc<Host>,
    run: &RunId,
    prompt: &str,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    let Some(client) = host.client.lock().expect("not poisoned").clone() else {
        return;
    };
    let model = host
        .choices
        .lock()
        .expect("not poisoned")
        .get(run)
        .map_or_else(
            || host.config.default_model(),
            |choice| choice.model.clone(),
        );
    let job = {
        let (writer, run, prompt) =
            (host.clone(), run.clone(), prompt.to_owned());
        host.runtime.spawn(async move {
            let title = crate::titles::write(&client, &model, &prompt).await?;
            writer.store.set_title(&run.0, &title).await?;
            anyhow::Ok(title)
        })
    };
    let run = run.clone();
    let workspace = workspace.downgrade();
    cx.spawn(async move |cx| {
        let title = match job.await {
            Ok(Ok(title)) => title,
            Ok(Err(error)) => {
                return eprintln!(
                    "tau-ui: cannot title run {}: {error:#}",
                    run.0
                );
            }
            Err(error) => {
                return eprintln!(
                    "tau-ui: cannot title run {}: {error}",
                    run.0
                );
            }
        };
        let _ = workspace.update(cx, |ws, cx| {
            ws.apply(HostUpdate::Titled { run, title }, cx)
        });
    })
    .detach();
}

/// `paths` as the model reads them: `a.rs`, `b.rs` and `c.rs`.
fn code_list(paths: &[String]) -> String {
    let quoted: Vec<String> =
        paths.iter().map(|path| format!("`{path}`")).collect();
    match quoted.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Starts `run`'s resolving turn with `prompt` (ADR 0014): its chat
/// shows the message as tau's, then the host resumes it.
fn resolve(
    host: &Arc<Host>,
    run: &RunId,
    prompt: String,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    workspace.update(cx, |ws, cx| {
        ws.apply(
            HostUpdate::TauTurn {
                run: run.clone(),
                prompt: prompt.clone(),
            },
            cx,
        )
    });
    if let Err(error) = host.start_resolving(run, &prompt) {
        workspace.update(cx, |ws, cx| {
            ws.apply(HostUpdate::ResumeFailed(run.clone()), cx);
            ws.apply(
                HostUpdate::alert(
                    "Could not start resolving the conflicts",
                    format!("{error:#}"),
                ),
                cx,
            );
        });
    }
}

fn update_in_background(
    host: &Arc<Host>,
    name: &str,
    workspace: &Entity<Workspace>,
    asked: bool,
    cx: &mut App,
) {
    {
        let mut updating = host.updating.lock().expect("not poisoned");
        if updating.iter().any(|repo| repo == name) {
            return;
        }
        updating.push(name.to_owned());
    }
    let catalog = host.catalog();
    workspace.update(cx, |ws, cx| ws.apply(HostUpdate::catalog(catalog), cx));
    let job = {
        let (updater, name) = (host.clone(), name.to_owned());
        host.runtime
            .spawn_blocking(move || updater.update_repo(&name))
    };
    let (host, name) = (host.clone(), name.to_owned());
    let workspace = workspace.downgrade();
    cx.spawn(async move |cx| {
        let result = match job.await {
            Ok(result) => result.map_err(|error| format!("{error:#}")),
            Err(error) => Err(error.to_string()),
        };
        host.updating
            .lock()
            .expect("not poisoned")
            .retain(|repo| *repo != name);
        let summary = match &result {
            Ok(updated) if updated.changed() => format!(
                "{name} updated to {}",
                updated.after.get(..7).unwrap_or(&updated.after)
            ),
            Ok(_) => format!("{name} is up to date"),
            Err(_) => format!("{name} was not updated"),
        };
        *host.last_update.lock().expect("not poisoned") = Some(summary);
        let catalog = host.catalog();
        let _ = workspace.update(cx, |ws, cx| {
            ws.apply(HostUpdate::catalog(catalog), cx);
            if let (true, Err(error)) = (asked, result) {
                ws.apply(
                    HostUpdate::alert(
                        format!("Could not update {name}"),
                        error,
                    ),
                    cx,
                );
            }
        });
    })
    .detach();
}

/// A pull request opened from a run, as far as tau pushed it.
#[derive(Debug, Clone)]
struct OpenPr {
    repo: String,
    branch: String,
    title: String,
    keep_pushing: bool,
    /// The branch's commit on GitHub, and the change id of the run's
    /// last commit in it.
    head: String,
    change: String,
}

/// What the host needs to push a run's commits.
struct Pushing {
    project: Project,
    repo: String,
    token: String,
    /// The run's commits, oldest first, after the ones pushed already.
    changes: Vec<tau_vcs::StackChange>,
    /// The local commit the first of them builds on, and its commit on
    /// GitHub.
    local_parent: String,
    remote_parent: String,
    /// The pull request's title, for commit messages.
    title: String,
}

impl Host {
    /// The commits on `run`'s stack, oldest first: the model's, and
    /// those its children landed (ADR 0014).
    fn stack(
        &self,
        run: &RunId,
        project: &Project,
    ) -> anyhow::Result<Vec<tau_vcs::StackChange>> {
        let Some(head) = project.bookmark(&bookmark(run))? else {
            return Ok(Vec::new());
        };
        Ok(project.stack(&head)?)
    }

    /// Writes a pull request draft from `run`: its changed turns as
    /// commits on its repository's default branch, its prompt as the
    /// title and its last answer as the description.
    pub fn prepare_pull_request(
        &self,
        run: &RunId,
    ) -> anyhow::Result<PullRequest> {
        let slot = self.slot_of_run(run)?;
        let repo = self.github_of(&slot.name).ok_or_else(|| {
            anyhow::anyhow!(
                "Pull requests need a repository cloned from GitHub; {} was \
                 not",
                slot.name
            )
        })?;
        let token = github::Token::load(&self.config.credentials)
            .ok_or_else(|| anyhow::anyhow!("Sign in to GitHub first"))?;
        let project = slot
            .project
            .wait()
            .ok_or_else(|| anyhow::anyhow!("{} has no project", slot.name))?;
        let changes = self.stack(run, &project)?;
        let (Some(first), Some(last)) = (changes.first(), changes.last())
        else {
            anyhow::bail!(
                "The run has no commits, so there is nothing to propose"
            );
        };
        let base = project.parent_of(&first.commit_id)?.ok_or_else(|| {
            anyhow::anyhow!("The run's first commit has no parent")
        })?;
        // Whether the default branch moved on under the run, touching
        // what the run touched.
        let _ = project.update(tau_vcs::UpdateFrom::Remote {
            url: &self.github.clone_url(&repo),
            token: Some(&token.token),
        });
        let trunk = project.trunk()?;
        let changed: Vec<String> = project
            .diff(&base, &last.commit_id)?
            .into_iter()
            .map(|file| file.path)
            .collect();
        let mergeable = trunk == base
            || project
                .diff(&base, &trunk)?
                .iter()
                .all(|file| !changed.contains(&file.path));
        let mut commits = Vec::new();
        let mut previous = base.clone();
        for change in &changes {
            let files = project.diff(&previous, &change.commit_id)?;
            commits.push(PrCommit {
                title: change
                    .description
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_owned(),
                added: files.iter().map(|file| file.added as u32).sum(),
                removed: files.iter().map(|file| file.removed as u32).sum(),
            });
            previous = change.commit_id.clone();
        }
        let view = self.history()?.into_iter().find(|view| &view.id == run);
        let prompt = view
            .as_ref()
            .and_then(|view| {
                view.items.iter().find_map(|item| match item {
                    crate::view::Item::User(text) => Some(text.clone()),
                    _ => None,
                })
            })
            .unwrap_or_default();
        let answer = view
            .as_ref()
            .and_then(|view| view.last_text().map(str::to_owned))
            .unwrap_or_default();
        let turns = view.as_ref().map_or(0, |view| view.turn);
        let head = format!(
            "tau/{}-{}",
            branch_slug(&prompt),
            run.0
                .chars()
                .rev()
                .take(6)
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>()
        );
        let draft = PullRequest {
            repo: repo.clone(),
            head,
            base: project.default_branch().unwrap_or_else(|| "main".into()),
            mergeable,
            summary: format!(
                "{} changed {} files over {turns} turns.",
                self.title_of(run)?,
                changed.len()
            ),
            tests: tests_passed(&answer),
            title: pr_title(&prompt),
            body: format!(
                "{answer}\n\n---\nMade with tau from run `{}`.",
                run.0
            ),
            commits,
            draft: true,
            keep_pushing: true,
            state: PrState::Draft,
        };
        self.drafts
            .lock()
            .expect("not poisoned")
            .insert(run.clone(), draft.clone());
        Ok(draft)
    }

    /// The draft written for `run`, if any.
    pub fn draft(&self, run: &RunId) -> Option<PullRequest> {
        self.drafts.lock().expect("not poisoned").get(run).cloned()
    }

    /// Pushes the run's changed turns to the draft's branch and opens
    /// the pull request, asking `reviewers` to review it.
    #[allow(clippy::too_many_arguments)]
    pub fn create_pull_request(
        &self,
        run: &RunId,
        draft: &PullRequest,
        title: &str,
        body: &str,
        as_draft: bool,
        keep_pushing: bool,
        reviewers: &[String],
    ) -> anyhow::Result<(github::Opened, String)> {
        let pushing = self.pushing(run, &draft.repo, title, None)?;
        let (head, change) =
            self.runtime
                .block_on(push(&self.github, &pushing, &draft.head))?;
        let token = pushing.token.clone();
        let opened = self
            .runtime
            .block_on(self.github.open_pull(
                &token,
                &draft.repo,
                title,
                body,
                &draft.head,
                &draft.base,
                as_draft,
            ))
            .map_err(anyhow::Error::msg)?;
        self.runtime
            .block_on(self.github.request_reviewers(
                &token,
                &draft.repo,
                opened.number,
                reviewers,
            ))
            .map_err(anyhow::Error::msg)?;
        self.prs.lock().expect("not poisoned").insert(
            run.clone(),
            OpenPr {
                repo: draft.repo.clone(),
                branch: draft.head.clone(),
                title: title.to_owned(),
                keep_pushing,
                head: head.clone(),
                change,
            },
        );
        Ok((opened, head))
    }

    /// What pushing `run` needs, after `from` (a commit on GitHub and
    /// the change id of the run's last commit in it), or from its base.
    fn pushing(
        &self,
        run: &RunId,
        repo: &str,
        title: &str,
        from: Option<(&str, &str)>,
    ) -> anyhow::Result<Pushing> {
        let slot = self.slot_of_run(run)?;
        let token = github::Token::load(&self.config.credentials)
            .ok_or_else(|| anyhow::anyhow!("Sign in to GitHub first"))?;
        let project = slot.project()?;
        let all = self.stack(run, &project)?;
        let first = all
            .first()
            .ok_or_else(|| anyhow::anyhow!("The run has no commits"))?;
        let base = project.parent_of(&first.commit_id)?.ok_or_else(|| {
            anyhow::anyhow!("The run's first commit has no parent")
        })?;
        let (local_parent, remote_parent, changes) = match from {
            None => (base.clone(), base, all),
            Some((remote, pushed)) => {
                // Changes keep their ids when a landing restacks them.
                let at =
                    all.iter().position(|change| change.change_id == pushed);
                let local = at.map_or(base, |at| all[at].commit_id.clone());
                let later = at.map_or(all.clone(), |at| all[at + 1..].to_vec());
                (local, remote.to_owned(), later)
            }
        };
        let title = title.to_owned();
        Ok(Pushing {
            project,
            repo: repo.to_owned(),
            token: token.token,
            changes,
            local_parent,
            remote_parent,
            title,
        })
    }

    /// Pushes the commits `run` made since its pull request's last
    /// push, if it has one that keeps pushing. Returns whether it pushed.
    pub fn push_later_commits(&self, run: &RunId) -> anyhow::Result<bool> {
        let Some(open) =
            self.prs.lock().expect("not poisoned").get(run).cloned()
        else {
            return Ok(false);
        };
        if !open.keep_pushing {
            return Ok(false);
        }
        let pushing = self.pushing(
            run,
            &open.repo,
            &open.title,
            Some((&open.head, &open.change)),
        )?;
        if pushing.changes.is_empty() {
            return Ok(false);
        }
        let (head, change) = self.runtime.block_on(push(
            &self.github,
            &pushing,
            &open.branch,
        ))?;
        if let Some(open) = self.prs.lock().expect("not poisoned").get_mut(run)
        {
            open.head = head;
            open.change = change;
        }
        Ok(true)
    }

    /// Whether `run` has a pull request that takes its later turns.
    pub fn keeps_pushing(&self, run: &RunId) -> bool {
        self.prs
            .lock()
            .expect("not poisoned")
            .get(run)
            .is_some_and(|open| open.keep_pushing)
    }

    /// How the checks on an opened pull request's head stand.
    pub fn pull_request_checks(
        &self,
        run: &RunId,
    ) -> anyhow::Result<crate::pull_request::Checks> {
        let open = self
            .prs
            .lock()
            .expect("not poisoned")
            .get(run)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("No pull request for the run"))?;
        let token = github::Token::load(&self.config.credentials)
            .ok_or_else(|| anyhow::anyhow!("Sign in to GitHub first"))?;
        self.runtime
            .block_on(self.github.checks(&token.token, &open.repo, &open.head))
            .map_err(anyhow::Error::msg)
    }
}

/// Makes a commit on GitHub for each turn in `pushing`, with the files
/// that turn changed, and points `branch` at the last. Returns the
/// branch's new commit and the run's last turn in it.
async fn push(
    api: &github::Api,
    pushing: &Pushing,
    branch: &str,
) -> anyhow::Result<(String, String)> {
    let (token, repo) = (&pushing.token, &pushing.repo);
    let mut remote = pushing.remote_parent.clone();
    let mut tree = api
        .commit_tree(token, repo, &remote)
        .await
        .map_err(anyhow::Error::msg)?;
    let mut local = pushing.local_parent.clone();
    let mut pushed = String::new();
    for change in &pushing.changes {
        let mut files = Vec::new();
        for file in pushing.project.diff(&local, &change.commit_id)? {
            let blob =
                match pushing.project.file_at(&change.commit_id, &file.path)? {
                    Some((content, executable)) => Some((
                        api.create_blob(token, repo, &content)
                            .await
                            .map_err(anyhow::Error::msg)?,
                        executable,
                    )),
                    None => None,
                };
            files.push(github::TreeFile {
                path: file.path,
                executable: blob
                    .as_ref()
                    .is_some_and(|(_, executable)| *executable),
                blob: blob.map(|(sha, _)| sha),
            });
        }
        tree = api
            .create_tree(token, repo, &tree, &files)
            .await
            .map_err(anyhow::Error::msg)?;
        // The model's own message (ADR 0014).
        let message = match change.description.trim() {
            "" => pushing.title.clone(),
            described => described.to_owned(),
        };
        remote = api
            .create_commit(token, repo, &message, &tree, &remote)
            .await
            .map_err(anyhow::Error::msg)?;
        local = change.commit_id.clone();
        pushed = change.change_id.clone();
    }
    api.set_branch(token, repo, branch, &remote)
        .await
        .map_err(anyhow::Error::msg)?;
    Ok((remote, pushed))
}

/// A pull request's title from the run's prompt: its first line, up to
/// 72 characters, with a capital first letter and no final period.
fn pr_title(prompt: &str) -> String {
    let line = prompt.lines().next().unwrap_or_default().trim();
    let mut title: String = line.chars().take(72).collect();
    if line.chars().count() > 72 {
        title = title.trim_end().to_owned() + "…";
    }
    let title = title.trim_end_matches('.').to_owned();
    let mut chars = title.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => "Changes from tau".into(),
    }
}

/// `14 tests passed`, from an answer that says how many passed.
fn tests_passed(answer: &str) -> Option<String> {
    let words: Vec<&str> = answer.split_whitespace().collect();
    words.windows(2).find_map(|pair| {
        let count: u32 = pair[0]
            .trim_matches(|c: char| !c.is_ascii_digit())
            .parse()
            .ok()?;
        pair[1]
            .starts_with("passed")
            .then(|| format!("{count} tests passed"))
    })
}

/// `agent` with `plugin`, if there is one.
fn with_plugin(
    agent: Agent,
    plugin: Option<impl tau_agent::plugin::Plugin>,
) -> Agent {
    match plugin {
        Some(plugin) => agent.plugin(plugin),
        None => agent,
    }
}

/// Asks GitHub about an opened pull request's checks until they are
/// done, for half an hour at most, and shows each change.
async fn watch_checks(
    host: Arc<Host>,
    run: RunId,
    workspace: gpui::WeakEntity<Workspace>,
    cx: &mut gpui::AsyncApp,
) {
    use crate::pull_request::Checks;
    for _ in 0..120 {
        let job = {
            let (checker, run) = (host.clone(), run.clone());
            host.runtime
                .spawn_blocking(move || checker.pull_request_checks(&run))
        };
        let Ok(Ok(checks)) = job.await else {
            return;
        };
        let updated = workspace.update(cx, |ws, cx| {
            if let Some(pr) = ws.pull_request(&run).cloned()
                && let PrState::Opened { number, url, .. } = pr.state
            {
                ws.apply(
                    HostUpdate::PullRequestState {
                        run: run.clone(),
                        state: PrState::Opened {
                            number,
                            url,
                            checks,
                        },
                    },
                    cx,
                );
            }
        });
        if updated.is_err() || checks != Checks::Running {
            return;
        }
        cx.background_executor()
            .timer(std::time::Duration::from_secs(15))
            .await;
    }
}

/// Refreshes the workspace's catalog once `slot` is imported, if it is
/// importing.
fn refresh_when_imported(
    host: &Arc<Host>,
    slot: &RepoSlot,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    if !matches!(slot.project.peek(), ProjectState::Importing) {
        return;
    }
    let project = slot.project.clone();
    let wait = host.runtime.spawn_blocking(move || project.wait());
    let host = host.clone();
    let workspace = workspace.downgrade();
    cx.spawn(async move |cx| {
        let _ = wait.await;
        let catalog = host.catalog();
        let _ = workspace
            .update(cx, |ws, cx| ws.apply(HostUpdate::catalog(catalog), cx));
    })
    .detach();
}

/// What [`Host::land`] and [`Host::preview_landing`] work with.
struct LandingPlan {
    parent: RunId,
    project: Project,
    parent_workspace: String,
    child_workspace: String,
    /// The child's newest commit, from its bookmark.
    child_head: String,
    /// The parent's workspace, where the landing runs.
    parent_vcs: tau_vcs::Vcs,
}

/// The latest link in `entries`, up to `seq` when given.
fn last_link(entries: &[(i64, String)], seq: Option<i64>) -> Option<Link> {
    entries
        .iter()
        .filter(|(at, _)| seq.is_none_or(|seq| *at <= seq))
        .filter_map(|(_, body)| Link::parse(body))
        .next_back()
}

/// See [`Host::branch_code`].
async fn branch_code(
    store: Store,
    slot: anyhow::Result<RepoSlot>,
    main: RunId,
    fork: RunId,
) -> anyhow::Result<BranchCode> {
    let slot = slot?;
    let project = tokio::task::spawn_blocking(move || slot.project()).await??;
    let record = store
        .run(&fork.0)
        .await?
        .ok_or_else(|| anyhow::anyhow!("No run {}", fork.0))?;
    let RunKind::Fork { parent, fork_seq } = record.kind else {
        anyhow::bail!("{} is not a fork", fork.0);
    };
    let base = last_link(
        &store.plugin_entries(&parent, WORKSPACE_PLUGIN).await?,
        Some(fork_seq),
    )
    .ok_or_else(|| anyhow::anyhow!("The fork point has no commit"))?;
    let head = |run: &RunId| {
        let store = store.clone();
        let run = run.0.to_string();
        let base = base.clone();
        async move {
            let entries = store.plugin_entries(&run, WORKSPACE_PLUGIN).await?;
            anyhow::Ok(last_link(&entries, None).unwrap_or(base))
        }
    };
    let main_head = head(&main).await?;
    let fork_head = head(&fork).await?;
    tokio::task::spawn_blocking(move || {
        // Each at the commit its change has now.
        let [base, main_head, fork_head]: [Link; 3] = project
            .current([base, main_head, fork_head])?
            .try_into()
            .map_err(|_| anyhow::anyhow!("Three links went in"))?;
        let (base, main_head, fork_head) =
            (base.commit_id, main_head.commit_id, fork_head.commit_id);
        let stats = |from: &str, to: &str| -> anyhow::Result<Vec<FileStat>> {
            Ok(project.diff(from, to)?.iter().map(file_stat).collect())
        };
        Ok(BranchCode {
            main: stats(&base, &main_head)?,
            fork: stats(&base, &fork_head)?,
            between: project
                .diff(&main_head, &fork_head)?
                .iter()
                .map(|file| FileChange {
                    stat: file_stat(file),
                    lines: parse_diff(&file.text),
                })
                .collect(),
        })
    })
    .await?
}

fn file_stat(file: &FileDiff) -> FileStat {
    FileStat {
        path: file.path.clone(),
        kind: match file.kind {
            ChangeKind::Added => FileKind::Added,
            ChangeKind::Modified => FileKind::Modified,
            ChangeKind::Removed => FileKind::Removed,
        },
        added: file.added,
        removed: file.removed,
    }
}

/// The saved model choices at `path`, or the defaults with `model` for
/// coder when there are none (or the file does not read).
fn load_settings(path: &std::path::Path, model: &str) -> ModelSettings {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| {
            let mut settings = ModelSettings::default();
            settings
                .set_default("coder", ModelChoice::new(model, Effort::Auto));
            settings
        })
}

/// A new workspace's name: unique, and sorting by when it was made.
/// A name for a new workspace: `slug` (a message's first words), then
/// the time in hex, so it says what it is for and stays unique.
fn workspace_name(slug: &str) -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    format!("{slug}-{millis:x}")
}

/// One stored run, rebuilt as the interface shows it.
async fn stored_view(
    store: &Store,
    record: &tau_store::RunRecord,
) -> anyhow::Result<RunView> {
    // The messages, and in place among them what tau-reasoning and
    // the constitution recorded: the effort each message ran at, and
    // the checks and verdicts on the calls.
    let timeline: Vec<Stored> = store
        .timeline(&record.id)
        .await?
        .into_iter()
        .filter_map(|entry| match entry {
            Entry::Message { body, .. } => {
                serde_json::from_str(&body).ok().map(Stored::Message)
            }
            Entry::Plugin { plugin, body }
                if plugin == tau_constitution::NAME
                    || plugin == LANDING_RECORD
                    || crate::plugins::registry().get(&plugin).is_some() =>
            {
                serde_json::from_str(&body)
                    .ok()
                    .map(|body| Stored::Record { plugin, body })
            }
            // fast-compaction's rewrite carries its ledger.
            Entry::Context { plugin, body }
                if plugin == tau_fast_compaction::NAME =>
            {
                serde_json::from_str(&body)
                    .ok()
                    .map(|body| Stored::Rewrite { plugin, body })
            }
            // What output pruning cut, for the call's card.
            Entry::Plugin { plugin, body }
                if plugin == tau_fast_compaction::NAME =>
            {
                serde_json::from_str::<serde_json::Value>(&body)
                    .ok()
                    .filter(|body| body["kind"] == "output")
                    .map(|body| Stored::Record { plugin, body })
            }
            _ => None,
        })
        .collect();
    let prompt = first_prompt(store, &record.id).await?;
    let mut view = RunView::from_timeline(
        RunId(record.id.clone().into()),
        record
            .title
            .clone()
            .unwrap_or_else(|| crate::titles::placeholder(&prompt)),
        &record.agent,
        &record.model,
        &timeline,
    )
    .in_repo(stored_repo(store, &record.id).await.unwrap_or_default())
    .started(started(&record.created_at));
    let stop = match record.status {
        Status::Done => StopReason::Stop,
        Status::Cancelled => StopReason::Cancelled,
        Status::Failed => StopReason::Error(
            record.error.clone().unwrap_or_else(|| "failed".into()),
        ),
        Status::Limit => StopReason::Error("stopped at a limit".into()),
        Status::Running => {
            StopReason::Error("interrupted: tau closed during the run".into())
        }
    };
    let plugin_cost = store
        .plugin_costs(&record.id)
        .await?
        .iter()
        .map(|cost| cost.cost_usd)
        .sum();
    view.finish_stored(stop, record.cost_usd, plugin_cost);
    if let RunKind::Fork { parent, fork_seq } = &record.kind {
        let turn = store
            .plugin_entries(parent, WORKSPACE_PLUGIN)
            .await?
            .iter()
            .filter(|(seq, _)| seq <= fork_seq)
            .filter_map(|(_, body)| Link::parse(body))
            .map(|link| link.turn)
            .next_back()
            .unwrap_or(0);
        view = view.with_origin(Origin::Fork {
            from: RunId(parent.clone().into()),
            turn,
        });
    }
    Ok(view)
}

/// The words a run was started with: its own first message, not one a
/// fork inherited.
async fn first_prompt(store: &Store, run: &str) -> anyhow::Result<String> {
    let body = store.first_prompt(run).await?;
    let words = body
        .and_then(|body| serde_json::from_str::<Message>(&body).ok())
        .and_then(|message| match message {
            Message::User(user) => Some(crate::view::user_words(&user.content)),
            _ => None,
        });
    Ok(words.unwrap_or_default())
}

/// Past runs, rebuilt from their stored transcripts, each under the
/// repository it recorded: the latest, and the runs `pinned` however old
/// they are.
pub async fn history(
    store: &Store,
    pinned: &[String],
) -> anyhow::Result<Vec<RunView>> {
    let mut records = store.recent_runs(HISTORY).await?;
    for id in pinned {
        if !records.iter().any(|record| &record.id == id)
            && let Some(record) = store.run(id).await?
        {
            records.push(record);
        }
    }
    let mut views = Vec::with_capacity(records.len());
    for record in &records {
        views.push(stored_view(store, record).await?);
    }
    // Sub-agents are left out of the list: they come back under the
    // runs that called them, however they ended.
    let parents: Vec<RunId> =
        views.iter().map(|view| view.id.clone()).collect();
    for parent in parents {
        for record in store.subagents(&parent.0).await? {
            let view = stored_view(store, &record).await?.with_origin(
                Origin::SubAgent {
                    parent: parent.clone(),
                },
            );
            if let Some(parent) =
                views.iter_mut().find(|view| view.id == parent)
            {
                parent.children.push(ChildRun {
                    id: view.id.clone(),
                    title: view.title.clone(),
                    kind: ChildKind::SubAgent,
                    status: view.status.clone(),
                    call: None,
                });
            }
            views.push(view);
        }
    }
    // Each fork is listed under the run it came from, too.
    let forks: Vec<(RunId, ChildRun)> = views
        .iter()
        .filter_map(|view| match &view.origin {
            Origin::Fork { from, .. } => Some((
                from.clone(),
                ChildRun {
                    id: view.id.clone(),
                    title: view.title.clone(),
                    kind: ChildKind::Fork,
                    status: view.status.clone(),
                    call: None,
                },
            )),
            _ => None,
        })
        .collect();
    for (parent, child) in forks {
        if let Some(view) = views.iter_mut().find(|view| view.id == parent) {
            // A finished fork that has not landed waits in its parent's
            // chat; a dropped one is closed, and the chat leaves it out.
            let landed = view.items.iter().any(|item| {
                matches!(item, crate::view::Item::Landed(card) if card.from == child.id)
            });
            if !landed && !child.status.is_live() {
                view.items.push(crate::view::Item::ForkReady {
                    fork: child.id.clone(),
                });
            }
            view.children.push(child);
        }
    }
    Ok(views)
}

/// `2026-09-28T14:03:11.402Z` as `2026-09-28 14:03`.
/// What a plugin that asks Jev says of itself on the Plugins screen:
/// `what` it does, and that it needs a key when there is none.
fn needs_jev(jev: bool, what: &str) -> String {
    if jev {
        format!("{what}, with Jev")
    } else {
        format!("{what}: needs a TypeSafe key (Models)")
    }
}

/// What a run's plugins that ask Jev say as it starts, for those that
/// do not say it themselves yet: fast-compaction and tau-constitution.
/// Each is there with or without a key, so a run without one says they
/// are off. (tau-goal says so only when there is a goal; see
/// `RunView::goal`.)
fn jev_statuses(jev: bool, rules: usize) -> [PluginStatus; 2] {
    const OFF: &str = "off · no TypeSafe key";
    let status = |name: &str, state: String| PluginStatus {
        name: name.into(),
        state,
        tone: Tone::Quiet,
    };
    [
        status(
            tau_fast_compaction::NAME,
            if jev {
                "pruning large outputs · watching the window".into()
            } else {
                OFF.into()
            },
        ),
        status(
            tau_constitution::NAME,
            match (jev, rules) {
                (false, _) => OFF.into(),
                (true, 0) => "no rules".into(),
                (true, 1) => "watching 1 rule".into(),
                (true, n) => format!("watching {n} rules"),
            },
        ),
    ]
}

/// The days of runs the Plugins screen's spend covers.
const SPEND_DAYS: u64 = 30;

/// The date `days` ago, as `YYYY-MM-DD`: a prefix the store's times
/// (`2026-09-28T14:03:11.402Z`) compare after when they are on or after
/// that day.
fn days_ago(days: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    civil_date(now / 86_400 - days.min(now / 86_400))
}

/// The proleptic Gregorian date `days` after 1970-01-01, as
/// `YYYY-MM-DD` (Howard Hinnant's `civil_from_days`).
fn civil_date(days: u64) -> String {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

fn started(created_at: &str) -> String {
    created_at.get(..16).map_or_else(
        || created_at.to_owned(),
        |minute| minute.replace('T', " "),
    )
}

/// Carries out onboarding when no model is configured: Sign in with
/// ChatGPT. Once a sign-in with plan use is saved, `ready` gets its
/// account to build a [`Host`] from, and the host handles sign-ins from
/// then on.
///
/// GitHub sign-ins work before a model is connected, so onboarding can
/// start with them.
pub fn onboard(
    workspace: &Entity<Workspace>,
    model: Option<String>,
    credentials: Credentials,
    cx: &mut App,
    ready: impl Fn(AccountId, &mut App) + 'static,
) {
    let done = std::rc::Rc::new(std::cell::Cell::new(false));
    let connected: accounts::Connected = {
        let done = done.clone();
        std::rc::Rc::new(move |account, cx| {
            // A sign-in without plan usage connects nothing yet.
            if let Some(account) = account {
                done.set(true);
                ready(account, cx)
            }
        })
    };
    let sign_ins = accounts::SignIns::default();
    let api = github::Api::default();
    github::restore(workspace, &credentials, &api, cx);
    cx.subscribe(workspace, move |workspace, event: &WorkspaceEvent, cx| {
        // Once a host runs, it answers.
        if done.get() {
            return;
        }
        let _ = sign_ins.handle(
            event,
            &workspace,
            &credentials,
            model.as_deref(),
            &connected,
            cx,
        ) || github::handle(event, &workspace, &credentials, &api, cx);
    })
    .detach();
}

/// Checks on the host's runtime whether the account just signed in may
/// use its plan, then, if onboarding is on the model step and it may
/// not, says so. Nothing happens signed out.
fn check_eligibility(
    host: &Arc<Host>,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    let Some(check) = host.check_eligibility() else {
        return;
    };
    let (host, workspace) = (host.clone(), workspace.downgrade());
    cx.spawn(async move |cx| {
        let _ = check.await;
        let catalog = host.catalog();
        let refused = host.not_eligible();
        let _ = workspace.update(cx, |ws, cx| {
            let account = catalog
                .models
                .access
                .active_account()
                .map(|account| account.label.clone())
                .unwrap_or_default();
            ws.apply(HostUpdate::catalog(catalog), cx);
            // Onboarding just signed in: say the account cannot share its
            // plan, rather than showing it as connected.
            if let (Some(detail), Route::Setup(SetupStep::Model)) =
                (refused, ws.route())
            {
                ws.update_setup(
                    SetupUpdate::Model(ModelAccess::NotEligible {
                        account,
                        detail,
                    }),
                    cx,
                );
            }
        });
    })
    .detach();
}

/// The refusal's words when `error` says plan use is not available to
/// the account: `403 subscription_sharing_user_not_eligible · request
/// req_…`. `None` for any other failure.
fn not_eligible(error: &tau_ai::chatgpt::ChatGptError) -> Option<String> {
    use tau_ai::chatgpt::ChatGptError;
    if error.recovery() != tau_ai::retry::Recovery::Restricted {
        return None;
    }
    Some(match error {
        ChatGptError::Api(api) => {
            let mut detail = api.status.to_string();
            if let Some(code) = api.code() {
                detail.push_str(&format!(" {code}"));
            }
            if let Some(id) = &api.request_id {
                detail.push_str(&format!(" · request {id}"));
            }
            detail
        }
        other => other.to_string(),
    })
}

/// Clones a GitHub repository on the host's runtime, reporting how it
/// goes to onboarding's list, and adds it to the sidebar once done.
fn clone_into_tau(
    host: &Arc<Host>,
    name: &str,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    let report = |state| {
        SetupUpdate::Clone(RepoClone {
            name: name.to_owned(),
            state,
        })
    };
    workspace.update(cx, |ws, cx| {
        ws.apply(
            HostUpdate::Setup(report(CloneState::Cloning {
                share: 0.3,
                detail: "fetching from GitHub".into(),
            })),
            cx,
        )
    });
    let job = {
        let (cloner, name) = (host.clone(), name.to_owned());
        host.runtime.spawn_blocking(move || {
            let repo = cloner.clone_github(&name)?;
            let main = cloner.main_view(&repo)?;
            anyhow::Ok((repo, main))
        })
    };
    let (host, name) = (host.clone(), name.to_owned());
    let workspace = workspace.downgrade();
    cx.spawn(async move |cx| {
        let cloned = match job.await {
            Ok(result) => result.map_err(|error| format!("{error:#}")),
            Err(error) => Err(error.to_string()),
        };
        let Some(workspace) = workspace.upgrade() else {
            return;
        };
        cx.update(|cx| {
            let state = match cloned {
                Ok((repo, main)) => {
                    if let Some(slot) = host.slot(&repo.name) {
                        refresh_when_imported(&host, &slot, &workspace, cx);
                    }
                    let main = main.map(Box::new);
                    workspace.update(cx, |ws, cx| {
                        ws.apply(HostUpdate::Repo { repo, main }, cx)
                    });
                    CloneState::Ready
                }
                Err(error) => CloneState::Failed(error),
            };
            let update = SetupUpdate::Clone(RepoClone { name, state });
            workspace
                .update(cx, |ws, cx| ws.apply(HostUpdate::Setup(update), cx));
        });
    })
    .detach();
}

/// A branch name's words from the prompt: its first four, or its
/// goal's, lowercase and joined by dashes.
pub fn branch_slug(prompt: &str) -> String {
    let read = crate::plugins::read_prompt(prompt);
    let prompt = read.as_deref().unwrap_or(prompt);
    let words: Vec<&str> = prompt
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .take(4)
        .collect();
    if words.is_empty() {
        "untitled".into()
    } else {
        words.join("-").to_lowercase()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each plugin that asks Jev says how it stands as a run starts:
    /// off without a key; on, the constitution counts its rules.
    #[hegel::test(test_cases = 200)]
    fn jev_plugins_say_whether_they_are_on(tc: hegel::TestCase) {
        use hegel::generators as gs;
        let jev = tc.draw(gs::booleans());
        let rules = tc.draw(gs::integers::<usize>().max_value(20));
        let statuses = jev_statuses(jev, rules);
        let names: Vec<&str> =
            statuses.iter().map(|status| status.name.as_str()).collect();
        assert_eq!(names, [tau_fast_compaction::NAME, tau_constitution::NAME]);
        let off = |status: &PluginStatus| status.state.starts_with("off");
        let [pruning, constitution] = &statuses;
        assert_eq!(off(pruning), !jev);
        assert_eq!(off(constitution), !jev);
        if !jev {
            for status in [pruning, constitution] {
                assert_eq!(status.state, "off · no TypeSafe key");
            }
        }
        if jev && rules > 0 {
            assert!(
                constitution.state.contains(&rules.to_string()),
                "{constitution:?}"
            );
        }
    }

    /// [`civil_date`] agrees with counting the days off one year and
    /// one month at a time.
    #[hegel::test(test_cases = 500)]
    fn civil_dates_match_counting_days(tc: hegel::TestCase) {
        use hegel::generators as gs;
        // Up to the year 2400 or so, past a century that is not a leap
        // year (2100) and one that is (2000).
        let days: u64 = tc.draw(gs::integers::<u64>().max_value(157_000));
        let leap = |year: u64| {
            (year.is_multiple_of(4) && !year.is_multiple_of(100))
                || year.is_multiple_of(400)
        };
        let (mut year, mut left) = (1970, days);
        while left >= if leap(year) { 366 } else { 365 } {
            left -= if leap(year) { 366 } else { 365 };
            year += 1;
        }
        let lengths = [
            31,
            if leap(year) { 29 } else { 28 },
            31,
            30,
            31,
            30,
            31,
            31,
            30,
            31,
            30,
            31,
        ];
        let mut month = 0;
        while left >= lengths[month] {
            left -= lengths[month];
            month += 1;
        }
        assert_eq!(
            civil_date(days),
            format!("{year:04}-{:02}-{:02}", month + 1, left + 1)
        );
    }

    /// A saved list keeps only repositories from GitHub, and opens only
    /// those it keeps: a local checkout listed before is dropped.
    #[test]
    fn a_saved_list_keeps_only_repositories_from_github() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("repos.json");
        let listed = |name: &str, github: Option<&str>| Listed {
            name: name.to_owned(),
            path: dir.path().join(name),
            hidden: false,
            github: github.map(str::to_owned),
            main: None,
        };
        RepoList {
            repos: vec![
                listed("tau-agent", None),
                listed("ascend", Some("cfcosta/ascend")),
            ],
            open: vec!["tau-agent".into(), "ascend".into()],
            ..RepoList::default()
        }
        .save(&path)
        .unwrap();
        let list = RepoList::load(&path);
        let names: Vec<&str> = list
            .repos
            .iter()
            .map(|listed| listed.name.as_str())
            .collect();
        assert_eq!(names, ["ascend"]);
        assert_eq!(list.open, ["ascend"]);
    }

    /// A sub-agent runs on the model its call asked for, else its
    /// caller's. An effort it asked for is kept when the model takes it
    /// and refused when not; without one, it gets the caller's, or auto
    /// where the model does not take that.
    #[hegel::test(test_cases = 300)]
    fn a_sub_agent_runs_on_what_its_call_asked(tc: hegel::TestCase) {
        use hegel::generators::{self as gs, Generator as _};
        use tau_ai::responses::request::ReasoningEffort;
        let ids: Vec<String> =
            plan_models().into_iter().map(|model| model.id).collect();
        let caller_model: String = tc.draw(gs::sampled_from(ids.clone()));
        let caller_effort = tc.draw(
            gs::sampled_from(Effort::offered(&caller_model)).print_as_debug(),
        );
        let caller = ModelChoice::new(caller_model, caller_effort);
        let asked = ChildModel {
            model: tc.draw(gs::optional(gs::sampled_from(ids))),
            effort: tc.draw(gs::optional(
                gs::sampled_from(ReasoningEffort::ALL.to_vec())
                    .print_as_debug(),
            )),
        };
        let model = asked.model.clone().unwrap_or(caller.model.clone());
        let offered = Effort::offered(&model);
        match (child_choice(&caller, &asked), asked.effort) {
            (Ok(choice), Some(effort)) => {
                assert_eq!(choice.model, model);
                assert_eq!(choice.effort, Effort::of(effort));
                assert!(offered.contains(&choice.effort));
            }
            (Err(error), Some(effort)) => {
                assert!(!offered.contains(&Effort::of(effort)));
                assert!(error.to_string().contains(&model), "{error}");
            }
            (Ok(choice), None) => {
                assert_eq!(choice.model, model);
                let kept = if offered.contains(&caller.effort) {
                    caller.effort
                } else {
                    Effort::Auto
                };
                assert_eq!(choice.effort, kept);
            }
            (Err(error), None) => panic!("refused without an effort: {error}"),
        }
    }

    #[test]
    fn workspace_names_start_with_the_slug_and_end_in_hex() {
        let name = workspace_name("fix-the-retry-loop");
        let hex = name
            .strip_prefix("fix-the-retry-loop-")
            .expect("the slug first");
        assert!(!hex.is_empty());
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()), "{name}");
    }

    #[test]
    fn branch_slugs_come_from_the_first_words() {
        assert_eq!(
            branch_slug("Fix the retry loop, please!"),
            "fix-the-retry-loop"
        );
        assert_eq!(branch_slug("  ?! "), "untitled");
        assert_eq!(
            branch_slug("/goal --continuations 3 all tests pass"),
            "all-tests-pass"
        );
    }

    #[test]
    fn only_restricted_refusals_are_not_eligible() {
        use tau_ai::chatgpt::{ApiError, ChatGptError};
        let refusal = |status, body: &str| {
            ChatGptError::Api(Box::new(ApiError::new(
                status,
                Some("req_1".into()),
                body.as_bytes(),
            )))
        };
        let not_eligible = refusal(
            403,
            r#"{"error":{"code":"subscription_sharing_user_not_eligible"}}"#,
        );
        assert_eq!(
            super::not_eligible(&not_eligible).as_deref(),
            Some("403 subscription_sharing_user_not_eligible · request req_1")
        );
        let limit = refusal(
            429,
            r#"{"error":{"code":"subscription_sharing_usage_limit_exceeded"}}"#,
        );
        assert_eq!(super::not_eligible(&limit), None);
        assert_eq!(super::not_eligible(&ChatGptError::PlanUsageDisabled), None);
    }
}
