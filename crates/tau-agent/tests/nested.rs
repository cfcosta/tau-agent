//! Nested calls (`docs/reference/plugins.md`, "Nested calls"): tools
//! that call tools through the loop, exposure, tool sources and per-run
//! tools, driven through `Agent` with `ScriptedModel`, `tau_store_sqlite::memory()`
//! and paused time.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use futures_util::future::join_all;
use hegel::generators as gs;
use serde_json::{Value, json};
use tau_agent::{
    agent::Agent,
    error::{PluginError, ToolError},
    event::RunEvent,
    plugin::{
        Decision,
        Plugin,
        PluginCtx,
        PluginRun,
        RunPlan,
        ToolCall,
        ToolResultView,
    },
    tool::{
        AgentTool,
        ENDED,
        ExecutionMode,
        Exposure,
        Namespace,
        ToolCtx,
        ToolOutput,
        ToolSource,
    },
};
use tau_ai::message::{InputBlock, Message};
use tau_testing::{block_on, scripted::ScriptedModel};
use tokio::time::Instant;

mod common;
use common::{assert_grammar, stored};

/// The text of an output.
fn text(output: &ToolOutput) -> String {
    output
        .content
        .iter()
        .filter_map(|block| match block {
            InputBlock::Text(text) => Some(text.text.as_str()),
            InputBlock::Image(_) => None,
        })
        .collect()
}

/// Answers with its `text` argument after `ms` virtual milliseconds
/// (stopping early on cancel), with `{ "echo": <args> }` as its
/// structured output, and logs when it ran and whether it was cancelled.
struct Echo {
    name: &'static str,
    exposure: Exposure,
    mode: ExecutionMode,
    schema: Value,
    log: Arc<Mutex<Vec<Ran>>>,
}

#[derive(Debug, Clone)]
struct Ran {
    start: Instant,
    end: Instant,
    cancelled: bool,
    plugin: Option<String>,
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
                    "ms": {"type": "integer", "minimum": 0}
                },
                "required": ["text"]
            }),
            log: Arc::default(),
        }
    }

    fn exposure(mut self, exposure: Exposure) -> Self {
        self.exposure = exposure;
        self
    }

    fn mode(mut self, mode: ExecutionMode) -> Self {
        self.mode = mode;
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
    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let start = Instant::now();
        let ms = args["ms"].as_u64().unwrap_or(0);
        let cancelled = tokio::select! {
            _ = ctx.cancel.cancelled() => true,
            _ = tokio::time::sleep(Duration::from_millis(ms)) => false,
        };
        self.log.lock().unwrap().push(Ran {
            start,
            end: Instant::now(),
            cancelled,
            plugin: ctx.plugin().map(|plugin| plugin.plugin().to_owned()),
        });
        if cancelled {
            return Err("cancelled".into());
        }
        Ok(ToolOutput {
            structured: Some(json!({ "echo": args })),
            ..ToolOutput::text(args["text"].as_str().unwrap_or_default())
        })
    }
}

/// Makes the nested calls its `calls` argument lists, one after another
/// or all at once (`parallel`), and answers with each result in order:
/// `{ "ok": text, "structured": … }` or `{ "err": message }`. With
/// `detach`, it starts them in tasks and returns at once.
#[derive(Default)]
struct Caller {
    exposure: Exposure,
    /// The context of its last call, to call through after it ended.
    kept: Arc<Mutex<Option<ToolCtx>>>,
    /// What `ToolCtx::catalog` and `ToolCtx::plugin` showed it.
    seen: Arc<Mutex<Vec<String>>>,
}

fn caller_schema() -> &'static Value {
    static SCHEMA: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    SCHEMA.get_or_init(|| {
        json!({
            "type": "object",
            "properties": {
                "calls": {"type": "array"},
                "parallel": {"type": "boolean"},
                "detach": {"type": "boolean"}
            },
            "required": ["calls"]
        })
    })
}

fn summary(result: Result<ToolOutput, ToolError>) -> Value {
    match result {
        Ok(output) => {
            json!({ "ok": text(&output), "structured": output.structured })
        }
        Err(error) => json!({ "err": error.to_string() }),
    }
}

