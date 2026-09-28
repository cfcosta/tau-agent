//! Plugins (`docs/reference/plugins.md`), driven through `Agent` with
//! `ScriptedModel` and `Store::memory()`.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_agent::{
    agent::{Agent, AgentError},
    event::{LimitKind, RunEvent, StopReason},
    hook::{Decision, ToolCall},
    limits::Limits,
    plugin::{
        FinishedRun,
        Plugin,
        PluginCtx,
        PluginRun,
        RunPlan,
        StopDecision,
    },
    tool::{AgentTool, ToolCtx, ToolOutput},
};
use tau_ai::{
    message::{
        AssistantMessage,
        InputBlock,
        Message,
        Usage,
        UsageCost,
        UserContent,
    },
    responses::request::ReasoningEffort,
};
use tau_store::{Status, Store};
use tau_testing::{block_on, scripted::ScriptedModel};

mod common;
use common::assert_grammar;

/// Changes a run's plan.
type PlanChange = Arc<dyn Fn(&mut RunPlan) + Send + Sync>;

/// A plugin built from closures, for one test at a time.
#[derive(Clone, Default)]
struct Probe {
    name: &'static str,
    /// Changes the plan in `start`.
    plan: Option<PlanChange>,
    /// Fails `start` with this message.
    fail_start: Option<&'static str>,
    /// Continues a stopping run this many times, then lets it stop.
    continue_times: u32,
    /// Charges this usage in `start`.
    charge: Option<Usage>,
    /// Stores this record in `start`.
    record: Option<Value>,
    tools: Vec<Arc<dyn AgentTool>>,
    log: Arc<Mutex<Vec<String>>>,
    finished: Arc<Mutex<Vec<(usize, StopReason, String)>>>,
    records_seen: Arc<Mutex<Vec<Vec<Value>>>>,
}

impl Probe {
    fn named(name: &'static str) -> Self {
        Self {
            name,
            ..Self::default()
        }
    }

    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

#[async_trait]
impl Plugin for Probe {
    fn name(&self) -> &str {
        self.name
    }

    fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        self.tools.clone()
    }

    async fn start(
        &self,
        plan: &mut RunPlan,
        ctx: &PluginCtx,
    ) -> anyhow::Result<Box<dyn PluginRun>> {
        self.log.lock().unwrap().push(format!(
            "{} start: instructions {:?}",
            self.name, plan.instructions
        ));
        self.records_seen
            .lock()
            .unwrap()
            .push(plan.records().to_vec());
        if let Some(message) = self.fail_start {
            anyhow::bail!(message);
        }
        if let Some(change) = &self.plan {
            change(plan);
        }
        if let Some(usage) = &self.charge {
            ctx.charge(usage);
        }
        if let Some(record) = &self.record {
            ctx.record(record).await?;
        }
        Ok(Box::new(ProbeRun {
            probe: self.clone(),
            calls: 0,
            continued: 0,
        }))
    }
}

struct ProbeRun {
    probe: Probe,
    /// Tool calls this run has seen: per run, not per plugin.
    calls: u32,
    continued: u32,
}

#[async_trait]
impl PluginRun for ProbeRun {
    async fn before_tool(
        &mut self,
        call: &mut ToolCall,
        _ctx: &PluginCtx,
    ) -> anyhow::Result<Decision> {
        self.calls += 1;
        if call.args["text"] == "forbidden" {
            return Ok(Decision::Block(format!(
                "{} blocks call {} of this run",
                self.probe.name, self.calls
            )));
        }
        Ok(Decision::Allow)
    }

    async fn after_tool(
        &mut self,
        _call: &ToolCall,
        output: &mut ToolOutput,
        _ctx: &PluginCtx,
    ) {
        if let Some(InputBlock::Text(text)) = output.content.first_mut() {
            text.text.push_str(&format!(" [{}]", self.probe.name));
        }
    }

    async fn before_stop(
        &mut self,
        _message: &AssistantMessage,
        _ctx: &PluginCtx,
    ) -> anyhow::Result<StopDecision> {
        if self.continued < self.probe.continue_times {
            self.continued += 1;
            return Ok(StopDecision::Continue(format!(
                "{} continue {}",
                self.probe.name, self.continued
            )));
        }
        Ok(StopDecision::Stop)
    }

