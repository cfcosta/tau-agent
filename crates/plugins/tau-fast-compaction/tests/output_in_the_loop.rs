//! Output pruning as a plugin of an `Agent`: a fake `exec_command` whose output
//! the tests choose, `ScriptedModel`, and a `FakeJev` that keeps the
//! chunks holding an error.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{Value, json};
use tau_agent::{
    agent::{Agent, Outcome},
    error::ToolError,
    event::RunEvent,
    tool::{AgentTool, ToolCtx, ToolOutput},
};
use tau_ai::message::{InputBlock, Message};
use tau_fast_compaction::{
    FastCompaction,
    NAME,
    OutputStats,
    Settings,
    output::HEADER,
};
use tau_jev::{Answer, JevError, fake::FakeJev};
use tau_testing::{block_on, scripted::ScriptedModel};

/// A fresh directory for one test's files.
fn scratch(name: &str) -> PathBuf {
    static CASE: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "tau-fast-compaction-{name}-{}-{}",
        std::process::id(),
        CASE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// How the fake `exec_command` answers.
#[derive(Clone)]
enum Answering {
    /// The whole output.
    Whole,
    /// A failed command, with the whole output.
    Failed,
}

/// `exec_command` (or another tool by `named`), answering every call with
/// the same output.
struct Exec {
    name: &'static str,
    output: String,
    answering: Answering,
    schema: Value,
}

impl Exec {
    fn new(output: String, answering: Answering) -> Self {
        Self {
            name: "exec_command",
            output,
            answering,
            schema: json!({
                "type": "object",
                "properties": {"cmd": {"type": "string"}},
            }),
        }
    }

    fn named(mut self, name: &'static str) -> Self {
        self.name = name;
        self
    }
}

#[async_trait]
impl AgentTool for Exec {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        "Runs a command."
    }
    fn parameters(&self) -> &Value {
        &self.schema
    }
    async fn call(
        &self,
        _args: Value,
        _ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        match &self.answering {
            Answering::Whole => Ok(ToolOutput::text(self.output.clone())),
            Answering::Failed => {
                Err(format!("{}\n\nCommand exited with code 101", self.output)
                    .into())
            }
        }
    }
}

