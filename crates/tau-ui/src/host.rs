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
//! `repos.json`; each run records its repository ([`RunTag`]), so past
//! runs come back from the store under theirs.

use std::{
    collections::{HashMap, HashSet},
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
use tau_artifacts::{Bytes, Quotas};
use tau_jev::TypeSafe;
use tau_store::{Entry, RunKind, Status, Store, TurnUsage};
use tau_tools::{path::Root, plugin::CodingTools};
use tau_ui_plugin::{HOST_RECORD, HostRecord, Services, TurnCommit, TurnHooks};
use tau_vcs::{
    ChangeKind,
    DEFAULT_WORKSPACE,
    FileDiff,
    Identity,
    Landing,
    Link,
    Project,
    ProjectRepo,
    RefusingSpawn,
    RefusingWait,
    RunWorkspace,
    Spawn,
    VcsPlugin,
    Wait,
    run_workspace::{PLUGIN as WORKSPACE_PLUGIN, bookmark},
    sub_agents::ChildModel,
};
use tokio::{runtime::Runtime, sync::mpsc};

use crate::{
    accounts::{self, Credentials},
    catalog::{Catalog, PluginInfo, ProjectStatus, Repo, Seam, StoreInfo},
    github,
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
        CodeState,
        ContextWindow,
        DROPPED_RECORD,
        Ending,
        FileChange,
        FileKind,
        FileStat,
        LANDING_RECORD,
        LandingRecord,
        Origin,
        RunView,
        Stored,
    },
    workspace::{Workspace, WorkspaceEvent},
};

mod artifacts;

/// Writes `settings` to `path`, making its directory.
pub(crate) async fn write_settings(
    path: &Path,
    settings: &ModelSettings,
) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    tokio::fs::write(path, serde_json::to_string_pretty(settings)?).await?;
    Ok(())
}

/// One file's saves, one at a time, each writing what is kept in memory
/// when its turn comes: a save that waited writes the newest state, so
/// saves never land out of order.
#[derive(Clone, Default)]
pub(crate) struct Saving(Arc<tokio::sync::Mutex<()>>);

impl Saving {
    /// Waits for the saves before it, then writes `latest()` with
    /// `write`.
    pub(crate) async fn save<T, F>(
        &self,
        latest: impl FnOnce() -> T,
        write: impl FnOnce(T) -> F,
    ) -> anyhow::Result<()>
    where
        F: std::future::Future<Output = anyhow::Result<()>>,
    {
        let _turn = self.0.lock().await;
        write(latest()).await
    }
}

/// A repository's main chat's title.
pub const MAIN_TITLE: &str = "main";

/// `base` on `choice`: its model and effort (auto leaves it to a
/// plugin, such as tau-reasoning), recording how it started (`start`,
/// whose effort is `choice`'s).
fn for_model(base: Agent, choice: &ModelChoice, start: HostRecord) -> Agent {
    let mut agent = base.model(&choice.model);
    if let Some(effort) = choice.effort.reasoning() {
        agent = agent.reasoning(effort);
    }
    agent.plugin(RunTag(HostRecord {
        effort: Some(choice.effort.label().to_owned()),
        ..start
    }))
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
    /// Jev for plugins that ask it, in place of TypeSafe's with the
    /// saved key: for tests.
    jev: Option<Arc<dyn tau_jev::Jev>>,
    /// What every plugin's Jev requests did this session.
    jev_meter: Arc<Mutex<crate::metered::Meter>>,
    /// How tau-memory searches notes: by meaning in the app, by keywords
    /// in tests, where no model is loaded.
    memory_search: tau_memory::ui::Search,
    store: Store,
    config: HostConfig,
    /// The repositories of this session, in the list's order. Removed
    /// ones stay here, unlisted, for their runs.
    repos: Arc<Mutex<Vec<RepoSlot>>>,
    list: Arc<Mutex<RepoList>>,
    /// What the host knows of each run of this session.
    sessions: Arc<Mutex<HashMap<RunId, SessionRun>>>,
    /// Repositories taking in new commits now.
    updating: Arc<Mutex<Vec<String>>>,
    /// What the latest update found.
    last_update: Arc<Mutex<Option<String>>>,
    /// The latest landing forecast asked for in each repository: a pass
    /// an earlier ask started shows nothing once a later one comes.
    forecasts: Arc<Mutex<HashMap<String, u64>>>,
    /// Pull request drafts written from runs, until they are opened.
    drafts: Mutex<HashMap<RunId, PullRequest>>,
    /// Pull requests opened from runs, for pushing their later turns.
    prs: Arc<Mutex<HashMap<RunId, OpenPr>>>,
    /// How to steer and cancel each run going on.
    runs: Arc<Mutex<HashMap<RunId, RunControl>>>,
    /// Each repository's main chat's sub-agents, which run beside it
    /// and outlive its turns (ADR 0026), by repository.
    sub_agents: Mutex<HashMap<String, tau_vcs::SubAgents>>,
    /// What each run going on was steered with and has not read yet.
    unread: Arc<Mutex<HashMap<RunId, Vec<String>>>>,
    /// Runs whose `RunEnd` went by while their outcome is still being
    /// stored: they stop in a moment.
    ending: Arc<Mutex<HashSet<RunId>>>,
    /// How long a forecast waits for more changes before it starts.
    forecast_wait: std::time::Duration,
    /// Jobs the host is doing off the interface's thread, until what
    /// they did is shown.
    jobs: Arc<std::sync::atomic::AtomicUsize>,
    /// The user's model choices, as loaded and last saved.
    settings: Arc<Mutex<ModelSettings>>,
    events: mpsc::UnboundedSender<RunEvent>,
    /// The plugins with their UI, each with its state on this host (ADR
    /// 0017).
    hosted: Vec<hosted::Hosted>,
    /// Held while a run starts, and while a sweep reads what the
    /// projects hold and who owns it, so a sweep never takes a starting
    /// run's workspace.
    /// The saved TypeSafe key, read once as the host starts and kept as
    /// it changes ([`Host::set_jev_key`]): Jev is asked for often.
    jev_key: Mutex<Option<String>>,
    /// The saves of `repos.json`, and of the model settings, in order.
    list_saving: Saving,
    settings_saving: Saving,
    /// Each repository's own locks, by name (ADR 0028).
    repo_states: Mutex<HashMap<String, Arc<RepoState>>>,
    /// Asks the catalog's builder for a new one ([`Host::catalog_changed`]).
    catalog_wanted: Arc<tokio::sync::Notify>,
    /// The latest catalog the builder made, for the interface to show.
    catalog_feed: tokio::sync::watch::Sender<Option<Catalog>>,
    /// The step after which landings stop, as if tau closed there: for
    /// tests ([`Host::cut_landing_after`]).
    cut_landing: Mutex<Option<LandingStep>>,
    /// Each main chat's landing queue, restored from the store the
    /// first time it is asked for (ADR 0024).
    lanes: Mutex<HashMap<RunId, queue::Lane>>,
    /// Held while a landing queue changes or drains: one at a time.
    /// What each chat's last landing preview found: what the person
    /// confirms when they land it.
    previews: Mutex<HashMap<RunId, queue::Preview>>,
    /// Hears of conflicts still on a main chat after its turn.
    conflicts_hook: Option<lanes::ConflictsHook>,
    /// What plugins' host halves tell the interface, and its receiving
    /// end until [`Self::attach`] takes it.
    pushes: mpsc::UnboundedSender<tau_ui_plugin::Push>,
    pushed: Mutex<Option<mpsc::UnboundedReceiver<tau_ui_plugin::Push>>>,
}

