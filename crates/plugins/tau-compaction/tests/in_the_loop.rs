//! Compaction inside the agent loop (`docs/reference/compaction.md`),
//! as a plugin of an `Agent` with `ScriptedModel`, `tau_store_sqlite::memory()` and
//! paused time.
//!
//! The scripts steer a second user message in before the first turn
//! ends, so the transcript has a clean cut point: with
//! `keep_recent_tokens(1)` the newest user message is kept and
//! everything before it is summarized.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use tau_agent::{
    agent::Agent,
    error::ToolError,
    event::{RunEvent, StopReason},
    tool::{ToolCtx, ToolOutput, TypedTool, typed},
};
use tau_ai::{
    message::{Message, StopReason as MessageStop, UserContent},
    responses::request::ReasoningEffort,
};
use tau_compaction::{
    Compaction,
    Record,
    SUMMARIZATION_SYSTEM_PROMPT,
    SUMMARY_PREFIX,
    TURN_PREFIX_SUMMARIZATION_PROMPT,
    UPDATE_SUMMARIZATION_PROMPT,
};
use tau_store::{Entry, Store};
use tau_testing::{block_on, scripted::ScriptedModel};

struct Read;

#[derive(Deserialize, JsonSchema)]
struct ReadArgs {
    path: String,
}

#[async_trait]
impl TypedTool for Read {
    type Args = ReadArgs;
    const NAME: &'static str = "read";
    const DESCRIPTION: &'static str = "Reads a file.";
    async fn call(
        &self,
        args: ReadArgs,
        _ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::text(format!("contents of {}", args.path)))
    }
}

fn settings() -> Compaction {
    Compaction::default()
        .context_window(1_000)
        .reserve_tokens(100)
        .keep_recent_tokens(1)
}

fn agent(llm: &ScriptedModel) -> Agent {
    Agent::new(llm.clone()).tool(typed(Read)).plugin(settings())
}

fn text(message: &Message) -> String {
    match message {
        Message::User(user) => match &user.content {
            UserContent::Text(text) => text.clone(),
            UserContent::Blocks(_) => panic!("text only"),
        },
        other => panic!("expected a user message, got {other:?}"),
    }
}

/// Starts `input`, steers `steer` in, and collects the events and the
/// outcome.
async fn run(
    agent: &Agent,
    store: &Store,
    input: &str,
    steer: &str,
) -> (Vec<RunEvent>, tau_agent::agent::Outcome) {
    use futures_util::StreamExt;
    let mut run = agent.start(input, store);
    run.steer(steer);
    let events = run.events().collect().await;
    (events, run.outcome().await.unwrap())
}

