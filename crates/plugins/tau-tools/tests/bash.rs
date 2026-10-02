//! `bash` (`tau_tools::bash`), against the rules in
//! `docs/reference/tools.md` ("bash", "Error strings") and pi's
//! `bash.ts` / `output-accumulator.ts`.
//!
//! Pure logic (the accumulator, the throttle, shell selection, the idle
//! window) is driven directly; real process tests run on a normal,
//! unpaused runtime (`docs/reference/testing.md`, "Async code": paused
//! time and real I/O must never mix).

#![cfg(unix)]

use std::{
    path::Path,
    time::{Duration, Instant},
};

use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_agent::{
    error::ToolError,
    tool::{AgentTool, RunId, ToolCtx, ToolOutput, ToolUpdates},
};
use tau_ai::message::InputBlock;
use tau_tools::{
    bash::{
        Accumulator,
        Bash,
        ChunkEvent,
        ProgressThrottle,
        choose_shell,
        drain_until_idle,
    },
    path::Root,
    truncate,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

fn text_of(output: &ToolOutput) -> &str {
    match &output.content[0] {
        InputBlock::Text(text) => &text.text,
        InputBlock::Image(_) => panic!("expected a text block"),
    }
}

fn structured_error(error: &ToolError) -> &Value {
    match error {
        ToolError::Output(output) => output.structured.as_ref().unwrap(),
        other => panic!("expected output error: {other}"),
    }
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

fn process_alive(pid: i32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

/// Polls until `pid` is gone, or panics after `timeout` (the kernel can
/// take a moment to finish tearing a killed process down after
/// `SIGKILL` is sent, even once our own call has returned).
async fn wait_until_dead(pid: i32, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while process_alive(pid) {
        assert!(
            Instant::now() < deadline,
            "pid {pid} was still alive after {timeout:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

// -- Pure properties: Accumulator ------------------------------------

/// Raw bytes, including invalid UTF-8 and multi-byte sequences, so a
/// chunk boundary can land anywhere, including mid-character; newlines
/// are mixed in often enough that the line limit is hit as well as the
/// byte limit.
#[hegel::composite]
fn raw_bytes(tc: &TestCase) -> Vec<u8> {
    tc.draw(
        gs::vecs(hegel::one_of!(gs::just(b'\n'), gs::integers::<u8>()))
            .max_size(400),
    )
}

/// Small limits, so both `MAX_LINES`- and `MAX_BYTES`-style cuts happen
/// often in a short generated input.
fn small_limits(tc: &TestCase) -> (usize, usize) {
    (
        tc.draw(gs::integers::<usize>().min_value(1).max_value(12)),
        tc.draw(hegel::one_of!(
            gs::integers::<usize>().min_value(1).max_value(40),
            gs::integers::<usize>().min_value(1).max_value(200),
        )),
    )
}

/// Splits `bytes` at an arbitrary set of points, drawn independently of
/// any other split of the same bytes.
fn chunk_at(bytes: &[u8], tc: &TestCase) -> Vec<Vec<u8>> {
    let mut cuts: Vec<usize> = tc.draw(
        gs::vecs(gs::integers::<usize>().min_value(0).max_value(bytes.len()))
            .max_size(20),
    );
    cuts.push(0);
    cuts.push(bytes.len());
    cuts.sort_unstable();
    cuts.dedup();
    cuts.windows(2)
        .map(|w| bytes[w[0]..w[1]].to_vec())
        .collect()
}

/// `bash` output re-chunked at any byte boundary, including inside a
/// multi-byte character, gives the same result
/// (`docs/reference/testing.md`, "tau-tools").
#[hegel::test(test_cases = 50)]
fn output_rechunked_at_any_byte_boundary_gives_the_same_result(tc: TestCase) {
    let bytes = tc.draw(raw_bytes());
    let chunks_a = chunk_at(&bytes, &tc);
    let chunks_b = chunk_at(&bytes, &tc);
    let (max_lines, max_bytes) = small_limits(&tc);

    let dir_a = tempfile::tempdir().unwrap();
    let dir_b = tempfile::tempdir().unwrap();
    let mut acc_a = Accumulator::new(max_lines, max_bytes, dir_a.path());
    let mut acc_b = Accumulator::new(max_lines, max_bytes, dir_b.path());
    for chunk in &chunks_a {
        acc_a.append(chunk);
    }
    for chunk in &chunks_b {
        acc_b.append(chunk);
    }
    acc_a.finish();
    acc_b.finish();

    // Compared on what the tool actually shows: the text, whether it
    // was cut, and the true totals (summed over decoded text
    // regardless of chunking, so always chunk-invariant). `by`,
    // `output_lines` and `last_line_partial` describe the *rolling
    // window*, not the full output, and the window's own char-boundary
    // rounding can legitimately differ by a UTF-8 character's worth of
    // bytes depending on exactly when a chunk crossed the rolling-trim
    // threshold -- with `MAX_BYTES` (50 KiB) that is noise, but a tiny
    // `max_bytes` here (as small as 1) can make it visible.
    let snap_a = acc_a.snapshot();
    let snap_b = acc_b.snapshot();
    assert_eq!(snap_a.content, snap_b.content);
    assert_eq!(snap_a.truncated(), snap_b.truncated());
    assert_eq!(snap_a.total_lines, snap_b.total_lines);
    assert_eq!(snap_a.total_bytes, snap_b.total_bytes);
    match (acc_a.spill_path(), acc_b.spill_path()) {
        (Some(a), Some(b)) => {
            assert_eq!(std::fs::read(a).unwrap(), std::fs::read(b).unwrap());
        }
        (None, None) => {}
        other => {
            panic!("one chunking spilled and the other did not: {other:?}")
        }
    }
}

/// `bash` keeps `truncate_tail` of the full output, and the spill file
/// holds the full output, whether the limit hit was lines or bytes
/// (`docs/reference/testing.md`, "tau-tools"): the content, the totals,
/// the "was it cut" flag and the spill all agree with the full output,
/// whether or not the rolling tail was trimmed.
#[hegel::test(test_cases = 50)]
fn keeps_truncate_tail_of_the_full_output_and_spills_it_whole(tc: TestCase) {
    let bytes = tc.draw(raw_bytes());
    let chunks = chunk_at(&bytes, &tc);
    let (max_lines, max_bytes) = small_limits(&tc);

    let dir = tempfile::tempdir().unwrap();
    let mut acc = Accumulator::new(max_lines, max_bytes, dir.path());
    for chunk in &chunks {
        acc.append(chunk);
    }
    acc.finish();
    let snapshot = acc.snapshot();

    let full_text = String::from_utf8_lossy(&bytes).into_owned();
    let reference = truncate::truncate_tail(&full_text, max_lines, max_bytes);

    match reference.by {
        Some(truncate::Limit::Lines) => tc.event("cut by lines"),
        Some(truncate::Limit::Bytes) => tc.event("cut by bytes"),
        None => tc.event("not cut"),
    }
    // The accumulator trims its tail to twice the byte limit once the
    // tail passes four times it.
    let rolling = (2 * max_bytes).max(1);
    if full_text.len() > 2 * rolling {
        tc.event("rolling tail trimmed");
    }
    assert_eq!(snapshot.content, reference.content);
    assert_eq!(snapshot.truncated(), reference.truncated());
    assert_eq!(snapshot.total_lines, reference.total_lines);
    assert_eq!(snapshot.total_bytes, reference.total_bytes);

    if reference.truncated() {
        let path = acc
            .spill_path()
            .expect("truncated output is always spilled");
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    } else {
        assert!(acc.spill_path().is_none());
    }
}

/// Property inventory: a generated TSV log remains byte-for-byte readable
/// after arbitrary chunking, even when a UTF-8 character crosses a chunk;
/// each row also agrees with an independently parsed TSV oracle.
/// Generator: rows use a fixed alphabet without tabs/newlines, so every
/// generated case is valid TSV. Cuts shrink toward fewer and smaller chunks.
/// The workspace hegel.toml controls case counts and CI's deterministic
/// profile; no per-test override is needed.
#[hegel::test]
fn full_capture_preserves_chunked_tsv_bytes_and_rows(tc: TestCase) {
    let fields: Vec<Vec<char>> = tc.draw(
        gs::vecs(
            gs::vecs(gs::sampled_from(vec!['a', 'é', '日', '🙂'])).max_size(80),
        )
        .min_size(1)
        .max_size(80),
    );
    let mut expected_rows = Vec::new();
    let mut expected = Vec::new();
    for (index, field) in fields.iter().enumerate() {
        let field: String = field.iter().collect();
        expected_rows.push((index, field.clone()));
        expected.extend(format!("{index}\t{field}\n").as_bytes());
    }
    let chunks = chunk_at(&expected, &tc);
    let dir = tempfile::tempdir().unwrap();
    let mut acc = Accumulator::new(3, 24, dir.path()).with_full_capture();
    for chunk in chunks {
        acc.append(&chunk);
    }
    acc.finish();
    let spill = std::fs::read(acc.spill_path().unwrap()).unwrap();
    assert_eq!(spill, expected);
    let parsed_rows: Vec<(usize, String)> = std::str::from_utf8(&spill)
        .unwrap()
        .lines()
        .map(|line| {
            let (index, field) = line.split_once('\t').unwrap();
            (index.parse().unwrap(), field.to_owned())
        })
        .collect();
    assert_eq!(parsed_rows, expected_rows);
}

/// What `bash` shows is `truncate_tail` of the full output even once the
/// rolling tail has been trimmed. A last line that starts before the
/// rolling tail is not dropped: its end shows after the cut marker, all
/// within the byte limit, so `cat` of a one-line minified file shows the
/// file's end.
#[hegel::test(test_cases = 200)]
#[hegel::explicit_test_case(
    bytes = [vec![b'x'; 250], vec![b'\n']].concat(),
    max_lines = 10usize,
    max_bytes = 40usize,
)]
#[hegel::explicit_test_case(
    bytes = vec![b'\n', b'\n', 0, b'\n', b'\n'],
    max_lines = 1usize,
    max_bytes = 1usize,
)]
fn the_rolling_tail_shows_what_truncating_the_full_output_shows(tc: TestCase) {
    let bytes = tc.draw(raw_bytes());
    let max_lines = tc.draw(gs::integers::<usize>().min_value(1).max_value(12));
    let max_bytes = tc.draw(gs::integers::<usize>().min_value(1).max_value(40));

    let dir = tempfile::tempdir().unwrap();
    let mut acc = Accumulator::new(max_lines, max_bytes, dir.path());
    acc.append(&bytes);
    acc.finish();

    let full_text = String::from_utf8_lossy(&bytes).into_owned();
    let reference = truncate::truncate_tail(&full_text, max_lines, max_bytes);
    let snapshot = acc.snapshot();
    assert_eq!(snapshot.content, reference.content);
    assert_eq!(snapshot.last_line_partial, reference.last_line_partial);
    assert!(snapshot.content.len() <= max_bytes);
    if snapshot.last_line_partial && max_bytes >= truncate::CUT.len() {
        assert!(snapshot.content.starts_with(truncate::CUT));
    }
}

/// `bash` progress updates stay under a fixed bound however many chunks
/// arrive (`docs/reference/testing.md`, "tau-tools"), and none is held
/// back needlessly: the first call is allowed, and a call is allowed
/// exactly when a full window has passed since the last allowed one.
#[hegel::test(test_cases = 500)]
fn progress_updates_stay_bounded_however_many_chunks_arrive(tc: TestCase) {
    let window = Duration::from_millis(100);
    let mut throttle = ProgressThrottle::new(window);
    // Steps of exactly one window come up often, to hit the boundary.
    let deltas: Vec<u64> = tc.draw(
        gs::vecs(hegel::one_of!(
            gs::just(100u64),
            gs::integers::<u64>().min_value(0).max_value(150),
        ))
        .max_size(200),
    );

    let start = Instant::now();
    let mut now = start;
    let mut allowed = 0usize;
    let mut last_allowed: Option<Instant> = None;
    for delta in &deltas {
        now += Duration::from_millis(*delta);
        let expected =
            last_allowed.is_none_or(|last| now.duration_since(last) >= window);
        if last_allowed.is_some_and(|last| now.duration_since(last) == window) {
            tc.event("exactly one window later");
        }
        assert_eq!(throttle.allow(now), expected, "at {:?}", now - start);
        if expected {
            allowed += 1;
            last_allowed = Some(now);
        }
    }
    let span = now.duration_since(start);
    let bound = (span.as_millis() / window.as_millis()) as usize + 1;
    assert!(
        allowed <= bound,
        "allowed {allowed} updates, bound was {bound}"
    );
}

// -- Shell selection ---------------------------------------------------

/// Shell selection: a configured path first, then `/bin/bash`, then
/// `bash` on `PATH`, then `sh` (`docs/reference/tools.md`, "bash",
/// "Shell").
#[test]
fn shell_selection_follows_the_documented_order() {
    let configured = Path::new("/opt/custom/bash");
    let on_path = Path::new("/usr/bin/bash");
    assert_eq!(
        choose_shell(Some(configured), true, Some(on_path)),
        configured
    );
    assert_eq!(
        choose_shell(None, true, Some(on_path)),
        Path::new("/bin/bash")
    );
    assert_eq!(choose_shell(None, false, Some(on_path)), on_path);
    assert_eq!(choose_shell(None, false, None), Path::new("sh"));
}

// -- The idle window (pure, driven by a scripted source) ---------------

/// A chunk arriving within the idle window restarts it, so output that
/// a fixed deadline measured from exit would miss is still captured
/// (`docs/reference/tools.md`, "bash", "Late output"; pi #5303).
#[test]
fn a_chunk_within_the_idle_window_restarts_it() {
    tau_testing::block_on(async {
        let (tx, rx) = mpsc::unbounded_channel::<ChunkEvent>();
        let exited = async {};
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(90)).await;
            let _ = tx.send(ChunkEvent::Data(b"early".to_vec()));
            // 150ms after exit: past a *fixed* 100ms-since-exit deadline,
            // but within 100ms of the chunk above.
            tokio::time::sleep(Duration::from_millis(60)).await;
            let _ = tx.send(ChunkEvent::Data(b"late".to_vec()));
            // Keep the sender alive past the idle window that should
            // follow the last chunk, so the timeout path (not EOF)
            // finalizes this run.
            tokio::time::sleep(Duration::from_millis(200)).await;
        });

        let mut collected = Vec::new();
        drain_until_idle(rx, exited, Duration::from_millis(100), |chunk| {
            collected.push(chunk.to_vec());
        })
        .await;

        assert_eq!(collected, vec![b"early".to_vec(), b"late".to_vec()]);
    });
}

