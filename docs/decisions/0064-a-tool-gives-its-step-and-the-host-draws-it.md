# 0064: A tool gives its step, and the host draws it

- Status: proposed. Follows
  [0060](0060-tool-calls-stand-on-a-spine.md) (the spine and its one
  language) and [0056](0056-luau-plugins-are-elm-over-coroutines.md)
  (drawings are plain functions of a frozen model).
- Date: 2026-11-19

## Context

ADR 0060 gave every call one line: a verb, a subject, a receipt, and a
body in one of five forms. Only the host knows that language. A plugin
does not say "this is the subject, this is the receipt"; it hands over
trees, and each one builds the line its own way.

A Luau tool is drawn in one of three ways:

- **`view.calls[call_id] = { head, label, body, folds }`** (skills, ask,
  canvas). The plugin keeps each call's data in its model so its view
  can find it again: skills writes `model.loaded[env.run.call]` in its
  tool and reads it back in `view`. `head` is a free tree (skills draws
  `ui.line({ ui.mono(name), ui.toned(description) })`) and `label` a
  string, so skills counts its own `N lines`.
- **`card = function(input, model, env)` on the tool's table** (todo).
  It gives a body only; the line is the host's guess.
- **Nothing** (memory). The host guesses the subject with
  `summarize_args` (`heiko-ui-plugin/src/run.rs`), a Rust function that
  looks for arguments named `cmd`, `pattern`, `path`, `query`, `task`,
  `id` or `title`. The receipt is the result's text. For memory this
  means:
  - `memory_link` (`from`, `to`, `type`) has no subject at all;
  - the receipts are prose ("Linked `a` relates `b`.");
  - no call has a body, so none opens;
  - a save shows twice: as a bare step, and in the note memory draws
    ("Saved 3 notes").

Rust plugins fill `CardView`: `head`, `label`, `failed`, `edge`,
`shape`, `body`, `folds`, `inset`, `kind`, `waits`. Each also keeps its
own open state (`open_key`, `opened`, `more` in heiko-tools) and draws
its own `show less` button. heiko-vcs's `shape` is a stamp, heiko-tools'
is a diffstat, and the receipt beside it says `+3 −1` again.

The five forms of ADR 0060 are `Node::Code`, `Node::Diff`,
`Node::Terminal` and tables among the rest of the tree. Luau reaches
only `ui.code`, through four calls in all the plugins.

## Decision

### One shape, `Step`, for Rust and Luau

A tool's plugin describes a call as a **step**. It says what only the
plugin knows. The host draws the rest.

| Field     | What it is                                                                |
| --------- | ------------------------------------------------------------------------- |
| `subject` | the one argument the call is about, as text; never JSON                   |
| `receipt` | a count and its noun, `+N −M`, or a stamp with a tone                     |
| `body`    | one of the five forms, or none                                            |
| `failed`  | why it failed, when the tool returned but the result says it did not work |
| `waits`   | it waits on the person (ADR 0060's stepping off)                          |
| `action`  | one button at the line's end (todo's undo)                                |
| `steps`   | nested calls, drawn as lettered steps (codemode)                          |

The receipt is a type, not a string:

- `Count { n, noun }`: `29 lines`, `5 notes`. The host writes the plural.
- `Diff { added, removed }`: `+3 −1`, or `new · +12`. The host also
  draws the diffstat, so no plugin gives a `shape`.
- `Stamp { text, tone }`: `exit 100`, `worked`, `relates`.

The body is one of five forms:

- `Code { lang, text }`: numbered lines;
- `Diff { text, path }`: added and removed lines;
- `Names { items }`: names in columns;
- `Output { lines }`: a terminal's lines, inset;
- `Record { pairs }`: keys and their values.

**A step that waits on the person** may give a free tree instead: ask's
questions and its hot action. That is the only body outside the five
forms.

### The host draws everything else

The host draws:

