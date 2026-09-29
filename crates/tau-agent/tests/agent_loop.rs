//! The agent loop (`docs/reference/agent-loop.md`), driven through
//! `Agent` with `ScriptedModel`, `Store::memory()` and paused time.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_agent::{
    agent::Agent,
    event::{LimitKind, RunEvent, StopReason},
    hook::{Decision, HookCtx, RunHook, ToolCall},
    limits::Limits,
    runner::{CANCELLED, TRUNCATED},
    tool::{AgentTool, ExecutionMode, ToolCtx, ToolOutput},
};
use tau_ai::message::{
    AssistantBlock,
    InputBlock,
    Message,
    StopReason as MessageStop,
};
use tau_store::Store;
use tau_testing::{block_on, scripted::ScriptedModel};

mod common;
use common::{assert_grammar, stored};
use tau_agent::error::{PluginError, ToolError};
use tokio::time::Instant;

/// A tool that sleeps `ms` virtual milliseconds (stopping early on
/// cancel), optionally fails, and records when it ran.
struct Probe {
    name: &'static str,
    mode: ExecutionMode,
    schema: Value,
    log: Arc<Mutex<Vec<Interval>>>,
}

#[derive(Debug, Clone)]
struct Interval {
    start: Instant,
    end: Instant,
}

impl Probe {
    fn new(
        name: &'static str,
        mode: ExecutionMode,
        log: Arc<Mutex<Vec<Interval>>>,
    ) -> Self {
        Self {
            name,
            mode,
            schema: json!({
                "type": "object",
                "properties": {
                    "ms": {"type": "integer", "minimum": 0},
                    "fail": {"type": "boolean"}
                },
                "required": ["ms"]
            }),
            log,
        }
    }
}

#[async_trait]
impl AgentTool for Probe {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        "Sleeps, then answers."
    }
    fn parameters(&self) -> &Value {
        &self.schema
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
        ctx.updates.send(ToolOutput::text("working"));
        let ms = args["ms"].as_u64().unwrap_or(0);
        let cancelled = tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(ms)) => false,
            _ = ctx.cancel.cancelled() => true,
        };
        self.log.lock().unwrap().push(Interval {
            start,
            end: Instant::now(),
        });
        if cancelled {
            return Err("cancelled".into());
        }
        if args["fail"].as_bool() == Some(true) {
            return Err("probe failed".into());
        }
        Ok(ToolOutput::text(format!("slept {ms}")))
    }
}

/// Records every event, in order.
#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<RunEvent>>>);

#[async_trait]
impl RunHook for Recorder {
    async fn on_event(&self, event: &RunEvent) {
        self.0.lock().unwrap().push(event.clone());
    }
}

impl Recorder {
    fn events(&self) -> Vec<RunEvent> {
        self.0.lock().unwrap().clone()
    }
}

/// A counter clock, so timestamps are deterministic.
fn counter_clock() -> tau_agent::runner::Clock {
    let next = Arc::new(Mutex::new(0u64));
    Arc::new(move || {
        let mut n = next.lock().unwrap();
        *n += 1;
        *n
    })
}