#[async_trait]
impl AgentTool for Caller {
    fn name(&self) -> &str {
        "caller"
    }
    fn description(&self) -> &str {
        "Calls tools."
    }
    fn parameters(&self) -> &Value {
        caller_schema()
    }
    fn exposure(&self) -> Exposure {
        self.exposure
    }
    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        *self.kept.lock().unwrap() = Some(ctx.clone());
        let catalog = ctx.catalog();
        self.seen.lock().unwrap().push(format!(
            "plugin {:?} tools {:?} namespaces {:?}",
            ctx.plugin().map(PluginCtx::plugin),
            catalog
                .tools()
                .iter()
                .map(|tool| tool.name())
                .collect::<Vec<_>>(),
            catalog
                .namespaces()
                .iter()
                .map(|space| &space.name)
                .collect::<Vec<_>>(),
        ));
        let calls: Vec<(String, Value)> = args["calls"]
            .as_array()
            .unwrap()
            .iter()
            .map(|call| {
                (
                    call["name"].as_str().unwrap().to_owned(),
                    call["args"].clone(),
                )
            })
            .collect();
        if args["detach"].as_bool().unwrap_or(false) {
            for (name, args) in calls {
                let ctx = ctx.clone();
                tokio::spawn(async move { ctx.call(&name, args).await });
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
            return Ok(ToolOutput::text("detached"));
        }
        let results: Vec<Value> = if args["parallel"].as_bool().unwrap_or(false)
        {
            join_all(
                calls
                    .iter()
                    .map(|(name, args)| ctx.call(name, args.clone())),
            )
            .await
            .into_iter()
            .map(summary)
            .collect()
        } else {
            let mut results = Vec::new();
            for (name, args) in calls {
                results.push(summary(ctx.call(&name, args).await));
            }
            results
        };
        Ok(ToolOutput::text(Value::Array(results).to_string()))
    }
}

/// A call's parent, as hooks see it.
type Parent = Option<String>;

/// A call `before_tool` saw: its id, tool and parent.
type Seen = (String, String, Parent);

/// Records every event, and every `before_tool` with its parent; blocks
/// nested calls to `block`.
#[derive(Default, Clone)]
struct Recorder {
    events: Arc<Mutex<Vec<RunEvent>>>,
    /// `(id, tool, parent)` per `before_tool`.
    before: Arc<Mutex<Vec<Seen>>>,
    /// `(id, parent)` per `after_tool`.
    after: Arc<Mutex<Vec<(String, Parent)>>>,
    block: Option<&'static str>,
}

#[async_trait]
impl PluginRun for Recorder {
    async fn before_tool(
        &mut self,
        call: &mut ToolCall,
        _ctx: &PluginCtx,
    ) -> Result<Decision, PluginError> {
        self.before.lock().unwrap().push((
            call.id.clone(),
            call.name.clone(),
            call.parent.clone(),
        ));
        if Some(call.name.as_str()) == self.block && call.parent.is_some() {
            return Ok(Decision::Block(format!("{} is blocked", call.name)));
        }
        Ok(Decision::Allow)
    }

