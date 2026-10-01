//! An in-process MCP server for the tests: rmcp's `server` feature,
//! reached over a `tokio::io::duplex`, or over stdio by a child process.
//!
//! Its tools:
//! - `echo {text}`: the text, and `{ "echo": text }` as structured
//!   content, with an output schema;
//! - `fail`: an `isError` result with no content;
//! - `slow {steps, ms}`: a progress notification every `ms`, `steps`
//!   times, then `done`;
//! - `hang`: waits until cancelled, and records the cancel;
//! - `crash`: drops the connection while the call is open.
//!
//! `tools/list` answers two tools a page. Every call is logged by name.
//!
//! Built with [`State::with_features`], it also offers resources and
//! prompts (two a page), and says when their lists change:
//! - `file:///notes.txt` (text), `file:///logo.png` (an image),
//!   `file:///data.bin` (other binary), `file:///flaky` (drops the
//!   connection the first time it is read), and two MCP apps' resources
//!   tau leaves out; the template `file:///notes/{name}`, and an app's;
//! - `greet {name, style?}` and `summary`, whose messages hold text, an
//!   embedded resource, a resource link and an image.
#![allow(dead_code)]

use std::{
    borrow::Cow,
    sync::{
        Arc,
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use rmcp::{
    Peer,
    RoleServer,
    ServerHandler,
    ServiceExt,
    model::{
        CallToolRequestParams,
        CallToolResponse,
        CallToolResult,
        ContentBlock,
        GetPromptRequestParams,
        GetPromptResponse,
        GetPromptResult,
        Implementation,
        ListPromptsResult,
        ListResourceTemplatesResult,
        ListResourcesResult,
        ListToolsResult,
        PaginatedRequestParams,
        ProgressNotificationParam,
        ProtocolVersion,
        ReadResourceRequestParams,
        ReadResourceResponse,
        ReadResourceResult,
        ServerCapabilities,
        ServerConfig,
        SubscriptionFilter,
        Tool,
        ToolAnnotations,
    },
    service::{
        NotificationContext,
        RequestContext,
        SubscriptionContext,
        SubscriptionSink,
    },
};
use serde_json::{Value, json};
use tau_mcp::config::Dial;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// What the server shares across its connections.
pub struct State {
    pub tools: Mutex<Vec<Tool>>,
    pub calls: Mutex<Vec<String>>,
    pub cancelled: Mutex<Vec<String>>,
    pub dials: AtomicUsize,
    /// Only 2025-11-25, so the client falls back to `initialize`.
    pub legacy: bool,
    pub instructions: Option<String>,
    /// Offers resources and prompts.
    pub features: bool,
    pub resources: Mutex<Vec<Value>>,
    pub templates: Mutex<Vec<Value>>,
    pub prompts: Mutex<Vec<Value>>,
    /// Every `resources/read`, by URI.
    pub reads: Mutex<Vec<String>>,
    peers: Mutex<Vec<Peer<RoleServer>>>,
    sinks: Mutex<Vec<SubscriptionSink>>,
    /// Stops the current connection's server.
    stop: Mutex<CancellationToken>,
    /// Dials wait while this is false.
    gate: watch::Sender<bool>,
}

fn schema(value: Value) -> Arc<serde_json::Map<String, Value>> {
    Arc::new(value.as_object().unwrap().clone())
}

pub fn tool(name: &str, description: &str) -> Tool {
    Tool::new(
        name.to_owned(),
        description.to_owned(),
        schema(json!({"type": "object", "properties": {}})),
    )
}

fn default_tools() -> Vec<Tool> {
    vec![
        Tool::new(
            "echo",
            "Echoes its text.",
            schema(json!({
                "type": "object",
                "properties": {"text": {"type": "string"}},
                "required": ["text"]
            })),
        )
        .with_raw_output_schema(schema(json!({
            "type": "object",
            "properties": {"echo": {"type": "string"}}
        })))
        .with_annotations(ToolAnnotations::new().read_only(true)),
        tool("fail", "Fails."),
        Tool::new(
            "slow",
            "Reports progress.",
            schema(json!({
                "type": "object",
                "properties": {"steps": {"type": "integer"}, "ms": {"type": "integer"}}
            })),
        ),
        tool("hang", "Never answers."),
        tool("crash", "Drops the connection.")
            .with_annotations(ToolAnnotations::new().destructive(true)),
    ]
}

fn default_resources() -> Vec<Value> {
    vec![
        json!({"uri": "file:///notes.txt", "name": "notes", "title": "Notes", "mimeType": "text/plain"}),
        json!({"uri": "file:///logo.png", "name": "logo", "mimeType": "image/png"}),
        json!({"uri": "file:///data.bin", "name": "data", "mimeType": "application/octet-stream", "size": 3}),
        json!({"uri": "file:///flaky", "name": "flaky"}),
        json!({"uri": "ui://app/main", "name": "app"}),
        json!({"uri": "file:///page.html", "name": "page", "mimeType": "text/html;profile=mcp-app"}),
    ]
}

fn default_templates() -> Vec<Value> {
    vec![
        json!({"uriTemplate": "file:///notes/{name}", "name": "note", "description": "A note by name."}),
        json!({"uriTemplate": "ui://app/{view}", "name": "views"}),
    ]
}

fn default_prompts() -> Vec<Value> {
    vec![
        json!({
            "name": "greet",
            "description": "Greets someone.",
            "arguments": [
                {"name": "name", "description": "Who.", "required": true},
                {"name": "style"}
            ]
        }),
        json!({"name": "summary", "title": "Summary"}),
    ]
}

/// The PNG `logo.png` holds.
pub const LOGO: &[u8] = b"\x89PNG\r\n\x1a\nlogo";

impl State {
    /// A server that also offers resources and prompts.
    pub fn with_features(legacy: bool) -> Arc<Self> {
        let state = Self::new(legacy);
        let mut state = Arc::try_unwrap(state).ok().expect("not shared");
        state.features = true;
        Arc::new(state)
    }

    pub fn new(legacy: bool) -> Arc<Self> {
        Arc::new(Self {
            tools: Mutex::new(default_tools()),
            calls: Mutex::default(),
            cancelled: Mutex::default(),
            dials: AtomicUsize::new(0),
            legacy,
            instructions: Some("Use echo to echo.\nMore details.".into()),
            features: false,
            resources: Mutex::new(default_resources()),
            templates: Mutex::new(default_templates()),
            prompts: Mutex::new(default_prompts()),
            reads: Mutex::default(),
            peers: Mutex::default(),
            sinks: Mutex::default(),
            stop: Mutex::new(CancellationToken::new()),
            gate: watch::channel(true).0,
        })
    }

    /// Opens or closes the gate dials wait at.
    pub fn gate(&self, open: bool) {
        self.gate.send_replace(open);
    }

    pub fn dials(&self) -> usize {
        self.dials.load(Ordering::SeqCst)
    }

    pub fn calls(&self, name: &str) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| *c == name)
            .count()
    }

    /// A dial that starts a server on a fresh duplex stream.
    pub fn dial(self: &Arc<Self>) -> Dial {
        let state = self.clone();
        Dial::new(move || {
            let state = state.clone();
            async move {
                state.dials.fetch_add(1, Ordering::SeqCst);
                let mut gate = state.gate.subscribe();
                let _ = gate.wait_for(|open| *open).await;
                let (client, server) = tokio::io::duplex(1 << 16);
                let stop = CancellationToken::new();
                *state.stop.lock().unwrap() = stop.clone();
                tokio::spawn(async move {
                    if let Ok(service) =
                        Server(state).serve_with_ct(server, stop).await
                    {
                        let _ = service.waiting().await;
                    }
                });
                Ok(client)
            }
        })
    }

    /// Adds or replaces a tool and tells every client.
    pub async fn add_tool(&self, tool: Tool) {
        {
            let mut tools = self.tools.lock().unwrap();
            tools.retain(|t| t.name != tool.name);
            tools.push(tool);
        }
        self.tools_changed().await;
    }

    /// Removes a tool and tells every client.
    pub async fn remove_tool(&self, name: &str) {
        self.tools.lock().unwrap().retain(|t| t.name != name);
        self.tools_changed().await;
    }

    async fn tools_changed(&self) {
        let peers = self.peers.lock().unwrap().clone();
        for peer in peers {
            let _ = peer.notify_tool_list_changed().await;
        }
        let sinks = self.sinks.lock().unwrap().clone();
        for sink in sinks {
            let _ = sink.notify_tool_list_changed().await;
        }
    }

    /// Adds a resource and tells every client.
    pub async fn add_resource(&self, resource: Value) {
        self.resources.lock().unwrap().push(resource);
        let peers = self.peers.lock().unwrap().clone();
        for peer in peers {
            let _ = peer.notify_resource_list_changed().await;
        }
        let sinks = self.sinks.lock().unwrap().clone();
        for sink in sinks {
            let _ = sink.notify_resource_list_changed().await;
        }
    }

    /// Adds a prompt and tells every client.
    pub async fn add_prompt(&self, prompt: Value) {
        self.prompts.lock().unwrap().push(prompt);
        let peers = self.peers.lock().unwrap().clone();
        for peer in peers {
            let _ = peer.notify_prompt_list_changed().await;
        }
        let sinks = self.sinks.lock().unwrap().clone();
        for sink in sinks {
            let _ = sink.notify_prompt_list_changed().await;
        }
    }

    pub fn reads(&self, uri: &str) -> usize {
        self.reads
            .lock()
            .unwrap()
            .iter()
            .filter(|read| *read == uri)
            .count()
    }

    /// Drops the current connection.
    pub fn crash(&self) {
        self.stop.lock().unwrap().cancel();
    }
}

