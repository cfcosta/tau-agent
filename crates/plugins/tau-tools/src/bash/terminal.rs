//! `bash` under a pseudo-terminal (the `terminal` feature;
//! `docs/decisions/0010-terminal-rendering.md`,
//! `docs/reference/tools.md`, "bash: terminal mode").
//!
//! [`tau_terminal::Command`] runs the command and does the terminal
//! work; this module maps its events onto the tool's contract:
//!
//! - the model's text is the terminal's plain text, through the same
//!   [`Accumulator`] as pipe mode (pi's limits, spill files of plain
//!   text), so plain output reads exactly as it does with pipes;
//! - the raw bytes go to callers in `details.term` of the progress
//!   updates, in order, coalesced but never dropped, all sent before the
//!   tool returns;
//! - the result's `details.term` holds what rebuilds the terminal: the
//!   raw bytes (or, past a cap, a VT snapshot), the size and the exit
//!   status. It stays on a failed result too.
//!
//! # Details
//!
//! A progress update is `ToolOutput { content: [the text so far],
//! details }`, where `details` is either absent (a text-only update) or
//!
//! ```json
//! {"term": {"seq": 0, "bytes": "<base64>"}}
//! ```
//!
//! `seq` counts the chunks from 0 with no gaps; the chunks, decoded and
//! joined in `seq` order, are every byte the command wrote. The result's
//! details are
//!
//! ```json
//! {"term": {"cols": 120, "rows": 40, "status": "exited",
//!           "exitCode": 0, "chunks": 3, "outputBytes": 5120,
//!           "replay": "stream", "bytes": "<base64>"}}
//! ```
//!
//! - `status` is `exited`, `timedOut` or `cancelled`; `exitCode` is the
//!   exit code (128 + N for signal N), or `null` when the tool gave up
//!   on a timeout or a cancel;
//! - `chunks` is how many update chunks were sent, and `outputBytes`
//!   how many bytes they held;
//! - `replay` is `stream` when `bytes` is all the raw output (up to
//!   [`tau_terminal::DEFAULT_REPLAY_CAP`]), or `snapshot` when it is the
//!   final terminal as VT sequences. Either one, written to a new
//!   `cols` by `rows` terminal, shows the finished command.

use std::{path::Path, time::Duration};

use base64::Engine as _;
use serde_json::{Value, json};
use tau_agent::{
    error::ToolError,
    tool::{ToolCtx, ToolOutput},
};
use tau_artifacts::Bytes;
use tau_terminal::{Command, Event, Finished, Replay};

use super::{
    Accumulator,
    BashArgs,
    CommandEnd,
    Outcome,
    exit_code_of,
    finish_observed_output,
};
use crate::truncate::{MAX_BYTES, MAX_LINES};

/// The key of the terminal's details, in updates and in the result.
pub const DETAILS_KEY: &str = "term";

/// Raw bytes wait at most this long before they are sent.
pub const COALESCE_WINDOW: Duration = Duration::from_millis(16);

/// Raw bytes are sent once this many are waiting, whatever the time.
pub const COALESCE_BYTES: usize = 64 << 10;