/// Past the threshold, the loop summarizes everything before the kept
/// messages in a request of its own, then goes on from the summary. The
/// summary request carries pi's system prompt, the output budget and the
/// serialized conversation; the next turn starts from the summary and
/// the kept message; the store holds the compaction record, pointing at
/// the kept message, and the summary; a `ContextRewritten` event from the compaction
/// plugin reports the size before; the summary's usage counts toward the
/// run.
#[test]
fn threshold_compaction_summarizes_older_messages() {
    let llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call("read", json!({"path": "src/lib.rs"}))
                .usage(5_000, 10)
                .cost(0.5)
        })
        .turn(|t| t.text("## Goal\nship it").cost(0.25))
        .turn(|t| t.text("done"));
    block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let agent = agent(&llm)
            .model("gpt-5.4-mini")
            .reasoning(ReasoningEffort::High);
        let (events, outcome) =
            run(&agent, &store, "go", "and then this").await;
        assert_eq!(outcome.stop, StopReason::Stop);
        assert_eq!(outcome.text, "done");
        assert_eq!(outcome.usage.cost.total, 0.75);
        // Seven rows: the input, the call and its result, the steered
        // message, the context entry, the summary, the answer. The
        // context entry keeps the steered message by reference.
        assert_eq!(outcome.checkpoint().seq(), 6);
        let compacted: Vec<u64> = events
            .iter()
            .filter_map(|e| match e {
                RunEvent::ContextRewritten {
                    tokens_before,
                    plugin,
                    ..
                } => {
                    assert_eq!(&**plugin, tau_compaction::NAME);
                    Some(*tokens_before)
                }
                _ => None,
            })
            .collect();
        assert_eq!(compacted.len(), 1);
        assert!(compacted[0] >= 5_010, "{compacted:?}");

        let requests = llm.requests();
        assert_eq!(requests.len(), 3);
        let summary = &requests[1];
        assert_eq!(
            summary.settings.instructions.as_deref(),
            Some(SUMMARIZATION_SYSTEM_PROMPT)
        );
        assert!(summary.settings.tools.is_empty());
        assert_eq!(summary.settings.model, "gpt-5.4-mini");
        assert_eq!(summary.settings.reasoning, Some(ReasoningEffort::High));
        assert_eq!(summary.transcript.len(), 1);
        let asked = text(&summary.transcript[0]);
        assert!(asked.starts_with("<conversation>\n[User]: go"), "{asked}");
        assert!(asked.contains("read(path=\"src/lib.rs\")"), "{asked}");
        assert!(!asked.contains("and then this"), "{asked}");

        let next = &requests[2].transcript;
        assert_eq!(next.len(), 2);
        let opening = text(&next[0]);
        assert!(opening.starts_with(SUMMARY_PREFIX), "{opening}");
        assert!(opening.contains("## Goal\nship it"), "{opening}");
        assert!(
            opening.contains("<read-files>\nsrc/lib.rs\n</read-files>"),
            "{opening}"
        );
        assert_eq!(text(&next[1]), "and then this");

        let entries = store.transcript(&outcome.run.0).await.unwrap();
        let Entry::Context { plugin, body, .. } = &entries[0] else {
            panic!("{entries:?}")
        };
        assert_eq!(plugin, tau_compaction::NAME);
        let record: Record = serde_json::from_str(body).unwrap();
        assert_eq!(record.message(), next[0]);
        assert_eq!(record.read_files, vec!["src/lib.rs".to_owned()]);
        // The context entry, the summary, the kept message, the answer.
        assert_eq!(entries.len(), 4);
        let record = store.run(&outcome.run.0).await.unwrap().unwrap();
        assert_eq!(record.cost_usd, 0.75);
    });
}

/// A second compaction updates the first summary: it uses pi's "update"
/// prompt with the previous summary, never summarizes the previous
/// summary message as conversation, and carries the file lists forward.
#[test]
fn a_second_compaction_updates_the_first_summary() {
    let llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call("read", json!({"path": "a.rs"}))
                .usage(5_000, 10)
        })
        .turn(|t| t.text("first summary"))
        .turn(|t| {
            t.tool_call("read", json!({"path": "b.rs"}))
                .usage(5_000, 10)
        })
        .turn(|t| t.text("second summary"))
        .turn(|t| t.text("done"));
    block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let agent = agent(&llm);
        let run = agent.start("go", &store);
        run.steer("then b");
        run.steer("then finish");
        let outcome = run.outcome().await.unwrap();
        assert_eq!(outcome.text, "done");

        let requests = llm.requests();
        let update = text(&requests[3].transcript[0]);
        assert!(update.ends_with(UPDATE_SUMMARIZATION_PROMPT), "{update}");
        assert!(
            update.contains("<previous-summary>\nfirst summary"),
            "{update}"
        );
        assert!(!update.contains(SUMMARY_PREFIX), "{update}");
        let last = text(&requests[4].transcript[0]);
        assert!(last.contains("second summary"), "{last}");
        assert!(
            last.contains("<read-files>\na.rs\nb.rs\n</read-files>"),
            "{last}"
        );
        // Loading the run drops everything before the latest record.
        let entries = store.transcript(&outcome.run.0).await.unwrap();
        let compactions = entries
            .iter()
            .filter(|e| matches!(e, Entry::Context { .. }))
            .count();
        assert_eq!(compactions, 1);
    });
}

