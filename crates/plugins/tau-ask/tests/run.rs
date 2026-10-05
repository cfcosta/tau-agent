//! `ask` in real runs: a scripted model, the store, and the person
//! played by the test. The call waits until the answer comes, and the
//! model reads it with its notes; a cancelled run stops the wait; asked
//! wrongly, the model is told why; a run that goes on closes the calls
//! its history left waiting; a call dropped mid-wait is closed too.

#![cfg(feature = "host")]

use futures_util::StreamExt;
use serde_json::{Value, json};
use tau_agent::{
    agent::Agent,
    event::{RunEvent, StopReason},
    tool::RunId,
};
use tau_ask::{
    Answer,
    NAME,
    Record,
    Reply,
    TOOL,
    host::{AskPlugin, Waiting},
};
use tau_store::{Entry, Store, TurnUsage};
use tau_testing::scripted::ScriptedModel;

/// Runs `future` with real time and I/O, failing it after `secs`: a
/// question that never reaches the person would wait forever.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tau_testing::block_on_io(async {
        tokio::time::timeout(std::time::Duration::from_secs(20), future)
            .await
            .expect("the run ends")
    })
}

fn questions() -> Value {
    json!({ "questions": [
        {
            "question": "How should the ask tool wait for your answer?",
            "header": "Waiting",
            "options": [
                { "label": "Hold the call open (Recommended)", "description": "Block until the answer comes." },
                { "label": "End the turn", "description": "Stop, and read the answer as the next message." }
            ]
        },
        {
            "question": "Where else should a waiting question show?",
            "header": "Surfaces",
            "multi_select": true,
            "options": [
                { "label": "Run list badge", "description": "A dot on the run." },
                { "label": "Notification", "description": "When the window is away." },
                { "label": "Parent run", "description": "For child runs." }
            ]
        }
    ]})
}

fn records(store: &Store, run: &RunId) -> Vec<Record> {
    block_on(store.records(&run.0, NAME))
        .unwrap()
        .iter()
        .map(|body| {
            Record::parse(&serde_json::from_str(body).unwrap()).unwrap()
        })
        .collect()
}

/// Runs `input` to the end; `asked` plays the person each time a call
/// asks, given the run's waiting calls, the run, the call and a way to
/// cancel the run.
fn run(
    agent: &Agent,
    store: &Store,
    input: &str,
    asked: impl Fn(&Waiting, &RunId, &str, &dyn Fn()),
    waiting: &Waiting,
) -> (RunId, Vec<RunEvent>, StopReason) {
    block_on(async {
        let mut run = agent.start(input, store);
        let id = run.id();
        let control = run.control();
        let mut events = Vec::new();
        {
            let mut stream = run.events();
            while let Some(event) = stream.next().await {
                if let RunEvent::PluginReport { plugin, body, .. } = &event
                    && &**plugin == NAME
                    && let Some(Record::Asked { call, .. }) =
                        Record::parse(body)
                {
                    asked(waiting, &id, &call, &|| control.cancel());
                }
                events.push(event);
            }
        }
        let outcome = run.outcome().await.unwrap();
        (id, events, outcome.stop)
    })
}