/// A chunk that arrives at the exact instant the idle window elapses is
/// still captured, not lost to the race between the timer and the data
/// (`docs/reference/tools.md`, "bash", "Late output"; pi #5303).
#[test]
fn a_chunk_at_the_idle_boundary_is_not_dropped() {
    tau_testing::block_on(async {
        let (tx, rx) = mpsc::unbounded_channel::<ChunkEvent>();
        let exited = async {};
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let _ = tx.send(ChunkEvent::Data(b"boundary".to_vec()));
        });

        let mut collected = Vec::new();
        drain_until_idle(rx, exited, Duration::from_millis(100), |chunk| {
            collected.push(chunk.to_vec());
        })
        .await;

        assert_eq!(collected, vec![b"boundary".to_vec()]);
    });
}

/// Output that arrives after the tool has already finalized is safely
/// discarded: the sender sees a closed channel, never a panic or a hang
/// (`docs/reference/tools.md`, "bash", "Late output"; pi #5208).
#[test]
fn output_after_finalize_is_dropped_without_panicking() {
    tau_testing::block_on(async {
        let (tx, rx) = mpsc::unbounded_channel::<ChunkEvent>();
        let exited = async {};
        drain_until_idle(
            rx,
            exited,
            Duration::from_millis(100),
            |_chunk: &[u8]| {
                panic!("no data should have arrived before finalizing");
            },
        )
        .await;

        assert!(tx.send(ChunkEvent::Data(b"late".to_vec())).is_err());
    });
}