/// When the cut falls inside the only turn, its start is summarized
/// with the turn-prefix prompt and merged under the history, which is
/// "No prior history.".
#[test]
fn a_split_turn_gets_a_prefix_summary() {
    let llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call("read", json!({"path": "a.rs"}))
                .usage(5_000, 10)
        })
        .turn(|t| {
            t.tool_call("read", json!({"path": "b.rs"}))
                .usage(5_000, 10)
        })
        .turn(|t| t.text("prefix summary"))
        .turn(|t| t.text("done"));
    block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        // Keep the last tool call and its result: the cut lands on the
        // second assistant message, inside the first turn.
        let agent = Agent::new(llm.clone())
            .tool(typed(Read))
            .plugin(settings().keep_recent_tokens(10));
        let outcome = agent.run("go", &store).await.unwrap();
        assert_eq!(outcome.text, "done");
        let requests = llm.requests();
        let asked = text(&requests[2].transcript[0]);
        assert!(asked.ends_with(TURN_PREFIX_SUMMARIZATION_PROMPT), "{asked}");
        let opening = text(&requests[3].transcript[0]);
        assert!(
            opening.contains(
                "No prior history.\n\n---\n\n**Turn Context (split turn):**\n\nprefix summary"
            ),
            "{opening}"
        );
    });
}

/// A context overflow compacts once and retries the turn once. The
/// failed response is not stored.
#[test]
fn an_overflow_compacts_and_retries_once() {
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("read", json!({"path": "a.rs"})))
        .turn(|t| {
            t.error(
                "context_length_exceeded",
                "Your input exceeds the context window of this model",
            )
        })
        .turn(|t| t.text("summary"))
        .turn(|t| t.text("done"));
    block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        // The threshold never fires: the window is huge.
        let agent = Agent::new(llm.clone())
            .tool(typed(Read))
            .plugin(settings().context_window(u64::MAX));
        let (_, outcome) = run(&agent, &store, "go", "then this").await;
        assert_eq!(outcome.stop, StopReason::Stop);
        assert_eq!(outcome.text, "done");
        let requests = llm.requests();
        assert_eq!(requests.len(), 4);
        assert_eq!(requests[3].transcript.len(), 2);
        let stored = store.transcript(&outcome.run.0).await.unwrap();
        let errors =
            stored
                .iter()
                .filter(|e| match e {
                    Entry::Message { body, .. } => {
                        serde_json::from_str::<serde_json::Value>(body).unwrap()
                            ["stopReason"]
                            == "error"
                    }
                    _ => false,
                })
                .count();
        assert_eq!(errors, 0);
    });
}

/// A second overflow after compacting fails the run with it.
#[test]
fn a_second_overflow_fails_the_run() {
    let overflow = |t: tau_testing::scripted::TurnBuilder| {
        t.error("context_length_exceeded", "too long")
    };
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("read", json!({"path": "a.rs"})))
        .turn(overflow)
        .turn(|t| t.text("summary"))
        .turn(overflow);
    block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let agent = Agent::new(llm.clone())
            .tool(typed(Read))
            .plugin(settings().context_window(u64::MAX));
        let (_, outcome) = run(&agent, &store, "go", "then this").await;
        assert!(
            matches!(&outcome.stop, StopReason::Error(m) if m.contains("context_length_exceeded")),
            "{:?}",
            outcome.stop
        );
        llm.assert_exhausted();
    });
}