    async fn after_tool_result(
        &mut self,
        view: &ToolResultView<'_>,
        _output: &mut ToolOutput,
        _ctx: &PluginCtx,
    ) -> Result<(), PluginError> {
        let call = view.call;
        self.after
            .lock()
            .unwrap()
            .push((call.id.clone(), call.parent.clone()));
        Ok(())
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

impl Recorder {
    fn events(&self) -> Vec<RunEvent> {
        self.events.lock().unwrap().clone()
    }
}

/// One turn calling `caller` with `args`, then a final answer.
fn script(args: Value) -> ScriptedModel {
    ScriptedModel::new()
        .turn(|t| t.tool_call("caller", args))
        .turn(|t| t.text("done"))
}

/// The id the scripted model gives the first tool call.
const FIRST: &str = "call_0|fc_0";

/// The outer call's result, parsed.
fn outer_result(transcript: &[Message]) -> Value {
    let results: Vec<_> = transcript
        .iter()
        .filter_map(|message| match message {
            Message::ToolResult(result) => Some(result),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 1, "one result in the transcript");
    assert_eq!(results[0].tool_call_id, FIRST);
    let text: String = results[0]
        .content
        .iter()
        .filter_map(|block| match block {
            InputBlock::Text(text) => Some(text.text.as_str()),
            InputBlock::Image(_) => None,
        })
        .collect();
    serde_json::from_str(&text).unwrap_or(Value::String(text))
}

/// The tool events with a parent, as `(kind, id, parent)`.
fn nested_events(events: &[RunEvent]) -> Vec<(&'static str, String, String)> {
    events
        .iter()
        .filter_map(|event| match event {
            RunEvent::ToolStart {
                call_id,
                parent: Some(parent),
                ..
            } => Some(("start", call_id.clone(), parent.clone())),
            RunEvent::ToolEnd {
                call_id,
                parent: Some(parent),
                ..
            } => Some(("end", call_id.clone(), parent.clone())),
            _ => None,
        })
        .collect()
}

/// A tool calls another through the loop: the nested call goes through
/// `before_tool` and `after_tool` with its parent, has events under the
/// parent with the id `<parent>/1`, returns its text and structured
/// output to the caller, and never reaches the transcript.
#[test]
fn a_tool_calls_another_through_the_loop() {
    block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let recorder = Recorder::default();
        let llm = script(json!({
            "calls": [{"name": "echo", "args": {"text": "hi"}}]
        }));
        let agent = Agent::new(llm.clone())
            .tool(Caller::default())
            .tool(Echo::new("echo"))
            .plugin(recorder.clone());
        let run = agent.start("go", &store);
        let id = run.id();
        run.outcome().await.unwrap();

        let transcript = stored(&store, &id.0).await;
        assert_eq!(
            outer_result(&transcript),
            json!([{
                "ok": "hi",
                "structured": {"echo": {"text": "hi"}}
            }])
        );
        let nested = format!("{FIRST}/1");
        let events = recorder.events();
        assert_grammar(&events);
        assert_eq!(
            nested_events(&events),
            vec![
                ("start", nested.clone(), FIRST.to_owned()),
                ("end", nested.clone(), FIRST.to_owned()),
            ]
        );
        // The nested call ends before its caller does.
        let end_of = |id: &str| {
            events.iter().position(|event| {
                matches!(event, RunEvent::ToolEnd { call_id, .. } if call_id == id)
            })
        };
        assert!(end_of(&nested) < end_of(FIRST));
        // Its end carries the structured output.
        assert!(events.iter().any(|event| matches!(
            event,
            RunEvent::ToolEnd { call_id, output, .. }
                if *call_id == nested && output.structured.is_some()
        )));
        assert_eq!(
            *recorder.before.lock().unwrap(),
            vec![
                (FIRST.to_owned(), "caller".to_owned(), None),
                (nested.clone(), "echo".to_owned(), Some(FIRST.to_owned())),
            ]
        );
        assert_eq!(
            *recorder.after.lock().unwrap(),
            vec![(nested, Some(FIRST.to_owned())), (FIRST.to_owned(), None),]
        );
        // The model saw only the outer call's result.
        let second = &llm.requests()[1].transcript;
        assert_eq!(second, &transcript[..second.len()]);
    });
}

/// A plugin's `before_tool` blocks a nested call as it blocks a model's:
/// the caller gets the reason as an error, and the tool never runs.
#[test]
fn before_tool_blocks_a_nested_call() {
    block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let recorder = Recorder {
            block: Some("echo"),
            ..Recorder::default()
        };
        let echo = Echo::new("echo");
        let log = echo.log.clone();
        let agent = Agent::new(script(json!({
            "calls": [{"name": "echo", "args": {"text": "hi"}}]
        })))
        .tool(Caller::default())
        .tool(echo)
        .plugin(recorder.clone());
        let run = agent.start("go", &store);
        let id = run.id();
        run.outcome().await.unwrap();

        let transcript = stored(&store, &id.0).await;
        assert_eq!(
            outer_result(&transcript),
            json!([{ "err": "echo is blocked" }])
        );
        assert!(log.lock().unwrap().is_empty());
        assert!(recorder.events().iter().any(|event| matches!(
            event,
            RunEvent::ToolEnd {
                parent: Some(_),
                is_error: true,
                ..
            }
        )));
    });
}