// -- Tool metadata -------------------------------------------------------

/// The tool's name and description are pinned verbatim from pi's
/// `createShellToolDefinition` (`docs/reference/tools.md`, "bash").
#[test]
fn name_and_description_are_pinned() {
    let dir = tempfile::tempdir().unwrap();
    let bash = Bash::new(Root::new(dir.path()));
    assert_eq!(bash.name(), "bash");
    assert_eq!(
        bash.description(),
        "Execute a bash command in the current working directory. \
Returns stdout and stderr. Output is truncated to last 2000 lines or \
50KB (whichever is hit first). If truncated, full output is saved to a \
temp file. Optionally provide a timeout in seconds."
    );
}

/// The parameter schema has `command` (required) and `timeout`
/// (optional), with pi's parameter descriptions verbatim
/// (`docs/reference/tools.md`, "bash": `{ command, timeout? }`).
#[test]
fn parameters_schema_matches_the_documented_shape() {
    let dir = tempfile::tempdir().unwrap();
    let bash = Bash::new(Root::new(dir.path()));
    let schema = bash.parameters();
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"]["command"]["type"], "string");
    assert_eq!(
        schema["properties"]["command"]["description"],
        "Shell command to execute"
    );
    assert_eq!(
        schema["properties"]["timeout"]["description"],
        "Timeout in seconds (optional, no default timeout)"
    );
    assert_eq!(schema["required"], json!(["command"]));
    let result_schema = bash.output_schema().unwrap();
    assert_eq!(result_schema["type"], "object");
    assert_eq!(result_schema["additionalProperties"], false);
    assert_eq!(result_schema["required"].as_array().unwrap().len(), 17);
}

