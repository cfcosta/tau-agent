//! tau-tree-compaction inside the agent loop
//! (`docs/reference/tree-compaction.md`), its Luau run by
//! tau-luau-plugins as tau runs it, in an `Agent` with `ScriptedModel` and
//! an in-memory store. Time is real: a hook's limit is a timer, which
//! paused time would let run out while the store works on a thread of its
//! own.
//!
//! The scripts steer a second user message in before the first turn
//! ends, so the transcript has a clean cut point: keeping 1 token, the
//! newest user message is kept and everything before it is folded. A turn
//! reporting 5,000 tokens against a 1,000-token window makes compaction
//! due.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::sync::{
    Arc,
    OnceLock,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use futures_util::StreamExt;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
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
use tau_codemode_host::{ToolCall, ToolEntry, ToolReply};
use tau_luau_plugins::Record;
use tau_luau_plugins_host::{
    agent::{Active, LuauPlugins},
    runtime::{Context, Hook, Loaded, Reach, load},
};
use tau_store::{Entry, Store};
use tau_testing::{block_on_io, scripted::ScriptedModel};
use tokio_util::sync::CancellationToken;

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

/// `future`'s output, on a thread of its own: the tests build their
/// agents inside the runtime that drives them.
fn aside<T: Send>(future: impl std::future::Future<Output = T> + Send) -> T {
    std::thread::scope(|scope| {
        scope.spawn(|| block_on_io(future)).join().unwrap()
    })
}

/// The plugin of `files` called `name`, loaded once as tau loads it.
fn loaded_of(cell: &'static OnceLock<Loaded>, name: &str) -> Loaded {
    cell.get_or_init(|| {
        let files = tau_luau_plugins_host::built_in::files(name).unwrap();
        aside(load(name, files)).unwrap()
    })
    .clone()
}

fn tree() -> Loaded {
    static LOADED: OnceLock<Loaded> = OnceLock::new();
    loaded_of(&LOADED, tau_luau_plugins::names::TREE_COMPACTION)
}

fn summary() -> Loaded {
    static LOADED: OnceLock<Loaded> = OnceLock::new();
    loaded_of(&LOADED, tau_luau_plugins::names::COMPACTION)
}

/// `loaded` with `settings` over its defaults.
fn active(loaded: Loaded, settings: &Value) -> Active {
    let mut merged = loaded.declaration.default_settings();
    for (key, value) in settings.as_object().unwrap() {
        merged[key] = value.clone();
    }
    Active {
        loaded,
        settings: merged,
    }
}

/// The test's settings: on, a window of 1,000 tokens, 100 of them
/// reserved, and 1 kept.
fn settings() -> Value {
    json!({ "on": true, "window": 1_000, "reserve_tokens": 100, "keep_recent_tokens": 1 })
}

/// The plugins, as tau runs them, the run on `model`.
fn plugins(active: Vec<Active>, model: &str) -> LuauPlugins {
    LuauPlugins::new(
        active,
        json!({ "kind": "main", "repo": "r", "model": model }),
        None,
    )
}

