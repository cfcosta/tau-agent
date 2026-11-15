//! A [`Host`] with a few tools, for the sandbox tests.

#![allow(dead_code)]

use std::{
    sync::{
        Arc,
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use heiko_codemode::store::Snapshot;
use heiko_codemode_host::{
    CancellationToken,
    Host,
    Namespace,
    Outcome,
    Request,
    ToolCall,
    ToolEntry,
    ToolReply,
    options,
    run,
};
use heiko_decide::Decider;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

/// Tools:
/// - `echo`: returns its arguments;
/// - `fail`: fails with `tool broke`;
/// - `sleep`: waits `ms` milliseconds and returns them;
/// - `slow_one`: `sleep`, but sequential;
/// - `hold`: waits until the test opens its gate (see [`Holds`]), once
///   the test has taken [`FakeHost::held`];
/// - `mcp__linear__list_issues`: an MCP-shaped result with `isError`.
#[derive(Default)]
pub struct FakeHost {
    pub calls: Mutex<Vec<ToolCall>>,
    /// Calls whose future was dropped before it finished.
    pub dropped: Arc<AtomicUsize>,
    /// Calls running now, and the most that ever ran at once.
    pub running: Arc<AtomicUsize>,
    pub peak: Arc<AtomicUsize>,
    pub decide: Option<Arc<dyn Decider>>,
    pub holds: Holds,
}

/// A `hold` call that has started, waiting for its gate.
pub struct Held {
    pub args: Value,
    /// The host's calls running as it started, itself among them.
    pub running: usize,
    gate: oneshot::Sender<Value>,
}

impl Held {
    /// Lets the call return its arguments.
    pub fn open(self) {
        let _ = self.gate.send(self.args);
    }
}

/// Where `hold` calls report that they started.
pub struct Holds {
    started: mpsc::UnboundedSender<Held>,
    heard: Mutex<Option<mpsc::UnboundedReceiver<Held>>>,
}

impl Default for Holds {
    fn default() -> Self {
        let (started, heard) = mpsc::unbounded_channel();
        Self {
            started,
            heard: Mutex::new(Some(heard)),
        }
    }
}

pub fn tool(name: &str, description: &str) -> ToolEntry {
    ToolEntry {
        name: name.into(),
        description: description.into(),
        input_schema: json!({ "type": "object", "properties": {} }),
        output_schema: None,
        namespace: None,
        sequential: false,
    }
}

impl FakeHost {
    pub fn with_decide(decide: impl Decider) -> Self {
        Self {
            decide: Some(Arc::new(decide)),
            ..Self::default()
        }
    }

    /// The `hold` calls as they start; taken once.
    pub fn held(&self) -> mpsc::UnboundedReceiver<Held> {
        self.holds.heard.lock().unwrap().take().expect("taken once")
    }

    pub fn names(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|call| call.name.clone())
            .collect()
    }
}

/// Counts a call that is dropped before it finishes.
struct Watch {
    dropped: Arc<AtomicUsize>,
    running: Arc<AtomicUsize>,
    finished: bool,
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.running.fetch_sub(1, Ordering::SeqCst);
        if !self.finished {
            self.dropped.fetch_add(1, Ordering::SeqCst);
        }
    }
}

#[async_trait]
impl Host for FakeHost {
    fn tools(&self) -> Vec<ToolEntry> {
        let mut sleep = tool("sleep", "Waits `ms` milliseconds.");
        sleep.input_schema = json!({
            "type": "object",
            "properties": { "ms": { "type": "integer" } },
            "required": ["ms"],
        });
        let mut slow = sleep.clone();
        slow.name = "slow_one".into();
        slow.sequential = true;
        let mut issues =
            tool("mcp__linear__list_issues", "List Linear issues.");
        issues.namespace = Some("mcp__linear".into());
        issues.input_schema = json!({
            "type": "object",
            "properties": { "team": { "type": "string" } },
        });
        issues.output_schema = Some(json!({
            "type": "object",
            "properties": {
                "content": { "type": "array" },
                "isError": { "type": "boolean" },
                "structuredContent": {
                    "type": "object",
                    "properties": { "count": { "type": "integer" } },
                    "required": ["count"],
                },
            },
        }));
        let mut tools = vec![
            tool("echo", "Returns its arguments."),
            tool("fail", "Always fails."),
            sleep,
            slow,
            issues,
        ];
        // Only a test that listens for held calls has `hold`.
        if self.holds.heard.lock().unwrap().is_none() {
            tools.push(tool("hold", "Waits until the test lets it go."));
        }
        tools
    }

    fn namespaces(&self) -> Vec<Namespace> {
        vec![Namespace {
            name: "mcp__linear".into(),
            description: Some("Linear's issue tracker.".into()),
            instructions: Some("Use team keys such as PI.".into()),
        }]
    }

    async fn call_tool(&self, call: ToolCall) -> Result<ToolReply, String> {
        self.calls.lock().unwrap().push(call.clone());
        let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        let mut watch = Watch {
            dropped: self.dropped.clone(),
            running: self.running.clone(),
            finished: false,
        };
        let result = match call.name.as_str() {
            "echo" => Ok(call.args),
            "fail" => Err("tool broke".into()),
            "sleep" | "slow_one" => {
                let ms = call.args["ms"].as_u64().unwrap_or(0);
                tokio::time::sleep(Duration::from_millis(ms)).await;
                Ok(json!(ms))
            }
            "hold" => {
                let (gate, opened) = oneshot::channel();
                let held = Held {
                    args: call.args,
                    running: now,
                    gate,
                };
                self.holds.started.send(held).expect("the test listens");
                opened.await.map_err(|_| "the gate was dropped".to_owned())
            }
            "mcp__linear__list_issues" => Ok(json!({
                "content": [{ "type": "text", "text": "no access" }],
                "isError": true,
                "structuredContent": { "count": 0 },
            })),
            other => Err(format!("unknown tool {other}")),
        };
        watch.finished = true;
        result.map(|value| ToolReply {
            value,
            error: (call.name == "mcp__linear__list_issues")
                .then(|| "no access".into()),
            usage: None,
            usage_complete: None,
        })
    }

    fn decide(&self) -> Option<Arc<dyn Decider>> {
        self.decide.clone()
    }
}

/// Runs `code` against `host` with an empty store.
pub async fn script(host: &Arc<FakeHost>, code: &str) -> Outcome {
    script_with(host, code, Snapshot::new(), CancellationToken::new()).await
}

pub async fn script_with(
    host: &Arc<FakeHost>,
    code: &str,
    store: Snapshot,
    cancel: CancellationToken,
) -> Outcome {
    let source = options::parse(code).expect("the source parses");
    run(
        host.clone(),
        Request {
            call_id: "call_1".into(),
            source,
            store,
            cancel,
            inputs: Vec::new(),
        },
    )
    .await
}

/// The outcome's text items.
pub fn texts(outcome: &Outcome) -> Vec<String> {
    outcome
        .items
        .iter()
        .filter_map(|item| item.text().map(str::to_owned))
        .collect()
}

/// The failure's head, or a panic if the script succeeded.
pub fn error(outcome: &Outcome) -> String {
    outcome.failure.as_ref().expect("the script failed").head()
}

/// Runs `work` beside `test`, which lets its held calls go; `work` may
/// end only once `test` has.
pub async fn beside<T: std::fmt::Debug>(
    work: impl Future<Output = T>,
    test: impl Future<Output = ()>,
) -> T {
    tokio::pin!(work);
    tokio::select! {
        biased;
        () = test => {}
        early = &mut work => panic!("it ended before the test let it: {early:?}"),
    }
    work.await
}
