//! Fast compaction as a plugin of an `Agent`, with `ScriptedModel`,
//! `Store::memory()` and a `FakeJev` whose answers the tests choose.

use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{Value, json};
use tau_agent::{
    agent::{Agent, Outcome},
    error::ToolError,
    event::{RunEvent, StopReason},
    tool::{AgentTool, ToolCtx, ToolOutput},
};
use tau_ai::message::{InputBlock, Message};
use tau_fast_compaction::{Details, FastCompaction, NAME, Settings};
use tau_jev::{JevError, fake::FakeJev};
use tau_store::{Entry, Store};
use tau_testing::{block_on, scripted::ScriptedModel};

/// Returns `size` characters of output that name the file it "read".
struct Read {
    schema: Value,
}

impl Read {
    fn new() -> Self {
        Self {
            schema: json!({
                "type": "object",
                "properties": {"path": {"type": "string"}, "size": {"type": "integer"}},
                "required": ["path", "size"],
            }),
        }
    }
}

#[async_trait]
impl AgentTool for Read {
    fn name(&self) -> &str {
        "read"
    }
    fn description(&self) -> &str {
        "Reads a file."
    }
    fn parameters(&self) -> &Value {
        &self.schema
    }
    async fn call(
        &self,
        args: Value,
        _ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let path = args["path"].as_str().unwrap_or_default();
        let size = args["size"].as_u64().unwrap_or(0) as usize;
        Ok(ToolOutput::text(format!(
            "CONTENTS OF {path}: {}",
            "x".repeat(size)
        )))
    }
}

/// Settings for a 1,000-token window, pinning only the last message.
fn settings() -> Settings {
    Settings {
        context_window: Some(1_000),
        preserve_recent: 1,
        ..Settings::default()
    }
}

/// Two reads, the second pushing the context past 60% of the window,
/// then an answer.
fn two_reads() -> ScriptedModel {
    ScriptedModel::new()
        .turn(|t| {
            t.tool_call("read", json!({"path": "a.rs", "size": 4000}))
                .usage(100, 10)
        })
        .turn(|t| {
            t.tool_call("read", json!({"path": "b.rs", "size": 10}))
                .usage(700, 10)
        })
        .turn(|t| t.text("done"))
}

async fn run(agent: &Agent, store: &Store) -> (Vec<RunEvent>, Outcome) {
    let mut run = agent.start("fix the bug in a.rs", store);
    let events = run.events().collect().await;
    (events, run.outcome().await.unwrap())
}

fn result_text(message: &Message) -> String {
    match message {
        Message::ToolResult(result) => match &result.content[0] {
            InputBlock::Text(text) => text.text.clone(),
            InputBlock::Image(_) => "<image>".into(),
        },
        other => panic!("expected a tool result, got {other:?}"),
    }
}

fn rewrites(events: &[RunEvent]) -> Vec<(String, u64, u64)> {
    events
        .iter()
        .filter_map(|e| match e {
            RunEvent::ContextRewritten {
                plugin,
                tokens_before,
                tokens_after,
                ..
            } => Some((plugin.to_string(), *tokens_before, *tokens_after)),
            _ => None,
        })
        .collect()
}

