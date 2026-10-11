//! `exec_command` and `write_stdin` (`tau_tools_host::exec`), against
//! `docs/reference/tools.md` ("exec_command and write_stdin"). Real
//! processes, on a normal, unpaused runtime
//! (`docs/reference/testing.md`, "Async code").

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]
#![cfg(unix)]

use std::{
    path::Path,
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use tau_agent::{
    error::ToolError,
    tool::{AgentTool, RunId, ToolCtx, ToolOutput, ToolUpdates},
};
use tau_ai::message::InputBlock;
use tau_tools_host::{
    exec::{ExecCommand, Processes, WriteStdin},
    path::Root,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

struct Tools {
    exec: ExecCommand,
    write: WriteStdin,
    processes: Processes,
    dir: tempfile::TempDir,
}

fn tools() -> Tools {
    let dir = tempfile::tempdir().unwrap();
    let processes = Processes::new();
    Tools {
        exec: ExecCommand::new(Root::new(dir.path()), processes.clone()),
        write: WriteStdin::new(processes.clone()),
        processes,
        dir,
    }
}

fn ctx() -> ToolCtx {
    let (updates, _) = ToolUpdates::channel("call_1");
    ToolCtx::new(CancellationToken::new(), updates, RunId("run_1".into()))
}

fn ctx_with_updates() -> (
    ToolCtx,
    CancellationToken,
    mpsc::UnboundedReceiver<(std::sync::Arc<str>, ToolOutput)>,
) {
    let (updates, receiver) = ToolUpdates::channel("call_1");
    let cancel = CancellationToken::new();
    let ctx = ToolCtx::new(cancel.clone(), updates, RunId("run_1".into()));
    (ctx, cancel, receiver)
}

fn text_of(output: &ToolOutput) -> String {
    match &output.content[0] {
        InputBlock::Text(text) => text.text.clone(),
        InputBlock::Image(_) => panic!("expected a text block"),
    }
}

fn failed(error: &ToolError) -> String {
    match error {
        ToolError::Output(output) => text_of(output),
        other => other.to_string(),
    }
}

/// The process id a running answer names.
fn process_of(output: &ToolOutput) -> i32 {
    output.structured.as_ref().unwrap()["process_id"]
        .as_i64()
        .expect("still running") as i32
}

fn alive(pid: i32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

async fn wait_until_dead(pid: i32) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while alive(pid) {
        assert!(Instant::now() < deadline, "pid {pid} is still alive");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_command_that_ends_answers_with_its_output() {
    let t = tools();
    let output = t
        .exec
        .call(json!({ "cmd": "echo hello; echo there >&2" }), ctx())
        .await
        .unwrap();
    assert_eq!(text_of(&output).replace("\r\n", "\n"), "hello\nthere");
    let structured = output.structured.unwrap();
    assert_eq!(structured["running"], false);
    assert_eq!(structured["exit_code"], 0);
    assert_eq!(structured["process_id"], Value::Null);
    assert!(t.processes.is_empty(), "an ended process does not stay");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failing_command_is_an_error_with_its_output_and_code() {
    let t = tools();
    let error = t
        .exec
        .call(json!({ "cmd": "echo oops; exit 7" }), ctx())
        .await
        .unwrap_err();
    let text = failed(&error);
    assert!(text.starts_with("oops"), "{text}");
    assert!(text.ends_with("Command exited with code 7"), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_command_with_no_output_says_so() {
    let t = tools();
    let output = t.exec.call(json!({ "cmd": "true" }), ctx()).await.unwrap();
    assert_eq!(text_of(&output), "(no output)");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_command_runs_in_the_workdir() {
    let t = tools();
    std::fs::create_dir(t.dir.path().join("sub")).unwrap();
    let output = t
        .exec
        .call(
            json!({ "cmd": "basename \"$PWD\"", "workdir": "sub" }),
            ctx(),
        )
        .await
        .unwrap();
    assert_eq!(text_of(&output).trim(), "sub");
    let error = t
        .exec
        .call(json!({ "cmd": "true", "workdir": "missing" }), ctx())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("not a directory"), "{error}");
}

/// A command still running when the yield time passes answers with what
/// it wrote and a process id; `write_stdin` reads the rest, and the
/// process is gone once its end was reported.
#[tokio::test(flavor = "multi_thread")]
async fn a_command_still_running_goes_on_in_the_background() {
    let t = tools();
    let first = t
        .exec
        .call(
            json!({ "cmd": "echo started; sleep 1; echo finished",
                    "yield_time_ms": 250 }),
            ctx(),
        )
        .await
        .unwrap();
    let text = text_of(&first).replace("\r\n", "\n");
    assert!(text.starts_with("started\n"), "{text}");
    assert!(text.contains("is still running"), "{text}");
    let id = process_of(&first);
    assert_eq!(t.processes.len(), 1);

    let rest = t
        .write
        .call(json!({ "process_id": id }), ctx())
        .await
        .unwrap();
    // What was read is not read again.
    assert_eq!(text_of(&rest).replace("\r\n", "\n"), "finished");
    assert_eq!(rest.structured.unwrap()["running"], false);
    assert!(t.processes.is_empty());

    let error = t
        .write
        .call(json!({ "process_id": id }), ctx())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("No process"), "{error}");
}

/// Everything a process writes arrives once, in order, over the calls
/// that read it.
#[tokio::test(flavor = "multi_thread")]
async fn output_is_not_lost_or_repeated_between_calls() {
    let t = tools();
    let mut all = String::new();
    let mut next = t
        .exec
        .call(
            json!({ "cmd": "for i in 1 2 3 4; do echo line$i; sleep 0.4; done",
                    "yield_time_ms": 250 }),
            ctx(),
        )
        .await
        .unwrap();
    loop {
        let text = text_of(&next).replace("\r\n", "\n");
        let body = text.split("\n\n[process").next().unwrap();
        if body != "(no new output)" && body != "(no output)" {
            all.push_str(body);
            all.push('\n');
        }
        let structured = next.structured.clone().unwrap();
        if structured["running"] == false {
            break;
        }
        next = t
            .write
            .call(
                json!({ "process_id": process_of(&next),
                        "yield_time_ms": 5000 }),
                ctx(),
            )
            .await
            .unwrap();
    }
    assert_eq!(all, "line1\nline2\nline3\nline4\n");
}

/// A command that reads input needs `tty`; with it, `write_stdin` types
/// to it.
#[tokio::test(flavor = "multi_thread")]
async fn input_reaches_a_tty_process() {
    let t = tools();
    let first = t
        .exec
        .call(
            json!({ "cmd": "read -p 'name? ' name; echo hi $name",
                    "tty": true, "yield_time_ms": 500 }),
            ctx(),
        )
        .await
        .unwrap();
    assert!(text_of(&first).starts_with("name?"), "{}", text_of(&first));
    let id = process_of(&first);
    let done = t
        .write
        .call(json!({ "process_id": id, "chars": "tau\n" }), ctx())
        .await
        .unwrap();
    let text = text_of(&done).replace("\r\n", "\n");
    assert!(text.contains("hi tau"), "{text}");
    assert_eq!(done.structured.unwrap()["running"], false);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_process_without_a_tty_takes_no_input() {
    let t = tools();
    let first = t
        .exec
        .call(json!({ "cmd": "sleep 30", "yield_time_ms": 250 }), ctx())
        .await
        .unwrap();
    let error = t
        .write
        .call(
            json!({ "process_id": process_of(&first), "chars": "x\n" }),
            ctx(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("tty: true"), "{error}");
}

#[tokio::test(flavor = "multi_thread")]
async fn control_c_interrupts_a_tty_process() {
    let t = tools();
    let first = t
        .exec
        .call(
            json!({ "cmd": "sleep 30", "tty": true, "yield_time_ms": 250 }),
            ctx(),
        )
        .await
        .unwrap();
    let error = t
        .write
        .call(
            json!({ "process_id": process_of(&first), "chars": "\u{3}" }),
            ctx(),
        )
        .await
        .unwrap_err();
    assert!(
        failed(&error).contains("Command exited with code 130"),
        "{error}"
    );
    assert!(t.processes.is_empty());
}

/// Cancelling a call does not stop the process: what it wrote
/// meanwhile is the next call's.
#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_call_leaves_the_process_running() {
    let t = tools();
    let (ctx, cancel, _updates) = ctx_with_updates();
    let call = tokio::spawn({
        let exec =
            ExecCommand::new(Root::new(t.dir.path()), t.processes.clone());
        async move {
            exec.call(
                json!({ "cmd": "echo before; sleep 1; echo after",
                        "yield_time_ms": 30000 }),
                ctx,
            )
            .await
        }
    });
    tokio::time::sleep(Duration::from_millis(400)).await;
    cancel.cancel();
    let error = call.await.unwrap().unwrap_err();
    assert!(error.to_string().contains("Operation aborted"), "{error}");
    assert!(error.to_string().contains("goes on"), "{error}");
    assert_eq!(t.processes.len(), 1);
    let rest = t
        .write
        .call(json!({ "process_id": 1 }), ctx_with_updates().0)
        .await
        .unwrap();
    let text = text_of(&rest).replace("\r\n", "\n");
    assert_eq!(text, "before\nafter");
}

/// Dropping a run's processes kills what still runs.
#[tokio::test(flavor = "multi_thread")]
async fn dropping_the_processes_kills_them() {
    let t = tools();
    let pidfile = t.dir.path().join("pid");
    let first = t
        .exec
        .call(
            json!({ "cmd": format!("echo $$ > {}; sleep 60", pidfile.display()),
                    "yield_time_ms": 500 }),
            ctx(),
        )
        .await
        .unwrap();
    let pid: i32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(alive(pid));
    assert!(first.structured.unwrap()["running"].as_bool().unwrap());
    let Tools {
        processes,
        exec,
        write,
        ..
    } = t;
    drop((exec, write));
    processes.terminate_all();
    wait_until_dead(pid).await;
}

/// Past the limit, the least recently used goes; one that exited
/// first.
#[tokio::test(flavor = "multi_thread")]
async fn the_store_keeps_a_limited_number_of_processes() {
    use tau_tools_host::exec::processes::MAX_PROCESSES;
    let t = tools();
    let mut calls = Vec::new();
    for _ in 0..(MAX_PROCESSES + 6) {
        let exec =
            ExecCommand::new(Root::new(t.dir.path()), t.processes.clone());
        calls.push(tokio::spawn(async move {
            exec.call(json!({ "cmd": "sleep 30", "yield_time_ms": 250 }), ctx())
                .await
                .unwrap()
        }));
    }
    for call in calls {
        call.await.unwrap();
    }
    assert_eq!(t.processes.len(), MAX_PROCESSES);
    t.processes.terminate_all();
}

/// The raw bytes stream in the updates' details, and the result's hold
/// what the call saw.
#[tokio::test(flavor = "multi_thread")]
async fn the_terminal_streams_in_the_details() {
    use base64::Engine as _;
    let t = tools();
    let (ctx, _cancel, mut updates) = ctx_with_updates();
    let output = t
        .exec
        .call(json!({ "cmd": "printf '\\033[31mred\\033[0m\\n'" }), ctx)
        .await
        .unwrap();
    let mut streamed = Vec::new();
    while let Ok((_, update)) = updates.try_recv() {
        let term = &update.details.as_ref().unwrap()["term"];
        streamed.extend(
            base64::engine::general_purpose::STANDARD
                .decode(term["bytes"].as_str().unwrap())
                .unwrap(),
        );
    }
    let streamed = String::from_utf8(streamed).unwrap();
    assert_eq!(streamed, "\u{1b}[31mred\u{1b}[0m\r\n");
    let term = &output.details.as_ref().unwrap()["term"];
    assert_eq!(term["status"], "exited");
    assert_eq!(term["exitCode"], 0);
    assert_eq!(term["replay"], "stream");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(term["bytes"].as_str().unwrap())
        .unwrap();
    assert_eq!(String::from_utf8(bytes).unwrap(), streamed);
    assert_eq!(text_of(&output), "red");
}

#[tokio::test(flavor = "multi_thread")]
async fn too_much_output_is_cut_to_the_last_lines() {
    let t = tools();
    let output = t
        .exec
        .call(json!({ "cmd": "seq 1 5000" }), ctx())
        .await
        .unwrap();
    let text = text_of(&output).replace("\r\n", "\n");
    assert!(
        text.starts_with("[output cut to the last 2000 of"),
        "{text}"
    );
    assert!(text.ends_with("\n5000"), "{}", &text[text.len() - 20..]);
    assert!(output.structured.unwrap()["truncated"].as_bool().unwrap());
}
