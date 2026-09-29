//! `bash`: run a shell command (`docs/reference/tools.md`, "bash"),
//! ported from pi's `bash.ts` and `output-accumulator.ts`.
//!
//! The pieces that do not need a real process are kept pure so tests can
//! drive them directly:
//!
//! - [`Accumulator`] tracks a rolling tail of decoded output, decides
//!   when to spill the full output to a file, and produces the
//!   [`truncate::truncate_tail`] snapshot the model sees.
//! - [`ProgressThrottle`] decides when a progress update is allowed,
//!   given the time of the last one.
//! - [`choose_shell`] is the shell-selection order, given the state of
//!   the filesystem as booleans instead of doing the checks itself.
//! - [`drain_until_idle`] is the "read until the pipes are idle" loop
//!   (`docs/reference/tools.md`, "bash", "Late output"), generic over any
//!   source of [`ChunkEvent`]s so it can be driven by a scripted source
//!   under paused time in tests, and by real pipes in production.
//!
//! Real process I/O (spawning, the process group, signals) lives only in
//! [`Bash::run`], which wires the pure pieces together.
//!
//! With the `terminal` feature, commands run under a pseudo-terminal
//! instead (`docs/decisions/0010-terminal-rendering.md`): see
//! `bash::terminal`, which adapts tau-terminal's runner to this tool.

#[cfg(feature = "terminal")]
pub mod terminal;

use std::{
    future::Future,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use tau_agent::{
    error::ToolError,
    tool::{AgentTool, ToolCtx, ToolOutput},
};
use tokio::{
    io::AsyncReadExt,
    sync::{Notify, mpsc, oneshot},
};

use crate::{
    path::Root,
    truncate::{self, Limit, MAX_BYTES, MAX_LINES},
};

/// How long the pipes must be idle, after the child has exited, before
/// the tool stops reading (`docs/reference/tools.md`, "bash", "Late
/// output"; pi's `EXIT_STDIO_GRACE_MS`).
const IDLE_WINDOW: Duration = Duration::from_millis(100);

/// How often a progress update may be sent while a command runs (pi's
/// `BASH_UPDATE_THROTTLE_MS`).
const THROTTLE_WINDOW: Duration = Duration::from_millis(100);

/// Ported verbatim from pi's `createShellToolDefinition` (`bash.ts:239`),
/// with `shellName = "bash"` and pi's `DEFAULT_MAX_LINES` / `DEFAULT_MAX_BYTES`.
const DESCRIPTION: &str = "Execute a bash command in the current working \
directory. Returns stdout and stderr. Output is truncated to last 2000 \
lines or 50KB (whichever is hit first). If truncated, full output is \
saved to a temp file. Optionally provide a timeout in seconds.";

/// `{ command, timeout? }` (`docs/reference/tools.md`, "bash").
#[derive(Debug, Deserialize, JsonSchema)]
pub struct BashArgs {
    /// Shell command to execute
    pub command: String,
    /// Timeout in seconds (optional, no default timeout)
    pub timeout: Option<f64>,
}

/// The `bash` tool. Runs `command` with the configured shell (or the
/// default search order) as `-c <command>`, in the tool's root
/// directory.
pub struct Bash {
    root: Root,
    shell: Option<PathBuf>,
    parameters: Value,
    /// Whether commands run under a pseudo-terminal.
    #[cfg(feature = "terminal")]
    terminal: bool,
}

impl Bash {
    pub fn new(root: Root) -> Self {
        let parameters = serde_json::to_value(schemars::schema_for!(BashArgs))
            .expect("a generated schema is valid JSON");
        Self {
            root,
            shell: None,
            parameters,
            #[cfg(feature = "terminal")]
            terminal: true,
        }
    }

    /// Runs commands under a pseudo-terminal (the default with the
    /// `terminal` feature), or with pipes as without it.
    #[cfg(feature = "terminal")]
    pub fn with_terminal(mut self, on: bool) -> Self {
        self.terminal = on;
        self
    }

    /// Uses `path` as the shell instead of the default search order.
    pub fn with_shell(mut self, path: impl Into<PathBuf>) -> Self {
        self.shell = Some(path.into());
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
}

#[async_trait]
impl AgentTool for Bash {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let args: BashArgs = serde_json::from_value(args)?;
        self.run(args, ctx).await
    }
}

/// The shell selection order (`docs/reference/tools.md`, "bash",
/// "Shell"): a configured path if one is set; otherwise `/bin/bash`,
/// then `bash` on `PATH`, then `sh`. Takes the filesystem state as
/// arguments so it can be tested without touching the real filesystem.
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

/// A chunk of output, or the end of one source (`docs/reference/tools.md`,
/// "bash", "Output": stdout and stderr merge in arrival order).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChunkEvent {
    Data(Vec<u8>),
    Eof,
}