/// Past 60% of the window, the plugin asks Jev about the one unpinned
/// call, and on "the call matters, its output does not", cuts the output
/// to its head and a note. The next request sends the pruned transcript;
/// the store keeps the rewrite and its ledger; Jev's usage is charged to
/// the run; Jev saw the call, never its output.
#[test]
fn a_stale_result_is_cut() {
    let model = two_reads();
    let jev =
        FakeJev::nouls(|id| if id.starts_with("call_") { 0.9 } else { 0.1 });
    block_on(async {
        let store = Store::memory().await.unwrap();
        let agent = Agent::new(model.clone())
            .tool(Read::new())
            .plugin(FastCompaction::new(jev.clone()).settings(settings()));
        let (events, outcome) = run(&agent, &store).await;
        assert_eq!(outcome.stop, StopReason::Stop);
        assert_eq!(outcome.text, "done");

        let rewritten = rewrites(&events);
        assert_eq!(rewritten.len(), 1, "{rewritten:?}");
        assert_eq!(rewritten[0].0, NAME);

        // The ledger is reported, before the rewrite it explains.
        let report = events
            .iter()
            .position(|event| {
                matches!(event, RunEvent::PluginReport { body, .. }
                if body["kind"] == "ledger")
            })
            .expect("a ledger report");
        let rewrite = events
            .iter()
            .position(|event| {
                matches!(event, RunEvent::ContextRewritten { .. })
            })
            .unwrap();
        assert!(report < rewrite);
        let RunEvent::PluginReport { body, .. } = &events[report] else {
            unreachable!()
        };
        let reported: Details = serde_json::from_value(body.clone()).unwrap();
        assert_eq!(reported.stats.results_dropped, 1);
        // Its cost is what the pass charged, before the report.
        let charged: f64 = events[..report]
            .iter()
            .filter_map(|event| match event {
                RunEvent::PluginCharged { plugin, usage, .. }
                    if &**plugin == NAME =>
                {
                    Some(usage.cost.total)
                }
                _ => None,
            })
            .sum();
        assert!(charged > 0.0);
        assert_eq!(reported.stats.cost, charged);

        let next = &model.requests()[2].transcript;
        let first = result_text(&next[2]);
        assert!(first.starts_with("CONTENTS OF a.rs: "), "{first}");
        assert!(
            first.contains("[fast-compaction truncated") && first.len() < 500,
            "{first}"
        );
        // The note names an archive holding the result whole.
        let (_, rest) = first.rsplit_once("; full result: ").unwrap();
        let (archive, _) =
            rest.split_once(" (read or grep it if needed)]").unwrap();
        assert_eq!(
            std::fs::read_to_string(archive).unwrap(),
            format!("CONTENTS OF a.rs: {}", "x".repeat(4000))
        );
        std::fs::remove_file(archive).unwrap();
        assert_eq!(
            result_text(&next[4]),
            format!("CONTENTS OF b.rs: {}", "x".repeat(10))
        );

        let asked = jev.requests();
        assert_eq!(asked.len(), 1);
        let ids: Vec<&String> = asked[0].questions.keys().collect();
        assert_eq!(ids, ["call_t1", "result_t1"]);
        let state = asked[0].state.to_string();
        assert!(!state.contains(&"x".repeat(100)), "the output reached Jev");
        assert!(state.contains("a.rs"));

        let entries = store.transcript(&outcome.run.0).await.unwrap();
        let Entry::Context { plugin, body } = &entries[0] else {
            panic!("{entries:?}");
        };
        assert_eq!(plugin, NAME);
        let details: Details = serde_json::from_str(body).unwrap();
        assert_eq!(details.stats.results_dropped, 1);
        assert!(details.stats.reduction_ratio >= 0.25);
        assert!(outcome.usage.cost.total > 0.0, "Jev's usage is charged");
    });
}

/// On "neither the call nor its output matters", the call goes with its
/// result.
#[test]
fn a_stale_call_is_dropped() {
    let model = two_reads();
    let jev = FakeJev::nouls(|_| 0.0);
    block_on(async {
        let store = Store::memory().await.unwrap();
        let agent = Agent::new(model.clone())
            .tool(Read::new())
            .plugin(FastCompaction::new(jev).settings(settings()));
        let (_, outcome) = run(&agent, &store).await;
        assert_eq!(outcome.text, "done");
        let next = &model.requests()[2].transcript;
        assert_eq!(next.len(), 3, "the first call and its result are gone");
        assert_eq!(
            result_text(&next[2]),
            format!("CONTENTS OF b.rs: {}", "x".repeat(10))
        );
    });
}

/// A pass that would save too little declines: nothing is rewritten, and
/// the cooldown keeps the next turn from asking again.
#[test]
fn a_small_saving_declines_and_cools_down() {
    let model = ScriptedModel::new()
        .turn(|t| {
            t.tool_call("read", json!({"path": "a.rs", "size": 10}))
                .usage(100, 10)
        })
        .turn(|t| {
            t.tool_call("read", json!({"path": "b.rs", "size": 10}))
                .usage(700, 10)
        })
        .turn(|t| {
            t.tool_call("read", json!({"path": "c.rs", "size": 10}))
                .usage(750, 10)
        })
        .turn(|t| t.text("done"));
    let jev = FakeJev::nouls(|_| 0.0);
    block_on(async {
        let store = Store::memory().await.unwrap();
        let agent = Agent::new(model.clone()).tool(Read::new()).plugin(
            FastCompaction::new(jev.clone()).settings(Settings {
                min_reduction_ratio: 0.9,
                ..settings()
            }),
        );
        let (events, outcome) = run(&agent, &store).await;
        assert_eq!(outcome.text, "done");
        assert!(rewrites(&events).is_empty());
        assert_eq!(
            jev.requests().len(),
            1,
            "the cooldown held the second pass"
        );
    });
}

