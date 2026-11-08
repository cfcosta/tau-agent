# Fast compaction

`tau-fast-compaction` (`plugins/tau-fast-compaction`) keeps a run's
context lean with Jev, TypeSafe's System One model, in two stages. It is
a Luau plugin tau comes with (ADR 0050): `src/` holds the rules,
and `src/plugin.luau` the hooks and what it draws. It never summarizes: what stays is verbatim, and what goes is archived or
can be re-run. When pruning cannot free enough, summarizing compaction
takes over.

- **Output pruning** trims a large `exec_command` or `write_stdin` result to the lines the task
  still needs, the moment it is produced, before the model first sees
  it. It changes nothing already sent, so it costs no resend.
- **History pruning** drops or cuts stale tool calls and results
  between turns, as a context rewrite.

History pruning is a port of
[`joelhooks/pi-fast-jev-compaction`](https://github.com/joelhooks/pi-fast-jev-compaction),
itself built on
[`tamaratran/fast-jev-compaction`](https://github.com/tamaratran/fast-jev-compaction),
onto tau's context seam ([plugins.md](plugins.md)). Output pruning, the
partitioned history and the token estimate come from
[`tamaratran/jev-pruner`](https://github.com/tamaratran/jev-pruner). All
three are MIT; the notices are in the crate's `THIRD_PARTY_NOTICES.md`.

## Using it

tau-luau-plugins runs it first of the compactions, before the tree and
tau-compaction, as tau comes with them. Context plugins are offered the
context in order, and the first that rewrites wins: when pruning
declines, the next one can take the turn. It needs a TypeSafe key: the
run's Jev. Without one it does nothing, and its status line says so.

An agent of its own adds it with `Active::built_in` in a
`LuauPlugins`, with a Jev, before tau-compaction. `with_archive` names
the archive directory; tau's is `archive` in the repository's data
directory, and the default is the temporary directory.

## Output pruning

The plugin's `after_tool_result` hook sees each tool result before the
model does, with whether the call failed, whether a tool made it, and
the transcript the model had seen when it made the call
([plugins.md](plugins.md)).

### Which outputs

- Only a **successful `exec_command` or `write_stdin` result** of text.
  A failed command comes back as an error, and errors are left as they are.
- Only a call the model made. A nested call (one a tool made with
  `ToolCtx::call`, such as a Codemode script's) goes back to that tool,
  never to the model, and is left as it is.
- The output is the result's text, which those tools already cut to
  the last 2,000 lines or 50 KB.
- Only an output estimated over `output_min_tokens` (10,000) is touched.
  Anything smaller passes untouched: no Jev call, no archive, no
  history read.
- An output holding a NUL byte or an image is left as it is.
- An output over 256 KiB is left as it is: the hook gets only its
  first 256 KiB, marked cut, and pruning needs it whole. tau's
  `exec_command` returns at most 50 KB, so this only stops a tool that
  returns more.

### What it does

1. **Chunks** the output: lines over 2,000 characters are split first,
   then the lines go in runs of `output_chunk_lines` (20), merged so there
   are at most 200 chunks. With two chunks or fewer, nothing could go,
   and the output is left as it is.
2. **Builds the state** Jev sees: what the state is (its `context`), the
   task (the `goal` setting, or the user's last three prompts), the
   **complete history** so far (the user's and the assistant's text,
   every tool call with its input, and every tool result verbatim), the
   command, and the chunks.
3. **Partitions the history** when it does not fit
   `output_max_state_tokens` (25,000) with the rest of the state: it is split in order into
   segments that fit, never truncated or left out. A field too large
   for a segment becomes continuations, each labeled with the field's
   name and the character offset it starts at. The history keeps at
   least half the room the rest of the state leaves.
4. **Asks** one yes/no question (Noul) per chunk: does any line in it
   need to remain available for the ongoing task. Yes: an error,
   warning, failure, summary or final result, or a value the task or
   history asks for. No: only routine progress, install logging, a
   repetitive listing or boilerplate nothing asks for. The guidance
   that applies to every chunk is said once, in the state's `context`:
   judge every line against instructions anywhere in the history; one
   needed line keeps its chunk; the full output is archived, but a line
   the task needs must not depend on the model looking there. The
   question itself stays short, so a request holds more of them.
   **Every chunk is asked about against every segment**, in states that
   fit `output_max_state_tokens` and requests that fit
   `output_max_request_tokens` (30,000), sent together (four at a
   time), at most `output_max_requests` (12) per output. The first and last chunks are never asked about, since they
   always stay, and neither is a chunk the allowance leaves short of a
   segment, since it stays too.
5. **Decides** each chunk. A chunk is **kept** when it is the first or
   the last; when any segment answered at or above
   `output_keep_threshold` (0.5); or when it was not answered against every segment. It goes
   when every segment answered under the threshold.
6. **Renders** a header line saying kept lines are verbatim and
   omissions are marked; the kept chunks, verbatim and in order; one
   `[N lines omitted]` for each run of dropped lines; and a footer
   naming the archive, `[full output: <path> (read or grep it if
needed)]`. The archive is a new
   file in the run's archive directory (`ctx.archive`) that only its
   owner can read.
7. **Replaces** the result only when the rendered text is estimated at
   least `output_min_reduction_ratio` (10%) smaller than the output.
   Otherwise the result stays, and no archive is written. A replaced
   result is not flagged as changed by a plugin: what it keeps is
   verbatim.

Whenever Jev was asked, the plugin adds an entry to `outputs` in its
state: the call, the output's lines, the chunks and how many stayed,
the lines dropped, the segments, the requests, the estimated tokens
before and after, whether the result was replaced, the archive, and
what Jev cost (`cost`, also charged to the run). The state is stored
with the run, so an interface shows it again when it reloads the run
from history. The plugin's view puts a pruned output on its call's card
(`calls[id].cut`): tau-ui shows it as two tabs, the terminal and the
text the model saw ([terminal.md](terminal.md)), with the lines kept and
the tokens before and after. The ledger page adds up what pruned
outputs saved.

### Safeguards

No rule guesses what a line means: no command categories, no patterns
for diagnostics or results, no document or secret detectors. Jev
decides, and the structure protects:

- the first and last chunks always stay;
- a keep in any segment keeps;
- a chunk not answered against every segment stays;
- any failure leaves the result as it was.

### Failure

A Jev error (after its client's retries), a missing or malformed
answer, a task and command that leave no room for the history, an
archive that cannot be written, or a cancel leaves the result untouched
and is written to the plugin's log. The run goes on.

### Cost

- Every Jev request is charged to the run, under tau-luau-plugins,
  including the answered ones of a pruning that then failed.
- Pruning at `after_tool_result` changes only the result being added, so the
  next request is still a delta
  ([openai-websocket.md](openai-websocket.md)): no resend.
- The requests for one output are sent together, so a pruned call takes
  about one Jev round trip longer.
- The requests grow with the output's size and its number of chunks:
  in the evaluation, 3 to 8 requests for outputs of 21,000 to 96,000
  estimated tokens, $0.0035 to $0.0076 each output. At those rates the
  allowance of 12 covers outputs of about 150,000 tokens, at most about
  $0.013; past it, the chunks left unasked stay, and the result is
  replaced only if it still saves `output_min_reduction_ratio`.

### Evaluation

`crates/evals/tau-output-pruning-eval` measures whether output pruning keeps the
lines a task needs. Each workload is a conversation that ends in one
long command output, generated from a seed with synthetic noise (cargo,
npm, pip, pytest, bundler and upload logs; no network), with **needles**:
lines the task needs, each in the output exactly once.

| Workload              | Needles                                                                                         |
| --------------------- | ----------------------------------------------------------------------------------------------- |
| `needle-error`        | one error line deep in a cargo build                                                            |
| `needle-detail`       | a bundle hash the prompt asks to remember, among hundreds like it                               |
| `summary-line`        | npm's totals line, with post-install noise after it                                             |
| `structured-json`     | one service's record in a large JSON registry                                                   |
| `all-noise`           | none: pruning should cut most of it                                                             |
| `cut-middle`          | a failed step above the 2,000-line tail `exec_command` keeps                                    |
| `multi-needle`        | three failing tests far apart                                                                   |
| `earlier-requirement` | a checksum only an earlier `read` of a large runbook asks for, so the history comes in segments |

The runner drives the real plugin, its Luau loaded as tau loads it,
through the agent loop, as the app does: a scripted model makes the
workload's calls, fake tools answer, and the fake `exec_command` cuts
its output to the tail like tau's (or, with `--whole`, returns every
output whole; one over 256 KiB is then left whole, see "Which
outputs"). It reads each output's stats from the plugin's state, and
measures layout and tokens with its own copy of the rules. Pruning sees only the text
the call returned, so a needle the cut removed is gone. Per trial it
records:

- **recall**: needles in the result the model saw, each as a whole line
  (a needle only in the archive does not count), and the same for what
  `exec_command` alone would have shown (**tail recall**);
- estimated tokens before (what `exec_command` alone shows) and after, and the
  **reduction**;
- the reduction against the whole output too (**of whole**);
- whether the result was replaced, Jev requests, input tokens, cost,
  latency, and Jev's answers by band: confident noise (at most 0.1),
  uncertain, and needed (0.5 or more: the chunk stays);
- in the JSON, each chunk's largest answer, its estimated tokens, and
  the chunks holding needles, to replay another keep rule offline on
  the same answers.

```sh
TYPESAFE_API_KEY=… cargo run -p tau-output-pruning-eval -- \
  --seeds 3 --budget-usd 0.5 --json results.json
```

`--workload NAME` picks workloads, `--list` lists them, and
`--budget-usd` stops once Jev's spend passes it. At $0.042 per million
input tokens, a workload costs about half a cent, so three seeds of all
eight cost about $0.13. Without `TYPESAFE_API_KEY` it refuses to run.
Its tests use fake Jevs and need no key.

Results against the real Jev, three seeds of each workload (24 outputs
of 21,000 to 96,000 estimated tokens, 3–8 requests each). They were
measured with `bash`, which spilled the whole output to a file for
pruning to read ([0038](../decisions/0038-commands-run-in-the-background.md)
removed that), under the old `spilled-middle` workload; they have not
been run again on `exec_command`:

| Workload              | Recall | Tail recall | Reduction | Of whole | Replaced | Cost    |
| --------------------- | ------ | ----------- | --------- | -------- | -------- | ------- |
| `needle-error`        | 3/3    | 2/3         | 95.8%     | 98.2%    | 3/3      | $0.0150 |
| `needle-detail`       | 3/3    | 1/3         | 93.1%     | 97.1%    | 3/3      | $0.0112 |
| `summary-line`        | 3/3    | 3/3         | 91.7%     | 98.1%    | 3/3      | $0.0210 |
| `structured-json`     | 12/12  | 4/12        | 96.6%     | 98.1%    | 3/3      | $0.0104 |
| `all-noise`           | —      | —           | 94.2%     | 98.8%    | 3/3      | $0.0160 |
| `spilled-middle`      | 3/3    | 0/3         | 92.4%     | 98.2%    | 3/3      | $0.0165 |
| `multi-needle`        | 9/9    | 3/9         | 92.1%     | 97.4%    | 3/3      | $0.0155 |
| `earlier-requirement` | 3/3    | 3/3         | 91.3%     | 91.3%    | 3/3      | $0.0198 |
| total                 | 36/36  | 16/36       | 93.4%     | 97.2%    | 24/24    | $0.1254 |

Every needle's chunk was answered 0.90 or more; noise mostly under
0.2, and 4 of 3,259 noise chunks at 0.5 or more (in `needle-detail`,
among hashes like the one asked for), which stay at little cost. About
$0.005 per pruned output.

Before these settings, the question told Jev that uncertain means
needed and every chunk above 0.1 stayed: Jev answered nearly every
noise chunk between 0.1 and 0.5, so nothing was replaced (0 of 24,
recall equal to tail recall). A more balanced question without the
floor, then the question's shared guidance moved into the `context`
(which halved the requests), gave the table above.

### Where it differs from jev-pruner

- **No heuristics.** jev-pruner leaves JSON, XML, YAML, diffs, and the
  output of `cat`, `jq`, `git diff` and the like untouched; keeps
  diagnostic and result lines and their neighbors by pattern; sorts
  commands into categories with guidance for Jev; tells Jev each
  chunk's information category; and shrinks oversized kept chunks line
  by line. tau leaves all of that to Jev and the structural safeguards;
  only the binary check remains.
- **Any failure keeps the output whole,** where jev-pruner retries with
  half the state on `max_tokens_exceeded`.
- **Nothing is asked that cannot change the outcome:** not the first
  and last chunks, and not the chunks the allowance leaves short of a
  segment.
- **A replacement must save `output_min_reduction_ratio`** of the whole
  output, and must not outgrow what the model would otherwise see;
  jev-pruner has no such floor.
- **No floor under the threshold.** jev-pruner also keeps any chunk
  answered above 0.1. Against the real Jev, noise answers sit between
  0.1 and 0.5 (see "Evaluation"), so that floor kept every chunk.
- **The question is short;** its shared guidance is in the state's
  `context`. jev-pruner repeats it in every question, which about
  doubles the requests an output takes.
- **The estimate is exact,** in tenths of a token (see "Token
  estimate").

## History pruning

### When a pass runs

- **Between turns,** when both hold:
  - the loop's token estimate has passed `compact_at_percent` (60%) of
    the context window; the window is the `window` setting when it is
    over 0, else the model registry's;
  - the context has grown by `cooldown_percent` (10%) of the window
    since the last pass that asked Jev.
- **As a run starts on a transcript it inherited** (a fork, a
  sub-agent, a resumed run), only when the estimate is within
  `start_reserve_tokens` (16,384, summarizing compaction's reserve) of
  the window. A fork's first request reads its parent's transcript from
  the parent's prompt cache, and any rewrite gives that up, so it only
  runs when the transcript would not fit anyway; then pruning goes
  before a summary would.
- **On a context overflow,** always.
- A pass with no unpinned tool call to ask about ends without asking,
  and does not start the cooldown.

### What a pass does

1. **Pins** the first message and the last `preserve_recent` (6; at
   least 1). A call is pinned when its call or its result is. Pinned
   calls are always kept.
2. **Builds the state** Jev sees: the goal (the `goal` setting, or the
   user's last three prompts), and the history, whole, with each tool
   call's name, input and a note of its result's size and status.
   **Never the result itself.**
3. **Partitions the history** when the whole of it does not fit
   `max_state_tokens` (25,000) with the goal, as output pruning does:
   in order, into segments that fit, never abridged; oversized fields
   become labeled continuations. Each segment's state then also lists
   the calls it asks about.
4. **Asks** two yes/no questions (Nouls) per unpinned call, against
   every segment, in batches that fit `max_request_tokens` (30,000) with
   the state, sent together (four at a time):
   - does knowing the call was made, with its input, still matter;
   - does its full output still need to stay verbatim.
5. **Decides** each call, at `keep_threshold` (0.5), on each answer's
   largest value over the segments: a keep in any segment keeps. A call
   not asked about against every segment, one too large to fit beside
   some, is kept.
   - result still needed: **keep**;
   - only the call still matters: **drop the result**, keeping its first
     `head_chars` (300) characters and a note saying how much was cut
     and naming an archive of the whole result, which the model can
     read instead of re-running the tool;
   - neither: **drop the call** and its result.
6. **Merges** the decisions into the run's ledger. A call's decision
   only escalates: keep, then drop the result, then drop the call.
7. **Applies** the ledger to the transcript: the calls it drops, and
   the results it cuts. tau drops an assistant message whose calls all
   went and which has no text left. Everything else stays as it was, in
   order.
8. **Rewrites** the context only if the pruned transcript is at least
   `min_reduction_ratio` (25%) smaller, measured as JSON
   (`ctx.measure`). Otherwise the pass declines, the ledger stays as it
   was, no archive is written, and the next context plugin gets the
   chance.
9. **Archives** each result it cuts, before the rewrite: the result's
   text, whole, in a new file in the run's archive directory that only
   its owner can read. The ledger keeps each archive's path.

### Cost

- Every Jev request is charged to the run, under tau-luau-plugins, at
  $0.042 per million input tokens, so it counts toward the run's limits
  and cost. A request that was answered is charged even when another
  failed.
- A partitioned history takes a request per segment and batch: more
  requests, each within the budget.
- A rewrite is one full resend of the transcript on the WebSocket; the
  requests after it are deltas again
  ([openai-websocket.md](openai-websocket.md)). The cooldown and the
  reduction threshold keep that rare.

### Failure

Any failure ends the pass without a rewrite and is written to the
plugin's log: a Jev error (after its client's retries), an answer
that is missing, of the wrong kind or out of range, a goal that leaves
no room for the history, an archive that cannot be written, or a
cancel. The run goes on with its transcript as it was. On an overflow
the failure is the hook's error, and the next context plugin is offered
the context.

### Storage and forks

A rewrite is stored as a `context` entry by `tau-luau-plugins`,
pointing at the messages it kept and followed by the ones it cut
([storage.md](storage.md)). The entry's details are the ledger, with
the archive of each cut result, and the pass's stats: calls, pinned,
kept, results and calls dropped, requests, the largest state's size and
`state_stage` (`whole`, or `N segments`), the characters before and
after, the reduction, and what Jev cost for the pass (`cost`).

The ledger and each pass are also in the plugin's state, which is
stored with the run and which a fork inherits, so a fork goes on from
its parent's ledger. Its view draws from that state, live or reloaded
from history:

- the status line, such as `3 pruned · −53%`;
- a note at each pass (`pass-N`), with what it saved and what Jev read;
- each call it pruned, marked on its card: `result dropped` or `call
dropped`, and the cut output's tabs;
- the inspector's count of kept calls, with a link to the ledger page;
- the ledger page: the context before and after the last pass, against
  the window, and each call's decision with Jev's two answers.

### Where it differs from pi

- **Once, not every turn.** pi re-applies its ledger to the context of
  every request. tau stores the pruned transcript once, because each
  rewrite costs a full resend.
- **Only a saving worth a resend.** pi applies any saving and leaves
  its reduction threshold to the choice between itself and summary
  compaction. tau declines below the threshold.
- **The cooldown is a share of the window.** pi waits a fixed 8,000
  tokens. Each rewrite costs a full resend, and on a large window a
  long run grows 8,000 tokens in a few turns, so pi's cooldown let it
  pay for a resend every few turns.
- **The cooldown starts only with a pass that asked Jev.** pi starts it
  with any pass, so a pass with everything pinned held the next useful
  one back.
- **A message holding only thinking stays** unless pruning emptied it;
  pi drops every such message.
- **The history is never abridged.** pi shrinks a history that does not
  fit: shorter inputs, abridged and collapsed texts, compacted calls,
  messages left out. tau partitions it, as jev-pruner does.
- **A cut result names its archive,** where pi tells the model to
  re-run the tool, which can be slow or have side effects.
- **No settings file or command.** Its settings are on the Plugins
  screen; its status line and notes report each pass.

## Token estimate

Both stages estimate tokens as jev-pruner does (`src/jev.ts`),
calibrated there against the usage Jev reports for real transcripts,
where it lands 2–18% above the true count: a word of ASCII letters is
one token per six letters, rounded up; a digit half a token; any other
character that is not a space nine tenths of a token, per UTF-16 unit.
A state or a set of questions (`estimate.state_tokens`) counts another
half token per digit. tau counts in exact tenths and rounds up once,
where the original sums floats: ten punctuation characters are 9
tokens here, 10 there. The output gate uses the plain estimate; every
budget a state or request must fit uses the state estimate.

## Settings

Its settings are flat, history pruning's (pi's defaults, above) and
output pruning's, the latter named `output_*`:

| Setting                      | Default |                                                               |
| ---------------------------- | ------- | ------------------------------------------------------------- |
| `goal`                       | empty   | the task, else the user's last three prompts                  |
| `keep_threshold`             | 0.5     | a Noul at or above keeps                                      |
| `preserve_recent`            | 6       | the newest messages pinned                                    |
| `max_state_tokens`           | 25,000  | per state                                                     |
| `max_request_tokens`         | 30,000  | per request                                                   |
| `head_chars`                 | 300     | what stays of a cut result                                    |
| `compact_at_percent`         | 60      | where a pass runs between turns                               |
| `min_reduction_ratio`        | 0.25    | of the transcript                                             |
| `cooldown_percent`           | 10      | of the window, between passes that asked Jev                  |
| `start_reserve_tokens`       | 16,384  | what a start keeps free                                       |
| `window`                     | 0       | for a model the registry does not know; 0 is the model's      |
| `output_enabled`             | `true`  |                                                               |
| `output_min_tokens`          | 10,000  | the gate                                                      |
| `output_chunk_lines`         | 20      | lines per chunk                                               |
| `output_keep_threshold`      | 0.5     | a Noul at or above keeps                                      |
| `output_max_state_tokens`    | 25,000  | per state                                                     |
| `output_max_request_tokens`  | 30,000  | per request                                                   |
| `output_max_requests`        | 12      | per output                                                    |
| `output_min_reduction_ratio` | 0.1     | of the whole output; nor larger than what the model would see |