/// Unknown names, `ModelOnly` tools and invalid arguments fail the
/// nested call with the message the model would get; `Nested` tools
/// are callable from tools and not declared, and the model cannot call
/// them.
#[test]
fn exposure_decides_what_a_tool_and_the_model_can_call() {
    block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let llm = ScriptedModel::new()
            .turn(|t| {
                t.tool_call(
                    "caller",
                    json!({"calls": [
                        {"name": "missing", "args": {}},
                        {"name": "caller", "args": {"calls": []}},
                        {"name": "hidden", "args": {"text": "nested"}},
                        {"name": "hidden", "args": {}},
                    ]}),
                )
                .tool_call("hidden", json!({"text": "model"}))
            })
            .turn(|t| t.text("done"));
        let agent = Agent::new(llm.clone())
            .tool(Caller {
                exposure: Exposure::ModelOnly,
                ..Caller::default()
            })
            .tool(Echo::new("hidden").exposure(Exposure::Nested));
        let run = agent.start("go", &store);
        let id = run.id();
        run.outcome().await.unwrap();

        let declared: Vec<String> = llm.requests()[0]
            .settings
            .tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect();
        assert_eq!(declared, ["caller"]);
        let transcript = stored(&store, &id.0).await;
        let results: Vec<_> = transcript
            .iter()
            .filter_map(|message| match message {
                Message::ToolResult(result) => Some(result.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(results.len(), 2);
        let first = results[0]
            .content
            .iter()
            .filter_map(|block| match block {
                InputBlock::Text(text) => Some(text.text.clone()),
                InputBlock::Image(_) => None,
            })
            .collect::<String>();
        let first: Value = serde_json::from_str(&first).unwrap();
        assert_eq!(first[0], json!({"err": "Tool missing not found"}));
        assert_eq!(
            first[1],
            json!({"err": "Tool caller cannot be called from a tool"})
        );
        assert_eq!(first[2]["ok"], "nested");
        assert!(
            first[3]["err"].as_str().unwrap().contains("text"),
            "{first}"
        );
        // The model's call to the nested tool is unknown to it.
        assert!(results[1].is_error);
        assert!(matches!(
            &results[1].content[0],
            InputBlock::Text(text) if text.text == "Tool hidden not found"
        ));
    });
}

/// A source's tools are resolved by name when a tool calls one: they
/// are callable, listed in the catalog with the source's namespaces,
/// never declared, and run with their plugin's context. A run tool of
/// the same name hides a source's.
#[test]
fn a_tool_sources_tools_are_callable_but_not_declared() {
    struct Source {
        tools: Vec<Arc<dyn AgentTool>>,
    }

    impl ToolSource for Source {
        fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
            self.tools.clone()
        }

        fn namespaces(&self) -> Vec<Namespace> {
            vec![Namespace {
                name: "mcp__x".into(),
                description: "A server.".into(),
                instructions: Some("Be kind.".into()),
                tools: vec!["mcp__x__echo".into()],
            }]
        }
    }

    struct Sourced(Arc<Source>);

    #[async_trait]
    impl Plugin for Sourced {
        fn name(&self) -> &str {
            "sourced"
        }

        fn tool_source(&self) -> Option<Arc<dyn ToolSource>> {
            Some(self.0.clone())
        }
    }

    block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let echo = Echo::new("mcp__x__echo").exposure(Exposure::Nested);
        let log = echo.log.clone();
        let shadowed = Echo::new("echo");
        let shadowed_log = shadowed.log.clone();
        let source = Arc::new(Source {
            tools: vec![Arc::new(echo), Arc::new(shadowed)],
        });
        let caller = Caller::default();
        let seen = caller.seen.clone();
        let local = Echo::new("echo");
        let local_log = local.log.clone();
        let llm = script(json!({"calls": [
            {"name": "mcp__x__echo", "args": {"text": "from x"}},
            {"name": "echo", "args": {"text": "local"}},
        ]}));
        let agent = Agent::new(llm.clone())
            .tool(caller)
            .tool(local)
            .plugin(Sourced(source));
        let run = agent.start("go", &store);
        let id = run.id();
        run.outcome().await.unwrap();

        let declared: Vec<String> = llm.requests()[0]
            .settings
            .tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect();
        assert_eq!(declared, ["caller", "echo"]);
        let transcript = stored(&store, &id.0).await;
        let result = outer_result(&transcript);
        assert_eq!(result[0]["ok"], "from x");
        assert_eq!(result[1]["ok"], "local");
        assert_eq!(log.lock().unwrap()[0].plugin.as_deref(), Some("sourced"));
        assert_eq!(local_log.lock().unwrap()[0].plugin, None);
        assert!(shadowed_log.lock().unwrap().is_empty());
        assert_eq!(
            *seen.lock().unwrap(),
            [
                r#"plugin None tools ["caller", "echo", "mcp__x__echo"] namespaces ["mcp__x"]"#
            ]
        );
    });
}

