//! The Codemode plugin in the loop: `Agent` with `ScriptedModel`,
//! `Store::memory()` and paused time.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use serde_json::{Value, json};
use tau_agent::{
    agent::Agent,
    error::{PluginError, ToolError},
    event::RunEvent,
    plugin::{Decision, Plugin, PluginCtx, PluginRun, RunPlan, ToolCall},
    tool::{
        AgentTool,
        ExecutionMode,
        Exposure,
        Namespace,
        ToolCtx,
        ToolOutput,
        ToolSource,
    },
};
use tau_ai::message::{InputBlock, Message, ToolResultMessage};
use tau_codemode::{CancellationToken, Codemode, PLUGIN};
use tau_jev::fake::FakeJev;
use tau_store::{Entry, Store};
use tau_testing::{block_on, scripted::ScriptedModel};
use tokio::time::Instant;

/// The id the scripted model gives the first tool call.
const FIRST: &str = "call_0|fc_0";

/// Answers with its `text` after `ms` virtual milliseconds, logging
/// when it ran. With an output schema, its structured output is
/// `{ "echo": <text> }`; with `fail`, it fails, keeping that output.
struct Echo {
    name: &'static str,
    exposure: Exposure,
    mode: ExecutionMode,
    schema: Value,
    output_schema: Option<Value>,
    log: Arc<Mutex<Vec<(Instant, Instant)>>>,
}

impl Echo {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            exposure: Exposure::Direct,
            mode: ExecutionMode::Parallel,
            schema: json!({
                "type": "object",
                "properties": {
                    "text": {"type": "string"},
                    "ms": {"type": "integer"},
                    "fail": {"type": "boolean"}
                }
            }),
            output_schema: None,
            log: Arc::default(),
        }
    }

    fn structured(mut self) -> Self {
        self.output_schema = Some(json!({
            "type": "object",
            "properties": {"echo": {"type": "string"}},
            "required": ["echo"]
        }));
        self
    }
}

#[async_trait]
impl AgentTool for Echo {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        "Echoes its text."
    }
    fn parameters(&self) -> &Value {
        &self.schema
    }
    fn exposure(&self) -> Exposure {
        self.exposure
    }
    fn execution_mode(&self) -> ExecutionMode {
        self.mode
    }
    fn output_schema(&self) -> Option<&Value> {
        self.output_schema.as_ref()
    }
    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let start = Instant::now();
        let ms = args["ms"].as_u64().unwrap_or(0);
        tokio::select! {
            _ = ctx.cancel.cancelled() => {}
            _ = tokio::time::sleep(Duration::from_millis(ms)) => {}
        }
        self.log.lock().unwrap().push((start, Instant::now()));
        let text = args["text"].as_str().unwrap_or_default();
        let output = ToolOutput {
            structured: Some(json!({ "echo": text })),
            ..ToolOutput::text(text)
        };
        if args["fail"].as_bool().unwrap_or(false) {
            return Err(ToolError::output(output));
        }
        Ok(output)
    }
}

/// Records every event; blocks nested calls to `block`.
#[derive(Default, Clone)]
struct Recorder {
    events: Arc<Mutex<Vec<RunEvent>>>,
    block: Option<&'static str>,
}

#[async_trait]
impl PluginRun for Recorder {
    async fn before_tool(
        &mut self,
        call: &mut ToolCall,
        _ctx: &PluginCtx,
    ) -> Result<Decision, PluginError> {
        if Some(call.name.as_str()) == self.block && call.parent.is_some() {
            return Ok(Decision::Block(format!("{} is blocked", call.name)));
        }
        Ok(Decision::Allow)
    }

    async fn on_event(&mut self, event: &RunEvent, _ctx: &PluginCtx) {
        self.events.lock().unwrap().push(event.clone());
    }
}

#[async_trait]
impl Plugin for Recorder {
    fn name(&self) -> &str {
        "recorder"
    }

    async fn start(
        &self,
        _plan: &mut RunPlan,
        _ctx: &PluginCtx,
    ) -> Result<Box<dyn PluginRun>, PluginError> {
        Ok(Box::new(self.clone()))
    }
}

/// A model that calls `codemode` with each script in turn, one per
/// run, answering `done` after each.
fn model(scripts: &[&str]) -> ScriptedModel {
    let mut model = ScriptedModel::new();
    for script in scripts {
        let code = script.to_string();
        model = model
            .turn(move |t| t.tool_call("codemode", json!({ "code": code })))
            .turn(|t| t.text("done"));
    }
    model
}

