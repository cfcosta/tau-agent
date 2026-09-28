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
    event::{RunEvent, StopReason},
    limits::Limits,
    plugin::{Plugin, PluginCtx, PluginRun, RunPlan},
    tool::RunId,
};
use tau_ai::{
    client::OpenAi,
    codex::{CodexAuth, CodexCredentials},
    message::{Message, UserContent},
    model::find,
};
use tau_compaction::Compaction;
use tau_constitution::{Constitution, ConstitutionPlugin};
use tau_jev::TypeSafe;
use tau_store::{Entry, RunKind, Status, Store};
use tau_tools::{path::Root, plugin::CodingTools};
use tau_vcs::{
    ChangeKind,
    FileDiff,
    Identity,
    Link,
    Project,
    RunWorkspace,
    VcsPlugin,
    run_workspace::PLUGIN as WORKSPACE_PLUGIN,
};
use tokio::{runtime::Runtime, sync::mpsc};

use crate::{
    accounts::{self, Access, Credentials},
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
    models::{
        AccessInfo,
        AccessKind,
        Effort,
        ModelChoice,
        ModelSettings,
        Models,
        coding_models,
    },
    pull_request::{PrCommit, PrState, PullRequest},
    setup::{CloneState, ModelAccess, RepoClone, SetupStep, SetupUpdate},
    view::{
        BranchCode,
        ChildKind,
        ChildRun,
        CodeState,
        ContextWindow,
        FileChange,
        FileKind,
        FileStat,
        Limits as ViewLimits,
        Origin,
        PlanField,
        PluginStatus,
        RunView,
        Tone,
        parse_diff,
    },
    workspace::{Workspace, WorkspaceEvent},
};

/// What the host needs to start.
#[derive(Debug, Clone)]
pub struct HostConfig {
    /// What runs use to reach a model at first; see
    /// [`Host::set_access`].
    pub access: Access,
    /// Where sign-ins and keys are kept.
    pub credentials: Credentials,
    pub model: String,
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

