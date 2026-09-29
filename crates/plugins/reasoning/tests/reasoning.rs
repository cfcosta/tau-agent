//! The effort a run gets: Jev's, when it is sure; the default when it
//! is not, fails, or someone chose one.

use std::{collections::BTreeMap, sync::Arc};

use futures_util::StreamExt;
use serde_json::Value;
use tau_agent::{agent::Agent, event::RunEvent};
use tau_ai::responses::request::ReasoningEffort;
use tau_jev::{Answer, JevError, fake::FakeJev};
use tau_reasoning::{Choice, NAME, Reasoning};
use tau_store::Store;
use tau_testing::{block_on, scripted::ScriptedModel};

/// A Jev that scores every task with `probabilities` (one per level,
/// lowest first) at `confidence`.
fn jev<const N: usize>(probabilities: [f64; N], confidence: f64) -> FakeJev {
    FakeJev::new(move |request| {
        let probabilities: BTreeMap<String, f64> = probabilities
            .iter()
            .enumerate()
            .map(|(n, p)| (n.to_string(), *p))
            .collect();
        let answers = request
            .questions
            .keys()
            .map(|id| {
                (
                    id.clone(),
                    Answer::Score {
                        score: 3.0,
                        probabilities: probabilities.clone(),
                        confidence,
                    },
                )
            })
            .collect();
        Ok(tau_jev::fake::response(answers, request))
    })
}

/// Runs a one-turn task on gpt-6-sol and returns the effort the model
/// was asked at, the plugin's reports, and what Jev was asked.
fn run(
    jev: FakeJev,
    set: Option<ReasoningEffort>,
) -> (Option<ReasoningEffort>, Vec<Value>, usize) {
    run_on("gpt-6-sol", jev, set)
}

fn run_on(
    model: &str,
    jev: FakeJev,
    set: Option<ReasoningEffort>,
) -> (Option<ReasoningEffort>, Vec<Value>, usize) {
    let llm = ScriptedModel::new().turn(|t| t.text("done"));
    let events: Vec<RunEvent> = block_on(async {
        let store = Store::memory().await.unwrap();
        let mut agent = Agent::new(llm.clone())
            .model(model)
            .instructions("You are a coding agent.")
            .plugin(Reasoning::new(Arc::new(jev.clone())));
        if let Some(effort) = set {
            agent = agent.reasoning(effort);
        }
        let mut run = agent.start("Track down the flaky lane test", &store);
        let events = run.events().collect().await;
        run.outcome().await.unwrap();
        events
    });
    let reports = events
        .into_iter()
        .filter_map(|event| match event {
            RunEvent::PluginReport { plugin, body, .. } if &*plugin == NAME => {
                Some(body)
            }
            _ => None,
        })
        .collect();
    let asked = llm.requests()[0].settings.reasoning;
    (asked, reports, jev.requests().len())
}

#[test]
fn a_confident_score_sets_the_effort() {
    let (asked, reports, calls) =
        run(jev([0.0, 0.01, 0.02, 0.84, 0.09, 0.04], 0.84), None);
    assert_eq!(asked, Some(ReasoningEffort::High));
    assert_eq!(calls, 1);
    let choice = Choice::parse(&reports[0]).unwrap();
    assert_eq!(choice.kind, "chose");
    assert_eq!(choice.effort, "high");
    assert_eq!(choice.chosen(), 3);
    assert_eq!(choice.levels.len(), 6, "gpt-6-sol takes none to max");
    assert_eq!(choice.levels[3].p, 0.84);
    assert!(choice.cost > 0.0);
}

#[test]
fn an_unsure_score_keeps_the_default() {
    let (asked, reports, _) =
        run(jev([0.1, 0.2, 0.3, 0.2, 0.1, 0.1], 0.3), None);
    assert_eq!(asked, None);
    let choice = Choice::parse(&reports[0]).unwrap();
    assert_eq!(
        (choice.kind.as_str(), choice.effort.as_str()),
        ("kept", "medium")
    );
}

/// Jev chooses among the efforts the run's model takes, and never one
/// it would reject.
#[test]
fn the_levels_are_the_models_efforts() {
    let efforts = |model: &str| {
        let (_, reports, _) =
            run_on(model, jev([1.0, 0.0, 0.0, 0.0, 0.0], 1.0), None);
        Choice::parse(&reports[0])
            .unwrap()
            .levels
            .into_iter()
            .map(|level| level.effort)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        efforts("gpt-5.5"),
        ["none", "low", "medium", "high", "xhigh"]
    );
    assert_eq!(
        efforts("gpt-6-astra"),
        ["low", "medium", "high", "xhigh", "max"]
    );
    let (asked, _, _) =
        run_on("gpt-6-astra", jev([1.0, 0.0, 0.0, 0.0, 0.0], 1.0), None);
    assert_eq!(asked, Some(ReasoningEffort::Low), "its lowest is low");
}

#[test]
fn a_model_that_does_not_reason_is_not_scored() {
    let (asked, reports, calls) =
        run_on("gpt-4.1", jev([1.0, 0.0, 0.0], 1.0), None);
    assert_eq!((asked, calls), (None, 0));
    assert!(reports.is_empty());
}