/// Every tool call in the transcript has exactly one result, right after
/// its assistant message and in source order.
fn assert_results_follow_calls(transcript: &[Message]) {
    for (i, message) in transcript.iter().enumerate() {
        let Message::Assistant(assistant) = message else {
            continue;
        };
        let calls: Vec<&str> = assistant
            .content
            .iter()
            .filter_map(|b| match b {
                AssistantBlock::ToolCall(c) => Some(c.id.as_str()),
                _ => None,
            })
            .collect();
        let results: Vec<&str> = transcript[i + 1..]
            .iter()
            .take_while(|m| matches!(m, Message::ToolResult(_)))
            .map(|m| match m {
                Message::ToolResult(r) => r.tool_call_id.as_str(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(results, calls, "results after message {i}");
    }
}

#[derive(Debug, Clone, hegel::PrettyPrintable)]
struct ScriptedCall {
    tool: &'static str,
    ms: u64,
    fail: bool,
}

#[hegel::composite]
fn scripted_call(tc: &TestCase, sequential_allowed: bool) -> ScriptedCall {
    let mut tools = vec!["probe", "probe", "missing"];
    if sequential_allowed {
        tools.push("serial");
    }
    ScriptedCall {
        tool: tc.draw(gs::sampled_from(tools)),
        ms: tc.draw(gs::integers::<u64>().max_value(500)),
        fail: tc.draw(gs::booleans()),
    }
}

/// The loop over generated scripts: tool calls that sleep, fail, or name
/// a missing or sequential tool, and sometimes a cancel at a drawn
/// virtual time.
#[hegel::test(test_cases = 60)]
fn loop_over_generated_scripts(tc: TestCase) {
    loop_over_generated_scripts_body(tc)
}

/// [`loop_over_generated_scripts`] with more cases, for the nightly tier.
#[hegel::test(profile = "nightly_slow")]
#[ignore = "nightly"]
fn loop_over_generated_scripts_nightly(tc: TestCase) {
    loop_over_generated_scripts_body(tc)
}

fn loop_over_generated_scripts_body(tc: TestCase) {
    let turns: Vec<Vec<ScriptedCall>> = tc
        // A turn without tool calls ends the run, so every turn before the
        // final text turn calls at least one tool.
        .draw(
            gs::vecs(gs::vecs(scripted_call(true)).min_size(1).max_size(3))
                .max_size(3),
        );
    let cancel_at =
        tc.draw(gs::optional(gs::integers::<u64>().max_value(1500)));
    let usages: Vec<(u64, u64)> = (0..=turns.len())
        .map(|_| {
            (
                tc.draw(gs::integers::<u64>().max_value(10_000)),
                tc.draw(gs::integers::<u64>().max_value(10_000)),
            )
        })
        .collect();

    let mut llm = ScriptedModel::new();
    for (calls, usage) in turns.iter().zip(&usages) {
        let think = tc.draw(gs::booleans());
        llm = llm.turn(|mut t| {
            if think {
                t = t.thinking("weighing the options");
            }
            for call in calls {
                t = t.tool_call(
                    call.tool,
                    json!({"ms": call.ms, "fail": call.fail}),
                );
            }
            t.usage(usage.0, usage.1)
        });
    }
    let last = *usages.last().unwrap();
    llm = llm.turn(|t| t.text("done").usage(last.0, last.1));

    block_on(async {
        let store = Store::memory().await.unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let recorder = Recorder::default();
        let agent = Agent::new(llm.clone())
            .tool(Probe::new("probe", ExecutionMode::Parallel, log.clone()))
            .tool(Probe::new("serial", ExecutionMode::Sequential, log.clone()))
            .hook(recorder.clone())
            .clock(counter_clock());
        let run = agent.start("go", &store);
        let id = run.id();
        let started = Instant::now();
        // How many events the recorder held when the cancel fired: the
        // runtime runs one task at a time, so this places the cancel
        // exactly within the event sequence.
        let cancelled_at: Arc<Mutex<Option<usize>>> = Arc::default();
        if let Some(ms) = cancel_at {
            let control = run.control();
            let recorder = recorder.clone();
            let cancelled_at = cancelled_at.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(ms)).await;
                control.cancel();
                *cancelled_at.lock().unwrap() = Some(recorder.events().len());
            });
        }
        let outcome = run.outcome().await.unwrap();
        // A cancel that fires after the run ended changes nothing.
        let cancelled_at = *cancelled_at.lock().unwrap();
        let events = recorder.events();
        let transcript = stored(&store, &id.0).await;

        // The run checks for a cancel when a turn's response and tools are
        // done, just before its `TurnEnd`, and a cancel during a response
        // aborts it. So the run was cancelled exactly when the cancel came
        // before the last `TurnEnd`; after it, the run had already stopped.
        let last_turn_end = events
            .iter()
            .rposition(|e| matches!(e, RunEvent::TurnEnd { .. }))
            .expect("every run takes a turn");
        if cancelled_at.is_some_and(|at| at <= last_turn_end) {
            tc.event("cancel before end");
            assert_eq!(outcome.stop, StopReason::Cancelled);
        } else {
            if cancel_at.is_some() {
                tc.event("cancel after end");
            }
            assert_eq!(outcome.stop, StopReason::Stop);
            assert_eq!(outcome.text, "done");
        }
        let (mut input, mut output) = (0, 0);
        for message in &transcript {
            if let Message::Assistant(a) = message {
                input += a.usage.input;
                output += a.usage.output;
            }
        }
        assert_eq!(
            (outcome.usage.input, outcome.usage.output),
            (input, output)
        );
        if let (Some(at), Some(ms)) = (cancelled_at, cancel_at) {
            // Nothing starts after a cancel, and running tools stop at it.
            assert!(
                events[at..]
                    .iter()
                    .all(|e| !matches!(e, RunEvent::ToolStart { .. })),
                "a tool started after the cancel"
            );
            let cancel_instant = started + Duration::from_millis(ms);
            for interval in log.lock().unwrap().iter() {
                assert!(
                    interval.end <= cancel_instant,
                    "a tool ran past the cancel"
                );
            }
        }

        // Every result answers its own call: the scripted calls pair with
        // the transcript's call blocks turn by turn, in source order.
        let mut scripted: std::collections::HashMap<String, &ScriptedCall> =
            Default::default();
        let assistant_turns = transcript.iter().filter_map(|m| match m {
            Message::Assistant(a) => Some(a),
            _ => None,
        });
        for (message, calls) in assistant_turns.zip(&turns) {
            let ids = message.content.iter().filter_map(|b| match b {
                AssistantBlock::ToolCall(c) => Some(c.id.clone()),
                _ => None,
            });
            for (call_id, call) in ids.zip(calls) {
                scripted.insert(call_id, call);
            }
        }
        for message in &transcript {
            let Message::ToolResult(result) = message else {
                continue;
            };
            let call = scripted[&result.tool_call_id];
            assert_eq!(result.tool_name, call.tool);
            let text: String = result
                .content
                .iter()
                .filter_map(|b| match b {
                    InputBlock::Text(t) => Some(t.text.as_str()),
                    InputBlock::Image(_) => None,
                })
                .collect();
            let end = events
                .iter()
                .position(|e| {
                    matches!(e, RunEvent::ToolEnd { call_id, .. }
                        if *call_id == result.tool_call_id)
                })
                .expect("every result has its ToolEnd");
            // A call that ended after the cancel may have been cut short.
            if cancelled_at.is_some_and(|at| end >= at)
                && result.is_error
                && (text == "cancelled" || text == CANCELLED)
            {
                continue;
            }
            let failed = call.tool == "missing" || call.fail;
            assert_eq!(result.is_error, failed, "{result:?} for {call:?}");
            let want = if call.tool == "missing" {
                "Tool missing not found".to_owned()
            } else if call.fail {
                "probe failed".to_owned()
            } else {
                format!("slept {}", call.ms)
            };
            assert_eq!(text, want, "result for {call:?}");
        }
        if turns.iter().flatten().any(|c| c.tool == "serial") {
            tc.event("serial batch");
        }
        if turns.iter().flatten().any(|c| c.tool == "missing") {
            tc.event("missing tool");
        }

        assert_grammar(&events);
        assert_results_follow_calls(&transcript);

        // ToolStart comes in source order within each turn, and every
        // started call ends exactly once.
        let starts: Vec<String> = events
            .iter()
            .filter_map(|e| match e {
                RunEvent::ToolStart { call_id, .. } => Some(call_id.clone()),
                _ => None,
            })
            .collect();
        let mut ends: Vec<String> = events
            .iter()
            .filter_map(|e| match e {
                RunEvent::ToolEnd { call_id, .. } => Some(call_id.clone()),
                _ => None,
            })
            .collect();
        let source: Vec<String> = transcript
            .iter()
            .flat_map(|m| match m {
                Message::Assistant(a) => a
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        AssistantBlock::ToolCall(c) => Some(c.id.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
                _ => vec![],
            })
            .collect();
        assert_eq!(starts, source);
        ends.sort();
        let mut sorted = starts.clone();
        sorted.sort();
        assert_eq!(ends, sorted);

        // No update after its call's end.
        let mut ended = std::collections::HashSet::new();
        for event in &events {
            match event {
                RunEvent::ToolEnd { call_id, .. } => {
                    ended.insert(call_id.clone());
                }
                RunEvent::ToolUpdate { call_id, .. } => {
                    assert!(!ended.contains(call_id), "update after end")
                }
                _ => {}
            }
        }

        // Deltas add up to what was stored, turn by turn, and every call
        // to a tool that exists reports its update before it ends.
        let assistants: Vec<&tau_ai::message::AssistantMessage> = transcript
            .iter()
            .filter_map(|m| match m {
                Message::Assistant(a) => Some(a),
                _ => None,
            })
            .collect();
        let mut turn_index = 0;
        let (mut text, mut thinking) = (String::new(), String::new());
        let mut fragments: std::collections::BTreeMap<String, String> =
            Default::default();
        for event in &events {
            match event {
                RunEvent::TextDelta { delta, .. } => text.push_str(delta),
                RunEvent::ThinkingDelta { delta, .. } => {
                    thinking.push_str(delta)
                }
                RunEvent::ToolCallDelta {
                    call_id,
                    json_fragment,
                    ..
                } => fragments
                    .entry(call_id.clone())
                    .or_default()
                    .push_str(json_fragment),
                RunEvent::TurnEnd { .. } => {
                    if cancel_at.is_none() {
                        let message = assistants[turn_index];
                        let mut want_text = String::new();
                        let mut want_thinking = String::new();
                        for block in &message.content {
                            match block {
                                AssistantBlock::Text(t) => {
                                    want_text.push_str(&t.text)
                                }
                                AssistantBlock::Thinking(t) => {
                                    want_thinking.push_str(&t.thinking)
                                }
                                AssistantBlock::ToolCall(call) => {
                                    let sent: Value = serde_json::from_str(
                                        &fragments[&call.id],
                                    )
                                    .unwrap();
                                    assert_eq!(
                                        sent,
                                        Value::Object(call.arguments.clone())
                                    );
                                }
                            }
                        }
                        assert_eq!(text, want_text, "turn {turn_index} text");
                        assert_eq!(
                            thinking, want_thinking,
                            "turn {turn_index} thinking"
                        );
                    }
                    turn_index += 1;
                    text.clear();
                    thinking.clear();
                }
                _ => {}
            }
        }
        if cancel_at.is_none() {
            let updated: Vec<&String> = events
                .iter()
                .filter_map(|e| match e {
                    RunEvent::ToolUpdate { call_id, .. } => Some(call_id),
                    _ => None,
                })
                .collect();
            let existing: Vec<String> = transcript
                .iter()
                .flat_map(|m| match m {
                    Message::Assistant(a) => a
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            AssistantBlock::ToolCall(c)
                                if c.name != "missing" =>
                            {
                                Some(c.id.clone())
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>(),
                    _ => vec![],
                })
                .collect();
            let mut updated: Vec<String> =
                updated.into_iter().cloned().collect();
            updated.sort();
            let mut existing = existing;
            existing.sort();
            assert_eq!(
                updated, existing,
                "one update per call to an existing tool"
            );
        }

        // Stored usage equals the sum of the turns' usage.
        let record = store.run(&id.0).await.unwrap().unwrap();
        let (mut input, mut output) = (0i64, 0i64);
        for message in &transcript {
            if let Message::Assistant(a) = message {
                input += (a.usage.input
                    + a.usage.cache_read
                    + a.usage.cache_write) as i64;
                output += a.usage.output as i64;
            }
        }
        assert_eq!(
            (record.input_tokens, record.output_tokens),
            (input, output)
        );

        // Without a cancel, parallel batches overlap and sequential ones
        // do not.
        if cancel_at.is_none() {
            let intervals = log.lock().unwrap().clone();
            let mut offset = 0;
            for calls in &turns {
                let ran: Vec<&ScriptedCall> =
                    calls.iter().filter(|c| c.tool != "missing").collect();
                let batch = &intervals[offset..offset + ran.len()];
                offset += ran.len();
                let sequential = ran.iter().any(|c| c.tool == "serial");
                if sequential {
                    let mut sorted = batch.to_vec();
                    sorted.sort_by_key(|i| i.start);
                    for pair in sorted.windows(2) {
                        assert!(
                            pair[0].end <= pair[1].start,
                            "sequential calls overlapped"
                        );
                    }
                } else {
                    // Calls that take time all start before any of them
                    // ends: run one after another, the second would start
                    // exactly when the first ended.
                    let timed: Vec<&Interval> =
                        batch.iter().filter(|i| i.end > i.start).collect();
                    if let Some(first_end) = timed.iter().map(|i| i.end).min() {
                        assert!(
                            timed.iter().all(|i| i.start < first_end),
                            "parallel calls did not overlap"
                        );
                    }
                }
            }
        }
    });
}

/// A tool call cut off by the output limit never runs; its result says
/// why (pi, `agent-loop.ts:408`).
#[test]
fn truncated_calls_never_run() {
    let llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call("probe", json!({"ms": 1}))
                .stop(MessageStop::Length)
        })
        .turn(|t| t.text("ok"));
    block_on(async {
        let store = Store::memory().await.unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let agent = Agent::new(llm).tool(Probe::new(
            "probe",
            ExecutionMode::Parallel,
            log.clone(),
        ));
        let run = agent.start("go", &store);
        let id = run.id();
        run.outcome().await.unwrap();
        assert!(log.lock().unwrap().is_empty());
        let transcript = stored(&store, &id.0).await;
        let Message::ToolResult(result) = &transcript[2] else {
            panic!("{transcript:?}")
        };
        assert!(result.is_error);
        let text = serde_json::to_string(&result.content).unwrap();
        assert!(text.contains(TRUNCATED), "{text}");
    });
}