/// Runs `args.command` with `shell -c` in `dir` under a pseudo-terminal.
pub(super) async fn run(
    shell: &Path,
    dir: &Path,
    args: &BashArgs,
    timeout: Option<Duration>,
    ctx: ToolCtx,
    artifacts: &Option<Bytes>,
) -> Result<ToolOutput, ToolError> {
    let accumulator = || {
        let acc = Accumulator::new(MAX_LINES, MAX_BYTES, std::env::temp_dir());
        if artifacts.is_some() {
            acc.with_full_capture()
        } else {
            acc
        }
    };
    let mut run = match Command::new(shell)
        .arg("-c")
        .arg(&args.command)
        .current_dir(dir)
        .spawn()
    {
        Ok(run) => run,
        Err(error) => {
            return finish_observed_output(
                accumulator(),
                artifacts,
                &ctx,
                false,
                CommandEnd {
                    outcome: Outcome::SpawnFailed,
                    exit_code: None,
                    timeout: args.timeout,
                    details: None,
                    error: Some(format!(
                        "failed to start {}: {error}",
                        shell.display()
                    )),
                },
            )
            .await;
        }
    };
    let killer = run.killer();

    let mut acc = accumulator();
    let mut stream = Stream::default();
    let mut screen = String::new();
    let mut outcome = Outcome::Done;
    let mut finished: Option<Result<Finished, String>> = None;

    let deadline = tokio::time::sleep(timeout.unwrap_or_default());
    tokio::pin!(deadline);
    let flush_at = tokio::time::sleep(COALESCE_WINDOW);
    tokio::pin!(flush_at);

    while finished.is_none() {
        tokio::select! {
            event = run.next() => match event {
                Some(Event::Output(bytes)) => {
                    if stream.pending.is_empty() {
                        flush_at
                            .as_mut()
                            .reset(tokio::time::Instant::now() + COALESCE_WINDOW);
                    }
                    stream.pending.extend_from_slice(&bytes);
                    if stream.pending.len() >= COALESCE_BYTES {
                        stream.flush(&ctx, &acc.preview(&screen));
                    }
                }
                Some(Event::Text(text)) => acc.append(text.as_bytes()),
                Some(Event::Screen(text)) => {
                    screen = text;
                    if stream.pending.is_empty() {
                        ctx.updates.send(ToolOutput::text(acc.preview(&screen)));
                    }
                }
                Some(Event::Exit(end)) => finished = Some(end),
                None => finished = Some(Err("the terminal stopped".into())),
            },
            () = &mut flush_at, if !stream.pending.is_empty() => {
                stream.flush(&ctx, &acc.preview(&screen));
            }
            () = ctx.cancel.cancelled(), if outcome == Outcome::Done => {
                outcome = Outcome::Cancelled;
                killer.kill();
            }
            () = &mut deadline, if timeout.is_some() && outcome == Outcome::Done => {
                outcome = Outcome::TimedOut;
                killer.kill();
            }
        }
    }

    let finished = match finished.expect("the loop ends with the run") {
        Ok(finished) => finished,
        Err(error) => {
            stream.flush(&ctx, &acc.preview(""));
            return finish_observed_output(
                acc,
                artifacts,
                &ctx,
                false,
                CommandEnd {
                    outcome,
                    exit_code: None,
                    timeout: args.timeout,
                    details: None,
                    error: Some(format!("terminal: {error}")),
                },
            )
            .await;
        }
    };
    acc.append(finished.text.as_bytes());
    // Every chunk goes out before the tool returns: updates sent after
    // are dropped.
    stream.flush(&ctx, &acc.preview(""));

    let exit_code = match outcome {
        Outcome::Done => Some(finished.status.map_or(1, exit_code_of)),
        Outcome::Cancelled | Outcome::TimedOut | Outcome::SpawnFailed => None,
    };
    let details = result_details(&finished, outcome, exit_code, stream.seq);
    finish_observed_output(
        acc,
        artifacts,
        &ctx,
        true,
        CommandEnd {
            outcome,
            exit_code,
            timeout: args.timeout,
            details: Some(details),
            error: None,
        },
    )
    .await
}

/// The raw bytes on their way to callers.
#[derive(Default)]
struct Stream {
    pending: Vec<u8>,
    /// The next chunk's number.
    seq: u64,
}

impl Stream {
    /// Sends what is pending as one chunk, with `text` as the update's
    /// content.
    fn flush(&mut self, ctx: &ToolCtx, text: &str) {
        if self.pending.is_empty() {
            return;
        }
        let bytes = std::mem::take(&mut self.pending);
        ctx.updates.send(ToolOutput {
            details: Some(chunk_details(self.seq, &bytes)),
            ..ToolOutput::text(text)
        });
        self.seq += 1;
    }
}

/// An update's details: chunk `seq` of the raw output.
pub fn chunk_details(seq: u64, bytes: &[u8]) -> Value {
    json!({
        DETAILS_KEY: {
            "seq": seq,
            "bytes": base64::engine::general_purpose::STANDARD.encode(bytes),
        }
    })
}

fn result_details(
    finished: &Finished,
    outcome: Outcome,
    exit_code: Option<i32>,
    chunks: u64,
) -> Value {
    let (replay, bytes) = match &finished.replay {
        Replay::Stream(bytes) => ("stream", bytes),
        Replay::Snapshot(bytes) => ("snapshot", bytes),
    };
    json!({
        DETAILS_KEY: {
            "cols": finished.size.cols,
            "rows": finished.size.rows,
            "status": match outcome {
                Outcome::Done => "exited",
                Outcome::TimedOut => "timedOut",
                Outcome::Cancelled => "cancelled",
                Outcome::SpawnFailed => "spawnFailed",
            },
            "exitCode": exit_code,
            "chunks": chunks,
            "outputBytes": finished.output_bytes,
            "replay": replay,
            "bytes": base64::engine::general_purpose::STANDARD.encode(bytes),
        }
    })
}