/// Reads [`ChunkEvent`]s from `rx` (a merge of stdout and stderr,
/// preserving arrival order), calling `on_chunk` for each data event,
/// until both sources report [`ChunkEvent::Eof`], or, once `exited`
/// resolves, the source has been idle for `idle`
/// (`docs/reference/tools.md`, "bash", "Late output"). A chunk that
/// arrives in the same instant the idle window elapses is still read
/// before finalizing (pi #5303). Output that arrives after this
/// function returns (the channel has no more receiver) is silently
/// dropped (pi #5208).
pub async fn drain_until_idle<F>(
    mut rx: mpsc::UnboundedReceiver<ChunkEvent>,
    exited: impl Future<Output = ()>,
    idle: Duration,
    mut on_chunk: F,
) where
    F: FnMut(&[u8]),
{
    tokio::pin!(exited);
    let mut has_exited = false;
    let mut eof_count = 0u8;
    let sleep = tokio::time::sleep(idle);
    tokio::pin!(sleep);
    let mut idle_armed = false;

    loop {
        tokio::select! {
            biased;
            event = rx.recv() => {
                match event {
                    Some(ChunkEvent::Data(bytes)) => {
                        on_chunk(&bytes);
                        if has_exited {
                            sleep.as_mut().reset(tokio::time::Instant::now() + idle);
                            idle_armed = true;
                        }
                    }
                    Some(ChunkEvent::Eof) => {
                        eof_count += 1;
                        if eof_count >= 2 {
                            break;
                        }
                    }
                    None => break,
                }
            }
            _ = &mut exited, if !has_exited => {
                has_exited = true;
                sleep.as_mut().reset(tokio::time::Instant::now() + idle);
                idle_armed = true;
            }
            () = &mut sleep, if idle_armed => {
                // A sender whose own timer fires at the same instant may
                // not have reached its `send` yet; give the executor a
                // few turns to run it before treating the pipes as idle
                // (pi #5303).
                for _ in 0..16 {
                    tokio::task::yield_now().await;
                }
                while let Ok(event) = rx.try_recv() {
                    if let ChunkEvent::Data(bytes) = event {
                        on_chunk(&bytes);
                    }
                }
                break;
            }
        }
    }
}

/// Reads `pipe` in a loop, sending each chunk (and a final `Eof`) to
/// `tx`. Ignored once the receiving end is gone.
fn spawn_pipe_reader<R>(tx: mpsc::UnboundedSender<ChunkEvent>, mut pipe: R)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut buf = [0u8; 8192];
        loop {
            match pipe.read(&mut buf).await {
                Ok(0) | Err(_) => {
                    let _ = tx.send(ChunkEvent::Eof);
                    return;
                }
                Ok(n) => {
                    if tx.send(ChunkEvent::Data(buf[..n].to_vec())).is_err() {
                        return;
                    }
                }
            }
        }
    });
}

/// Decides when a progress update is allowed: immediately the first
/// time, then no more than once per `window`
/// (`docs/reference/tools.md`, "bash", "Progress").
pub struct ProgressThrottle {
    window: Duration,
    last: Option<Instant>,
}

impl ProgressThrottle {
    pub fn new(window: Duration) -> Self {
        Self { window, last: None }
    }

    /// Whether an update may be sent at `now`. Recording state as if it
    /// were sent when it returns `true`.
    pub fn allow(&mut self, now: Instant) -> bool {
        let allowed = match self.last {
            Some(last) => now.saturating_duration_since(last) >= self.window,
            None => true,
        };
        if allowed {
            self.last = Some(now);
        }
        allowed
    }
}

