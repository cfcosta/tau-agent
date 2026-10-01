//! The constitution in a real run: a scripted model, a fake Jev, and the
//! store. Calls a rule forbids are refused with the rule, doubtful ones
//! run and are flagged, a final answer that breaks a rule goes back, and
//! every decision is reported and recorded. Edited rules apply from the
//! next tool call, and a constitution comes back from the store as saved.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use futures_util::StreamExt;
use hegel::{
    TestCase,
    generators as gs,
    generators::{Generator as _, PrintableGenerator},
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use tau_agent::{
    agent::Agent,
    error::ToolError,
    event::{RunEvent, StopReason},
    tool::{ToolCtx, ToolOutput, TypedTool, typed},
};
use tau_ai::message::{InputBlock, Message};
use tau_constitution::{
    Check,
    Constitution,
    ConstitutionPlugin,
    Live,
    NAME,
    OnError,
    Target,
    Verdict,
    VerdictKind,
};
use tau_jev::{JevError, fake::FakeJev};
use tau_store::{Store, StoredConstitution, StoredRule};
use tau_testing::{block_on, scripted::ScriptedModel};

fn rule(id: &str, text: &str, on: &str, review: f64, block: f64) -> StoredRule {
    StoredRule {
        id: id.into(),
        text: text.into(),
        targets: vec![on.into()],
        review,
        block,
    }
}

/// R2 on what `write` writes, R6 on the final answer.
fn rules() -> Constitution {
    Constitution::from_stored(StoredConstitution {
        on_error: "allow".into(),
        max_holds: 3,
        rules: vec![
            rule(
                "R2",
                "Library code returns errors. No unwrap or expect.",
                "write.content",
                0.3,
                0.8,
            ),
            rule(
                "R6",
                "The final answer names the tests that ran.",
                "final answer",
                0.4,
                0.75,
            ),
        ],
    })
    .unwrap()
}

/// Writes nothing; counts its calls.
#[derive(Clone, Default)]
struct Write(Arc<AtomicUsize>);

#[derive(Deserialize, JsonSchema)]
struct WriteArgs {
    #[allow(dead_code)]
    path: String,
    #[allow(dead_code)]
    content: String,
}

#[async_trait]
impl TypedTool for Write {
    type Args = WriteArgs;
    const NAME: &'static str = "write";
    const DESCRIPTION: &'static str = "Writes a file.";
    async fn call(
        &self,
        _args: WriteArgs,
        _ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ToolOutput::text("written"))
    }
}

fn write(content: &str) -> Value {
    json!({ "path": "src/lib.rs", "content": content })
}

/// Runs `llm` with the constitution and a Jev that scores with `jev`,
/// and returns every event, the writes that ran, the requests Jev got,
/// and the plugin's stored records.
fn run(
    llm: &ScriptedModel,
    jev: FakeJev,
    rules: Constitution,
) -> (Vec<RunEvent>, usize, FakeJev, Vec<Value>) {
    let (events, writes, records) =
        block_on(run_with(llm, Arc::new(jev.clone()), rules));
    (events, writes, jev, records)
}

/// [`run`] with any Jev, returning the events, the writes that ran,
/// and the plugin's stored records.
async fn run_with(
    llm: &ScriptedModel,
    jev: Arc<dyn tau_jev::Jev>,
    rules: Constitution,
) -> (Vec<RunEvent>, usize, Vec<Value>) {
    let store = Store::memory().await.unwrap();
    let writes = Write::default();
    let agent = Agent::new(llm.clone())
        .tool(typed(writes.clone()))
        .plugin(ConstitutionPlugin::new(jev, rules));
    let mut run = agent.start("fix it", &store);
    let id = run.id();
    let events: Vec<RunEvent> = run.events().collect().await;
    run.outcome().await.unwrap();
    let records = store
        .plugin_entries(&id.0, NAME)
        .await
        .unwrap()
        .iter()
        .map(|(_, body)| serde_json::from_str(body).unwrap())
        .collect();
    (events, writes.0.load(Ordering::SeqCst), records)
}

fn verdicts(events: &[RunEvent]) -> Vec<Verdict> {
    events
        .iter()
        .filter_map(|event| match event {
            RunEvent::PluginReport { plugin, body, .. }
                if &**plugin == NAME =>
            {
                Verdict::parse(body)
            }
            _ => None,
        })
        .collect()
}

