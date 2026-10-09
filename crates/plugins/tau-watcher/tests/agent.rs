//! The watcher in an agent loop, driven with `ScriptedModel`: the 6th
//! step asks once, `learn: none` leaves no record, a note leaves one,
//! and a run without the plugin asks nothing.

use async_trait::async_trait;
use serde_json::{Value, json};
use tau_agent::{
    agent::Agent,
    error::ToolError,
    plugin::Plugin,
    tool::{AgentTool, ToolCtx, ToolOutput},
};
use tau_store::Store;
use tau_testing::{block_on_io, scripted::ScriptedModel};
use tau_ui_plugin::{HostHalf as _, RunKind, testing::run_ctx};
use tau_watcher::{
    WatcherHost,
    WatcherPlugin,
    prompt,
    record::{Answer, NAME, Record, Tag},
    state::State,
    ui::settings::Settings,
};

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

const NOTE: &str = "learn: `cargo test` rebuilds in debug, so each run takes minutes.\n\
tag: Heads up\n\
explain:\n\
**Debug tests rebuild everything**\n\
- `cargo test` builds in its own profile.\n\
- Nothing from `--release` is reused.\n\
- Each run recompiles every crate.";

/// `before` steps of tool calls, the side request's reply, `after` more
/// steps of tool calls, and the final answer.
fn script(before: usize, reply: Option<&str>, after: usize) -> ScriptedModel {
    let mut model = ScriptedModel::new();
    for n in 0..before {
        model = model.turn(|t| {
            t.tool_call("echo", json!({"text": format!("step {n}")}))
        });
    }
    if let Some(reply) = reply {
        let reply = reply.to_owned();
        model = model.turn(move |t| t.text(reply));
    }
    for n in 0..after {
        model = model.turn(|t| {
            t.tool_call("echo", json!({"text": format!("later {n}")}))
        });
    }
    model.turn(|t| t.text("done"))
}

fn watching(model: &ScriptedModel) -> Agent {
    Agent::new(model.clone())
        .tool(Echo::new())
        .plugin(WatcherPlugin::default())
}

async fn records(store: &Store, run: &tau_agent::tool::RunId) -> Vec<Value> {
    store
        .records(&run.0, NAME)
        .await
        .unwrap()
        .iter()
        .map(|body| serde_json::from_str(body).unwrap())
        .collect()
}

/// The requests that carried the watcher's instructions.
fn checks(model: &ScriptedModel) -> Vec<tau_testing::scripted::Request> {
    model
        .requests()
        .into_iter()
        .filter(|request| {
            request.settings.instructions.as_deref()
                == Some(prompt::INSTRUCTIONS)
        })
        .collect()
}

#[test]
fn the_sixth_step_asks_once_and_none_leaves_no_record() {
    let model = script(5, Some("learn: none"), 0);
    let agent = watching(&model);
    block_on_io(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let outcome = agent.run("run the tests", &store).await.unwrap();
        assert!(records(&store, &outcome.run).await.is_empty());
    });
    model.assert_exhausted();
    let checks = checks(&model);
    assert_eq!(checks.len(), 1);
    // The side request read the whole conversation, fenced as data.
    let asked = format!("{:?}", checks[0].transcript);
    assert!(asked.contains("run the tests"), "{asked}");
    assert!(asked.contains("step 4"), "{asked}");
    assert!(asked.contains("<earlier_notes>"), "{asked}");
}

#[test]
fn a_note_leaves_one_record_and_the_run_goes_on() {
    let model = script(5, Some(NOTE), 0);
    let agent = watching(&model);
    block_on_io(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let outcome = agent.run("run the tests", &store).await.unwrap();
        let stored = records(&store, &outcome.run).await;
        assert_eq!(stored.len(), 1, "{stored:?}");
        let state = State::from_records(&stored);
        let note = &state.notes[0];
        assert_eq!((note.step, note.tag), (6, Tag::HeadsUp));
        assert!(note.explain.is_some());
        assert!(state.waiting());
    });
    model.assert_exhausted();
    assert_eq!(checks(&model).len(), 1);
}