    async fn finish(&mut self, run: &FinishedRun<'_>, _ctx: &PluginCtx) {
        self.probe.finished.lock().unwrap().push((
            run.transcript.len(),
            run.stop.clone(),
            run.text.to_owned(),
        ));
    }
}

/// Echoes its `text` argument.
struct Echo {
    schema: Value,
}

impl Echo {
    fn new() -> Self {
        Self {
            schema: json!({
                "type": "object",
                "properties": {"text": {"type": "string"}},
                "required": ["text"],
            }),
        }
    }
}

#[async_trait]
impl AgentTool for Echo {
    fn name(&self) -> &str {
        "echo"
    }
    fn description(&self) -> &str {
        "Echoes its text."
    }
    fn parameters(&self) -> &Value {
        &self.schema
    }
    async fn call(
        &self,
        args: Value,
        _ctx: ToolCtx,
    ) -> anyhow::Result<ToolOutput> {
        Ok(ToolOutput::text(args["text"].as_str().unwrap_or_default()))
    }
}

fn user_texts(message: &Message) -> Vec<String> {
    match message {
        Message::User(user) => match &user.content {
            UserContent::Text(text) => vec![text.clone()],
            UserContent::Blocks(blocks) => blocks
                .iter()
                .map(|block| match block {
                    InputBlock::Text(text) => text.text.clone(),
                    InputBlock::Image(_) => "<image>".into(),
                })
                .collect(),
        },
        other => panic!("expected a user message, got {other:?}"),
    }
}

fn tool_result_text(message: &Message) -> String {
    match message {
        Message::ToolResult(result) => match &result.content[0] {
            InputBlock::Text(text) => text.text.clone(),
            InputBlock::Image(_) => "<image>".into(),
        },
        other => panic!("expected a tool result, got {other:?}"),
    }
}

/// `start` shapes the run's settings and first message before the session
/// opens. Plugins start in registration order, and each sees what the
/// ones before it set.
#[test]
fn start_shapes_the_run() {
    block_on(async {
        let model = ScriptedModel::new().turn(|t| t.text("done"));
        let first = Probe {
            plan: Some(Arc::new(|plan: &mut RunPlan| {
                plan.instructions = Some("Be brief.".into());
                plan.context.push("note: the sky is blue".into());
            })),
            ..Probe::named("first")
        };
        let second = Probe {
            plan: Some(Arc::new(|plan: &mut RunPlan| {
                plan.reasoning = Some(ReasoningEffort::Low);
                plan.context.push("note: water is wet".into());
                plan.input = format!("{} (rewritten)", plan.input);
            })),
            ..Probe::named("second")
        };
        let agent = Agent::new(model.clone())
            .instructions("Be thorough.")
            .plugin(first.clone())
            .plugin(second.clone());
        let store = Store::memory().await.unwrap();
        agent.run("what color?", &store).await.unwrap();

        assert_eq!(
            first.log(),
            ["first start: instructions Some(\"Be thorough.\")"]
        );
        assert_eq!(
            second.log(),
            ["second start: instructions Some(\"Be brief.\")"]
        );
        let requests = model.requests();
        let settings = &requests[0].settings;
        assert_eq!(settings.instructions.as_deref(), Some("Be brief."));
        assert_eq!(settings.reasoning, Some(ReasoningEffort::Low));
        assert_eq!(
            user_texts(&requests[0].transcript[0]),
            [
                "note: the sky is blue",
                "note: water is wet",
                "what color? (rewritten)"
            ]
        );
    });
}

/// A plugin that fails to start fails the run: the model is never asked,
/// and the store records the run as failed.
#[test]
fn a_failed_start_fails_the_run() {
    block_on(async {
        let model = ScriptedModel::new();
        let broken = Probe {
            fail_start: Some("no key"),
            ..Probe::named("broken")
        };
        let later = Probe::named("later");
        let agent = Agent::new(model.clone())
            .plugin(broken)
            .plugin(later.clone());
        let store = Store::memory().await.unwrap();
        let run = agent.start("hi", &store);
        let id = run.id();
        let error = run.outcome().await.unwrap_err();
        let AgentError::Plugin { plugin, message } = &error else {
            panic!("{error:?}");
        };
        assert_eq!((plugin.as_str(), message.as_str()), ("broken", "no key"));
        assert!(later.log().is_empty(), "later plugins do not start");
        assert!(model.requests().is_empty());
        let record = store.run(&id.0).await.unwrap().unwrap();
        assert_eq!(record.status, Status::Failed);
        assert_eq!(record.error.as_deref(), Some(error.to_string().as_str()));
    });
}

