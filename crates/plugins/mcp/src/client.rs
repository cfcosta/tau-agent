//! The MCP client: the one module that touches `rmcp`, whose API moves
//! between majors (`docs/decisions/0018-codemode-and-mcp.md`).
//!
//! A [`Session`] is one live connection to one server: it lists tools,
//! calls them with a timeout that progress restarts, routes progress to
//! the call that asked for it, and reports list changes and its own end
//! as [`SessionEvent`]s. Everything it hands out is plain JSON or this
//! crate's types.
//!
//! Roots and server logging are deprecated by SEP-2577 in the newest
//! protocol, but servers on 2025-11-25 still use them, so this module
//! allows deprecated items.
#![allow(deprecated)]

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};

use rmcp::{
    ClientHandler,
    ClientServiceExt,
    Peer,
    RoleClient,
    ServiceError,
    model::{
        CallToolRequest,
        CallToolRequestParams,
        CancelledNotificationParam,
        ClientCapabilities,
        ClientConfig,
        ClientRequest,
        GetPromptRequest,
        GetPromptRequestParams,
        Implementation,
        JsonObject,
        ListRootsResult,
        LoggingLevel,
        LoggingMessageNotificationParam,
        ProgressNotificationParam,
        ProgressToken,
        ProtocolVersion,
        ReadResourceRequest,
        ReadResourceRequestParams,
        Root,
        ServerNotification,
        ServerResult,
        SubscriptionFilter,
    },
    service::{
        ClientInitializeError,
        ClientLifecycleMode,
        NotificationContext,
        PeerRequestOptions,
        RequestContext,
        RunningService,
        RunningServiceCancellationToken,
    },
    transport::{
        StreamableHttpClientTransport,
        TokioChildProcess,
        auth::{AuthClient, AuthError},
        streamable_http_client::{
            AuthRequiredError,
            InsufficientScopeError,
            StreamableHttpClientTransportConfig,
            StreamableHttpError,
        },
    },
};
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    sync::{mpsc, watch},
};
use tokio_util::sync::CancellationToken;

use crate::{
    auth::{GrantKey, TokenStore, http::mcp_client},
    config::Dial,
    connection::{
        Annotations,
        AuthNeed,
        Progress,
        PromptArgument,
        PromptInfo,
        ResourceInfo,
        TemplateInfo,
        ToolInfo,
    },
};

/// How long the process group of a closed stdio server has between
/// SIGTERM and SIGKILL.
const KILL_GRACE: Duration = Duration::from_secs(2);

/// Where a server is, its configuration resolved: variables and `~/`
/// expanded, `cwd` made absolute.
#[derive(Clone)]
pub(crate) enum Endpoint {
    Stdio {
        program: String,
        args: Vec<String>,
        env: Vec<(String, String)>,
        cwd: Option<PathBuf>,
    },
    Http {
        url: String,
        headers: Vec<(String, String)>,
        /// Where its sign-in is kept, when OAuth applies: the entry sends
        /// no `Authorization` header of its own.
        auth: Option<HttpAuth>,
    },
    Stream(Dial),
}

/// An HTTP server's sign-in: its grant, and the configured client's
/// secret, expanded.
#[derive(Clone)]
pub(crate) struct HttpAuth {
    pub store: TokenStore,
    pub key: GrantKey,
    pub client_secret: Option<String>,
}

impl Endpoint {
    pub fn is_http(&self) -> bool {
        matches!(self, Self::Http { .. })
    }

    /// Whether OAuth applies: a 401 asks for a sign-in.
    pub fn signs_in(&self) -> bool {
        matches!(self, Self::Http { auth: Some(_), .. })
    }
}

/// What a session tells its connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionEvent {
    /// The server's tools changed: list them again.
    ToolsChanged,
    /// Its resources or resource templates changed.
    ResourcesChanged,
    /// Its prompts changed.
    PromptsChanged,
    /// The connection ended.
    Closed,
}

/// A connect that failed.
#[derive(Debug, Clone)]
pub(crate) struct ConnectError {
    pub message: String,
    /// A network error, a timeout, or an HTTP 408, 429 or 5xx but 501:
    /// worth another try.
    pub transient: bool,
    /// The server asked for a sign-in, or for more scopes.
    pub auth: Option<AuthNeed>,
}

impl ConnectError {
    fn fatal(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            transient: false,
            auth: None,
        }
    }
}

/// A call that failed before the server gave a result.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CallError {
    /// The connection ended before the result came.
    Disconnected,
    /// The time ran out with no result and no progress.
    Timeout(Duration),
    /// The run's token was cancelled; the server was told.
    Cancelled,
    /// The server answered with an error, or with something tau cannot
    /// use.
    Protocol(String),
}

type Routes =
    Arc<Mutex<HashMap<ProgressToken, mpsc::UnboundedSender<Progress>>>>;