fn agent(llm: &ScriptedModel) -> Agent {
    Agent::new(llm.clone())
        .tool(typed(Read))
        .plugin(plugins(vec![active(tree(), &settings())], "m"))
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

/// Which Luau plugin made each rewrite, as its stored details say.
async fn rewrites(store: &Store, run: &str) -> Vec<String> {
    store
        .transcript(run)
        .await
        .unwrap()
        .iter()
        .filter_map(|entry| match entry {
            Entry::Context { body, .. } => {
                let body: Value = serde_json::from_str(body).unwrap();
                Some(body["plugin"].as_str().unwrap().to_owned())
            }
            _ => None,
        })
        .collect()
}

/// The tree's last state in the run `run`.
async fn state(store: &Store, run: &str) -> Value {
    store
        .records(run, tau_luau_plugins::NAME)
        .await
        .unwrap()
        .iter()
        .rev()
        .find_map(|body| match serde_json::from_str(body) {
            Ok(Record::State { plugin, state })
                if plugin == tau_luau_plugins::names::TREE_COMPACTION =>
            {
                Some(state)
            }
            _ => None,
        })
        .unwrap_or_default()
}

/// The labelled lines of a view message.
fn lines_of(view: &str) -> Vec<String> {
    let start = view.find("<chat>\n").expect("a view") + "<chat>\n".len();
    view[start..]
        .trim_end_matches("\n</chat>")
        .lines()
        .map(str::to_owned)
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
    block_on_io(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let agent = Agent::new(llm.clone())
            .tool(typed(Read))
            .plugin(plugins(vec![active(tree(), &settings())], "gpt-5.4-mini"))
            .model("gpt-5.4-mini")
            .reasoning(ReasoningEffort::High)
            .clock(Arc::new(|| 1_234));
        let (events, outcome) =
            run(&agent, &store, "go", "and then this").await;
        assert_eq!(outcome.text, "done");
        assert!(events.iter().any(|event| matches!(
            event,
            RunEvent::ContextRewritten { plugin, .. }
                if &**plugin == tau_luau_plugins::NAME
        )));
        assert_eq!(
            rewrites(&store, &outcome.run.0).await,
            [tau_luau_plugins::names::TREE_COMPACTION]
        );
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
        let instructions = compactor.settings.instructions.as_deref().unwrap();
        assert!(instructions.starts_with("You keep the memory of tau"));
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
        let lines = lines_of(&text(&next[0]));
        assert_eq!(lines[0], "0+1|user: go");
        assert_eq!(lines[1], r#"1+1|tool: read {"path":"src/lib.rs"}"#);
        assert!(lines[2].starts_with("2+1|echo: read gave "), "{}", lines[2]);
        assert_eq!(lines.len(), 3);
        assert_eq!(text(&next[1]), "and then this");
        assert_eq!(next.len(), 2);
        // What the plugin list reads.
        let state = state(&store, &outcome.run.0).await;
        assert_eq!(state["compacted"]["messages"], 3);
        assert_eq!(state["compacted"]["asked"], 1);
        llm.assert_exhausted();
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
    block_on_io(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let run = agent(&llm).start("go", &store);
        run.steer("and then this");
        run.steer("and also this");
        let outcome = run.outcome().await.unwrap();
        assert_eq!(outcome.text, "done");
        let requests = llm.requests();
        let context = text(&requests[3].transcript[0]);
        assert!(
            context.starts_with("<chat>\nuser: go\ntool: read"),
            "{context}"
        );
        let view = text(&requests[4].transcript[0]);
        let labels: Vec<String> = lines_of(&view)
            .iter()
            .filter_map(|line| {
                line.split_once('|').map(|(label, _)| label.to_owned())
            })
            .collect();
        // go, read a, its result; and then this, read b, its result.
        assert_eq!(labels, ["0+1", "1+1", "2+1", "3+1", "4+1", "5+1"]);
        assert!(view.contains("3+1|user: and then this"));
        // The second compaction says it folded three messages, not
        // counting the view it opened with.
        let state = state(&store, &outcome.run.0).await;
        assert_eq!(state["compactions"], 2);
        assert_eq!(state["compacted"]["messages"], 3);
    });
}

/// A fork of a compacted run starts from the view and can still zoom
/// into what was folded, down to a message whole: its history comes back
/// from the state it inherits.
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
        .turn(|t| t.tool_call("zoom", json!({"id": 0, "n": 2})).usage(10, 10))
        .turn(|t| t.tool_call("zoom", json!({"id": 1, "n": 2})).usage(10, 10))
        .turn(|t| t.text("forked").usage(10, 10));
    block_on_io(async {
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
        // A leaf opens to its message whole, a pair to its two lines, and
        // a line that is not one says so.
        let results: Vec<String> = requests
            .last()
            .unwrap()
            .transcript
            .iter()
            .filter(|message| matches!(message, Message::ToolResult(_)))
            .map(text)
            .collect();
        assert_eq!(
            results,
            [
                format!("2+1|echo: [read] {}", contents("src/lib.rs")),
                "0+1|user: go\n1+1|tool: read {\"path\":\"src/lib.rs\"}"
                    .to_owned(),
                "No line 1+2.".to_owned(),
            ]
        );
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
    block_on_io(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let (_, outcome) =
            run(&agent(&llm), &store, "go", "and then this").await;
        assert_eq!(outcome.text, "done");
        assert!(rewrites(&store, &outcome.run.0).await.is_empty());
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

/// Off, the run is not offered `zoom`, and nothing folds.
#[test]
fn off_there_is_no_zoom() {
    let llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call("read", json!({"path": "src/lib.rs"}))
                .usage(5_000, 10)
        })
        .turn(|t| t.text("done").usage(10, 10));
    block_on_io(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let mut off = settings();
        off["on"] = json!(false);
        let agent = Agent::new(llm.clone())
            .tool(typed(Read))
            .plugin(plugins(vec![active(tree(), &off)], "m"));
        let (_, outcome) = run(&agent, &store, "go", "and then this").await;
        assert_eq!(outcome.text, "done");
        assert!(
            llm.requests()[0]
                .settings
                .tools
                .iter()
                .all(|tool| tool.name != "zoom")
        );
        assert!(rewrites(&store, &outcome.run.0).await.is_empty());
        llm.assert_exhausted();
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
    block_on_io(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let (_, outcome) =
            run(&agent(&llm), &store, "go", "and then this").await;
        let entries = store.transcript(&outcome.run.0).await.unwrap();
        let Entry::Context { plugin, body, .. } = &entries[0] else {
            panic!("{entries:?}")
        };
        assert_eq!(plugin, tau_luau_plugins::NAME);
        let body: Value = serde_json::from_str(body).unwrap();
        assert_eq!(body["plugin"], tau_luau_plugins::names::TREE_COMPACTION);
        let details = &body["details"];
        assert_eq!(details["entries"], 3);
        assert_eq!(details["view"].as_array().unwrap().len(), 3);
        assert!(details["tokens_before"].as_u64().unwrap() >= 5_010);
        let next = &llm.requests()[2].transcript;
        let kept = state(&store, &outcome.run.0).await;
        assert_eq!(kept["history"]["entries"].as_array().unwrap().len(), 3);
        assert_eq!(kept["history"]["view"], details["view"]);
        assert!(text(&next[0]).ends_with(&format!(
            "<chat>\n{}\n</chat>",
            lines_of(&text(&next[0])).join("\n")
        )));
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
    block_on_io(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let (_, outcome) =
            run(&agent(&llm), &store, "go", "and then this").await;
        assert_eq!(outcome.text, "done");
        assert_eq!(
            rewrites(&store, &outcome.run.0).await,
            [tau_luau_plugins::names::TREE_COMPACTION]
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
    block_on_io(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let both = plugins(
            vec![
                active(tree(), &settings()),
                active(
                    summary(),
                    &json!({ "window": 1_000, "reserve_tokens": 100, "keep_recent_tokens": 1 }),
                ),
            ],
            "m",
        );
        let agent = Agent::new(llm.clone()).tool(typed(Read)).plugin(both);
        let (_, outcome) = run(&agent, &store, "go", "and then this").await;
        assert_eq!(outcome.text, "done");
        assert_eq!(
            rewrites(&store, &outcome.run.0).await,
            [tau_luau_plugins::names::COMPACTION]
        );
        let logged = store
            .records(&outcome.run.0, tau_luau_plugins::NAME)
            .await
            .unwrap()
            .iter()
            .any(|body| {
                matches!(serde_json::from_str(body),
                    Ok(Record::Log { plugin, lines })
                        if plugin == tau_luau_plugins::names::TREE_COMPACTION
                            && lines[0].starts_with("the compactor failed on"))
            });
        assert!(logged, "the tree's failure is in its log");
        llm.assert_exhausted();
    });
}

/// The model `infer` asks, counting how many asks are in flight at once.
struct Counting {
    running: AtomicUsize,
    most: AtomicUsize,
    asked: AtomicUsize,
}

#[async_trait]
impl Reach for Counting {
    fn tools(&self) -> Vec<ToolEntry> {
        vec![ToolEntry {
            name: "infer".into(),
            description: "Asks the model one task.".into(),
            input_schema: json!({ "type": "object" }),
            output_schema: None,
            namespace: None,
            sequential: false,
        }]
    }

    async fn call(&self, _: ToolCall) -> Result<ToolReply, String> {
        let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
        self.most.fetch_max(now, Ordering::SeqCst);
        // Lets the other asks in flight start before this answers.
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        self.asked.fetch_add(1, Ordering::SeqCst);
        self.running.fetch_sub(1, Ordering::SeqCst);
        Ok(ToolReply::success(
            json!({ "ok": true, "value": "echo: a line" }),
        ))
    }
}

/// No more than `jobs` lines are asked for at once, and every slot is
/// used when there is work for it.
#[test]
fn no_more_than_jobs_requests_run_at_once() {
    for (jobs, count) in [(1, 3), (3, 7), (6, 20)] {
        let loaded = tree();
        let messages: Vec<Value> = (0..count)
            .map(|id| {
                json!({
                    "index": id + 1,
                    "role": "toolResult",
                    "text": format!("{id} {}", "z".repeat(600)),
                    "tokens": 1,
                    "call": "c",
                    "name": "read",
                    "ok": true,
                })
            })
            .chain([json!({ "index": count + 1, "role": "user", "text": "next", "tokens": 1 })])
            .collect();
        let mut settings = loaded.declaration.default_settings();
        settings["on"] = json!(true);
        settings["jobs"] = json!(jobs);
        settings["keep_recent_tokens"] = json!(1);
        settings["view_bytes"] = json!(1_000_000);
        let context = Context {
            run: json!({ "id": "r", "kind": "main" }),
            now: json!({ "unix": 0, "iso": "1970-01-01T00:00:00Z", "weekday": "Thursday" }),
            settings,
            state: json!({}),
            secrets: None,
            store: None,
            job: None,
        };
        let counting = Arc::new(Counting {
            running: AtomicUsize::new(0),
            most: AtomicUsize::new(0),
            asked: AtomicUsize::new(0),
        });
        let outcome = block_on_io(loaded.call(
            &Hook::RewriteContext,
            json!({ "trigger": "overflow", "turn": 1, "tokens": 5_000, "messages": messages }),
            &context,
            counting.clone(),
            CancellationToken::new(),
        ));
        assert_eq!(outcome.error, None);
        assert!(!outcome.value["rewrite"].is_null());
        assert_eq!(counting.asked.load(Ordering::SeqCst), count);
        let most = counting.most.load(Ordering::SeqCst);
        assert_eq!(most, jobs, "{most} at once, {jobs} allowed");
    }
}