/// A snapshot of an [`Accumulator`]'s output: [`truncate::truncate_tail`]
/// of the full output seen so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub content: String,
    /// `None` when nothing was cut, over the *full* output (not just the
    /// rolling tail).
    pub by: Option<Limit>,
    pub total_lines: usize,
    pub total_bytes: usize,
    pub output_lines: usize,
    pub output_bytes: usize,
    pub last_line_partial: bool,
}

impl Snapshot {
    pub fn truncated(&self) -> bool {
        self.by.is_some()
    }
}

/// Incrementally tracks a command's merged stdout/stderr
/// (`docs/reference/tools.md`, "bash", "Output"), ported from pi's
/// `OutputAccumulator`.
///
/// Keeps a rolling tail of at least `2 * max_bytes` decoded text (its
/// cut rounded back to a character boundary), decoding UTF-8 across
/// chunk boundaries so a multi-byte character split
/// between two `append` calls still decodes correctly. Once the total
/// output passes `max_lines` or `max_bytes`, the full raw output is
/// spilled to a file under `spill_dir`.
pub struct Accumulator {
    max_lines: usize,
    max_bytes: usize,
    max_rolling_bytes: usize,
    spill_dir: PathBuf,

    pending_utf8: Vec<u8>,
    tail: String,
    tail_starts_at_line_boundary: bool,

    total_decoded_bytes: usize,
    total_raw_bytes: usize,
    completed_lines: usize,
    has_open_line: bool,

    raw_buffer: Vec<u8>,
    spill: Option<(PathBuf, std::fs::File)>,
    finished: bool,
}

