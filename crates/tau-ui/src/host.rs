//! Runs real agents behind the workspace: the seams from
//! [`crate::workspace`] wired to `tau-agent`.
//!
//! The host owns a tokio runtime beside GPUI's executor. A new run starts
//! on that runtime; a task reads its events and sends them over a channel
//! the workspace drains on the UI thread. Steering and cancelling go the
//! other way through each run's [`RunControl`].
//!
//! Runs do not work in the user's checkout. The host copies it into a
//! [`Project`] under `$XDG_DATA_HOME/tau/repos`, and each run gets a jj
//! workspace there ([`RunWorkspace`]) with a commit per turn, so a fork
//! starts from a turn's conversation and code. Past runs come back from
//! the store as history.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use futures_util::StreamExt;
use gpui::{App, Entity};
use tau_agent::{
    agent::{Agent, Checkpoint, RunControl},
    event::{RunEvent, StopReason},
    limits::Limits,
    tool::RunId,
};
use tau_ai::{
    client::OpenAi,
    codex::{
        BrowserLogin,
        CodexAuth,
        CodexCredentials,
        DeviceLogin,
        ORIGINATOR,
        oauth,
    },
    message::{Message, UserContent},
    model::find,
};
use tau_compaction::Compaction;
use tau_store::{Entry, RunKind, Status, Store};
use tau_tools::{path::Root, plugin::CodingTools};
use tau_vcs::{
    Identity,
    Link,
    Project,
    RunWorkspace,
    VcsPlugin,
    run_workspace::PLUGIN as WORKSPACE_PLUGIN,
};
use tokio::{runtime::Runtime, sync::mpsc};

use crate::{
    catalog::{Catalog, PluginInfo, PluginScreen, Seam, StoreInfo},
    setup::{DeviceCode, GitHub, ModelAccess, SetupUpdate},
    view::{
        ChildKind,
        ChildRun,
        ContextWindow,
        Limits as ViewLimits,
        NoteBody,
        Origin,
        PlanField,
        PluginNote,
        PluginStatus,
        RunUpdate,
        RunView,
        Tone,
    },
    workspace::{Workspace, WorkspaceEvent},
};

/// How the host reaches a model.
#[derive(Clone, PartialEq, Eq)]
pub enum Access {
    /// A ChatGPT sign-in, from this credentials file.
    Codex(PathBuf),
    /// An OpenAI API key.
    ApiKey(String),
}

impl std::fmt::Debug for Access {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Codex(path) => f.debug_tuple("Codex").field(path).finish(),
            Self::ApiKey(_) => f.write_str("ApiKey(..)"),
        }
    }
}

impl Access {
    /// Codex credentials if saved, else `OPENAI_API_KEY`, else a key
    /// saved during onboarding.
    pub fn detect() -> Option<Self> {
        if let Some(path) =
            CodexCredentials::default_path().filter(|path| path.exists())
        {
            return Some(Self::Codex(path));
        }
        std::env::var(tau_ai::client::API_KEY_VAR)
            .ok()
            .or_else(|| {
                api_key_path()
                    .and_then(|path| std::fs::read_to_string(path).ok())
            })
            .map(|key| key.trim().to_owned())
            .filter(|key| !key.is_empty())
            .map(Self::ApiKey)
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Codex(_) => "ChatGPT (Codex)",
            Self::ApiKey(_) => "OpenAI API key",
        }
    }

    /// How the model reads in onboarding: `gpt-5.5 · Codex`.
    fn short_label(&self, model: &str) -> String {
        match self {
            Self::Codex(_) => format!("{model} · Codex"),
            Self::ApiKey(_) => format!("{model} · API key"),
        }
    }
}

/// Where onboarding keeps an API key: `$XDG_CONFIG_HOME/tau/openai-key`,
/// beside the Codex credentials.
pub fn api_key_path() -> Option<PathBuf> {
    CodexCredentials::default_path()
        .and_then(|path| path.parent().map(|dir| dir.join("openai-key")))
}

/// Saves an API key, readable only by the user.
pub fn save_api_key(key: &str) -> std::io::Result<PathBuf> {
    use std::io::Write;
    let path = api_key_path().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "no home directory")
    })?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(&path)?.write_all(key.as_bytes())?;
    Ok(path)
}

/// What the host needs to start.
#[derive(Debug, Clone)]
pub struct HostConfig {
    pub access: Access,
    pub model: String,
    /// The checkout runs work on. The host clones it into a project of
    /// its own and gives each run a workspace there; if it cannot (not
    /// a Git repository, no `git`), runs work in it directly.
    pub root: PathBuf,
    /// The run store, usually `$XDG_DATA_HOME/tau/runs.db`.
    pub store: PathBuf,
    /// Where projects live, usually `$XDG_DATA_HOME/tau/repos`.
    pub repos: PathBuf,
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