/// The text of the tool results the model saw, in its last request.
fn results(llm: &ScriptedModel) -> Vec<String> {
    let last = llm.requests().pop().unwrap();
    last.transcript
        .iter()
        .filter_map(|message| match message {
            Message::ToolResult(result) => Some(
                result
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        InputBlock::Text(text) => Some(text.text.clone()),
                        _ => None,
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect()
}

#[test]
fn a_call_that_breaks_a_rule_is_refused_with_the_rule() {
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("write", write("let x = y.unwrap();")))
        .turn(|t| t.text("I ran cargo test: 3 passed."));
    // R2 is broken; R6 holds.
    let jev = FakeJev::new(|request| {
        let state = request.state.to_string();
        let score = if state.contains("unwrap") { 0.95 } else { 0.05 };
        Ok(tau_jev::fake::response(
            request
                .questions
                .keys()
                .map(|id| (id.clone(), tau_jev::Answer::Noul { noul: score }))
                .collect(),
            request,
        ))
    });
    let (events, writes, jev, records) = run(&llm, jev, rules());

    assert_eq!(writes, 0, "the call did not run");
    let told = results(&llm);
    assert!(told[0].contains("Blocked by tau-constitution"), "{told:?}");
    assert!(told[0].contains("rule R2"), "{told:?}");
    assert!(told[0].contains("No unwrap or expect"), "{told:?}");

    // Jev saw the tool and only the field the rule names.
    let asked = &jev.requests()[0];
    assert_eq!(
        asked.state,
        json!({"tool": "write", "arguments": {"content": "let x = y.unwrap();"}})
    );
    assert_eq!(asked.questions.len(), 1);

    // The report comes before the ToolEnd it explains.
    let report = events
        .iter()
        .position(|event| matches!(event, RunEvent::PluginReport { .. }))
        .unwrap();
    let end = events
        .iter()
        .position(|event| matches!(event, RunEvent::ToolEnd { .. }))
        .unwrap();
    assert!(report < end);
    let found = verdicts(&events);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].kind, VerdictKind::Blocked);
    assert_eq!(found[0].rule, "R2");
    assert_eq!(found[0].tool.as_deref(), Some("write"));
    assert!(found[0].reason.is_some());

    // It is recorded with the run, for history.
    // The call's check with every score, its verdict, then the check
    // of the final answer, which passed.
    assert_eq!(records.len(), 3);
    let answer = Check::parse(&records[2]).unwrap();
    assert_eq!(
        (answer.call_id, answer.scores[0].rule.as_str()),
        (None, "R6")
    );
    let check = Check::parse(&records[0]).unwrap();
    assert_eq!(
        check.call_id.as_deref(),
        Some(found[0].call_id.as_deref().unwrap())
    );
    assert_eq!(check.scores[0].rule, "R2");
    assert!(check.cost > 0.0, "Jev's cost is counted");
    assert_eq!(Verdict::parse(&records[1]).unwrap(), found[0]);
}

#[test]
fn a_doubtful_call_runs_and_is_flagged() {
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("write", write("maybe")))
        .turn(|t| t.text("Tests: cargo test, all passed."));
    let jev = FakeJev::nouls(|_| 0.5);
    let (events, writes, _, _) = run(&llm, jev, rules());
    assert_eq!(writes, 1);
    let kinds: Vec<(VerdictKind, String)> = verdicts(&events)
        .into_iter()
        .map(|verdict| (verdict.kind, verdict.rule))
        .collect();
    // R2 on the call, then R6 on the answer: both only doubtful.
    assert_eq!(
        kinds,
        [
            (VerdictKind::Flagged, "R2".to_owned()),
            (VerdictKind::Flagged, "R6".to_owned())
        ]
    );
    assert_eq!(results(&llm), ["written"]);
}

#[test]
fn a_final_answer_that_breaks_a_rule_goes_back_until_the_cap() {
    // The first answer breaks R6; the second follows it.
    let llm = ScriptedModel::new()
        .turn(|t| t.text("Done."))
        .turn(|t| t.text("Done. cargo test: 12 passed."));
    let asked = Arc::new(AtomicUsize::new(0));
    let count = asked.clone();
    let jev = FakeJev::nouls(move |_| {
        if count.fetch_add(1, Ordering::SeqCst) == 0 {
            0.9
        } else {
            0.1
        }
    });
    let (events, _, _, _) = run(&llm, jev, rules());
    let held: Vec<&RunEvent> = events
        .iter()
        .filter(|event| matches!(event, RunEvent::Continued { .. }))
        .collect();
    assert_eq!(held.len(), 1);
    let RunEvent::Continued {
        plugin, message, ..
    } = held[0]
    else {
        unreachable!()
    };
    assert_eq!(&**plugin, NAME);
    assert!(message.contains("rule R6"), "{message}");
    let held = &verdicts(&events)[0];
    assert_eq!(held.kind, VerdictKind::Held);
    assert_eq!((held.hold, held.max_holds), (Some(1), Some(3)));
    assert!(matches!(
        events.last(),
        Some(RunEvent::RunEnd {
            stop: StopReason::Stop,
            ..
        })
    ));

    // Past the cap, a broken answer stands, flagged.
    let mut capped = rules();
    capped.max_holds = 1;
    let llm = ScriptedModel::new()
        .turn(|t| t.text("Done."))
        .turn(|t| t.text("Still done."));
    let (events, _, _, _) = run(&llm, FakeJev::nouls(|_| 0.9), capped);
    let kinds: Vec<VerdictKind> = verdicts(&events)
        .iter()
        .map(|verdict| verdict.kind)
        .collect();
    assert_eq!(kinds, [VerdictKind::Held, VerdictKind::Flagged]);
    llm.assert_exhausted();
}