impl Accumulator {
    pub fn new(
        max_lines: usize,
        max_bytes: usize,
        spill_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            max_lines,
            max_bytes,
            max_rolling_bytes: (max_bytes * 2).max(1),
            spill_dir: spill_dir.into(),
            pending_utf8: Vec::new(),
            tail: String::new(),
            tail_starts_at_line_boundary: true,
            total_decoded_bytes: 0,
            total_raw_bytes: 0,
            completed_lines: 0,
            has_open_line: false,
            raw_buffer: Vec::new(),
            spill: None,
            finished: false,
        }
    }

    /// Appends a chunk of raw output. Panics if called after [`finish`](Self::finish).
    pub fn append(&mut self, chunk: &[u8]) {
        assert!(!self.finished, "append after finish");
        if chunk.is_empty() {
            return;
        }
        self.total_raw_bytes += chunk.len();
        let decoded = self.decode(chunk);
        self.push_decoded(&decoded);

        if self.spill.is_some() || self.should_spill() {
            self.ensure_spill();
            if let Some((_, file)) = &mut self.spill {
                use std::io::Write;
                let _ = file.write_all(chunk);
            }
        } else {
            self.raw_buffer.extend_from_slice(chunk);
        }
    }

    /// Flushes any pending partial UTF-8 sequence and spills the file if
    /// the limits were hit but no chunk since crossed the threshold.
    pub fn finish(&mut self) {
        if self.finished {
            return;
        }
        self.finished = true;
        let rest = self.flush_decoder();
        self.push_decoded(&rest);
        if self.should_spill() {
            self.ensure_spill();
        }
    }

    /// [`truncate::truncate_tail`] of the full output seen so far, with
    /// `total_lines`/`total_bytes` over the *whole* output rather than
    /// just the rolling tail. A line the rolling tail starts inside
    /// counts as the longer line it is: shown, cut and marked, only when
    /// it is the last line.
    pub fn snapshot(&self) -> Snapshot {
        let tail = truncate::truncate_cut_tail(
            &self.tail,
            !self.tail_starts_at_line_boundary,
            self.max_lines,
            self.max_bytes,
        );
        let total_lines = self.total_lines();
        let truncated = total_lines > self.max_lines
            || self.total_decoded_bytes > self.max_bytes;
        let by = if truncated {
            Some(tail.by.unwrap_or(
                if self.total_decoded_bytes > self.max_bytes {
                    Limit::Bytes
                } else {
                    Limit::Lines
                },
            ))
        } else {
            None
        };
        Snapshot {
            content: tail.content,
            by,
            total_lines,
            total_bytes: self.total_decoded_bytes,
            output_lines: tail.output_lines,
            output_bytes: tail.output_bytes,
            last_line_partial: tail.last_line_partial,
        }
    }

    /// What the model would see if `rest` came next and the output
    /// ended: [`truncate::truncate_tail`] of the rolling tail followed by
    /// `rest`, for a progress update. Leaves the accumulator as it is.
    pub fn preview(&self, rest: &str) -> String {
        let mut text = self.tail.clone();
        text.push_str(rest);
        truncate::truncate_cut_tail(
            &text,
            !self.tail_starts_at_line_boundary,
            self.max_lines,
            self.max_bytes,
        )
        .content
    }

    /// Where the full output was spilled, once truncation required it.
    pub fn spill_path(&self) -> Option<&Path> {
        self.spill.as_ref().map(|(path, _)| path.as_path())
    }

    fn total_lines(&self) -> usize {
        self.completed_lines + usize::from(self.has_open_line)
    }

    fn should_spill(&self) -> bool {
        self.total_raw_bytes > self.max_bytes
            || self.total_decoded_bytes > self.max_bytes
            || self.total_lines() > self.max_lines
    }

    fn ensure_spill(&mut self) {
        if self.spill.is_some() {
            return;
        }
        let path = self.spill_dir.join(spill_filename());
        if let Ok(mut file) = std::fs::File::create(&path) {
            use std::io::Write;
            let _ = file.write_all(&self.raw_buffer);
            self.raw_buffer.clear();
            self.spill = Some((path, file));
        }
    }

    /// Decodes `chunk` as UTF-8, carrying an incomplete trailing
    /// sequence over to the next call so a multi-byte character split
    /// across chunks still decodes correctly. Invalid bytes are
    /// replaced, matching `String::from_utf8_lossy` over the whole
    /// stream regardless of how it was chunked.
    fn decode(&mut self, chunk: &[u8]) -> String {
        self.pending_utf8.extend_from_slice(chunk);
        let buf = std::mem::take(&mut self.pending_utf8);
        let mut out = String::new();
        let mut start = 0;
        loop {
            match std::str::from_utf8(&buf[start..]) {
                Ok(s) => {
                    out.push_str(s);
                    return out;
                }
                Err(e) => {
                    let valid_up_to = e.valid_up_to();
                    out.push_str(
                        std::str::from_utf8(&buf[start..start + valid_up_to])
                            .expect("valid_up_to bounds a valid prefix"),
                    );
                    match e.error_len() {
                        Some(bad) => {
                            out.push('\u{FFFD}');
                            start += valid_up_to + bad;
                        }
                        None => {
                            self.pending_utf8 =
                                buf[start + valid_up_to..].to_vec();
                            return out;
                        }
                    }
                }
            }
        }
    }

    fn flush_decoder(&mut self) -> String {
        if self.pending_utf8.is_empty() {
            return String::new();
        }
        let bytes = std::mem::take(&mut self.pending_utf8);
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn push_decoded(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.total_decoded_bytes += text.len();
        self.tail.push_str(text);
        if self.tail.len() > self.max_rolling_bytes * 2 {
            self.trim_tail();
        }

        let mut newlines = 0usize;
        let mut last_newline = None;
        for (i, _) in text.match_indices('\n') {
            newlines += 1;
            last_newline = Some(i);
        }
        if newlines == 0 {
            self.has_open_line = true;
        } else {
            self.completed_lines += newlines;
            let rest = &text[last_newline.expect("newlines > 0") + 1..];
            self.has_open_line = !rest.is_empty();
        }
    }

    fn trim_tail(&mut self) {
        if self.tail.len() <= self.max_rolling_bytes {
            return;
        }
        // Back to a character boundary, so the tail never holds less
        // than `max_rolling_bytes`: whatever the byte limit keeps of the
        // full output is in it.
        let mut start = self.tail.len() - self.max_rolling_bytes;
        while !self.tail.is_char_boundary(start) {
            start -= 1;
        }
        if start > 0 {
            self.tail_starts_at_line_boundary =
                self.tail.as_bytes()[start - 1] == b'\n';
        }
        self.tail = self.tail[start..].to_owned();
    }
}

