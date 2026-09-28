//! Runs real agents behind the workspace: the seams from
//! [`crate::workspace`] wired to `tau-agent`.
//!
//! The host owns a tokio runtime beside GPUI's executor. A new run starts
//! on that runtime; a task reads its events and sends them over a channel
//! the workspace drains on the UI thread. Steering and cancelling go the
//! other way through each run's [`RunControl`].

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use futures_util::StreamExt;
use gpui::{App, Entity};
use tau_agent::{
    agent::{Agent, RunControl},
    event::RunEvent,
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
    model::find,
};
use tau_compaction::Compaction;
use tau_store::Store;
use tau_tools::{path::Root, plugin::CodingTools};
use tokio::{runtime::Runtime, sync::mpsc};

use crate::{
    catalog::{Catalog, PluginInfo, PluginScreen, Seam, StoreInfo},
    setup::{DeviceCode, GitHub, ModelAccess, SetupUpdate},
    view::{
        ContextWindow,
        Limits as ViewLimits,
        PlanField,
        PluginStatus,
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
    /// The directory the coding tools work in.
    pub root: PathBuf,
    /// The run store, usually `$XDG_DATA_HOME/tau/runs.db`.
    pub store: PathBuf,
}

impl HostConfig {
    pub fn default_store() -> PathBuf {
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(|home| PathBuf::from(home).join(".local/share"))
            })
            .unwrap_or_else(|| PathBuf::from("."));
        data.join("tau").join("runs.db")
    }
}

pub struct Host {
    runtime: Runtime,
    agent: Agent,
    store: Store,
    config: HostConfig,
    runs: Arc<Mutex<HashMap<RunId, RunControl>>>,
    events: mpsc::UnboundedSender<RunEvent>,
}

const MAX_TURNS: u32 = 50;

const INSTRUCTIONS: &str = "You are tau, a coding agent working in the \
    user's repository. Use the tools to read and change files and to run \
    commands. Be concise, and say which tests you ran.";

impl Host {
    /// Opens the store and builds the agent. Returns the receiving end of
    /// the event channel for [`Host::attach`].
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
            .plugin(CodingTools::new(Root::new(config.root.clone())))
            .plugin(compaction);
        Ok(Self::with_agent(runtime, agent, store, config))
    }

    /// A host over an agent built elsewhere: another model, other
    /// plugins, or a scripted model in tests.
    pub fn with_agent(
        runtime: Runtime,
        agent: Agent,
        store: Store,
        config: HostConfig,
    ) -> (Self, mpsc::UnboundedReceiver<RunEvent>) {
        let (events, receiver) = mpsc::unbounded_channel();
        let host = Self {
            runtime,
            agent,
            store,
            config,
            runs: Arc::default(),
            events,
        };
        (host, receiver)
    }

    /// Whether `run` is still going.
    pub fn is_running(&self, run: &RunId) -> bool {
        self.runs.lock().expect("not poisoned").contains_key(run)
    }

    /// What the workspace shows beyond runs: the agent's plugins and the
    /// store.
    pub fn catalog(&self) -> Catalog {
        Catalog {
            agent: "coder".into(),
            agent_source: Some(format!(
                "{} · {}",
                self.config.access.label(),
                self.config.root.display()
            )),
            plugins: vec![
                PluginInfo {
                    name: "tau-tools".into(),
                    description: "read bash edit write grep find ls".into(),
                    seams: vec![Seam::Tools],
                    spend: 0.0,
                    screen: None,
                },
                PluginInfo {
                    name: "tau-compaction".into(),
                    description:
                        "Summarizes the context when it nears the window".into(),
                    seams: vec![Seam::Start, Seam::Rewrite],
                    spend: 0.0,
                    screen: Some(PluginScreen::Ledger),
                },
            ],
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

    /// Starts a run and returns its view, ready to be pushed into the
    /// workspace before its first event arrives.
    pub fn start(&self, prompt: &str) -> RunView {
        let _guard = self.runtime.enter();
        let mut run = self.agent.start(prompt, &self.store);
        let id = run.id();
        self.runs
            .lock()
            .expect("not poisoned")
            .insert(id.clone(), run.control());
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
        self.view(id, prompt)
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
                name: "root".into(),
                value: self.config.root.display().to_string(),
                set_by: None,
            },
        ];
        view.plugins = vec![
            PluginStatus {
                name: "tau-tools".into(),
                state: "7 tools".into(),
                tone: Tone::Quiet,
            },
            PluginStatus {
                name: "tau-compaction".into(),
                state: "watching the window".into(),
                tone: Tone::Quiet,
            },
        ];
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
    /// runs' events drive the workspace.
    pub fn attach(
        self,
        workspace: &Entity<Workspace>,
        mut events: mpsc::UnboundedReceiver<RunEvent>,
        cx: &mut App,
    ) {
        let host = Arc::new(self);
        let handler = host.clone();
        cx.subscribe(
            workspace,
            move |workspace, event: &WorkspaceEvent, cx| match event {
                WorkspaceEvent::NewRun { prompt } => {
                    let view = handler.start(prompt);
                    workspace.update(cx, |ws, cx| ws.push_run(view, cx));
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
