//! Goals in real runs: a scripted model, a fake Jev, and the store. The
//! run goes on until Jev says the goal holds, stops when the goal runs
//! out of continuations or budget, and keeps its goal across resumes,
//! pauses, extensions and clears that an interface stores.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures_util::StreamExt;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use tau_agent::{
    agent::Agent,
    event::{RunEvent, StopReason},
    limits::Limits,
    tool::{RunId, ToolCtx, ToolOutput, TypedTool, typed},
};
use tau_ai::message::{Message, UserContent};
use tau_goal::{
    CONTINUATION_PREFIX,
    Command,
    DEFAULT_BUDGET,
    DEFAULT_CONTINUATIONS,
    Exhausted,
    Goal,
    GoalPlugin,
    NAME,
    Record,
    Status,
    set_input,
    set_message,
};
use tau_jev::{Answer, JevError, fake::FakeJev};
use tau_store::{Entry, Store, TurnUsage};
use tau_testing::scripted::ScriptedModel;

thread_local! {
    /// The test's runtime. The store needs one with I/O, and an
    /// in-memory store lives in the runtime that opened it.
    static RUNTIME: tokio::runtime::Runtime =
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    RUNTIME.with(|runtime| runtime.block_on(future))
}

/// A Jev that answers each request with the next probability, and the
/// last one after that.
fn jev(answers: &[f64]) -> FakeJev {
    let answers = Arc::new(Mutex::new(answers.to_vec()));
    FakeJev::new(move |request| {
        let mut answers = answers.lock().unwrap();
        let p = if answers.len() > 1 {
            answers.remove(0)
        } else {
            answers[0]
        };
        let answers = request
            .questions
            .keys()
            .map(|id| (id.clone(), Answer::Noul { noul: p }))
            .collect();
        Ok(tau_jev::fake::response(answers, request))
    })
}

fn agent(llm: &ScriptedModel, jev: FakeJev) -> Agent {
    Agent::new(llm.clone())
        .tool(typed(Tests))
        .plugin(GoalPlugin::new(Arc::new(jev)))
        .limits(Limits::default().max_continuations(100))
}

fn records(store: &Store, run: &RunId) -> Vec<Value> {
    block_on(store.records(&run.0, NAME))
        .unwrap()
        .iter()
        .map(|body| serde_json::from_str(body).unwrap())
        .collect()
}

fn kinds(records: &[Value]) -> Vec<String> {
    records
        .iter()
        .map(|body| body["kind"].as_str().unwrap().to_owned())
        .collect()
}

/// Runs `input` (resuming `resume` when given) to the end, returning
/// the run and its events.
fn run(
    agent: &Agent,
    store: &Store,
    input: &str,
    resume: Option<&RunId>,
) -> (RunId, Vec<RunEvent>, StopReason) {
    block_on(async {
        let mut run = match resume {
            Some(id) => agent.resume(id).start(input, store),
            None => agent.start(input, store),
        };
        let id = run.id();
        let events: Vec<RunEvent> = run.events().collect().await;
        let outcome = run.outcome().await.unwrap();
        (id, events, outcome.stop)
    })
}

fn continued(events: &[RunEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            RunEvent::Continued {
                plugin, message, ..
            } if &**plugin == NAME => Some(message.clone()),
            _ => None,
        })
        .collect()
}

fn text_of(message: &Message) -> String {
    match message {
        Message::User(user) => match &user.content {
            UserContent::Text(text) => text.clone(),
            UserContent::Blocks(_) => String::new(),
        },
        _ => String::new(),
    }
}

/// Runs the tests: its output is what Jev judges.
#[derive(Clone)]
struct Tests;

#[derive(Deserialize, JsonSchema)]
struct TestsArgs {
    #[allow(dead_code)]
    command: String,
}

#[async_trait]
impl TypedTool for Tests {
    type Args = TestsArgs;
    const NAME: &'static str = "bash";
    const DESCRIPTION: &'static str = "Runs a command.";
    async fn call(
        &self,
        _args: TestsArgs,
        _ctx: ToolCtx,
    ) -> anyhow::Result<ToolOutput> {
        Ok(ToolOutput::text("14 tests run: 14 passed"))
    }
}

