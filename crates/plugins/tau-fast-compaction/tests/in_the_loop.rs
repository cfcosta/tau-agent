//! tau-fast-compaction's history pruning inside the agent loop, its Luau
//! run by tau-luau-plugins as tau runs it, with `ScriptedModel`,
//! `tau_store_sqlite::memory()` and a `FakeJev` whose answers the tests
//! choose. Time is real: a hook's limit is a timer, which paused time
//! would let run out while the store works on a thread of its own.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

mod common;

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
use tau_jev::{JevError, fake::FakeJev};
use tau_store::Store;
use tau_testing::{block_on_io, scripted::ScriptedModel};

use crate::common::{fast, logs, rewrites, scratch, state};

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
fn settings() -> Value {
    json!({ "window": 1_000, "preserve_recent": 1 })
}

/// The settings with `more` over them.
fn with(more: Value) -> Value {
    let mut out = settings();
    for (key, value) in more.as_object().unwrap() {
        out[key] = value.clone();
    }
    out
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

fn rewritten(events: &[RunEvent]) -> Vec<(String, u64, u64)> {
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
    // Where the plugin archives by default: the system's temporary
    // directory.
    let archives = std::env::temp_dir();
    block_on_io(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let agent = Agent::new(model.clone()).tool(Read::new()).plugin(fast(
            settings(),
            &jev,
            &archives,
        ));
        let (events, outcome) = run(&agent, &store).await;
        assert_eq!(outcome.stop, StopReason::Stop);
        assert_eq!(outcome.text, "done");

        let rewritten = rewritten(&events);
        assert_eq!(rewritten.len(), 1, "{rewritten:?}");
        assert_eq!(rewritten[0].0, tau_luau_plugins::NAME);
        // The cut result's size is gone from the context, though the
        // last call's usage still counts it.
        assert!(rewritten[0].2 + 500 < rewritten[0].1, "{rewritten:?}");

        // What Jev cost for the pass is what the run was charged.
        let kept = state(&store, &outcome.run.0).await;
        let stats = &kept["passes"][0];
        assert_eq!(stats["results_dropped"], 1);
        let charged = crate::common::charged(&events);
        assert!(charged > 0.0);
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

        let stored = rewrites(&store, &outcome.run.0).await;
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].0, tau_fast_compaction::NAME);
        let details = &stored[0].1;
        assert_eq!(details["stats"]["results_dropped"], 1);
        assert!(details["stats"]["reduction_ratio"].as_f64().unwrap() >= 0.25);
        assert_eq!(details["stats"]["cost"].as_f64().unwrap(), charged);
        assert!(outcome.usage.cost.total > 0.0, "Jev's usage is charged");
    });
}