// -- Real processes (normal, unpaused runtime) --------------------------

/// stdout and stderr merge into one result.
#[tokio::test]
async fn success_returns_stdout_and_stderr_merged() {
    let dir = tempfile::tempdir().unwrap();
    let bash = Bash::new(Root::new(dir.path()));
    let (ctx, _cancel, _rx) = ctx_with_updates();
    let output = bash
        .call(json!({"command": "echo out; echo err 1>&2"}), ctx)
        .await
        .unwrap();
    let text = text_of(&output);
    assert!(text.contains("out"), "{text}");
    assert!(text.contains("err"), "{text}");
    let structured = output.structured.as_ref().unwrap();
    assert_eq!(structured["status"], "exited");
    assert_eq!(structured["exit_code"], 0);
    assert_eq!(structured["output"], text);
    assert_eq!(structured["error"], Value::Null);
}

/// A command with no output returns `(no output)`.
#[tokio::test]
async fn no_output_reports_a_placeholder() {
    let dir = tempfile::tempdir().unwrap();
    let bash = Bash::new(Root::new(dir.path()));
    let (ctx, _cancel, _rx) = ctx_with_updates();
    let output = bash.call(json!({"command": "true"}), ctx).await.unwrap();
    assert_eq!(text_of(&output), "(no output)");
    assert_eq!(output.structured.as_ref().unwrap()["output"], "");
}

