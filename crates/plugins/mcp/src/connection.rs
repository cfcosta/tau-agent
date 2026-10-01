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
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use crate::{
    auth::{GrantKey, SignInRequest, TokenStore},
    client::{
        CallError,
        ConnectError,
        Endpoint,
        Features,
        HttpAuth,
        Session,
        SessionEvent,
    },
    config::{
        EnvLookup,
        HttpConfig,
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
    /// The server asked for a sign-in (or for more scopes). Calls fail
    /// at once until someone signs in, here or in another process;
    /// nothing opens a browser on its own.
    #[serde(rename = "needs-auth")]
    NeedsAuth,
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
            Self::NeedsAuth => "needs-auth",
            Self::Closed => "closed",
        })
    }
}

/// Why a server wants a sign-in.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AuthNeed {
    /// The server's `WWW-Authenticate` challenge, when it gave one.
    pub challenge: Option<String>,
    /// Scopes it said it needs beyond those granted
    /// (`insufficient_scope`).
    pub scope: Option<String>,
}

impl AuthNeed {
    pub(crate) fn challenged(challenge: &str) -> Self {
        Self {
            challenge: Some(challenge.trim().to_owned()),
            scope: None,
        }
    }

    /// What the connection's error says.
    pub fn message(&self) -> String {
        match &self.scope {
            Some(scope) => format!(
                "the server needs more access ({scope}): sign in again on \
                 the MCP servers page"
            ),
            None => "the server asks you to sign in: sign in on the MCP \
                     servers page"
                .to_owned(),
        }
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
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize,
)]
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

/// A resource as its server lists it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ResourceInfo {
    pub uri: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

/// A resource template as its server lists it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TemplateInfo {
    pub uri_template: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

/// A prompt as its server lists it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PromptInfo {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub arguments: Vec<PromptArgument>,
}

/// One of a prompt's arguments.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PromptArgument {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub required: bool,
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
    /// Where sign-ins are kept; without it, OAuth does not apply and a
    /// 401 fails the server.
    pub auth: Option<TokenStore>,
}

impl Environment {
    /// The process's environment and `$HOME`, without a grants file.
    pub fn process(repo: Option<PathBuf>) -> Self {
        Self {
            env: crate::config::process_env(),
            home: std::env::var_os("HOME").map(PathBuf::from),
            repo,
            auth: None,
        }
    }

    /// The same, keeping sign-ins in `store`.
    pub fn with_auth(mut self, store: Option<TokenStore>) -> Self {
        self.auth = store;
        self
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
                auth: match self.oauth(config) {
                    Some((store, key)) => Some(HttpAuth {
                        store,
                        key,
                        client_secret: self.client_secret(http)?,
                    }),
                    None => None,
                },
            },
            Transport::Stream(dial) => Endpoint::Stream(dial.clone()),
        })
    }
}

impl Environment {
    /// Where `config`'s sign-in is kept, when OAuth applies: an HTTP
    /// server without an `Authorization` header, and a grants file.
    pub fn oauth(
        &self,
        config: &ServerConfig,
    ) -> Option<(TokenStore, GrantKey)> {
        let Transport::Http(http) = &config.transport else {
            return None;
        };
        let store = self.auth.clone().filter(|_| http.uses_oauth())?;
        let key = GrantKey {
            url: http.url.clone(),
            client: http.oauth.as_ref().and_then(|o| o.client_id.clone()),
        };
        Some((store, key))
    }

    /// The configured client's secret, `${VAR}` expanded.
    fn client_secret(
        &self,
        http: &HttpConfig,
    ) -> Result<Option<String>, String> {
        http.oauth
            .as_ref()
            .and_then(|oauth| oauth.client_secret.as_deref())
            .map(|secret| {
                expand_vars(secret, &*self.env).map_err(|name| {
                    format!("the environment variable `{name}` is not set")
                })
            })
            .transpose()
    }
}

#[derive(Default)]
struct Inner {
    session: Option<Arc<Session>>,
    /// The last tools listed; kept while disconnected, so a call to one
    /// connects again.
    tools: Vec<ToolInfo>,
    /// What the server offers besides tools, and the lists of it; kept
    /// while disconnected, like the tools.
    features: Features,
    resources: Vec<ResourceInfo>,
    templates: Vec<TemplateInfo>,
    prompts: Vec<PromptInfo>,
    instructions: Option<String>,
    /// The protocol version of the last session.
    protocol: Option<String>,
    connecting: bool,
}

/// What a connect lists: tools always, resources, templates and prompts
/// when the server offers them.
struct Lists {
    tools: Vec<ToolInfo>,
    resources: Vec<ResourceInfo>,
    templates: Vec<TemplateInfo>,
    prompts: Vec<PromptInfo>,
}