/// A steered message is added after the running tool batch, once.
#[test]
fn steering_lands_after_the_batch() {
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("probe", json!({"ms": 100})))
        .turn(|t| t.text("heard you"));
    block_on(async {
        let store = Store::memory().await.unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let agent = Agent::new(llm.clone()).tool(Probe::new(
            "probe",
            ExecutionMode::Parallel,
            log,
        ));
        let run = agent.start("go", &store);
        let id = run.id();
        tokio::time::sleep(Duration::from_millis(10)).await;
        run.steer("also check the tests");
        let outcome = run.outcome().await.unwrap();
        assert_eq!(outcome.text, "heard you");
        let transcript = stored(&store, &id.0).await;
        let kinds: Vec<&str> = transcript.iter().map(|m| m.role()).collect();
        assert_eq!(
            kinds,
            ["user", "assistant", "toolResult", "user", "assistant"]
        );
        // The model saw the steered message on its second request.
        let requests = llm.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].transcript.len(), 4);
    });
}

/// A `RunControl` steers and cancels while another task reads events.
#[test]
fn control_steers_and_cancels_while_events_are_read() {
    use futures_util::StreamExt;
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("probe", json!({"ms": 100})))
        .turn(|t| t.tool_call("probe", json!({"ms": 5_000})))
        .turn(|t| t.text("never reached"));
    block_on(async {
        let store = Store::memory().await.unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let agent = Agent::new(llm.clone()).tool(Probe::new(
            "probe",
            ExecutionMode::Parallel,
            log,
        ));
        let mut run = agent.start("go", &store);
        let control = run.control();
        assert_eq!(control.id(), run.id());
        let reader = tokio::spawn(async move {
            let mut ends = Vec::new();
            {
                let mut events = run.events();
                while let Some(event) = events.next().await {
                    if let RunEvent::RunEnd { stop, .. } = event {
                        ends.push(stop);
                    }
                }
            }
            (ends, run.outcome().await)
        });
        tokio::time::sleep(Duration::from_millis(10)).await;
        control.steer("also check the tests");
        // Cancel once the second request, which carries the steered
        // message, is out and its 5 s tool call is running.
        while llm.requests().len() < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        control.cancel();
        let (ends, outcome) = reader.await.unwrap();
        assert_eq!(ends, [StopReason::Cancelled]);
        assert_eq!(outcome.unwrap().stop, StopReason::Cancelled);
        let second = &llm.requests()[1];
        assert_eq!(second.transcript.len(), 4, "the steered message");
        assert_eq!(llm.requests().len(), 2, "no request after the cancel");
    });
}