/// A build log of `lines` lines, with an error halfway.
fn build_log(lines: usize) -> String {
    (0..lines)
        .map(|n| {
            if n == lines / 2 {
                "error[E0425]: cannot find value `widget` in this scope".into()
            } else {
                format!("   Compiling crate-number-{n} v0.1.0 (/src/crate-{n})")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Keeps a chunk when its text has an error, and drops the rest.
fn keeps_errors() -> FakeJev {
    FakeJev::new(|request| {
        let texts: std::collections::BTreeMap<String, String> =
            request.state["chunks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|chunk| {
                    (
                        chunk["id"].as_str().unwrap().to_owned(),
                        chunk["text"].as_str().unwrap().to_owned(),
                    )
                })
                .collect();
        let answers = request
            .questions
            .keys()
            .map(|id| {
                let noul = if texts[id].contains("error[") {
                    0.95
                } else {
                    0.02
                };
                (id.clone(), Answer::Noul { noul })
            })
            .collect();
        Ok(tau_jev::fake::response(answers, request))
    })
}

/// One `cargo build`, then an answer.
fn one_build() -> ScriptedModel {
    ScriptedModel::new()
        .turn(|t| t.tool_call("exec_command", json!({"cmd": "cargo build"})))
        .turn(|t| t.text("done"))
}

fn settings(archive_dir: &Path) -> Settings {
    Settings {
        archive_dir: archive_dir.to_owned(),
        ..Settings::default()
    }
}

async fn run(
    model: &ScriptedModel,
    exec: Exec,
    jev: FakeJev,
    settings: Settings,
) -> (Vec<RunEvent>, Outcome) {
    let store = tau_store_sqlite::memory().await.unwrap();
    let agent = Agent::new(model.clone())
        .tool(exec)
        .plugin(FastCompaction::new(jev).settings(settings));
    let mut run = agent.start("fix the build", &store);
    let events = run.events().collect().await;
    (events, run.outcome().await.unwrap())
}

/// The text of the result the model's second request ends with.
fn seen_result(model: &ScriptedModel) -> String {
    let requests = model.requests();
    match requests[1].transcript.last().unwrap() {
        Message::ToolResult(result) => match &result.content[..] {
            [InputBlock::Text(text)] => text.text.clone(),
            other => panic!("{other:?}"),
        },
        other => panic!("expected a tool result, got {other:?}"),
    }
}

fn output_reports(events: &[RunEvent]) -> Vec<OutputStats> {
    events
        .iter()
        .filter_map(|event| match event {
            RunEvent::PluginReport { plugin, body, .. }
                if &**plugin == NAME && body["kind"] == "output" =>
            {
                Some(serde_json::from_value(body.clone()).unwrap())
            }
            _ => None,
        })
        .collect()
}

/// The archive a pruned output names in its footer.
fn footer_archive(text: &str) -> String {
    let (_, rest) = text.rsplit_once("[full output: ").unwrap();
    rest.strip_suffix(" (read or grep it if needed)]")
        .unwrap()
        .to_owned()
}

/// A large output reaches the model pruned: the header, the error line
/// from its middle, markers for what went, and a footer naming an
/// archive, readable only by its owner, that holds the output whole.
/// Jev saw the command, the task and the chunks; its usage is charged,
/// and a report says how it went.
#[test]
fn a_large_output_reaches_the_model_pruned() {
    let dir = scratch("whole");
    let log = build_log(3000);
    let model = one_build();
    let jev = keeps_errors();
    block_on(async {
        let (events, outcome) = run(
            &model,
            Exec::new(log.clone(), Answering::Whole),
            jev.clone(),
            settings(&dir),
        )
        .await;
        assert_eq!(outcome.text, "done");
        let seen = seen_result(&model);
        assert!(seen.starts_with(HEADER), "{seen}");
        assert!(seen.contains("error[E0425]: cannot find value `widget`"));
        assert!(seen.contains(" lines omitted]"));
        assert!(
            seen.len() < log.len() / 2,
            "{} of {}",
            seen.len(),
            log.len()
        );
        let archive = footer_archive(&seen);
        assert!(archive.starts_with(dir.to_str().unwrap()));
        assert_eq!(std::fs::read_to_string(&archive).unwrap(), log);
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&archive).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);

        let asked = jev.requests();
        assert!(!asked.is_empty() && asked.len() <= 12);
        assert_eq!(asked[0].state["command"], "cargo build");
        assert_eq!(asked[0].state["task"], "fix the build");
        let history = asked[0].state["history"].to_string();
        assert!(history.contains("fix the build"), "{history}");
        assert!(outcome.usage.cost.total > 0.0, "Jev's usage is charged");

        let reports = output_reports(&events);
        assert_eq!(reports.len(), 1);
        assert!(reports[0].pruned);
        assert_eq!(reports[0].archive.as_deref(), Some(archive.as_str()));
        assert!(reports[0].tokens_after < reports[0].tokens_before);
        assert!(reports[0].dropped_lines > 0);
        assert_eq!(reports[0].lines, 3000);
        // The report's cost is what the plugin charged for it.
        let charged: f64 = events
            .iter()
            .filter_map(|event| match event {
                RunEvent::PluginCharged { plugin, usage, .. }
                    if &**plugin == NAME =>
                {
                    Some(usage.cost.total)
                }
                _ => None,
            })
            .sum();
        assert!(charged > 0.0);
        assert_eq!(reports[0].cost, charged);
    });
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The report is also stored with the run, for history to read back.
#[test]
fn the_report_is_recorded() {
    let dir = scratch("recorded");
    let model = one_build();
    block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let agent = Agent::new(model.clone())
            .tool(Exec::new(build_log(3000), Answering::Whole))
            .plugin(
                FastCompaction::new(keeps_errors()).settings(settings(&dir)),
            );
        let mut run = agent.start("fix the build", &store);
        let id = run.id();
        let events: Vec<RunEvent> = run.events().collect().await;
        run.outcome().await.unwrap();
        let reports = output_reports(&events);
        let records: Vec<OutputStats> = store
            .records(&id.0, NAME)
            .await
            .unwrap()
            .iter()
            .map(|body| {
                let body: Value = serde_json::from_str(body).unwrap();
                assert_eq!(body["kind"], "output");
                serde_json::from_value(body).unwrap()
            })
            .collect();
        assert_eq!(records, reports);
        assert!(records[0].pruned);
    });
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A `write_stdin` result is pruned like an `exec_command` one; Jev is
/// told what was written.
#[test]
fn a_write_stdin_result_is_pruned_too() {
    let dir = scratch("stdin");
    let log = build_log(3000);
    let model = ScriptedModel::new()
        .turn(|t| {
            t.tool_call("write_stdin", json!({"process_id": 7, "chars": "y\n"}))
        })
        .turn(|t| t.text("done"));
    let jev = keeps_errors();
    block_on(async {
        let (events, _) = run(
            &model,
            Exec::new(log, Answering::Whole).named("write_stdin"),
            jev.clone(),
            settings(&dir),
        )
        .await;
        let seen = seen_result(&model);
        assert!(seen.starts_with(HEADER), "{seen}");
        assert!(seen.contains("error[E0425]"));
        assert_eq!(jev.requests()[0].state["command"], "write_stdin: y\n");
        assert!(output_reports(&events)[0].pruned);
    });
    std::fs::remove_dir_all(&dir).unwrap();
}

/// An output under the threshold, and a failed command however large,
/// reach the model as they were, without a Jev call.
#[test]
fn small_or_failed_outputs_are_untouched() {
    let dir = scratch("untouched");
    for (log, answering, expected) in [
        (build_log(50), Answering::Whole, build_log(50)),
        (
            build_log(3000),
            Answering::Failed,
            format!("{}\n\nCommand exited with code 101", build_log(3000)),
        ),
    ] {
        let model = one_build();
        let jev = keeps_errors();
        block_on(async {
            let (events, _) = run(
                &model,
                Exec::new(log, answering),
                jev.clone(),
                settings(&dir),
            )
            .await;
            assert_eq!(seen_result(&model), expected);
            assert!(jev.requests().is_empty());
            assert!(output_reports(&events).is_empty());
        });
    }
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// With output pruning off, a large output is untouched too.
#[test]
fn output_pruning_can_be_off() {
    let dir = scratch("off");
    let log = build_log(3000);
    let model = one_build();
    let jev = keeps_errors();
    let mut settings = settings(&dir);
    settings.output.enabled = false;
    block_on(async {
        run(
            &model,
            Exec::new(log.clone(), Answering::Whole),
            jev.clone(),
            settings,
        )
        .await;
    });
    assert_eq!(seen_result(&model), log);
    assert!(jev.requests().is_empty());
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A Jev failure leaves the output as it was, reported as a plugin
/// error, and writes no archive.
#[test]
fn a_jev_failure_leaves_the_output() {
    let dir = scratch("failure");
    let log = build_log(3000);
    let model = one_build();
    block_on(async {
        let jev = FakeJev::new(|_| Err(JevError::Status(503)));
        let (events, outcome) = run(
            &model,
            Exec::new(log.clone(), Answering::Whole),
            jev,
            settings(&dir),
        )
        .await;
        assert_eq!(outcome.text, "done");
        assert_eq!(seen_result(&model), log);
        let errors: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                RunEvent::PluginError {
                    plugin, message, ..
                } if &**plugin == NAME => Some(message.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(errors, ["Jev answered with status 503"]);
    });
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// When Jev keeps everything, the output is untouched: nothing went, so
/// nothing would be saved; the report says so.
#[test]
fn keeping_everything_leaves_the_output() {
    let dir = scratch("kept");
    let log = build_log(3000);
    let model = one_build();
    block_on(async {
        let (events, _) = run(
            &model,
            Exec::new(log.clone(), Answering::Whole),
            FakeJev::nouls(|_| 0.9),
            settings(&dir),
        )
        .await;
        assert_eq!(seen_result(&model), log);
        let reports = output_reports(&events);
        assert_eq!(reports.len(), 1);
        assert!(!reports[0].pruned);
        assert_eq!(reports[0].dropped_lines, 0);
    });
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    std::fs::remove_dir_all(&dir).unwrap();
}
