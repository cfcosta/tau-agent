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
        PluginInfo,
        PluginScreen,
        ProjectStatus,
        Repo,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Listed {
    name: String,
    path: PathBuf,
    /// Removed from tau: not listed, but its runs keep its name.
    #[serde(default)]
    hidden: bool,
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
        .limits(Limits::default().max_turns(MAX_TURNS)))
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
            store,
            choices: Arc::default(),
            settings: Arc::new(Mutex::new(settings)),
            config,
            repos: Arc::new(Mutex::new(vec![slot])),
            home,
            list: Arc::new(Mutex::new(list)),
            run_repos: Arc::default(),
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
        self.add_repo(&dir.to_string_lossy())
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
        plugins.push(PluginInfo {
            name: "tau-compaction".into(),
            description: "Summarizes the context when it nears the window"
                .into(),
            seams: vec![Seam::Start, Seam::Rewrite],
            spend: 0.0,
            screen: Some(PluginScreen::Ledger),
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
        let project = match (importing, state) {
            (Some(slot), _) => ProjectStatus::Importing(slot.name.clone()),
            (None, ProjectState::Ready(_)) => {
                ProjectStatus::Ready(home.name.clone())
            }
            (None, ProjectState::Checkout(why)) => ProjectStatus::Checkout(why),
            (None, ProjectState::Importing) => {
                ProjectStatus::Importing(home.name.clone())
            }
        };
        // In the list's order, which adding a repository again keeps.
        let repos = list
            .repos
            .iter()
            .filter(|listed| !listed.hidden)
            .filter(|listed| slots.iter().any(|slot| slot.name == listed.name))
            .map(|listed| {
                Repo::new(&listed.name, listed.path.display().to_string())
            })
            .collect();
        Catalog {
            agent: "coder".into(),
            agent_source: Some(source),
            plugins,
            jev: None,
            repos,
            open_repos: list.open,
            store: StoreInfo {
                path: self.config.store.display().to_string(),
                size: std::fs::metadata(&self.config.store)
                    .map(|meta| format!("{:.1} MB", meta.len() as f64 / 1e6))
                    .unwrap_or_default(),
                sample_query:
                    "select agent, sum(cost_usd) from runs group by agent"
                        .into(),
            },
            pull_requests: false,
            project,
            models: self.models(),
        }
    }

    /// The agent for one run, with its tools on the run's workspace, and
    /// the workspace's name.
    fn agent_for_run(
        &self,
        choice: &ModelChoice,
        repo: &RepoSlot,
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
        if let Some(effort) = choice.effort.reasoning() {
            agent = agent.reasoning(effort);
        }
        // Compaction steps in by the run's own model's window.
        let mut compaction = Compaction::default();
        if let Some(model) = find(&choice.model) {
            compaction = compaction.context_window(model.context_window);
        }
        let agent = agent.plugin(compaction).plugin(RepoTag(repo.name.clone()));
        let Some(project) = repo.project.wait() else {
            let tools = CodingTools::new(Root::new(repo.path.clone()));
            return Ok((agent.plugin(tools), None));
        };
        let name = workspace_name();
        let workspace = RunWorkspace::new(project.clone(), &name, identity())?;
        let agent = agent
            .plugin(CodingTools::new(Root::new(workspace.dir())))
            .plugin(VcsPlugin::new(workspace.vcs().clone()))
            .plugin(workspace);
        Ok((agent, Some(name)))
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
        let (agent, workspace) = self.agent_for_run(choice, &repo)?;
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
        let (seq, link) = self.link(run, turn)?.ok_or_else(|| match turn {
            Some(turn) => {
                anyhow::anyhow!("Turn {turn} has no commit to fork from")
            }
            None => {
                anyhow::anyhow!("The run has no finished turn to fork from yet")
            }
        })?;
        let (agent, workspace) = self.agent_for_run(choice, &repo)?;
        let _guard = self.runtime.enter();
        let forked = agent
            .fork(&Checkpoint::at(run.clone(), seq))
            .start(prompt, &self.store);
        let id = self.track(forked, workspace, choice, &repo.name);
        Ok(self.view(id, prompt).with_origin(Origin::Fork {
            from: run.clone(),
            turn: link.turn,
        }))
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
        view.plugins.push(PluginStatus {
            name: "tau-compaction".into(),
            state: "watching the window".into(),
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

/// A short run title from the prompt: its first words.
pub fn title(prompt: &str) -> String {
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
    }
}
