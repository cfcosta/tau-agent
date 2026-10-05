//! `bash` under a pseudo-terminal (the `terminal` feature), against
//! `docs/reference/tools.md` ("bash: terminal mode"): the model's text,
//! the byte stream in the updates and the result's details. The
//! terminal's own behavior is tested in tau-terminal.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0027)"
)]
#![cfg(all(unix, feature = "terminal"))]

use base64::Engine as _;
use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_agent::{
    error::ToolError,
    tool::{AgentTool, RunId, ToolCtx, ToolOutput, ToolUpdates},
};
use tau_ai::message::InputBlock;
use tau_terminal::{Options, Size, Terminal};
use tau_tools::{
    bash::{Accumulator, Bash},
    path::Root,
};
use tokio_util::sync::CancellationToken;

fn text_of(output: &ToolOutput) -> &str {
    match &output.content[0] {
        InputBlock::Text(text) => &text.text,
        InputBlock::Image(_) => panic!("expected a text block"),
    }
}

fn decode(value: &Value) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(value.as_str().expect("base64 text"))
        .unwrap()
}

/// What a call produced: its result, and its updates in order.
struct Called {
    result: Result<ToolOutput, ToolError>,
    updates: Vec<ToolOutput>,
}

impl Called {
    fn structured(&self) -> &Value {
        let output = match &self.result {
            Ok(output) => output,
            Err(ToolError::Output(output)) => output,
            Err(other) => panic!("a failure with no output: {other}"),
        };
        output.structured.as_ref().expect("structured bash result")
    }

    /// The result's text, failed or not.
    fn text(&self) -> String {
        match &self.result {
            Ok(output) => text_of(output).to_owned(),
            Err(error) => error.to_string(),
        }
    }

    /// The result's details, failed or not.
    fn details(&self) -> &Value {
        let output = match &self.result {
            Ok(output) => output,
            Err(ToolError::Output(output)) => output,
            Err(other) => panic!("a failure with no output: {other}"),
        };
        &output.details.as_ref().expect("the result has details")["term"]
    }

    /// The raw bytes of the updates' chunks, checking they come in
    /// order with no gap.
    fn streamed(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut next = 0;
        for update in &self.updates {
            let Some(details) = &update.details else {
                continue;
            };
            let chunk = &details["term"];
            assert_eq!(chunk["seq"], json!(next), "chunks in order");
            next += 1;
            bytes.extend(decode(&chunk["bytes"]));
        }
        assert_eq!(self.details()["chunks"], json!(next));
        bytes
    }
}

async fn call(bash: &Bash, args: Value, cancel: CancellationToken) -> Called {
    let (updates, mut receiver) = ToolUpdates::channel("call_1");
    let ctx = ToolCtx::new(cancel, updates, RunId("run_1".into()));
    let result = bash.call(args, ctx).await;
    let mut updates = Vec::new();
    while let Ok((_, update)) = receiver.try_recv() {
        updates.push(update);
    }
    Called { result, updates }
}

async fn run(command: &str) -> Called {
    let dir = tempfile::tempdir().unwrap();
    let bash = Bash::new(Root::new(dir.path()));
    call(&bash, json!({"command": command}), CancellationToken::new()).await
}

/// The result's text: the spill notice's path is left out, since two
/// runs spill to two files.
fn comparable(text: &str) -> String {
    match text.split_once("\n\nFull output: ") {
        Some((kept, _)) => format!("{kept}\n\nFull output: <spill>"),
        None => text.to_owned(),
    }
}

fn spill(text: &str) -> Option<String> {
    let (_, path) = text.split_once("\n\nFull output: ")?;
    Some(std::fs::read_to_string(path.trim()).unwrap())
}

// -- The model's text --------------------------------------------------

/// Characters plain output is made of: no control characters, no tab.
const PLAIN: [char; 9] = ['a', 'Z', '0', ' ', '-', 'é', '€', '日', '🙂'];