/// The command runs in the tool's root directory
/// (`docs/reference/tools.md`, "bash").
#[tokio::test]
async fn runs_in_the_tools_root_directory() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("marker"), b"x").unwrap();
    let bash = Bash::new(Root::new(dir.path()));
    let (ctx, _cancel, _rx) = ctx_with_updates();
    let output = bash.call(json!({"command": "ls"}), ctx).await.unwrap();
    assert!(text_of(&output).contains("marker"));
}

/// `with_shell` overrides the default search order
/// (`docs/reference/tools.md`, "bash", "Shell").
#[tokio::test]
async fn with_shell_overrides_the_default_search_order() {
    let dir = tempfile::tempdir().unwrap();
    // A fake "shell" that proves it (and not the default search order)
    // ran, then hands the command to a real shell so the tool still
    // gets a normal result.
    let fake_shell = dir.path().join("fake-shell");
    std::fs::write(
        &fake_shell,
        "#!/bin/sh\necho FAKE_SHELL_USED\nexec /bin/sh -c \"$2\"\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            &fake_shell,
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }

    let bash = Bash::new(Root::new(dir.path())).with_shell(&fake_shell);
    let (ctx, _cancel, _rx) = ctx_with_updates();
    let output = bash
        .call(json!({"command": "echo real-command-ran"}), ctx)
        .await
        .unwrap();
    let text = text_of(&output);
    assert!(text.contains("FAKE_SHELL_USED"), "{text}");
    assert!(text.contains("real-command-ran"), "{text}");
}

/// Error string (`docs/reference/tools.md#error-strings`): a non-zero
/// exit, with no output.
#[tokio::test]
async fn error_string_nonzero_exit_no_output() {
    let dir = tempfile::tempdir().unwrap();
    let bash = Bash::new(Root::new(dir.path()));
    let (ctx, _cancel, _rx) = ctx_with_updates();
    let error = bash
        .call(json!({"command": "exit 3"}), ctx)
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "(no output)\n\nCommand exited with code 3"
    );
    let structured = structured_error(&error);
    assert_eq!(structured["status"], "exited");
    assert_eq!(structured["exit_code"], 3);
    assert_eq!(structured["output"], "");
}