#[test]
fn when_jev_cannot_answer_the_constitution_decides() {
    let failing =
        || FakeJev::new(|_| Err(JevError::Transport("offline".into())));
    let script = || {
        ScriptedModel::new()
            .turn(|t| t.tool_call("write", write("x")))
            .turn(|t| t.text("ok"))
    };
    // By default the call runs, and the failure is reported.
    let llm = script();
    // R6 moved somewhere no call reaches, so only the write is checked.
    let mut rules = rules();
    rules.rules[1].on = vec![Target::parse("none.none").unwrap()];
    let (events, writes, _, _) = run(&llm, failing(), rules.clone());
    assert_eq!(writes, 1);
    assert!(events.iter().any(|event| matches!(
        event,
        RunEvent::PluginReport { body, .. } if body["kind"] == "error"
    )));
    // `on_error = "block"` refuses what it cannot check.
    let llm = script();
    let mut strict = rules;
    strict.on_error = OnError::Block;
    let (_, writes, _, _) = run(&llm, failing(), strict);
    assert_eq!(writes, 0);
    assert!(results(&llm)[0].contains("cannot check"));
}

/// Whenever Jev fails, `on_error` decides, for calls and final answers
/// alike, and the failure is reported and recorded as it happened:
/// `allow` lets the call run and the answer stand; `block` refuses the
/// call, and sends the answer back while holds are left.
#[hegel::test(test_cases = 60)]
fn on_error_decides_wherever_jev_fails(tc: TestCase) {
    let block = tc.draw(gs::booleans());
    let max_holds = tc.draw(gs::integers::<u32>().max_value(3));
    // Whether each of Jev's requests fails; past the list, they pass.
    let fails: Vec<bool> = tc.draw(gs::vecs(gs::booleans()).max_size(8));
    let writes = tc.draw(gs::integers::<usize>().max_value(2));
    let mut rules = rules();
    rules.on_error = if block {
        OnError::Block
    } else {
        OnError::Allow
    };
    rules.max_holds = max_holds;
    // One reply per write, then answers enough for every hold.
    let mut llm = ScriptedModel::new();
    for _ in 0..writes {
        llm = llm.turn(|t| t.tool_call("write", write("x")));
    }
    for _ in 0..=max_holds {
        llm = llm.turn(|t| t.text("Done."));
    }
    let fails = Arc::new(std::sync::Mutex::new(fails.into_iter()));
    let jev = FakeJev::new(move |request| {
        if fails.lock().unwrap().next().unwrap_or(false) {
            return Err(JevError::Transport("offline".into()));
        }
        // Passing: every rule kept.
        let answers = request
            .questions
            .keys()
            .map(|id| (id.clone(), tau_jev::Answer::Noul { noul: 0.0 }))
            .collect();
        Ok(tau_jev::fake::response(answers, request))
    });
    let (events, ran, records) = block_on(run_with(&llm, Arc::new(jev), rules));
    let reports: Vec<Value> = events
        .iter()
        .filter_map(|event| match event {
            RunEvent::PluginReport { plugin, body, .. }
                if &**plugin == NAME =>
            {
                Some(body.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(records, reports, "what is reported is recorded");
    let failed_calls = reports
        .iter()
        .filter(|body| body["kind"] == "error" && body["call_id"].is_string())
        .count();
    assert_eq!(ran, writes - if block { failed_calls } else { 0 });
    let continued = events
        .iter()
        .filter(|event| matches!(event, RunEvent::Continued { .. }))
        .count();
    let held: Vec<bool> = reports
        .iter()
        .filter(|body| body["kind"] == "error" && body["call_id"].is_null())
        .map(|body| body["held"].as_bool().unwrap())
        .collect();
    assert_eq!(continued, held.iter().filter(|held| **held).count());
    assert!(continued as u32 <= max_holds);
    if !block {
        assert_eq!(continued, 0, "allow lets every answer stand");
    } else {
        // Each failure holds until the holds run out.
        for (n, held) in held.iter().enumerate() {
            assert_eq!(*held, (n as u32) < max_holds, "{held:?}");
        }
    }
    assert!(matches!(
        events.last(),
        Some(RunEvent::RunEnd {
            stop: StopReason::Stop,
            ..
        })
    ));
}

/// Writes nothing, like [`Write`], and on its first call replaces the
/// rules with `then`: an edit made in the UI while the run goes on.
#[derive(Clone)]
struct EditsRules {
    writes: Write,
    live: Live,
    then: Constitution,
}

#[async_trait]
impl TypedTool for EditsRules {
    type Args = WriteArgs;
    const NAME: &'static str = "write";
    const DESCRIPTION: &'static str = "Writes a file.";
    async fn call(
        &self,
        args: WriteArgs,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        self.live.set(self.then.clone());
        self.writes.call(args, ctx).await
    }
}

#[test]
fn an_edit_applies_from_the_next_tool_call() {
    let live = Live::new(Constitution::default());
    let tool = EditsRules {
        writes: Write::default(),
        live: live.clone(),
        then: rules(),
    };
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("write", write("x.unwrap()")))
        .turn(|t| t.tool_call("write", write("y.unwrap()")))
        .turn(|t| t.text("ok, ran cargo test"));
    let jev = FakeJev::nouls(|_| 0.9);
    let events: Vec<RunEvent> = block_on(async {
        let store = Store::memory().await.unwrap();
        let agent = Agent::new(llm.clone())
            .tool(typed(tool.clone()))
            .plugin(ConstitutionPlugin::live(Arc::new(jev), live));
        let mut run = agent.start("fix it", &store);
        let events = run.events().collect().await;
        run.outcome().await.unwrap();
        events
    });
    // No rules at the first call: it ran unchecked. The rules it put in
    // place checked the second, in the same run, and refused it.
    assert_eq!(tool.writes.0.load(Ordering::SeqCst), 1);
    let first = verdicts(&events);
    assert_eq!(first[0].kind, VerdictKind::Blocked, "{first:?}");
    assert_eq!(first[0].rule, "R2");
}

fn constitution() -> impl PrintableGenerator<Constitution> {
    // Constitution is tau's own type, so its drawn values print through Debug.
    constitution_unprinted().print_as_debug()
}

#[hegel::composite]
fn constitution_unprinted(tc: &TestCase) -> Constitution {
    let mut constitution = Constitution {
        on_error: tc.draw(
            gs::sampled_from(vec![OnError::Allow, OnError::Block])
                .print_as_debug(),
        ),
        max_holds: tc.draw(gs::integers::<u32>().max_value(9)),
        ..Constitution::default()
    };
    for _ in 0..tc.draw(gs::integers::<usize>().max_value(5)) {
        let review = tc.draw(gs::floats::<f64>().min_value(0.0).max_value(1.0));
        let block =
            tc.draw(gs::floats::<f64>().min_value(review).max_value(1.0));
        let on: Vec<String> = tc.draw(
            gs::vecs(gs::sampled_from(vec![
                "write.content".to_owned(),
                "bash.command".to_owned(),
                "final answer".to_owned(),
            ]))
            .min_size(1)
            .max_size(3),
        );
        let text = format!("rule {}", tc.draw(gs::text().max_size(20)));
        constitution.add(&text, &on, review, block).unwrap();
        // Removing one now and then leaves gaps in the ids.
        if tc.draw(gs::booleans()) {
            let id = constitution.rules[0].id.clone();
            constitution.remove(&id);
        }
    }
    constitution
}

/// What is saved for a repository comes back as saved, and none saved is
/// no rules.
#[hegel::test(test_cases = 100)]
fn a_constitution_comes_back_from_the_store_as_saved(tc: TestCase) {
    let constitution = tc.draw(constitution());
    block_on(async {
        let store = Store::memory().await.unwrap();
        assert_eq!(
            Constitution::load(&store, "/repo").await.unwrap(),
            Constitution::default()
        );
        constitution.save(&store, "/repo").await.unwrap();
        assert_eq!(
            Constitution::load(&store, "/repo").await.unwrap(),
            constitution
        );
        assert_eq!(
            Constitution::load(&store, "/other").await.unwrap(),
            Constitution::default()
        );
    });
}

/// Asks the real Jev about a call that breaks a rule and one that does
/// not. Needs `TYPESAFE_API_KEY` and the network:
/// `cargo test -p tau-constitution -- --ignored`.
#[test]
#[ignore = "needs TYPESAFE_API_KEY and the network"]
fn the_real_jev_tells_a_broken_rule_from_a_kept_one() {
    let jev = tau_jev::TypeSafe::from_env().expect("TYPESAFE_API_KEY");
    // The network needs a runtime with I/O.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let rules = Constitution::from_stored(StoredConstitution {
        on_error: "allow".into(),
        max_holds: 3,
        rules: vec![rule(
            "R1",
            "Library code never calls unwrap() or expect(); it returns errors with ?.",
            "write.content",
            0.5,
            0.8,
        )],
    })
    .unwrap();
    let script = |content: &'static str| {
        ScriptedModel::new()
            .turn(move |t| t.tool_call("write", write(content)))
            .turn(|t| t.text("done"))
    };
    let broken = script("pub fn port(s: &str) -> u16 { s.parse().unwrap() }");
    let (events, writes, _) = runtime.block_on(run_with(
        &broken,
        Arc::new(jev.clone()),
        rules.clone(),
    ));
    let verdict = &verdicts(&events)[0];
    println!("broken: {:.3}", verdict.score);
    assert_eq!(verdict.kind, VerdictKind::Blocked);
    assert_eq!(writes, 0);

    let kept = script(
        "pub fn port(s: &str) -> Result<u16, ParseIntError> { s.parse() }",
    );
    let (events, writes, _) =
        runtime.block_on(run_with(&kept, Arc::new(jev), rules));
    println!("kept: {:?}", verdicts(&events));
    assert!(
        verdicts(&events)
            .iter()
            .all(|v| v.kind != VerdictKind::Blocked)
    );
    assert_eq!(writes, 1);
}