/// Hooks run in order; the first that blocks wins and later hooks do not
/// run; an erroring hook blocks; changed arguments are validated again.
#[test]
fn before_tool_hooks() {
    struct Block(&'static str, Arc<Mutex<Vec<&'static str>>>);
    #[async_trait]
    impl RunHook for Block {
        async fn before_tool(
            &self,
            call: &mut ToolCall,
            _: &HookCtx,
        ) -> Result<Decision, PluginError> {
            self.1.lock().unwrap().push(self.0);
            match (self.0, call.args["ms"].as_u64()) {
                ("blocker", Some(1)) => Ok(Decision::Block("not today".into())),
                ("breaker", Some(2)) => return Err("hook exploded".into()),
                ("mangler", Some(3)) => {
                    call.args = json!({"ms": "not a number at all"});
                    Ok(Decision::Allow)
                }
                _ => Ok(Decision::Allow),
            }
        }
    }
    let llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call("probe", json!({"ms": 1}))
                .tool_call("probe", json!({"ms": 2}))
                .tool_call("probe", json!({"ms": 3}))
                .tool_call("probe", json!({"ms": 4}))
        })
        .turn(|t| t.text("ok"));
    block_on(async {
        let store = Store::memory().await.unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let agent = Agent::new(llm)
            .tool(Probe::new("probe", ExecutionMode::Parallel, log.clone()))
            .hook(Block("blocker", seen.clone()))
            .hook(Block("breaker", seen.clone()))
            .hook(Block("mangler", seen.clone()));
        let run = agent.start("go", &store);
        let id = run.id();
        run.outcome().await.unwrap();
        let transcript = stored(&store, &id.0).await;
        let texts: Vec<(String, bool)> = transcript[2..6]
            .iter()
            .map(|m| match m {
                Message::ToolResult(r) => {
                    (serde_json::to_string(&r.content).unwrap(), r.is_error)
                }
                _ => panic!(),
            })
            .collect();
        assert!(texts[0].0.contains("not today") && texts[0].1);
        assert!(texts[1].0.contains("hook exploded") && texts[1].1);
        assert!(
            texts[2].1,
            "invalid changed arguments must not run: {:?}",
            texts[2]
        );
        assert!(texts[3].0.contains("slept 4") && !texts[3].1);
        // Only the last call ran.
        assert_eq!(log.lock().unwrap().len(), 1);
        // The blocker stopped the chain for call 1: later hooks never saw it.
        let order = seen.lock().unwrap().clone();
        assert_eq!(&order[..3], ["blocker", "blocker", "breaker"]);
    });
}