/// A plugin adds a tool for one run in `start`: that run declares it,
/// the next one does not, and the tool runs with the plugin's context.
#[test]
fn add_tool_declares_a_tool_for_that_run_only() {
    #[derive(Clone)]
    struct Adder {
        log: Arc<Mutex<Vec<Ran>>>,
        runs: Arc<Mutex<u32>>,
        plans: Arc<Mutex<Vec<Vec<String>>>>,
    }

    #[async_trait]
    impl Plugin for Adder {
        fn name(&self) -> &str {
            "adder"
        }

        async fn start(
            &self,
            plan: &mut RunPlan,
            _ctx: &PluginCtx,
        ) -> Result<Box<dyn PluginRun>, PluginError> {
            let mut runs = self.runs.lock().unwrap();
            *runs += 1;
            if *runs == 1 {
                let mut echo = Echo::new("extra");
                echo.log = self.log.clone();
                plan.add_tool(Arc::new(echo));
            }
            self.plans.lock().unwrap().push(
                plan.tools().iter().map(|t| t.name().to_owned()).collect(),
            );
            Ok(Box::new(()))
        }
    }

    block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let adder = Adder {
            log: Arc::default(),
            runs: Arc::default(),
            plans: Arc::default(),
        };
        let llm = ScriptedModel::new()
            .turn(|t| t.tool_call("extra", json!({"text": "x"})))
            .turn(|t| t.text("done"))
            .turn(|t| t.text("again"));
        let agent = Agent::new(llm.clone())
            .tool(Echo::new("echo"))
            .plugin(adder.clone());
        agent.run("one", &store).await.unwrap();
        agent.run("two", &store).await.unwrap();

        let declared: Vec<Vec<String>> = llm
            .requests()
            .iter()
            .map(|request| {
                request
                    .settings
                    .tools
                    .iter()
                    .map(|t| t.name.clone())
                    .collect()
            })
            .collect();
        assert_eq!(
            declared,
            [vec!["echo", "extra"], vec!["echo", "extra"], vec!["echo"]]
        );
        assert_eq!(
            *adder.plans.lock().unwrap(),
            [vec!["echo", "extra"], vec!["echo"]]
        );
        assert_eq!(
            adder.log.lock().unwrap()[0].plugin.as_deref(),
            Some("adder")
        );
    });
}

/// A call made once its caller has ended fails, and ending the caller
/// cancels the nested calls it left running, which still end with
/// events.
#[test]
fn ending_the_caller_ends_its_nested_calls() {
    block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let recorder = Recorder::default();
        let echo = Echo::new("echo");
        let log = echo.log.clone();
        let caller = Caller::default();
        let kept = caller.kept.clone();
        let agent = Agent::new(script(json!({
            "calls": [{"name": "echo", "args": {"text": "slow", "ms": 60000}}],
            "detach": true
        })))
        .tool(caller)
        .tool(echo)
        .plugin(recorder.clone());
        agent.run("go", &store).await.unwrap();

        let ran = log.lock().unwrap().clone();
        assert_eq!(ran.len(), 1);
        assert!(ran[0].cancelled);
        assert!(ran[0].end - ran[0].start < Duration::from_secs(1));
        let nested = format!("{FIRST}/1");
        assert_eq!(
            nested_events(&recorder.events()),
            vec![
                ("start", nested.clone(), FIRST.to_owned()),
                ("end", nested, FIRST.to_owned()),
            ]
        );
        let ctx = kept.lock().unwrap().clone().unwrap();
        assert_eq!(ctx.call_id(), FIRST);
        let late = ctx.call("echo", json!({"text": "late"})).await;
        assert_eq!(late.unwrap_err().to_string(), ENDED);
    });
}