/// What the host knows of one run of this session: the repository it
/// works in, its workspace, and the model it runs on.
#[derive(Debug, Clone, Default)]
struct SessionRun {
    repo: Option<String>,
    workspace: Option<String>,
    choice: Option<ModelChoice>,
}

mod attach;
mod config;
mod forecast;
mod history;
mod hosted;
mod instructions;
mod landing;
mod lanes;
mod onboarding;
mod pull_request;
mod push;
pub mod queue;
mod repos;
mod startup;
mod sub_agents;

pub use self::{
    config::HostConfig,
    history::history,
    instructions::{AGENTS_FILE, AGENTS_HEADING, AGENTS_LIMIT, agents_section},
    landing::{LANDING_INTENT, LandingStep},
    lanes::{ConflictsHook, DrainReport, QUEUE_PLUGIN},
    onboarding::onboard,
    startup::CUT_OFF,
};
use self::{
    config::*,
    history::*,
    instructions::RepoInstructions,
    onboarding::*,
    pull_request::*,
    repos::*,
};

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
        // No cap on turns, as in Codex and pi: compaction keeps a long
        // run going, and the person stops it. Plugins that hold a stop
        // cap themselves: the constitution by its holds, a goal by its
        // continuations.
        .limits(Limits::default().max_continuations(u32::MAX));
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

/// What `vcs_land` answers on a main chat, which commits on trunk.
const MAIN_DOES_NOT_LAND: &str = "The main chat commits straight to \
    trunk, so it has nothing to land: commit with `vcs_commit` and you \
    are done.";

/// What `vcs_land` answers on a sub-agent, which lands as it ends.
const SUB_AGENTS_DO_NOT_LAND: &str = "A sub-agent's commits land on its \
    caller when it ends, so it has nothing to propose: commit with \
    `vcs_commit`, then answer.";

fn identity() -> Identity {
    Identity {
        name: "tau".into(),
        email: "tau@localhost".into(),
    }
}

impl Host {
    /// What the host knows of `run` this session.
    fn session_of(&self, run: &RunId) -> SessionRun {
        let sessions = self.sessions.lock().expect("not poisoned");
        sessions.get(run).cloned().unwrap_or_default()
    }

    /// Changes what the host knows of `run` this session.
    fn session<R>(
        &self,
        run: &RunId,
        f: impl FnOnce(&mut SessionRun) -> R,
    ) -> R {
        f(self
            .sessions
            .lock()
            .expect("not poisoned")
            .entry(run.clone())
            .or_default())
    }