/// A read-only request: reading a resource or getting a prompt.
enum ReadOnly<'a> {
    Resource(&'a str),
    Prompt(&'a str, serde_json::Map<String, Value>),
}

impl ReadOnly<'_> {
    fn what(&self) -> String {
        match self {
            Self::Resource(uri) => format!("the read of {uri}"),
            Self::Prompt(name, _) => format!("the prompt {name}"),
        }
    }
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
    /// Whether it ever started connecting.
    started: AtomicBool,
    /// Why it waits for a sign-in, and the sign-in it last connected
    /// under ([`TokenStore::fingerprint`]).
    auth: Mutex<(Option<AuthNeed>, Option<String>)>,
    /// Bumped whenever anything [`Self::watch`] reports changes.
    revision: watch::Sender<u64>,
}

impl Drop for Connection {
    /// Closes the session once nothing holds the connection: a pool let
    /// go of it, and the last run that used it ended.
    fn drop(&mut self) {
        let session = self
            .inner
            .get_mut()
            .ok()
            .and_then(|inner| inner.session.take());
        if let (Some(session), Ok(runtime)) =
            (session, tokio::runtime::Handle::try_current())
        {
            runtime.spawn(async move { session.close().await });
        }
    }
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
            started: AtomicBool::new(false),
            auth: Mutex::default(),
            revision: watch::channel(0).0,
        })
    }

    /// Where its sign-in is kept, when OAuth applies to it.
    pub fn oauth(&self) -> Option<(TokenStore, GrantKey)> {
        self.environment.oauth(&self.config)
    }

    /// Why it waits for a sign-in, while it does.
    pub fn auth_need(&self) -> Option<AuthNeed> {
        (self.status().state == State::NeedsAuth)
            .then(|| self.auth.lock().expect("auth lock").0.clone())
            .flatten()
    }

    /// What signing in to it needs: the grants file, the server, its
    /// `oauth` block (`clientSecret` expanded) and what it asked for.
    pub fn sign_in_request(
        &self,
    ) -> Result<(TokenStore, SignInRequest), String> {
        let name = &self.config.name;
        let Transport::Http(http) = &self.config.transport else {
            return Err(format!(
                "`{name}` is not an HTTP server: it has no sign-in"
            ));
        };
        let (store, _) = self.oauth().ok_or_else(|| {
            format!(
                "`{name}` sends an Authorization header of its own, or tau has \
                 no configuration directory to keep a sign-in in"
            )
        })?;
        let mut oauth = http.oauth.clone().unwrap_or_default();
        oauth.client_secret = self.environment.client_secret(http)?;
        let need = self.auth_need().unwrap_or_default();
        Ok((
            store,
            SignInRequest {
                url: http.url.clone(),
                oauth,
                challenge: need.challenge,
                scope: need.scope,
            },
        ))
    }

    /// Whether its grant changed (a sign-in or sign-out, here or in
    /// another process) since it last connected.
    fn grant_changed(&self) -> bool {
        let Some((store, key)) = self.oauth() else {
            return false;
        };
        let now = store.fingerprint(&key);
        self.auth.lock().expect("auth lock").1 != now
    }

    /// Drops the session, if any, and connects again: after a sign-in
    /// or a sign-out.
    pub fn restart(self: &Arc<Self>) {
        let session =
            self.inner.lock().expect("connection lock").session.take();
        if let Some(session) = session {
            tokio::spawn(async move { session.close().await });
        }
        if self.status().state == State::Connected {
            self.set_status(State::Disconnected, None);
        }
        self.connect();
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
        self.revision.send_modify(|revision| *revision += 1);
    }

    fn changed(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.revision.send_modify(|revision| *revision += 1);
    }

    /// A receiver that sees a change whenever the status, the tools, the
    /// instructions, the resources, the templates or the prompts change,
    /// for an interface that shows them. It ends when the connection is
    /// dropped.
    pub fn watch(&self) -> watch::Receiver<u64> {
        self.revision.subscribe()
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

    /// The protocol version the last session agreed on with the server,
    /// such as `2025-11-25`; none before the first connect.
    pub fn protocol(&self) -> Option<String> {
        self.inner.lock().expect("connection lock").protocol.clone()
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

    /// Whether the server offers resources.
    pub fn offers_resources(&self) -> bool {
        self.inner
            .lock()
            .expect("connection lock")
            .features
            .resources
    }

    /// Whether the server offers prompts.
    pub fn offers_prompts(&self) -> bool {
        self.inner.lock().expect("connection lock").features.prompts
    }

    /// The resources last listed, but MCP apps' (`ui://`).
    pub fn resources(&self) -> Vec<ResourceInfo> {
        self.inner
            .lock()
            .expect("connection lock")
            .resources
            .clone()
    }

    /// The resource templates last listed, but MCP apps'.
    pub fn templates(&self) -> Vec<TemplateInfo> {
        self.inner
            .lock()
            .expect("connection lock")
            .templates
            .clone()
    }

    /// The prompts last listed.
    pub fn prompts(&self) -> Vec<PromptInfo> {
        self.inner.lock().expect("connection lock").prompts.clone()
    }

    /// Whether it ever started connecting.
    pub fn started(&self) -> bool {
        self.started.load(Ordering::SeqCst)
    }

    /// Starts connecting in the background if it never did: a server
    /// that failed is not tried again here, only by a call,
    /// [`Self::connect`] or a reconnect.
    pub fn start(self: &Arc<Self>) {
        if !self.started() {
            self.connect();
        }
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
            self.started.store(true, Ordering::SeqCst);
            self.set_status(State::Connecting, None);
            let seen = self
                .oauth()
                .and_then(|(store, key)| store.fingerprint(&key));
            *self.auth.lock().expect("auth lock") = (None, seen);
        }
        let this = self.clone();
        tokio::spawn(async move { this.run_connect().await });
    }

    async fn run_connect(self: Arc<Self>) {
        let outcome = tokio::time::timeout(CONNECT_TIMEOUT, self.open())
            .await
            .unwrap_or_else(|_| {
                Err((
                    format!("no answer in {} s", CONNECT_TIMEOUT.as_secs()),
                    None,
                ))
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
            Ok((session, lists, events)) => {
                inner.instructions = session.instructions().map(str::to_owned);
                inner.protocol = session.protocol().map(str::to_owned);
                inner.features = session.features();
                inner.tools = lists.tools;
                inner.resources = lists.resources;
                inner.templates = lists.templates;
                inner.prompts = lists.prompts;
                inner.session = Some(session.clone());
                drop(inner);
                // The generation first: whoever sees `Connected` (a run
                // waiting in `ready`) must not read the tools cached for
                // the generation before.
                self.changed();
                self.set_status(State::Connected, None);
                tokio::spawn(watch_session(
                    Arc::downgrade(&self),
                    session,
                    events,
                ));
            }
            Err((error, None)) => {
                drop(inner);
                self.set_status(State::Failed, Some(error));
            }
            Err((_, Some(need))) => {
                drop(inner);
                let message = need.message();
                self.auth.lock().expect("auth lock").0 = Some(need);
                self.set_status(State::NeedsAuth, Some(message));
            }
        }
    }

    /// Connects, retrying an HTTP server's transient errors, and lists
    /// the tools, and the resources, templates and prompts it offers.
    /// A server that asks for a sign-in fails with why.
    async fn open(
        &self,
    ) -> Result<
        (Arc<Session>, Lists, mpsc::UnboundedReceiver<SessionEvent>),
        (String, Option<AuthNeed>),
    > {
        let endpoint = self
            .environment
            .endpoint(&self.config)
            .map_err(|error| (error, None))?;
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
                Err(error) => return Err((error.message, error.auth)),
            }
        };
        let tools = match session.list_tools().await {
            Ok(tools) => tools,
            Err(error) => {
                session.close().await;
                return Err((error, None));
            }
        };
        // A list that fails leaves it empty: the tools still work.
        let (resources, templates) =
            list_resources(&self.config.name, &session).await;
        let prompts = list_prompts(&self.config.name, &session).await;
        Ok((
            session,
            Lists {
                tools,
                resources,
                templates,
                prompts,
            },
            events,
        ))
    }

    /// A live session, connecting first when there is none. Waits for a
    /// connect under way. Fails when the connect fails, the connection is
    /// closed, or `cancel` fires.
    pub(crate) async fn session(
        self: &Arc<Self>,
        cancel: &CancellationToken,
    ) -> Result<Arc<Session>, String> {
        loop {
            if self.grant_changed() {
                // Signed in or out since: the session's authorization is
                // not the grant's any more.
                let session =
                    self.inner.lock().expect("connection lock").session.take();
                if let Some(session) = session {
                    tokio::spawn(async move { session.close().await });
                }
            } else if self.status().state == State::NeedsAuth {
                return Err(self
                    .status()
                    .error
                    .unwrap_or_else(|| AuthNeed::default().message()));
            }
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

    /// Reads the resource `uri` and returns its `ReadResourceResult` as
    /// JSON. Read-only, so a connection that drops under it is connected
    /// again and the read sent once more.
    pub async fn read_resource(
        self: &Arc<Self>,
        uri: &str,
        cancel: &CancellationToken,
    ) -> Result<Value, String> {
        self.read_only(ReadOnly::Resource(uri), cancel).await
    }

    /// Gets the prompt `name` with `arguments` and returns its
    /// `GetPromptResult` as JSON. Read-only: tried twice, as
    /// [`Self::read_resource`].
    pub async fn get_prompt(
        self: &Arc<Self>,
        name: &str,
        arguments: serde_json::Map<String, Value>,
        cancel: &CancellationToken,
    ) -> Result<Value, String> {
        self.read_only(ReadOnly::Prompt(name, arguments), cancel)
            .await
    }

    async fn read_only(
        self: &Arc<Self>,
        request: ReadOnly<'_>,
        cancel: &CancellationToken,
    ) -> Result<Value, String> {
        let server = self.config.name.clone();
        let timeout = self.config.timeout();
        let mut tries = 0;
        loop {
            tries += 1;
            let session = self.session(cancel).await.map_err(|error| {
                if cancel.is_cancelled() {
                    format!("{} was cancelled", request.what())
                } else {
                    format!("MCP server {server} is not connected: {error}")
                }
            })?;
            let features = session.features();
            let result = match &request {
                ReadOnly::Resource(uri) => {
                    if !features.resources {
                        return Err(format!(
                            "MCP server {server} does not offer resources"
                        ));
                    }
                    session.read_resource(uri, timeout, cancel).await
                }
                ReadOnly::Prompt(name, arguments) => {
                    if !features.prompts {
                        return Err(format!(
                            "MCP server {server} does not offer prompts"
                        ));
                    }
                    session
                        .get_prompt(name, arguments.clone(), timeout, cancel)
                        .await
                }
            };
            match result {
                Ok(value) => return Ok(value),
                Err(CallError::Disconnected) => {
                    self.lost(&session);
                    if tries >= 2 {
                        return Err(format!(
                            "MCP server {server} closed the connection during {}",
                            request.what()
                        ));
                    }
                }
                Err(CallError::Timeout(after)) => {
                    return Err(format!(
                        "MCP server {server} gave no answer to {} in {} s",
                        request.what(),
                        after.as_secs_f64()
                    ));
                }
                Err(CallError::Cancelled) => {
                    return Err(format!("{} was cancelled", request.what()));
                }
                Err(CallError::Protocol(message)) => {
                    return Err(format!(
                        "MCP server {server} failed {}: {message}",
                        request.what()
                    ));
                }
            }
        }
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

impl Connection {
    /// Lists the resources and templates again, after the server said
    /// they changed.
    async fn relist_resources(&self, session: &Arc<Session>) {
        let (resources, templates) =
            list_resources(&self.config.name, session).await;
        self.update(session, |inner| {
            inner.resources = resources;
            inner.templates = templates;
        });
    }

    /// Lists the prompts again, after the server said they changed.
    async fn relist_prompts(&self, session: &Arc<Session>) {
        let prompts = list_prompts(&self.config.name, session).await;
        self.update(session, |inner| inner.prompts = prompts);
    }

    /// Applies `change` if `session` is still the connection's.
    fn update(&self, session: &Arc<Session>, change: impl FnOnce(&mut Inner)) {
        let mut inner = self.inner.lock().expect("connection lock");
        if inner
            .session
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, session))
        {
            change(&mut inner);
            drop(inner);
            self.changed();
        }
    }
}

/// The server's resources and templates, but MCP apps', when it offers
/// resources; empty when it does not, or when listing fails.
async fn list_resources(
    server: &str,
    session: &Session,
) -> (Vec<ResourceInfo>, Vec<TemplateInfo>) {
    if !session.features().resources {
        return (Vec::new(), Vec::new());
    }
    let resources = session.list_resources().await.unwrap_or_else(|error| {
        tracing::warn!(target: "tau_mcp::server", server, "{error}");
        Vec::new()
    });
    // A server may offer resources without templates.
    let templates = session.list_templates().await.unwrap_or_else(|error| {
        tracing::debug!(target: "tau_mcp::server", server, "{error}");
        Vec::new()
    });
    (
        resources
            .into_iter()
            .filter(|r| {
                !crate::resources::is_app(&r.uri, r.mime_type.as_deref())
            })
            .collect(),
        templates
            .into_iter()
            .filter(|t| {
                !crate::resources::is_app(
                    &t.uri_template,
                    t.mime_type.as_deref(),
                )
            })
            .collect(),
    )
}

/// The server's prompts when it offers them; empty when it does not, or
/// when listing fails.
async fn list_prompts(server: &str, session: &Session) -> Vec<PromptInfo> {
    if !session.features().prompts {
        return Vec::new();
    }
    session.list_prompts().await.unwrap_or_else(|error| {
        tracing::warn!(target: "tau_mcp::server", server, "{error}");
        Vec::new()
    })
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
            SessionEvent::ResourcesChanged => {
                connection.relist_resources(&session).await
            }
            SessionEvent::PromptsChanged => {
                connection.relist_prompts(&session).await
            }
            SessionEvent::Closed => {
                connection.lost(&session);
                return;
            }
        }
    }
}