/// Plain output: lines of [`PLAIN`], some longer than the terminal is
/// wide, joined by `\n`.
#[hegel::composite]
fn plain_output(tc: &TestCase) -> String {
    let lines: Vec<Vec<char>> = tc.draw(
        gs::vecs(gs::vecs(gs::sampled_from(PLAIN.to_vec())).max_size(300))
            .max_size(30),
    );
    let mut text = lines
        .iter()
        .map(|line| line.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    if tc.draw(gs::booleans()) {
        text.push('\n');
    }
    text
}

/// Plain output (no escapes, no `\r`) gives the model exactly the text
/// pipe mode gives: the same `truncate_tail`, the same "was it cut", the
/// same totals and the same spill, whatever the limits and however the
/// terminal's reads chunk it (`docs/reference/tools.md`, "bash:
/// terminal mode").
#[hegel::test(test_cases = 100)]
fn plain_output_gives_the_text_pipes_give(tc: TestCase) {
    let text = tc.draw(plain_output());
    let max_lines = tc.draw(gs::integers::<usize>().min_value(1).max_value(12));
    let max_bytes =
        tc.draw(gs::integers::<usize>().min_value(1).max_value(400));

    let pipe_dir = tempfile::tempdir().unwrap();
    let mut pipe = Accumulator::new(max_lines, max_bytes, pipe_dir.path());
    pipe.append(text.as_bytes());
    pipe.finish();

    // What the pseudo-terminal hands the tool: `\n` as `\r\n`, in reads
    // of any size, through a recording terminal, into the accumulator.
    let raw = text.replace('\n', "\r\n").into_bytes();
    let mut cuts: Vec<usize> = tc.draw(
        gs::vecs(gs::integers::<usize>().min_value(0).max_value(raw.len()))
            .max_size(10),
    );
    cuts.extend([0, raw.len()]);
    cuts.sort_unstable();
    cuts.dedup();
    let term_dir = tempfile::tempdir().unwrap();
    let mut term = Accumulator::new(max_lines, max_bytes, term_dir.path());
    let mut terminal = Terminal::new(Options {
        size: Size::TOOL,
        record: true,
        ..Options::default()
    })
    .unwrap();
    for window in cuts.windows(2) {
        terminal.write(&raw[window[0]..window[1]]).unwrap();
        term.append(terminal.take_recorded().as_bytes());
    }
    term.append(terminal.text().unwrap().as_bytes());
    term.finish();

    let (a, b) = (pipe.snapshot(), term.snapshot());
    assert_eq!(b.content, a.content);
    assert_eq!(b.truncated(), a.truncated());
    assert_eq!(b.total_lines, a.total_lines);
    assert_eq!(b.total_bytes, a.total_bytes);
    match (pipe.spill_path(), term.spill_path()) {
        (Some(a), Some(b)) => {
            assert_eq!(std::fs::read(b).unwrap(), std::fs::read(a).unwrap());
        }
        (None, None) => {}
        other => panic!("one spilled and the other did not: {other:?}"),
    }
}

/// Real commands with plain output give the same result in both modes,
/// spill included: the spill holds plain text.
#[tokio::test(flavor = "multi_thread")]
async fn plain_commands_read_the_same_with_pipes() {
    let dir = tempfile::tempdir().unwrap();
    let terminal = Bash::new(Root::new(dir.path()));
    let pipes = Bash::new(Root::new(dir.path())).with_terminal(false);
    for command in [
        "echo hello",
        "printf 'no newline'",
        "true",
        "printf 'a\\n\\n\\nb\\n\\n'",
        "echo out; echo err >&2; exit 4",
        "seq 1 3000",
        "seq 1 100000 | tr -d '\\n'",
        "printf '%0300d\\n' 7",
    ] {
        let args = json!({"command": command});
        let a = call(&pipes, args.clone(), CancellationToken::new()).await;
        let b = call(&terminal, args, CancellationToken::new()).await;
        assert_eq!(comparable(&b.text()), comparable(&a.text()), "{command}");
        assert_eq!(b.result.is_ok(), a.result.is_ok(), "{command}");
        assert_eq!(spill(&b.text()), spill(&a.text()), "{command}");
    }
}

/// A progress bar redrawn with `\r` shows the model its last frame
/// only.
#[tokio::test(flavor = "multi_thread")]
async fn a_progress_bar_shows_its_last_frame() {
    let called = run("printf '10%%\\r50%%\\r100%%\\n'; echo done").await;
    assert_eq!(called.text(), "100%\ndone\n");
}

/// Colors reach the byte stream and not the model.
#[tokio::test(flavor = "multi_thread")]
async fn colors_reach_the_bytes_and_not_the_model() {
    let called = run("[ -t 1 ] && printf '\\033[32mok\\033[0m\\n'").await;
    assert_eq!(called.text(), "ok\n");
    assert_eq!(called.streamed(), b"\x1b[32mok\x1b[0m\r\n");
}

// -- The byte stream and the details -----------------------------------

/// Every byte the command wrote reaches the updates, once and in order,
/// before the tool returns; the result's details hold the same bytes,
/// the size and the status.
#[tokio::test(flavor = "multi_thread")]
async fn every_byte_is_streamed_and_the_result_rebuilds_the_terminal() {
    let called = run("seq 1 50000").await;
    let expected: Vec<u8> = (1..=50_000)
        .flat_map(|i| format!("{i}\r\n").into_bytes())
        .collect();
    assert_eq!(called.streamed(), expected);

    let details = called.details();
    assert_eq!(details["cols"], json!(120));
    assert_eq!(details["rows"], json!(40));
    assert_eq!(details["status"], json!("exited"));
    assert_eq!(details["exitCode"], json!(0));
    assert_eq!(details["replay"], json!("stream"));
    assert_eq!(details["outputBytes"], json!(expected.len()));
    assert_eq!(decode(&details["bytes"]), expected);
    // Many chunks, coalesced: far fewer than one per line.
    let chunks = details["chunks"].as_u64().unwrap();
    assert!((1..50_000).contains(&chunks), "{chunks}");
}

/// A failed command keeps its details: the terminal of a failure is the
/// one worth seeing.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_command_keeps_its_details() {
    let called = run("echo hi; exit 3").await;
    assert_eq!(called.text(), "hi\n\n\nCommand exited with code 3");
    assert!(matches!(called.result, Err(ToolError::Output(_))));
    let details = called.details();
    assert_eq!(details["status"], json!("exited"));
    assert_eq!(details["exitCode"], json!(3));
    assert_eq!(decode(&details["bytes"]), b"hi\r\n");
    assert_eq!(called.structured()["status"], "exited");
    assert_eq!(called.structured()["exit_code"], 3);
    assert_eq!(called.structured()["output"], "hi\n");
}