/// An unknown tool gets pi's message and the run goes on.
#[test]
fn unknown_tool_is_an_error_result() {
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("nope", json!({})))
        .turn(|t| t.text("ok"));
    block_on(async {
        let store = Store::memory().await.unwrap();
        let run = Agent::new(llm).start("go", &store);
        let id = run.id();
        assert_eq!(run.outcome().await.unwrap().stop, StopReason::Stop);
        let transcript = stored(&store, &id.0).await;
        let Message::ToolResult(result) = &transcript[2] else {
            panic!()
        };
        assert!(
            serde_json::to_string(&result.content)
                .unwrap()
                .contains("Tool nope not found")
        );
    });
}

/// A failed model response ends the run with its error.
#[test]
fn model_error_ends_the_run() {
    let llm =
        ScriptedModel::new().turn(|t| t.error("insufficient_quota", "boom"));
    block_on(async {
        let store = Store::memory().await.unwrap();
        let outcome = Agent::new(llm).run("go", &store).await.unwrap();
        assert!(
            matches!(&outcome.stop, StopReason::Error(m) if m.contains("boom")),
            "{:?}",
            outcome.stop
        );
        let record = store.run(&outcome.run.0).await.unwrap().unwrap();
        assert_eq!(record.status, tau_store::Status::Failed);
    });
}