    /// `models.json` beside the sign-in, in tau's config directory.
    pub fn default_settings() -> PathBuf {
        CodexCredentials::default_path()
            .and_then(|path| path.parent().map(|dir| dir.join("models.json")))
            .unwrap_or_else(|| PathBuf::from("models.json"))
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
    ) -> anyhow::Result<Box<dyn PluginRun>> {
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
    /// What runs reach models with; `None` after signing out of all.
    access: Mutex<Option<Access>>,
    github: github::Api,
    /// Jev for tau-constitution, in place of TypeSafe's with the saved
    /// key: for tests.
    jev: Option<Arc<dyn tau_jev::Jev>>,
    /// What every plugin's Jev requests did this session.
    jev_meter: Arc<Mutex<crate::metered::Meter>>,
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

/// The agent runs start from, reaching models with `access`.
fn coder(
    runtime: &Runtime,
    access: &Access,
    model: &str,
) -> anyhow::Result<Agent> {
    // Clients must be created inside the runtime.
    let _guard = runtime.enter();
    let client = match access {
        Access::Codex(path) => OpenAi::codex(CodexAuth::from_file(path)?),
        Access::ApiKey(key) => OpenAi::new(key.clone()),
    };
    Ok(Agent::new(client)
        .name("coder")
        .model(model)
        .instructions(INSTRUCTIONS)
        // Plugins that hold a stop cap themselves: the constitution by
        // its holds, a goal by its continuations.
        .limits(
            Limits::default()
                .max_turns(MAX_TURNS)
                .max_continuations(u32::MAX),
        ))
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
        let agent = coder(&runtime, &config.access, &config.model)?;
        let (host, events) = Self::with_agent(runtime, agent, store, config);
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
        let settings = load_settings(&config.settings, &config.model);
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
            access: Mutex::new(Some(config.access.clone())),
            github: github::Api::default(),
            jev: None,
            jev_meter: Arc::default(),
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
        match self.github_of(name) {
            Some(full_name) => {
                let token = github::Token::load(&self.config.credentials);
                project.update(tau_vcs::UpdateFrom::Remote {
                    url: &self.github.clone_url(&full_name),
                    token: token.as_ref().map(|token| token.token.as_str()),
                })
            }
            None => project.update(tau_vcs::UpdateFrom::Checkout(&slot.path)),
        }
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
                description: "Prunes stale tool history with Jev".into(),
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
        if self.access().is_none() {
            anyhow::bail!(
                "tau is signed out of every model. Sign in with ChatGPT or \
                 add an API key on the Models screen."
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
                ..tau_fast_compaction::Settings::default()
            };
            agent = agent.plugin(
                tau_fast_compaction::FastCompaction::shared(jev.clone())
                    .settings(settings),
            );
        }
        let agent = agent.plugin(compaction).plugin(RepoTag(repo.name.clone()));
        // The repository's rules, checked with Jev when there is a key.
        let constitution = jev.map(|jev| {
            ConstitutionPlugin::from_file(jev, self.constitution_path(repo))
        });
        // The conversation's goal, checked with Jev too; after the
        // constitution, whose hold of a stop wins.
        let goal = self.jev().map(tau_goal::GoalPlugin::new);
        let Some(project) = repo.project.wait() else {
            let tools = CodingTools::new(Root::new(repo.path.clone()));
            let agent = agent.plugin(tools);
            let agent = with_plugin(with_plugin(agent, constitution), goal);
            return Ok((agent, None));
        };
        let name = workspace.unwrap_or_else(workspace_name);
        let workspace = RunWorkspace::new(project.clone(), &name, identity())?;
        let agent = agent
            .plugin(CodingTools::new(Root::new(workspace.dir())))
            .plugin(VcsPlugin::new(workspace.vcs().clone()))
            .plugin(workspace);
        let agent = with_plugin(with_plugin(agent, constitution), goal);
        Ok((agent, Some(name)))
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

    /// Where a repository's constitution is kept: in tau's directory for
    /// it, so it is edited from tau and applies to the next run, without
    /// a commit in the repository.
    fn constitution_path(&self, repo: &RepoSlot) -> PathBuf {
        self.config
            .project_dir_of(&repo.path)
            .join("constitution.toml")
    }

    /// The constitution of repository `name`, for its screen.
    fn repo_constitution(&self, slot: &RepoSlot) -> CatalogConstitution {
        let path = self.constitution_path(slot);
        let (loaded, error) = match Constitution::load(&path) {
            Ok(loaded) => (loaded, None),
            Err(error) => (Constitution::default(), Some(format!("{error:#}"))),
        };
        CatalogConstitution {
            path: path.display().to_string(),
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

    /// A repository's constitution file, made with no rules if there is
    /// none, for editing by hand.
    pub fn constitution_file(&self, repo: &str) -> anyhow::Result<PathBuf> {
        let slot = self
            .slot(repo)
            .ok_or_else(|| anyhow::anyhow!("No repository {repo}"))?;
        let path = self.constitution_path(&slot);
        if !path.exists() {
            Constitution::default().save(&path)?;
        }
        Ok(path)
    }

    /// Adds a rule to a repository's constitution (review and block at
    /// their defaults), or removes one, and saves it.
    pub fn edit_rules(
        &self,
        repo: &str,
        edit: impl FnOnce(&mut Constitution) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        let slot = self
            .slot(repo)
            .ok_or_else(|| anyhow::anyhow!("No repository {repo}"))?;
        let path = self.constitution_path(&slot);
        let mut constitution = Constitution::load(&path)?;
        edit(&mut constitution)?;
        constitution.save(&path)
    }

    /// What runs reach models with, if anything.
    pub fn access(&self) -> Option<Access> {
        self.access.lock().expect("not poisoned").clone()
    }

    fn access_label(&self) -> &'static str {
        self.access().map_or("signed out", |access| access.label())
    }

    /// Runs started from now on reach models with `access`; runs going
    /// on keep theirs. `None` stops new runs until one is set.
    pub fn set_access(&self, access: Option<Access>) -> anyhow::Result<()> {
        if let Some(access) = &access {
            let agent = coder(&self.runtime, access, &self.config.model)?;
            *self.base.lock().expect("not poisoned") = agent;
        }
        *self.access.lock().expect("not poisoned") = access;
        Ok(())
    }

    /// Forgets that kind of access, and runs on what is left, if
    /// anything.
    pub fn sign_out(&self, kind: AccessKind) -> anyhow::Result<Option<Access>> {
        self.config.credentials.forget(kind)?;
        let left = self.config.credentials.access();
        self.set_access(left.clone())?;
        Ok(left)
    }

    /// The models the picker offers, with what this sign-in can run, and
    /// the user's choices.
    pub fn models(&self) -> Models {
        let access = self.access();
        let codex = matches!(access, Some(Access::Codex(_)));
        let credentials = &self.config.credentials;
        Models {
            options: coding_models(|id| match &access {
                None => false,
                Some(Access::Codex(_)) => tau_ai::codex::MODELS.contains(&id),
                Some(Access::ApiKey(_)) => true,
            }),
            settings: self.settings.lock().expect("not poisoned").clone(),
            access: AccessInfo {
                label: self.access_label().into(),
                chatgpt: codex,
                api_key: matches!(access, Some(Access::ApiKey(_))),
                saved: [AccessKind::ChatGpt, AccessKind::ApiKey]
                    .into_iter()
                    .filter(|kind| credentials.has(*kind))
                    .collect(),
                jev: credentials.jev_key().is_some(),
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
                ModelChoice::new(self.config.model.clone(), Effort::Auto)
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
                state: "watching the window".into(),
                tone: Tone::Quiet,
            });
        }
        view.plugins.push(PluginStatus {
            name: "tau-compaction".into(),
            state: "watching the window".into(),
            tone: Tone::Quiet,
        });
        let rules = Constitution::load(&self.constitution_path(&repo))
            .map_or(0, |constitution| constitution.rules.len());
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
            Ok(runs) => workspace.update(cx, |ws, cx| ws.add_history(runs, cx)),
            Err(error) => eprintln!("tau-ui: cannot read past runs: {error:#}"),
        }
        let host = Arc::new(self);
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
        // A sign-in from the Models screen changes what new runs use.
        let connected: accounts::Connected = {
            let (host, workspace) = (host.clone(), workspace.downgrade());
            std::rc::Rc::new(move |access, cx| {
                let applied = host.set_access(Some(access));
                let catalog = host.catalog();
                let _ = workspace.update(cx, |ws, cx| {
                    ws.set_catalog(catalog, cx);
                    if let Err(error) = applied {
                        ws.show_alert(
                            "Could not use the new sign-in",
                            format!("{error:#}"),
                            cx,
                        );
                    }
                });
            })
        };
        cx.subscribe(
            workspace,
            move |workspace, event: &WorkspaceEvent, cx| {
                if accounts::handle_sign_in(
                    event,
                    &workspace,
                    &handler.config.credentials,
                    &handler.config.model,
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
                WorkspaceEvent::SignOut(kind) => match handler.sign_out(*kind) {
                    Ok(left) => {
                        let catalog = handler.catalog();
                        workspace.update(cx, |ws, cx| {
                            ws.set_catalog(catalog, cx);
                            if left.is_none() {
                                // Nothing left to run on: set it up again.
                                ws.update_setup(
                                    SetupUpdate::Model(ModelAccess::None),
                                    cx,
                                );
                                ws.start_setup(SetupStep::Model, cx);
                            }
                        })
                    }
                    Err(error) => workspace.update(cx, |ws, cx| {
                        ws.show_alert(
                            "Could not sign out",
                            format!("{error:#}"),
                            cx,
                        )
                    }),
                },
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
                        workspace.update(cx, |ws, cx| ws.push_run(view, cx))
                    }
                    Err(error) => workspace.update(cx, |ws, cx| {
                        ws.show_alert(
                            "Could not start the run",
                            format!("{error:#}"),
                            cx,
                        )
                    }),
                },
                WorkspaceEvent::AddRule { repo, text, on } => {
                    let added = handler.edit_rules(repo, |rules| {
                        rules
                            .add(
                                text,
                                on,
                                tau_constitution::rules::DEFAULT_REVIEW,
                                tau_constitution::rules::DEFAULT_BLOCK,
                            )
                            .map(drop)
                    });
                    let catalog = handler.catalog();
                    workspace.update(cx, |ws, cx| {
                        ws.set_catalog(catalog, cx);
                        if let Err(error) = added {
                            ws.show_alert(
                                "Could not add the rule",
                                format!("{error:#}"),
                                cx,
                            );
                        }
                    });
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
                            Ok(draft) => ws.set_pull_request(&run, draft, cx),
                            Err(error) => {
                                ws.back(cx);
                                ws.show_alert("Could not write the pull request", error, cx)
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
                            ws.set_pull_request_state(&run, state, cx)
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
                            .update(cx, |ws, cx| ws.set_query_result(result, cx));
                    })
                    .detach();
                }
                WorkspaceEvent::EditConstitution { repo } => {
                    match handler.constitution_file(repo) {
                        Ok(path) => cx.open_with_system(&path),
                        Err(error) => workspace.update(cx, |ws, cx| {
                            ws.show_alert(
                                "Could not open the constitution",
                                format!("{error:#}"),
                                cx,
                            )
                        }),
                    }
                }
                WorkspaceEvent::RemoveRule { repo, id } => {
                    let removed = handler.edit_rules(repo, |rules| {
                        rules.remove(id);
                        Ok(())
                    });
                    let catalog = handler.catalog();
                    workspace.update(cx, |ws, cx| {
                        ws.set_catalog(catalog, cx);
                        if let Err(error) = removed {
                            ws.show_alert(
                                "Could not remove the rule",
                                format!("{error:#}"),
                                cx,
                            );
                        }
                    });
                }
                WorkspaceEvent::JevKey { key } => {
                    let saved =
                        handler.config.credentials.set_jev_key(key.as_deref());
                    let catalog = handler.catalog();
                    workspace.update(cx, |ws, cx| {
                        ws.set_catalog(catalog, cx);
                        if let Err(error) = saved {
                            ws.show_alert(
                                "Could not save the TypeSafe key",
                                error.to_string(),
                                cx,
                            );
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
                            ws.resume_failed(run, cx);
                            ws.show_alert(
                                "Could not go on with the run",
                                format!("{error:#}"),
                                cx,
                            )
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
                        workspace.update(cx, |ws, cx| ws.push_run(view, cx))
                    }
                    Err(error) => workspace.update(cx, |ws, cx| {
                        ws.show_alert(
                            "Could not fork the run",
                            format!("{error:#}"),
                            cx,
                        )
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
                            ws.set_branch_code(&main, &fork, code, cx)
                        });
                    })
                    .detach();
                }
                WorkspaceEvent::SaveModelSettings(settings) => {
                    if let Err(error) = handler.save_settings(settings.clone())
                    {
                        workspace.update(cx, |ws, cx| {
                            ws.show_alert(
                                "Could not save the model settings",
                                format!("{error:#}"),
                                cx,
                            )
                        });
                    }
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
                            workspace.update(cx, |ws, cx| ws.add_repo(repo, cx))
                        }
                        Err(error) => workspace.update(cx, |ws, cx| {
                            ws.show_alert(
                                "Could not add the repository",
                                format!("{error:#}"),
                                cx,
                            )
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
                    let _ = workspace
                        .update(cx, |ws, cx| ws.set_catalog(catalog, cx));
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
                let applied =
                    workspace.update(cx, |ws, cx| ws.apply_event(&event, cx));
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
    workspace.update(cx, |ws, cx| ws.set_catalog(catalog, cx));
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
            ws.set_catalog(catalog, cx);
            if let (true, Err(error)) = (asked, result) {
                ws.show_alert(format!("Could not update {name}"), error, cx);
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
    /// inherits as a fork, in order.
    fn changed_turns(&self, run: &RunId) -> anyhow::Result<Vec<Link>> {
        let bodies = self
            .runtime
            .block_on(self.store.records(&run.0, WORKSPACE_PLUGIN))?;
        Ok(bodies
            .iter()
            .filter_map(|body| Link::parse(body))
            .filter(|link| link.changed)
            .collect())
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
        let links = self.changed_turns(run)?;
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
        let all = self.changed_turns(run)?;
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
                ws.set_pull_request_state(
                    &run,
                    PrState::Opened {
                        number,
                        url,
                        checks,
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
        let _ = workspace.update(cx, |ws, cx| ws.set_catalog(catalog, cx));
    })
    .detach();
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
    .ok_or_else(|| anyhow::anyhow!("The fork point has no commit"))?
    .commit_id;
    let head = |run: &RunId| {
        let store = store.clone();
        let run = run.0.to_string();
        let base = base.clone();
        async move {
            let entries = store.plugin_entries(&run, WORKSPACE_PLUGIN).await?;
            anyhow::Ok(
                last_link(&entries, None).map_or(base, |link| link.commit_id),
            )
        }
    };
    let main_head = head(&main).await?;
    let fork_head = head(&fork).await?;
    tokio::task::spawn_blocking(move || {
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

/// Past runs, rebuilt from their stored transcripts, each under the
/// repository it recorded, or `home` if it recorded none.
pub async fn history(
    store: &Store,
    home: &str,
) -> anyhow::Result<Vec<RunView>> {
    let records = store.recent_runs(HISTORY).await?;
    let mut views = Vec::with_capacity(records.len());
    for record in &records {
        let messages: Vec<Message> = store
            .transcript(&record.id)
            .await?
            .into_iter()
            .filter_map(|entry| match entry {
                Entry::Message { body, .. } => serde_json::from_str(&body).ok(),
                _ => None,
            })
            .collect();
        let mut users = messages.iter().filter_map(|message| match message {
            Message::User(user) => Some(match &user.content {
                UserContent::Text(text) => text.clone(),
                UserContent::Blocks(_) => String::new(),
            }),
            _ => None,
        });
        // A fork's own prompt is its last user message.
        let prompt = match record.kind {
            RunKind::Fork { .. } => users.next_back(),
            _ => {
                let mut users = users;
                users.next()
            }
        }
        .unwrap_or_default();
        let mut view = RunView::from_messages(
            RunId(record.id.clone().into()),
            title(&prompt),
            &record.agent,
            &record.model,
            &messages,
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
            Status::Running => StopReason::Error(
                "interrupted: tau closed during the run".into(),
            ),
        };
        view.finish_stored(stop, record.cost_usd);
        // What the plugins recorded: the effort chosen, and the
        // constitution's checks and verdicts on the calls.
        for plugin in [tau_reasoning::NAME, tau_constitution::NAME] {
            for body in store.records(&record.id, plugin).await? {
                if let Ok(body) = serde_json::from_str(&body) {
                    view.report(plugin, &body);
                }
            }
        }
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
        views.push(view);
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

/// Carries out onboarding when no model is configured: the ChatGPT
/// sign-in (browser or device code) and API keys. Once one works and is
/// saved, `ready` gets the new [`Access`] to build a [`Host`] from, and
/// the host handles sign-ins from then on.
///
/// GitHub sign-ins work before a model is connected, so onboarding can
/// start with them.
pub fn onboard(
    workspace: &Entity<Workspace>,
    model: String,
    credentials: Credentials,
    cx: &mut App,
    ready: impl Fn(Access, &mut App) + 'static,
) {
    let done = std::rc::Rc::new(std::cell::Cell::new(false));
    let connected: accounts::Connected = {
        let done = done.clone();
        std::rc::Rc::new(move |access, cx| {
            done.set(true);
            ready(access, cx)
        })
    };
    let api = github::Api::default();
    github::restore(workspace, &credentials, &api, cx);
    cx.subscribe(workspace, move |workspace, event: &WorkspaceEvent, cx| {
        // Once a host runs, it answers.
        if done.get() {
            return;
        }
        let _ = accounts::handle_sign_in(
            event,
            &workspace,
            &credentials,
            &model,
            &connected,
            cx,
        ) || github::handle(event, &workspace, &credentials, &api, cx);
    })
    .detach();
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
        ws.update_setup(
            report(CloneState::Cloning {
                share: 0.3,
                detail: "fetching from GitHub".into(),
            }),
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
        let _ = cx.update(|cx| {
            let state = match cloned {
                Ok(repo) => {
                    if let Some(slot) = host.slot(&repo.name) {
                        refresh_when_imported(&host, &slot, &workspace, cx);
                    }
                    workspace.update(cx, |ws, cx| ws.add_repo(repo, cx));
                    CloneState::Ready
                }
                Err(error) => CloneState::Failed(error),
            };
            let update = SetupUpdate::Clone(RepoClone { name, state });
            workspace.update(cx, |ws, cx| ws.update_setup(update, cx));
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
}
