//! Runs real agents behind the workspace: the seams from
//! [`crate::workspace`] wired to `tau-agent`.
//!
//! The host owns a tokio runtime beside GPUI's executor. A new run starts
//! on that runtime; a task reads its events and sends them over a channel
//! the workspace drains on the UI thread. Steering and cancelling go the
//! other way through each run's [`RunControl`].
//!
//! Runs do not work in the user's checkouts. The host copies each
//! repository into a [`Project`] under `$XDG_DATA_HOME/tau/repos`, and
//! each run gets a jj workspace there ([`RunWorkspace`]) with a commit
//! per turn, so a fork starts from a turn's conversation and code. The
//! repositories tau lists, and which the sidebar shows open, are kept in
//! `repos.json`; each run records its repository ([`RepoTag`]), so past
//! runs come back from the store under theirs.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use futures_util::StreamExt;
use gpui::{App, Entity};
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
    Delegate,
    FileDiff,
    Identity,
    Landing,
    Link,
    Project,
    RunWorkspace,
    VcsPlugin,
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
    memory::{Memories, stale_on_commit},
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
    /// The checkout runs work on. The host clones it into a project of
    /// its own and gives each run a workspace there; if it cannot (not
    /// a Git repository, no `git`), runs work in it directly.
    pub root: PathBuf,
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

    /// The project directory for `root`.
    pub fn project_dir(&self) -> PathBuf {
        self.project_dir_of(&self.root)
    }

    /// The project directory for the checkout at `path`: its name and a
    /// hash of its full path, so two checkouts with one name get two
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
}

impl RepoList {
    fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    /// Lists the checkout at `path`, or lists it again if it was
    /// removed, and returns its name: the directory's, made unique.
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
        });
        name
    }
}

/// The plugin name under which a run records its repository.
pub const REPO_PLUGIN: &str = "repo";

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
    /// Runs work in the checkout itself, for this reason.
    Checkout(String),
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

    /// The project, once the import is over; `None` if runs work in the
    /// checkout.
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