/// The client side of a session: answers the server's requests and
/// passes its notifications on.
#[derive(Clone)]
struct Handler {
    server: Arc<str>,
    roots: Vec<Root>,
    routes: Routes,
    events: mpsc::UnboundedSender<SessionEvent>,
    /// The version `initialize` asks for.
    protocol: ProtocolVersion,
}

impl ClientHandler for Handler {
    async fn on_progress(
        &self,
        params: ProgressNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        let route = self
            .routes
            .lock()
            .expect("routes lock")
            .get(&params.progress_token)
            .cloned();
        if let Some(route) = route {
            let _ = route.send(Progress {
                progress: params.progress,
                total: params.total,
                message: params.message,
            });
        }
    }

    async fn on_tool_list_changed(
        &self,
        _context: NotificationContext<RoleClient>,
    ) {
        let _ = self.events.send(SessionEvent::ToolsChanged);
    }

    async fn on_resource_list_changed(
        &self,
        _context: NotificationContext<RoleClient>,
    ) {
        let _ = self.events.send(SessionEvent::ResourcesChanged);
    }

    async fn on_prompt_list_changed(
        &self,
        _context: NotificationContext<RoleClient>,
    ) {
        let _ = self.events.send(SessionEvent::PromptsChanged);
    }

    async fn on_logging_message(
        &self,
        params: LoggingMessageNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        log_message(&self.server, &params);
    }

    async fn list_roots(
        &self,
        _context: RequestContext<RoleClient>,
    ) -> Result<ListRootsResult, rmcp::ErrorData> {
        Ok(ListRootsResult::new(self.roots.clone()))
    }

    fn get_info(&self) -> ClientConfig {
        // Roots is a deprecated field of the capabilities, so it is set
        // through serde rather than by name.
        let capabilities: ClientCapabilities =
            serde_json::from_value(serde_json::json!({ "roots": {} }))
                .unwrap_or_default();
        let mut config = ClientConfig::new(
            capabilities,
            Implementation::new("tau", env!("CARGO_PKG_VERSION")),
        );
        config.protocol_version = self.protocol.clone();
        config
    }
}

/// A server's `notifications/message`, in tau's log.
fn log_message(server: &str, params: &LoggingMessageNotificationParam) {
    let logger = params.logger.as_deref().unwrap_or_default();
    let data = match &params.data {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    match params.level {
        LoggingLevel::Debug => {
            tracing::debug!(target: "tau_mcp::server", server, logger, "{data}");
        }
        LoggingLevel::Info | LoggingLevel::Notice => {
            tracing::info!(target: "tau_mcp::server", server, logger, "{data}");
        }
        LoggingLevel::Warning => {
            tracing::warn!(target: "tau_mcp::server", server, logger, "{data}");
        }
        _ => {
            tracing::error!(target: "tau_mcp::server", server, logger, "{data}")
        }
    }
}

/// One live connection to a server.
pub(crate) struct Session {
    peer: Peer<RoleClient>,
    stop: Mutex<Option<RunningServiceCancellationToken>>,
    done: watch::Receiver<bool>,
    routes: Routes,
    instructions: Option<String>,
    features: Features,
    protocol: Option<String>,
}

/// What a server offers besides tools, from its capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Features {
    pub resources: bool,
    pub prompts: bool,
}