#[test]
fn a_rule_is_edited_in_place() {
    let mut rules = rules();
    rules
        .replace(
            "R2",
            "No unwrap anywhere.",
            &["edit.newText".into()],
            0.2,
            0.6,
        )
        .unwrap();
    let r2 = &rules.rules[0];
    assert_eq!(
        (r2.id.as_str(), r2.text.as_str()),
        ("R2", "No unwrap anywhere.")
    );
    assert_eq!((r2.review, r2.block), (0.2, 0.6));
    assert_eq!(rules.rules[1].id, "R6", "the order stays");
    // Checked: thresholds in order, somewhere to apply.
    assert!(rules.replace("R2", "x", &[], 0.2, 0.6).is_err());
    assert!(
        rules
            .replace("R2", "x", &["edit.newText".into()], 0.9, 0.6)
            .is_err()
    );
    assert!(
        rules
            .replace("R9", "x", &["edit.newText".into()], 0.2, 0.6)
            .is_err()
    );
    // The round trip keeps it.
    let again = Constitution::from_stored(rules.to_stored()).unwrap();
    assert_eq!(again, rules);
}

#[test]
fn a_rule_is_tried_on_what_a_check_would_show() {
    let rules = rules();
    let jev = FakeJev::nouls(|_| 0.9);
    let calls = vec![
        ("write".to_owned(), write("x.unwrap()")),
        // Not about write.content: skipped.
        ("bash".to_owned(), json!({ "command": "ls" })),
        ("write".to_owned(), write("Ok(x)")),
    ];
    let answers = vec!["Done.".to_owned()];
    let (trials, cost) = block_on(tau_constitution::try_rule(
        &jev,
        &rules.rules[0],
        &calls,
        &answers,
    ))
    .unwrap();
    // R2 is on write.content, not the final answer.
    let shown: Vec<&str> = trials.iter().map(|t| t.shown.as_str()).collect();
    assert_eq!(shown, ["x.unwrap()", "Ok(x)"]);
    assert!(trials.iter().all(|t| t.tool.as_deref() == Some("write")));
    assert!(cost > 0.0);
    // Jev saw what the check sees: the tool and the field, nothing else.
    let requests = jev.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].state,
        json!({ "tool": "write", "arguments": { "content": "x.unwrap()" } })
    );

    // A final-answer rule tries the answers.
    let (trials, _) = block_on(tau_constitution::try_rule(
        &jev,
        &rules.rules[1],
        &calls,
        &answers,
    ))
    .unwrap();
    assert_eq!(trials.len(), 1);
    assert_eq!(
        (trials[0].tool.as_deref(), trials[0].shown.as_str()),
        (None, "Done.")
    );
}