/// A file name unique enough for a temp file: `tau-bash-<hex>.log`
/// (`docs/reference/tools.md`, "bash", "Output").
fn spill_filename() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = u128::from(std::process::id());
    let mixed = nanos ^ (pid << 64) ^ u128::from(counter);
    format!("tau-bash-{mixed:016x}.log")
}

/// `text`, then `status` on its own paragraph; just `status` if `text`
/// is empty (pi's `appendStatus`, `bash.ts:341`).
fn append_status(text: &str, status: impl std::fmt::Display) -> String {
    if text.is_empty() {
        status.to_string()
    } else {
        format!("{text}\n\n{status}")
    }
}

/// `text`, or `(no output)` if it is empty.
fn or_no_output(text: String) -> String {
    if text.is_empty() {
        "(no output)".to_string()
    } else {
        text
    }
}

fn validate_timeout(
    timeout: Option<f64>,
) -> Result<Option<Duration>, ToolError> {
    match timeout {
        None => Ok(None),
        Some(t) if t.is_finite() && t > 0.0 => {
            Ok(Some(Duration::from_secs_f64(t)))
        }
        Some(_) => Err(ToolError::from(
            "Invalid timeout: must be a finite number of seconds",
        )),
    }
}

fn kill_group(pid: u32) {
    // SAFETY: `kill` with a negative pid signals the whole process
    // group; it has no memory-safety preconditions.
    unsafe {
        libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
    }
}

fn exit_code_of(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
        .unwrap_or(1)
}

/// What ended the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// The command exited (or its output finished draining) on its own.
    Done,
    Cancelled,
    TimedOut,
}

impl Bash {
    async fn run(
        &self,
        args: BashArgs,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let timeout = validate_timeout(args.timeout)?;

        if ctx.cancel.is_cancelled() {
            return Err(ToolError::from("Command aborted"));
        }

        let shell = self.resolve_shell();
        #[cfg(feature = "terminal")]
        if self.terminal {
            return terminal::run(&shell, self.root.dir(), &args, timeout, ctx)
                .await;
        }
        let mut command = tokio::process::Command::new(&shell);
        command
            .arg("-c")
            .arg(&args.command)
            .current_dir(self.root.dir())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command.process_group(0);

        let mut child = command.spawn().map_err(|error| {
            format!("failed to start {}: {error}", shell.display())
        })?;
        let pid = child.id().ok_or("spawned child has no pid")?;
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");

        let (tx, rx) = mpsc::unbounded_channel();
        spawn_pipe_reader(tx.clone(), stdout);
        spawn_pipe_reader(tx, stderr);

        let notify = Arc::new(Notify::new());
        let (code_tx, code_rx) = oneshot::channel::<i32>();
        {
            let notify = notify.clone();
            tokio::spawn(async move {
                let code = match child.wait().await {
                    Ok(status) => exit_code_of(status),
                    Err(_) => 1,
                };
                notify.notify_one();
                let _ = code_tx.send(code);
            });
        }

        let mut acc =
            Accumulator::new(MAX_LINES, MAX_BYTES, std::env::temp_dir());
        let mut throttle = ProgressThrottle::new(THROTTLE_WINDOW);

        let outcome = {
            let exited = {
                let notify = notify.clone();
                async move { notify.notified().await }
            };
            let on_chunk = |chunk: &[u8]| {
                acc.append(chunk);
                if throttle.allow(Instant::now()) {
                    ctx.updates.send(ToolOutput::text(acc.snapshot().content));
                }
            };
            let drain_fut = drain_until_idle(rx, exited, IDLE_WINDOW, on_chunk);
            tokio::pin!(drain_fut);

            let sleep =
                tokio::time::sleep(timeout.unwrap_or(Duration::from_secs(0)));
            tokio::pin!(sleep);

            let outcome = tokio::select! {
                () = &mut drain_fut => Outcome::Done,
                () = ctx.cancel.cancelled() => Outcome::Cancelled,
                () = &mut sleep, if timeout.is_some() => Outcome::TimedOut,
            };

            if !matches!(outcome, Outcome::Done) {
                kill_group(pid);
                drain_fut.await;
            }
            outcome
        };

        let exit_code = match outcome {
            Outcome::Done => Some(code_rx.await.unwrap_or(1)),
            Outcome::Cancelled | Outcome::TimedOut => None,
        };
        finish(acc, outcome, exit_code, args.timeout, None)
    }
}