/// A rejected summary on overflow writes nothing and fails the run with
/// both errors: here the summary hit the token cap.
#[test]
fn a_rejected_summary_on_overflow_fails_the_run() {
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("read", json!({"path": "a.rs"})))
        .turn(|t| t.error("context_length_exceeded", "too long"))
        .turn(|t| t.text("cut off").stop(MessageStop::Length));
    block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let agent = Agent::new(llm.clone())
            .tool(typed(Read))
            .plugin(settings().context_window(u64::MAX));
        let (events, outcome) = run(&agent, &store, "go", "then this").await;
        let StopReason::Error(message) = &outcome.stop else {
            panic!("{:?}", outcome.stop)
        };
        assert!(message.contains("context_length_exceeded"), "{message}");
        assert!(
            message.contains("compaction failed: Summarization failed: generation hit the token cap"),
            "{message}"
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, RunEvent::ContextRewritten { .. }))
        );
        let stored = store.transcript(&outcome.run.0).await.unwrap();
        assert!(stored.iter().all(|e| matches!(e, Entry::Message { .. })));
    });
}

/// A rejected summary past the threshold writes nothing; the run goes
/// on uncompacted, and the next turn does not try again.
#[test]
fn a_rejected_summary_past_the_threshold_leaves_the_run_going() {
    let llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call("read", json!({"path": "a.rs"}))
                .usage(5_000, 10)
        })
        .turn(|t| t.tool_call("read", json!({"path": "x"})))
        .turn(|t| {
            t.tool_call("read", json!({"path": "b.rs"}))
                .usage(5_000, 10)
        })
        .turn(|t| t.text("done"));
    block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let (events, outcome) =
            run(&agent(&llm), &store, "go", "then this").await;
        assert_eq!(outcome.text, "done");
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, RunEvent::ContextRewritten { .. }))
        );
        let requests = llm.requests();
        assert_eq!(requests.len(), 4);
        // The turn after the rejected summary saw the whole transcript.
        assert_eq!(text(&requests[2].transcript[0]), "go");
        llm.assert_exhausted();
    });
}

/// After a rejected summary, compaction waits two turns, then tries
/// again: a run with no turn cap is not left uncompacted for good.
#[test]
fn a_rejected_summary_is_tried_again_after_a_wait() {
    let read = |path: &'static str| {
        move |t: tau_testing::scripted::TurnBuilder| {
            t.tool_call("read", json!({"path": path})).usage(5_000, 10)
        }
    };
    let llm = ScriptedModel::new()
        .turn(read("a.rs"))
        // Rejected: a summary is text.
        .turn(|t| t.tool_call("read", json!({"path": "x"})))
        .turn(read("b.rs"))
        .turn(read("c.rs"))
        // The history's summary, then the split turn's prefix.
        .turn(|t| t.text("## Goal\nship it"))
        .turn(|t| t.text("## Original Request\nread b.rs"))
        .turn(|t| t.text("done"));
    block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let (events, outcome) =
            run(&agent(&llm), &store, "go", "then this").await;
        assert_eq!(outcome.text, "done");
        let rewrites = events
            .iter()
            .filter(|e| matches!(e, RunEvent::ContextRewritten { .. }))
            .count();
        assert_eq!(rewrites, 1);
        llm.assert_exhausted();
    });
}

/// A fork of a compacted run starts from the summary: the store drops
/// everything before the record, and the fork's first request is the
/// summary, the kept messages, and the fork's input.
#[test]
fn a_fork_of_a_compacted_run_starts_from_the_summary() {
    let llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call("read", json!({"path": "a.rs"}))
                .usage(5_000, 10)
        })
        .turn(|t| t.text("summary"))
        .turn(|t| t.text("done"))
        .turn(|t| t.text("forked"));
    block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let agent = agent(&llm);
        let (_, base) = run(&agent, &store, "go", "then this").await;
        let fork = agent
            .fork(&base.checkpoint())
            .run("and now", &store)
            .await
            .unwrap();
        assert_eq!(fork.text, "forked");
        let requests = llm.requests();
        let seen = &requests[3].transcript;
        let mut expected = requests[2].transcript.clone();
        expected.truncate(2);
        assert_eq!(seen[..2], expected[..]);
        assert_eq!(text(&seen[3]), "and now");
        assert_eq!(seen.len(), 4);
    });
}