/// `max_turns` ends a run that keeps calling tools.
#[test]
fn turn_limit_ends_the_run() {
    let mut llm = ScriptedModel::new();
    for _ in 0..5 {
        llm = llm.turn(|t| t.tool_call("probe", json!({"ms": 0})));
    }
    block_on(async {
        let store = Store::memory().await.unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let outcome = Agent::new(llm)
            .tool(Probe::new("probe", ExecutionMode::Parallel, log))
            .limits(Limits::default().max_turns(2))
            .run("go", &store)
            .await
            .unwrap();
        assert_eq!(outcome.stop, StopReason::Limit(LimitKind::Turns));
        let record = store.run(&outcome.run.0).await.unwrap().unwrap();
        assert_eq!(record.status, tau_store::Status::Limit);
    });
}

/// A slow subscriber holds its own run and no other. The slow run's hook
/// waits on a gate; the other run finishes while the gate is shut, and
/// the slow run finishes only once it opens.
#[test]
fn slow_hook_holds_only_its_run() {
    struct Gate(Arc<tokio::sync::Semaphore>);
    #[async_trait]
    impl RunHook for Gate {
        async fn on_event(&self, _: &RunEvent) {
            let _permit = self.0.acquire().await.unwrap();
        }
    }
    block_on(async {
        let store = Store::memory().await.unwrap();
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let slow = Agent::new(ScriptedModel::new().turn(|t| t.text("slow")))
            .hook(Gate(gate.clone()))
            .start("go", &store);
        let fast = Agent::new(ScriptedModel::new().turn(|t| t.text("fast")))
            .run("go", &store)
            .await
            .unwrap();
        assert_eq!(fast.text, "fast");
        let mut slow = std::pin::pin!(slow.outcome());
        assert!(
            futures_util::FutureExt::now_or_never(slow.as_mut()).is_none(),
            "the slow run finished with its hook blocked"
        );
        gate.add_permits(1_000);
        assert_eq!(slow.await.unwrap().text, "slow");
    });
}