/// A listed repository: its checkout, and the project runs in it work
/// in.
#[derive(Clone)]
struct RepoSlot {
    name: String,
    path: PathBuf,
    project: Arc<ProjectSlot>,
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
    /// The checkout the host started in; runs that recorded no
    /// repository belong to it.
    home: String,
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
    /// The user's model choices, as loaded and last saved.
    settings: Arc<Mutex<ModelSettings>>,
    events: mpsc::UnboundedSender<RunEvent>,
}

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
    in bash. Be concise, and say which tests you ran.";

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
        // Copying checkouts can take a while; the window opens first.
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
        for listed in listed {
            let slot = RepoSlot {
                name: listed.name,
                path: listed.path,
                project: ProjectSlot::new(ProjectState::Importing),
            };
            host.spawn_import(&slot)?;
            slots.push(slot);
        }
        *host.repos.lock().expect("not poisoned") = slots;
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
                project.set(match Project::open_or_import(&source, dir, identity()) {
                    Ok(project) => ProjectState::Ready(project),
                    Err(error) => {
                        eprintln!(
                            "tau-ui: runs will work in {source} itself: {error:#}"
                        );
                        ProjectState::Checkout(format!("{error:#}"))
                    }
                });
            })?;
        Ok(())
    }

    /// A host over an agent built elsewhere: another model, other
    /// plugins, or a scripted model in tests. Runs work in `config.root`
    /// until [`Self::with_project`] gives them workspaces.
    pub fn with_agent(
        runtime: Runtime,
        agent: Agent,
        store: Store,
        config: HostConfig,
    ) -> (Self, mpsc::UnboundedReceiver<RunEvent>) {
        let (events, receiver) = mpsc::unbounded_channel();
        let settings = load_settings(&config.settings, &config.default_model());
        // The checkout the host starts in is always listed.
        let mut list = RepoList::load(&config.repo_list);
        let home = list.list(&config.root);
        if let Err(error) = list.save(&config.repo_list) {
            eprintln!("tau-ui: cannot save the repository list: {error:#}");
        }
        let slot = RepoSlot {
            name: home.clone(),
            path: canonical(&config.root),
            project: ProjectSlot::new(ProjectState::Checkout(
                "no project was given".into(),
            )),
        };
        let host = Self {
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
            repos: Arc::new(Mutex::new(vec![slot])),
            home,
            list: Arc::new(Mutex::new(list)),
            run_repos: Arc::default(),
            updating: Arc::default(),
            last_update: Arc::default(),
            drafts: Mutex::default(),
            prs: Arc::default(),
            runs: Arc::default(),
            workspaces: Arc::default(),
            events,
        };
        (host, receiver)
    }

    /// Gives each run in the checkout a workspace in `project`, and a
    /// commit per turn.
    pub fn with_project(self, project: Project) -> Self {
        self.home_slot().project.set(ProjectState::Ready(project));
        self
    }

    fn home_slot(&self) -> RepoSlot {
        self.slot(&self.home)
            .expect("the checkout's repository is always there")
    }

    fn slot(&self, name: &str) -> Option<RepoSlot> {
        self.repos
            .lock()
            .expect("not poisoned")
            .iter()
            .find(|slot| slot.name == name)
            .cloned()
    }

    /// The repository `run` works in: as this session started it, as
    /// the store recorded it, or the checkout's.
    fn slot_of_run(&self, run: &RunId) -> RepoSlot {
        let known = self
            .run_repos
            .lock()
            .expect("not poisoned")
            .get(run)
            .cloned();
        let name = known.or_else(|| {
            self.runtime.block_on(stored_repo(&self.store, &run.0))
        });
        name.and_then(|name| self.slot(&name))
            .unwrap_or_else(|| self.home_slot())
    }

    /// The checkout's project, waiting for the import if it is still
    /// going; `None` when runs work in the checkout.
    pub fn project(&self) -> Option<Project> {
        self.home_slot().project.wait()
    }

    /// A listed repository's project, waiting for its import.
    pub fn project_of(&self, repo: &str) -> Option<Project> {
        self.slot(repo)?.project.wait()
    }

    /// Where sign-ins and keys are kept.
    pub fn credentials(&self) -> &Credentials {
        &self.config.credentials
    }

    /// The name the checkout is listed under.
    pub fn home(&self) -> &str {
        &self.home
    }

    /// Whether a repository is still being imported.
    pub fn is_importing(&self) -> bool {
        self.repos
            .lock()
            .expect("not poisoned")
            .iter()
            .any(|slot| matches!(slot.project.peek(), ProjectState::Importing))
    }

    /// Lists the checkout at `path` (`~` for the home directory) and
    /// starts copying it. Returns it as the sidebar shows it.
    pub fn add_repo(&self, path: &str) -> anyhow::Result<Repo> {
        let path = crate::repos::expand_home(path);
        if !path.is_dir() {
            anyhow::bail!("{} is not a directory", path.display());
        }
        let name = {
            let mut list = self.list.lock().expect("not poisoned");
            let name = list.list(&path);
            list.save(&self.config.repo_list)?;
            name
        };
        if self.slot(&name).is_none() {
            let slot = RepoSlot {
                name: name.clone(),
                path: canonical(&path),
                project: ProjectSlot::new(ProjectState::Importing),
            };
            self.spawn_import(&slot)?;
            self.repos.lock().expect("not poisoned").push(slot);
        }
        Ok(Repo::new(name, canonical(&path).display().to_string()))
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
    /// sign-in, unless it was cloned before, and lists it like a
    /// checkout. Blocks for the clone.
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
        let repo = self.add_repo(&dir.to_string_lossy())?;
        let mut list = self.list.lock().expect("not poisoned");
        if let Some(listed) = list
            .repos
            .iter_mut()
            .find(|listed| listed.name == repo.name)
        {
            listed.github = Some(full_name.to_owned());
        }
        list.save(&self.config.repo_list)?;
        Ok(repo)
    }

    /// Brings new commits into a repository's project: from GitHub for
    /// one cloned from there, else from its checkout. New runs start
    /// from the new trunk; runs going on keep their code. Blocks.
    pub fn update_repo(&self, name: &str) -> anyhow::Result<tau_vcs::Updated> {
        let slot = self
            .slot(name)
            .ok_or_else(|| anyhow::anyhow!("No repository {name}"))?;
        let project = slot.project.wait().ok_or_else(|| {
            anyhow::anyhow!(
                "Runs in {name} work in its checkout, which is always current"
            )
        })?;
        let updated = match self.github_of(name) {
            Some(full_name) => {
                let token = github::Token::load(&self.config.credentials);
                project.update(tau_vcs::UpdateFrom::Remote {
                    url: &self.github.clone_url(&full_name),
                    token: token.as_ref().map(|token| token.token.as_str()),
                })
            }
            None => project.update(tau_vcs::UpdateFrom::Checkout(&slot.path)),
        };
        Ok(updated?)
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

    /// Stores a change to `run`'s goal for tau-goal, which reads it at
    /// its next check, or when the run starts again.
    pub fn goal_record(
        &self,
        run: &RunId,
        record: &tau_goal::Record,
    ) -> anyhow::Result<()> {
        let entry = Entry::Plugin {
            plugin: tau_goal::NAME.into(),
            body: record.to_value().to_string(),
        };
        self.runtime.block_on(self.store.append_turn(
            &run.0,
            &[entry],
            tau_store::TurnUsage::default(),
        ))?;
        Ok(())
    }

    /// Remembers which repositories the sidebar shows open.
    pub fn set_open_repos(&self, open: Vec<String>) -> anyhow::Result<()> {
        let mut list = self.list.lock().expect("not poisoned");
        list.open = open;
        list.save(&self.config.repo_list)
    }

    /// Whether `run` is still going.
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
        Some(self.slot_of_run(run).project.wait()?.workspace_dir(&name))
    }

    /// What the workspace shows beyond runs: the agent's plugins and the
    /// store.
    pub fn catalog(&self) -> Catalog {
        let mut plugins = vec![PluginInfo {
            name: "tau-tools".into(),
            description: "read bash edit write grep find ls".into(),
            seams: vec![Seam::Tools],
            spend: 0.0,
            screen: None,
        }];
        let home = self.home_slot();
        let state = home.project.peek();
        if matches!(state, ProjectState::Ready(_)) {
            plugins.extend([
                PluginInfo {
                    name: "tau-vcs".into(),
                    description: "status diff log show describe commit new \
                                  restore undo, on the run's workspace"
                        .into(),
                    seams: vec![Seam::Tools],
                    spend: 0.0,
                    screen: None,
                },
                PluginInfo {
                    name: "workspace".into(),
                    description: "A jj workspace per run, and a commit per \
                                  turn to fork from"
                        .into(),
                    seams: vec![Seam::Start],
                    spend: 0.0,
                    screen: None,
                },
            ]);
        }
        if self.jev().is_some() {
            plugins.push(PluginInfo {
                name: tau_fast_compaction::NAME.into(),
                description: "Prunes large bash outputs as they arrive, and \
                              stale tool history, with Jev"
                    .into(),
                seams: vec![Seam::Start, Seam::Rewrite],
                spend: 0.0,
                screen: Some(PluginScreen::Ledger),
            });
        }
        plugins.push(PluginInfo {
            name: "tau-compaction".into(),
            description: "Summarizes the context when it nears the window"
                .into(),
            seams: vec![Seam::Start, Seam::Rewrite],
            spend: 0.0,
            screen: None,
        });
        let source = match &state {
            ProjectState::Ready(project) => format!(
                "{} · {} → {}",
                self.access_label(),
                self.config.root.display(),
                project.root().display()
            ),
            _ => format!(
                "{} · {}",
                self.access_label(),
                self.config.root.display()
            ),
        };
        let slots = self.repos.lock().expect("not poisoned").clone();
        let list = self.list.lock().expect("not poisoned").clone();
        // Any import going on shows; else how the checkout's runs work.
        let importing = slots.iter().find(|slot| {
            matches!(slot.project.peek(), ProjectState::Importing)
        });
        let updating =
            self.updating.lock().expect("not poisoned").first().cloned();
        let project = match (importing, state) {
            (Some(slot), _) => ProjectStatus::Importing(slot.name.clone()),
            (None, _) if updating.is_some() => {
                ProjectStatus::Updating(updating.unwrap_or_default())
            }
            (None, ProjectState::Ready(_)) => {
                ProjectStatus::Ready(home.name.clone())
            }
            (None, ProjectState::Checkout(why)) => ProjectStatus::Checkout(why),
            (None, ProjectState::Importing) => {
                ProjectStatus::Importing(home.name.clone())
            }
        };
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
                repo.constitution = self.repo_constitution(slot);
                repo.memory = self.memories.catalog(&self.memory_dir(slot));
                Some(repo)
            })
            .collect();
        let rules: usize =
            repos.iter().map(|repo| repo.constitution.rules.len()).sum();
        let jev = self.config.credentials.jev_key().is_some();
        plugins.push(PluginInfo {
            name: tau_constitution::NAME.into(),
            description: if jev {
                format!(
                    "{rules} rules across your repositories, checked with Jev"
                )
            } else {
                "Checks calls against each repository's rules: needs a \
                 TypeSafe key (Models)"
                    .into()
            },
            seams: vec![Seam::BeforeTool, Seam::BeforeStop],
            spend: 0.0,
            screen: Some(PluginScreen::Constitution),
        });
        plugins.push(PluginInfo {
            name: tau_goal::NAME.into(),
            description: if jev {
                "Keeps a conversation going until its /goal holds, checked \
                 with Jev"
                    .into()
            } else {
                "Keeps a conversation going until its /goal holds: needs a \
                 TypeSafe key (Models)"
                    .into()
            },
            seams: vec![Seam::Start, Seam::AfterTool, Seam::BeforeStop],
            spend: 0.0,
            screen: None,
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
        });
        Catalog {
            agent: "coder".into(),
            agent_source: Some(source),
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
    fn agent_for_run(
        &self,
        choice: &ModelChoice,
        repo: &RepoSlot,
        workspace: Option<String>,
    ) -> anyhow::Result<(Agent, Option<String>)> {
        if self.account().is_none() {
            anyhow::bail!(
                "tau has no ChatGPT plan to run on. Sign in with ChatGPT and \
                 enable plan use on the Models screen."
            );
        }
        let mut agent = self
            .base
            .lock()
            .expect("not poisoned")
            .clone()
            .model(&choice.model);
        let jev = self.jev();
        // "auto" leaves the effort to tau-reasoning, when it can ask Jev.
        if choice.effort == Effort::Auto
            && let Some(jev) = &jev
        {
            agent = agent.plugin(tau_reasoning::Reasoning::new(jev.clone()));
        }
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
                archive_dir: self.archive_dir(repo),
                ..tau_fast_compaction::Settings::default()
            };
            agent = agent.plugin(
                tau_fast_compaction::FastCompaction::shared(jev.clone())
                    .settings(settings),
            );
        }
        let agent = agent.plugin(compaction).plugin(RepoTag(repo.name.clone()));
        // The repository's rules, checked with Jev when there is a key.
        // Rules that cannot be read fail the run: they are never skipped.
        let constitution = match jev {
            Some(jev) => {
                Some(ConstitutionPlugin::live(jev, self.constitution(repo)?))
            }
            None => None,
        };
        // The conversation's goal, checked with Jev too; after the
        // constitution, whose hold of a stop wins.
        let goal = self.jev().map(tau_goal::GoalPlugin::new);
        let memory = self.memory_plugin(repo);
        let Some(project) = repo.project.wait() else {
            let tools = CodingTools::new(Root::new(repo.path.clone()));
            let agent = with_plugin(agent.plugin(tools), memory);
            let agent = with_plugin(with_plugin(agent, constitution), goal);
            return Ok((agent, None));
        };
        let name = workspace.unwrap_or_else(workspace_name);
        // A run and its sub-agents work the same way, each in its own
        // workspace: tools, memory and the repository's rules. Only the
        // run itself keeps the conversation's goal and can delegate, so
        // sub-agents do not nest.
        let on_workspace = {
            let memory = memory.clone();
            let constitution = constitution.clone();
            move |agent: Agent, workspace: RunWorkspace| {
                // Notes about the files a turn's commit changed may be
                // stale.
                let workspace = match &memory {
                    Some(memory) => {
                        workspace.on_commit(stale_on_commit(memory.clone()))
                    }
                    None => workspace,
                };
                let agent = agent
                    .plugin(CodingTools::new(Root::new(workspace.dir())))
                    .plugin(VcsPlugin::new(workspace.vcs().clone()))
                    .plugin(workspace);
                let agent = with_plugin(agent, memory.clone());
                with_plugin(agent, constitution.clone())
            }
        };
        let workspace = RunWorkspace::new(project.clone(), &name, identity())?;
        let delegate = {
            let (base, on_workspace) = (agent.clone(), on_workspace.clone());
            Delegate::new(workspace.clone(), identity(), move |child| {
                Ok(on_workspace(base.clone(), child))
            })
        };
        let agent = on_workspace(agent.tool(delegate), workspace);
        Ok((with_plugin(agent, goal), Some(name)))
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

    /// What a repository's constitution is stored under: its checkout,
    /// as the repository list keeps it.
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
            max_continuations: loaded.max_holds,
            error,
        }
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

    /// Starts a run and returns its view, ready to be pushed into the
    /// workspace before its first event arrives.
    /// It works in `repo`, or in the checkout's repository when no
    /// repository has that name.
    pub fn start(
        &self,
        prompt: &str,
        choice: &ModelChoice,
        repo: &str,
    ) -> anyhow::Result<RunView> {
        let repo = self.slot(repo).unwrap_or_else(|| self.home_slot());
        // An effort the model does not take falls back to auto.
        let choice = &choice.clone().fitted();
        let (agent, workspace) = self.agent_for_run(choice, &repo, None)?;
        let _guard = self.runtime.enter();
        let run = agent.start(prompt, &self.store);
        let id = self.track(run, workspace, choice, &repo.name);
        Ok(self.view(id, prompt))
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
        let repo = self.slot_of_run(run);
        if repo.project.wait().is_none() {
            anyhow::bail!(
                "Forking needs a project: {} could not be copied",
                repo.path.display()
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
        // An effort the model does not take falls back to auto.
        let choice = &choice.clone().fitted();
        let (agent, workspace) = self.agent_for_run(choice, &repo, None)?;
        let _guard = self.runtime.enter();
        let forked = agent
            .fork(&Checkpoint::at(source.clone(), seq))
            .after_turn(link.turn)
            .start(prompt, &self.store);
        let id = self.track(forked, workspace, choice, &repo.name);
        Ok(self.view(id, prompt).with_origin(Origin::Fork {
            from: source,
            turn: link.turn,
        }))
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
        let repo = self.slot_of_run(run);
        // The workspace its last turn worked in, which still has its
        // files.
        let known = self
            .workspaces
            .lock()
            .expect("not poisoned")
            .get(run)
            .cloned();
        let workspace = match known {
            Some(name) => Some(name),
            None => self.link(run, None)?.map(|(_, link)| link.workspace),
        };
        // An effort the model does not take falls back to auto.
        let choice = &choice.clone().fitted();
        let (agent, workspace) =
            self.agent_for_run(choice, &repo, workspace)?;
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
        Ok(self.runtime.block_on(landing.parent_vcs.land(
            &landing.child_head,
            bookmark(&landing.parent),
            false,
        ))?)
    }

    /// Lands `child` on its parent (ADR 0009): restacks its changes onto
    /// the parent's newest commit, records them as links in the parent,
    /// and closes the child: its workspace goes, and so does its
    /// bookmark. Both runs must be idle.
    pub fn land(&self, child: &RunId) -> anyhow::Result<Landing> {
        let plan = self.landing(child)?;
        let landing = self.runtime.block_on(plan.parent_vcs.land(
            &plan.child_head,
            bookmark(&plan.parent),
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
            title: title(&self.stored_prompt(child)?),
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
        let project = self
            .slot_of_run(child)
            .project
            .wait()
            .ok_or_else(|| anyhow::anyhow!("The runs have no project"))?;
        self.no_open_children(child, &project)?;
        if let Some(head) = project.bookmark(&bookmark(child))? {
            let keep = match project.bookmark(&bookmark(&parent))? {
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
            RunKind::Fork { parent, .. } | RunKind::Subagent { parent } => {
                Ok(RunId(parent.into()))
            }
            RunKind::Root => anyhow::bail!("{} has no parent", child.0),
        }
    }

    /// Refuses when `run` has children still open: running, or holding
    /// changes `run` does not have. They land or are dropped first.
    fn no_open_children(
        &self,
        run: &RunId,
        project: &Project,
    ) -> anyhow::Result<()> {
        let children: Vec<String> = self
            .runtime
            .block_on(self.store.recent_runs(1000))?
            .into_iter()
            .filter(|other| match &other.kind {
                RunKind::Fork { parent, .. } | RunKind::Subagent { parent } => {
                    parent.as_str() == &*run.0
                }
                RunKind::Root => false,
            })
            .map(|other| other.id)
            .collect();
        let head = project.bookmark(&bookmark(run))?;
        let mut open = Vec::new();
        for id in children {
            let child = RunId(id.clone().into());
            let unlanded = match (project.bookmark(&bookmark(&child))?, &head) {
                (None, _) => false,
                (Some(_), None) => true,
                (Some(theirs), Some(ours)) => {
                    !project.is_ancestor(&theirs, ours)?
                }
            };
            if self.is_running(&child) || unlanded {
                open.push(id);
            }
        }
        if !open.is_empty() {
            anyhow::bail!(
                "{} has children that have not landed ({}); land or drop \
                 them first",
                run.0,
                open.join(", ")
            );
        }
        Ok(())
    }

    /// The words `run` was started with, from its stored transcript.
    fn stored_prompt(&self, run: &RunId) -> anyhow::Result<String> {
        self.runtime.block_on(async {
            let kind = self
                .store
                .run(&run.0)
                .await?
                .map_or(RunKind::Root, |record| record.kind);
            let messages: Vec<Message> = self
                .store
                .transcript(&run.0)
                .await?
                .into_iter()
                .filter_map(|entry| match entry {
                    Entry::Message { body, .. } => {
                        serde_json::from_str(&body).ok()
                    }
                    _ => None,
                })
                .collect();
            anyhow::Ok(prompt_of(&kind, messages.iter()))
        })
    }

    /// Everything landing `child` needs, once both runs are idle.
    fn landing(&self, child: &RunId) -> anyhow::Result<LandingPlan> {
        let parent = self.parent_of(child)?;
        for run in [child, &parent] {
            if self.is_running(run) {
                anyhow::bail!("{} is still running; land once it stops", run.0);
            }
        }
        let project = self
            .slot_of_run(child)
            .project
            .wait()
            .ok_or_else(|| anyhow::anyhow!("The runs have no project"))?;
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
        self.no_open_children(child, &project)?;
        let parent_workspace = workspace_of(&parent)?;
        let child_workspace = workspace_of(child)?;
        // Opening a workspace that is gone would make a new one on
        // trunk; landing there would lose the parent's work.
        if !project.workspaces()?.contains(&parent_workspace) {
            anyhow::bail!("The parent's workspace is gone");
        }
        let child_head =
            project.bookmark(&bookmark(child))?.ok_or_else(|| {
                anyhow::anyhow!("{} has no changes to land", child.0)
            })?;
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
        let Some(project) = self.slot_of_run(run).project.wait() else {
            return Ok(());
        };
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
            self.slot_of_run(main).project,
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
    pub fn history(&self) -> anyhow::Result<Vec<RunView>> {
        self.runtime.block_on(history(&self.store, &self.home))
    }

    /// Follows a started run: its control, its workspace, and a task
    /// that forwards its events.
    fn track(
        &self,
        mut run: tau_agent::agent::Run,
        workspace: Option<String>,
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
        if let Some(name) = workspace {
            self.workspaces
                .lock()
                .expect("not poisoned")
                .insert(id.clone(), name);
        }
        let events = self.events.clone();
        let runs = self.runs.clone();
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
        });
        id
    }

    fn view(&self, id: RunId, prompt: &str) -> RunView {
        let choice = self
            .choices
            .lock()
            .expect("not poisoned")
            .get(&id)
            .cloned()
            .unwrap_or_else(|| {
                ModelChoice::new(self.config.default_model(), Effort::Auto)
            });
        let repo = self.slot_of_run(&id);
        let mut view = RunView::new(id, title(prompt), "coder", &choice.model)
            .in_repo(repo.name.clone())
            .started("just now");
        view.push_user(prompt);
        view.limits = ViewLimits {
            max_turns: Some(MAX_TURNS),
            ..ViewLimits::default()
        };
        view.context = ContextWindow {
            window: find(&choice.model).map(|model| model.context_window),
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
                    _ => repo.path.display().to_string(),
                },
                set_by: workspace.as_ref().map(|_| "workspace".to_owned()),
            },
        ];
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
        if self.jev().is_some() {
            view.plugins.push(PluginStatus {
                name: tau_fast_compaction::NAME.into(),
                state: "pruning large outputs · watching the window".into(),
                tone: Tone::Quiet,
            });
        }
        view.plugins.push(PluginStatus {
            name: "tau-compaction".into(),
            state: "watching the window".into(),
            tone: Tone::Quiet,
        });
        view.plugins.push(PluginStatus {
            name: tau_memory::plugin::NAME.into(),
            state: match self
                .memories
                .catalog(&self.memory_dir(&repo))
                .notes
                .len()
            {
                0 => "no notes yet".into(),
                1 => "1 note".into(),
                n => format!("{n} notes"),
            },
            tone: Tone::Quiet,
        });
        let rules = self
            .constitution(&repo)
            .map_or(0, |live| live.get().rules.len());
        view.plugins.push(PluginStatus {
            name: tau_constitution::NAME.into(),
            state: match (self.config.credentials.jev_key(), rules) {
                (None, _) => "off · no TypeSafe key".into(),
                (Some(_), 0) => "no rules".into(),
                (Some(_), 1) => "watching 1 rule".into(),
                (Some(_), n) => format!("watching {n} rules"),
            },
            tone: Tone::Quiet,
        });
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
                        workspace.update(cx, |ws, cx| ws.apply(HostUpdate::Run(Box::new(view)), cx))
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
                WorkspaceEvent::Goal { run, record } => {
                    if let Err(error) = handler.goal_record(run, record) {
                        eprintln!("tau-ui: cannot save the goal: {error:#}");
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
                    if let Err(error) = handler.resume(run, prompt, model) {
                        workspace.update(cx, |ws, cx| {
                            ws.apply(HostUpdate::ResumeFailed(run.clone()), cx);
                            ws.apply(HostUpdate::alert("Could not go on with the run", format!("{error:#}")), cx)
                        })
                    }
                }
                WorkspaceEvent::Fork {
                    run,
                    turn,
                    prompt,
                    model,
                } => match handler.fork(run, *turn, prompt, model) {
                    Ok(view) => {
                        workspace.update(cx, |ws, cx| ws.apply(HostUpdate::Run(Box::new(view)), cx))
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
                    let landed =
                        handler.land(run).map_err(|error| format!("{error:#}"));
                    workspace.update(cx, |ws, cx| ws.apply(HostUpdate::Landed { run: run.clone(), landing: landed }, cx));
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
                WorkspaceEvent::AddRepo { path } => {
                    match handler.add_repo(path) {
                        Ok(repo) => {
                            if let Some(slot) = handler.slot(&repo.name) {
                                refresh_when_imported(
                                    &handler, &slot, &workspace, cx,
                                );
                            }
                            workspace.update(cx, |ws, cx| ws.apply(HostUpdate::Repo(repo), cx))
                        }
                        Err(error) => workspace.update(cx, |ws, cx| {
                            ws.apply(HostUpdate::alert("Could not add the repository", format!("{error:#}")), cx)
                        }),
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
                WorkspaceEvent::Steer { run, text } => handler.steer(run, text),
                WorkspaceEvent::Cancel { run } => handler.cancel(run),
                other => eprintln!("tau-ui: not handled yet: {other:?}"),
                }
            },
        )
        .detach();
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
                // A pull request that keeps pushing takes the turn.
                if let RunEvent::TurnEnd { run, .. } = &event
                    && host.keeps_pushing(run)
                {
                    let (pusher, run) = (host.clone(), run.clone());
                    host.runtime.spawn_blocking(move || {
                        if let Err(error) = pusher.push_later_turns(&run) {
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
    /// The branch's commit on GitHub, and the run's last turn in it.
    head: String,
    turn: u32,
}

/// What the host needs to push a run's commits.
struct Pushing {
    project: Project,
    repo: String,
    token: String,
    /// Changed turns, in order, after the ones pushed already.
    links: Vec<Link>,
    /// The local commit the first of them builds on, and its commit on
    /// GitHub.
    local_parent: String,
    remote_parent: String,
    /// The pull request's title, for commit messages.
    title: String,
}

impl Host {
    /// The turns `run` took that changed files, its own and the ones it
    /// inherits as a fork, in order, each at the commit its change has
    /// now.
    fn changed_turns(
        &self,
        run: &RunId,
        project: &Project,
    ) -> anyhow::Result<Vec<Link>> {
        let bodies = self
            .runtime
            .block_on(self.store.records(&run.0, WORKSPACE_PLUGIN))?;
        Ok(project.current(
            bodies
                .iter()
                .filter_map(|body| Link::parse(body))
                .filter(|link| link.changed),
        )?)
    }

    /// Writes a pull request draft from `run`: its changed turns as
    /// commits on its repository's default branch, its prompt as the
    /// title and its last answer as the description.
    pub fn prepare_pull_request(
        &self,
        run: &RunId,
    ) -> anyhow::Result<PullRequest> {
        let slot = self.slot_of_run(run);
        let repo = self.github_of(&slot.name).ok_or_else(|| {
            anyhow::anyhow!(
                "Pull requests need a repository added from GitHub; {} is a \
                 local checkout",
                slot.name
            )
        })?;
        let token = github::Token::load(&self.config.credentials)
            .ok_or_else(|| anyhow::anyhow!("Sign in to GitHub first"))?;
        let project = slot
            .project
            .wait()
            .ok_or_else(|| anyhow::anyhow!("{} has no project", slot.name))?;
        let links = self.changed_turns(run, &project)?;
        let (Some(first), Some(last)) = (links.first(), links.last()) else {
            anyhow::bail!(
                "The run changed no files, so there is nothing to propose"
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
        for link in &links {
            let files = project.diff(&previous, &link.commit_id)?;
            commits.push(PrCommit {
                title: format!("Turn {}", link.turn),
                added: files.iter().map(|file| file.added as u32).sum(),
                removed: files.iter().map(|file| file.removed as u32).sum(),
            });
            previous = link.commit_id.clone();
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
            title(&prompt),
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
                title(&prompt),
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
        let (head, turn) =
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
                turn,
            },
        );
        Ok((opened, head))
    }

    /// What pushing `run` needs, after `from` (a commit on GitHub and
    /// the run's last turn in it), or from its base.
    fn pushing(
        &self,
        run: &RunId,
        repo: &str,
        title: &str,
        from: Option<(&str, u32)>,
    ) -> anyhow::Result<Pushing> {
        let slot = self.slot_of_run(run);
        let token = github::Token::load(&self.config.credentials)
            .ok_or_else(|| anyhow::anyhow!("Sign in to GitHub first"))?;
        let project = slot
            .project
            .wait()
            .ok_or_else(|| anyhow::anyhow!("{} has no project", slot.name))?;
        let all = self.changed_turns(run, &project)?;
        let first = all
            .first()
            .ok_or_else(|| anyhow::anyhow!("The run changed no files"))?;
        let base = project.parent_of(&first.commit_id)?.ok_or_else(|| {
            anyhow::anyhow!("The run's first commit has no parent")
        })?;
        let (local_parent, remote_parent, links) = match from {
            None => (base.clone(), base, all),
            Some((remote, turn)) => {
                let local = all
                    .iter()
                    .rev()
                    .find(|link| link.turn <= turn)
                    .map_or(base, |link| link.commit_id.clone());
                let later =
                    all.into_iter().filter(|link| link.turn > turn).collect();
                (local, remote.to_owned(), later)
            }
        };
        let title = title.to_owned();
        Ok(Pushing {
            project,
            repo: repo.to_owned(),
            token: token.token,
            links,
            local_parent,
            remote_parent,
            title,
        })
    }

    /// Pushes the turns `run` took since its pull request's last push,
    /// if it has one that keeps pushing. Returns whether it pushed.
    pub fn push_later_turns(&self, run: &RunId) -> anyhow::Result<bool> {
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
            Some((&open.head, open.turn)),
        )?;
        if pushing.links.is_empty() {
            return Ok(false);
        }
        let (head, turn) = self.runtime.block_on(push(
            &self.github,
            &pushing,
            &open.branch,
        ))?;
        if let Some(open) = self.prs.lock().expect("not poisoned").get_mut(run)
        {
            open.head = head;
            open.turn = turn;
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
) -> anyhow::Result<(String, u32)> {
    let (token, repo) = (&pushing.token, &pushing.repo);
    let mut remote = pushing.remote_parent.clone();
    let mut tree = api
        .commit_tree(token, repo, &remote)
        .await
        .map_err(anyhow::Error::msg)?;
    let mut local = pushing.local_parent.clone();
    let mut turn = 0;
    for link in &pushing.links {
        let mut files = Vec::new();
        for file in pushing.project.diff(&local, &link.commit_id)? {
            let blob =
                match pushing.project.file_at(&link.commit_id, &file.path)? {
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
        let message = format!("{} (turn {})", pushing.title, link.turn);
        remote = api
            .create_commit(token, repo, &message, &tree, &remote)
            .await
            .map_err(anyhow::Error::msg)?;
        local = link.commit_id.clone();
        turn = link.turn;
    }
    api.set_branch(token, repo, branch, &remote)
        .await
        .map_err(anyhow::Error::msg)?;
    Ok((remote, turn))
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
    project: Arc<ProjectSlot>,
    main: RunId,
    fork: RunId,
) -> anyhow::Result<BranchCode> {
    let project = tokio::task::spawn_blocking(move || project.wait())
        .await?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Runs work in the checkout itself, so there are no commits to \
             compare"
            )
        })?;
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
fn workspace_name() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNT: AtomicU32 = AtomicU32::new(0);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    format!("run-{millis:x}-{}", COUNT.fetch_add(1, Ordering::Relaxed))
}

/// One stored run, rebuilt as the interface shows it.
async fn stored_view(
    store: &Store,
    record: &tau_store::RunRecord,
    home: &str,
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
                if plugin == tau_reasoning::NAME
                    || plugin == tau_constitution::NAME
                    || plugin == LANDING_RECORD =>
            {
                serde_json::from_str(&body)
                    .ok()
                    .map(|body| Stored::Record { plugin, body })
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
    let messages: Vec<&Message> = timeline
        .iter()
        .filter_map(|entry| match entry {
            Stored::Message(message) => Some(message),
            Stored::Record { .. } => None,
        })
        .collect();
    let prompt = prompt_of(&record.kind, messages.iter().copied());
    let mut view = RunView::from_timeline(
        RunId(record.id.clone().into()),
        title(&prompt),
        &record.agent,
        &record.model,
        &timeline,
    )
    .in_repo(
        stored_repo(store, &record.id)
            .await
            .unwrap_or_else(|| home.to_owned()),
    )
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
    view.finish_stored(stop, record.cost_usd);
    let goal: Vec<serde_json::Value> = store
        .records(&record.id, tau_goal::NAME)
        .await?
        .iter()
        .filter_map(|body| serde_json::from_str(body).ok())
        .collect();
    view.set_goal_records(&goal);
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

/// The words a run was started with: a fork's own prompt is its last
/// user message, any other run's its first.
fn prompt_of<'a>(
    kind: &RunKind,
    messages: impl DoubleEndedIterator<Item = &'a Message>,
) -> String {
    let mut users = messages.filter_map(|message| match message {
        Message::User(user) => Some(crate::view::user_words(&user.content)),
        _ => None,
    });
    match kind {
        RunKind::Fork { .. } => users.next_back(),
        _ => users.next(),
    }
    .unwrap_or_default()
}

/// Past runs, rebuilt from their stored transcripts, each under the
/// repository it recorded, or `home` if it recorded none.
pub async fn history(
    store: &Store,
    home: &str,
) -> anyhow::Result<Vec<RunView>> {
    let records = store.recent_runs(HISTORY).await?;
    let mut views = Vec::with_capacity(records.len());
    for record in &records {
        views.push(stored_view(store, record, home).await?);
    }
    // Sub-agents are left out of the list: they come back under the
    // runs that called them, however they ended.
    let parents: Vec<RunId> =
        views.iter().map(|view| view.id.clone()).collect();
    for parent in parents {
        for record in store.subagents(&parent.0).await? {
            let view = stored_view(store, &record, home).await?.with_origin(
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
        host.runtime
            .spawn_blocking(move || cloner.clone_github(&name))
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
                Ok(repo) => {
                    if let Some(slot) = host.slot(&repo.name) {
                        refresh_when_imported(&host, &slot, &workspace, cx);
                    }
                    workspace.update(cx, |ws, cx| {
                        ws.apply(HostUpdate::Repo(repo), cx)
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

/// A short run title from the prompt: its first words, or its goal's.
pub fn title(prompt: &str) -> String {
    let goal = tau_goal::set_message(prompt);
    let prompt = goal.as_deref().unwrap_or(prompt);
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

    #[test]
    fn titles_come_from_the_first_words() {
        assert_eq!(title("Fix the retry loop, please!"), "fix-the-retry-loop");
        assert_eq!(title("  ?! "), "untitled");
        assert_eq!(
            title("/goal --continuations 3 all tests pass"),
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