#[test]
fn commands_are_read_with_their_limits() {
    assert_eq!(
        Command::parse("/goal tests pass"),
        Some(Command::Set {
            condition: "tests pass".into(),
            continuations: DEFAULT_CONTINUATIONS,
            budget: DEFAULT_BUDGET,
        })
    );
    assert_eq!(
        Command::parse("/goal --continuations 3 --budget $0.50 all green"),
        Some(Command::Set {
            condition: "all green".into(),
            continuations: 3,
            budget: 0.5,
        })
    );
    assert_eq!(Command::parse("/goal clear"), Some(Command::Clear));
    for not in [
        "/goal",
        "/goal   ",
        "/goals x",
        "fix /goal x",
        "/goal --budget",
    ] {
        assert_eq!(Command::parse(not), None, "{not}");
    }
    // The model's input names the condition, which reads back.
    let input = set_input("tests pass");
    assert!(input.starts_with("/goal tests pass\n\n"));
    assert_eq!(set_message(&input).as_deref(), Some("tests pass"));
    assert_eq!(set_message("fix the tests"), None);
}

#[test]
fn the_run_goes_on_until_jev_says_the_goal_holds() {
    let llm = ScriptedModel::new()
        .turn(|t| t.text("done?"))
        .turn(|t| t.text("done now?"))
        .turn(|t| {
            t.tool_call("bash", json!({ "command": "cargo nextest run" }))
        })
        .turn(|t| t.text("14 passed."));
    let fake = jev(&[0.1, 0.2, 0.9]);
    let store = block_on(Store::memory()).unwrap();
    let agent = agent(&llm, fake.clone());
    let (id, events, stop) =
        run(&agent, &store, "/goal --budget 5 the tests pass", None);
    assert_eq!(stop, StopReason::Stop);
    llm.assert_exhausted();

    // The model got the condition and how goals work, not the command.
    let requests = llm.requests();
    let first = text_of(&requests[0].transcript[0]);
    assert_eq!(first, set_input("the tests pass"));
    // Sent back twice, with the goal.
    let sent = continued(&events);
    assert_eq!(sent.len(), 2);
    assert!(sent[0].starts_with(CONTINUATION_PREFIX), "{}", sent[0]);
    assert!(sent[0].contains("Goal: the tests pass"));
    assert!(sent[1].contains("Continuation 2 of 10"));

    // The last check saw the tests' output.
    let last = fake.requests().pop().unwrap();
    assert_eq!(last.state["goal"], "the tests pass");
    assert_eq!(last.state["final_answer"], "14 passed.");
    assert_eq!(
        last.state["recent_tool_results"][0]["output"],
        "14 tests run: 14 passed"
    );

    let records = records(&store, &id);
    assert_eq!(kinds(&records), ["set", "check", "check", "check"]);
    let goal = Goal::fold(&records).unwrap();
    assert_eq!(goal.status, Status::Met);
    assert_eq!(goal.continuations, 2);
    assert_eq!(goal.budget, 5.0);
    let checks: Vec<(u32, bool, Option<u32>)> = goal
        .checks
        .iter()
        .map(|check| (check.n, check.met, check.continuation))
        .collect();
    assert_eq!(
        checks,
        [(1, false, Some(1)), (2, false, Some(2)), (3, true, None)]
    );
    assert!(goal.spent > 0.0, "Jev's checks count toward the goal");
    // Every record was reported, as it happened.
    let reports = events
        .iter()
        .filter(|event| {
            matches!(event, RunEvent::PluginReport { plugin, .. }
                if &**plugin == NAME)
        })
        .count();
    assert_eq!(reports, 4);
}

