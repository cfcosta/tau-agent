//! Tree compaction inside the agent loop (`docs/reference/tree-compaction.md`),
//! as a plugin of an `Agent` with `ScriptedModel` and an in-memory
//! store.
//!
//! The scripts steer a second user message in before the first turn
//! ends, so the transcript has a clean cut point: with
//! `keep_recent_tokens(1)` the newest user message is kept and
//! everything before it is folded. A turn reporting 5,000 tokens against
//! a 1,000-token window makes compaction due.

use async_trait::async_trait;
use futures_util::StreamExt;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use tau_agent::{
    agent::{Agent, Outcome},
    error::ToolError,
    event::RunEvent,
    tool::{ToolCtx, ToolOutput, TypedTool, typed},
};
use tau_ai::{
    message::{
        AssistantBlock,
        AssistantMessage,
        InputBlock,
        Message,
        StopReason,
        TextContent,
        Usage,
        UserContent,
    },
    responses::request::ReasoningEffort,
};
use tau_store::{Entry, Store};
use tau_testing::{block_on, scripted::ScriptedModel};
use tau_tree_compaction::{
    Details,
    NAME,
    Record,
    TreeCompaction,
    VIEW_PREFIX,
    build::COMPACT_PROMPT,
    view_message,
};

/// What `read` returns: long enough that its line needs the model.
fn contents(path: &str) -> String {
    format!("contents of {path}: {}", "fn main() {} ".repeat(60))
}

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
        _: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::text(contents(&args.path)))
    }
}

fn settings() -> TreeCompaction {
    TreeCompaction::default()
        .context_window(1_000)
        .reserve_tokens(100)
        .keep_recent_tokens(1)
}

fn agent(llm: &ScriptedModel) -> Agent {
    Agent::new(llm.clone()).tool(typed(Read)).plugin(settings())
}

/// A compactor's answer: a line naming the step it was asked, so the
/// test can tell lines apart.
fn line(transcript: &[Message]) -> AssistantMessage {
    let Message::User(user) = &transcript[0] else {
        panic!("a compactor request opens with its step")
    };
    let UserContent::Blocks(blocks) = &user.content else {
        panic!("context and step blocks")
    };
    let InputBlock::Text(step) = &blocks[1] else {
        panic!("text")
    };
    let asked = step.text.lines().last().unwrap_or_default();
    let text = format!("echo: read gave {} bytes", asked.len());
    AssistantMessage {
        content: vec![AssistantBlock::Text(TextContent {
            text,
            text_signature: None,
        })],
        model: "m".into(),
        response_id: None,
        usage: Usage::default(),
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: 0,
    }
}

fn text(message: &Message) -> String {
    match message {
        Message::User(user) => match &user.content {
            UserContent::Text(text) => text.clone(),
            UserContent::Blocks(blocks) => tau_ai::message::text_of(blocks),
        },
        Message::ToolResult(result) => {
            tau_ai::message::text_of(&result.content)
        }
        Message::Assistant(reply) => reply.text(),
    }
}

async fn run(
    agent: &Agent,
    store: &Store,
    input: &str,
    steer: &str,
) -> (Vec<RunEvent>, Outcome) {
    let mut run = agent.start(input, store);
    run.steer(steer);
    let events = run.events().collect().await;
    (events, run.outcome().await.unwrap())
}

fn rewrites(events: &[RunEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            RunEvent::ContextRewritten { plugin, .. } => {
                Some(plugin.to_string())
            }
            _ => None,
        })
        .collect()
}