/// A plugin's tools join the agent's, and its tool hooks see only its own
/// run's calls: two concurrent runs each count from one.
#[test]
fn tools_and_tool_hooks_are_per_run() {
    block_on(async {
        let script = || {
            ScriptedModel::new()
                .turn(|t| {
                    t.tool_call("echo", json!({"text": "hello"}))
                        .tool_call("echo", json!({"text": "forbidden"}))
                })
                .turn(|t| t.text("done"))
        };
        let probe = Probe {
            tools: vec![Arc::new(Echo::new())],
            ..Probe::named("guard")
        };
        let store = Store::memory().await.unwrap();
        let (a, b) = (script(), script());
        let run_a = Agent::new(a.clone())
            .plugin(probe.clone())
            .start("a", &store);
        let run_b = Agent::new(b.clone()).plugin(probe).start("b", &store);
        let (_, _) = (
            run_a.outcome().await.unwrap(),
            run_b.outcome().await.unwrap(),
        );
        for model in [a, b] {
            let transcript = &model.requests()[1].transcript;
            assert_eq!(tool_result_text(&transcript[2]), "hello [guard]");
            // A blocked call never ran, so `after_tool` never saw it.
            assert_eq!(
                tool_result_text(&transcript[3]),
                "guard blocks call 2 of this run"
            );
        }
    });
}

/// `before_stop` keeps a run going, one turn per continuation, until the
/// plugin lets it stop or the run reaches `max_continuations`. Each
/// continuation is a user message and a `Continued` event.
#[hegel::test(test_cases = 60)]
fn continuations_are_capped(tc: TestCase) {
    let wanted = tc.draw(gs::integers::<u32>().max_value(5));
    let cap = tc.draw(gs::integers::<u32>().max_value(4));
    let turns = wanted.min(cap) + 1;
    block_on(async {
        let mut model = ScriptedModel::new();
        for n in 0..turns {
            model = model.turn(|t| t.text(format!("answer {n}")));
        }
        let probe = Probe {
            continue_times: wanted,
            ..Probe::named("reviewer")
        };
        let agent = Agent::new(model.clone())
            .limits(Limits::default().max_continuations(cap))
            .plugin(probe.clone());
        let store = Store::memory().await.unwrap();
        let mut run = agent.start("go", &store);
        let mut events = Vec::new();
        {
            use futures_util::StreamExt;
            let mut stream = run.events();
            while let Some(event) = stream.next().await {
                events.push(event);
            }
        }
        let outcome = run.outcome().await.unwrap();
        model.assert_exhausted();
        assert_grammar(&events);
        assert_eq!(outcome.stop, StopReason::Stop);
        assert_eq!(outcome.text, format!("answer {}", turns - 1));

        let continued: Vec<String> = events
            .iter()
            .filter_map(|e| match e {
                RunEvent::Continued {
                    message, plugin, ..
                } => {
                    assert_eq!(&**plugin, "reviewer");
                    Some(message.clone())
                }
                _ => None,
            })
            .collect();
        let expected: Vec<String> = (1..turns)
            .map(|n| format!("reviewer continue {n}"))
            .collect();
        assert_eq!(continued, expected);

        let transcript = &model.requests()[turns as usize - 1].transcript;
        for n in 1..turns as usize {
            assert_eq!(
                user_texts(&transcript[2 * n]),
                [format!("reviewer continue {n}")]
            );
        }
        let finished = probe.finished.lock().unwrap().clone();
        assert_eq!(
            finished,
            [(2 * turns as usize, StopReason::Stop, outcome.text.clone())]
        );
    });
}

