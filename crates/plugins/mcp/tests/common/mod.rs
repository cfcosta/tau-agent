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
        Implementation,
        ListToolsResult,
        PaginatedRequestParams,
        ProgressNotificationParam,
        ProtocolVersion,
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

impl State {
    pub fn new(legacy: bool) -> Arc<Self> {
        Arc::new(Self {
            tools: Mutex::new(default_tools()),
            calls: Mutex::default(),
            cancelled: Mutex::default(),
            dials: AtomicUsize::new(0),
            legacy,
            instructions: Some("Use echo to echo.\nMore details.".into()),
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

    /// Drops the current connection.
    pub fn crash(&self) {
        self.stop.lock().unwrap().cancel();
    }
}

#[derive(Clone)]
pub struct Server(pub Arc<State>);

fn capabilities() -> ServerCapabilities {
    ServerCapabilities::builder()
        .enable_tools()
        .enable_tool_list_changed()
        .build()
}

impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        let config = ServerConfig::new(capabilities())
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
        Some(requested.supported_by(&capabilities()))
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