/// The tool's result once the output is read: `acc`'s
/// [`truncate::truncate_tail`] with the spill notice, then the status
/// `outcome` and `exit_code` call for (`docs/reference/tools.md`,
/// "bash", "Error strings"). `details`, when given, stay on the result,
/// failed or not.
fn finish(
    mut acc: Accumulator,
    outcome: Outcome,
    exit_code: Option<i32>,
    timeout: Option<f64>,
    details: Option<Value>,
) -> Result<ToolOutput, ToolError> {
    acc.finish();
    let snapshot = acc.snapshot();
    let truncated = snapshot.truncated();
    let mut content = snapshot.content;
    if truncated && let Some(path) = acc.spill_path() {
        content.push_str(&format!("\n\nFull output: {}", path.display()));
    }

    let failed = |text: String| match &details {
        Some(details) => ToolError::output(ToolOutput {
            details: Some(details.clone()),
            ..ToolOutput::text(text)
        }),
        None => ToolError::from(text),
    };
    match outcome {
        Outcome::Cancelled => {
            Err(failed(append_status(&content, "Command aborted")))
        }
        Outcome::TimedOut => {
            // Report the value the caller gave us, not one recovered
            // from the `Duration` (`bash.ts:361`: pi reports the input
            // number verbatim).
            let secs = timeout.expect("timed out implies a timeout was set");
            Err(failed(append_status(
                &content,
                format!("Command timed out after {secs} seconds"),
            )))
        }
        Outcome::Done => {
            let exit_code = exit_code.unwrap_or(1);
            let display = or_no_output(content);
            if exit_code == 0 {
                Ok(ToolOutput {
                    details,
                    ..ToolOutput::text(display)
                })
            } else {
                Err(failed(append_status(
                    &display,
                    format!("Command exited with code {exit_code}"),
                )))
            }
        }
    }
}

