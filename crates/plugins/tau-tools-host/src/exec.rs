//! `exec_command` and `write_stdin`: run a shell command, and go on
//! with it (`docs/reference/tools.md`, "exec_command and write_stdin";
//! `docs/decisions/0038-commands-run-in-the-background.md`), as Codex's
//! unified exec does.
//!
//! `exec_command` starts a command under a pseudo-terminal and waits up
//! to its yield time for it to end. One that ends in time answers with
//! its output and exit code. One that does not keeps running: the
//! answer says so, with a process id, and the output so far.
//! `write_stdin` takes that id: it sends input (a line, or `^C`),
//! then waits and answers the same way. With no input it only polls.
//! A process outlives the call and the turn, in the run's
//! [`Processes`]; one whose end was reported goes.
//!
//! - [`clean`] turns a terminal's bytes into the plain text the model
//!   reads, as it arrives;
//! - [`processes`] keeps the processes and what they wrote between
//!   calls.
//!
//! The raw bytes go to callers in `details.term`, for a terminal view:
//! each update carries a chunk (`{"term": {"seq": 0, "bytes":
//! "<base64>"}}`, `seq` counting the call's chunks from 0), and the
//! result's details hold what the call saw:
//!
//! ```json
//! {"term": {"cols": 120, "rows": 40, "status": "running",
//!           "exitCode": null, "process": 3, "chunks": 3,
//!           "outputBytes": 5120, "replay": "stream",
//!           "bytes": "<base64>"}}
//! ```
//!
//! `status` is `running` or `exited`; `exitCode` is the exit code (128
//! + N for signal N) once it exited.

pub mod clean;
pub mod processes;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use base64::Engine as _;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use tau_agent::{
    error::ToolError,
    launch::{Launch, Launcher},
    tool::{AgentTool, ToolCtx, ToolOutput},
};
use tau_terminal::Command;

pub use self::processes::{Exit, Processes};
use self::processes::{MAX_RAW, Process, Read};
use crate::{
    path::Root,
    truncate::{self, MAX_BYTES, MAX_LINES},
};

/// The key of the terminal's details, in updates and in the result.
pub const DETAILS_KEY: &str = "term";

/// Raw bytes wait at most this long before they are sent.
pub const COALESCE_WINDOW: Duration = Duration::from_millis(16);

/// What `exec_command` waits for a command to end, unless told.
pub const DEFAULT_YIELD_MS: u64 = 10_000;

/// The shortest wait, and the longest for a call that sends input.
pub const MIN_YIELD_MS: u64 = 250;
pub const MAX_YIELD_MS: u64 = 30_000;

/// What `write_stdin` waits when it sends input and is not told.
pub const DEFAULT_WRITE_YIELD_MS: u64 = 1_000;

/// A call that only polls waits at least this long, and at most
/// [`MAX_POLL_MS`]: an empty poll is for waiting.
pub const MIN_POLL_MS: u64 = 5_000;
pub const MAX_POLL_MS: u64 = 300_000;

const EXEC_DESCRIPTION: &str = "Run a shell command in the working \
directory, on a terminal. Waits up to `yield_time_ms` for it to end and \
returns its output and exit code. A command still running then is kept \
going in the background: the result says `process <id> is still running`, \
and `write_stdin` with that id reads more of its output, or sends it \
input. Output is cut to the last 2000 lines or 50KB per call. Set `tty` \
to true for a command that reads input; otherwise its stdin is closed. Set \
`notify` to true on a long command (a build, a test run) to be told when it \
ends instead of polling.";

const WRITE_DESCRIPTION: &str = "Send input to a process `exec_command` \
left running, or read its new output. `chars` is written to its terminal \
as typed: end a line with \\n, send \\u0003 for ^C to interrupt it and \
\\u0004 for end of input. Leave `chars` out to only poll, which waits at \
least 5 seconds for output. Returns the output since the last call and \
whether the process is still running. The process needs `tty: true` to \
take input.";

/// A process that left running ended, and nothing had reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ended {
    pub process: i32,
    pub command: String,
    pub code: i32,
}

/// Told when a process started with `notify` ends.
pub type Notifier = Arc<dyn Fn(Ended) + Send + Sync>;

/// `{ cmd, workdir?, yield_time_ms?, tty?, notify? }`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExecArgs {
    /// Shell command to execute
    pub cmd: String,
    /// Directory to run it in, relative to the working directory
    /// (optional)
    pub workdir: Option<String>,
    /// Milliseconds to wait for it to end before returning with it still
    /// running (optional, 10000; 250 to 30000)
    pub yield_time_ms: Option<u64>,
    /// Run it with its stdin on the terminal, so `write_stdin` can send it
    /// input (optional, false: stdin is closed)
    pub tty: Option<bool>,
    /// If it is still running when this call returns, wake you when it
    /// ends, without a poll (optional, false)
    pub notify: Option<bool>,
}

