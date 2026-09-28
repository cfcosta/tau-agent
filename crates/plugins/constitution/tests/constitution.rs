//! The constitution in a real run: a scripted model, a fake Jev, and the
//! store. Calls a rule forbids are refused with the rule, doubtful ones
//! run and are flagged, a final answer that breaks a rule goes back, and
//! every decision is reported and recorded.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use futures_util::StreamExt;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use tau_agent::{
    agent::Agent,
    event::{RunEvent, StopReason},
    tool::{ToolCtx, ToolOutput, TypedTool, typed},
};
use tau_ai::message::{InputBlock, Message};
use tau_constitution::{
    Constitution,
    ConstitutionPlugin,
    NAME,
    Verdict,
    VerdictKind,
};
use tau_jev::{JevError, fake::FakeJev};
use tau_store::Store;
use tau_testing::{block_on, scripted::ScriptedModel};

const RULES: &str = r#"
    [[rule]]
    id = "R2"
    text = "Library code returns errors. No unwrap or expect."
    on = ["write.content"]
    review = 0.3
    block = 0.8

    [[rule]]
    id = "R6"
    text = "The final answer names the tests that ran."
    on = ["final answer"]
    review = 0.4
    block = 0.75
"#;

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
    ) -> anyhow::Result<ToolOutput> {
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
    rules: &str,
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
    rules: &str,
) -> (Vec<RunEvent>, usize, Vec<Value>) {
    let store = Store::memory().await.unwrap();
    let writes = Write::default();
    let agent = Agent::new(llm.clone()).tool(typed(writes.clone())).plugin(
        ConstitutionPlugin::new(jev, Constitution::parse(rules).unwrap()),
    );
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
    let (events, writes, jev, records) = run(&llm, jev, RULES);

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
    assert_eq!(records.len(), 1);
    assert_eq!(Verdict::parse(&records[0]).unwrap(), found[0]);
}

#[test]
fn a_doubtful_call_runs_and_is_flagged() {
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("write", write("maybe")))
        .turn(|t| t.text("Tests: cargo test, all passed."));
    let jev = FakeJev::nouls(|_| 0.5);
    let (events, writes, _, _) = run(&llm, jev, RULES);
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
    let (events, _, _, _) = run(&llm, jev, RULES);
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
    assert_eq!(verdicts(&events)[0].kind, VerdictKind::Held);
    assert!(matches!(
        events.last(),
        Some(RunEvent::RunEnd {
            stop: StopReason::Stop,
            ..
        })
    ));

    // Past the cap, a broken answer stands, flagged.
    let capped = format!("max_holds = 1\n{RULES}");
    let llm = ScriptedModel::new()
        .turn(|t| t.text("Done."))
        .turn(|t| t.text("Still done."));
    let (events, _, _, _) = run(&llm, FakeJev::nouls(|_| 0.9), &capped);
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
    let rules =
        RULES.replace("on = [\"final answer\"]", "on = [\"none.none\"]");
    let (events, writes, _, _) = run(&llm, failing(), &rules);
    assert_eq!(writes, 1);
    assert!(events.iter().any(|event| matches!(
        event,
        RunEvent::PluginReport { body, .. } if body["kind"] == "error"
    )));
    // `on_error = "block"` refuses what it cannot check.
    let llm = script();
    let strict = format!("on_error = \"block\"\n{rules}");
    let (_, writes, _, _) = run(&llm, failing(), &strict);
    assert_eq!(writes, 0);
    assert!(results(&llm)[0].contains("cannot check"));
}

#[test]
fn a_broken_constitution_file_fails_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("constitution.toml");
    std::fs::write(&path, "[[rule]]\nid = \"A\"\n").unwrap();
    block_on(async {
        let store = Store::memory().await.unwrap();
        let agent = Agent::new(ScriptedModel::new()).plugin(
            ConstitutionPlugin::from_file(
                Arc::new(FakeJev::nouls(|_| 0.0)),
                &path,
            ),
        );
        let error = agent.run("go", &store).await.unwrap_err();
        assert!(error.to_string().contains("constitution.toml"), "{error}");
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
    let rules = r#"
        [[rule]]
        id = "R1"
        text = "Library code never calls unwrap() or expect(); it returns errors with ?."
        on = ["write.content"]
    "#;
    let script = |content: &'static str| {
        ScriptedModel::new()
            .turn(move |t| t.tool_call("write", write(content)))
            .turn(|t| t.text("done"))
    };
    let broken = script("pub fn port(s: &str) -> u16 { s.parse().unwrap() }");
    let (events, writes, _) =
        runtime.block_on(run_with(&broken, Arc::new(jev.clone()), rules));
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
