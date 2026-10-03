//! The effort a run gets: Jev's, when it is sure; the default when it
//! is not, fails, or someone chose one. On the models whose cache
//! survives a change of effort, Jev picks again when its lease ends.

use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{Value, json};
use tau_agent::{
    agent::Agent,
    error::ToolError,
    event::RunEvent,
    tool::{AgentTool, ToolCtx, ToolOutput},
};
use tau_ai::responses::request::ReasoningEffort;
use tau_jev::{Answer, JevError, Request, fake::FakeJev};
use tau_reasoning::{Choice, NAME, Reasoning, Record, Verdict};
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
    let choice = Record::parse(&reports[0])
        .and_then(Record::into_choice)
        .unwrap();
    assert_eq!(choice.verdict, Verdict::Chose);
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
    let choice = Record::parse(&reports[0])
        .and_then(Record::into_choice)
        .unwrap();
    assert_eq!(
        (choice.verdict, choice.effort.as_str()),
        (Verdict::Kept, "medium")
    );
}

/// Jev chooses among the efforts the run's model takes, and never one
/// it would reject.
#[test]
fn the_levels_are_the_models_efforts() {
    let efforts = |model: &str| {
        let (_, reports, _) =
            run_on(model, jev([1.0, 0.0, 0.0, 0.0, 0.0], 1.0), None);
        Record::parse(&reports[0])
            .and_then(Record::into_choice)
            .unwrap()
            .levels
            .into_iter()
            .map(|level| level.effort)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        efforts("gpt-6-sol"),
        ["none", "low", "medium", "high", "xhigh", "max"]
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
        let choice = Record::parse(&reports[0])
            .and_then(Record::into_choice)
            .expect("a choice");
        println!(
            "{task}\n  -> {} ({:?}, confidence {:.2})",
            choice.effort, choice.verdict, choice.confidence
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
    let unsure = Record::parse(&reports[0])
        .and_then(Record::into_choice)
        .unwrap();
    assert_eq!(unsure.verdict, Verdict::Kept);
    assert_eq!(unsure.runs_at.as_deref(), Some("high"));
    assert_eq!(reports[1]["kind"], "error");
    assert_eq!(reports[1]["runs_at"], "high");
}

/// A Jev that answers each request with the next of `answers`: sure of
/// the level at that index, for the lease named.
fn scripted(answers: Vec<(usize, &'static str)>) -> FakeJev {
    let answers = Arc::new(std::sync::Mutex::new(answers.into_iter()));
    FakeJev::new(move |request| {
        let (level, lease) =
            answers.lock().unwrap().next().expect("an answer left");
        let mut answers = BTreeMap::from([(
            "effort".to_owned(),
            Answer::Score {
                score: level as f64,
                probabilities: BTreeMap::from([(level.to_string(), 1.0)]),
                confidence: 0.9,
            },
        )]);
        if request.questions.contains_key("lease") {
            answers.insert(
                "lease".into(),
                Answer::Choice {
                    choice: lease.into(),
                    probabilities: BTreeMap::from([(lease.into(), 1.0)]),
                    confidence: 0.9,
                },
            );
        }
        Ok(tau_jev::fake::response(answers, request))
    })
}

/// Fails when its `text` is "fail", and echoes it otherwise.
struct Probe {
    schema: Value,
}

impl Probe {
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
impl AgentTool for Probe {
    fn name(&self) -> &str {
        "probe"
    }
    fn description(&self) -> &str {
        "Echoes its text, or fails on \"fail\"."
    }
    fn parameters(&self) -> &Value {
        &self.schema
    }
    async fn call(
        &self,
        args: Value,
        _ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        match args["text"].as_str().unwrap_or_default() {
            "fail" => return Err("the build failed: 3 errors".into()),
            text => Ok(ToolOutput::text(text)),
        }
    }
}

/// Runs "fix the lane race" on gpt-6-sol with one probe call per entry
/// of `calls`, then a final answer, with [`Reasoning::redecide`] set to
/// `redecide`. Returns the effort of each request, what Jev was asked,
/// and the plugin's reports.
fn tool_run(
    redecide: bool,
    jev: FakeJev,
    calls: &[&'static str],
) -> (Vec<Option<ReasoningEffort>>, Vec<Request>, Vec<Value>) {
    let mut llm = ScriptedModel::new();
    for text in calls {
        llm = llm.turn(|t| {
            t.text(format!("Next I run the probe on {text}."))
                .tool_call("probe", json!({"text": text}))
        });
    }
    let llm = llm.turn(|t| t.text("done"));
    let events: Vec<RunEvent> = block_on(async {
        let store = Store::memory().await.unwrap();
        let agent = Agent::new(llm.clone())
            .model("gpt-6-sol")
            .tool(Probe::new())
            .plugin(Reasoning::new(Arc::new(jev.clone())).redecide(redecide));
        let mut run = agent.start("fix the lane race", &store);
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
    let efforts = llm
        .requests()
        .iter()
        .map(|request| request.settings.reasoning)
        .collect();
    (efforts, jev.requests(), reports)
}

/// With `redecide`, a `one_call` lease has Jev pick again before the next
/// request, and a `tool_chain` lease holds while the tools succeed.
/// Each pick is reported and says the turn it is for.
#[test]
fn a_lease_says_when_jev_picks_again() {
    use ReasoningEffort::{High, Low};
    // gpt-6-sol's levels: none, low, medium, high, xhigh, max.
    let jev = scripted(vec![(3, "one_call"), (1, "tool_chain")]);
    let (efforts, asked, reports) = tool_run(true, jev, &["a", "b", "c"]);
    assert_eq!(efforts, [Some(High), Some(Low), Some(Low), Some(Low)]);
    assert_eq!(asked.len(), 2);
    assert!(asked[0].questions.contains_key("lease"));
    let step = &asked[1].state;
    assert_eq!(step["step"], "tool_step");
    assert_eq!(step["effort_now"], "high");
    assert_eq!(step["agent_said"], "Next I run the probe on a.");
    assert_eq!(step["tool_results"]["calls"], 1);
    assert_eq!(step["tool_results"]["failed"], 0);
    let picks: Vec<Choice> = reports
        .iter()
        .map(|body| Record::parse(body).and_then(Record::into_choice).unwrap())
        .collect();
    assert_eq!(
        picks
            .iter()
            .map(|c| (c.step.as_str(), c.turn, c.lease.as_deref()))
            .collect::<Vec<_>>(),
        [
            ("user_turn", None, Some("one_call")),
            ("tool_step", Some(2), Some("tool_chain")),
        ]
    );
    assert_eq!(picks[1].runs_at.as_deref(), Some("low"));
}

/// A failed tool call ends even a `user_turn` lease, and Jev sees the
/// failure first among the results.
#[test]
fn a_failed_tool_ends_the_lease() {
    use ReasoningEffort::{Medium, Xhigh};
    let jev = scripted(vec![(2, "user_turn"), (4, "tool_chain")]);
    let (efforts, asked, _) = tool_run(true, jev, &["ok", "fail", "ok"]);
    assert_eq!(
        efforts,
        [Some(Medium), Some(Medium), Some(Xhigh), Some(Xhigh)]
    );
    assert_eq!(asked.len(), 2);
    let results = &asked[1].state["tool_results"];
    assert_eq!(results["failed"], 1);
    assert_eq!(results["excerpts"][0]["failed"], true);
    assert_eq!(results["excerpts"][0]["tool"], "probe");
    assert!(
        results["excerpts"][0]["text"]
            .as_str()
            .unwrap()
            .contains("3 errors")
    );
}

/// Everything the plugin reports, choices and failures alike, it also
/// records, in the same order: a stored run shows what a live one did.
/// Its own context record for the next message comes last.
#[hegel::test(test_cases = 30)]
fn what_is_reported_is_recorded(tc: hegel::TestCase) {
    use hegel::generators as gs;
    // Each request Jev gets: sure of a level for a lease, or failing.
    let answers: Vec<Option<(usize, &'static str)>> = tc.draw(
        gs::vecs(gs::optional(hegel::tuples!(
            gs::integers::<usize>().max_value(5),
            gs::sampled_from(vec!["one_call", "tool_chain", "user_turn"]),
        )))
        .min_size(1)
        .max_size(4),
    );
    let calls: Vec<&'static str> =
        tc.draw(gs::vecs(gs::sampled_from(vec!["ok", "fail"])).max_size(3));
    let answers = Arc::new(std::sync::Mutex::new(answers.into_iter()));
    let jev = FakeJev::new(move |request| {
        // Past the drawn answers, Jev fails.
        let Some((level, lease)) = answers.lock().unwrap().next().flatten()
        else {
            return Err(JevError::Transport("offline".into()));
        };
        let mut answers = BTreeMap::from([(
            "effort".to_owned(),
            Answer::Score {
                score: level as f64,
                probabilities: BTreeMap::from([(level.to_string(), 1.0)]),
                confidence: 0.9,
            },
        )]);
        if request.questions.contains_key("lease") {
            answers.insert(
                "lease".into(),
                Answer::Choice {
                    choice: lease.into(),
                    probabilities: BTreeMap::from([(lease.into(), 1.0)]),
                    confidence: 0.9,
                },
            );
        }
        Ok(tau_jev::fake::response(answers, request))
    });
    let mut llm = ScriptedModel::new();
    for text in &calls {
        llm = llm.turn(|t| t.tool_call("probe", json!({"text": text})));
    }
    let llm = llm.turn(|t| t.text("done"));
    block_on(async {
        let store = Store::memory().await.unwrap();
        let agent = Agent::new(llm)
            .model("gpt-6-sol")
            .tool(Probe::new())
            .plugin(Reasoning::new(Arc::new(jev)).redecide(true));
        let mut run = agent.start("fix the lane race", &store);
        let events: Vec<RunEvent> = run.events().collect().await;
        let outcome = run.outcome().await.unwrap();
        let reports: Vec<Value> = events
            .into_iter()
            .filter_map(|event| match event {
                RunEvent::PluginReport { plugin, body, .. }
                    if &*plugin == NAME =>
                {
                    Some(body)
                }
                _ => None,
            })
            .collect();
        let records: Vec<Value> = store
            .records(&outcome.run.0, NAME)
            .await
            .unwrap()
            .iter()
            .map(|body| serde_json::from_str(body).unwrap())
            .collect();
        let (last, kept) = records.split_last().unwrap();
        assert_eq!(last["kind"], "context");
        assert_eq!(kept, reports.as_slice());
    });
}

/// By default Jev is asked once, with no lease, and the effort holds for
/// the whole run: a change would cost the next request its cache.
#[test]
fn by_default_the_effort_holds_for_the_run() {
    let jev = scripted(vec![(1, "one_call")]);
    let (efforts, asked, _) = tool_run(false, jev, &["a", "fail"]);
    assert_eq!(efforts, [Some(ReasoningEffort::Low); 3]);
    assert_eq!(asked.len(), 1);
    assert!(!asked[0].questions.contains_key("lease"));
}

/// A short message brings the task before it and what the agent last
/// proposed; a long one stands on its own.
#[test]
fn a_short_ask_brings_the_task_it_answers() {
    let jev = scripted(vec![(3, "user_turn"); 3]);
    let llm = ScriptedModel::new()
        .turn(|t| t.text("I propose moving the lease into the lane."))
        .turn(|t| t.text("done"))
        .turn(|t| t.text("done"));
    block_on(async {
        let store = Store::memory().await.unwrap();
        let agent = Agent::new(llm.clone())
            .model("gpt-6-sol")
            .plugin(Reasoning::new(Arc::new(jev.clone())));
        let first = agent
            .run("Design where retry leases live", &store)
            .await
            .unwrap();
        for message in ["yes, do it", &"now write the docs ".repeat(8)] {
            agent
                .resume(&first.run)
                .start(message, &store)
                .outcome()
                .await
                .unwrap();
        }
    });
    let asked = jev.requests();
    assert_eq!(asked[0].state.get("previous_task"), None);
    assert_eq!(
        asked[1].state["previous_task"],
        "Design where retry leases live"
    );
    assert_eq!(
        asked[1].state["last_proposal"],
        "I propose moving the lease into the lane."
    );
    assert_eq!(asked[2].state.get("previous_task"), None);
}

/// A stored run replays request by request: Jev scores the message, a
/// lease that holds skips a request, and a failed tool ends it. Each
/// decision says what the stored request went out at.
#[test]
fn a_stored_run_replays_through_the_policy() {
    use tau_reasoning::replay::{Entry, replay};
    let stored = scripted(vec![(3, "user_turn"); 2]);
    let mut llm = ScriptedModel::new();
    for text in ["a", "fail"] {
        llm = llm.turn(|t| t.tool_call("probe", json!({"text": text})));
    }
    let llm = llm.turn(|t| t.text("done"));
    let decisions = block_on(async {
        let store = Store::memory().await.unwrap();
        let agent = Agent::new(llm)
            .model("gpt-6-sol")
            .tool(Probe::new())
            .plugin(Reasoning::new(Arc::new(stored)).redecide(true));
        let run = agent.run("fix the lane race", &store).await.unwrap();
        let timeline: Vec<Entry> = store
            .timeline(&run.run.0)
            .await
            .unwrap()
            .into_iter()
            .filter_map(|entry| match entry {
                tau_store::Entry::Message { body, .. } => {
                    serde_json::from_str(&body).ok().map(Entry::Message)
                }
                tau_store::Entry::Plugin { plugin, body } if plugin == NAME => {
                    serde_json::from_str(&body).ok().map(Entry::Record)
                }
                _ => None,
            })
            .collect();
        let jev = scripted(vec![(1, "tool_chain"), (4, "tool_chain")]);
        let picker = Reasoning::new(Arc::new(jev))
            .redecide(true)
            .picker("gpt-6-sol");
        replay(&picker, "", &timeline).await
    });
    let summary: Vec<_> = decisions
        .iter()
        .map(|d| {
            (
                d.request,
                d.step,
                d.recorded.as_deref(),
                d.asked.is_some(),
                d.runs_at.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            (1, "user_turn", Some("high"), true, Some("low")),
            (2, "tool_step", Some("high"), false, Some("low")),
            (3, "tool_step", Some("high"), true, Some("xhigh")),
        ]
    );
}

/// The sticky-effort rule, as the plugin's docs say it, by cases: a
/// reference for [`tau_reasoning::runs_at`].
fn reference_runs_at(
    picked: Option<ReasoningEffort>,
    last: Option<Option<ReasoningEffort>>,
    prefix: u64,
    sticky_after: u64,
) -> (Option<ReasoningEffort>, Option<u64>) {
    let Some(picked) = picked else {
        // Unsure or failed: as the last message ran, or the default.
        return (last.unwrap_or(None), None);
    };
    let Some(last) = last else {
        // Nothing ran yet: nothing to keep.
        return (Some(picked), None);
    };
    if prefix < sticky_after || last == Some(picked) {
        (Some(picked), None)
    } else {
        (last, Some(prefix))
    }
}

/// An effort, drawn by its place in [`ReasoningEffort::ALL`].
fn effort(tc: &hegel::TestCase) -> ReasoningEffort {
    use hegel::generators as gs;
    ReasoningEffort::ALL[tc.draw(gs::integers::<usize>().max_value(6))]
}

/// For any pick, last effort, prefix and threshold, the message runs as
/// the reference says; what it runs at is Jev's pick or the last effort,
/// and only a kept effort says how many tokens it kept.
#[hegel::test(test_cases = 500)]
fn sticky_effort_follows_the_rule(tc: hegel::TestCase) {
    use hegel::generators as gs;
    let picked = tc.draw(gs::booleans()).then(|| effort(&tc));
    let last = tc
        .draw(gs::booleans())
        .then(|| tc.draw(gs::booleans()).then(|| effort(&tc)));
    let sticky_after = tc.draw(gs::integers::<u64>().max_value(100_000));
    // The threshold itself and just under it, as often as anything else.
    let prefix = match tc.draw(gs::integers::<u8>().max_value(2)) {
        0 => sticky_after,
        1 => sticky_after.saturating_sub(1),
        _ => tc.draw(gs::integers::<u64>().max_value(200_000)),
    };
    let got = tau_reasoning::runs_at(picked, last, prefix, sticky_after);
    assert_eq!(got, reference_runs_at(picked, last, prefix, sticky_after));
    let (runs, kept) = got;
    assert!(runs == picked || Some(runs) == last || picked.is_none());
    if let Some(tokens) = kept {
        assert_eq!(tokens, prefix);
        assert!(prefix >= sticky_after);
        assert_ne!(runs, picked);
    }
}

/// Runs a conversation on gpt-6-sol: a first message whose answer is
/// `answer_words` words long, at `first` (an effort the person chose, or
/// Jev's `first_pick`, by its place among the model's levels), then a
/// resumed message, or a fork, with Jev sure of `next_pick`. Returns the efforts the two messages ran at
/// and the second's choice.
fn conversation(
    first: Option<ReasoningEffort>,
    first_pick: usize,
    answer_words: usize,
    fork: bool,
    next_pick: usize,
    sticky_after: u64,
) -> (Option<ReasoningEffort>, Option<ReasoningEffort>, Choice) {
    let answer = "word ".repeat(answer_words);
    let llm = ScriptedModel::new()
        .turn(move |t| t.text(answer.clone()))
        .turn(|t| t.text("next"));
    // Jev is asked only where nobody chose: last pick first off.
    let mut picks = vec![next_pick];
    if first.is_none() {
        picks.push(first_pick);
    }
    let picks = Arc::new(std::sync::Mutex::new(picks));
    let jev = FakeJev::new(move |request| {
        let level = picks.lock().unwrap().pop().expect("a pick");
        let answers = BTreeMap::from([(
            "effort".to_owned(),
            Answer::Score {
                score: level as f64,
                probabilities: BTreeMap::from([(level.to_string(), 1.0)]),
                confidence: 0.9,
            },
        )]);
        Ok(tau_jev::fake::response(answers, request))
    });
    let reports = block_on(async {
        let store = Store::memory().await.unwrap();
        let plugin = Reasoning::new(Arc::new(jev)).sticky_after(sticky_after);
        let auto = Agent::new(llm.clone())
            .model("gpt-6-sol")
            .instructions("You are a coding agent.")
            .plugin(plugin);
        let starter = match first {
            Some(effort) => auto.clone().reasoning(effort),
            None => auto.clone(),
        };
        let outcome = starter.run("design the pool", &store).await.unwrap();
        let mut run = if fork {
            auto.fork(&outcome.checkpoint())
                .start("and the lanes", &store)
        } else {
            auto.resume(&outcome.run).start("and the lanes", &store)
        };
        let events: Vec<RunEvent> = run.events().collect().await;
        run.outcome().await.unwrap();
        events
            .into_iter()
            .filter_map(|event| match event {
                RunEvent::PluginReport { plugin, body, .. }
                    if &*plugin == NAME =>
                {
                    Record::parse(&body).and_then(Record::into_choice)
                }
                _ => None,
            })
            .collect::<Vec<_>>()
    });
    let asked: Vec<_> = llm
        .requests()
        .iter()
        .map(|request| request.settings.reasoning)
        .collect();
    (
        asked[0],
        asked[1],
        reports.last().cloned().expect("a choice"),
    )
}

/// A resumed message or a fork keeps the effort the conversation last
/// ran at, Jev's or one the person chose, once its prefix reaches the
/// threshold, and says how many tokens that kept; under it, Jev's pick
/// runs.
#[hegel::test(test_cases = 20)]
fn a_long_conversation_keeps_its_effort(tc: hegel::TestCase) {
    use hegel::generators as gs;
    let levels: Vec<ReasoningEffort> = tau_reasoning::levels_for("gpt-6-sol")
        .into_iter()
        .map(|level| level.effort)
        .collect();
    let first = tc
        .draw(gs::optional(gs::integers::<usize>().max_value(5)))
        .map(|n| levels[n]);
    let first_pick = tc.draw(gs::integers::<usize>().max_value(5));
    let next_pick = tc.draw(gs::integers::<usize>().max_value(5));
    let answer_words = tc.draw(gs::sampled_from(vec![10, 4_000]));
    let fork = tc.draw(gs::booleans());
    let sticky_after = 1_000;
    let (ran, next, choice) = conversation(
        first,
        first_pick,
        answer_words,
        fork,
        next_pick,
        sticky_after,
    );
    assert_eq!(ran, first.or(Some(levels[first_pick])));
    let picked = levels[next_pick];
    let long = answer_words >= 1_000;
    let expected = if long { ran } else { Some(picked) };
    assert_eq!(next, expected);
    assert_eq!(choice.runs_at.as_deref(), expected.map(|e| e.as_str()));
    match choice.kept_for_cache {
        Some(tokens) => {
            assert!(long && ran != Some(picked));
            assert!(tokens >= sticky_after);
        }
        None => assert!(!long || ran == Some(picked)),
    }
}