    /// The project directory for `root`: its name and a hash of its full
    /// path, so two checkouts with one name get two projects.
    pub fn project_dir(&self) -> PathBuf {
        let full = std::fs::canonicalize(&self.root)
            .unwrap_or_else(|_| self.root.clone());
        let name = full.file_name().map_or("project".into(), |name| {
            name.to_string_lossy().into_owned()
        });
        self.repos
            .join(format!("{name}-{:08x}", fnv(&full.to_string_lossy())))
    }
}

/// A stable 32-bit FNV-1a hash, for directory names.
fn fnv(text: &str) -> u32 {
    text.bytes().fold(0x811c_9dc5, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    })
}

pub struct Host {
    runtime: Runtime,
    /// The agent every run starts from; each run adds its own tools.
    base: Agent,
    store: Store,
    config: HostConfig,
    project: Option<Project>,
    runs: Arc<Mutex<HashMap<RunId, RunControl>>>,
    /// The workspace each run of this session works in.
    workspaces: Arc<Mutex<HashMap<RunId, String>>>,
    events: mpsc::UnboundedSender<RunEvent>,
}

const MAX_TURNS: u32 = 50;

/// How many past runs history shows.
const HISTORY: u32 = 50;

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
        // Clients must be created inside the runtime.
        let client = {
            let _guard = runtime.enter();
            match &config.access {
                Access::Codex(path) => {
                    OpenAi::codex(CodexAuth::from_file(path)?)
                }
                Access::ApiKey(key) => OpenAi::new(key.clone()),
            }
        };
        let mut compaction = Compaction::default();
        if let Some(model) = find(&config.model) {
            compaction = compaction.context_window(model.context_window);
        }
        let agent = Agent::new(client)
            .name("coder")
            .model(&config.model)
            .instructions(INSTRUCTIONS)
            .limits(Limits::default().max_turns(MAX_TURNS))
            .plugin(compaction);
        let project = match Project::open_or_import(
            &config.root.to_string_lossy(),
            config.project_dir(),
            identity(),
        ) {
            Ok(project) => Some(project),
            Err(error) => {
                eprintln!(
                    "tau-ui: runs will work in {} itself: {error:#}",
                    config.root.display()
                );
                None
            }
        };
        let (mut host, events) =
            Self::with_agent(runtime, agent, store, config);
        host.project = project;
        Ok((host, events))
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
        let host = Self {
            runtime,
            base: agent,
            store,
            config,
            project: None,
            runs: Arc::default(),
            workspaces: Arc::default(),
            events,
        };
        (host, receiver)
    }

    /// Gives each run a workspace in `project`, and a commit per turn.
    pub fn with_project(mut self, project: Project) -> Self {
        self.project = Some(project);
        self
    }

    pub fn project(&self) -> Option<&Project> {
        self.project.as_ref()
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
        Some(self.project.as_ref()?.workspace_dir(&name))
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
        if self.project.is_some() {
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
        let source = match &self.project {
            Some(project) => format!(
                "{} · {} → {}",
                self.config.access.label(),
                self.config.root.display(),
                project.root().display()
            ),
            None => format!(
                "{} · {}",
                self.config.access.label(),
                self.config.root.display()
            ),
        };
        Catalog {
            agent: "coder".into(),
            agent_source: Some(source),
            plugins,
            jev: None,
            memory: Default::default(),
            constitution: Default::default(),
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
        }
    }

    /// The agent for one run, with its tools on the run's workspace, and
    /// the workspace's name.
    fn agent_for_run(&self) -> anyhow::Result<(Agent, Option<String>)> {
        let Some(project) = &self.project else {
            let tools = CodingTools::new(Root::new(self.config.root.clone()));
            return Ok((self.base.clone().plugin(tools), None));
        };
        let name = workspace_name();
        let workspace = RunWorkspace::new(project.clone(), &name, identity())?;
        let agent = self
            .base
            .clone()
            .plugin(CodingTools::new(Root::new(workspace.dir())))
            .plugin(VcsPlugin::new(workspace.vcs().clone()))
            .plugin(workspace);
        Ok((agent, Some(name)))
    }

    /// Starts a run and returns its view, ready to be pushed into the
    /// workspace before its first event arrives.
    pub fn start(&self, prompt: &str) -> anyhow::Result<RunView> {
        let (agent, workspace) = self.agent_for_run()?;
        let _guard = self.runtime.enter();
        let run = agent.start(prompt, &self.store);
        let id = self.track(run, workspace);
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
    ) -> anyhow::Result<RunView> {
        if self.project.is_none() {
            anyhow::bail!(
                "Forking needs a project: {} could not be cloned",
                self.config.root.display()
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
        let (agent, workspace) = self.agent_for_run()?;
        let _guard = self.runtime.enter();
        let forked = agent
            .fork(&Checkpoint::at(run.clone(), seq))
            .start(prompt, &self.store);
        let id = self.track(forked, workspace);
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
        let Some(project) = self.project.clone() else {
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

    /// Runs from earlier sessions, newest first, rebuilt from the store.
    pub fn history(&self) -> anyhow::Result<Vec<RunView>> {
        self.runtime.block_on(history(&self.store))
    }

    /// Follows a started run: its control, its workspace, and a task
    /// that forwards its events.
    fn track(
        &self,
        mut run: tau_agent::agent::Run,
        workspace: Option<String>,
    ) -> RunId {
        let id = run.id();
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
        let mut view =
            RunView::new(id, title(prompt), "coder", &self.config.model)
                .started("just now");
        view.push_user(prompt);
        view.limits = ViewLimits {
            max_turns: Some(MAX_TURNS),
            ..ViewLimits::default()
        };
        view.context = ContextWindow {
            window: find(&self.config.model).map(|model| model.context_window),
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
                value: self.config.model.clone(),
                set_by: None,
            },
            PlanField {
                name: "access".into(),
                value: self.config.access.label().into(),
                set_by: None,
            },
            PlanField {
                name: "workspace".into(),
                value: match (&self.project, &workspace) {
                    (Some(project), Some(name)) => {
                        project.workspace_dir(name).display().to_string()
                    }
                    _ => self.config.root.display().to_string(),
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
        let handler = host.clone();
        cx.subscribe(
            workspace,
            move |workspace, event: &WorkspaceEvent, cx| match event {
                WorkspaceEvent::NewRun { prompt } => {
                    match handler.start(prompt) {
                        Ok(view) => {
                            workspace.update(cx, |ws, cx| ws.push_run(view, cx))
                        }
                        Err(error) => {
                            eprintln!("tau-ui: cannot start the run: {error:#}")
                        }
                    }
                }
                WorkspaceEvent::Fork { run, turn, prompt } => {
                    match handler.fork(run, *turn, prompt) {
                        Ok(view) => {
                            workspace.update(cx, |ws, cx| ws.push_run(view, cx))
                        }
                        Err(error) => workspace.update(cx, |ws, cx| {
                            ws.update_run(
                                run,
                                RunUpdate::Note(PluginNote {
                                    plugin: "workspace".into(),
                                    text: format!("could not fork: {error:#}"),
                                    detail: None,
                                    tone: Tone::Danger,
                                    body: NoteBody::None,
                                }),
                                cx,
                            );
                        }),
                    }
                }
                WorkspaceEvent::KeepBranch { run } => {
                    if let Err(error) = handler.keep_branch(run) {
                        eprintln!(
                            "tau-ui: cannot drop the other branches: {error:#}"
                        );
                    }
                }
                WorkspaceEvent::Steer { run, text } => handler.steer(run, text),
                WorkspaceEvent::Cancel { run } => handler.cancel(run),
                other => eprintln!("tau-ui: not handled yet: {other:?}"),
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

/// A new workspace's name: unique, and sorting by when it was made.
fn workspace_name() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNT: AtomicU32 = AtomicU32::new(0);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    format!("run-{millis:x}-{}", COUNT.fetch_add(1, Ordering::Relaxed))
}

/// Past runs, rebuilt from their stored transcripts.
pub async fn history(store: &Store) -> anyhow::Result<Vec<RunView>> {
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
            _ => users.into_iter().next(),
        }
        .unwrap_or_default();
        let mut view = RunView::from_messages(
            RunId(record.id.clone().into()),
            title(&prompt),
            &record.agent,
            &record.model,
            &messages,
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

/// How a sign-in running on its own thread is going.
enum SignIn {
    Code(DeviceCode),
    Done(Result<CodexCredentials, String>),
}

/// Carries out onboarding when no model is configured: the ChatGPT
/// sign-in (browser or device code) and API keys. Once one works and is
/// saved, `ready` gets the new [`Access`] to build a [`Host`] from.
///
/// GitHub needs tau's GitHub App, which does not exist yet, so GitHub
/// sign-ins answer with an error and onboarding starts at the model.
pub fn onboard(
    workspace: &Entity<Workspace>,
    model: String,
    cx: &mut App,
    ready: impl Fn(Access, &mut App) + 'static,
) {
    let ready = std::rc::Rc::new(ready);
    cx.subscribe(workspace, move |workspace, event: &WorkspaceEvent, cx| {
        let connect = {
            let ready = ready.clone();
            let model = model.clone();
            move |access: Access,
                  workspace: &Entity<Workspace>,
                  cx: &mut App| {
                let label = access.short_label(&model);
                ready(access, cx);
                workspace.update(cx, |ws, cx| {
                    ws.update_setup(
                        SetupUpdate::Model(ModelAccess::Connected { label }),
                        cx,
                    )
                });
            }
        };
        let fail =
            |workspace: &Entity<Workspace>, error: String, cx: &mut App| {
                workspace.update(cx, |ws, cx| {
                    ws.update_setup(
                        SetupUpdate::Model(ModelAccess::Failed(error)),
                        cx,
                    )
                });
            };
        match event {
            WorkspaceEvent::CodexSignIn { device } => {
                let browser =
                    (!device).then(|| BrowserLogin::start(ORIGINATOR));
                if let Some(login) = &browser {
                    cx.open_url(&login.url);
                    let pending = ModelAccess::SigningIn {
                        url: Some(login.url.clone()),
                        device: None,
                    };
                    workspace.update(cx, |ws, cx| {
                        ws.update_setup(SetupUpdate::Model(pending), cx)
                    });
                }
                let (progress, mut updates) = mpsc::unbounded_channel();
                std::thread::spawn(move || {
                    let done = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|error| error.to_string())
                        .and_then(|runtime| {
                            runtime.block_on(sign_in(browser, &progress))
                        });
                    let _ = progress.send(SignIn::Done(done));
                });
                let workspace = workspace.downgrade();
                cx.spawn(async move |cx| {
                    while let Some(update) = updates.recv().await {
                        let Some(workspace) = workspace.upgrade() else {
                            return;
                        };
                        let applied = cx.update(|cx| match update {
                            SignIn::Code(code) => {
                                let pending = ModelAccess::SigningIn {
                                    url: None,
                                    device: Some(code),
                                };
                                workspace.update(cx, |ws, cx| {
                                    ws.update_setup(
                                        SetupUpdate::Model(pending),
                                        cx,
                                    )
                                });
                            }
                            SignIn::Done(Ok(credentials)) => {
                                match CodexCredentials::default_path()
                                    .ok_or_else(|| {
                                        "no home directory".to_owned()
                                    })
                                    .and_then(|path| {
                                        credentials
                                            .save(&path)
                                            .map(|()| path)
                                            .map_err(|error| error.to_string())
                                    }) {
                                    Ok(path) => connect(
                                        Access::Codex(path),
                                        &workspace,
                                        cx,
                                    ),
                                    Err(error) => fail(&workspace, error, cx),
                                }
                            }
                            SignIn::Done(Err(error)) => {
                                fail(&workspace, error, cx)
                            }
                        });
                        if applied.is_err() {
                            return;
                        }
                    }
                })
                .detach();
            }
            WorkspaceEvent::ApiKey { key } => match save_api_key(key) {
                Ok(_) => connect(Access::ApiKey(key.clone()), &workspace, cx),
                Err(error) => fail(&workspace, error.to_string(), cx),
            },
            WorkspaceEvent::GitHubSignIn
            | WorkspaceEvent::GitHubCheck
            | WorkspaceEvent::GitHubToken { .. } => {
                workspace.update(cx, |ws, cx| {
                    ws.update_setup(
                        SetupUpdate::GitHub(GitHub::Failed(
                            "tau cannot sign in to GitHub yet: it needs its \
                             GitHub App. Connect a model to start."
                                .into(),
                        )),
                        cx,
                    )
                });
            }
            _ => {}
        }
    })
    .detach();
}

/// Signs in to ChatGPT: waits for the browser, or asks for a device code
/// and reports it before waiting.
async fn sign_in(
    browser: Option<BrowserLogin>,
    progress: &mpsc::UnboundedSender<SignIn>,
) -> Result<CodexCredentials, String> {
    let credentials = match browser {
        Some(login) => login.wait().await,
        None => {
            let login =
                DeviceLogin::start().await.map_err(|e| e.to_string())?;
            let url = oauth::DEVICE_VERIFICATION_URL;
            let _ = progress.send(SignIn::Code(DeviceCode {
                code: login.user_code.clone(),
                url: url.trim_start_matches("https://").to_owned(),
                expires: "15 minutes".into(),
            }));
            login.wait().await
        }
    };
    credentials.map_err(|error| error.to_string())
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