impl Session {
    /// Connects with rmcp's `Auto` lifecycle, 2026-07-28 preferred and
    /// 2025-11-25 as the fallback, and starts watching for tool list
    /// changes.
    pub async fn connect(
        server: &str,
        endpoint: &Endpoint,
        root: Option<&Path>,
        events: mpsc::UnboundedSender<SessionEvent>,
    ) -> Result<Self, ConnectError> {
        let routes = Routes::default();
        let list_events = events.clone();
        let mut handler = Handler {
            server: server.into(),
            roots: root
                .map(|root| {
                    vec![Root::new(format!("file://{}", root.display()))]
                })
                .unwrap_or_default(),
            routes: routes.clone(),
            events: events.clone(),
            protocol: ProtocolVersion::V_2026_07_28,
        };
        let auto = ClientLifecycleMode::Auto {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            legacy_version: Some(ProtocolVersion::V_2025_11_25),
        };
        let mut opened = open(server, endpoint, handler.clone(), auto).await;
        // `Auto` falls back to `initialize` only for a server that does
        // not know `server/discover`. One that knows it but offers only
        // 2025-11-25 gets a new connection that initializes.
        if let Err(OpenError::Lifecycle(error)) = &opened
            && let ClientInitializeError::NoCompatibleProtocolVersion {
                server_supported,
                ..
            } = &**error
            && server_supported.contains(&ProtocolVersion::V_2025_11_25)
        {
            handler.protocol = ProtocolVersion::V_2025_11_25;
            opened = open(
                server,
                endpoint,
                handler,
                ClientLifecycleMode::Initialize,
            )
            .await;
        }
        let (service, group) = match opened {
            Ok(opened) => opened,
            Err(OpenError::Transport(message)) => {
                return Err(ConnectError::fatal(message));
            }
            Err(OpenError::Lifecycle(error)) => {
                let auth =
                    endpoint.signs_in().then(|| auth_need(&error)).flatten();
                return Err(ConnectError {
                    transient: auth.is_none() && transient(&error),
                    message: describe(&error),
                    auth,
                });
            }
        };
        let peer = service.peer().clone();
        let info = peer.peer_info();
        let instructions = info
            .as_ref()
            .and_then(|info| info.instructions.clone())
            .filter(|text| !text.trim().is_empty());
        let protocol =
            info.as_ref().map(|info| info.protocol_version.to_string());
        let capabilities = info.as_ref().map(|info| &info.capabilities);
        let features = Features {
            resources: capabilities.is_some_and(|c| c.resources.is_some()),
            prompts: capabilities.is_some_and(|c| c.prompts.is_some()),
        };
        // On 2026-07-28 list changes come only through
        // `subscriptions/listen`, for the lists the server says change.
        let listens = info.as_ref().is_some_and(|info| {
            info.protocol_version >= ProtocolVersion::V_2026_07_28
        });
        let changes = Changes {
            tools: listens
                && capabilities
                    .and_then(|c| c.tools.as_ref())
                    .and_then(|tools| tools.list_changed)
                    .unwrap_or(false),
            resources: listens
                && capabilities
                    .and_then(|c| c.resources.as_ref())
                    .and_then(|resources| resources.list_changed)
                    .unwrap_or(false),
            prompts: listens
                && capabilities
                    .and_then(|c| c.prompts.as_ref())
                    .and_then(|prompts| prompts.list_changed)
                    .unwrap_or(false),
        };
        let stop = service.cancellation_token();
        let (done_tx, done) = watch::channel(false);
        tokio::spawn(async move {
            let _ = service.waiting().await;
            if let Some(group) = group {
                end_group(group).await;
            }
            let _ = done_tx.send(true);
            let _ = events.send(SessionEvent::Closed);
        });
        if changes.any() {
            tokio::spawn(watch_lists(peer.clone(), changes, list_events));
        }
        Ok(Self {
            peer,
            stop: Mutex::new(Some(stop)),
            done,
            routes,
            instructions,
            features,
            protocol,
        })
    }

    /// The protocol version the session runs on, as the server gave it.
    pub fn protocol(&self) -> Option<&str> {
        self.protocol.as_deref()
    }

    /// The server's instructions, if it gave any.
    pub fn instructions(&self) -> Option<&str> {
        self.instructions.as_deref()
    }

    /// What the server offers besides tools.
    pub fn features(&self) -> Features {
        self.features
    }

    /// Whether the connection has ended.
    pub fn is_closed(&self) -> bool {
        *self.done.borrow() || self.peer.is_transport_closed()
    }

    /// Every tool, every page of `tools/list`.
    pub async fn list_tools(&self) -> Result<Vec<ToolInfo>, String> {
        let tools = self
            .peer
            .list_all_tools()
            .await
            .map_err(|error| format!("cannot list the tools: {error}"))?;
        Ok(tools
            .into_iter()
            .map(|tool| {
                let annotations = tool.annotations.as_ref();
                ToolInfo {
                    name: tool.name.to_string(),
                    title: tool
                        .title
                        .clone()
                        .or_else(|| annotations.and_then(|a| a.title.clone())),
                    description: tool
                        .description
                        .as_ref()
                        .map(ToString::to_string),
                    input_schema: Value::Object((*tool.input_schema).clone()),
                    output_schema: tool
                        .output_schema
                        .as_ref()
                        .map(|schema| Value::Object((**schema).clone())),
                    annotations: Annotations {
                        read_only: annotations.and_then(|a| a.read_only_hint),
                        destructive: annotations
                            .and_then(|a| a.destructive_hint),
                        idempotent: annotations.and_then(|a| a.idempotent_hint),
                        open_world: annotations.and_then(|a| a.open_world_hint),
                    },
                }
            })
            .collect())
    }

    /// Every resource, every page of `resources/list`.
    pub async fn list_resources(&self) -> Result<Vec<ResourceInfo>, String> {
        let resources =
            self.peer.list_all_resources().await.map_err(|error| {
                format!("cannot list the resources: {error}")
            })?;
        Ok(resources
            .iter()
            .filter_map(|resource| serde_json::to_value(resource).ok())
            .map(|value| ResourceInfo {
                uri: text(&value, "uri").unwrap_or_default(),
                name: text(&value, "name").unwrap_or_default(),
                title: text(&value, "title"),
                description: text(&value, "description"),
                mime_type: text(&value, "mimeType"),
                size: value.get("size").and_then(Value::as_u64),
            })
            .collect())
    }