/// The run's stored tool results.
async fn results(store: &Store, run: &str) -> Vec<ToolResultMessage> {
    store
        .transcript(run)
        .await
        .unwrap()
        .into_iter()
        .filter_map(|entry| match entry {
            Entry::Message { body, .. } => {
                match serde_json::from_str(&body).unwrap() {
                    Message::ToolResult(result) => Some(result),
                    _ => None,
                }
            }
            _ => None,
        })
        .collect()
}

/// A result's text items after the header.
fn items(result: &ToolResultMessage) -> Vec<String> {
    result
        .content
        .iter()
        .skip(1)
        .filter_map(|block| match block {
            InputBlock::Text(text) => Some(text.text.clone()),
            InputBlock::Image(_) => None,
        })
        .collect()
}

/// The one result of a run that made one codemode call.
async fn only_result(store: &Store, run: &str) -> ToolResultMessage {
    let mut results = results(store, run).await;
    assert_eq!(results.len(), 1, "one result in the transcript");
    results.remove(0)
}

/// The last result in a run's transcript, a fork's inherited ones
/// before it.
async fn last_result(store: &Store, run: &str) -> ToolResultMessage {
    results(store, run).await.pop().unwrap()
}

/// A codemode call makes one transcript result however many nested
/// calls it makes; each nested call starts and ends under it, as
/// `<parent>/<n>`, and the details' rows carry the same ids.
#[test]
fn many_nested_calls_make_one_result() {
    block_on(async {
        let store = Store::memory().await.unwrap();
        let recorder = Recorder::default();
        let agent = Agent::new(model(&[r#"
local out = {}
for i = 1, 5 do
    table.insert(out, tools.echo({ text = tostring(i) }))
end
local a, b = parallel(
    function() return tools.echo({ text = "a", ms = 10 }) end,
    function() return tools.echo({ text = "b", ms = 10 }) end
)
return table.concat(out, ",") .. a .. b
"#]))
        .tool(Echo::new("echo"))
        .plugin(Codemode::new(None))
        .plugin(recorder.clone());
        let outcome = agent.run("go", &store).await.unwrap();

        let result = only_result(&store, &outcome.run.0).await;
        assert!(!result.is_error);
        assert_eq!(items(&result), ["1,2,3,4,5ab"]);
        let expected: Vec<String> =
            (1..=7).map(|n| format!("{FIRST}/{n}")).collect();
        let events = recorder.events.lock().unwrap().clone();
        let mut starts = Vec::new();
        let mut ends = Vec::new();
        for event in &events {
            match event {
                RunEvent::ToolStart {
                    call_id,
                    parent: Some(parent),
                    ..
                } => {
                    assert_eq!(parent, FIRST);
                    starts.push(call_id.clone());
                }
                RunEvent::ToolEnd {
                    call_id,
                    parent: Some(parent),
                    ..
                } => {
                    assert_eq!(parent, FIRST);
                    ends.push(call_id.clone());
                }
                _ => {}
            }
        }
        assert_eq!(starts, expected);
        ends.sort();
        let mut sorted = expected.clone();
        sorted.sort();
        assert_eq!(ends, sorted);
        let details = events
            .iter()
            .find_map(|event| match event {
                RunEvent::ToolEnd {
                    call_id,
                    parent: None,
                    output,
                    ..
                } if call_id == FIRST => output.details.clone(),
                _ => None,
            })
            .unwrap();
        let ids: Vec<&str> = details["calls"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, expected);
        assert_eq!(details["complete"], true);
    });
}

/// A plugin's `before_tool` blocks a nested call: `pcall` catches the
/// reason, and uncaught it fails the script at the call's line.
#[test]
fn before_tool_blocks_a_nested_call() {
    block_on(async {
        let store = Store::memory().await.unwrap();
        let echo = Echo::new("echo");
        let log = echo.log.clone();
        let agent = Agent::new(model(&[
            "local ok, err = pcall(tools.echo, { text = 'x' })\n\
             return { ok = ok, err = err }",
            "\ntools.echo({ text = 'x' })",
        ]))
        .tool(echo)
        .plugin(Codemode::new(None))
        .plugin(Recorder {
            block: Some("echo"),
            ..Recorder::default()
        });
        let caught = agent.run("one", &store).await.unwrap();
        let uncaught = agent.run("two", &store).await.unwrap();

        let result = only_result(&store, &caught.run.0).await;
        assert!(!result.is_error);
        let value: Value = serde_json::from_str(&items(&result)[0]).unwrap();
        assert_eq!(value, json!({ "ok": false, "err": "echo is blocked" }));
        let result = only_result(&store, &uncaught.run.0).await;
        assert!(result.is_error);
        let last = items(&result).pop().unwrap();
        assert!(
            last.starts_with("Script error:\ncodemode:2: echo is blocked"),
            "{last}"
        );
        assert!(log.lock().unwrap().is_empty(), "the tool never ran");
    });
}

/// A tool with an output schema returns its structured output as a
/// table, even from a failed call that carries one; without a schema,
/// a call returns its text, and a failure raises its text.
#[test]
fn structured_output_returns_a_table() {
    block_on(async {
        let store = Store::memory().await.unwrap();
        let recorder = Recorder::default();
        let agent = Agent::new(model(&[r#"
local t = tools.typed({ text = "hi" })
local failed = tools.typed({ text = "bad", fail = true })
local plain = tools.plain({ text = "hi" })
local ok, err = pcall(tools.plain, { text = "oops", fail = true })
return { t.echo, failed.echo, plain, ok, err }
"#]))
        .tool(Echo::new("typed").structured())
        .tool(Echo::new("plain"))
        .plugin(Codemode::new(None))
        .plugin(recorder.clone());
        let outcome = agent.run("go", &store).await.unwrap();

        let result = only_result(&store, &outcome.run.0).await;
        let value: Value = serde_json::from_str(&items(&result)[0]).unwrap();
        assert_eq!(value, json!(["hi", "bad", "hi", false, "oops"]));
        assert!(!result.is_error, "the script handled both failed calls");
        let calls = result.details.as_ref().unwrap()["calls"]
            .as_array()
            .unwrap();
        let statuses: Vec<_> = calls
            .iter()
            .map(|row| row["status"].as_str().unwrap())
            .collect();
        assert_eq!(statuses, ["ok", "error", "ok", "error"]);
        assert_eq!(calls[1]["error"], "bad");
        assert_eq!(calls[3]["error"], "oops");
        let events = recorder.events.lock().unwrap();
        let failed: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                RunEvent::ToolEnd {
                    parent: Some(_),
                    is_error,
                    ..
                } => Some(*is_error),
                _ => None,
            })
            .collect();
        assert_eq!(failed, [false, true, false, true]);
    });
}

/// Jev's usage is charged to the run: it reaches the run's total and is
/// stored as the plugin's cost.
#[test]
fn jev_usage_reaches_the_run() {
    block_on(async {
        let store = Store::memory().await.unwrap();
        let jev = FakeJev::nouls(|_| 0.25);
        let agent = Agent::new(model(&[
            "return jev.noul({ state = 1, question = 'odd?' }).probability",
        ]))
        .plugin(Codemode::new(Some(Arc::new(jev.clone()))));
        let outcome = agent.run("go", &store).await.unwrap();

        let result = only_result(&store, &outcome.run.0).await;
        assert_eq!(items(&result), ["0.25"]);
        assert_eq!(jev.requests().len(), 1);
        let costs = store.plugin_costs(&outcome.run.0).await.unwrap();
        assert_eq!(costs.len(), 1);
        assert_eq!(costs[0].plugin, PLUGIN);
        assert!(costs[0].input_tokens > 0);
        assert!(costs[0].cost_usd > 0.0);
        assert!(outcome.usage.input >= costs[0].input_tokens as u64);
        assert!(outcome.usage.cost.total >= costs[0].cost_usd);
    });
}

/// A Jev request makes no run events of its own: the codemode call
/// reports its row in its own updates, as it starts and as it ends,
/// before the call ends, so the card shows it live.
#[test]
fn jev_requests_reach_the_run_as_updates() {
    block_on(async {
        let store = Store::memory().await.unwrap();
        let recorder = Recorder::default();
        let agent = Agent::new(model(&[
            "tools.echo({ text = 'a' })\nreturn jev.noul({ state = 1, question = 'odd?' }).probability",
        ]))
        .tool(Echo::new("echo"))
        .plugin(Codemode::new(Some(Arc::new(FakeJev::nouls(|_| 0.25)))))
        .plugin(recorder.clone());
        agent.run("go", &store).await.unwrap();

        let events = recorder.events.lock().unwrap().clone();
        let updates: Vec<(usize, tau_codemode::live::JevUpdate)> = events
            .iter()
            .enumerate()
            .filter_map(|(at, event)| match event {
                RunEvent::ToolUpdate {
                    call_id,
                    partial,
                    parent: None,
                    ..
                } if call_id == FIRST => Some((
                    at,
                    tau_codemode::live::JevUpdate::from_details(
                        partial.details.as_ref()?,
                    )?,
                )),
                _ => None,
            })
            .collect();
        let statuses: Vec<(&str, usize)> = updates
            .iter()
            .map(|(_, update)| {
                (update.row["status"].as_str().unwrap(), update.after)
            })
            .collect();
        assert_eq!(statuses, [("running", 1), ("ok", 1)]);
        assert_eq!(updates[0].1.row["id"], format!("{FIRST}/jev/1"));
        let end = events
            .iter()
            .position(|event| {
                matches!(event, RunEvent::ToolEnd { call_id, .. } if call_id == FIRST)
            })
            .unwrap();
        assert!(updates.iter().all(|(at, _)| *at < end));
    });
}

/// Without a Jev, `jev` is nil and the description says so.
#[test]
fn without_jev_the_global_is_nil() {
    let plugin = Codemode::new(None);
    assert!(plugin.tool().description().contains("Jev is not available"));
    let plugin = Codemode::new(Some(Arc::new(FakeJev::nouls(|_| 0.5))));
    assert!(plugin.tool().description().contains("jev.noul"));
}

/// The store holds across runs along the fork chain: a fork sees what
/// its ancestors wrote, not what a sibling wrote; a failed script
/// writes nothing.
#[test]
fn the_store_follows_forks() {
    block_on(async {
        let store = Store::memory().await.unwrap();
        let agent = Agent::new(model(&[
            "store('a', 1)",
            "store('b', load('a') + 1)\nreturn load('b')",
            "return { a = load('a'), b = load('b') or 'none' }",
            "store('c', 3)\nerror('boom')",
            "return load('c') or 'none'",
        ]))
        .plugin(Codemode::new(None));
        let root = agent.run("root", &store).await.unwrap();
        let fork = agent
            .fork(&root.checkpoint())
            .run("fork", &store)
            .await
            .unwrap();
        let sibling = agent
            .fork(&root.checkpoint())
            .run("sibling", &store)
            .await
            .unwrap();
        let failed = agent
            .fork(&sibling.checkpoint())
            .run("failed", &store)
            .await
            .unwrap();
        let after = agent
            .fork(&failed.checkpoint())
            .run("after", &store)
            .await
            .unwrap();

        assert_eq!(items(&last_result(&store, &fork.run.0).await), ["2"]);
        let value: Value = serde_json::from_str(
            &items(&last_result(&store, &sibling.run.0).await)[0],
        )
        .unwrap();
        assert_eq!(value, json!({ "a": 1, "b": "none" }));
        assert!(last_result(&store, &failed.run.0).await.is_error);
        assert_eq!(items(&last_result(&store, &after.run.0).await), ["none"]);
        let records = store.records(&after.run.0, PLUGIN).await.unwrap();
        assert_eq!(records.len(), 1, "only the root's write: {records:?}");
    });
}

/// `codemode` is not callable from a script: not on `tools`, not in
/// `ALL_TOOLS`.
#[test]
fn codemode_cannot_call_itself() {
    block_on(async {
        let store = Store::memory().await.unwrap();
        let agent = Agent::new(model(&[r#"
local names = {}
for _, tool in ALL_TOOLS do table.insert(names, tool.name) end
table.sort(names)
return { tools.codemode == nil, table.concat(names, ",") }
"#]))
        .tool(Echo::new("echo"))
        .plugin(Codemode::new(None));
        let outcome = agent.run("go", &store).await.unwrap();

        let result = only_result(&store, &outcome.run.0).await;
        let value: Value = serde_json::from_str(&items(&result)[0]).unwrap();
        assert_eq!(
            value,
            json!([
                true,
                "echo,infer,module_define,module_inspect,module_list,module_promote,module_select,module_test"
            ])
        );
    });
}

/// `start` puts the signatures of the run's `Direct` tools in its
/// context: not codemode's, not a `Nested` tool's.
#[test]
fn start_lists_the_direct_tools_signatures() {
    block_on(async {
        let store = Store::memory().await.unwrap();
        let llm = ScriptedModel::new().turn(|t| t.text("done"));
        let mut nested = Echo::new("hidden");
        nested.exposure = Exposure::Nested;
        let agent = Agent::new(llm.clone())
            .tool(Echo::new("echo").structured())
            .tool(nested)
            .plugin(Codemode::new(None));
        agent.run("go", &store).await.unwrap();

        let first = format!("{:?}", llm.requests()[0].transcript);
        assert!(
            first.contains(
                "function tools.echo(args: { fail: boolean?, ms: number?, \
                 text: string? }): { echo: string }"
            ),
            "{first}"
        );
        assert!(!first.contains("tools.hidden"), "{first}");
        assert!(!first.contains("tools.codemode"), "{first}");
    });
}

/// Among one script's calls, a `Sequential` tool's call runs alone; the
/// others overlap. Calls start in the order the script made them, so a
/// call after a sequential one waits for it.
#[test]
fn a_sequential_tool_runs_alone_within_a_script() {
    block_on(async {
        let store = Store::memory().await.unwrap();
        let mut seq = Echo::new("seq");
        seq.mode = ExecutionMode::Sequential;
        let seq_log = seq.log.clone();
        let par = Echo::new("par");
        let par_log = par.log.clone();
        let agent = Agent::new(model(&[r#"
parallel(
    function() return tools.par({ ms = 100 }) end,
    function() return tools.par({ ms = 100 }) end,
    function() return tools.seq({ ms = 100 }) end,
    function() return tools.par({ ms = 100 }) end,
    function() return tools.seq({ ms = 100 }) end
)
"#]))
        .tool(seq)
        .tool(par)
        .plugin(Codemode::new(None));
        agent.run("go", &store).await.unwrap();

        let seqs = seq_log.lock().unwrap().clone();
        let pars = par_log.lock().unwrap().clone();
        assert_eq!((seqs.len(), pars.len()), (2, 3));
        let all: Vec<(Instant, Instant)> =
            seqs.iter().chain(&pars).copied().collect();
        for (start, end) in &seqs {
            let overlapping =
                all.iter().filter(|(s, e)| s < end && start < e).count();
            assert_eq!(overlapping, 1, "a sequential call runs alone");
        }
        let par_overlap = pars.iter().enumerate().any(|(i, (s, e))| {
            pars.iter()
                .enumerate()
                .any(|(j, (s2, e2))| i != j && s2 < e && s < e2)
        });
        assert!(par_overlap, "parallel calls still overlap");
    });
}

/// A source whose tools appear once it is asked to be ready, recording
/// what it was asked for.
#[derive(Default)]
struct Lazy {
    asked: Mutex<Vec<Option<Vec<String>>>>,
}

#[async_trait]
impl ToolSource for Lazy {
    fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        if self.asked.lock().unwrap().is_empty() {
            return Vec::new();
        }
        let mut echo = Echo::new("mcp__x__echo");
        echo.exposure = Exposure::Nested;
        vec![Arc::new(echo)]
    }

    fn namespaces(&self) -> Vec<Namespace> {
        vec![Namespace {
            name: "mcp__x".into(),
            description: "X.".into(),
            instructions: None,
            tools: vec!["mcp__x__echo".into()],
        }]
    }

    async fn ready(
        &self,
        namespaces: Option<&[String]>,
        _cancel: &CancellationToken,
    ) {
        self.asked
            .lock()
            .unwrap()
            .push(namespaces.map(<[String]>::to_vec));
    }
}

struct Sourced(Arc<Lazy>);

#[async_trait]
impl Plugin for Sourced {
    fn name(&self) -> &str {
        "sourced"
    }

    fn tool_source(&self) -> Option<Arc<dyn ToolSource>> {
        Some(self.0.clone())
    }
}

/// A script waits for the servers it names before it starts, so it
/// can call their tools; one that searches waits for every server; one
/// that names none waits for nothing.
#[test]
fn a_script_waits_for_the_servers_it_needs() {
    block_on(async {
        let store = Store::memory().await.unwrap();
        let lazy = Arc::new(Lazy::default());
        let agent = Agent::new(model(&[
            "return 1",
            "return tools.mcp__x__echo({ text = 'x' })",
            "return #search_tools('echo')",
        ]))
        .plugin(Sourced(lazy.clone()))
        .plugin(Codemode::new(None));
        let none = agent.run("one", &store).await.unwrap();
        let named = agent.run("two", &store).await.unwrap();
        let all = agent.run("three", &store).await.unwrap();

        assert_eq!(items(&only_result(&store, &none.run.0).await), ["1"]);
        assert_eq!(items(&only_result(&store, &named.run.0).await), ["x"]);
        assert_eq!(items(&only_result(&store, &all.run.0).await), ["1"]);
        assert_eq!(
            *lazy.asked.lock().unwrap(),
            [Some(vec!["mcp__x".to_owned()]), None]
        );
    });
}