/// The run sends the agent's model, instructions and reasoning, and tool
/// schemas in strict form.
#[test]
fn settings_reach_the_model() {
    let llm = ScriptedModel::new().turn(|t| t.text("ok"));
    block_on(async {
        let store = Store::memory().await.unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        Agent::new(llm.clone())
            .model("gpt-5.4-mini")
            .instructions("Be brief.")
            .reasoning(tau_ai::responses::request::ReasoningEffort::High)
            .tool(Probe::new("probe", ExecutionMode::Parallel, log))
            .run("go", &store)
            .await
            .unwrap();
        let settings = &llm.requests()[0].settings;
        assert_eq!(settings.model, "gpt-5.4-mini");
        assert_eq!(settings.instructions.as_deref(), Some("Be brief."));
        assert_eq!(
            settings.reasoning,
            Some(tau_ai::responses::request::ReasoningEffort::High)
        );
        let tool = &settings.tools[0];
        assert_eq!((tool.name.as_str(), tool.strict), ("probe", true));
        assert_eq!(tool.parameters["additionalProperties"], json!(false));
        assert_eq!(tool.description, "Sleeps, then answers.");
    });
}

/// Cancelling a run mid-batch ends it as cancelled; the running tool sees
/// the token and every call still gets a result. The cancel waits for the
/// tool to start rather than for virtual time, which store writes can
/// move.
#[test]
fn cancel_mid_batch() {
    struct Waiter(Arc<tokio::sync::Notify>, Value);
    #[async_trait]
    impl AgentTool for Waiter {
        fn name(&self) -> &str {
            "wait"
        }
        fn description(&self) -> &str {
            "Waits for cancellation."
        }
        fn parameters(&self) -> &Value {
            &self.1
        }
        async fn call(
            &self,
            _: Value,
            ctx: ToolCtx,
        ) -> Result<ToolOutput, ToolError> {
            self.0.notify_one();
            ctx.cancel.cancelled().await;
            return Err("cancelled while waiting".into());
        }
    }
    let llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call("wait", json!({}))
                .tool_call("probe", json!({"ms": 1}))
        })
        .turn(|t| t.text("never"));
    block_on(async {
        let store = Store::memory().await.unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let started = Arc::new(tokio::sync::Notify::new());
        let run = Agent::new(llm.clone())
            .tool(Waiter(started.clone(), json!({"type": "object"})))
            .tool(Probe::new("probe", ExecutionMode::Parallel, log.clone()))
            .start("go", &store);
        let id = run.id();
        started.notified().await;
        run.cancel();
        let outcome = run.outcome().await.unwrap();
        assert_eq!(outcome.stop, StopReason::Cancelled);
        let transcript = stored(&store, &id.0).await;
        assert_results_follow_calls(&transcript);
        let Message::ToolResult(waited) = &transcript[2] else {
            panic!("{transcript:?}")
        };
        assert!(waited.is_error);
        assert!(
            serde_json::to_string(&waited.content)
                .unwrap()
                .contains("cancelled while waiting")
        );
        // The run stopped: the model was asked only once.
        assert_eq!(llm.requests().len(), 1);
        let record = store.run(&id.0).await.unwrap().unwrap();
        assert_eq!(record.status, tau_store::Status::Cancelled);
    });
}

/// A response that failed runs none of its tool calls.
#[test]
fn failed_response_runs_no_tools() {
    let llm = ScriptedModel::new().turn_with(|_| {
        let mut message = tau_ai::message::AssistantMessage {
            content: vec![AssistantBlock::ToolCall(
                tau_ai::message::ToolCall {
                    id: "call_1|fc_1".into(),
                    name: "probe".into(),
                    arguments: json!({"ms": 1}).as_object().unwrap().clone(),
                },
            )],
            api: tau_ai::message::API.into(),
            provider: tau_ai::message::PROVIDER.into(),
            model: "gpt-5.5".into(),
            response_id: None,
            usage: Default::default(),
            stop_reason: MessageStop::Error,
            error_message: Some("server_error: boom".into()),
            timestamp: 0,
        };
        message.usage.output = 1;
        message
    });
    block_on(async {
        let store = Store::memory().await.unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let outcome = Agent::new(llm)
            .tool(Probe::new("probe", ExecutionMode::Parallel, log.clone()))
            .run("go", &store)
            .await
            .unwrap();
        assert!(
            matches!(outcome.stop, StopReason::Error(_)),
            "{:?}",
            outcome.stop
        );
        assert!(log.lock().unwrap().is_empty());
    });
}