/// `{ process_id, chars?, yield_time_ms? }`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct WriteArgs {
    /// The process id `exec_command` returned
    pub process_id: i32,
    /// Text to write to its terminal (optional: leave out to only poll)
    pub chars: Option<String>,
    /// Milliseconds to wait for output (optional; 250 to 30000 with
    /// input, 5000 to 300000 without)
    pub yield_time_ms: Option<u64>,
}

/// The `exec_command` tool. Runs `cmd` with the configured shell (or
/// the default search order) as `-c <cmd>`, in the tool's root
/// directory.
pub struct ExecCommand {
    root: Root,
    shell: Option<PathBuf>,
    parameters: Value,
    output_schema: Value,
    /// What commands start through, when the host gives one: the
    /// repository's environment (`docs/reference/environment.md`).
    launcher: Option<Arc<dyn Launcher>>,
    processes: Processes,
    /// Who hears of a `notify` process ending, when the host gives one.
    notifier: Option<Notifier>,
}

impl ExecCommand {
    pub fn new(root: Root, processes: Processes) -> Self {
        Self {
            root,
            shell: None,
            parameters: serde_json::to_value(schemars::schema_for!(ExecArgs))
                .expect("a generated schema is valid JSON"),
            output_schema: output_schema(),
            launcher: None,
            processes,
            notifier: None,
        }
    }

    /// Tells `notifier` when a process started with `notify` ends
    /// unreported.
    pub fn with_notifier(mut self, notifier: Notifier) -> Self {
        self.notifier = Some(notifier);
        self
    }

    /// Uses `path` as the shell instead of the default search order.
    pub fn with_shell(mut self, path: impl Into<PathBuf>) -> Self {
        self.shell = Some(path.into());
        self
    }

    /// Starts commands through `launcher`, which may hold the first one
    /// back while the environment loads.
    pub fn with_launcher(mut self, launcher: Arc<dyn Launcher>) -> Self {
        self.launcher = Some(launcher);
        self
    }

    /// The shell to run, per [`choose_shell`].
    fn resolve_shell(&self) -> PathBuf {
        choose_shell(
            self.shell.as_deref(),
            Path::new("/bin/bash").exists(),
            which("bash").as_deref(),
        )
    }

    /// What the command starts through. A cancel while the launcher
    /// holds it back ends the call before the command starts.
    async fn launch(&self, ctx: &ToolCtx) -> Result<Launch, ToolError> {
        let Some(launcher) = &self.launcher else {
            return Ok(Launch::default());
        };
        tokio::select! {
            launch = launcher.launch(self.root.dir()) => Ok(launch),
            () = ctx.cancel.cancelled() => Err(aborted()),
        }
    }
}

#[async_trait]
impl AgentTool for ExecCommand {
    fn name(&self) -> &str {
        "exec_command"
    }

    fn description(&self) -> &str {
        EXEC_DESCRIPTION
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn output_schema(&self) -> Option<&Value> {
        Some(&self.output_schema)
    }

    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let args: ExecArgs = serde_json::from_value(args)?;
        if ctx.cancel.is_cancelled() {
            return Err(aborted());
        }
        let window = Duration::from_millis(
            args.yield_time_ms
                .unwrap_or(DEFAULT_YIELD_MS)
                .clamp(MIN_YIELD_MS, MAX_YIELD_MS),
        );
        let dir = match &args.workdir {
            Some(workdir) => {
                let dir = self.root.resolve(workdir);
                if !dir.is_dir() {
                    return Err(format!(
                        "workdir is not a directory: {}",
                        dir.display()
                    )
                    .into());
                }
                dir
            }
            None => self.root.dir().to_owned(),
        };
        let shell = self.resolve_shell();
        let launch = self.launch(&ctx).await?;
        let command = Command::new(&shell)
            .arg("-c")
            .arg(&args.cmd)
            .through(launch.prefix.iter().cloned())
            .current_dir(dir)
            .interactive(args.tty.unwrap_or(false));
        let command = launch
            .env
            .iter()
            .fold(command, |command, (key, value)| command.env(key, value));
        let process = self.processes.start(command, args.cmd.clone()).map_err(
            |error| {
                ToolError::from(format!(
                    "failed to start {}: {error}",
                    shell.display()
                ))
            },
        )?;
        let result = report(&self.processes, &process, &ctx, window).await;
        // Still kept: it goes on after this call.
        if args.notify.unwrap_or(false)
            && let Some(notifier) = self.notifier.clone()
            && self.processes.get(process.id).is_some()
        {
            process.watch_end(&self.processes, move |process, exit| {
                notifier(Ended {
                    process: process.id,
                    command: process.command.clone(),
                    code: exit.code,
                })
            });
        }
        result
    }
}