- the verb (the tool's name) in its kind's color;
- the square and its state;
- the number;
- the time past one second;
- folding, and the person's opens and closes, kept per run;
- `+ N more <noun>` when a body is cut short, from the form's own count;
- the red edge, from `failed`.

Plugins stop keeping open state and stop drawing `more` or `show less`.

### Rust: `CardView` becomes `Step`

`points::CARD` gives a `Step`. `head`, `label`, `edge`, `shape`,
`folds` and `inset` go:

- the subject replaces `head`;
- the receipt replaces `label` and `shape`;
- `failed` gives the edge;
- every body folds;
- `Output` is inset.

heiko-tools, heiko-vcs, heiko-mcp, heiko-codemode and
heiko-luau-plugins each move over. `summarize_args` goes too: a tool
that gives no step shows its verb alone.

### Luau: `step` on the tool's table

```lua
memory_search = {
	kind = "search",
	call = search_call,
	-- `call` is { args, result, details, error }, frozen; so is `model`.
	step = function(call, model, env)
		return {
			subject = call.args.query,
			receipt = ui.count(#call.details.hits, "note"),
			body = ui.names(titles(call.details.hits)),
		}
	end,
}
```

`step` is a plain function, like `view` and `card` (ADR 0056). It runs
while the call goes (with no `result` yet) and again after it ends. The
model is there for a step that depends on what happened after the call,
such as ask's reply. heiko-std gains the receipts (`ui.count`,
`ui.diffstat`, `ui.stamp`) and the forms (`ui.code`, `ui.diff`,
`ui.names`, `ui.output`, `ui.record`).

- **`card` goes.** Todo's `card` becomes its `step`.
- **`view.calls` keeps only what a plugin draws on another plugin's
  call:** a `badge` beside the line and a `body` under it (the
  constitution's verdicts, a flag). A plugin can no longer redraw the
  line of its own tool through `view.calls`.
- **A tool returns what its step needs in `details`.** Skills returns
  the skill's name, description and instructions there, and
  `model.loaded` goes.

### Memory's five steps

| Call             | Subject       | Receipt                                      | Body                                 |
| ---------------- | ------------- | -------------------------------------------- | ------------------------------------ |
| `memory_write`   | the title     | stamp: the type, `new` or `updated`          | `Code`: the note as markdown         |
| `memory_search`  | the query     | count: notes                                 | `Names`: each note's id and title    |
| `memory_read`    | the id        | count: lines                                 | `Code`: the note, with its links     |
| `memory_attempt` | the note's id | stamp: worked (good), failed (bad), untested | `Record`: what was tried, evidence   |
| `memory_link`    | `from → to`   | stamp: the link's type                       | none; `why` is in a `Record` if given |

The note "Saved N notes" stays for the memory pass after a run, whose
saves are not calls in it. The calls the run's own model makes are
steps only.

## Alternatives considered

- **Give memory a `view.calls` entry, as skills has.** It fixes memory
  today, but leaves three ways to draw a call. Memory would also build
  its line by hand and keep its calls in its model.
- **Luau only, with `CardView` left as it is.** Rust and Luau tools
  would share a look but not a contract. The Rust plugins' open state
  and `more` buttons would stay, and so would the repeated diffstat.
- **Keep `summarize_args`, and extend it with memory's argument
  names.** That is a guess written in Rust about a Luau package's
  arguments. It breaks as soon as the package renames one.
- **Free trees for subject and receipt.** These are what `head` and
  `label` are now. The lines drift apart again because nothing holds
  them to one language.

## Consequences

- Every tool's line is built the same way, so it can be tested without
  drawing: a step is data. A Luau test (`t.fixture`) can check a tool's
  step, and a Rust test can check `Step` values.
- Plugins lose some freedom on their own calls' lines: no icons, no
  extra words, no second color. A plugin that needs more draws a note
  (ADR 0057) or a badge.
- heiko-tools loses its open-state keys and `more`. Opens kept under
  the old keys are lost once, which is fine with no users.
- `ToolKind` and the receipt and body types live in `heiko-ui-plugin`,
  beside `Step`, where both kinds of plugin reach them.
- The phone gets steps over the wire as data, not trees, and draws
  them the same way.

## Steps

1. `Step`, `Receipt` and `StepBody` in heiko-ui-plugin. The host draws
   them: folding and opens kept per run, `+ N more`, the diffstat from
   a `Diff` receipt, and the edge from `failed`.
2. heiko-tools, heiko-vcs, heiko-mcp and heiko-codemode give steps. Their
   open state, `more` buttons and shapes go. `CardView` and
   `summarize_args` go.
3. Luau: `step` on a tool's table, the receipts and forms in heiko-std,
   and `view.calls` narrowed to `badge` and `body`. `card` goes.
4. Skills, todo, ask and canvas give steps. Skills drops
   `model.loaded`.
5. Memory gives its five steps, with `details` from each tool.
6. Screenshots of a run with memory, skills and todo calls, and of
   `docs/screenshots`' screens with calls.