/// Past the threshold, the messages before the kept one fold into the
/// history: a short one is its own line, word for word, a long one is
/// compressed by a request of its own, and the next turn opens with the
/// view and then the kept message. The run declares `zoom`.
#[test]
fn older_messages_fold_into_the_view() {
    let llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call("read", json!({"path": "src/lib.rs"}))
                .usage(5_000, 10)
        })
        .turn_with(line)
        .turn(|t| t.text("done").usage(10, 10));
    block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let agent = agent(&llm)
            .model("gpt-5.4-mini")
            .reasoning(ReasoningEffort::High)
            .clock(std::sync::Arc::new(|| 1_234));
        let (events, outcome) =
            run(&agent, &store, "go", "and then this").await;
        assert_eq!(outcome.text, "done");
        assert_eq!(rewrites(&events), [NAME]);
        let requests = llm.requests();
        assert_eq!(requests.len(), 3);
        assert!(
            requests[0]
                .settings
                .tools
                .iter()
                .any(|tool| tool.name == "zoom")
        );
        // The one request for a line: the result, whole.
        let compactor = &requests[1];
        assert_eq!(
            compactor.settings.instructions.as_deref(),
            Some(COMPACT_PROMPT)
        );
        // The run's clock stamps what the compactor is sent, and the
        // view.
        let stamp = |message: &Message| match message {
            Message::User(user) => user.timestamp,
            other => panic!("expected a user message, got {other:?}"),
        };
        assert_eq!(stamp(&compactor.transcript[0]), 1_234);
        assert_eq!(stamp(&requests[2].transcript[0]), 1_234);
        // The run's model and effort build its lines.
        assert_eq!(compactor.settings.model, "gpt-5.4-mini");
        assert_eq!(compactor.settings.reasoning, Some(ReasoningEffort::High));
        assert!(compactor.settings.tools.is_empty());
        assert!(
            text(&compactor.transcript[0])
                .ends_with(&format!("echo: [read] {}", contents("src/lib.rs")))
        );
        // The next turn: the view, then the kept message.
        let next = &requests[2].transcript;
        let view = text(&next[0]);
        assert!(view.starts_with(VIEW_PREFIX), "{view}");
        let lines: Vec<&str> = view[VIEW_PREFIX.len()..]
            .trim_end_matches("\n</chat>")
            .lines()
            .collect();
        assert_eq!(lines[0], "0+1|user: go");
        assert_eq!(lines[1], r#"1+1|tool: read {"path":"src/lib.rs"}"#);
        assert!(lines[2].starts_with("2+1|echo: read gave "), "{}", lines[2]);
        assert_eq!(lines.len(), 3);
        assert_eq!(text(&next[1]), "and then this");
        assert_eq!(next.len(), 2);
        // What the plugin list folds.
        assert!(events.iter().any(|event| matches!(event,
            RunEvent::PluginReport { plugin, body, .. }
                if &**plugin == NAME && body["kind"] == "compacted"
                    && body["messages"] == 3 && body["asked"] == 1)));
        llm.assert_exhausted();
    });
}

/// After a compaction, `zoom` opens a line down to its message, whole.
#[test]
fn zoom_gives_a_folded_message_back_whole() {
    let llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call("read", json!({"path": "src/lib.rs"}))
                .usage(5_000, 10)
        })
        .turn_with(line)
        .turn(|t| t.tool_call("zoom", json!({"id": 2, "n": 1})).usage(10, 10))
        .turn(|t| t.tool_call("zoom", json!({"id": 0, "n": 2})).usage(10, 10))
        .turn(|t| t.tool_call("zoom", json!({"id": 1, "n": 2})).usage(10, 10))
        .turn(|t| t.text("done").usage(10, 10));
    block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let (_, outcome) =
            run(&agent(&llm), &store, "go", "and then this").await;
        assert_eq!(outcome.text, "done");
        let last = llm.requests().pop().unwrap().transcript;
        let results: Vec<String> = last
            .iter()
            .filter(|message| matches!(message, Message::ToolResult(_)))
            .map(text)
            .collect();
        assert_eq!(
            results[0],
            format!("2+1|echo: [read] {}", contents("src/lib.rs"))
        );
        assert_eq!(
            results[1],
            "0+1|user: go\n1+1|tool: read {\"path\":\"src/lib.rs\"}"
        );
        assert_eq!(results[2], "No line 1+2.");
    });
}

