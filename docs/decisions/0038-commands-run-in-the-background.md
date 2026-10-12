# 0038: Commands run in the background: `exec_command` and `write_stdin`

- Status: accepted. Replaces `bash` (pi's tool, ported in the first
  tools). Amends [0010](0010-terminal-rendering.md): the model's text
  of a command is no longer the emulator's own.
- Date: 2026-10-11

## Context

`bash` ran a command to the end. The call blocked until it exited,
and the tool killed the command's process group on a timeout or a
cancel. A dev server, a watcher, a long build, a program that asks a
question: none could be run, only waited for until the timeout killed it. The
model's workaround, `cmd &`, lost the output and the exit code.

Codex's unified exec solves this with two tools
(`codex-rs/core/src/unified_exec`). `exec_command` starts a command on a
terminal and waits a short yield time. If it ended, the call answers with
its output. If not, the call answers with what it wrote and a process id,
and the process goes on in a store that outlives the call and the turn.
`write_stdin` takes the id: it sends input or only polls, waits, and
answers the same way. By default nothing wakes the model when a process
ends; it polls. With `notify: true` on the call, the end of a process
left running wakes it (below). The store holds at most 64 processes and prunes the least recently
used, and ends them with the session. We match Codex here because its
models have been trained on exactly this contract.

## Decision

- **`bash` goes**, with its timeout, its spill file, its artifact
  publication and its pipe mode. Every command runs on a terminal.
- **`exec_command { cmd, workdir?, yield_time_ms?, tty? }`** starts the
  command and waits 250 to 30000 ms (10000 by default). **`write_stdin {
  process_id, chars?, yield_time_ms? }`** types to a process and reads it;
  with no `chars` it polls, and waits at least 5 s. Both answer with the
  new output only, once, and whether the process still runs; a non-zero
  exit is an error carrying the output, as `bash`'s was.
- **Input needs `tty: true`.** Without it stdin is `/dev/null`, as
  before, so a command that reads input still ends at once instead of
  hanging. With it, stdin is the terminal: a line, `^D` (`\u0004`) and
  `^C` (`\u0003`, which the terminal turns into `SIGINT`) reach the
  process like a person's keys. `tau-terminal` gains
  `Command::interactive` and `Run::input` for this.
- **Processes live in the run's workspace.** The host keeps one
  `exec::Processes` per workspace directory and gives it to every turn's
  tools, so a process survives turns (each turn builds new tools). A
  workspace that closes (a landing, a dropped chat) kills its
  processes; so does dropping the store, so nothing outlives tau.
  Tests and evals that build `CodingTools` get a store of their own.
- **A cancelled call leaves the process running.** The call fails with
  `Operation aborted`, saying the process goes on, and what the process
  wrote meanwhile is the next call's, as Codex stores the process before
  its first wait.
- **The model reads plain text from a small cleaner.** Between calls
  the text must be read incrementally and exactly once, which the
  emulator's screen cannot give (rows that stay on screen would repeat,
  and a program that never scrolls shows nothing until it ends).
  `exec::clean::Cleaner` reads the raw bytes: `\r` and erase redraw a
  line, backspace and tab move the cursor, other escapes are dropped,
  and a line still being written is said as far as it is and not said
  again. The raw bytes still go to the card in `details.term`, and
  tau-ui still draws them with libghostty-vt, so the card shows the
  true screen and the model reads its text. 0010's guarantee that both
  come from one engine holds only for the card; the cleaner is covered
  by a chunking-invariance property test.
- **Limits.** Each answer is `truncate_tail` (2000 lines or 50 KiB). At
  most 1 MiB of unread text and 4 MiB of raw bytes per call are kept;
  lines lost past that are counted in a note. 64 processes at most.
- **Output pruning** (tau-fast-compaction) prunes the results of both
  tools, from the text of the result: with no spill file, that text is
  the whole output, and the archive is written when it prunes.
- **Command artifacts go.** Only `read` publishes artifacts. The
  structured result for codemode is `{ process_id, running, exit_code,
  output, truncated, dropped_lines }`.

## Alternatives considered

- **Keep `bash` beside the new tools.** Two ways to run a command, and
  the model would pick the one it knows. Codex has one; so do we.
- **A `background: true` flag on `bash`.** A call that is sometimes
  blocking and sometimes not, and no way to read or feed the process
  afterwards.
- **Start a turn whenever a process ends**, as tau does for sub-agents
  (0031). Codex does not, and a process the model forgot about would
  start turns nobody asked for. So the wake is opt-in per command
  (`notify`): the model asks for it for a build or a test run, not for a
  server. It goes through the host (`Host::steer`, then a tau turn if the
  run is idle) and is told once, only if no call had reported the end.
- **Feed the emulator's text across calls.** Screen rows repeat or are
  withheld, as above.

## Consequences

- A dev server or a watcher can run while the model works on, and its
  output is read when the model asks.
- The model polls, or sets `notify`; a process that ends unreported
  waits in the store, and one never read is killed with the workspace.
- Commands that expect input need `tty: true`, which the tool's
  description says.
- `bash`'s timeout is gone: a stuck command is interrupted with
  `^C` by `write_stdin`, or ends with the run.
- Nested codemode scripts see the same two tools, and the old
  artifact grants for command output are no longer made.