#[test]
fn errors_and_debug_output() {
    use tau_agent::agent::AgentError;
    let llm = tau_ai::llm::LlmError {
        message: "down".into(),
    };
    assert_eq!(AgentError::Llm(llm).to_string(), "model provider: down");
    assert_eq!(
        AgentError::Schema {
            tool: "t".into(),
            message: "bad".into()
        }
        .to_string(),
        "tool t has an invalid schema: bad"
    );
    assert_eq!(AgentError::Panicked.to_string(), "the run's task panicked");
    let agent = Agent::new(ScriptedModel::new())
        .name("lead")
        .model("gpt-5.5");
    let shown = format!("{agent:?}");
    assert!(
        shown.contains("lead") && shown.contains("gpt-5.5"),
        "{shown}"
    );
    block_on(async {
        let store = Store::memory().await.unwrap();
        let run = Agent::new(ScriptedModel::new().turn(|t| t.text("x")))
            .start("go", &store);
        let shown = format!("{run:?}");
        assert!(shown.contains(&*run.id().0), "{shown}");
        run.outcome().await.unwrap();
    });
}

/// With warm-up on, each run warms its session up once, with the run's
/// settings, before its first turn; with it off (the default), never.
#[test]
fn warm_up_runs_once_per_run_when_on() {
    let llm = ScriptedModel::new()
        .turn(|t| t.text("one"))
        .turn(|t| t.text("two"))
        .turn(|t| t.text("three"));
    block_on(async {
        let store = Store::memory().await.unwrap();
        let agent = Agent::new(llm.clone()).instructions("Be brief.");
        agent.run("cold", &store).await.unwrap();
        assert!(llm.warm_ups().is_empty());
        let warm = agent.warmup(true);
        warm.run("warm", &store).await.unwrap();
        warm.run("warm again", &store).await.unwrap();
        let warm_ups = llm.warm_ups();
        assert_eq!(warm_ups.len(), 2);
        assert_eq!(warm_ups[0], llm.requests()[1].settings);
    });
}

/// `tools` adds several tools at once, after any added one by one, in
/// order.
#[test]
fn tools_adds_several_in_order() {
    let llm = ScriptedModel::new().turn(|t| t.text("ok"));
    block_on(async {
        let store = Store::memory().await.unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let kit: Vec<Arc<dyn AgentTool>> = vec![
            Arc::new(Probe::new("a", ExecutionMode::Parallel, log.clone())),
            Arc::new(Probe::new("b", ExecutionMode::Parallel, log.clone())),
        ];
        Agent::new(llm.clone())
            .tool(Probe::new("first", ExecutionMode::Parallel, log))
            .tools(kit)
            .run("go", &store)
            .await
            .unwrap();
        let names: Vec<String> = llm.requests()[0]
            .settings
            .tools
            .iter()
            .map(|t| t.name.clone())
            .collect();
        assert_eq!(names, ["first", "a", "b"]);
    });
}

/// A batch holding a sequential tool runs its calls one at a time, in
/// source order, parallel calls included; a batch of parallel tools
/// alone runs its calls together.
#[test]
fn a_sequential_tool_serializes_its_batch() {
    let llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call("probe", json!({"ms": 100}))
                .tool_call("serial", json!({"ms": 100}))
                .tool_call("probe", json!({"ms": 100}))
        })
        .turn(|t| {
            t.tool_call("probe", json!({"ms": 100}))
                .tool_call("probe", json!({"ms": 100}))
        })
        .turn(|t| t.text("done"));
    block_on(async {
        let store = Store::memory().await.unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let agent = Agent::new(llm.clone())
            .tool(Probe::new("probe", ExecutionMode::Parallel, log.clone()))
            .tool(Probe::new("serial", ExecutionMode::Sequential, log.clone()));
        let outcome = agent.run("go", &store).await.unwrap();
        assert_eq!(outcome.text, "done");
        let log = log.lock().unwrap().clone();
        assert_eq!(log.len(), 5);
        for pair in log[..3].windows(2) {
            assert!(pair[1].start >= pair[0].end, "{log:?}");
        }
        assert_eq!(log[3].start, log[4].start, "{log:?}");
    });
}