/// An overflow runs a pass whatever the window share, and the turn is
/// retried on the pruned transcript.
#[test]
fn an_overflow_prunes_and_retries() {
    let model = ScriptedModel::new()
        .turn(|t| t.tool_call("read", json!({"path": "a.rs", "size": 4000})))
        .turn(|t| t.tool_call("read", json!({"path": "b.rs", "size": 10})))
        .turn(|t| t.error("context_length_exceeded", "too long"))
        .turn(|t| t.text("done"));
    let jev = FakeJev::nouls(|_| 0.0);
    block_on(async {
        let store = Store::memory().await.unwrap();
        let agent = Agent::new(model.clone()).tool(Read::new()).plugin(
            FastCompaction::new(jev).settings(Settings {
                context_window: Some(u64::MAX),
                ..settings()
            }),
        );
        let (events, outcome) = run(&agent, &store).await;
        assert_eq!(outcome.text, "done");
        model.assert_exhausted();
        assert_eq!(rewrites(&events).len(), 1);
        assert_eq!(model.requests()[3].transcript.len(), 3);
    });
}

/// A Jev failure is reported and leaves the run going, uncompacted.
#[test]
fn a_jev_failure_is_reported() {
    let model = two_reads();
    let jev = FakeJev::new(|_| Err(JevError::Status(503)));
    block_on(async {
        let store = Store::memory().await.unwrap();
        let agent = Agent::new(model.clone())
            .tool(Read::new())
            .plugin(FastCompaction::new(jev).settings(settings()));
        let (events, outcome) = run(&agent, &store).await;
        assert_eq!(outcome.text, "done");
        let errors: Vec<String> = events
            .iter()
            .filter_map(|e| match e {
                RunEvent::PluginError {
                    plugin, message, ..
                } => {
                    assert_eq!(&**plugin, NAME);
                    Some(message.clone())
                }
                _ => None,
            })
            .collect();
        assert_eq!(errors, ["Jev answered with status 503"]);
        assert_eq!(model.requests()[2].transcript.len(), 5);
    });
}

/// With pruning declined, summarizing compaction added after it takes the
/// overflow: the two compose in the order they were added.
#[test]
fn compaction_follows_when_pruning_cannot_help() {
    let model = ScriptedModel::new()
        .turn(|t| t.tool_call("read", json!({"path": "a.rs", "size": 10})))
        .turn(|t| t.error("context_length_exceeded", "too long"))
        .turn(|t| t.text("summary"))
        .turn(|t| t.text("done"));
    let jev = FakeJev::nouls(|_| 1.0);
    block_on(async {
        let store = Store::memory().await.unwrap();
        let agent = Agent::new(model.clone())
            .tool(Read::new())
            .plugin(FastCompaction::new(jev.clone()).settings(Settings {
                context_window: Some(u64::MAX),
                ..settings()
            }))
            .plugin(
                tau_compaction::Compaction::default()
                    .context_window(u64::MAX)
                    .keep_recent_tokens(1),
            );
        let mut run = agent.start("go", &store);
        run.steer("then this");
        let events: Vec<RunEvent> = run.events().collect().await;
        let outcome = run.outcome().await.unwrap();
        assert_eq!(outcome.text, "done");
        let by: Vec<String> =
            rewrites(&events).into_iter().map(|r| r.0).collect();
        assert_eq!(by, ["compaction"]);
        assert_eq!(jev.requests().len(), 1, "pruning was asked first");
    });
}

/// A fork of a pruned run starts from the pruned transcript, and the
/// plugin resumes its ledger from the stored rewrite.
#[test]
fn a_fork_resumes_the_ledger() {
    let model = two_reads().turn(|t| t.text("forked"));
    let jev =
        FakeJev::nouls(|id| if id.starts_with("call_") { 0.9 } else { 0.1 });
    block_on(async {
        let store = Store::memory().await.unwrap();
        let agent = Agent::new(model.clone())
            .tool(Read::new())
            .plugin(FastCompaction::new(jev).settings(settings()));
        let (_, base) = run(&agent, &store).await;
        let fork = agent
            .fork(&base.checkpoint())
            .run("and now", &store)
            .await
            .unwrap();
        assert_eq!(fork.text, "forked");
        let seen = &model.requests()[3].transcript;
        assert!(result_text(&seen[2]).contains("[fast-compaction truncated"));
    });
}

/// Debug output shows the settings, not the Jev client.
#[test]
fn debug_shows_the_settings() {
    let plugin =
        FastCompaction::new(FakeJev::nouls(|_| 1.0)).settings(settings());
    let debug = format!("{plugin:?}");
    assert!(
        debug.starts_with("FastCompaction { settings: Settings {"),
        "{debug}"
    );
    assert!(debug.contains("preserve_recent: 1"), "{debug}");
}