    /// Every resource template, every page of
    /// `resources/templates/list`.
    pub async fn list_templates(&self) -> Result<Vec<TemplateInfo>, String> {
        let templates =
            self.peer
                .list_all_resource_templates()
                .await
                .map_err(|error| {
                    format!("cannot list the resource templates: {error}")
                })?;
        Ok(templates
            .iter()
            .filter_map(|template| serde_json::to_value(template).ok())
            .map(|value| TemplateInfo {
                uri_template: text(&value, "uriTemplate").unwrap_or_default(),
                name: text(&value, "name").unwrap_or_default(),
                title: text(&value, "title"),
                description: text(&value, "description"),
                mime_type: text(&value, "mimeType"),
            })
            .collect())
    }

    /// Every prompt, every page of `prompts/list`.
    pub async fn list_prompts(&self) -> Result<Vec<PromptInfo>, String> {
        let prompts = self
            .peer
            .list_all_prompts()
            .await
            .map_err(|error| format!("cannot list the prompts: {error}"))?;
        Ok(prompts
            .iter()
            .filter_map(|prompt| serde_json::to_value(prompt).ok())
            .map(|value| PromptInfo {
                name: text(&value, "name").unwrap_or_default(),
                title: text(&value, "title"),
                description: text(&value, "description"),
                arguments: value
                    .get("arguments")
                    .and_then(Value::as_array)
                    .map(|arguments| {
                        arguments
                            .iter()
                            .map(|argument| PromptArgument {
                                name: text(argument, "name")
                                    .unwrap_or_default(),
                                description: text(argument, "description"),
                                required: argument
                                    .get("required")
                                    .and_then(Value::as_bool)
                                    .unwrap_or(false),
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            })
            .collect())
    }

    /// `resources/read` of `uri`: its `ReadResourceResult` as JSON.
    pub async fn read_resource(
        &self,
        uri: &str,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Result<Value, CallError> {
        let request = ClientRequest::ReadResourceRequest(
            ReadResourceRequest::new(ReadResourceRequestParams::new(uri)),
        );
        match self.request(request, timeout, cancel).await? {
            ServerResult::ReadResourceResult(result) => {
                serde_json::to_value(&result)
                    .map_err(|error| CallError::Protocol(error.to_string()))
            }
            other => Err(unexpected(&other, "the resource")),
        }
    }

    /// `prompts/get` of `name` with `arguments`: its `GetPromptResult` as
    /// JSON.
    pub async fn get_prompt(
        &self,
        name: &str,
        arguments: JsonObject,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Result<Value, CallError> {
        let mut params = GetPromptRequestParams::new(name);
        params.arguments = Some(arguments);
        let request =
            ClientRequest::GetPromptRequest(GetPromptRequest::new(params));
        match self.request(request, timeout, cancel).await? {
            ServerResult::GetPromptResult(result) => {
                serde_json::to_value(&result)
                    .map_err(|error| CallError::Protocol(error.to_string()))
            }
            other => Err(unexpected(&other, "the prompt")),
        }
    }

    /// Sends `request` and waits up to `timeout` for its answer, or until
    /// `cancel`, which tells the server.
    async fn request(
        &self,
        request: ClientRequest,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Result<ServerResult, CallError> {
        let options = PeerRequestOptions::with_timeout(timeout);
        let handle = self
            .peer
            .send_cancellable_request(request, options)
            .await
            .map_err(call_error)?;
        let id = handle.id.clone();
        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                let notice = CancelledNotificationParam::new(
                    Some(id),
                    Some("cancelled by tau".to_owned()),
                );
                let _ = self.peer.notify_cancelled(notice).await;
                Err(CallError::Cancelled)
            }
            response = handle.await_response() => response.map_err(call_error),
        }
    }

    /// Calls `tool` and returns its `CallToolResult` as JSON. `timeout`
    /// starts again at each progress notification, which also goes to
    /// `on_progress`. Cancelling `cancel` sends `notifications/cancelled`
    /// and fails the call; the server may still finish it. The call is
    /// never sent twice.
    pub async fn call(
        &self,
        tool: &str,
        arguments: Option<JsonObject>,
        timeout: Duration,
        on_progress: &(dyn Fn(Progress) + Send + Sync),
        cancel: &CancellationToken,
    ) -> Result<Value, CallError> {
        let mut params = CallToolRequestParams::new(tool.to_owned());
        params.arguments = arguments;
        let request =
            ClientRequest::CallToolRequest(CallToolRequest::new(params));
        let options = PeerRequestOptions::with_timeout(timeout)
            .reset_timeout_on_progress();
        let handle = self
            .peer
            .send_cancellable_request(request, options)
            .await
            .map_err(call_error)?;
        let id = handle.id.clone();
        let (sender, mut progress) = mpsc::unbounded_channel();
        let route = RouteGuard {
            routes: self.routes.clone(),
            token: handle.progress_token.clone(),
        };
        route
            .routes
            .lock()
            .expect("routes lock")
            .insert(route.token.clone(), sender);
        let response = handle.await_response();
        tokio::pin!(response);
        let response = loop {
            tokio::select! {
                biased;
                () = cancel.cancelled() => {
                    let notice = CancelledNotificationParam::new(
                        Some(id),
                        Some("cancelled by tau".to_owned()),
                    );
                    let _ = self.peer.notify_cancelled(notice).await;
                    return Err(CallError::Cancelled);
                }
                response = &mut response => break response,
                Some(update) = progress.recv() => on_progress(update),
            }
        };
        // Progress that came in with the result still counts.
        while let Ok(update) = progress.try_recv() {
            on_progress(update);
        }
        drop(route);
        match response.map_err(call_error)? {
            ServerResult::CallToolResult(result) => {
                serde_json::to_value(&result)
                    .map_err(|error| CallError::Protocol(error.to_string()))
            }
            ServerResult::InputRequiredResult(_) => Err(CallError::Protocol(
                "the server asked for input, which tau cannot give".into(),
            )),
            ServerResult::CreateTaskResult(_) => Err(CallError::Protocol(
                "the server started a task, which tau does not support".into(),
            )),
            _ => Err(CallError::Protocol(
                "the server's answer is not a tool result".into(),
            )),
        }
    }

    /// Ends the session: rmcp's cancel, which closes the transport (and
    /// for a stdio server closes its stdin, waits, then kills it), then
    /// SIGTERM to the process group and SIGKILL 2 s later.
    pub async fn close(&self) {
        let stop = self.stop.lock().expect("stop lock").take();
        if let Some(stop) = stop {
            stop.cancel();
        }
        let mut done = self.done.clone();
        let _ = done.wait_for(|done| *done).await;
    }
}

/// A running service and, for a stdio server, its process group.
type Opened = (RunningService<RoleClient, Handler>, Option<i32>);

/// Why [`open`] failed.
enum OpenError {
    /// The transport could not be set up.
    Transport(String),
    /// rmcp's lifecycle failed.
    Lifecycle(Box<ClientInitializeError>),
}

/// Opens the transport and runs `lifecycle` over it.
async fn open(
    server: &str,
    endpoint: &Endpoint,
    handler: Handler,
    lifecycle: ClientLifecycleMode,
) -> Result<Opened, OpenError> {
    let ours = OpenError::Transport;
    let mut group = None;
    let service = match endpoint {
        Endpoint::Stdio {
            program,
            args,
            env,
            cwd,
        } => {
            let mut command = tokio::process::Command::new(program);
            command.args(args).envs(env.iter().map(|(k, v)| (k, v)));
            if let Some(cwd) = cwd {
                command.current_dir(cwd);
            }
            // Its own process group, so closing it reaches whatever it
            // started.
            command.process_group(0);
            let (child, stderr) = TokioChildProcess::builder(command)
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|error| {
                    ours(format!("cannot start `{program}`: {error}"))
                })?;
            group = child.id().map(|pid| pid as i32);
            if let Some(stderr) = stderr {
                tokio::spawn(log_stderr(server.to_owned(), stderr));
            }
            handler.serve_with_lifecycle(child, lifecycle).await
        }
        Endpoint::Http { url, headers, auth } => {
            let mut config =
                StreamableHttpClientTransportConfig::with_uri(url.as_str());
            for (name, value) in headers {
                let name =
                    reqwest::header::HeaderName::from_bytes(name.as_bytes())
                        .map_err(|_| {
                            ours(format!("`{name}` is not a valid header name"))
                        })?;
                let value = reqwest::header::HeaderValue::from_str(value)
                    .map_err(|_| {
                        ours(format!(
                            "the value of header `{name}` is not valid"
                        ))
                    })?;
                config.custom_headers.insert(name, value);
            }
            let manager = match auth {
                Some(auth) => crate::auth::authorization(
                    url,
                    &auth.store,
                    &auth.key,
                    auth.client_secret.as_deref(),
                )
                .await
                .map_err(ours)?,
                None => None,
            };
            match manager {
                // Signed in: the access token goes with every request,
                // refreshed when it is about to expire or turned down.
                Some(manager) => {
                    let transport = StreamableHttpClientTransport::with_client(
                        AuthClient::new(HttpClient(mcp_client()), manager),
                        config,
                    );
                    handler.serve_with_lifecycle(transport, lifecycle).await
                }
                None => {
                    let transport = StreamableHttpClientTransport::with_client(
                        HttpClient(mcp_client()),
                        config,
                    );
                    handler.serve_with_lifecycle(transport, lifecycle).await
                }
            }
        }
        Endpoint::Stream(dial) => {
            let stream = dial.open().await.map_err(|error| {
                ours(format!("cannot open the stream: {error}"))
            })?;
            let (read, write) = tokio::io::split(stream);
            handler.serve_with_lifecycle((read, write), lifecycle).await
        }
    };
    match service {
        Ok(service) => Ok((service, group)),
        Err(error) => {
            if let Some(group) = group {
                tokio::spawn(end_group(group));
            }
            Err(OpenError::Lifecycle(Box::new(error)))
        }
    }
}

/// tau's HTTP client for rmcp: reqwest's, but an error that names no
/// request, answering a request posted in a session, counts as the
/// session having expired.
///
/// A server that restarted no longer knows its sessions. The spec has it
/// answer 404, on which rmcp starts a new session and posts the request
/// again. The reference servers (the TypeScript SDK's examples, such as
/// `@modelcontextprotocol/server-everything`) answer 400 with a JSON-RPC
/// error without an `id` instead, which rmcp hands on uncorrelated: the
/// request then waits out its timeout, and so does every request after
/// it, since nothing marks the session gone. The server rejected the
/// request before running it, so starting a new session and posting it
/// again cannot run a tool twice.
#[derive(Clone)]
struct HttpClient(reqwest::Client);

impl rmcp::transport::streamable_http_client::StreamableHttpClient
    for HttpClient
{
    type Error = reqwest::Error;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: rmcp::model::ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<
            reqwest::header::HeaderName,
            reqwest::header::HeaderValue,
        >,
    ) -> Result<
        rmcp::transport::streamable_http_client::StreamableHttpPostResponse,
        StreamableHttpError<reqwest::Error>,
    > {
        let in_session = session_id.is_some();
        let is_request =
            matches!(message, rmcp::model::ClientJsonRpcMessage::Request(_));
        let response = self
            .0
            .post_message(uri, message, session_id, auth_header, custom_headers)
            .await;
        expired_session(response, in_session && is_request)
    }

    async fn post_message_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        message: rmcp::model::ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<
            reqwest::header::HeaderName,
            reqwest::header::HeaderValue,
        >,
        max_sse_event_size: usize,
    ) -> Result<
        rmcp::transport::streamable_http_client::StreamableHttpPostResponse,
        StreamableHttpError<reqwest::Error>,
    > {
        let in_session = session_id.is_some();
        let is_request =
            matches!(message, rmcp::model::ClientJsonRpcMessage::Request(_));
        let response = self
            .0
            .post_message_with_max_sse_event_size(
                uri,
                message,
                session_id,
                auth_header,
                custom_headers,
                max_sse_event_size,
            )
            .await;
        expired_session(response, in_session && is_request)
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_header: Option<String>,
        custom_headers: HashMap<
            reqwest::header::HeaderName,
            reqwest::header::HeaderValue,
        >,
    ) -> Result<(), StreamableHttpError<reqwest::Error>> {
        self.0
            .delete_session(uri, session_id, auth_header, custom_headers)
            .await
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<
            reqwest::header::HeaderName,
            reqwest::header::HeaderValue,
        >,
    ) -> Result<
        futures_util::stream::BoxStream<
            'static,
            Result<
                sse_stream::Sse,
                rmcp::transport::streamable_http_client::SseError,
            >,
        >,
        StreamableHttpError<reqwest::Error>,
    > {
        self.0
            .get_stream(
                uri,
                session_id,
                last_event_id,
                auth_header,
                custom_headers,
            )
            .await
    }

    async fn get_stream_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<
            reqwest::header::HeaderName,
            reqwest::header::HeaderValue,
        >,
        max_sse_event_size: usize,
    ) -> Result<
        futures_util::stream::BoxStream<
            'static,
            Result<
                sse_stream::Sse,
                rmcp::transport::streamable_http_client::SseError,
            >,
        >,
        StreamableHttpError<reqwest::Error>,
    > {
        self.0
            .get_stream_with_max_sse_event_size(
                uri,
                session_id,
                last_event_id,
                auth_header,
                custom_headers,
                max_sse_event_size,
            )
            .await
    }
}

/// A request posted in a session answered by an error that names no
/// request becomes [`StreamableHttpError::SessionExpired`].
fn expired_session(
    response: Result<
        rmcp::transport::streamable_http_client::StreamableHttpPostResponse,
        StreamableHttpError<reqwest::Error>,
    >,
    request_in_session: bool,
) -> Result<
    rmcp::transport::streamable_http_client::StreamableHttpPostResponse,
    StreamableHttpError<reqwest::Error>,
> {
    use rmcp::{
        model::ServerJsonRpcMessage,
        transport::streamable_http_client::StreamableHttpPostResponse,
    };
    match response {
        Ok(StreamableHttpPostResponse::Json(
            ServerJsonRpcMessage::Error(error),
            _,
        )) if request_in_session && error.id.is_none() => {
            tracing::debug!(
                target: "tau_mcp::server",
                "an uncorrelated error answered a request in a session, \
                 taken as an expired session: {}",
                error.error.message
            );
            Err(StreamableHttpError::SessionExpired)
        }
        other => other,
    }
}

/// Removes a call's progress route when the call ends, however it ends.
struct RouteGuard {
    routes: Routes,
    token: ProgressToken,
}

impl Drop for RouteGuard {
    fn drop(&mut self) {
        if let Ok(mut routes) = self.routes.lock() {
            routes.remove(&self.token);
        }
    }
}

fn call_error(error: ServiceError) -> CallError {
    match error {
        ServiceError::Timeout { timeout } => CallError::Timeout(timeout),
        ServiceError::TransportClosed | ServiceError::TransportSend(_) => {
            CallError::Disconnected
        }
        ServiceError::McpError(error) => {
            CallError::Protocol(error.message.to_string())
        }
        other => CallError::Protocol(other.to_string()),
    }
}

/// The lists whose changes a session listens for.
#[derive(Debug, Clone, Copy)]
struct Changes {
    tools: bool,
    resources: bool,
    prompts: bool,
}

impl Changes {
    fn any(self) -> bool {
        self.tools || self.resources || self.prompts
    }
}

/// On the 2026-07-28 protocol, list changes come through
/// `subscriptions/listen`; on older ones, as plain notifications to the
/// handler.
async fn watch_lists(
    peer: Peer<RoleClient>,
    changes: Changes,
    events: mpsc::UnboundedSender<SessionEvent>,
) {
    let mut filter = SubscriptionFilter::builder();
    if changes.tools {
        filter = filter.tools_list_changed();
    }
    if changes.resources {
        filter = filter.resources_list_changed();
    }
    if changes.prompts {
        filter = filter.prompts_list_changed();
    }
    let Ok(mut subscription) = peer.listen(filter.build()).await else {
        return;
    };
    while let Ok(Some(notification)) = subscription.next().await {
        let event = match notification {
            ServerNotification::ToolListChangedNotification(_) => {
                SessionEvent::ToolsChanged
            }
            ServerNotification::ResourceListChangedNotification(_) => {
                SessionEvent::ResourcesChanged
            }
            ServerNotification::PromptListChangedNotification(_) => {
                SessionEvent::PromptsChanged
            }
            _ => continue,
        };
        if events.send(event).is_err() {
            return;
        }
    }
}

/// A stdio server's stderr, line by line, in tau's log.
async fn log_stderr(server: String, stderr: tokio::process::ChildStderr) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        tracing::debug!(target: "tau_mcp::server", server, "stderr: {line}");
    }
}

