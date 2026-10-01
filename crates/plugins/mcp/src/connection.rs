//! One connection per server, shared by every run
//! (`docs/reference/mcp.md`, "Connections").
//!
//! A [`Connection`] starts connecting in the background when the plugin
//! is built, keeps the server's tools and instructions, lists the tools
//! again when the server says they changed, and connects again, lazily,
//! when a call finds it dropped. HTTP connects retry transient errors
//! after 250 ms and 1 s; tool calls are never retried.

use std::{
    fmt,
    path::PathBuf,
    sync::{
        Arc,
        Mutex,
        Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use serde::Serialize;
use serde_json::Value;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use crate::{
    client::{CallError, ConnectError, Endpoint, Session, SessionEvent},
    config::{
        EnvLookup,
        Origin,
        ServerConfig,
        Transport,
        expand_home,
        expand_vars,
    },
};

/// The delays before the second and third try of an HTTP connect.
pub const HTTP_RETRY_DELAYS: [Duration; 2] =
    [Duration::from_millis(250), Duration::from_secs(1)];

/// How long one connect may take, listing the tools included.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Where a connection is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// Connecting, or listing its tools.
    Connecting,
    Connected,
    /// Was connected and dropped; the next call connects again.
    Disconnected,
    /// Could not connect; the next call tries again.
    Failed,
    /// Closed with the plugin, or disabled.
    Closed,
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Connecting => "connecting",
            Self::Connected => "connected",
            Self::Disconnected => "disconnected",
            Self::Failed => "failed",
            Self::Closed => "closed",
        })
    }
}

/// A connection's state and its last error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Status {
    pub state: State,
    pub error: Option<String>,
}

/// The MCP hints a tool gives about itself, kept for the constitution
/// and the interface. `None` is a hint the tool did not give.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Annotations {
    #[serde(rename = "readOnlyHint", skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,
    #[serde(
        rename = "destructiveHint",
        skip_serializing_if = "Option::is_none"
    )]
    pub destructive: Option<bool>,
    #[serde(
        rename = "idempotentHint",
        skip_serializing_if = "Option::is_none"
    )]
    pub idempotent: Option<bool>,
    #[serde(rename = "openWorldHint", skip_serializing_if = "Option::is_none")]
    pub open_world: Option<bool>,
}

/// A tool as its server lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolInfo {
    /// The server's name for it.
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub input_schema: Value,
    pub output_schema: Option<Value>,
    pub annotations: Annotations,
}

/// A progress notification for a call.
#[derive(Debug, Clone, PartialEq)]
pub struct Progress {
    pub progress: f64,
    pub total: Option<f64>,
    pub message: Option<String>,
}

/// Why a call failed before the server gave a result. Its `Display` is
/// what the model reads.
#[derive(Debug, Clone, PartialEq)]
pub enum CallFailure {
    /// The server could not be reached.
    NotConnected { server: String, error: String },
    /// The server no longer lists the tool.
    Withdrawn { server: String, tool: String },
    /// The connection ended during the call. The call is not sent again:
    /// it may have had its effect.
    Disconnected { server: String, tool: String },
    /// No result and no progress within the server's timeout.
    TimedOut {
        server: String,
        tool: String,
        after: Duration,
    },
    /// The run's token fired; the server was told.
    Cancelled { server: String, tool: String },
    /// The server answered with a protocol error.
    Protocol {
        server: String,
        tool: String,
        message: String,
    },
}

impl fmt::Display for CallFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotConnected { server, error } => {
                write!(f, "MCP server {server} is not connected: {error}")
            }
            Self::Withdrawn { server, tool } => {
                write!(f, "Tool {tool} is no longer offered by server {server}")
            }
            Self::Disconnected { server, tool } => write!(
                f,
                "MCP server {server} closed the connection during the call to {tool}; the call was not retried, and it may have had its effect"
            ),
            Self::TimedOut {
                server,
                tool,
                after,
            } => write!(
                f,
                "MCP tool {server}/{tool} gave no result or progress in {} s",
                after.as_secs_f64()
            ),
            Self::Cancelled { server, tool } => {
                write!(f, "The call to MCP tool {server}/{tool} was cancelled")
            }
            Self::Protocol {
                server,
                tool,
                message,
            } => write!(f, "MCP tool {server}/{tool} failed: {message}"),
        }
    }
}