/// What a plugin charges counts toward the run's usage, its limits, and
/// its stored cost.
#[test]
fn charged_usage_counts() {
    block_on(async {
        let charge = Usage {
            input: 1000,
            cost: UsageCost {
                input: 0.75,
                total: 0.75,
                ..UsageCost::default()
            },
            ..Usage::default()
        };
        let model = ScriptedModel::new()
            .turn(|t| t.tool_call("echo", json!({"text": "x"})).cost(0.5))
            .turn(|t| t.text("done"));
        let probe = Probe {
            charge: Some(charge),
            tools: vec![Arc::new(Echo::new())],
            ..Probe::named("judge")
        };
        let agent = Agent::new(model.clone())
            .limits(Limits::default().max_usd(1.0))
            .plugin(probe);
        let store = Store::memory().await.unwrap();
        let outcome = agent.run("go", &store).await.unwrap();
        assert_eq!(outcome.stop, StopReason::Limit(LimitKind::Usd));
        assert_eq!(outcome.usage.cost.total, 1.25);
        assert!(outcome.usage.input >= 1000);
        assert_eq!(model.requests().len(), 1, "the limit ends the run");
        let record = store.run(&outcome.run.0).await.unwrap().unwrap();
        assert_eq!(record.cost_usd, 1.25);
    });
}

/// Records a plugin stores go with the run: a fork's `start` gets its
/// ancestors' records, in order, and a root run gets none. The model
/// never sees them.
#[test]
fn records_reach_forks() {
    block_on(async {
        let model = ScriptedModel::new()
            .turn(|t| t.text("one"))
            .turn(|t| t.text("two"))
            .turn(|t| t.text("three"));
        let probe = Probe {
            record: Some(json!({"ledger": 1})),
            ..Probe::named("ledger")
        };
        let agent = Agent::new(model.clone()).plugin(probe.clone());
        let store = Store::memory().await.unwrap();
        let root = agent.run("first", &store).await.unwrap();
        let fork = agent
            .fork(&root.checkpoint())
            .run("second", &store)
            .await
            .unwrap();
        agent
            .fork(&fork.checkpoint())
            .run("third", &store)
            .await
            .unwrap();

        let seen = probe.records_seen.lock().unwrap().clone();
        assert_eq!(
            seen,
            [
                vec![],
                vec![json!({"ledger": 1})],
                vec![json!({"ledger": 1}), json!({"ledger": 1})],
            ]
        );
        for request in model.requests() {
            assert!(
                request
                    .transcript
                    .iter()
                    .all(|m| !format!("{m:?}").contains("ledger")),
                "records never reach the model"
            );
        }
    });
}

/// A plugin that fails in `before_stop` does not keep the run going: the
/// failure is reported as an event, and the run stops and finishes.
#[test]
fn a_failing_before_stop_is_reported_and_stops() {
    struct Failing;

    #[async_trait]
    impl Plugin for Failing {
        fn name(&self) -> &str {
            "failing"
        }

        async fn start(
            &self,
            _plan: &mut RunPlan,
            _ctx: &PluginCtx,
        ) -> anyhow::Result<Box<dyn PluginRun>> {
            Ok(Box::new(FailingRun))
        }
    }

    struct FailingRun;

    #[async_trait]
    impl PluginRun for FailingRun {
        async fn before_stop(
            &mut self,
            _message: &AssistantMessage,
            _ctx: &PluginCtx,
        ) -> anyhow::Result<StopDecision> {
            anyhow::bail!("judge unreachable")
        }
    }

    block_on(async {
        let model = ScriptedModel::new().turn(|t| t.text("done"));
        let later = Probe::named("later");
        let agent = Agent::new(model.clone())
            .plugin(Failing)
            .plugin(later.clone());
        let store = Store::memory().await.unwrap();
        let mut run = agent.start("go", &store);
        let mut events = Vec::new();
        {
            use futures_util::StreamExt;
            let mut stream = run.events();
            while let Some(event) = stream.next().await {
                events.push(event);
            }
        }
        let outcome = run.outcome().await.unwrap();
        assert_eq!(outcome.stop, StopReason::Stop);
        assert_grammar(&events);
        let errors: Vec<(&str, &str)> = events
            .iter()
            .filter_map(|e| match e {
                RunEvent::PluginError {
                    plugin, message, ..
                } => Some((&**plugin, message.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(errors, [("failing", "judge unreachable")]);
        assert_eq!(later.finished.lock().unwrap().len(), 1);
    });
}