#[test]
fn a_goal_stops_when_out_of_continuations_or_budget() {
    // Out of continuations: one allowed, then the run stops.
    let llm = ScriptedModel::new()
        .turn(|t| t.text("tried"))
        .turn(|t| t.text("tried again"));
    let store = block_on(Store::memory()).unwrap();
    let (id, events, _) = run(
        &agent(&llm, jev(&[0.1])),
        &store,
        "/goal --continuations 1 the tests pass",
        None,
    );
    llm.assert_exhausted();
    assert_eq!(continued(&events).len(), 1);
    let stored = records(&store, &id);
    assert_eq!(kinds(&stored), ["set", "check", "check", "stopped"]);
    let goal = Goal::fold(&stored).unwrap();
    assert_eq!(goal.status, Status::Stopped(Exhausted::Continuations));

    // Out of budget: each turn costs $0.25 of $0.30.
    let llm = ScriptedModel::new()
        .turn(|t| t.text("tried").cost(0.25))
        .turn(|t| t.text("tried again").cost(0.25));
    let store = block_on(Store::memory()).unwrap();
    let (id, _, _) = run(
        &agent(&llm, jev(&[0.1])),
        &store,
        "/goal --budget 0.30 the tests pass",
        None,
    );
    llm.assert_exhausted();
    let goal = Goal::fold(&records(&store, &id)).unwrap();
    assert_eq!(goal.status, Status::Stopped(Exhausted::Budget));
    assert!(goal.spent >= 0.5);
}

/// Stores `record` with `run` for the plugin, as an interface does.
fn store_record(store: &Store, run: &RunId, record: &Record) {
    let entry = Entry::Plugin {
        plugin: NAME.into(),
        body: record.to_value().to_string(),
    };
    block_on(store.append_turn(&run.0, &[entry], TurnUsage::default()))
        .unwrap();
}

#[test]
fn a_goal_outlives_the_run_and_takes_what_an_interface_stores() {
    let llm = ScriptedModel::new()
        .turn(|t| t.text("tried"))
        .turn(|t| t.text("tried again"))
        .turn(|t| t.text("paused, so no check"))
        .turn(|t| t.text("done"));
    let fake = jev(&[0.1, 0.1, 0.9]);
    let store = block_on(Store::memory()).unwrap();
    let agent = agent(&llm, fake.clone());
    let (id, _, _) =
        run(&agent, &store, "/goal --continuations 1 it works", None);
    assert_eq!(fake.requests().len(), 2);
    assert_eq!(
        Goal::fold(&records(&store, &id)).unwrap().status,
        Status::Stopped(Exhausted::Continuations)
    );

    // Paused: the next start is not checked.
    store_record(&store, &id, &Record::Extended { by: 1 });
    store_record(&store, &id, &Record::Paused);
    run(&agent, &store, "look at it again", Some(&id));
    assert_eq!(fake.requests().len(), 2);

    // Resumed, with one more continuation: met.
    store_record(&store, &id, &Record::Resumed);
    let (_, events, _) = run(&agent, &store, "keep going", Some(&id));
    assert!(continued(&events).is_empty());
    let goal = Goal::fold(&records(&store, &id)).unwrap();
    assert_eq!(goal.status, Status::Met);
    assert_eq!(goal.max_continuations, 2);
    assert_eq!(goal.checks.len(), 3);
    llm.assert_exhausted();

    // Cleared from a message: no goal left, and the model is told.
    let llm = ScriptedModel::new().turn(|t| t.text("ok"));
    let agent = self::agent(&llm, jev(&[0.1]));
    run(&agent, &store, "/goal clear", Some(&id));
    assert!(Goal::fold(&records(&store, &id)).is_none());
    let last = llm.requests().pop().unwrap();
    assert_eq!(
        text_of(last.transcript.last().unwrap()),
        "The goal is cleared."
    );
}

#[test]
fn no_goal_means_no_checks_and_a_failed_check_stops() {
    let llm = ScriptedModel::new().turn(|t| t.text("hello"));
    let fake = jev(&[0.1]);
    let store = block_on(Store::memory()).unwrap();
    let (id, events, _) = run(&agent(&llm, fake.clone()), &store, "hi", None);
    assert!(fake.requests().is_empty());
    assert!(continued(&events).is_empty());
    assert!(records(&store, &id).is_empty());

    let llm = ScriptedModel::new().turn(|t| t.text("hello"));
    let failing = FakeJev::new(|_| Err(JevError::Status(503)));
    let store = block_on(Store::memory()).unwrap();
    let (id, events, stop) =
        run(&agent(&llm, failing), &store, "/goal it works", None);
    assert_eq!(stop, StopReason::Stop);
    assert!(continued(&events).is_empty());
    let goal = Goal::fold(&records(&store, &id)).unwrap();
    assert_eq!(goal.status, Status::Active, "still set, just unchecked");
    assert!(goal.error.unwrap().contains("503"));
}