impl std::error::Error for CallFailure {}

/// What resolving a server's entry needs: tau's environment for
/// `${VAR}`, the home directory for `~/`, and the repository, which
/// relative `cwd`s start from and which servers get as their root.
#[derive(Clone)]
pub struct Environment {
    pub env: EnvLookup,
    pub home: Option<PathBuf>,
    pub repo: Option<PathBuf>,
}

impl Environment {
    /// The process's environment and `$HOME`.
    pub fn process(repo: Option<PathBuf>) -> Self {
        Self {
            env: crate::config::process_env(),
            home: std::env::var_os("HOME").map(PathBuf::from),
            repo,
        }
    }
}

impl Environment {
    /// The entry's endpoint, or why it cannot be reached.
    pub(crate) fn endpoint(
        &self,
        config: &ServerConfig,
    ) -> Result<Endpoint, String> {
        let expand = |pairs: &[(String, String)]| {
            pairs
                .iter()
                .map(|(key, value)| {
                    expand_vars(value, &*self.env)
                        .map(|value| (key.clone(), value))
                        .map_err(|name| {
                            format!(
                                "the environment variable `{name}` is not set"
                            )
                        })
                })
                .collect::<Result<Vec<_>, String>>()
        };
        let home = self.home.as_deref();
        Ok(match &config.transport {
            Transport::Stdio(stdio) => Endpoint::Stdio {
                program: expand_home(&stdio.command, home),
                args: stdio
                    .args
                    .iter()
                    .map(|arg| expand_home(arg, home))
                    .collect(),
                env: expand(&stdio.env)?,
                cwd: stdio.cwd.as_ref().map(|cwd| {
                    let cwd = PathBuf::from(expand_home(cwd, home));
                    match &self.repo {
                        Some(repo) if cwd.is_relative() => repo.join(cwd),
                        _ => cwd,
                    }
                }),
            },
            Transport::Http(http) => Endpoint::Http {
                url: http.url.clone(),
                headers: expand(&http.headers)?,
            },
            Transport::Stream(dial) => Endpoint::Stream(dial.clone()),
        })
    }
}

#[derive(Default)]
struct Inner {
    session: Option<Arc<Session>>,
    /// The last tools listed; kept while disconnected, so a call to one
    /// connects again.
    tools: Vec<ToolInfo>,
    instructions: Option<String>,
    connecting: bool,
}

/// One server's connection.
pub struct Connection {
    config: ServerConfig,
    origin: Origin,
    environment: Environment,
    status: watch::Sender<Status>,
    inner: Mutex<Inner>,
    /// Bumped whenever the tools or the instructions change.
    generation: AtomicU64,
}

impl Connection {
    /// A connection to `config`'s server, not started: see
    /// [`Self::connect`]. A disabled server's is closed.
    pub fn new(
        config: ServerConfig,
        origin: Origin,
        environment: Environment,
    ) -> Arc<Self> {
        let state = if config.enabled {
            State::Disconnected
        } else {
            State::Closed
        };
        Arc::new(Self {
            config,
            origin,
            environment,
            status: watch::channel(Status { state, error: None }).0,
            inner: Mutex::default(),
            generation: AtomicU64::new(0),
        })
    }

    pub fn name(&self) -> &str {
        &self.config.name
    }

    pub fn config(&self) -> &ServerConfig {
        &self.config
    }

    pub fn origin(&self) -> Origin {
        self.origin
    }