/// SIGTERM to the process group `group`, then SIGKILL after
/// [`KILL_GRACE`] if any of it is still there.
async fn end_group(group: i32) {
    let alive = || {
        // SAFETY: kill with signal 0 only checks the group exists.
        unsafe { libc::kill(-group, 0) == 0 }
    };
    if !alive() {
        return;
    }
    // SAFETY: signals the group the server leads; nothing else is in it.
    unsafe { libc::kill(-group, libc::SIGTERM) };
    let deadline = tokio::time::Instant::now() + KILL_GRACE;
    while tokio::time::Instant::now() < deadline {
        if !alive() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // SAFETY: as above.
    unsafe { libc::kill(-group, libc::SIGKILL) };
}

/// The error for an answer that is not the one asked for.
fn unexpected(result: &ServerResult, what: &str) -> CallError {
    CallError::Protocol(match result {
        ServerResult::InputRequiredResult(_) => {
            "the server asked for input, which tau cannot give".to_owned()
        }
        _ => format!("the server's answer is not {what}"),
    })
}

/// The string field `key` of `value`.
fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// What a failed connect says.
fn describe(error: &ClientInitializeError) -> String {
    match error {
        ClientInitializeError::TransportError { error, .. } => {
            format!("cannot connect: {}", error.error)
        }
        ClientInitializeError::LegacyFallbackFailed { fallback, .. } => {
            describe(fallback)
        }
        other => format!("cannot connect: {other}"),
    }
}

/// Whether a failed connect is worth another try: a network error, or
/// an HTTP 408, 429 or 5xx but 501.
fn transient(error: &ClientInitializeError) -> bool {
    match error {
        ClientInitializeError::TransportError { error, .. } => {
            transient_source(&*error.error)
        }
        ClientInitializeError::LegacyFallbackFailed { discover, fallback } => {
            transient(discover) || transient(fallback)
        }
        ClientInitializeError::ConnectionClosed(_) => true,
        _ => false,
    }
}

fn transient_source(error: &(dyn std::error::Error + 'static)) -> bool {
    if let Some(error) =
        error.downcast_ref::<StreamableHttpError<reqwest::Error>>()
    {
        return match error {
            StreamableHttpError::Client(error) => transient_reqwest(error),
            StreamableHttpError::UnexpectedServerResponse(text) => {
                response_status(text).is_some_and(transient_status)
            }
            StreamableHttpError::Io(_)
            | StreamableHttpError::UnexpectedEndOfStream
            | StreamableHttpError::ControlRequestTimeout
            | StreamableHttpError::TransportChannelClosed => true,
            _ => false,
        };
    }
    if let Some(error) = error.downcast_ref::<reqwest::Error>() {
        return transient_reqwest(error);
    }
    if error.downcast_ref::<std::io::Error>().is_some() {
        return true;
    }
    match error.source() {
        Some(source) => transient_source(source),
        // The HTTP worker may hand the error on as text only; rmcp writes
        // a failed response as "unexpected server response: HTTP <status>".
        None => error
            .to_string()
            .split_once("unexpected server response: ")
            .and_then(|(_, rest)| response_status(rest))
            .is_some_and(transient_status),
    }
}

/// The status of rmcp's `HTTP <status>: <body>` text for a failed
/// response.
fn response_status(text: &str) -> Option<u16> {
    text.strip_prefix("HTTP ")?.get(..3)?.parse().ok()
}

fn transient_reqwest(error: &reqwest::Error) -> bool {
    match error.status() {
        Some(status) => transient_status(status.as_u16()),
        None => error.is_connect() || error.is_timeout() || error.is_request(),
    }
}

/// 408, 429, and every 5xx but 501.
pub(crate) fn transient_status(status: u16) -> bool {
    matches!(status, 408 | 429)
        || ((500..600).contains(&status) && status != 501)
}

/// What a failed connect says about signing in: the server answered
/// 401 (a sign-in), or 403 with `insufficient_scope` (more scopes), and
/// its challenge, when the answer carried one.
fn auth_need(error: &ClientInitializeError) -> Option<AuthNeed> {
    match error {
        ClientInitializeError::TransportError { error, .. } => {
            auth_source(&*error.error)
        }
        ClientInitializeError::LegacyFallbackFailed { discover, fallback } => {
            auth_need(fallback).or_else(|| auth_need(discover))
        }
        _ => None,
    }
}

fn auth_source(error: &(dyn std::error::Error + 'static)) -> Option<AuthNeed> {
    if let Some(error) =
        error.downcast_ref::<StreamableHttpError<reqwest::Error>>()
    {
        match error {
            StreamableHttpError::AuthRequired(error) => {
                return Some(AuthNeed::challenged(
                    &error.www_authenticate_header,
                ));
            }
            StreamableHttpError::InsufficientScope(error) => {
                return Some(AuthNeed {
                    scope: error
                        .required_scope
                        .clone()
                        .or_else(|| scope_of(&error.www_authenticate_header)),
                    ..AuthNeed::challenged(&error.www_authenticate_header)
                });
            }
            StreamableHttpError::Auth(AuthError::AuthorizationRequired) => {
                return Some(AuthNeed::default());
            }
            StreamableHttpError::UnexpectedServerResponse(text)
                if response_status(text) == Some(401) =>
            {
                return Some(AuthNeed::default());
            }
            StreamableHttpError::Client(error)
                if error.status().map(|s| s.as_u16()) == Some(401) =>
            {
                return Some(AuthNeed::default());
            }
            _ => {}
        }
    }
    if let Some(error) = error.downcast_ref::<AuthRequiredError>() {
        return Some(AuthNeed::challenged(&error.www_authenticate_header));
    }
    if let Some(error) = error.downcast_ref::<InsufficientScopeError>() {
        return Some(AuthNeed {
            scope: error.required_scope.clone(),
            ..AuthNeed::challenged(&error.www_authenticate_header)
        });
    }
    if let Some(source) = error.source() {
        return auth_source(source);
    }
    // The HTTP worker may hand the error on as text only.
    let text = error.to_string();
    if let Some((_, challenge)) = text.split_once("authorization required: ") {
        return Some(AuthNeed::challenged(challenge));
    }
    if let Some((_, challenge)) = text.split_once("insufficient scope: ") {
        return Some(AuthNeed {
            scope: scope_of(challenge),
            ..AuthNeed::challenged(challenge)
        });
    }
    text.split_once("unexpected server response: ")
        .and_then(|(_, rest)| response_status(rest))
        .filter(|status| *status == 401)
        .map(|_| AuthNeed::default())
}

/// The `scope` parameter of a `WWW-Authenticate` challenge.
pub(crate) fn scope_of(challenge: &str) -> Option<String> {
    let at = challenge.find("scope=")?;
    // `scope=` but not `insufficient_scope` or a longer name.
    let before = challenge[..at].chars().next_back();
    if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
        return scope_of(&challenge[at + 6..]);
    }
    let rest = &challenge[at + 6..];
    let value = match rest.strip_prefix('"') {
        Some(quoted) => quoted.split('"').next()?,
        None => rest.split([',', ' ']).next()?,
    };
    (!value.is_empty()).then(|| value.to_owned())
}