/// Unit tests of private helpers, with local fixtures
/// (`docs/reference/testing.md`: "Properties of private helpers stay in
/// `#[cfg(test)]` modules").
#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    /// `is_executable` requires a regular file with at least one
    /// executable bit, not just any bit set (`docs/reference/tools.md`,
    /// "bash", "Shell": `bash` on `PATH`); `which` returns the first
    /// match, or `None` when there is none.
    #[test]
    fn which_finds_the_first_executable_match_on_path() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("myshell");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o644))
            .unwrap();
        assert!(!is_executable(&exe), "0o644 has no executable bit");

        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755))
            .unwrap();
        assert!(is_executable(&exe), "0o755 is executable by its owner");

        let subdir = dir.path().join("adir");
        std::fs::create_dir(&subdir).unwrap();
        std::fs::set_permissions(
            &subdir,
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        assert!(!is_executable(&subdir), "a directory is never executable");

        let path_var = format!("{}:/nonexistent-dir-xyz", dir.path().display());
        // SAFETY: nextest runs each test in its own process.
        unsafe { std::env::set_var("PATH", &path_var) };
        assert_eq!(which("myshell"), Some(exe));
        assert_eq!(which("no-such-binary-xyz"), None);
    }

    /// The rolling tail is left alone at exactly twice `max_bytes`, and
    /// trimmed back down to twice `max_bytes` the byte after
    /// (`docs/reference/tools.md`, "bash", "Output": "a rolling tail of
    /// about 2 * MAX_BYTES is kept").
    #[test]
    fn the_rolling_tail_trims_only_strictly_past_twice_max_bytes() {
        let dir = tempfile::tempdir().unwrap();
        // max_rolling_bytes = 200; the trim trigger is at 400.
        let mut acc = Accumulator::new(1_000_000, 100, dir.path());
        for _ in 0..4 {
            acc.append(&[b'a'; 100]);
        }
        assert_eq!(
            acc.tail.len(),
            400,
            "exactly twice max_bytes must not trim yet"
        );
        acc.append(b"x");
        assert_eq!(
            acc.tail.len(),
            200,
            "one byte past twice max_bytes must trim back to it"
        );
    }

    /// Trimming the rolling tail always cuts at a UTF-8 character
    /// boundary, searching *back* from the target byte offset, so the
    /// tail never holds less than it should (`docs/reference/tools.md`,
    /// "bash", "Output").
    #[test]
    fn trim_tail_lands_on_the_previous_char_boundary() {
        let dir = tempfile::tempdir().unwrap();
        // max_rolling_bytes = 8; six 3-byte characters (18 bytes) cross
        // the trigger (16) once, with a target cut at byte 10, which is
        // inside the fourth character (bytes 9..12). The previous
        // boundary is at 9, leaving nine bytes of the 18-byte tail.
        let mut acc = Accumulator::new(1_000_000, 4, dir.path());
        for _ in 0..6 {
            acc.append("€".as_bytes());
        }
        assert_eq!(acc.tail.len(), 9);
        // The byte just before the cut (offset 8, inside the third
        // character) is not a newline.
        assert!(!acc.tail_starts_at_line_boundary);
    }

    /// Spilling triggers on any of the three limits alone: decoded bytes
    /// (isolated from raw bytes with an invalid byte, which expands to a
    /// 3-byte replacement character), or lines -- not only when several
    /// are crossed together (`docs/reference/tools.md`, "bash",
    /// "Output").
    #[test]
    fn spilling_triggers_on_any_one_limit_alone() {
        let dir = tempfile::tempdir().unwrap();

        // raw = 1 byte (within max_bytes = 2); decoded = 3 bytes (a
        // replacement character), over it.
        let mut by_decoded_bytes = Accumulator::new(1_000_000, 2, dir.path());
        by_decoded_bytes.append(&[0x80]);
        assert!(
            by_decoded_bytes.spill_path().is_some(),
            "decoded bytes over the limit must spill"
        );

        // 3 lines, well under a huge max_bytes.
        let mut by_lines = Accumulator::new(2, 1_000_000, dir.path());
        by_lines.append(b"a\nb\nc\n");
        assert!(
            by_lines.spill_path().is_some(),
            "lines over the limit must spill"
        );

        // Well within every limit: no spill.
        let mut neither = Accumulator::new(1_000_000, 1_000_000, dir.path());
        neither.append(b"a\nb\n");
        assert!(
            neither.spill_path().is_none(),
            "within every limit must not spill"
        );
    }

    /// Regression: bytes that decode to more than the byte limit are
    /// reported as cut, though what is kept of them is empty (found by
    /// `output_rechunked_at_any_byte_boundary_gives_the_same_result`
    /// with `bytes = [0x80, 0x80, 0x80]`, `max_lines = max_bytes = 1`).
    /// The kept end of the one line is under a character, and too short
    /// for the cut marker, so it is empty; the one kept line meets the
    /// line limit, which `truncate_tail` reports as `Lines`.
    #[test]
    fn snapshot_of_output_over_the_limit_is_cut() {
        let dir = tempfile::tempdir().unwrap();
        let mut acc = Accumulator::new(1, 1, dir.path());
        acc.append(&[0x80, 0x80, 0x80]);
        acc.finish();
        let snapshot = acc.snapshot();
        assert_eq!(snapshot.content, "");
        assert_eq!(snapshot.by, Some(Limit::Lines));
        assert!(snapshot.last_line_partial);
    }

    /// The spill file name has the documented shape,
    /// `tau-bash-<hex>.log` (`docs/reference/tools.md`, "bash",
    /// "Output"), and is different every time.
    #[test]
    fn spill_filename_has_the_documented_shape_and_is_unique() {
        let a = spill_filename();
        let b = spill_filename();
        for name in [&a, &b] {
            assert!(name.starts_with("tau-bash-"), "{name}");
            assert!(name.ends_with(".log"), "{name}");
            let hex = &name["tau-bash-".len()..name.len() - ".log".len()];
            assert!(
                !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit()),
                "{name}"
            );
        }
        assert_ne!(a, b);
    }

    /// `truncated` (via `snapshot().by`) is strict: exactly at either
    /// limit is not truncated, one past it is
    /// (`docs/reference/tools.md`, "bash", "Output"; matches
    /// `truncate::truncate_tail`'s own "whole" check).
    #[test]
    fn truncated_is_strict_not_inclusive_of_the_limits() {
        let dir = tempfile::tempdir().unwrap();

        let mut at_byte_limit = Accumulator::new(1_000_000, 3, dir.path());
        at_byte_limit.append(b"abc"); // decoded = 3, exactly max_bytes
        assert!(!at_byte_limit.snapshot().truncated());

        let mut at_line_limit = Accumulator::new(2, 1_000_000, dir.path());
        at_line_limit.append(b"a\nb"); // 1 completed + 1 open = 2, exactly max_lines
        assert!(!at_line_limit.snapshot().truncated());

        let mut over_bytes = Accumulator::new(1_000_000, 3, dir.path());
        over_bytes.append(b"abcd"); // 4 > 3
        assert!(over_bytes.snapshot().truncated());

        let mut over_lines = Accumulator::new(2, 1_000_000, dir.path());
        over_lines.append(b"a\nb\nc"); // 3 > 2
        assert!(over_lines.snapshot().truncated());
    }

    /// `should_spill` is strict: exactly at either limit does not spill,
    /// one past it does (`docs/reference/tools.md`, "bash", "Output").
    #[test]
    fn spilling_is_strict_not_inclusive_of_the_limits() {
        let dir = tempfile::tempdir().unwrap();

        let mut at_byte_limit = Accumulator::new(1_000_000, 3, dir.path());
        at_byte_limit.append(b"abc");
        assert!(at_byte_limit.spill_path().is_none());

        let mut at_line_limit = Accumulator::new(2, 1_000_000, dir.path());
        at_line_limit.append(b"a\nb");
        assert!(at_line_limit.spill_path().is_none());

        let mut over_bytes = Accumulator::new(1_000_000, 3, dir.path());
        over_bytes.append(b"abcd");
        assert!(over_bytes.spill_path().is_some());

        let mut over_lines = Accumulator::new(2, 1_000_000, dir.path());
        over_lines.append(b"a\nb\nc");
        assert!(over_lines.spill_path().is_some());
    }

    /// A trailing newline closes the current line: no open line is left
    /// dangling after it (`docs/reference/tools.md`, "bash", "Output").
    #[test]
    fn a_trailing_newline_does_not_leave_an_open_line() {
        let dir = tempfile::tempdir().unwrap();
        let mut acc = Accumulator::new(1_000_000, 1_000_000, dir.path());
        acc.append(b"a\n");
        assert!(!acc.has_open_line);
    }

    /// Trimming records whether the byte right before the cut is a
    /// newline, so the next snapshot knows the tail already
    /// starts at a line boundary (`docs/reference/tools.md`, "bash",
    /// "Output").
    #[test]
    fn trim_tail_correctly_flags_a_line_boundary_at_the_cut() {
        let dir = tempfile::tempdir().unwrap();
        // max_rolling_bytes = 4; five "a\n" pairs (10 bytes) cross the
        // trigger (8) once, cutting at byte 6 -- right after a '\n'.
        let mut acc = Accumulator::new(1_000_000, 2, dir.path());
        for _ in 0..5 {
            acc.append(b"a\n");
        }
        assert_eq!(acc.tail, "a\na\n");
        assert!(acc.tail_starts_at_line_boundary);
    }

    /// A last line the rolling tail starts inside is shown as its end,
    /// after the cut marker, within the byte limit
    /// (`docs/reference/tools.md`, "bash", "Output").
    #[test]
    fn a_cut_last_line_shows_its_end_marked() {
        let dir = tempfile::tempdir().unwrap();
        // max_rolling_bytes = 20; 81 bytes cross the trigger (40).
        let mut acc = Accumulator::new(10, 10, dir.path());
        acc.append(&[b'x'; 80]);
        acc.append(b"\n");
        acc.finish();
        assert!(!acc.tail_starts_at_line_boundary);
        let snapshot = acc.snapshot();
        assert_eq!(snapshot.content, format!("{}xxxxxxx", truncate::CUT));
        assert_eq!(snapshot.content.len(), 10);
        assert!(snapshot.last_line_partial);
    }
}