/// Error string: a non-zero exit keeps the output printed before it.
#[tokio::test]
async fn error_string_nonzero_exit_with_output() {
    let dir = tempfile::tempdir().unwrap();
    let bash = Bash::new(Root::new(dir.path()));
    let (ctx, _cancel, _rx) = ctx_with_updates();
    let error = bash
        .call(json!({"command": "echo hi; exit 3"}), ctx)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "hi\n\n\nCommand exited with code 3");
}

/// Error string: a timeout, keeping the output printed before it.
#[tokio::test]
async fn error_string_timeout_with_output() {
    let dir = tempfile::tempdir().unwrap();
    let bash = Bash::new(Root::new(dir.path()));
    let (ctx, _cancel, _rx) = ctx_with_updates();
    let error = bash
        .call(json!({"command": "echo hi; sleep 5", "timeout": 0.2}), ctx)
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "hi\n\n\nCommand timed out after 0.2 seconds"
    );
    let structured = structured_error(&error);
    assert_eq!(structured["status"], "timed_out");
    assert_eq!(structured["exit_code"], Value::Null);
    assert_eq!(structured["output"], "hi\n");
}

/// Error string: cancellation, keeping the output printed before it.
#[tokio::test]
async fn error_string_cancelled_with_output() {
    let dir = tempfile::tempdir().unwrap();
    let bash = Bash::new(Root::new(dir.path()));
    let (ctx, cancel, _rx) = ctx_with_updates();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        cancel.cancel();
    });
    let error = bash
        .call(json!({"command": "echo hi; sleep 5"}), ctx)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "hi\n\n\nCommand aborted");
}

/// Error string: a run cancelled before it starts never spawns a
/// process.
#[tokio::test]
async fn error_string_already_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("ran");
    let bash = Bash::new(Root::new(dir.path()));
    let (ctx, cancel, _rx) = ctx_with_updates();
    cancel.cancel();
    let error = bash
        .call(
            json!({"command": format!("touch {}", marker.display())}),
            ctx,
        )
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "Command aborted");
    let structured = structured_error(&error);
    assert_eq!(structured["status"], "cancelled");
    assert_eq!(structured["output"], "");
    assert!(!marker.exists(), "the command ran despite being cancelled");
}

#[tokio::test]
async fn missing_shell_has_spawn_failed_result() {
    let dir = tempfile::tempdir().unwrap();
    let shell = dir.path().join("missing-shell");
    let bash = Bash::new(Root::new(dir.path())).with_shell(&shell);
    let (ctx, _cancel, _rx) = ctx_with_updates();
    let error = bash
        .call(json!({"command": "true"}), ctx)
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with(&format!("failed to start {}: ", shell.display()))
    );
    let structured = structured_error(&error);
    assert_eq!(structured["status"], "spawn_failed");
    assert_eq!(structured["exit_code"], Value::Null);
    assert_eq!(structured["output"], "");
    assert_eq!(structured["error"], error.to_string());
}

/// Error string: an invalid `timeout` argument, for every kind of
/// non-finite or non-positive value.
#[tokio::test]
async fn error_string_invalid_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let bash = Bash::new(Root::new(dir.path()));
    // `serde_json::Value` cannot hold NaN or infinity at all (its
    // `Number` type refuses them, and the JSON text form `1e400` fails
    // to parse rather than saturating), so zero and negative are the
    // only non-finite-or-not-positive values that can actually reach
    // `Bash::call`.
    let cases = [
        json!({"command": "true", "timeout": 0.0}),
        json!({"command": "true", "timeout": -1.0}),
    ];
    for args in cases {
        let (ctx, _cancel, _rx) = ctx_with_updates();
        let error = bash.call(args.clone(), ctx).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "Invalid timeout: must be a finite number of seconds",
            "args = {args}"
        );
    }
}

/// `kill -KILL $$` reports exit code 137 and keeps the output printed
/// before the kill (`docs/reference/testing.md`, "tau-tools").
#[tokio::test]
async fn sigkill_reports_code_137_and_keeps_earlier_output() {
    let dir = tempfile::tempdir().unwrap();
    let bash = Bash::new(Root::new(dir.path()));
    let (ctx, _cancel, _rx) = ctx_with_updates();
    let error = bash
        .call(json!({"command": "echo before; kill -KILL $$"}), ctx)
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "before\n\n\nCommand exited with code 137"
    );
}