/// A sequential tool's nested calls run one at a time; a parallel
/// tool's run together.
#[test]
fn a_sequential_tools_nested_calls_run_one_at_a_time() {
    for mode in [ExecutionMode::Sequential, ExecutionMode::Parallel] {
        block_on(async {
            let store = tau_store_sqlite::memory().await.unwrap();
            let echo = Echo::new("echo").mode(mode);
            let log = echo.log.clone();
            let call =
                json!({"name": "echo", "args": {"text": "x", "ms": 100}});
            let agent = Agent::new(script(json!({
                "calls": [call.clone(), call.clone(), call],
                "parallel": true
            })))
            .tool(Caller::default())
            .tool(echo);
            agent.run("go", &store).await.unwrap();

            let mut ran = log.lock().unwrap().clone();
            ran.sort_by_key(|ran| ran.start);
            assert_eq!(ran.len(), 3);
            let overlap = ran.windows(2).any(|w| w[1].start < w[0].end);
            assert_eq!(overlap, mode == ExecutionMode::Parallel, "{mode:?}");
        });
    }
}

/// For any list of nested calls, made one after another or together,
/// to tools that answer, fail or cannot be called: the calls are
/// numbered `<parent>/1` to `<parent>/n` in the order they start, each
/// starts and ends once under its parent, the caller gets each result
/// in order, and the transcript holds only the outer call's result.
#[hegel::test(test_cases = 60)]
fn nested_calls_are_numbered_in_order(tc: hegel::TestCase) {
    let names: Vec<&str> = tc.draw(
        gs::vecs(gs::sampled_from(vec![
            "echo", "hidden", "missing", "caller",
        ]))
        .max_size(6),
    );
    let parallel: bool = tc.draw(gs::booleans());
    block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let recorder = Recorder::default();
        let calls: Vec<Value> = names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                json!({"name": name, "args": {"text": format!("{i}")}})
            })
            .collect();
        let agent = Agent::new(script(json!({
            "calls": calls,
            "parallel": parallel
        })))
        .tool(Caller {
            exposure: Exposure::ModelOnly,
            ..Caller::default()
        })
        .tool(Echo::new("echo"))
        .tool(Echo::new("hidden").exposure(Exposure::Nested))
        .plugin(recorder.clone());
        let run = agent.start("go", &store);
        let id = run.id();
        run.outcome().await.unwrap();

        let events = recorder.events();
        assert_grammar(&events);
        let nested = nested_events(&events);
        let starts: Vec<String> = nested
            .iter()
            .filter(|(kind, ..)| *kind == "start")
            .map(|(_, id, _)| id.clone())
            .collect();
        let expected: Vec<String> =
            (1..=names.len()).map(|n| format!("{FIRST}/{n}")).collect();
        assert_eq!(starts, expected);
        let mut ends: Vec<String> = nested
            .iter()
            .filter(|(kind, ..)| *kind == "end")
            .map(|(_, id, _)| id.clone())
            .collect();
        ends.sort();
        let mut sorted = expected.clone();
        sorted.sort();
        assert_eq!(ends, sorted);
        assert!(nested.iter().all(|(_, _, parent)| parent == FIRST));

        let transcript = stored(&store, &id.0).await;
        let results = outer_result(&transcript);
        let results = results.as_array().unwrap();
        assert_eq!(results.len(), names.len());
        for (i, (name, result)) in names.iter().zip(results).enumerate() {
            match *name {
                "echo" | "hidden" => assert_eq!(result["ok"], format!("{i}")),
                "missing" => {
                    assert_eq!(result["err"], "Tool missing not found")
                }
                _ => assert_eq!(
                    result["err"],
                    "Tool caller cannot be called from a tool"
                ),
            }
        }
    });
}