/// A timeout kills the group and says so in the details, with no exit
/// code; the output before it is kept.
#[tokio::test(flavor = "multi_thread")]
async fn a_timeout_is_in_the_details() {
    let dir = tempfile::tempdir().unwrap();
    let bash = Bash::new(Root::new(dir.path()));
    let called = call(
        &bash,
        json!({"command": "echo hi; sleep 5", "timeout": 0.3}),
        CancellationToken::new(),
    )
    .await;
    assert_eq!(called.text(), "hi\n\n\nCommand timed out after 0.3 seconds");
    let details = called.details();
    assert_eq!(details["status"], json!("timedOut"));
    assert_eq!(details["exitCode"], Value::Null);
    assert_eq!(called.structured()["status"], "timed_out");
    assert_eq!(called.structured()["output"], "hi\n");
}

/// A cancel kills the group and says so in the details.
#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_is_in_the_details() {
    let dir = tempfile::tempdir().unwrap();
    let bash = Bash::new(Root::new(dir.path()));
    let cancel = CancellationToken::new();
    let later = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        later.cancel();
    });
    let called =
        call(&bash, json!({"command": "echo hi; sleep 5"}), cancel).await;
    assert_eq!(called.text(), "hi\n\n\nCommand aborted");
    assert_eq!(called.details()["status"], json!("cancelled"));
    assert_eq!(called.structured()["status"], "cancelled");
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_terminal_shell_reports_spawn_failure() {
    let dir = tempfile::tempdir().unwrap();
    let shell = dir.path().join("missing-shell");
    let bash = Bash::new(Root::new(dir.path())).with_shell(&shell);
    let called =
        call(&bash, json!({"command": "true"}), CancellationToken::new()).await;
    assert!(
        called
            .text()
            .starts_with(&format!("failed to start {}: ", shell.display()))
    );
    assert_eq!(called.structured()["status"], "spawn_failed");
    assert_eq!(called.structured()["error"], called.text());
}