/// A result's text blocks, joined with "\n".
fn text_of(output: &tau_agent::tool::ToolOutput) -> String {
    output
        .content
        .iter()
        .filter_map(|block| match block {
            tau_ai::message::InputBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn tool_results(events: &[RunEvent]) -> Vec<(String, bool)> {
    events
        .iter()
        .filter_map(|event| match event {
            RunEvent::ToolEnd {
                output, is_error, ..
            } => Some((text_of(output), *is_error)),
            _ => None,
        })
        .collect()
}

fn answered() -> Reply {
    Reply::Answered {
        answers: vec![
            Answer {
                picked: vec!["Hold the call open (Recommended)".into()],
                other: None,
                note: Some("Cancel must drop the sender too.".into()),
            },
            Answer {
                picked: vec!["Run list badge".into(), "Parent run".into()],
                other: Some("the tray icon".into()),
                note: None,
            },
        ],
    }
}

#[test]
fn the_call_waits_for_the_answer_and_the_model_reads_it_with_its_notes() {
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call(TOOL, questions()))
        .turn(|t| t.text("Holding the call open."));
    let store = block_on(tau_store_sqlite::memory()).unwrap();
    let waiting = Waiting::default();
    let agent = Agent::new(llm.clone()).plugin(AskPlugin::new(waiting.clone()));
    let (id, events, stop) = run(
        &agent,
        &store,
        "add a plugin that asks me questions",
        |waiting, run, call, _| {
            assert!(waiting.waits(&run.0, call), "the call waits as it asks");
            // An answer that does not fit what was asked is refused, and
            // the call goes on waiting.
            let wrong = Reply::Answered {
                answers: vec![Answer::default()],
            };
            assert!(waiting.answer(&run.0, call, wrong).is_err());
            waiting.answer(&run.0, call, answered()).unwrap();
        },
        &waiting,
    );
    assert_eq!(stop, StopReason::Stop);
    llm.assert_exhausted();

    let results = tool_results(&events);
    assert_eq!(results.len(), 1);
    let (text, error) = &results[0];
    assert!(!error);
    assert_eq!(
        text,
        "The person answered:\n\
         \"How should the ask tool wait for your answer?\" = \"Hold the call open (Recommended)\"\n  \
         note: Cancel must drop the sender too.\n\
         \"Where else should a waiting question show?\" = \"Run list badge, Parent run, the tray icon\""
    );
    // The model's next request carries the answer.
    let requests = llm.requests();
    let sent = serde_json::to_string(&requests[1].transcript).unwrap();
    assert!(sent.contains("Cancel must drop the sender too."));

    let records = records(&store, &id);
    assert!(
        matches!(&records[..], [Record::Asked { .. }, Record::Answered { reply, .. }] if *reply == answered())
    );
    // The question reached the interface while the call waited, before
    // the call's end.
    let asked_at = events
        .iter()
        .position(|e| {
            matches!(e, RunEvent::PluginReport { body, .. }
                if matches!(Record::parse(body), Some(Record::Asked { .. })))
        })
        .unwrap();
    let ended_at = events
        .iter()
        .position(|e| matches!(e, RunEvent::ToolEnd { .. }))
        .unwrap();
    assert!(asked_at < ended_at);
    assert!(
        !waiting.waits(&id.0, "call_1"),
        "nothing waits once answered"
    );
}

#[test]
fn declining_tells_the_model_to_go_on() {
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call(TOOL, questions()))
        .turn(|t| t.text("Going on."));
    let store = block_on(tau_store_sqlite::memory()).unwrap();
    let waiting = Waiting::default();
    let agent = Agent::new(llm.clone()).plugin(AskPlugin::new(waiting.clone()));
    let (_, events, _) = run(
        &agent,
        &store,
        "go",
        |waiting, run, call, _| {
            waiting.answer(&run.0, call, Reply::Declined).unwrap()
        },
        &waiting,
    );
    let (text, error) = &tool_results(&events)[0];
    assert!(!error);
    assert!(text.starts_with("The person declined to answer."), "{text}");
}

#[test]
fn a_cancelled_run_stops_waiting_and_closes_the_call() {
    let llm = ScriptedModel::new().turn(|t| t.tool_call(TOOL, questions()));
    let store = block_on(tau_store_sqlite::memory()).unwrap();
    let waiting = Waiting::default();
    let agent = Agent::new(llm.clone()).plugin(AskPlugin::new(waiting.clone()));
    let (id, _, stop) =
        run(&agent, &store, "go", |_, _, _, cancel| cancel(), &waiting);
    assert_eq!(stop, StopReason::Cancelled);
    assert!(!waiting.waits(&id.0, "call_1"));
    let records = records(&store, &id);
    assert!(
        matches!(records.last(), Some(Record::Closed { .. })),
        "{records:?}"
    );
    assert!(waiting.answer(&id.0, "call_1", Reply::Declined).is_err());
}

#[test]
fn questions_asked_wrongly_come_back_with_why() {
    let mut wrong = questions();
    wrong["questions"][0]["options"][1]["label"] = json!("Other");
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call(TOOL, wrong))
        .turn(|t| t.text("ok"));
    let store = block_on(tau_store_sqlite::memory()).unwrap();
    let waiting = Waiting::default();
    let agent = Agent::new(llm.clone()).plugin(AskPlugin::new(waiting.clone()));
    let (id, events, _) = run(
        &agent,
        &store,
        "go",
        |_, _, _, _| panic!("nothing is asked"),
        &waiting,
    );
    let (text, error) = &tool_results(&events)[0];
    assert!(error);
    assert!(text.contains("\"Other\""), "{text}");
    assert!(records(&store, &id).is_empty());
}