/// Below the threshold, and with no overflow, the loop never asks for a
/// summary, however many turns the run takes.
#[test]
fn no_compaction_below_the_threshold() {
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("read", json!({"path": "a.rs"})))
        .turn(|t| t.tool_call("read", json!({"path": "b.rs"})))
        .turn(|t| t.text("done"));
    block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let agent = Agent::new(llm.clone())
            .tool(typed(Read))
            .plugin(settings().context_window(u64::MAX));
        let (events, outcome) = run(&agent, &store, "go", "then this").await;
        assert_eq!(outcome.text, "done");
        assert_eq!(llm.requests().len(), 3);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, RunEvent::ContextRewritten { .. }))
        );
    });
}

/// An overflow is recognized by its wording too, when the failure has
/// no code that says so: here a failed response from a factory turn.
#[test]
fn an_overflow_is_recognized_by_its_wording() {
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("read", json!({"path": "a.rs"})))
        .turn_with(|_| tau_ai::message::AssistantMessage {
            content: Vec::new(),
            model: "gpt-5.5".into(),
            response_id: None,
            usage: Default::default(),
            stop_reason: MessageStop::Error,
            error_message: Some(
                "Your input exceeds the context window of this model".into(),
            ),
            timestamp: 0,
        })
        .turn(|t| t.text("summary"))
        .turn(|t| t.text("done"));
    block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let agent = Agent::new(llm.clone())
            .tool(typed(Read))
            .plugin(settings().context_window(u64::MAX));
        let (events, outcome) = run(&agent, &store, "go", "then this").await;
        assert_eq!(outcome.text, "done");
        assert!(
            events
                .iter()
                .any(|e| matches!(e, RunEvent::ContextRewritten { .. }))
        );
    });
}

/// A context overflow reported by its code (not its wording) compacts,
/// and the summary request goes through the retry policy too.
#[test]
fn an_overflow_code_compacts_and_the_summary_is_retried() {
    use futures_util::StreamExt;
    let llm = ScriptedModel::new()
        .turn(|t| t.text("first"))
        .turn(|t| t.error("context_length_exceeded", "no room"))
        .turn(|t| t.error("server_error", "try again"))
        .turn(|t| t.text("summary"))
        .turn(|t| t.text("done"));
    block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let agent = Agent::new(llm.clone()).plugin(
            Compaction::default()
                .context_window(u64::MAX)
                .keep_recent_tokens(1),
        );
        let mut run = agent.start("go", &store);
        run.steer("more");
        let events: Vec<RunEvent> = run.events().collect().await;
        let outcome = run.outcome().await.unwrap();
        assert_eq!(outcome.text, "done");
        assert!(
            events
                .iter()
                .any(|e| matches!(e, RunEvent::ContextRewritten { .. }))
        );
        llm.assert_exhausted();
    });
}

/// A summary request that keeps failing retryably is tried as many times
/// as the policy allows, and then compaction fails.
#[test]
fn a_failing_summary_is_tried_as_the_policy_allows() {
    let llm = ScriptedModel::new()
        .turn(|t| t.text("first"))
        .turn(|t| t.error("context_length_exceeded", "no room"))
        .turn(|t| t.error("server_error", "try again"))
        .turn(|t| t.error("server_error", "try again"))
        .turn(|t| t.text("never"));
    block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let policy = tau_ai::retry::RetryPolicy {
            max_attempts: 2,
            ..tau_ai::retry::RetryPolicy::default()
        };
        let agent = Agent::new(llm.clone()).retry(policy).plugin(
            Compaction::default()
                .context_window(u64::MAX)
                .keep_recent_tokens(1),
        );
        let run = agent.start("go", &store);
        run.steer("more");
        let outcome = run.outcome().await.unwrap();
        let StopReason::Error(message) = &outcome.stop else {
            panic!("{:?}", outcome.stop)
        };
        assert!(message.contains("compaction failed"), "{message}");
        assert_eq!(llm.requests().len(), 4);
        assert_eq!(llm.remaining(), 1);
    });
}