    /// Opens the store and the project and builds the agent. Returns the
    /// receiving end of the event channel for [`Host::attach`].
    #[allow(
        clippy::disallowed_methods,
        reason = "builds the runtime, then waits on it before anything runs on it (ADR 0028)"
    )]
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
        let store = runtime.block_on(tau_store_sqlite::open(&config.store))?;
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
        host.memory_search = tau_memory::ui::Search::Semantic;
        host.host_plugins();
        host.runtime.block_on(host.list_plugins_repo());
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
            host.spawn_import(&slot);
            slots.push(slot);
        }
        *host.repos.lock().expect("not poisoned") = slots;
        for listed in &listed {
            host.setting_up(host.main_of(&listed.name))?;
        }
        Ok((host, events))
    }

    /// A host over an agent built elsewhere: another model, other
    /// plugins, or a scripted model in tests. It lists no repository
    /// until one is cloned, or given with [`Self::with_repo`].
    #[allow(
        clippy::disallowed_methods,
        reason = "builds the host on its runtime before anything runs on it (ADR 0028)"
    )]
    pub fn with_agent(
        runtime: Runtime,
        agent: Agent,
        store: Store,
        config: HostConfig,
    ) -> (Self, mpsc::UnboundedReceiver<RunEvent>) {
        let (events, receiver) = mpsc::unbounded_channel();
        let (pushes, pushed) = mpsc::unbounded_channel();
        // Runs the last tau left running were cut off as it closed: this
        // one runs none of them.
        if let Err(error) = runtime.block_on(store.interrupt_running()) {
            eprintln!("tau-ui: cannot mark interrupted runs: {error:#}");
        }
        let (settings, list, jev_key) = runtime.block_on(async {
            (
                load_settings(&config.settings, &config.default_model()).await,
                RepoList::load(&config.repo_list).await,
                config.credentials.jev_key().await,
            )
        });
        let mut host = Self {
            runtime,
            base: Mutex::new(agent),
            account: Mutex::new(Some(config.account.clone())),
            client: Mutex::new(None),
            not_eligible: Arc::default(),
            github: github::Api::default(),
            jev: None,
            jev_meter: Arc::default(),
            memory_search: tau_memory::ui::Search::Keywords,
            store,
            settings: Arc::new(Mutex::new(settings)),
            config,
            repos: Arc::default(),
            list: Arc::new(Mutex::new(list)),
            sessions: Arc::default(),
            updating: Arc::default(),
            last_update: Arc::default(),
            forecasts: Arc::default(),
            drafts: Mutex::default(),
            prs: Arc::default(),
            ending: Arc::default(),
            forecast_wait: forecast::SETTLE,
            jobs: Arc::default(),
            runs: Arc::default(),
            sub_agents: Mutex::default(),
            unread: Arc::default(),
            events,
            hosted: Vec::new(),
            jev_key: Mutex::new(jev_key),
            list_saving: Saving::default(),
            settings_saving: Saving::default(),
            repo_states: Mutex::default(),
            catalog_wanted: Arc::default(),
            catalog_feed: tokio::sync::watch::Sender::new(None),
            cut_landing: Mutex::new(None),
            lanes: Mutex::default(),
            previews: Mutex::default(),
            conflicts_hook: None,
            pushes,
            pushed: Mutex::new(Some(pushed)),
        };
        host.host_plugins();
        (host, receiver)
    }

    /// Where sign-ins and keys are kept.
    pub fn credentials(&self) -> &Credentials {
        &self.config.credentials
    }

    /// Asks `jev` instead of TypeSafe's, for tests.
    pub fn with_jev(mut self, jev: Arc<dyn tau_jev::Jev>) -> Self {
        self.jev = Some(jev);
        self
    }

    /// Reaches GitHub at `api`, for tests.
    pub fn with_github(mut self, api: github::Api) -> Self {
        self.github = api;
        self
    }

    /// Closes a conversation, or opens it again, for the sidebar.
    pub async fn set_closed(
        &self,
        run: &RunId,
        closed: bool,
    ) -> anyhow::Result<()> {
        if closed && self.is_main(run) {
            anyhow::bail!("A repository's main chat stays open");
        }
        {
            let mut list = self.list.lock().expect("not poisoned");
            list.closed.retain(|id| **id != *run.0);
            if closed {
                list.closed.push(run.0.to_string());
            }
        }
        self.save_list().await
    }

    /// Writes `repos.json` as the list is kept now (ADR 0028).
    pub(super) async fn save_list(&self) -> anyhow::Result<()> {
        let path = self.config.repo_list.clone();
        self.list_saving
            .save(
                || self.list.lock().expect("not poisoned").clone(),
                async move |list| list.save(&path).await,
            )
            .await
    }

    /// Writes the model settings as they are kept now (ADR 0028).
    pub(super) async fn persist_settings(&self) -> anyhow::Result<()> {
        let path = self.config.settings.clone();
        self.settings_saving
            .save(
                || self.settings.lock().expect("not poisoned").clone(),
                async move |settings| write_settings(&path, &settings).await,
            )
            .await
    }

    /// `plugin`'s records for `run`, along its fork chain, as stored.
    pub async fn plugin_records(
        &self,
        run: &RunId,
        plugin: &str,
    ) -> Vec<serde_json::Value> {
        self.store
            .records(&run.0, plugin)
            .await
            .unwrap_or_default()
            .iter()
            .filter_map(|body| serde_json::from_str(body).ok())
            .collect()
    }

    /// Whether `run` is still going.
    /// Waits for a run whose `RunEnd` went by to finish storing its
    /// outcome, a moment at most. A run still working is not waited on.
    async fn settle(&self, run: &RunId) {
        for _ in 0..500 {
            if !self.ending.lock().expect("not poisoned").contains(run)
                || !self.is_running(run)
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    /// Whether the host is still doing something it was asked, or that
    /// a run's end set off: a landing, a forecast, an update.
    pub fn busy(&self) -> bool {
        self.jobs.load(std::sync::atomic::Ordering::SeqCst) > 0
    }

    /// Counts a job as begun, until the guard is dropped.
    pub(crate) fn job(&self) -> Job {
        self.jobs.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Job(self.jobs.clone())
    }

    pub fn is_running(&self, run: &RunId) -> bool {
        self.runs.lock().expect("not poisoned").contains_key(run)
            || self.sub_agent_running(run)
    }

    /// The workspace `run` works in, if it started in this session.
    pub async fn workspace(&self, run: &RunId) -> Option<PathBuf> {
        let name = self.session_of(run).workspace?;
        Some(
            self.slot_of_run(run)
                .await
                .ok()?
                .project
                .wait()
                .await?
                .workspace_dir(&name),
        )
    }

    /// What the workspace shows beyond runs: the agent's plugins and the
    /// store.
    pub async fn catalog(&self) -> Catalog {
        let mut plugins = vec![PluginInfo {
            name: "workspace".into(),
            description: "A jj workspace per run, and a commit per \
                              turn to fork from"
                .into(),
            seams: vec![Seam::Start],
            spend: 0.0,
            page: None,
            group: tau_ui_plugin::Group::Environment,
            note: None,
            entries: Vec::new(),
            settings: false,
        }];
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
        // In the list's order, which adding a repository again keeps.
        let mut repos: Vec<Repo> = Vec::new();
        for listed in list.repos.iter().filter(|listed| !listed.hidden) {
            let Some(slot) = slots.iter().find(|slot| slot.name == listed.name)
            else {
                continue;
            };
            let mut repo =
                Repo::new(&listed.name, listed.path.display().to_string());
            repo.main = listed.main.as_deref().map(|main| RunId(main.into()));
            repo.own = listed.own;
            // What the main chat would push, for a repository from
            // GitHub (ADR 0023).
            if listed.github.is_some()
                && let ProjectState::Ready(project) = slot.project.peek()
            {
                let (unpushed, trunk) = project
                    .run(|project| {
                        (
                            project
                                .unpushed()
                                .map_or(0, |changes| changes.len() as u32),
                            project.trunk_name().ok(),
                        )
                    })
                    .await;
                repo.unpushed = unpushed;
                repo.trunk = trunk;
            }
            repo.plugins = self.registered_repo_data(slot).await;
            repos.push(repo);
        }
        // The plugins with their UI, with their data and settings.
        let (registered, plugin_data, plugin_settings) =
            self.registered_catalog().await;
        plugins.extend(registered);
        // What each plugin cost over the runs of the last 30 days.
        let spend = self
            .store
            .plugin_spend(&days_ago(SPEND_DAYS))
            .await
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
            repo_plugin_settings: self.repo_plugin_settings(),
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
            store: StoreInfo {
                path: self.config.store.display().to_string(),
                size: tokio::fs::metadata(&self.config.store)
                    .await
                    .map(|meta| format!("{:.1} MB", meta.len() as f64 / 1e6))
                    .unwrap_or_default(),
                sample_query:
                    "select agent, sum(cost_usd) from runs group by agent"
                        .into(),
            },
            pull_requests: github::Token::load(&self.config.credentials)
                .await
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
    /// trunk and alone gets `spawn` and `wait`, as runs nest one level (ADR
    /// 0016).
    async fn agent_for_run(
        &self,
        choice: &ModelChoice,
        repo: &RepoSlot,
        name: String,
        main: bool,
        resolving: bool,
    ) -> anyhow::Result<(Agent, String)> {
        if self.account().is_none() {
            anyhow::bail!(
                "tau has no ChatGPT plan to run on. Sign in with ChatGPT and \
                 enable plan use on the Models screen."
            );
        }
        // What hangs on the model: its effort. A sub-agent can run on
        // another model than its caller.
        let for_model = {
            let base = self.base.lock().expect("not poisoned").clone();
            let start = HostRecord {
                repo: repo.name.clone(),
                access: Some(self.access_label().to_owned()),
                ..HostRecord::default()
            };
            move |choice: &ModelChoice, workspace: &RunWorkspace| {
                let start = HostRecord {
                    workspace: Some(workspace.dir().display().to_string()),
                    ..start.clone()
                };
                for_model(base.clone(), choice, start)
            }
        };
        // The plugins with their UI, after the rest: the repository's
        // rules check what the tools do, tau-goal's hold of a stop comes
        // after theirs, and context compaction goes by the run's model.
        let registered = self.registered(repo).await;
        let project = repo.project().await?;
        // What the plugins give the run's commands: an environment.
        let launcher = self.launcher_of(repo).await;
        let artifacts =
            Bytes::new(project.root().join("artifacts"), Quotas::default())?;
        // A run and its sub-agents work the same way, each in its own
        // workspace: tools, then what plugins add, which hear each turn's
        // commit. Every run declares the same tools, in the same order,
        // so each reads the others' prompt cache (ADR 0022): only the
        // main chat can spawn sub-agents, so sub-agents do not nest, and the
        // others' `spawn` and `wait` refuse.
        // `refusal`: `None` when the run proposes its own landing with
        // `vcs_land` (ADR 0014); else why it does not, which its
        // `vcs_land` answers.
        let on_workspace =
            move |agent: Agent,
                  workspace: RunWorkspace,
                  refusal: Option<&'static str>| {
                let hooks = TurnHooks::default();
                let workspace = {
                    let hooks = hooks.clone();
                    workspace.on_turn(move |turn| {
                        hooks.turned(&TurnCommit {
                            change_id: turn.change_id.clone(),
                            paths: turn.paths.clone(),
                        })
                    })
                };
                let vcs = VcsPlugin::new(workspace.vcs().clone());
                let vcs = match refusal {
                    None => vcs.landing(),
                    Some(refusal) => vcs.refusing_landing(refusal),
                };
                let dir = workspace.dir();
                let tools = CodingTools::new(Root::new(dir.clone()))
                    .with_artifacts(artifacts.clone());
                let tools = match &launcher {
                    Some(launcher) => tools.with_launcher(launcher.clone()),
                    None => tools,
                };
                let services = Services::default()
                    .with(hooks)
                    .with(tau_ui_plugin::WorkspaceDir(dir.clone()));
                // The repository's instructions come after the workspace,
                // which makes the directory as the run starts.
                let agent = agent
                    .plugin(tools)
                    .plugin(vcs)
                    .plugin(workspace)
                    .plugin(RepoInstructions { dir });
                (agent, services)
            };
        let workspace = RunWorkspace::new(project.clone(), &name, identity())?;
        // A main chat commits on trunk: it has nothing to land.
        let workspace = if main {
            workspace
                .commits_to(project.run(|project| project.trunk_name()).await?)
        } else {
            workspace
        };
        let agent = for_model(choice, &workspace);
        let models: Vec<String> =
            plan_models().into_iter().map(|model| model.id).collect();
        // Main's sub-agents, and every other run's refusing tools, in
        // the same place, so all declare the same tools (ADR 0022).
        let (agent, refusal) = if main {
            let agents = self.sub_agents_of(&repo.name);
            let child_on_workspace = on_workspace.clone();
            let registered = registered.clone();
            let caller = choice.clone();
            let refused = models.clone();
            let spawn = Spawn::new(
                workspace.clone(),
                identity(),
                agents.clone(),
                &models,
                move |child, asked| {
                    let choice = match child_choice(&caller, asked) {
                        Ok(choice) => choice,
                        Err(error) => {
                            return Box::pin(std::future::ready(Err(error)));
                        }
                    };
                    let agent = for_model(&choice, &child)
                        .tool(RefusingSpawn::new(&refused))
                        .tool(RefusingWait::default());
                    let (agent, services) = child_on_workspace(
                        agent,
                        child,
                        Some(SUB_AGENTS_DO_NOT_LAND),
                    );
                    let built = registered(
                        agent,
                        tau_ui_plugin::RunKind::SubAgent,
                        &choice,
                        services,
                    );
                    Box::pin(async move {
                        built.await.map_err(|error| format!("{error:#}").into())
                    })
                },
            );
            let wait = Wait::new(workspace.clone(), agents);
            (agent.tool(spawn).tool(wait), Some(MAIN_DOES_NOT_LAND))
        } else {
            let agent = agent
                .tool(RefusingSpawn::new(&models))
                .tool(RefusingWait::default());
            (agent, None)
        };
        // tau's turn resolving a landing's conflicts on main stops only
        // once they are resolved, or after one more try (ADR 0024).
        let hold = (main && resolving).then(|| lanes::ResolveHold {
            vcs: workspace.vcs().clone(),
        });
        let (agent, services) = on_workspace(agent, workspace, refusal);
        let agent = match hold {
            Some(hold) => agent.plugin(hold),
            None => agent,
        };
        let kind = if main {
            tau_ui_plugin::RunKind::Main
        } else {
            tau_ui_plugin::RunKind::Chat
        };
        let agent = registered(agent, kind, choice, services).await?;
        Ok((agent, name))
    }

    /// Jev, when there is a TypeSafe key (or one given for tests).
    fn jev(&self) -> Option<Arc<dyn tau_jev::Jev>> {
        let inner = self.jev.clone().or_else(|| {
            self.jev_key
                .lock()
                .expect("not poisoned")
                .clone()
                .map(|key| {
                    Arc::new(TypeSafe::new(key)) as Arc<dyn tau_jev::Jev>
                })
        })?;
        Some(Arc::new(crate::metered::Metered::new(
            inner,
            self.jev_meter.clone(),
        )))
    }

    /// Whether a TypeSafe key is saved.
    pub(super) fn has_jev_key(&self) -> bool {
        self.jev_key.lock().expect("not poisoned").is_some()
    }

    /// Saves the TypeSafe key, or forgets it with `None`; runs started
    /// from now on ask Jev with it.
    pub async fn set_jev_key(&self, key: Option<&str>) -> anyhow::Result<()> {
        self.config.credentials.set_jev_key(key).await?;
        *self.jev_key.lock().expect("not poisoned") = key
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(str::to_owned);
        Ok(())
    }

    /// What Jev did this session, for the Plugins screen, when there is
    /// a key.
    fn jev_stats(&self) -> Option<crate::catalog::JevStats> {
        if self.jev.is_none() && !self.has_jev_key() {
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

    /// Starts a chat in `repo`, which must be listed: a fork of the
    /// repository's main chat at its latest turn, on that turn's code,
    /// or at its start when it has none. Returns the chat's view, ready
    /// to be pushed into the workspace before its first event arrives.
    pub async fn start(
        &self,
        prompt: &str,
        choice: &ModelChoice,
        repo: &str,
    ) -> anyhow::Result<RunView> {
        let repo = self
            .slot(repo)
            .ok_or_else(|| anyhow::anyhow!("No repository {repo}"))?;
        let main = self.main_of(&repo.name).await?;
        // A chat started now would fork conflicted code.
        self.refuse_fork_of(&main).await?;
        let (source, seq, turn) = match self.fork_point(&main, None).await? {
            Some((source, seq, link)) => (source, seq, link.turn),
            None => (main, -1, 0),
        };
        self.fork_at(&repo, source, seq, turn, prompt, choice).await
    }

    /// Forks `run` after `turn` (its latest turn when `None`): a new run
    /// on `prompt` that has the run's conversation up to that turn and
    /// works on that turn's code, in a workspace of its own.
    pub async fn fork(
        &self,
        run: &RunId,
        turn: Option<u32>,
        prompt: &str,
        choice: &ModelChoice,
    ) -> anyhow::Result<RunView> {
        let repo = self.slot_of_run(run).await?;
        // Runs nest one level (ADR 0016): only a main chat has runs
        // under it.
        if !self.is_main(run) {
            anyhow::bail!(
                "Only a repository's main chat can be forked: a chat under \
                 it, like a sub-agent, has nothing under it"
            );
        }
        self.refuse_fork_of(run).await?;
        let (source, seq, link) = self
            .fork_point(run, turn)
            .await?
            .ok_or_else(|| match turn {
                Some(turn) => {
                    anyhow::anyhow!("Turn {turn} has no commit to fork from")
                }
                None => anyhow::anyhow!(
                    "The run has no finished turn to fork from yet"
                ),
            })?;
        self.fork_at(&repo, source, seq, link.turn, prompt, choice)
            .await
    }

    /// Starts a run on `prompt` that continues `source` from its entry
    /// `seq`, after its turn `turn`, in a workspace of its own in `repo`.
    async fn fork_at(
        &self,
        repo: &RepoSlot,
        source: RunId,
        seq: i64,
        turn: u32,
        prompt: &str,
        choice: &ModelChoice,
    ) -> anyhow::Result<RunView> {
        let state = self.repo_state(&repo.name);
        let _starting = state.starting.lock().await;
        // An effort the model does not take falls back to auto.
        let choice = &choice.clone().fitted();
        // Named after what it was asked, so the workspace says what it is
        // for.
        let name = workspace_name(&branch_slug(prompt));
        // A fork is a chat under the main chat: it spawns none.
        let (agent, workspace) =
            self.agent_for_run(choice, repo, name, false, false).await?;
        let forked = agent
            .fork(&Checkpoint::at(source.clone(), seq))
            .after_turn(turn)
            .start(prompt, &self.store);
        let id = self.track(forked, workspace, choice, &repo.name);
        // What each plugin's state is as the fork inherits it: a goal set
        // in the main chat, which tau-goal goes on checking; then what it
        // says as the fork starts.
        let starting = self
            .starting(&self.run_ctx(tau_ui_plugin::RunKind::Chat, repo, choice))
            .await;
        let mut inherited: Vec<(String, Vec<serde_json::Value>)> = Vec::new();
        for hosted in &self.hosted {
            let name = hosted.plugin.name();
            let mut records = self.plugin_records_at(&source, seq, name).await;
            records.extend(
                starting
                    .iter()
                    .filter(|(plugin, _)| plugin == name)
                    .map(|(_, body)| body.clone()),
            );
            inherited.push((name.to_owned(), records));
        }
        let mut view = self
            .view(id, prompt, repo)
            .await
            .with_origin(Origin::Fork { from: source, turn });
        for (plugin, records) in inherited {
            view.restate(&plugin, &records);
        }
        Ok(view)
    }

    /// `plugin`'s records a fork of `source` at `seq` inherits: along
    /// `source`'s chain, without what `source` stored after `seq`.
    async fn plugin_records_at(
        &self,
        source: &RunId,
        seq: i64,
        plugin: &str,
    ) -> Vec<serde_json::Value> {
        let all = self.plugin_records(source, plugin).await;
        let after = self
            .store
            .plugin_entries(&source.0, plugin)
            .await
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
    async fn fork_point(
        &self,
        run: &RunId,
        turn: Option<u32>,
    ) -> anyhow::Result<Option<(RunId, i64, Link)>> {
        let mut run = run.clone();
        loop {
            if let Some((seq, link)) = self.link(&run, turn).await? {
                return Ok(Some((run, seq, link)));
            }
            if turn.is_none() {
                return Ok(None);
            }
            let record = self.store.run(&run.0).await?;
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
    pub async fn resume(
        &self,
        run: &RunId,
        prompt: &str,
        choice: &ModelChoice,
    ) -> anyhow::Result<()> {
        self.resume_as(run, prompt, choice, false).await
    }

    /// [`Host::resume`]; `resolving` for tau's turn resolving what a
    /// landing left in conflict on a main chat, whose stop is held once
    /// while conflicts remain.
    async fn resume_as(
        &self,
        run: &RunId,
        prompt: &str,
        choice: &ModelChoice,
        resolving: bool,
    ) -> anyhow::Result<()> {
        if self.is_running(run) {
            anyhow::bail!("The run is still going; steer it instead");
        }
        self.refuse_ended(run).await?;
        let repo = self.slot_of_run(run).await?;
        let state = self.repo_state(&repo.name);
        let _starting = state.starting.lock().await;
        // The workspace its last turn worked in, which still has its
        // files.
        let known = self.session_of(run).workspace;
        // A main chat works in the repository's own checkout, the
        // default workspace, not one of its own.
        let main = self.is_main(run);
        // A run cut off in its first turn has no link yet: the workspace
        // it recorded as it started.
        let workspace = match known {
            _ if main => Some(DEFAULT_WORKSPACE.to_owned()),
            Some(name) => Some(name),
            None => match self.link(run, None).await? {
                Some((_, link)) => Some(link.workspace),
                None => self.started_in(run).await,
            },
        };
        // A main chat catches up with trunk first: its commits move
        // trunk, which may have moved without it.
        if main && let Some(name) = &workspace {
            self.catch_up(&repo.project().await?, name).await?;
        }
        // An effort the model does not take falls back to auto.
        let choice = &choice.clone().fitted();
        // A run without a workspace yet gets one, named after the
        // message.
        let workspace =
            workspace.unwrap_or_else(|| workspace_name(&branch_slug(prompt)));
        let (agent, workspace) = self
            .agent_for_run(choice, &repo, workspace, main, resolving)
            .await?;
        let resumed = agent.resume(run).start(prompt, &self.store);
        self.track(resumed, workspace, choice, &repo.name);
        // Nothing lands on a main chat while it works.
        if main {
            self.main_started(run).await;
        }
        Ok(())
    }

    /// The name of the workspace `run` recorded as it started, if it
    /// did and it is still there.
    async fn started_in(&self, run: &RunId) -> Option<String> {
        let dir = stored_start(&self.store, &run.0).await?.workspace?;
        let dir = Path::new(&dir);
        dir.join(".jj")
            .is_dir()
            .then(|| dir.file_name())
            .flatten()
            .map(|name| name.to_string_lossy().into_owned())
    }

    /// The link of `turn` in `run` (the latest when `None`), with the
    /// `seq` to fork at.
    async fn link(
        &self,
        run: &RunId,
        turn: Option<u32>,
    ) -> anyhow::Result<Option<(i64, Link)>> {
        let entries =
            self.store.plugin_entries(&run.0, WORKSPACE_PLUGIN).await?;
        let mut links = entries.iter().filter_map(|(seq, body)| {
            Link::parse(body).map(|link| (*seq, link))
        });
        Ok(match turn {
            Some(turn) => links.find(|(_, link)| link.turn == turn),
            None => links.next_back(),
        })
    }

    /// `run`'s title: the one a model wrote, or until then the
    /// placeholder for the words it started with.
    pub(crate) async fn title_of(&self, run: &RunId) -> anyhow::Result<String> {
        let written = self
            .store
            .run(&run.0)
            .await?
            .and_then(|record| record.title);
        match written {
            Some(title) => Ok(title),
            None => {
                Ok(crate::titles::placeholder(&self.stored_prompt(run).await?))
            }
        }
    }

    /// The words `run` was started with, from the store.
    async fn stored_prompt(&self, run: &RunId) -> anyhow::Result<String> {
        first_prompt(&self.store, &run.0).await
    }

    /// Runs `future` on the host's runtime and waits for it: for tests,
    /// which are synchronous. `clippy.toml` keeps everything else from
    /// calling it (ADR 0028).
    #[allow(
        clippy::disallowed_methods,
        reason = "the tests' way onto the host's runtime (ADR 0028)"
    )]
    pub fn block_on<F: std::future::Future>(&self, future: F) -> F::Output {
        self.runtime.block_on(future)
    }

    /// What the catalog lists changed: the host builds a new one on its
    /// runtime and pushes it to the interface (ADR 0028). Changes that
    /// come together make one build.
    pub fn catalog_changed(&self) {
        self.catalog_wanted.notify_one();
    }

    /// Starts the catalog's builder, which makes one whenever it is
    /// asked, and pushes it to the receivers it returns. It stops once
    /// the host is gone.
    pub(super) fn follow_catalog(
        self: &Arc<Self>,
    ) -> tokio::sync::watch::Receiver<Option<Catalog>> {
        let (wanted, host) =
            (self.catalog_wanted.clone(), Arc::downgrade(self));
        self.runtime.spawn(async move {
            loop {
                wanted.notified().await;
                let Some(host) = host.upgrade() else {
                    return;
                };
                let catalog = host.catalog().await;
                host.catalog_feed.send_replace(Some(catalog));
            }
        });
        self.catalog_changed();
        self.catalog_feed.subscribe()
    }

    /// The catalog now, for the process's entry point to open its window
    /// with, before the interface runs.
    #[allow(
        clippy::disallowed_methods,
        reason = "the process's entry point, before the interface runs (ADR 0028)"
    )]
    pub fn catalog_now(&self) -> Catalog {
        self.runtime.block_on(self.catalog())
    }

    /// Runs `job`'s future on the host's runtime (ADR 0028): what the
    /// interface asks of the host, which it awaits without blocking.
    pub fn spawn<T, F>(
        self: &Arc<Self>,
        job: impl FnOnce(Arc<Host>) -> F,
    ) -> tokio::task::JoinHandle<T>
    where
        T: Send + 'static,
        F: std::future::Future<Output = T> + Send + 'static,
    {
        self.runtime.spawn(job(self.clone()))
    }

    /// Waits for `future` while the host is being built, before anything
    /// runs on its runtime.
    #[allow(
        clippy::disallowed_methods,
        reason = "building the host: nothing runs on its runtime yet (ADR 0028)"
    )]
    pub(super) fn setting_up<F: std::future::Future>(
        &self,
        future: F,
    ) -> F::Output {
        self.runtime.block_on(future)
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
        self.sessions.lock().expect("not poisoned").insert(
            id.clone(),
            SessionRun {
                repo: Some(repo.to_owned()),
                workspace: Some(workspace),
                choice: Some(choice.clone()),
            },
        );
        self.runs
            .lock()
            .expect("not poisoned")
            .insert(id.clone(), run.control());
        let events = self.events.clone();
        let runs = self.runs.clone();
        let ending = self.ending.clone();
        self.runtime.spawn(async move {
            let id = run.id();
            {
                let mut stream = run.events();
                while let Some(event) = stream.next().await {
                    // Its end goes by before its outcome is stored: it
                    // stops in a moment (`Host::settle`). Marked here, by
                    // the task that unmarks it, so the mark cannot outlive
                    // the run.
                    if matches!(&event, RunEvent::RunEnd { run, .. } if *run == id)
                    {
                        ending.lock().expect("not poisoned").insert(id.clone());
                    }
                    if events.send(event).is_err() {
                        break;
                    }
                }
            }
            // The outcome is stored by the run; the events said it all.
            let _ = run.outcome().await;
            runs.lock().expect("not poisoned").remove(&id);
            ending.lock().expect("not poisoned").remove(&id);
        });
        id
    }

    async fn view(&self, id: RunId, prompt: &str, repo: &RepoSlot) -> RunView {
        let choice = self.session_of(&id).choice.unwrap_or_else(|| {
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
        for (plugin, body) in self.starting(&run).await {
            view.fold(&plugin, &body);
        }
        view.context = ContextWindow {
            window: find(&choice.model).map(|model| model.context_window),
            ..ContextWindow::default()
        };
        let workspace = self.session_of(&view.id).workspace;
        let dir = match (repo.project.ready(), &workspace) {
            (Some(project), Some(name)) => {
                Some(project.workspace_dir(name).display().to_string())
            }
            _ => None,
        };
        view.plugins = Vec::new();
        view.set_base_plan(
            &choice.model,
            choice.effort.label(),
            self.access_label(),
            dir.as_deref().or(workspace.as_ref().map(|_| "")),
        );
        view
    }

    /// Steers `run` with `text` if it is going: it reads it before its
    /// next turn. Returns whether it was going; one that is not goes on
    /// with the message instead ([`Host::resume`]). A chat that landed
    /// or was dropped is refused: it takes no more messages.
    pub async fn steer(&self, run: &RunId, text: &str) -> anyhow::Result<bool> {
        self.refuse_ended(run).await?;
        // A run whose end went by reads nothing more.
        self.settle(run).await;
        let control = self.runs.lock().expect("not poisoned").get(run).cloned();
        let Some(control) = control.or_else(|| self.sub_agent_control(run))
        else {
            return Ok(false);
        };
        control.steer(text);
        self.unread
            .lock()
            .expect("not poisoned")
            .entry(run.clone())
            .or_default()
            .push(text.to_owned());
        Ok(true)
    }

    /// `run` read `text` it was steered with.
    pub fn steer_read(&self, run: &RunId, text: &str) {
        let mut unread = self.unread.lock().expect("not poisoned");
        if let Some(texts) = unread.get_mut(run)
            && let Some(at) = texts.iter().position(|unread| unread == text)
        {
            texts.remove(at);
        }
    }

    /// What `run`, which stopped, was steered with and never read.
    pub fn take_unread(&self, run: &RunId) -> Vec<String> {
        self.unread
            .lock()
            .expect("not poisoned")
            .remove(run)
            .unwrap_or_default()
    }

    /// The model `run` last ran on.
    pub fn choice_of(&self, run: &RunId) -> ModelChoice {
        self.session_of(run).choice.unwrap_or_else(|| {
            ModelChoice::new(self.config.default_model(), Effort::Auto)
        })
    }

    /// How `run` ended for good, if it did: landed on its parent, or
    /// dropped. Such a chat takes no more messages.
    pub async fn ending_of(
        &self,
        run: &RunId,
    ) -> anyhow::Result<Option<Ending>> {
        stored_ending(&self.store, &run.0).await
    }

    /// Fails, saying why, when `run` landed or was dropped: going on
    /// would rebuild its workspace and a new `tau/<run>` bookmark, a
    /// branch nothing lands.
    async fn refuse_ended(&self, run: &RunId) -> anyhow::Result<()> {
        match self.ending_of(run).await? {
            None => Ok(()),
            Some(Ending::Landed { on, .. }) => {
                let on = self.title_of(&on).await?;
                anyhow::bail!(
                    "This chat landed on {on}, so it no longer takes \
                     messages. Its work is on {on}: start a new chat from it."
                )
            }
            Some(Ending::Dropped) => anyhow::bail!(
                "This chat was dropped, so it no longer takes messages. \
                 Start a new chat from main."
            ),
        }
    }

    /// Stops `run`: one of the host's, or a sub-agent going on.
    pub fn cancel(&self, run: &RunId) {
        if let Some(control) = self.runs.lock().expect("not poisoned").get(run)
        {
            control.cancel();
            return;
        }
        self.stop_sub_agent(run);
    }
}

/// The days of runs the Plugins screen's spend covers.
const SPEND_DAYS: u64 = 30;

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

/// A job the host is doing, counted by [`Host::busy`] until dropped.
pub(crate) struct Job(Arc<std::sync::atomic::AtomicUsize>);

impl Drop for Job {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A saved list keeps only repositories from GitHub, and tau's own,
    /// and opens only those it keeps: a local checkout listed before is
    /// dropped, the plugins repository stays (ADR 0027).
    #[test]
    fn a_saved_list_keeps_only_repositories_from_github_and_taus_own() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("repos.json");
        let listed = |name: &str, github: Option<&str>, own: bool| Listed {
            name: name.to_owned(),
            path: dir.path().join(name),
            hidden: false,
            github: github.map(str::to_owned),
            main: None,
            own,
        };
        let saved = RepoList {
            repos: vec![
                listed("tau-agent", None, false),
                listed("ascend", Some("cfcosta/ascend"), false),
                listed("tau-plugins", None, true),
            ],
            open: vec![
                "tau-agent".into(),
                "ascend".into(),
                "tau-plugins".into(),
            ],
            ..RepoList::default()
        };
        tau_testing::block_on_io(saved.save(&path)).unwrap();
        let list = tau_testing::block_on_io(RepoList::load(&path));
        let names: Vec<&str> = list
            .repos
            .iter()
            .map(|listed| listed.name.as_str())
            .collect();
        assert_eq!(names, ["ascend", "tau-plugins"]);
        assert_eq!(list.open, ["ascend", "tau-plugins"]);
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