/// The `write_stdin` tool, on the same [`Processes`] as
/// [`ExecCommand`].
pub struct WriteStdin {
    processes: Processes,
    parameters: Value,
    output_schema: Value,
}

impl WriteStdin {
    pub fn new(processes: Processes) -> Self {
        Self {
            processes,
            parameters: serde_json::to_value(schemars::schema_for!(WriteArgs))
                .expect("a generated schema is valid JSON"),
            output_schema: output_schema(),
        }
    }
}

#[async_trait]
impl AgentTool for WriteStdin {
    fn name(&self) -> &str {
        "write_stdin"
    }

    fn description(&self) -> &str {
        WRITE_DESCRIPTION
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn output_schema(&self) -> Option<&Value> {
        Some(&self.output_schema)
    }

    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let args: WriteArgs = serde_json::from_value(args)?;
        let Some(process) = self.processes.get(args.process_id) else {
            return Err(format!(
                "No process {}: it ended and was reported, or was never \
                 started.",
                args.process_id
            )
            .into());
        };
        let chars = args.chars.unwrap_or_default();
        let window = if chars.is_empty() {
            args.yield_time_ms
                .unwrap_or(MIN_POLL_MS)
                .clamp(MIN_POLL_MS, MAX_POLL_MS)
        } else {
            args.yield_time_ms
                .unwrap_or(DEFAULT_WRITE_YIELD_MS)
                .clamp(MIN_YIELD_MS, MAX_YIELD_MS)
        };
        if ctx.cancel.is_cancelled() {
            return Err(aborted());
        }
        if !chars.is_empty() {
            if !process.has_input() {
                return Err(format!(
                    "Process {} has no input: its stdin is closed. Start \
                     it with `tty: true` to send it input.",
                    process.id
                )
                .into());
            }
            // A process whose end was not reported has nothing to read
            // input: say so with what it wrote.
            if !process.has_exited() {
                process.write(&chars).await.map_err(|error| {
                    ToolError::from(format!(
                        "could not write to process {}: {error}",
                        process.id
                    ))
                })?;
            }
        }
        report(
            &self.processes,
            &process,
            &ctx,
            Duration::from_millis(window),
        )
        .await
    }
}

fn aborted() -> ToolError {
    ToolError::from(crate::ABORTED)
}

/// What a call read of `process` over `window`: waits for it to end or
/// the window to pass, streaming what arrives, then answers.
async fn report(
    processes: &Processes,
    process: &Arc<Process>,
    ctx: &ToolCtx,
    window: Duration,
) -> Result<ToolOutput, ToolError> {
    // One call at a time reads a process.
    let _turn = process.turn.lock().await;
    let mut changes = process.changes();
    let deadline = tokio::time::sleep(window);
    tokio::pin!(deadline);

    let mut seen = Read::default();
    let mut seq = 0u64;
    let mut cancelled = false;
    loop {
        if process.ready() {
            let read = process.read();
            let fresh_raw = read.raw.clone();
            seen.merge(read);
            if !fresh_raw.is_empty() {
                ctx.updates.send(ToolOutput {
                    details: Some(chunk_details(seq, &fresh_raw)),
                    ..ToolOutput::text(seen.text.text.clone())
                });
                seq += 1;
            }
            if seen.exit.is_some() {
                break;
            }
        }
        tokio::select! {
            biased;
            () = ctx.cancel.cancelled() => {
                cancelled = true;
                break;
            }
            () = &mut deadline => break,
            changed = changes.changed() => {
                if changed.is_err() {
                    break;
                }
                // Output comes in bursts: wait a moment for the rest.
                tokio::select! {
                    () = tokio::time::sleep(COALESCE_WINDOW) => {}
                    () = &mut deadline => {}
                }
            }
        }
    }
    if cancelled {
        // What it wrote meanwhile is the next call's.
        process.unread(seen);
        return Err(ToolError::from(format!(
            "{} Process {} goes on; write_stdin reads it.",
            crate::ABORTED,
            process.id
        )));
    }
    // Reported once its end is: nothing more to poll.
    if seen.exit.is_some() {
        process.take_end();
        processes.remove(process.id);
    }
    answer(process, seen, seq)
}