/// A second compaction folds only what came since: the history keeps
/// its first messages' lines and ids, the view grows, and the new
/// lines' requests read the old lines as context.
#[test]
fn a_second_compaction_folds_only_what_came_since() {
    let llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call("read", json!({"path": "a.rs"}))
                .usage(5_000, 10)
        })
        .turn_with(line)
        .turn(|t| {
            t.tool_call("read", json!({"path": "b.rs"}))
                .usage(5_000, 10)
        })
        .turn_with(line)
        .turn(|t| t.text("done").usage(10, 10));
    block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let run = agent(&llm).start("go", &store);
        run.steer("and then this");
        run.steer("and also this");
        let outcome = run.outcome().await.unwrap();
        assert_eq!(outcome.text, "done");
        let outcome_run = outcome.run.0.clone();
        let requests = llm.requests();
        let second = &requests[3];
        let context = text(&second.transcript[0]);
        assert!(
            context.starts_with("<chat>\nuser: go\ntool: read"),
            "{context}"
        );
        let view = text(&requests[4].transcript[0]);
        let labels: Vec<&str> = view[VIEW_PREFIX.len()..]
            .lines()
            .filter_map(|line| line.split_once('|').map(|(label, _)| label))
            .collect();
        // go, read a, its result; and then this, read b, its result.
        assert_eq!(labels, ["0+1", "1+1", "2+1", "3+1", "4+1", "5+1"]);
        assert!(view.contains("3+1|user: and then this"));
        // Each compaction says how many messages it folded: the second
        // not counting the view it opened with.
        let folded: Vec<u64> = store
            .records(&outcome_run, NAME)
            .await
            .unwrap()
            .iter()
            .filter_map(|body| match serde_json::from_str(body).unwrap() {
                Record::Compacted { messages, .. } => Some(messages as u64),
                _ => None,
            })
            .collect();
        assert_eq!(folded, [3, 3]);
    });
}

/// A fork of a compacted run starts from the view and can still zoom
/// into what was folded: its history comes back from the run's records.
#[test]
fn a_fork_of_a_compacted_run_can_zoom_into_the_history() {
    let llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call("read", json!({"path": "src/lib.rs"}))
                .usage(5_000, 10)
        })
        .turn_with(line)
        .turn(|t| t.text("done").usage(10, 10))
        .turn(|t| t.tool_call("zoom", json!({"id": 2, "n": 1})).usage(10, 10))
        .turn(|t| t.text("forked").usage(10, 10));
    block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let agent = agent(&llm);
        let (_, base) = run(&agent, &store, "go", "and then this").await;
        let fork = agent
            .fork(&base.checkpoint())
            .run("and now", &store)
            .await
            .unwrap();
        assert_eq!(fork.text, "forked");
        let requests = llm.requests();
        // The fork opens with the base's view.
        assert_eq!(requests[3].transcript[0], requests[2].transcript[0]);
        let last = &requests[4].transcript;
        let Some(Message::ToolResult(result)) = last.last() else {
            panic!("the zoom's result")
        };
        assert_eq!(
            tau_ai::message::text_of(&result.content),
            format!("2+1|echo: [read] {}", contents("src/lib.rs"))
        );
        llm.assert_exhausted();
    });
}

/// When a line cannot be built, the tree writes nothing and
/// tau-compaction, after it, summarizes instead.
#[test]
fn a_failed_tree_leaves_the_summary_to_tau_compaction() {
    let llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call("read", json!({"path": "src/lib.rs"}))
                .usage(5_000, 10)
        })
        .turn(|t| t.error("invalid_prompt", "refused"))
        .turn(|t| t.text("## Goal\nship it"))
        .turn(|t| t.text("done").usage(10, 10));
    block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let agent = agent(&llm).plugin(
            tau_compaction::Compaction::default()
                .context_window(1_000)
                .reserve_tokens(100)
                .keep_recent_tokens(1),
        );
        let (events, outcome) =
            run(&agent, &store, "go", "and then this").await;
        assert_eq!(outcome.text, "done");
        assert_eq!(rewrites(&events), [tau_compaction::NAME]);
        assert!(events.iter().any(|event| matches!(event,
            RunEvent::PluginError { plugin, .. } if &**plugin == NAME)));
        llm.assert_exhausted();
    });
}