/// On "neither the call nor its output matters", the call goes with its
/// result.
#[test]
fn a_stale_call_is_dropped() {
    let model = two_reads();
    let jev = FakeJev::nouls(|_| 0.0);
    block_on_io(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let agent = Agent::new(model.clone()).tool(Read::new()).plugin(fast(
            settings(),
            &jev,
            &scratch("stale-call"),
        ));
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

/// A pass that would save too little declines: nothing is rewritten,
/// and the cooldown keeps the next turns from asking again. The cooldown
/// is a share of the window: on a 100,000-token window, growing 9,000
/// tokens after a pass does not run another, and growing past 10,000
/// does.
#[test]
fn the_cooldown_grows_with_the_window() {
    let read = |path: &str, input| {
        let path = path.to_owned();
        move |t: tau_testing::scripted::TurnBuilder| {
            t.tool_call("read", json!({"path": path, "size": 10}))
                .usage(input, 10)
        }
    };
    // The first turn's one call is pinned, so nothing is asked there.
    let model = ScriptedModel::new()
        .turn(read("a.rs", 100))
        .turn(read("b.rs", 61_000))
        .turn(read("c.rs", 70_000))
        .turn(read("d.rs", 72_000))
        .turn(|t| t.text("done"));
    let jev = FakeJev::nouls(|_| 0.0);
    block_on_io(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let agent = Agent::new(model.clone()).tool(Read::new()).plugin(fast(
            with(json!({ "window": 100_000, "min_reduction_ratio": 0.9 })),
            &jev,
            &scratch("cooldown"),
        ));
        let (events, outcome) = run(&agent, &store).await;
        assert_eq!(outcome.text, "done");
        assert!(rewritten(&events).is_empty(), "each pass saves too little");
        assert_eq!(jev.requests().len(), 2, "passes at 61k and 72k only");
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
    block_on_io(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let agent = Agent::new(model.clone()).tool(Read::new()).plugin(fast(
            with(json!({ "window": 1_000_000_000_000u64 })),
            &jev,
            &scratch("overflow"),
        ));
        let (events, outcome) = run(&agent, &store).await;
        assert_eq!(outcome.text, "done");
        model.assert_exhausted();
        assert_eq!(rewritten(&events).len(), 1);
        assert_eq!(model.requests()[3].transcript.len(), 3);
    });
}

/// A Jev failure is in the plugin's log and leaves the run going,
/// uncompacted.
#[test]
fn a_jev_failure_is_reported() {
    let model = two_reads();
    let jev = FakeJev::new(|_| Err(JevError::Status(503)));
    block_on_io(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let agent = Agent::new(model.clone()).tool(Read::new()).plugin(fast(
            settings(),
            &jev,
            &scratch("jev-failure"),
        ));
        let (_, outcome) = run(&agent, &store).await;
        assert_eq!(outcome.text, "done");
        let logged = logs(&store, &outcome.run.0).await;
        assert_eq!(logged.len(), 1, "{logged:?}");
        assert!(
            logged[0].contains("Jev answered with status 503"),
            "{logged:?}"
        );
        assert_eq!(model.requests()[2].transcript.len(), 5);
    });
}

/// A fork of a pruned run starts from the pruned transcript, and the
/// plugin resumes its ledger from the stored rewrite: a later pass in the
/// fork, told the old result now matters, still holds its cut, since
/// decisions only escalate. Started afresh, the fork's ledger would hold
/// the milder answer.
#[test]
fn a_fork_resumes_the_ledger() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let forked = Arc::new(AtomicBool::new(false));
    let told = forked.clone();
    // Before the fork, a.rs's output does not matter; after it, it does,
    // and c.rs's does not.
    let jev = FakeJev::nouls(move |id| match id {
        "result_t1" if !told.load(Ordering::SeqCst) => 0.1,
        "result_t3" => 0.1,
        _ => 0.9,
    });
    let read = |path: &str, size: u64, input: u64| {
        let path = path.to_owned();
        move |t: tau_testing::scripted::TurnBuilder| {
            t.tool_call("read", json!({"path": path, "size": size}))
                .usage(input, 10)
        }
    };
    let model = two_reads()
        // The fork's turns: c.rs is the last call, and kept, at the first
        // pass; the second, past the cooldown, may judge it.
        .turn(read("c.rs", 4000, 100))
        .turn(read("d.rs", 10, 1300))
        .turn(|t| t.text("forked"));
    let archives = tempfile::tempdir().unwrap();
    block_on_io(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let agent = Agent::new(model.clone()).tool(Read::new()).plugin(fast(
            with(json!({ "start_reserve_tokens": 0 })),
            &jev,
            archives.path(),
        ));
        let (_, base) = run(&agent, &store).await;
        forked.store(true, Ordering::SeqCst);
        let fork = agent
            .fork(&base.checkpoint())
            .run("and now", &store)
            .await
            .unwrap();
        assert_eq!(fork.text, "forked");
        let seen = &model.requests()[3].transcript;
        assert!(result_text(&seen[2]).contains("[fast-compaction truncated"));

        let stored = rewrites(&store, &fork.run.0).await;
        let details = &stored.last().unwrap().1;
        let action = |call: &str| {
            details["decisions"]
                .as_array()
                .unwrap()
                .iter()
                .find(|decision| decision["call_id"] == call)
                .map(|decision| decision["action"].as_str().unwrap().to_owned())
        };
        // Jev's questions name calls `t1`, `t2`… in order; the ledger
        // names them by their ids.
        assert_eq!(
            action("call_2|fc_2").as_deref(),
            Some("drop_result"),
            "c.rs"
        );
        assert_eq!(
            action("call_0|fc_0").as_deref(),
            Some("drop_result"),
            "a.rs"
        );
    });
}

/// A fork starts on its parent's transcript past 60% of the window
/// without a pass, which would give up the parent's prompt cache; one
/// within the start reserve of the window gets a pass first.
#[test]
fn a_fork_prunes_at_start_only_when_it_must() {
    // About 6,000 tokens of output in a 10,000-token window.
    let read = || {
        ScriptedModel::new()
            .turn(|t| {
                t.tool_call("read", json!({"path": "a.rs", "size": 24_000}))
                    .usage(100, 10)
            })
            // The scripted model adds the new output, about 6,000
            // tokens, to what it reports.
            .turn(|t| t.text("done").usage(100, 10))
            .turn(|t| t.text("forked"))
    };
    let at = |start_reserve_tokens: u64| json!({ "window": 10_000, "preserve_recent": 1, "start_reserve_tokens": start_reserve_tokens });
    for (reserve, asked) in [(1_000, 0), (5_000, 1)] {
        let model = read();
        let jev = FakeJev::nouls(|_| 0.1);
        block_on_io(async {
            let store = tau_store_sqlite::memory().await.unwrap();
            // The parent never prunes.
            let mut never = at(reserve);
            never["compact_at_percent"] = json!(100);
            let parent = Agent::new(model.clone())
                .tool(Read::new())
                .plugin(fast(never, &jev, &scratch("fork-parent")));
            let (_, base) = run(&parent, &store).await;
            assert!(jev.requests().is_empty());
            let child = Agent::new(model.clone())
                .tool(Read::new())
                .plugin(fast(at(reserve), &jev, &scratch("fork-child")));
            let fork = child
                .fork(&base.checkpoint())
                .run("and now", &store)
                .await
                .unwrap();
            assert_eq!(fork.text, "forked");
            assert_eq!(jev.requests().len(), asked, "reserve {reserve}");
        });
    }
}