/// The result of a call: the output cut to the model's limits, and how
/// the process stands.
fn answer(
    process: &Process,
    seen: Read,
    chunks: u64,
) -> Result<ToolOutput, ToolError> {
    let Read {
        text,
        raw,
        raw_lost,
        exit,
        output_bytes,
    } = seen;
    let cut = truncate::truncate_tail(&text.text, MAX_LINES, MAX_BYTES);
    let mut content = cut.content.clone();
    let mut notes = Vec::new();
    if text.dropped > 0 {
        notes.push(format!(
            "{} earlier lines were dropped unread: the process wrote \
             faster than it was read",
            text.dropped
        ));
    }
    if cut.truncated() {
        notes.push(format!(
            "output cut to the last {} of {} lines",
            cut.output_lines, cut.total_lines
        ));
    }
    if !notes.is_empty() {
        content = format!("[{}]\n{content}", notes.join("; "));
    }
    let running = exit.is_none();
    let id = process.id;
    let footer = match exit {
        None if process.has_input() => Some(format!(
            "[process {id} is still running: write_stdin with process_id \
             {id} sends it input or reads its output]"
        )),
        None => Some(format!(
            "[process {id} is still running: write_stdin with process_id \
             {id} and no chars reads its output]"
        )),
        Some(exit) if exit.code != 0 => {
            Some(format!("Command exited with code {}", exit.code))
        }
        Some(_) => None,
    };
    let body = if content.is_empty() {
        if running {
            "(no new output)".to_owned()
        } else {
            "(no output)".to_owned()
        }
    } else {
        content
    };
    let display = match &footer {
        Some(footer) => format!("{body}\n\n{footer}"),
        None => body.clone(),
    };
    let structured = json!({
        "process_id": running.then_some(id),
        "running": running,
        "exit_code": exit.map(|exit| exit.code),
        "output": body,
        "truncated": cut.truncated(),
        "dropped_lines": text.dropped,
    });
    let details = json!({
        DETAILS_KEY: {
            "cols": process.size.cols,
            "rows": process.size.rows,
            "status": if running { "running" } else { "exited" },
            "exitCode": exit.map(|exit| exit.code),
            "process": id,
            "chunks": chunks,
            "outputBytes": output_bytes,
            "replay": "stream",
            "lost": raw_lost,
            "bytes": base64::engine::general_purpose::STANDARD.encode(&raw),
        }
    });
    let output = ToolOutput {
        details: Some(details),
        structured: Some(structured),
        ..ToolOutput::text(display)
    };
    match exit {
        Some(exit) if exit.code != 0 => Err(ToolError::output(output)),
        _ => Ok(output),
    }
}

impl Read {
    /// Adds what a later read found.
    fn merge(&mut self, later: Read) {
        let Read {
            text,
            mut raw,
            raw_lost,
            exit,
            output_bytes,
        } = later;
        if !text.text.is_empty() {
            if !self.text.text.is_empty() {
                self.text.text.push('\n');
            }
            self.text.text.push_str(&text.text);
        }
        self.text.dropped += text.dropped;
        self.raw.append(&mut raw);
        if self.raw.len() > MAX_RAW {
            let cut = self.raw.len() - MAX_RAW;
            self.raw.drain(..cut);
            self.raw_lost = true;
        }
        self.raw_lost |= raw_lost;
        self.exit = exit.or(self.exit);
        self.output_bytes = output_bytes;
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

fn output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "process_id": {"type": ["integer", "null"]},
            "running": {"type": "boolean"},
            "exit_code": {"type": ["integer", "null"]},
            "output": {"type": "string"},
            "truncated": {"type": "boolean"},
            "dropped_lines": {"type": "integer", "minimum": 0}
        },
        "required": ["process_id", "running", "exit_code", "output",
            "truncated", "dropped_lines"],
        "additionalProperties": false
    })
}

/// The shell selection order (`docs/reference/tools.md`, "Shell"): a
/// configured path if one is set; otherwise `/bin/bash`, then `bash` on
/// `PATH`, then `sh`. Takes the filesystem state as arguments so it can
/// be tested without touching the real filesystem.
pub fn choose_shell(
    configured: Option<&Path>,
    bin_bash_exists: bool,
    bash_on_path: Option<&Path>,
) -> PathBuf {
    if let Some(path) = configured {
        return path.to_owned();
    }
    if bin_bash_exists {
        return PathBuf::from("/bin/bash");
    }
    if let Some(path) = bash_on_path {
        return path.to_owned();
    }
    PathBuf::from("sh")
}

/// The first executable named `name` on `PATH`.
fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

fn exit_code_of(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
        .unwrap_or(1)
}
