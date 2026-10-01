//! Plugins (`docs/reference/plugins.md`), driven through `Agent` with
//! `ScriptedModel` and `Store::memory()`.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
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
use tau_agent::error::{PluginError, ToolError};

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
    /// Publishes these in `start`, in order.
    publish: Vec<Value>,
    tools: Vec<Arc<dyn AgentTool>>,
    log: Arc<Mutex<Vec<String>>>,
    finished: Arc<Mutex<Vec<(usize, StopReason, String)>>>,
    records_seen: Arc<Mutex<Vec<Vec<Value>>>>,
    /// What `start` saw of the plan and its context.
    seen: Arc<Mutex<Vec<String>>>,
    /// Charges this usage in `finish`.
    charge_at_finish: Option<Usage>,
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
    ) -> Result<Box<dyn PluginRun>, PluginError> {
        self.log.lock().unwrap().push(format!(
            "{} start: instructions {:?}",
            self.name, plan.instructions
        ));
        self.seen.lock().unwrap().push(format!(
            "model {} kind {:?} workflow {:?} now {} ctx {ctx:?}",
            plan.model(),
            plan.kind(),
            plan.workflow(),
            ctx.now()
        ));
        self.records_seen
            .lock()
            .unwrap()
            .push(plan.records().to_vec());
        if let Some(message) = self.fail_start {
            return Err(message.into());
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
        for body in &self.publish {
            ctx.publish(body).await?;
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
    ) -> Result<Decision, PluginError> {
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
    ) -> Result<StopDecision, PluginError> {
        if self.continued < self.probe.continue_times {
            self.continued += 1;
            return Ok(StopDecision::Continue(format!(
                "{} continue {}",
                self.probe.name, self.continued
            )));
        }
        Ok(StopDecision::Stop)
    }

    async fn finish(&mut self, run: &FinishedRun<'_>, ctx: &PluginCtx) {
        if let Some(usage) = &self.probe.charge_at_finish {
            ctx.charge(usage);
        }
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
    ) -> Result<ToolOutput, ToolError> {
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
            .clock(Arc::new(|| 42))
            .model("gpt-5.4-mini")
            .instructions("Be thorough.")
            .plugin(first.clone())
            .plugin(second.clone());
        let store = Store::memory().await.unwrap();
        let outcome = agent
            .run(
                tau_agent::agent::Input::new("what color?").workflow("w1"),
                &store,
            )
            .await
            .unwrap();
        let seen = first.seen.lock().unwrap().clone();
        assert_eq!(
            seen,
            [format!(
                "model gpt-5.4-mini kind Root workflow Some(\"w1\") now 42 ctx \
                 PluginCtx {{ run: {:?}, plugin: \"first\", .. }}",
                outcome.run
            )]
        );

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
/// continuation is a user message and a `Continued` event. Checked for
/// every pair of wanted continuations (0–5) and cap (0–4): the whole
/// domain is 30 runs, fewer than a property would draw.
#[test]
fn continuations_are_capped() {
    for wanted in 0..=5 {
        for cap in 0..=4 {
            continuations_are_capped_at(wanted, cap);
        }
    }
}

fn continuations_are_capped_at(wanted: u32, cap: u32) {
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

/// Each plugin's charges are stored as its own cost, adding up, and
/// within the run's total. Those made while the run goes come to the
/// subscriber as `PluginCharged` after `RunStart`, one per charge; those
/// made while it finishes, after `RunEnd`, are only stored.
#[hegel::test(test_cases = 40)]
fn charges_are_kept_per_plugin(tc: hegel::TestCase) {
    use hegel::generators::{self as gs, Generator as _};
    let names = tc.draw(gs::subsequences(vec!["judge", "pruner", "goal"]));
    // Costs are multiples of 1/1024, so sums are exact.
    let cost = || {
        gs::integers::<u32>()
            .max_value(1024)
            .map(|n| f64::from(n) / 1024.0)
    };
    let charge = |input: u64, total: f64| Usage {
        input,
        output: input / 2,
        cost: UsageCost {
            total,
            ..UsageCost::default()
        },
        ..Usage::default()
    };
    let mut probes = Vec::new();
    for name in names {
        let at_start = tc.draw(gs::optional(hegel::tuples!(
            gs::integers::<u64>().max_value(10_000),
            cost(),
        )));
        let at_finish = tc.draw(gs::optional(hegel::tuples!(
            gs::integers::<u64>().max_value(10_000),
            cost(),
        )));
        probes.push(Probe {
            charge: at_start.map(|(input, usd)| charge(input, usd)),
            charge_at_finish: at_finish.map(|(input, usd)| charge(input, usd)),
            ..Probe::named(name)
        });
    }
    block_on(async {
        let model = ScriptedModel::new().turn(|t| t.text("done").cost(0.5));
        let mut agent = Agent::new(model);
        for probe in &probes {
            agent = agent.plugin(probe.clone());
        }
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
        assert_grammar(&events);
        let reported: Vec<(&str, f64)> = events
            .iter()
            .filter_map(|event| match event {
                RunEvent::PluginCharged { plugin, usage, .. } => {
                    Some((&**plugin, usage.cost.total))
                }
                _ => None,
            })
            .collect();
        let started: Vec<(&str, f64)> = probes
            .iter()
            .filter_map(|probe| {
                Some((probe.name, probe.charge.as_ref()?.cost.total))
            })
            .collect();
        assert_eq!(reported, started, "one event per charge, in order");

        let mut costs = store.plugin_costs(&outcome.run.0).await.unwrap();
        costs.sort_by(|a, b| a.plugin.cmp(&b.plugin));
        let mut expected: Vec<(String, i64, i64, f64)> = probes
            .iter()
            .filter(|probe| {
                probe.charge.is_some() || probe.charge_at_finish.is_some()
            })
            .map(|probe| {
                let mut all = Usage::default();
                for usage in [&probe.charge, &probe.charge_at_finish]
                    .into_iter()
                    .flatten()
                {
                    all += usage;
                }
                (
                    probe.name.to_owned(),
                    all.input as i64,
                    all.output as i64,
                    all.cost.total,
                )
            })
            .collect();
        expected.sort_by(|a, b| a.0.cmp(&b.0));
        let stored: Vec<(String, i64, i64, f64)> = costs
            .into_iter()
            .map(|cost| {
                (
                    cost.plugin,
                    cost.input_tokens,
                    cost.output_tokens,
                    cost.cost_usd,
                )
            })
            .collect();
        assert_eq!(stored, expected);

        let charged: f64 = expected.iter().map(|line| line.3).sum();
        let record = store.run(&outcome.run.0).await.unwrap().unwrap();
        assert_eq!(record.cost_usd, 0.5 + charged, "within the run's total");
        assert_eq!(outcome.usage.cost.total, 0.5 + charged);
    });
}

/// What a plugin publishes is reported and recorded alike: the
/// subscriber sees each body, after `RunStart`, and the store has the
/// same bodies in the same order.
#[hegel::test(test_cases = 30)]
fn published_bodies_are_reported_and_recorded(tc: hegel::TestCase) {
    use hegel::generators as gs;
    let bodies: Vec<Value> = tc
        .draw(gs::vecs(gs::integers::<u32>()).max_size(5))
        .into_iter()
        .map(|n| json!({ "n": n }))
        .collect();
    block_on(async {
        let model = ScriptedModel::new().turn(|t| t.text("done"));
        let probe = Probe {
            publish: bodies.clone(),
            ..Probe::named("publisher")
        };
        let store = Store::memory().await.unwrap();
        let mut run = Agent::new(model).plugin(probe).start("go", &store);
        let mut events = Vec::new();
        {
            use futures_util::StreamExt;
            let mut stream = run.events();
            while let Some(event) = stream.next().await {
                events.push(event);
            }
        }
        let outcome = run.outcome().await.unwrap();
        assert_grammar(&events);
        let reported: Vec<Value> = events
            .into_iter()
            .filter_map(|event| match event {
                RunEvent::PluginReport { body, .. } => Some(body),
                _ => None,
            })
            .collect();
        assert_eq!(reported, bodies);
        let recorded: Vec<Value> = store
            .records(&outcome.run.0, "publisher")
            .await
            .unwrap()
            .iter()
            .map(|body| serde_json::from_str(body).unwrap())
            .collect();
        assert_eq!(recorded, bodies);
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
        ) -> Result<Box<dyn PluginRun>, PluginError> {
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
        ) -> Result<StopDecision, PluginError> {
            return Err("judge unreachable".into());
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

/// How a [`Pruner`] rewrites the context it is offered.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Prune {
    /// Drops everything before the last user message.
    KeepLastUser,
    /// Drops the last message, which the loop must reject.
    DropLast,
    /// Declines, as a pruner that cannot free enough would.
    Decline,
}

/// A context plugin that prunes when offered the context: at turn
/// boundaries, overflows, or both.
#[derive(Clone)]
struct Pruner {
    name: &'static str,
    how: Prune,
    /// Rewrites only on this trigger; `None` for both.
    on: Option<tau_agent::plugin::Trigger>,
    offered: Arc<Mutex<Vec<(tau_agent::plugin::Trigger, usize)>>>,
    resumed: Arc<Mutex<Vec<Option<Value>>>>,
}

impl Pruner {
    fn new(how: Prune) -> Self {
        Self {
            name: "pruner",
            how,
            on: None,
            offered: Arc::default(),
            resumed: Arc::default(),
        }
    }
}

#[async_trait]
impl Plugin for Pruner {
    fn name(&self) -> &str {
        self.name
    }

    async fn start(
        &self,
        plan: &mut RunPlan,
        _ctx: &PluginCtx,
    ) -> Result<Box<dyn PluginRun>, PluginError> {
        self.resumed
            .lock()
            .unwrap()
            .push(plan.last_rewrite().cloned());
        Ok(Box::new(self.clone()))
    }
}

#[async_trait]
impl PluginRun for Pruner {
    async fn rewrite_context(
        &mut self,
        view: &tau_agent::plugin::ContextView<'_>,
        _ctx: &PluginCtx,
    ) -> Result<Option<tau_agent::plugin::Rewrite>, PluginError> {
        self.offered
            .lock()
            .unwrap()
            .push((view.trigger, view.transcript.len()));
        if self.on.is_some_and(|on| on != view.trigger) {
            return Ok(None);
        }
        let messages = match self.how {
            Prune::Decline => return Ok(None),
            Prune::KeepLastUser => {
                let from = view
                    .transcript
                    .iter()
                    .rposition(|m| matches!(m, Message::User(_)))
                    .unwrap();
                if from == 0 {
                    return Ok(None);
                }
                view.transcript[from..].to_vec()
            }
            Prune::DropLast => {
                view.transcript[..view.transcript.len() - 1].to_vec()
            }
        };
        Ok(Some(tau_agent::plugin::Rewrite {
            messages,
            details: json!({"pruned_at": view.transcript.len()}),
        }))
    }
}

/// Collects a run's events and outcome.
async fn run_to_end(
    agent: &Agent,
    store: &Store,
    input: &str,
    steer: Option<&str>,
) -> (Vec<RunEvent>, tau_agent::agent::Outcome) {
    use futures_util::StreamExt;
    let mut run = agent.start(input, store);
    if let Some(steer) = steer {
        run.steer(steer);
    }
    let events = run.events().collect().await;
    (events, run.outcome().await.unwrap())
}

/// A rewrite between turns replaces the working transcript: the next
/// request sends it, the store keeps a context entry naming the plugin
/// followed by the new messages, an event reports it, and a fork starts
/// from it, with the plugin's details handed back.
#[test]
fn a_rewrite_replaces_the_transcript() {
    block_on(async {
        let model = ScriptedModel::new()
            .turn(|t| t.tool_call("echo", json!({"text": "x"})))
            .turn(|t| t.text("done"))
            .turn(|t| t.text("forked"));
        // Only at turn ends: the fork below keeps what it inherited.
        let pruner = Pruner {
            on: Some(tau_agent::plugin::Trigger::TurnEnd),
            ..Pruner::new(Prune::KeepLastUser)
        };
        let agent = Agent::new(model.clone())
            .tool(Echo::new())
            .plugin(pruner.clone());
        let store = Store::memory().await.unwrap();
        let (events, outcome) =
            run_to_end(&agent, &store, "go", Some("then this")).await;
        assert_grammar(&events);
        assert_eq!(outcome.text, "done");

        let requests = model.requests();
        assert_eq!(requests[1].transcript.len(), 1);
        assert_eq!(user_texts(&requests[1].transcript[0]), ["then this"]);
        let rewritten: Vec<(&str, u64, u64)> = events
            .iter()
            .filter_map(|e| match e {
                RunEvent::ContextRewritten {
                    plugin,
                    tokens_before,
                    tokens_after,
                    ..
                } => Some((&**plugin, *tokens_before, *tokens_after)),
                _ => None,
            })
            .collect();
        assert_eq!(rewritten.len(), 1);
        assert_eq!(rewritten[0].0, "pruner");
        assert!(rewritten[0].2 < rewritten[0].1, "{rewritten:?}");

        let entries = store.transcript(&outcome.run.0).await.unwrap();
        let tau_store::Entry::Context { plugin, body } = &entries[0] else {
            panic!("{entries:?}");
        };
        assert_eq!(plugin, "pruner");
        assert_eq!(body, &json!({"pruned_at": 4}).to_string());
        assert_eq!(entries.len(), 3, "context, kept message, answer");

        let fork = agent
            .fork(&outcome.checkpoint())
            .run("and now", &store)
            .await
            .unwrap();
        assert_eq!(fork.text, "forked");
        assert_eq!(model.requests()[2].transcript.len(), 3);
        assert_eq!(
            pruner.resumed.lock().unwrap().clone(),
            [None, Some(json!({"pruned_at": 4}))]
        );
    });
}

/// A rewrite that drops the message the next request answers is
/// rejected: the loop reports it and goes on with the transcript as it
/// was, storing no context entry.
#[test]
fn a_bad_rewrite_is_rejected() {
    block_on(async {
        let model = ScriptedModel::new()
            .turn(|t| t.tool_call("echo", json!({"text": "x"})))
            .turn(|t| t.text("done"));
        let agent = Agent::new(model.clone())
            .tool(Echo::new())
            .plugin(Pruner::new(Prune::DropLast));
        let store = Store::memory().await.unwrap();
        let (events, outcome) = run_to_end(&agent, &store, "go", None).await;
        assert_eq!(outcome.text, "done");
        assert_eq!(model.requests()[1].transcript.len(), 3);
        let errors: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                RunEvent::PluginError { message, .. } => Some(message.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            errors,
            [
                "rejected rewrite: it does not end with the transcript's last message"
            ]
        );
        let entries = store.transcript(&outcome.run.0).await.unwrap();
        assert!(
            entries
                .iter()
                .all(|e| matches!(e, tau_store::Entry::Message { .. }))
        );
    });
}

/// On an overflow, plugins are offered the context in order until one
/// rewrites it: the first rewrite wins, later plugins are not offered
/// that overflow, and the turn is retried once on the new transcript.
#[test]
fn the_first_rewrite_takes_an_overflow() {
    use tau_agent::plugin::Trigger;
    block_on(async {
        let model = ScriptedModel::new()
            .turn(|t| t.tool_call("echo", json!({"text": "x"})))
            .turn(|t| t.error("context_length_exceeded", "too long"))
            .turn(|t| t.text("done"));
        let declines = Pruner {
            name: "declines",
            ..Pruner::new(Prune::Decline)
        };
        let prunes = Pruner {
            name: "prunes",
            on: Some(Trigger::Overflow),
            ..Pruner::new(Prune::KeepLastUser)
        };
        let later = Pruner {
            name: "later",
            on: Some(Trigger::Overflow),
            ..Pruner::new(Prune::KeepLastUser)
        };
        let agent = Agent::new(model.clone())
            .tool(Echo::new())
            .plugin(declines.clone())
            .plugin(prunes.clone())
            .plugin(later.clone());
        let store = Store::memory().await.unwrap();
        let (events, outcome) =
            run_to_end(&agent, &store, "go", Some("then this")).await;
        assert_eq!(outcome.text, "done");
        model.assert_exhausted();
        assert!(
            declines
                .offered
                .lock()
                .unwrap()
                .contains(&(Trigger::Overflow, 4))
        );
        // Every plugin is offered each turn end; only the first to
        // rewrite takes the overflow.
        assert_eq!(
            prunes.offered.lock().unwrap().clone(),
            [(Trigger::TurnEnd, 4), (Trigger::Overflow, 4)]
        );
        assert_eq!(
            later.offered.lock().unwrap().clone(),
            [(Trigger::TurnEnd, 4)]
        );
        let by: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                RunEvent::ContextRewritten { plugin, .. } => Some(&**plugin),
                _ => None,
            })
            .collect();
        assert_eq!(by, ["prunes"]);
        assert_eq!(
            user_texts(&model.requests()[2].transcript[0]),
            ["then this"]
        );
    });
}

/// A run that starts on an inherited transcript offers it for a rewrite
/// before its first request, so it can fit the run's model; a run that
/// starts fresh has nothing to offer.
#[test]
fn a_fork_is_offered_its_context_before_it_asks() {
    use tau_agent::plugin::Trigger;
    block_on(async {
        let model = ScriptedModel::new()
            .turn(|t| t.text("done"))
            .turn(|t| t.text("forked"));
        let pruner = Pruner {
            on: Some(Trigger::Start),
            ..Pruner::new(Prune::KeepLastUser)
        };
        let agent = Agent::new(model.clone()).plugin(pruner.clone());
        let store = Store::memory().await.unwrap();
        let (_, outcome) = run_to_end(&agent, &store, "go", None).await;
        assert_eq!(pruner.offered.lock().unwrap().clone(), []);

        let fork = agent
            .fork(&outcome.checkpoint())
            .run("and now", &store)
            .await
            .unwrap();
        assert_eq!(fork.text, "forked");
        // The inherited "go" and "done", then "and now": offered at
        // the start, and cut to the last user message before it asked.
        assert_eq!(
            pruner.offered.lock().unwrap().clone(),
            [(Trigger::Start, 3)]
        );
        assert_eq!(model.requests()[1].transcript.len(), 1);
        assert_eq!(user_texts(&model.requests()[1].transcript[0]), ["and now"]);
    });
}

/// What a plugin charges while the run finishes still counts: in the
/// outcome and in the stored cost.
#[test]
fn usage_charged_at_finish_is_stored() {
    block_on(async {
        let model = ScriptedModel::new().turn(|t| t.text("done").cost(0.5));
        let probe = Probe {
            charge_at_finish: Some(Usage {
                cost: UsageCost {
                    total: 0.25,
                    ..UsageCost::default()
                },
                ..Usage::default()
            }),
            ..Probe::named("distiller")
        };
        let agent = Agent::new(model).plugin(probe);
        let store = Store::memory().await.unwrap();
        let outcome = agent.run("go", &store).await.unwrap();
        assert_eq!(outcome.usage.cost.total, 0.75);
        let record = store.run(&outcome.run.0).await.unwrap().unwrap();
        assert_eq!(record.cost_usd, 0.75);
    });
}

/// A hook's `after_tool` still changes the output the model sees: hooks
/// run through the plugin seams.
#[test]
fn a_hook_changes_tool_output() {
    struct Tag;

    #[async_trait]
    impl tau_agent::hook::RunHook for Tag {
        async fn after_tool(
            &self,
            _call: &ToolCall,
            output: &mut ToolOutput,
            _ctx: &tau_agent::hook::HookCtx,
        ) {
            *output = ToolOutput::text("tagged");
        }
    }

    block_on(async {
        let model = ScriptedModel::new()
            .turn(|t| t.tool_call("echo", json!({"text": "x"})))
            .turn(|t| t.text("done"));
        let agent = Agent::new(model.clone()).tool(Echo::new()).hook(Tag);
        let store = Store::memory().await.unwrap();
        agent.run("go", &store).await.unwrap();
        assert_eq!(
            tool_result_text(&model.requests()[1].transcript[2]),
            "tagged"
        );
    });
}

/// Watches rewrites: records the length of each transcript a rewrite
/// replaced, and of the rewrite.
#[derive(Clone, Default)]
struct Watcher {
    seen: Arc<Mutex<Vec<(usize, usize)>>>,
}

#[async_trait]
impl Plugin for Watcher {
    fn name(&self) -> &str {
        "watcher"
    }

    async fn start(
        &self,
        _plan: &mut RunPlan,
        _ctx: &PluginCtx,
    ) -> Result<Box<dyn PluginRun>, PluginError> {
        Ok(Box::new(self.clone()))
    }
}

#[async_trait]
impl PluginRun for Watcher {
    async fn rewritten(
        &mut self,
        replaced: &[Message],
        rewrite: &tau_agent::plugin::Rewrite,
        _ctx: &PluginCtx,
    ) -> Result<(), PluginError> {
        self.seen
            .lock()
            .unwrap()
            .push((replaced.len(), rewrite.messages.len()));
        Ok(())
    }
}

/// Every plugin, the rewriter included, is handed the transcript a
/// rewrite replaced, whole, before the next request goes out.
#[test]
fn plugins_see_what_a_rewrite_replaced() {
    block_on(async {
        let model = ScriptedModel::new()
            .turn(|t| t.tool_call("echo", json!({"text": "x"})))
            .turn(|t| t.text("done"));
        let watcher = Watcher::default();
        let agent = Agent::new(model.clone())
            .tool(Echo::new())
            .plugin(Pruner::new(Prune::KeepLastUser))
            .plugin(watcher.clone());
        let store = Store::memory().await.unwrap();
        let (events, _) =
            run_to_end(&agent, &store, "go", Some("then this")).await;
        assert_grammar(&events);
        // The four messages before the rewrite, and the one it kept.
        assert_eq!(watcher.seen.lock().unwrap().clone(), [(4, 1)]);
        assert_eq!(model.requests()[1].transcript.len(), 1);
    });
}

/// What a [`Picker`] was offered: the turn, the model, the effort and the
/// transcript's length.
type Offer = (u32, String, Option<ReasoningEffort>, usize);

/// Picks each turn's effort from a script, and logs what it was offered.
#[derive(Clone)]
struct Picker {
    name: &'static str,
    /// The effort to pick at each turn, from the first; `None` picks
    /// nothing, and so does a turn past the end.
    picks: Vec<Option<ReasoningEffort>>,
    /// Fails every call instead.
    fails: bool,
    offered: Arc<Mutex<Vec<Offer>>>,
}

impl Picker {
    fn new(name: &'static str, picks: Vec<Option<ReasoningEffort>>) -> Self {
        Self {
            name,
            picks,
            fails: false,
            offered: Arc::default(),
        }
    }
}

#[async_trait]
impl Plugin for Picker {
    fn name(&self) -> &str {
        self.name
    }

    fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        vec![Arc::new(Echo::new())]
    }

    async fn start(
        &self,
        _plan: &mut RunPlan,
        _ctx: &PluginCtx,
    ) -> Result<Box<dyn PluginRun>, PluginError> {
        Ok(Box::new(self.clone()))
    }
}

#[async_trait]
impl PluginRun for Picker {
    async fn before_request(
        &mut self,
        view: &tau_agent::plugin::RequestView<'_>,
        _ctx: &PluginCtx,
    ) -> Result<Option<ReasoningEffort>, PluginError> {
        if self.fails {
            return Err("classifier unreachable".into());
        }
        self.offered.lock().unwrap().push((
            view.turn,
            view.model.to_owned(),
            view.effort,
            view.transcript.len(),
        ));
        Ok(self.picks.get(view.turn as usize - 1).copied().flatten())
    }
}

/// `before_request` sets the effort of each turn's request, and it holds
/// for the turns after it until a plugin picks another. A failing plugin
/// is reported and picks nothing; the plugins after it are still asked.
#[test]
fn before_request_picks_each_turns_effort() {
    use ReasoningEffort::{High, Low, Medium};
    block_on(async {
        let model = ScriptedModel::new()
            .turn(|t| t.tool_call("echo", json!({"text": "a"})))
            .turn(|t| t.tool_call("echo", json!({"text": "b"})))
            .turn(|t| t.text("done"));
        let broken = Picker {
            fails: true,
            ..Picker::new("broken", vec![])
        };
        let picker = Picker::new("picker", vec![Some(High), None, Some(Low)]);
        let agent = Agent::new(model.clone())
            .model("gpt-6-sol")
            .reasoning(Medium)
            .plugin(broken)
            .plugin(picker.clone());
        let store = Store::memory().await.unwrap();
        let (events, outcome) = run_to_end(&agent, &store, "go", None).await;
        assert_eq!(outcome.stop, StopReason::Stop);
        assert_grammar(&events);

        let efforts: Vec<_> = model
            .requests()
            .iter()
            .map(|request| request.settings.reasoning)
            .collect();
        assert_eq!(efforts, [Some(High), Some(High), Some(Low)]);
        let sol = || "gpt-6-sol".to_owned();
        assert_eq!(
            *picker.offered.lock().unwrap(),
            [
                (1, sol(), Some(Medium), 1),
                (2, sol(), Some(High), 3),
                (3, sol(), Some(High), 5),
            ]
        );
        let errors = events
            .iter()
            .filter(|e| {
                matches!(e, RunEvent::PluginError { plugin, message, .. }
                    if &**plugin == "broken" && message == "classifier unreachable")
            })
            .count();
        assert_eq!(errors, 3);
    });
}