    /// A number that grows whenever the tools or instructions change.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    pub fn status(&self) -> Status {
        self.status.borrow().clone()
    }

    fn set_status(&self, state: State, error: Option<String>) {
        self.status.send_replace(Status { state, error });
    }

    fn changed(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// The tools last listed.
    pub fn tools(&self) -> Vec<ToolInfo> {
        self.inner.lock().expect("connection lock").tools.clone()
    }

    pub fn instructions(&self) -> Option<String> {
        self.inner
            .lock()
            .expect("connection lock")
            .instructions
            .clone()
    }

    /// Whether the server lists `tool` now.
    pub fn offers(&self, tool: &str) -> bool {
        self.inner
            .lock()
            .expect("connection lock")
            .tools
            .iter()
            .any(|info| info.name == tool)
    }

    /// Starts connecting in the background, unless the connection is
    /// connected, connecting, or closed.
    pub fn connect(self: &Arc<Self>) {
        {
            let mut inner = self.inner.lock().expect("connection lock");
            let live = inner.session.as_ref().is_some_and(|s| !s.is_closed());
            if live || inner.connecting || self.status().state == State::Closed
            {
                return;
            }
            inner.connecting = true;
            self.set_status(State::Connecting, None);
        }
        let this = self.clone();
        tokio::spawn(async move { this.run_connect().await });
    }

    async fn run_connect(self: Arc<Self>) {
        let outcome = tokio::time::timeout(CONNECT_TIMEOUT, self.open())
            .await
            .unwrap_or_else(|_| {
                Err(format!("no answer in {} s", CONNECT_TIMEOUT.as_secs()))
            });
        let mut inner = self.inner.lock().expect("connection lock");
        inner.connecting = false;
        if self.status().state == State::Closed {
            if let Ok((session, ..)) = outcome {
                tokio::spawn(async move { session.close().await });
            }
            return;
        }
        match outcome {
            Ok((session, tools, events)) => {
                inner.instructions = session.instructions().map(str::to_owned);
                inner.tools = tools;
                inner.session = Some(session.clone());
                drop(inner);
                self.set_status(State::Connected, None);
                self.changed();
                tokio::spawn(watch_session(
                    Arc::downgrade(&self),
                    session,
                    events,
                ));
            }
            Err(error) => {
                drop(inner);
                self.set_status(State::Failed, Some(error));
            }
        }
    }

    /// Connects, retrying an HTTP server's transient errors, and lists
    /// the tools.
    async fn open(
        &self,
    ) -> Result<
        (
            Arc<Session>,
            Vec<ToolInfo>,
            mpsc::UnboundedReceiver<SessionEvent>,
        ),
        String,
    > {
        let endpoint = self.environment.endpoint(&self.config)?;
        let mut attempt = 0;
        let (session, events) = loop {
            let (sender, events) = mpsc::unbounded_channel();
            let result = Session::connect(
                &self.config.name,
                &endpoint,
                self.environment.repo.as_deref(),
                sender,
            )
            .await;
            match result {
                Ok(session) => break (Arc::new(session), events),
                Err(ConnectError {
                    transient: true, ..
                }) if endpoint.is_http()
                    && attempt < HTTP_RETRY_DELAYS.len() =>
                {
                    tokio::time::sleep(HTTP_RETRY_DELAYS[attempt]).await;
                    attempt += 1;
                }
                Err(error) => return Err(error.message),
            }
        };
        match session.list_tools().await {
            Ok(tools) => Ok((session, tools, events)),
            Err(error) => {
                session.close().await;
                Err(error)
            }
        }
    }

    /// A live session, connecting first when there is none. Waits for a
    /// connect under way. Fails when the connect fails, the connection is
    /// closed, or `cancel` fires.
    pub(crate) async fn session(
        self: &Arc<Self>,
        cancel: &CancellationToken,
    ) -> Result<Arc<Session>, String> {
        loop {
            {
                let inner = self.inner.lock().expect("connection lock");
                if let Some(session) = &inner.session
                    && !session.is_closed()
                {
                    return Ok(session.clone());
                }
            }
            let mut status = self.status.subscribe();
            self.connect();
            let settled = tokio::select! {
                status = status.wait_for(|status| status.state != State::Connecting) => {
                    status.map(|status| status.clone()).ok()
                }
                () = cancel.cancelled() => return Err("cancelled".into()),
            };
            match settled {
                Some(Status {
                    state: State::Connected,
                    ..
                }) => {}
                Some(Status {
                    state: State::Closed,
                    ..
                })
                | None => {
                    return Err("the connection is closed".into());
                }
                Some(Status { error, .. }) => {
                    return Err(
                        error.unwrap_or_else(|| "cannot connect".into())
                    );
                }
            }
        }
    }

    /// Calls the server's tool `tool` with `args` (a JSON object, or
    /// null for none) and returns its `CallToolResult` as JSON. Connects
    /// first when the connection dropped; never sends the call twice.
    /// The server's timeout starts again at each progress notification,
    /// which also goes to `on_progress`. `cancel` sends
    /// `notifications/cancelled`; the server may still finish the call.
    pub async fn call(
        self: &Arc<Self>,
        tool: &str,
        args: Value,
        on_progress: &(dyn Fn(Progress) + Send + Sync),
        cancel: &CancellationToken,
    ) -> Result<Value, CallFailure> {
        let server = self.config.name.clone();
        let session = self.session(cancel).await.map_err(|error| {
            if cancel.is_cancelled() {
                CallFailure::Cancelled {
                    server: server.clone(),
                    tool: tool.to_owned(),
                }
            } else {
                CallFailure::NotConnected {
                    server: server.clone(),
                    error,
                }
            }
        })?;
        if !self.offers(tool) {
            return Err(CallFailure::Withdrawn {
                server,
                tool: tool.to_owned(),
            });
        }
        let arguments = match args {
            Value::Object(map) => Some(map),
            Value::Null => None,
            _ => {
                return Err(CallFailure::Protocol {
                    server,
                    tool: tool.to_owned(),
                    message: "the arguments must be a JSON object".into(),
                });
            }
        };
        let result = session
            .call(tool, arguments, self.config.timeout(), on_progress, cancel)
            .await;
        let tool = tool.to_owned();
        result.map_err(|error| match error {
            CallError::Disconnected => {
                self.lost(&session);
                CallFailure::Disconnected { server, tool }
            }
            CallError::Timeout(after) => CallFailure::TimedOut {
                server,
                tool,
                after,
            },
            CallError::Cancelled => CallFailure::Cancelled { server, tool },
            CallError::Protocol(message) => CallFailure::Protocol {
                server,
                tool,
                message,
            },
        })
    }

    /// Waits until the connection is not connecting, or `cancel`.
    pub async fn settled(&self, cancel: &CancellationToken) {
        let mut status = self.status.subscribe();
        tokio::select! {
            _ = status.wait_for(|status| status.state != State::Connecting) => {}
            () = cancel.cancelled() => {}
        }
    }

    /// Forgets `session` after it failed under a call: the next call
    /// connects again.
    pub(crate) fn lost(&self, session: &Arc<Session>) {
        let mut inner = self.inner.lock().expect("connection lock");
        if inner
            .session
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, session))
        {
            inner.session = None;
            drop(inner);
            if self.status().state == State::Connected {
                self.set_status(
                    State::Disconnected,
                    Some("the server closed the connection".into()),
                );
            }
        }
    }

    /// Closes the connection for good.
    pub async fn shutdown(&self) {
        let session = {
            let mut inner = self.inner.lock().expect("connection lock");
            self.set_status(State::Closed, None);
            inner.session.take()
        };
        if let Some(session) = session {
            session.close().await;
        }
    }

    /// Lists the tools again, after the server said they changed.
    async fn relist(&self, session: &Arc<Session>) {
        let Ok(tools) = session.list_tools().await else {
            return;
        };
        let mut inner = self.inner.lock().expect("connection lock");
        if inner
            .session
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, session))
        {
            inner.tools = tools;
            drop(inner);
            self.changed();
        }
    }
}

/// Follows a session's events for as long as its connection lives.
async fn watch_session(
    connection: Weak<Connection>,
    session: Arc<Session>,
    mut events: mpsc::UnboundedReceiver<SessionEvent>,
) {
    while let Some(event) = events.recv().await {
        let Some(connection) = connection.upgrade() else {
            return;
        };
        match event {
            SessionEvent::ToolsChanged => connection.relist(&session).await,
            SessionEvent::Closed => {
                connection.lost(&session);
                return;
            }
        }
    }
}