#[test]
fn an_unreadable_reply_is_recorded_as_dropped_and_shows_nothing() {
    let model =
        script(5, Some("Sure, here you go: watch out for debug builds."), 0);
    let agent = watching(&model);
    block_on_io(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let outcome = agent.run("run the tests", &store).await.unwrap();
        let stored = records(&store, &outcome.run).await;
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0]["kind"], "dropped");
        assert!(State::from_records(&stored).notes.is_empty());
    });
}

#[test]
fn no_second_check_while_a_note_waits() {
    // The 12th request would be due, but the note is unanswered.
    let model = script(5, Some(NOTE), 6);
    let agent = watching(&model);
    block_on_io(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        agent.run("run the tests", &store).await.unwrap();
    });
    model.assert_exhausted();
    assert_eq!(checks(&model).len(), 1);
}

#[test]
fn a_check_resumes_once_the_note_is_answered_and_the_prompt_lists_it() {
    // Six steps in the first run; the second goes on from the 7th and
    // reaches the 12th, which asks.
    let mut model = script(5, Some(NOTE), 0);
    for n in 0..5 {
        model = model.turn(|t| {
            t.tool_call("echo", json!({"text": format!("more {n}")}))
        });
    }
    let model = model
        .turn(|t| t.text("learn: none"))
        .turn(|t| t.text("done again"));
    let agent = watching(&model);
    block_on_io(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let first = agent.run("run the tests", &store).await.unwrap();
        // The person said they knew it.
        let answer = serde_json::to_string(&Record::Answered {
            key: "n0".into(),
            answer: Answer::Knew,
        })
        .unwrap();
        store
            .append_turn(
                &first.run.0,
                &[tau_store::Entry::Plugin {
                    plugin: NAME.into(),
                    body: answer,
                }],
                tau_store::TurnUsage::default(),
            )
            .await
            .unwrap();
        agent
            .resume(&first.run)
            .run("and the docs", &store)
            .await
            .unwrap();
    });
    model.assert_exhausted();
    let checks = checks(&model);
    assert_eq!(checks.len(), 2);
    let asked = format!("{:?}", checks[1].transcript);
    assert!(asked.contains("person_already_knew"), "{asked}");
    assert!(asked.contains("rebuilds in debug"), "{asked}");
}

#[test]
fn a_message_written_past_a_waiting_note_is_recorded() {
    let model = script(5, Some(NOTE), 0).turn(|t| t.text("again"));
    let agent = watching(&model);
    block_on_io(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let first = agent.run("run the tests", &store).await.unwrap();
        agent
            .resume(&first.run)
            .run("and now the docs", &store)
            .await
            .unwrap();
        let state = State::from_records(&records(&store, &first.run).await);
        assert_eq!(state.ignored, 1);
        assert_eq!(state.notes[0].typed_past, 1);
    });
}

#[test]
fn off_by_default_nothing_is_added_and_nothing_is_asked() {
    let ctx = run_ctx(RunKind::Chat);
    let plugins: Vec<Box<dyn Plugin>> =
        block_on_io(WatcherHost.agent_plugins(&(), &ctx, &Settings::default()))
            .unwrap();
    assert!(plugins.is_empty());
    // A run built from what the host gives: six steps, no check.
    let model = script(5, None, 0);
    let mut agent = Agent::new(model.clone()).tool(Echo::new());
    for plugin in plugins {
        agent = agent.boxed_plugin(plugin);
    }
    block_on_io(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let outcome = agent.run("run the tests", &store).await.unwrap();
        assert!(records(&store, &outcome.run).await.is_empty());
    });
    model.assert_exhausted();
    assert!(checks(&model).is_empty());
}

#[test]
fn switched_on_it_is_added_but_not_for_sub_agents() {
    let on = Settings {
        enabled: true,
        ..Settings::default()
    };
    for (kind, expected) in [
        (RunKind::Main, 1),
        (RunKind::Chat, 1),
        (RunKind::SubAgent, 0),
    ] {
        let plugins =
            block_on_io(WatcherHost.agent_plugins(&(), &run_ctx(kind), &on))
                .unwrap();
        assert_eq!(plugins.len(), expected, "{kind:?}");
    }
}