/// Updates carry the text so far as their content, so a caller that
/// shows only text still shows progress.
#[tokio::test(flavor = "multi_thread")]
async fn updates_carry_the_text_so_far() {
    let called = run("echo one; sleep 0.3; echo two").await;
    let texts: Vec<&str> = called.updates.iter().map(text_of).collect();
    assert!(texts.contains(&"one\n"), "{texts:?}");
    assert_eq!(called.text(), "one\ntwo\n");
}

/// A command's stdin is empty, so a prompt fails at once instead of
/// waiting.
#[tokio::test(flavor = "multi_thread")]
async fn a_prompt_fails_at_once() {
    let started = std::time::Instant::now();
    let called = run("read -r answer || echo no-input").await;
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert_eq!(called.text(), "no-input\n");
}

/// `with_terminal(false)` runs with pipes: no terminal in the details,
/// and no details on the updates, as without the feature. The result's
/// details still say what became of the output's artifact, as every
/// `bash` result's do.
#[tokio::test(flavor = "multi_thread")]
async fn pipes_mode_has_no_terminal_details() {
    let dir = tempfile::tempdir().unwrap();
    let bash = Bash::new(Root::new(dir.path())).with_terminal(false);
    let called = call(
        &bash,
        json!({"command": "echo x"}),
        CancellationToken::new(),
    )
    .await;
    let output = called.result.unwrap();
    assert_eq!(
        output.details,
        Some(json!({
            "artifact": null,
            "artifact_error": "artifact storage is unavailable",
            "source_complete": true,
        }))
    );
    assert!(called.updates.iter().all(|update| update.details.is_none()));
}

// -- Persistence ---------------------------------------------------------

/// A run's stored `bash` results keep their terminal details, failed or
/// not, so a reopened run can rebuild each terminal from the store
/// alone.
#[test]
fn stored_results_keep_the_terminal() {
    use tau_agent::agent::Agent;
    use tau_ai::message::Message;
    use tau_store::{Entry, Store};
    use tau_testing::scripted::ScriptedModel;
    use tau_tools::plugin::CodingTools;

    let dir = tempfile::tempdir().unwrap();
    let llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call(
                "bash",
                json!({"command": "printf '\\033[1mbold\\033[0m\\n'"}),
            )
            .tool_call("bash", json!({"command": "echo no; exit 2"}))
        })
        .turn(|t| t.text("done"));
    let entries = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let store = Store::memory().await.unwrap();
            let outcome = Agent::new(llm)
                .plugin(CodingTools::new(Root::new(dir.path())))
                .run("go", &store)
                .await
                .unwrap();
            store.transcript(&outcome.run.0).await.unwrap()
        });
    let results: Vec<_> = entries
        .into_iter()
        .filter_map(|entry| match entry {
            Entry::Message { body, .. } => {
                match serde_json::from_str::<Message>(&body).unwrap() {
                    Message::ToolResult(result) => Some(result),
                    _ => None,
                }
            }
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 2);

    let bold = &results[0].details.as_ref().unwrap()["term"];
    assert!(!results[0].is_error);
    assert_eq!(bold["exitCode"], json!(0));
    assert_eq!(decode(&bold["bytes"]), b"\x1b[1mbold\x1b[0m\r\n");

    let failed = &results[1].details.as_ref().unwrap()["term"];
    assert!(results[1].is_error);
    assert_eq!(failed["exitCode"], json!(2));
    assert_eq!(decode(&failed["bytes"]), b"no\r\n");
}