/// `kill -TERM $$` reports exit code 143 and keeps the output printed
/// before the kill (`docs/reference/testing.md`, "tau-tools").
#[tokio::test]
async fn sigterm_reports_code_143_and_keeps_earlier_output() {
    let dir = tempfile::tempdir().unwrap();
    let bash = Bash::new(Root::new(dir.path()));
    let (ctx, _cancel, _rx) = ctx_with_updates();
    let error = bash
        .call(json!({"command": "echo before; kill -TERM $$"}), ctx)
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "before\n\n\nCommand exited with code 143"
    );
}

/// A grandchild process is killed with its group on cancel
/// (`docs/reference/testing.md`, "tau-tools").
#[tokio::test]
async fn cancel_kills_a_grandchild_with_its_group() {
    let dir = tempfile::tempdir().unwrap();
    let pidfile = dir.path().join("pid");
    let bash = Bash::new(Root::new(dir.path()));
    let (ctx, cancel, _rx) = ctx_with_updates();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        cancel.cancel();
    });
    // The command sleeps far longer than the call should ever take, so
    // if the group were not killed, `call` would only return once that
    // sleep finished on its own (`drain_until_idle` waits for the real
    // exit) -- checking `process_alive` afterwards would then pass for
    // the wrong reason. Bounding the elapsed time catches that.
    let command = format!("sleep 20 & echo $! > {}; wait", pidfile.display());
    let started = Instant::now();
    let result = bash.call(json!({"command": command}), ctx).await;
    let elapsed = started.elapsed();
    assert!(result.is_err());
    assert!(
        elapsed < Duration::from_secs(5),
        "call took {elapsed:?}; the group was not killed promptly"
    );

    let pid: i32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    wait_until_dead(pid, Duration::from_secs(2)).await;
}

/// A grandchild process is killed with its group on timeout
/// (`docs/reference/testing.md`, "tau-tools").
#[tokio::test]
async fn timeout_kills_a_grandchild_with_its_group() {
    let dir = tempfile::tempdir().unwrap();
    let pidfile = dir.path().join("pid");
    let bash = Bash::new(Root::new(dir.path()));
    let (ctx, _cancel, _rx) = ctx_with_updates();
    let command = format!("sleep 20 & echo $! > {}; wait", pidfile.display());
    let started = Instant::now();
    let result = bash
        .call(json!({"command": command, "timeout": 0.3}), ctx)
        .await;
    let elapsed = started.elapsed();
    assert!(result.is_err());
    assert!(
        elapsed < Duration::from_secs(5),
        "call took {elapsed:?}; the group was not killed promptly"
    );

    let pid: i32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    wait_until_dead(pid, Duration::from_secs(2)).await;
}

/// Output over `MAX_LINES` is truncated to its tail, and the full
/// output is spilled to the temp file the notice points at
/// (`docs/reference/tools.md`, "bash", "Output").
#[tokio::test]
async fn truncated_real_output_is_spilled_in_full() {
    let dir = tempfile::tempdir().unwrap();
    let bash = Bash::new(Root::new(dir.path()));
    let (ctx, _cancel, _rx) = ctx_with_updates();
    let output = bash
        .call(json!({"command": "seq 1 3000"}), ctx)
        .await
        .unwrap();
    let text = text_of(&output);
    assert!(text.contains("Full output: "), "{text}");
    let path = text.rsplit("Full output: ").next().unwrap().trim();
    let full = std::fs::read_to_string(path).unwrap();
    assert_eq!(full.lines().count(), 3000);
    assert!(full.starts_with("1\n"));
    assert!(full.trim_end().ends_with("3000"));
}

/// Progress is reported as `ToolUpdate`s while the command runs.
#[tokio::test]
async fn progress_updates_are_sent_while_running() {
    let dir = tempfile::tempdir().unwrap();
    let bash = Bash::new(Root::new(dir.path()));
    let (ctx, _cancel, mut rx) = ctx_with_updates();
    bash.call(json!({"command": "echo one; sleep 0.3; echo two"}), ctx)
        .await
        .unwrap();
    assert!(
        rx.try_recv().is_ok(),
        "expected at least one progress update"
    );
}