#[test]
fn a_run_that_goes_on_closes_what_its_history_left_waiting() {
    let llm = ScriptedModel::new()
        .turn(|t| t.text("first"))
        .turn(|t| t.text("second"));
    let store = block_on(tau_store_sqlite::memory()).unwrap();
    let waiting = Waiting::default();
    let agent = Agent::new(llm.clone()).plugin(AskPlugin::new(waiting.clone()));
    let (first, _, _) = run(&agent, &store, "go", |_, _, _, _| {}, &waiting);
    // A call the first run left waiting, as an app that quit leaves it.
    let ask = serde_json::from_value(questions()).unwrap();
    let left = Record::Asked {
        call: "call_9".into(),
        ask,
    };
    block_on(store.append_turn(
        &first.0,
        &[Entry::Plugin {
            plugin: NAME.into(),
            body: left.to_value().to_string(),
        }],
        TurnUsage::default(),
    ))
    .unwrap();

    let second = block_on(async {
        let mut run = agent.resume(&first).start("go on", &store);
        let id = run.id();
        let _: Vec<RunEvent> = run.events().collect().await;
        run.outcome().await.unwrap();
        id
    });
    let records = records(&store, &second);
    assert!(
        records.contains(&Record::Closed {
            call: "call_9".into()
        }),
        "{records:?}"
    );
}

/// Only the model asks: a script calling it would drop the call with
/// the person still answering.
#[test]
fn only_the_model_can_call_ask() {
    use tau_agent::tool::{AgentTool, Exposure};
    let tool = tau_ask::host::AskTool::new(Waiting::default());
    assert_eq!(tool.exposure(), Exposure::ModelOnly);
    // The limits are in the schema the model reads.
    let schema = tool.parameters().to_string();
    assert!(schema.contains("\"maxItems\":4"), "{schema}");
}

/// A plugin whose tool runs `ask` under a short timeout: the timeout
/// drops the call's future while it waits, as a script that ends drops
/// the calls it made.
struct Racer {
    waiting: Waiting,
}

const RACER: &str = "racer";

struct RacerTool {
    ask: tau_ask::host::AskTool,
}

#[async_trait::async_trait]
impl tau_agent::tool::AgentTool for RacerTool {
    fn name(&self) -> &str {
        RACER
    }

    fn description(&self) -> &str {
        "Asks, then gives up."
    }

    fn parameters(&self) -> &Value {
        self.ask.parameters()
    }

    async fn call(
        &self,
        args: Value,
        ctx: tau_agent::tool::ToolCtx,
    ) -> Result<tau_agent::tool::ToolOutput, tau_agent::error::ToolError> {
        let wait = std::time::Duration::from_millis(50);
        match tokio::time::timeout(wait, self.ask.call(args, ctx)).await {
            Ok(output) => output,
            Err(_) => Ok(tau_agent::tool::ToolOutput::text("gave up")),
        }
    }
}

#[async_trait::async_trait]
impl tau_agent::plugin::Plugin for Racer {
    fn name(&self) -> &str {
        RACER
    }

    fn tools(&self) -> Vec<std::sync::Arc<dyn tau_agent::tool::AgentTool>> {
        vec![std::sync::Arc::new(RacerTool {
            ask: tau_ask::host::AskTool::new(self.waiting.clone()),
        })]
    }
}

#[test]
fn a_call_dropped_while_it_waits_is_closed() {
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call(RACER, questions()))
        .turn(|t| t.text("ok"));
    let store = block_on(tau_store_sqlite::memory()).unwrap();
    let waiting = Waiting::default();
    let agent = Agent::new(llm.clone()).plugin(Racer {
        waiting: waiting.clone(),
    });
    let (id, events, _) = run(&agent, &store, "go", |_, _, _, _| {}, &waiting);
    assert!(!waiting.waits(&id.0, "call_1"), "forgotten once dropped");
    // The close was reported at once, and is stored in the background.
    let reported: Vec<Record> = events
        .iter()
        .filter_map(|event| match event {
            RunEvent::PluginReport { body, .. } => Record::parse(body),
            _ => None,
        })
        .collect();
    assert!(
        matches!(&reported[..], [Record::Asked { .. }, Record::Closed { .. }]),
        "{reported:?}"
    );
    let stored = block_on(async {
        for _ in 0..100 {
            let stored = store.records(&id.0, RACER).await.unwrap();
            if stored.len() == 2 {
                return stored;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        store.records(&id.0, RACER).await.unwrap()
    });
    assert!(stored[1].contains("\"closed\""), "{stored:?}");
}