/// Below the threshold nothing is folded, and `zoom` says there is
/// nothing to open.
#[test]
fn below_the_threshold_nothing_folds() {
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("zoom", json!({"id": 0, "n": 1})).usage(10, 10))
        .turn(|t| t.text("done").usage(10, 10));
    block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let (events, outcome) =
            run(&agent(&llm), &store, "go", "and then this").await;
        assert_eq!(outcome.text, "done");
        assert!(rewrites(&events).is_empty());
        let last = llm.requests().pop().unwrap().transcript;
        let Some(Message::ToolResult(result)) = last
            .iter()
            .find(|message| matches!(message, Message::ToolResult(_)))
        else {
            panic!("the zoom's result")
        };
        assert!(result.is_error);
        assert_eq!(
            tau_ai::message::text_of(&result.content),
            "Nothing has been compacted yet: there is no line to open."
        );
    });
}

/// The stored rewrite holds the view and how many entries it covers,
/// and its message is the view message the next turn opens with.
#[test]
fn the_rewrite_keeps_the_view() {
    let llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call("read", json!({"path": "src/lib.rs"}))
                .usage(5_000, 10)
        })
        .turn_with(line)
        .turn(|t| t.text("done").usage(10, 10));
    block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let (_, outcome) =
            run(&agent(&llm), &store, "go", "and then this").await;
        let entries = store.transcript(&outcome.run.0).await.unwrap();
        let Entry::Context { plugin, body, .. } = &entries[0] else {
            panic!("{entries:?}")
        };
        assert_eq!(plugin, NAME);
        let details: Details = serde_json::from_str(body).unwrap();
        assert_eq!(details.entries, 3);
        assert_eq!(details.view.len(), 3);
        assert!(details.tokens_before >= 5_010);
        let next = &llm.requests()[2].transcript;
        let records = store.records(&outcome.run.0, NAME).await.unwrap();
        let folded: Vec<Record> = records
            .iter()
            .map(|body| serde_json::from_str(body).unwrap())
            .collect();
        let Some(Record::Folded {
            first: 0,
            entries,
            nodes,
        }) = folded.first()
        else {
            panic!("{folded:?}")
        };
        assert_eq!(entries.len(), 3);
        let history = tau_tree_compaction::tree::History::restore(
            entries.clone(),
            nodes.clone(),
            details.view.clone(),
        );
        assert_eq!(view_message(&history, details.timestamp), next[0]);
    });
}

/// After a failed compaction, tree compaction waits two turns, then
/// tries again: a failing compactor is not paid for every turn.
#[test]
fn a_failed_compaction_is_tried_again_after_a_wait() {
    let read = |path: &'static str| {
        move |t: tau_testing::scripted::TurnBuilder| {
            t.tool_call("read", json!({"path": path})).usage(5_000, 10)
        }
    };
    let llm = ScriptedModel::new()
        .turn(read("a.rs"))
        .turn(|t| t.error("invalid_prompt", "refused"))
        // Turn 2 is inside the wait: nothing is asked.
        .turn(read("b.rs"))
        .turn(read("c.rs"))
        // Turn 3: the results of a.rs and b.rs need a line each.
        .turn_with(line)
        .turn_with(line)
        .turn(|t| t.text("done").usage(10, 10));
    block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let (events, outcome) =
            run(&agent(&llm), &store, "go", "and then this").await;
        assert_eq!(outcome.text, "done");
        assert_eq!(rewrites(&events), [NAME]);
        llm.assert_exhausted();
    });
}

/// The settings' builders set what they name.
#[test]
fn the_builders_set_their_fields() {
    let settings = TreeCompaction::default()
        .reserve_tokens(1)
        .keep_recent_tokens(2)
        .context_window(3)
        .view_bytes(4)
        .jobs(5);
    assert_eq!(
        (
            settings.reserve_tokens,
            settings.keep_recent_tokens,
            settings.context_window,
            settings.view_bytes,
            settings.jobs
        ),
        (1, 2, Some(3), 4, 5)
    );
}
