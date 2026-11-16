# 0060: Tool calls stand on a spine, in one language

- Status: accepted, being implemented. Follows
  [0057](0057-the-interface-is-flat-and-printed.md) ("A tool call is a
  line") and [0045](0045-everything-is-a-plugin.md) (a tool's plugin
  draws its card).
- Date: 2026-11-16

## Context

ADR 0057 made a tool call a line, but each plugin still drew its line
its own way. On 2026-11-16 the demo showed:

- **Four ways to open a call:** `+ 25 more lines`, `+`, `code` and
  `show`.
- **Arguments shown three ways:** a path in a column, codemode's raw
  JSON (`{"path":"README.md"}`), and a skill's prose.
- **Results with no common unit:** `lines 1–29 of 29`, `10 ms`,
  `1 line`, a bare `4`, `3 keys`. Only codemode gave a time.
- **State only sometimes:** codemode ticked each call, while the
  built-in tools showed nothing until one failed.
- **Two looks for one thing:** a Luau plugin's tool (todo) drew as a
  note with a kanji, and a Rust tool drew as a line.
- **Colors that meant two things:** run and agents were both purple,
  and "other" and list were two cyans.

A busy turn was a long column of lines, and the model's words were hard
to find between them.

The choices were drawn on a canvas
(https://claude.ai/artifact/UW8PXr8ah19D4tEZxspdP5): a ledger of ruled
rows, calls as sentences, and calls as steps on a spine. The person
chose the spine and the parts that all three shared.

## Decision

### A stretch of calls is a list of numbered steps

The calls between two things that are not calls are one **stretch**:
the model's words, the person's message, or a plugin's note. What sits
between two of the model's calls, a turn's end or its reasoning, does
not end a stretch.

- **Steps.** Each call in a stretch is a numbered step. Numbering starts
  again after each of the person's messages.
- **The spine.** A step is a row on a thin vertical line, the spine:
  the step's square, then the verb, the subject and the receipt.
- **The head.** A stretch of more than one call opens with a head line.
  It holds a tick strip with one tick per step, in the step's kind's
  color (red for a failed or refused step, dimmed while one runs). Then
  come the steps' range (`steps 1–4`), what they did
  (`read · grep · edit ×2`), how many failed or are at work, and
  `fold`.
- **Folding.** A folded stretch is its head line alone, which reads
  `4 steps`. A stretch folds once something that is not a call follows
  it and none of its calls is still going, unless the person opened it;
  so the stretch at work stays open. The person's folds and opens are
  kept per run.
- **The square.** It says how the step stands:
  - quiet: done;
  - outlined in green: running;
  - outlined in blue: its body is open;
  - filled in red: failed or refused;
  - outlined in orange: it waits on the person.
- **Stepping off.** A refused step and one that waits on the person
  leave the spine. They are drawn full width: a refused step under a red
  rule, with what it proposed and why; a waiting one under an orange
  rule, with its question and the hot action. One that waits still
  shows when its stretch is folded.
- **Nested calls.** A call that makes calls (codemode) lists them under
  its row as lettered steps (a, b, c) in smaller squares.

### Six kinds

Every tool has a kind, and the kind gives its verb a color:

| Kind   | Color  | Tools                                         |
| ------ | ------ | --------------------------------------------- |
| look   | blue   | read, ls, image, artifact_read, vcs_show, skill |
| search | teal   | grep, find, memory recall                     |
| change | orange | edit, write, commit, todo, memory save        |
| run    | cyan   | exec_command, write_stdin, codemode           |
| reach  | yellow | http, MCP tools, github                       |
| people | purple | ask, spawning an agent, messaging one         |

Run moves from purple to cyan, so purple means only agents and people.
Yellow, a warm color, stays text only. A tool's plugin declares the
kind: a Rust plugin in its `CardView` (`kind`), and a Luau plugin in
its tool's table (`kind = "change"`). A tool with no kind is written in
the plain text color.

### One line

- **Verb:** the tool's name.
- **Subject:** the one argument that names what the call is about: a
  path, a pattern, a command, `server · tool`, a title. It is never
  JSON.
- **Receipt:** a count and its noun (`29 lines`, `5 matches · 2 files`,
  `+3 −1`, `3 entries`), or a stamp (`exit 100`). A time shows only past
  one second.
- **Opening:** clicking anywhere on the line opens or closes it; there
  is no `+` or `−` at its end. A body cut short ends with
  `+ N more <noun>`, always in that form.

### Five bodies on one band

An open call's body sits on the flat band, in one of five forms:

- code, with its lines numbered;
- a diff, with its added and removed lines;
- names, in columns;
- output, as a terminal's lines;
- a record, as keys and their values.

There is no label on the band: its language, size or kind is in the
line's receipt.

### Plugin tools draw as lines

A Luau plugin's tool draws as a step like any other. Todo's calls are
steps (`todo_file`, the issue's title, its id and an undo). Plugin
*notes* (ADR 0057), which are not tool calls, keep their kanji.

## Alternatives considered

- **A ledger:** every call a ruled row in four columns, with no folding.
  It is the simplest to build, but twelve calls stay twelve rows.
- **Sentences:** every call a past-tense sentence ("Read retry.rs · 29
  lines"), and a stretch folded into a sentence or two. It reads well,
  but every tool, the Luau ones too, would have to write its own
  sentences, and one path is harder to find by scanning.
- **Leave each plugin's line its own.** That is what we had, and the
  list in Context is the result.

## Consequences

- A long turn folds to one line per stretch once it ends, and the
  model's words lead.
- `CardView` gains `kind` and `waits`, and a Luau tool's table gains
  `kind`. The kinds move into `heiko-ui-plugin`, where both Rust and
  Luau plugins reach them.
- Codemode's card stops showing raw JSON and per-call milliseconds.
- Todo's tool calls stop drawing a note with 件.
- Old screenshots show the old lines until they are taken again.

## Steps

1. The kinds: `ToolKind`, the theme's colors, `CardView::kind`, the Luau
   tool's `kind`, and run in cyan.
2. The spine: stretches, the head line and its fold, the numbered
   squares and stepping off.
3. One line: subjects instead of JSON, receipts in counts, the whole
   line opens, and `+ N more <noun>`.
4. Codemode's nested calls as lettered steps.
5. Todo's calls as steps.
6. Screenshots.