#[test]
fn a_chosen_effort_stands() {
    let (asked, reports, calls) = run(
        jev([0.0, 0.0, 0.0, 1.0, 0.0, 0.0], 1.0),
        Some(ReasoningEffort::Low),
    );
    assert_eq!(asked, Some(ReasoningEffort::Low));
    assert!(reports.is_empty());
    assert_eq!(calls, 0, "Jev is not asked");
}

#[test]
fn a_failed_request_keeps_the_default_and_says_so() {
    let failing = FakeJev::new(|_| Err(JevError::Transport("offline".into())));
    let (asked, reports, _) = run(failing, None);
    assert_eq!(asked, None);
    assert_eq!(reports[0]["kind"], "error");
}

/// Asks the real Jev about a hard task and a trivial one. Needs
/// `TYPESAFE_API_KEY` and the network:
/// `cargo test -p tau-reasoning -- --ignored --nocapture`.
#[test]
#[ignore = "needs TYPESAFE_API_KEY and the network"]
fn the_real_jev_scores_tasks() {
    let jev =
        Arc::new(tau_jev::TypeSafe::from_env().expect("TYPESAFE_API_KEY"));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for task in [
        "What does the README say the license is?",
        "Find why lanes on a draining WebSocket lose their continuation \
         under load, and fix the race without breaking the delta chain.",
    ] {
        let llm = ScriptedModel::new().turn(|t| t.text("done"));
        let reports: Vec<Value> = runtime.block_on(async {
            let store = Store::memory().await.unwrap();
            let agent =
                Agent::new(llm.clone()).plugin(Reasoning::new(jev.clone()));
            let mut run = agent.start(task, &store);
            let events: Vec<RunEvent> = run.events().collect().await;
            run.outcome().await.unwrap();
            events
                .into_iter()
                .filter_map(|event| match event {
                    RunEvent::PluginReport { body, .. } => Some(body),
                    _ => None,
                })
                .collect()
        });
        let choice = Choice::parse(&reports[0]).expect("a choice");
        println!(
            "{task}\n  -> {} ({}, confidence {:.2})",
            choice.effort, choice.kind, choice.confidence
        );
    }
}

/// An unsure score, or a failed request, goes on at the effort the
/// chat's last message ran at, and records it.
#[test]
fn an_unsure_message_goes_on_as_the_last_one() {
    // Sure of high (index 3 on gpt-6-sol), then unsure, then failing.
    let answers = Arc::new(std::sync::Mutex::new(vec![
        None,
        Some(([0.1, 0.2, 0.3, 0.2, 0.1, 0.1], 0.3)),
        Some(([0.0, 0.01, 0.02, 0.9, 0.05, 0.02], 0.9)),
    ]));
    let jev = FakeJev::new(move |request| {
        let Some((probabilities, confidence)) =
            answers.lock().unwrap().pop().unwrap()
        else {
            return Err(JevError::Transport("offline".into()));
        };
        let probabilities: BTreeMap<String, f64> = probabilities
            .iter()
            .enumerate()
            .map(|(n, p)| (n.to_string(), *p))
            .collect();
        let answers = request
            .questions
            .keys()
            .map(|id| {
                (
                    id.clone(),
                    Answer::Score {
                        score: 3.0,
                        probabilities: probabilities.clone(),
                        confidence,
                    },
                )
            })
            .collect();
        Ok(tau_jev::fake::response(answers, request))
    });
    let llm = ScriptedModel::new()
        .turn(|t| t.text("one"))
        .turn(|t| t.text("two"))
        .turn(|t| t.text("three"));
    let reports: Vec<Value> = block_on(async {
        let store = Store::memory().await.unwrap();
        let agent = Agent::new(llm.clone())
            .model("gpt-6-sol")
            .plugin(Reasoning::new(Arc::new(jev)));
        let first = agent.run("design the retry policy", &store).await.unwrap();
        let mut reports = Vec::new();
        for message in ["thanks", "and now?"] {
            let mut run = agent.resume(&first.run).start(message, &store);
            let events: Vec<RunEvent> = run.events().collect().await;
            run.outcome().await.unwrap();
            reports.extend(events.into_iter().filter_map(
                |event| match event {
                    RunEvent::PluginReport { plugin, body, .. }
                        if &*plugin == NAME =>
                    {
                        Some(body)
                    }
                    _ => None,
                },
            ));
        }
        reports
    });
    let asked: Vec<_> = llm
        .requests()
        .iter()
        .map(|request| request.settings.reasoning)
        .collect();
    assert_eq!(asked, [Some(ReasoningEffort::High); 3]);
    let unsure = Choice::parse(&reports[0]).unwrap();
    assert_eq!(unsure.kind, "kept");
    assert_eq!(unsure.runs_at.as_deref(), Some("high"));
    assert_eq!(reports[1]["kind"], "error");
    assert_eq!(reports[1]["runs_at"], "high");
}