#[derive(Clone)]
pub struct Server(pub Arc<State>);

fn capabilities(features: bool) -> ServerCapabilities {
    if features {
        ServerCapabilities::builder()
            .enable_tools()
            .enable_tool_list_changed()
            .enable_resources()
            .enable_resources_list_changed()
            .enable_prompts()
            .enable_prompts_list_changed()
            .build()
    } else {
        ServerCapabilities::builder()
            .enable_tools()
            .enable_tool_list_changed()
            .build()
    }
}

/// Page `start` of `items`, two a page, as `(page, next cursor)`.
fn page<T: serde::de::DeserializeOwned>(
    items: &[Value],
    request: Option<PaginatedRequestParams>,
) -> (Vec<T>, Option<String>) {
    let start: usize = request
        .and_then(|r| r.cursor)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let end = (start + 2).min(items.len());
    (
        items[start..end]
            .iter()
            .map(|item| serde_json::from_value(item.clone()).unwrap())
            .collect(),
        (end < items.len()).then(|| end.to_string()),
    )
}

impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        let config = ServerConfig::new(capabilities(self.0.features))
            .with_server_info(Implementation::new("test-server", "1.0.0"));
        match &self.0.instructions {
            Some(text) => config.with_instructions(text.clone()),
            None => config,
        }
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        if self.0.legacy {
            Cow::Owned(vec![ProtocolVersion::V_2025_11_25])
        } else {
            Cow::Borrowed(ProtocolVersion::KNOWN_VERSIONS)
        }
    }

    async fn on_initialized(&self, context: NotificationContext<RoleServer>) {
        self.0.peers.lock().unwrap().push(context.peer);
    }

    fn accepted_subscription_filter(
        &self,
        requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        Some(requested.supported_by(&capabilities(self.0.features)))
    }

    async fn listen(
        &self,
        context: SubscriptionContext,
    ) -> Result<(), rmcp::ErrorData> {
        self.0.sinks.lock().unwrap().push(context.sink().clone());
        context.cancelled().await;
        Ok(())
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        let tools = self.0.tools.lock().unwrap().clone();
        let start: usize = request
            .and_then(|r| r.cursor)
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);
        let end = (start + 2).min(tools.len());
        Ok(ListToolsResult {
            tools: tools[start..end].to_vec(),
            next_cursor: (end < tools.len()).then(|| end.to_string()),
            ..ListToolsResult::default()
        })
    }

    async fn list_resources(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, rmcp::ErrorData> {
        let items = self.0.resources.lock().unwrap().clone();
        let (resources, next_cursor) = page(&items, request);
        Ok(ListResourcesResult {
            resources,
            next_cursor,
            ..ListResourcesResult::default()
        })
    }

    async fn list_resource_templates(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, rmcp::ErrorData> {
        let items = self.0.templates.lock().unwrap().clone();
        let (resource_templates, next_cursor) = page(&items, request);
        Ok(ListResourceTemplatesResult {
            resource_templates,
            next_cursor,
            ..ListResourceTemplatesResult::default()
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, rmcp::ErrorData> {
        use base64::Engine;
        let uri = request.uri.clone();
        self.0.reads.lock().unwrap().push(uri.clone());
        let blob = |bytes: &[u8]| {
            base64::engine::general_purpose::STANDARD.encode(bytes)
        };
        let contents = match uri.as_str() {
            "file:///notes.txt" => {
                json!([{"uri": uri, "mimeType": "text/plain", "text": "Remember the milk."}])
            }
            "file:///logo.png" => {
                json!([{"uri": uri, "mimeType": "image/png", "blob": blob(LOGO)}])
            }
            "file:///data.bin" => json!([
                {"uri": uri, "mimeType": "application/octet-stream", "blob": blob(b"\x00\x01\x02")},
                {"uri": "ui://app/main", "mimeType": "text/html;profile=mcp-app", "text": "<app>"}
            ]),
            "file:///flaky" => {
                if self.0.reads(&uri) == 1 {
                    self.0.crash();
                    tokio::time::sleep(Duration::from_secs(30)).await;
                }
                json!([{"uri": uri, "text": "steady"}])
            }
            other => match other.strip_prefix("file:///notes/") {
                Some(name) => {
                    json!([{"uri": uri, "text": format!("note {name}")}])
                }
                None => {
                    return Err(rmcp::ErrorData::resource_not_found(
                        format!("no resource {other}"),
                        None,
                    ));
                }
            },
        };
        let result: ReadResourceResult =
            serde_json::from_value(json!({ "contents": contents })).unwrap();
        Ok(ReadResourceResponse::Complete(result))
    }

    async fn list_prompts(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, rmcp::ErrorData> {
        let items = self.0.prompts.lock().unwrap().clone();
        let (prompts, next_cursor) = page(&items, request);
        Ok(ListPromptsResult {
            prompts,
            next_cursor,
            ..ListPromptsResult::default()
        })
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, rmcp::ErrorData> {
        let args = request.arguments.unwrap_or_default();
        let arg = |name: &str| args.get(name).and_then(Value::as_str);
        let messages = match request.name.as_str() {
            "greet" => {
                let Some(name) = arg("name") else {
                    return Err(rmcp::ErrorData::invalid_params(
                        "greet needs a name",
                        None,
                    ));
                };
                let style = arg("style").unwrap_or("plain");
                json!([
                    {"role": "user", "content": {"type": "text", "text": format!("Say hello to {name}, {style}.")}}
                ])
            }
            "summary" => json!([
                {"role": "user", "content": {"type": "text", "text": "Summarize these."}},
                {"role": "user", "content": {"type": "resource", "resource": {"uri": "file:///notes.txt", "text": "Remember the milk."}}},
                {"role": "user", "content": {"type": "resource_link", "uri": "file:///data.bin", "name": "data"}},
                {"role": "user", "content": {"type": "image", "data": "aGk=", "mimeType": "image/png"}}
            ]),
            other => {
                return Err(rmcp::ErrorData::invalid_params(
                    format!("no prompt {other}"),
                    None,
                ));
            }
        };
        let result: GetPromptResult = serde_json::from_value(
            json!({ "description": "A prompt.", "messages": messages }),
        )
        .unwrap();
        Ok(GetPromptResponse::Complete(result))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, rmcp::ErrorData> {
        let name = request.name.to_string();
        self.0.calls.lock().unwrap().push(name.clone());
        let args = request.arguments.unwrap_or_default();
        let result = match name.as_str() {
            "echo" => {
                let text = args
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let mut result =
                    CallToolResult::success(vec![ContentBlock::text(text)]);
                result.structured_content = Some(json!({ "echo": text }));
                result
            }
            "fail" => CallToolResult::error(vec![]),
            "slow" => {
                let steps =
                    args.get("steps").and_then(Value::as_u64).unwrap_or(3);
                let ms = args.get("ms").and_then(Value::as_u64).unwrap_or(10);
                let token = context.meta.get_progress_token();
                for step in 0..steps {
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                    if let Some(token) = &token {
                        let _ = context
                            .peer
                            .notify_progress(
                                ProgressNotificationParam::new(
                                    token.clone(),
                                    step as f64 + 1.0,
                                )
                                .with_total(steps as f64)
                                .with_message(format!("step {}", step + 1)),
                            )
                            .await;
                    }
                }
                CallToolResult::success(vec![ContentBlock::text("done")])
            }
            "hang" => {
                tokio::select! {
                    () = context.ct.cancelled() => {
                        self.0.cancelled.lock().unwrap().push(name);
                    }
                    () = tokio::time::sleep(Duration::from_secs(30)) => {}
                }
                CallToolResult::success(vec![ContentBlock::text("late")])
            }
            "crash" => {
                self.0.crash();
                tokio::time::sleep(Duration::from_secs(30)).await;
                CallToolResult::success(vec![])
            }
            other => {
                let known = self
                    .0
                    .tools
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|t| t.name == other);
                if !known {
                    return Err(rmcp::ErrorData::invalid_params(
                        format!("no tool {other}"),
                        None,
                    ));
                }
                CallToolResult::success(vec![ContentBlock::text(format!(
                    "{other} ran"
                ))])
            }
        };
        Ok(CallToolResponse::Complete(result))
    }
}
