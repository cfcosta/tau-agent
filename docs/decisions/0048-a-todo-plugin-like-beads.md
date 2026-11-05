# 0048: Issues runs can pick up: a todo plugin like beads

- Status: accepted. Design canvas:
  <https://claude.ai/artifact/4XyXyNm5R3ZE99EjFAwZvf> (the board, a run
  working an issue, an epic's graph, and two phone screens).
- Date: 2026-11-04

## Context

A run's work outlives its turn, but what is left to do does not. When a
run finds a second bug while fixing the first, it can say so in prose,
start a sub-agent, or forget it. The person keeps the list in their
head, in a notes file, or in GitHub issues that no run reads.

beads (`bd`, and its Rust port beads_rust, `br`) answers this for
coding agents with a small issue tracker built for them: issues with a
priority and a type, links between them, and one question the agent
asks first, "what is ready?", meaning open and blocked by nothing.
Agents claim an issue, work it, file what they find on the way, and
close it with a reason. Its storage is SQLite with a JSONL export kept
in the repository.

tau has what beads lacks around it: runs with a lifecycle, a store,
phones, and plugins with a UI (0045, 0046). It lacks the list.

## Decision

### A plugin, `tau-todo`, with beads' model

An **issue** has:

- an id: `td-` and four or more hex digits of a hash, longer on a
  collision. A child of an epic is `td-2a00.1`;
- a title, a description in Markdown, and "done when" criteria: a list
  of lines, each checked or not;
- a type: `bug`, `feature`, `task`, `epic` or `chore`;
- a priority, `P0` (drop everything) to `P4` (some day), `P2` by
  default;
- a status: `open`, `in_progress`, `blocked`, `deferred` (with an
  optional date it comes back) or `closed` (with a reason);
- labels, a claim (below), comments (append only), and when it was
  made, changed and closed.

**Links** are typed, as in beads:

- `blocks`: the only link that decides readiness. A cycle of `blocks`
  is refused when it is made, with the path that closes it.
- `parent`: an epic and its children. An epic shows its children's
  progress, and can close when they all have.
- `found_while`: the issue a run was working when it filed this one.
  It records where work came from and does not block.
- `related`: shown, nothing else.

An issue is **ready** when it is open, not deferred (or past its date),
and every issue that blocks it is closed. Ready issues sort by
priority, then age. `blocked` is not stored as a status for an open
issue with an open blocker: it is computed, so it is never stale. The
stored `blocked` status is for an issue blocked by something outside
tau, and says what.

### Two kinds of task: issues and steps

- **Issues** are the project's: per repository, kept until closed,
  shared by the person and every chat, worked by chats. Everything
  above is about them.
- **Steps** are the model's own: a run's checklist of how it means to
  do what it is doing, set with `todo_steps` like Codex's `update_plan`. A
  step is a line and
  a status, `pending`, `in_progress` or `done`, with at most one in
  progress. No priority, no links, no store: steps are the run's
  records (`PluginCtx::publish`), folded along its fork chain like the
  canvas's items (0040), so a fork goes on with its parent's steps.

They meet in three places:

- A run that claims an issue plans its own steps. The issue's "done
  when" criteria say what finished means; the steps say how. Steps do
  not start from the criteria.
- A step that turns out bigger than the run becomes an issue: the model
  calls `todo_create`, or the person clicks "Make an issue" on the step.
  The step shows the new id and is done; the issue is `found_while` the
  run's issue, if it has one.
- When a run's claim ends, its steps not done go into the comment that
  ends it, so whoever takes the issue next starts from them.

A run with no issue still has steps: a quick chat gets the model's
checklist without touching the board.

### Runs have beads' whole tool set

The plugin gives every run these tools (`tools`, per workspace):

| Tool            | Does                                                                              |
| --------------- | --------------------------------------------------------------------------------- |
| `todo_ready`    | Ready issues, filtered by label, type or priority; with `--blocked`, what waits on what |
| `todo_list`     | Issues by status, label, type, priority, claim; as a tree under epics            |
| `todo_show`     | One issue with its links, criteria and comments                                   |
| `todo_search`   | Full text over titles, descriptions and comments                                  |
| `todo_create`   | A new issue; `found_while` is set to the run's claim when it has one             |
| `todo_update`   | Title, description, type, priority, labels, status, criteria                      |
| `todo_claim`    | Claim an issue for this run; refused when another live run holds it               |
| `todo_check`    | Check or uncheck a criterion                                                      |
| `todo_link`     | Add or remove a link; refuses a `blocks` cycle                                    |
| `todo_comment`  | Add a comment                                                                     |
| `todo_close`    | Close with a reason; reopen                                                       |
| `todo_defer`    | Defer, optionally until a date                                                    |
| `todo_dispatch` | Ask the person to start a chat on an issue, or send it to a chat; an Allow card   |
| `todo_steps`    | Replace the run's steps with a new list (the model's own; see above)              |

`todo_dispatch` is how a chat hands work to another: it proposes, the
person allows. The call shows an Allow card in the run, and on phones,
with the issue and where it would go (a new chat, or a named one);
allowing starts the chat with the issue claimed, denying returns the
reason to the model. It is the one issue action a run cannot take
alone.

A run may close, reopen and reprioritize as freely as the person can,
as with beads. Closing with unchecked criteria is allowed, but the
reason must say why, and the board shows the issue as closed with open
criteria. The person sees every change as a tool card and can undo it
from the board.

The plugin adds a short paragraph to a run's prompt (`prompt`): that
the repository keeps issues, to ask `todo_ready` before choosing work,
to claim what it works on, to file what it finds rather than drift, and
to close with a reason. It names kinds of work, never a language or a
tool of one (`tests/shipped.rs`).

### A claim belongs to a run

beads leaves a stale claim for a person to find. tau knows when a run
ends, so a claim is the run's, not a name:

- `todo_claim` sets the issue `in_progress` and records the run.
- When the run finishes, is cancelled, or is dropped, its claims end:
  the issue goes back to `open` with a comment saying what the run did
  (its last message, and whether it landed). A run that lands keeps
  the issue `in_progress` only if it did not close it, so the person
  sees work that landed without closing what it was for.
- A fork inherits nothing: the parent keeps its claim. A sub-agent may
  claim on its own.
- A claim by a run whose process died is ended at start, with the
  crash recovery that already ends its run (0026, `Host::recover`).

### Storage: the plugin's own SQLite, replicated, never in the repository

Issues live in a SQLite database the plugin owns, one per repository,
under tau's data directory, as `tau-constitution-host` keeps its own.
They are not in the repository: every run works in its own jj
workspace, so a file there would be edited by parallel runs and
conflict when they land. Tools write through the host half, one write
at a time, so parallel runs never conflict and a claim is atomic.

The interface gets issues as the plugin's repository data
(`repo_data`), which is part of the state the computer sends phones
(0046): a phone draws the board from the same data and the same tree.
A person's action on a phone (close, comment, start a run) is the
plugin's own action (`act`), carried to the host like any other.

A JSONL export in beads' format, `todo_export`, writes the issues to a
file the person names, for git or for beads' own tools. An import
reads beads' JSONL, so a repository that used beads keeps its issues.

### What the person sees: the board and the chat

Two modes, over the same data.

**The board** is a page per repository, reached from a Todo entry under
it in the sidebar (`points::SIDEBAR_REPO`, a `NavEntry` with the number
ready). Its header switches between three views and holds filter
chips (`label:phone`, `p0`, a type, a run):

- **Board**, the default: a kanban with four columns, Ready, In
  progress, Blocked and Done (closed today; older ones by filter).
  Deferred issues show only through a filter. A card shows its
  priority, id, type and title, and below them its labels, its run (in
  progress), what it waits on (blocked) or the commit it landed in
  (done), and its criteria checked out of all.
- **List**: the same issues as grouped rows, with a side panel for the
  selected issue: its run, description, criteria, links and actions.
- **Graph**: an epic's issues with the `graph` widget, blockers to the
  left of what they block, `found_while` dashed, the critical path to
  the epic's last open issue drawn apart.

Moving a card is an action, not only a status:

- Ready to In progress starts a chat on the issue, with the issue
  claimed and its text as the first message. "Send to chat…" on a card
  gives it to a chat that exists instead, steering it; `@td-4e1b` in a
  composer does the same from the chat's side.
- To Done asks for a close reason.
- To Blocked asks what it waits on: an issue (a `blocks` link) or a
  line of text (the stored `blocked` status).
- In progress back to Ready ends the run's claim and leaves the run
  going.

**The chat**: a run that holds an issue shows it in a band above the
composer (`points::COMPOSER_BAND`), so it stays in view while the
person types and steers. The band shows the issue, its criteria
checked out of all, the "done when" list, the issues the run found,
and the next ready ones.
Under it, the run's steps, the one in progress marked, a promoted
step showing its new issue's id. A run with no issue shows only its
steps. The band folds to one line ("Next: …, 1/3"), and the fold is
the person's, kept in the interface's state.

In the transcript, the tools that change an issue draw small inline
lines rather than full cards: "criterion 1 met", "filed td-a3f9",
"P1 to P0", each with an undo. Tools that read (`todo_ready`,
`todo_show`, `todo_list`) draw ordinary cards (`points::CARD`). The
run's row in the sidebar notes its issue's id (`points::RUN_ROW`).

**On a phone** the board is a list with the columns as tabs (working
now, then ready, then blocked), each card with "Start a run". An issue
opens full screen, with criteria to tick and a field that comments or
steers the run that holds it. In a run, the band is folded above the
composer and opens the issue when tapped.

## Built in Luau, after five seams

tau-todo is a Luau plugin. It is the plugin that makes Luau a full
tier (0045's "What Luau gains"): everything it needs that Luau lacks
is a seam 0045 already promises, built first, each with its own Rust
test, and used by tau-todo as its first caller.

What Luau has today and tau-todo uses as it is: tools with schemas and
cards, `run_end` (a claim ends with its run), per-run state kept as the
run's records (a run's steps), view trees with `graph`, `tabs`,
`input`, `modal`, `section` and `table`, actions, settings, and Luau
tests. The issue graph, readiness, cycles, ids, transitions and the
beads format are plain Luau.

The five seams, in the order they land:

1. **Repository data and a store.** `data` / `repo_data` for Luau, and
   a host-kept store per plugin and repository with transactions, so
   `todo_claim` reads and writes in one step. Luau sees the store as
   calls (`ctx.store`), never a file or SQL.
2. **Points and pages.** A Luau plugin contributes to
   `SIDEBAR_REPO`, `COMPOSER_BAND` and `RUN_ROW`, and has pages of its
   own reached by `ui.link`.
3. **Starting and steering chats.** A host call that starts a chat in a
   repository with a first message and a claim recorded by the plugin,
   and one that steers a chat that exists. Both go through the Allow
   card when a run asks (`todo_dispatch`), and not when the person's
   own click asks.
4. **A start hook.** A Luau hook the host calls once at start, after
   its own recovery, with the runs that died, so the plugin ends their
   claims.
5. **The `board` widget.** Columns of cards, a drag between columns an
   action with the card and the two columns, drawn by the kernel and
   reached from Luau as `ui.board`, as `ui.graph` is.

Export and import of beads' JSONL are host actions the person starts,
with a file they pick; the plugin never names a path.

A Luau `view` keeps its 200 ms limit: the store answers the board's
queries already sorted and filtered, and the view only draws.

## Testing: models first, one per layer

tau-todo is tested the way the November rounds tested tau's stateful
parts (docs/plan-test-quality.md): a hegel state machine per layer,
each checked against a small model after every step, with fault probes
proving each model can fail and a nightly twin running it long. The
models are written before the code they test, from this ADR: each
statement above that can be false is an invariant below, and the
table at the end maps them.

Every model shares one **reference model**, `todo::model` in the
plugin's test support: plain Rust over `BTreeMap`s, a few hundred
lines, no SQLite, no async. It holds issues, links, claims, steps and
runs, and answers every query the slow, obvious way: ready is a filter
over every issue and every link, a cycle is found by walking every
path. It is the oracle for the layers above it, and it is kept small
enough to read in one sitting.

### 1. The issue graph: laws over pure functions

`crates/plugins/tau-todo/tests/graph_laws.rs`, property tests over the
plugin's pure Luau core (the graph module: no store, no clock), called
through the Luau host from Rust, so hegel draws the inputs.

- **Ready.** For any graph, `ready()` equals the reference model's
  filter, in the same order (priority, then age, then id).
- **Cycles.** Adding a `blocks` link is refused exactly when the
  reference model finds a path back, and the refusal names a real path.
  Links of the other kinds never are.
- **Computed blocked.** An open issue is shown blocked exactly when an
  open issue blocks it; closing the last blocker makes it ready in the
  same call.
- **Epics.** An epic's progress counts exactly its children; it can
  close when they all have, and not before.
- **Ids.** Ids are unique across any sequence of creates, including
  forced hash collisions (a test hasher); a child's id is its parent's
  plus the next number; an id never comes back after a delete.
- **Transitions.** Every status change the tools allow is in the table
  of allowed transitions, and every other one is refused with a
  message naming the status it had.
- **Export.** `import(export(g)) == g` for every graph, with every
  field; the export is byte-stable for the same graph (sorted, so it
  diffs well in git).

### 2. The store: a state machine, then a crash anywhere

`tests/store_model.rs` drives the real SQLite store: create, update,
link, unlink, claim, unclaim, check, comment, close, reopen, defer
(with an injected clock moved forward and back), delete, export and
import, from one to three writers at once. After every step the store
reads back exactly the reference model.

`tests/crash_model.rs` does what the host's crash model does
(`crates/tau-ui/tests/crash_model.rs`): every write goes through a
counter that lets a drawn number through and fails the rest, as if tau
died there. The test reopens the store from the same file, then
checks:

- the store equals the reference model as of some prefix of the
  writes it was given, never a mix: no half-made issue, no link to a
  missing issue, no claim without its status;
- a second reopen changes nothing;
- `every_write_is_a_place_to_close` walks every write of every tool in
  turn, so no crash point depends on a draw.

### 3. Claims and runs: the plugin in the real host

`crates/tau-ui/tests/todo_model.rs` runs the real host with the model
the sync and crash models use, which answers only when the test says
so, so runs stay live as long as the test wants. The fake model's
turns call the `todo_*` tools.

- **Rules:** chats start on issues (from the board, from "Send to
  chat", from `@td-`); runs claim, call every tool with valid and
  invalid arguments, set steps, promote a step, finish, are cancelled,
  crash, land and are dropped; a run forks; a sub-agent claims; the
  host restarts.
- **Invariants:**
  - every claim is held by a live run, and an issue has at most one;
  - when a claim ends, the issue is open again with a comment holding
    the run's steps not done, unless the run closed it;
  - a fork inherits its parent's steps and none of its claims;
  - a promoted step is done, names its issue, and the issue is
    `found_while` the run's issue;
  - steps have at most one in progress, and `todo_steps` replaces the
    list whole: the folded steps equal the last call's;
  - a tool given bad arguments returns an error the model can read
    and changes nothing;
  - after a restart, the claims of runs that died are ended, as at a
    normal end.

**Claims are linearizable.** A rule fires one to three `todo_claim`
calls for the same issue from different runs at once. The test records
each call's start, end and answer, and checks the history against the
reference model run sequentially (a Wing and Gong check; histories are
short enough to try every order): some order of the calls, consistent
with their timing, explains every answer. Exactly one wins.

### 4. Dispatch: one Allow, two devices

The same model adds `todo_dispatch`. The person's answer can come from
the computer, a phone, or both at once.

- An allowed dispatch starts exactly one chat, with the issue claimed,
  however many devices clicked Allow and in whatever order.
- A denied one starts nothing and returns the reason to the model,
  once.
- A dispatch whose run ended before the answer is withdrawn on every
  device, and a late Allow starts nothing.
- A dispatch for an issue another run has since claimed is refused
  when allowed, with that run named.

### 5. The board and the band on every device

`crates/tau-ui/tests/todo_sync_model.rs`, built on the sync model's
harness (the real feed and encoding, in process, phones dropped and
brought back by the test):

- **Rules:** drag cards between columns on the computer and on one or
  two phones, including the same card on two devices at once; choose
  the board, the list or the graph; filter; fold and open the band;
  tick a criterion; click "Make an issue"; restart the computer.
- **Invariants:**
  - every move is the action it stands for: to In progress starts a
    chat with the issue claimed, to Done closes with the reason given,
    to Blocked links or records what it waits on, back to Ready ends
    the claim and leaves the run going;
  - the same card dragged on two devices ends in one column on every
    device, the one the stated rule picks (the first the computer
    takes), and the other device's move is shown as refused, not lost;
  - once everything arrives, every phone's board, band and fold equal
    the computer's;
  - each card shows what the reference model says: its column, run,
    blocker, landed commit and criteria count.

The kanban widget gets its own layout property in the design test:
cards never overlap or leave their column, and a long title wraps
inside its card. The graph view's: no two nodes overlap, every edge
joins its two nodes, and every blocker is left of what it blocks.

### 6. Oracles

- **The reference model**, for every layer above.
- **beads_rust.** Where `br` is installed, a property exports a drawn
  graph, imports it into `br` in a temporary directory, and compares
  `br ready --json` and `br blocked --json` with ours. It is skipped,
  not failed, where `br` is missing, and runs in the nightly set.
- **Tool schemas.** A test runs every `todo_*` schema through
  `tau_agent::schema::to_strict` and the oracle in
  `crates/tau-agent/tests/schema.rs` (every object closed and fully
  required), and validates drawn arguments with the `jsonschema`
  crate, so a provider never refuses a run for a schema this plugin
  added.

### 7. Fault probes

Before the plugin lands, each fault below is put in by hand, must fail
at least one model, and is listed with its shrunk case in the commit
that adds the model:

| Fault                                                     | Expected to fail             |
| --------------------------------------------------------- | ---------------------------- |
| ready keeps an issue whose blocker closed                 | graph laws                   |
| the cycle check skips paths longer than two               | graph laws                   |
| export drops `defer_until` or a comment                   | graph laws (round trip)      |
| an issue's row and its links written in two transactions  | crash model                  |
| a claim not ended when its run is cancelled               | claims model                 |
| a fork inherits its parent's claim                        | claims model                 |
| steps not folded along the fork chain                     | claims model                 |
| `todo_claim` reads, then writes, outside one transaction  | linearizability check        |
| Allow from two devices starts two chats                   | dispatch                     |
| a drag to In progress only changes the status             | board model                  |
| the band's fold kept in a gpui entity, not the state      | board model (phone differs)  |
| ready sorted by age before priority                       | graph laws, and beads oracle |

A probe that survives means the model has a hole: the rule or the
invariant that should have caught it is added before the probe counts.

### 8. Budget

Each model's default run stays under 30 seconds, as the others do; each
has a nightly twin (`#[ignore = "nightly"]`, profile `nightly` or
`nightly_slow`). Failures are shrunk by hegel and replayed from
`.hegel` first. The clock (deferral dates, claim ages) and id hashing
are injected, so every run is deterministic for its seed.

### What each statement is checked by

| Statement in this ADR                                  | Checked by                         |
| ------------------------------------------------------ | ---------------------------------- |
| ready: open, not deferred, no open blocker; sorted     | graph laws; beads oracle           |
| a `blocks` cycle is refused, with its path             | graph laws                         |
| blocked is computed, never stale                       | graph laws                         |
| an epic closes when its children have                  | graph laws                         |
| issues survive any crash, never half-written           | crash model                        |
| a claim belongs to one live run and ends with it       | claims model; linearizability      |
| a fork inherits steps, not claims                      | claims model                       |
| steps: one in progress, replaced whole                 | claims model                       |
| a promoted step becomes a `found_while` issue          | claims model                       |
| a dispatch is allowed once, by the person              | dispatch                           |
| a card's move is the action it stands for              | board model                        |
| phones show the board and the band as the computer     | board model                        |
| the export is beads' format, and round-trips           | graph laws; beads oracle           |

## As built

What landed differs from the text above where the seams or the tests
showed a better way, or where the kernel had no piece for it:

- **One key an issue.** The store keeps each issue, its links with it,
  under one key (`issue/<id>`), and a deleted id under `retired/<id>`:
  a change is one write, and a hook's writes go in one transaction
  when it ends (seam 1). The crash model's fault "an issue's row and
  its links written in two transactions" became "each write of a hook
  committed alone", which fails both crash tests.
- **A hook never waits on a chat.** Starting or steering a chat (seam
  3) is what an action answers, `tau.start_chat` or `tau.steer_chat`;
  the host carries it out once the action ended and its store is
  written, then calls the action it names next with the chat's run. A
  tool asks the person with `tau.allow`: the host shows the Allow card
  and waits outside any hook, so no store is held while a chat starts
  whose own hooks may want it. Starting a chat on an issue holds it as
  `starting` until the chat's run takes it; `recover` lets go of that
  too.
- **`recover` covers every repository with a store**, not only those
  listed as tau starts: tau lists them after its plugins resume.
- **`run_end` runs however a run ended**: the run's cancel no longer
  cuts it off, so a stopped run's claims end too.
- **Landing.** A claim ends when its run ends, landed or not: the run
  ends before tau lands it, so "keeps it in progress if it landed
  without closing it" is not built.
- **The board.** The kernel's `board` widget shows its columns as tabs
  when too narrow for them, as on a phone, where each ready card has
  "Start a run" and the issue's panel the rest. To Done and to Blocked
  ask in a modal whose question the store keeps, so every device shows
  it. A move names the column it was dragged from, and is refused when
  the card moved elsewhere first. The Graph view's widget has no dashed
  edges: `found_while` is listed on the issue rather than drawn, and
  the critical way is in the accent.
- **beads' JSONL.** No Luau plugin names a path, and there is no file
  picker a plugin reaches: the board's beads view shows the export to
  copy, and takes pasted JSONL to import. "Done when" criteria are
  beads' `acceptance_criteria` as a Markdown checklist, and a stored
  blocked status its `notes`.
- **Modules.** A Luau plugin's `lib/` modules may require each other,
  out of a cycle; tau-todo's are `graph`, `issues`, `beads`, `time` and
  `steps`.
- **Tests.** Every layer above landed: the graph laws, the store and
  crash models, the claims and dispatch model in the real host
  (`crates/tau-ui/tests/todo_model.rs`), the board sync model
  (`todo_sync_model.rs`), the schema checks, and the beads_rust oracle,
  skipped where `br` is missing (it was, where this was built). Forks
  are not in the claims model: a claim is keyed by its run's id, which
  a fork does not share, and steps follow the fork chain as every
  plugin's records do. Each fault probe is listed, with what failed it,
  in the commit that added its model; one survived as the same as the
  plugin (a claim refused for an issue another run holds is refused
  anyway, as that issue is in progress).

## Alternatives considered

- **Issues in the repository, like beads.** They would follow forks
  and branches, but parallel runs would merge or conflict on landing,
  and a phone would need the repository to read them.
- **GitHub issues through `tau-github`.** Not every repository has a
  forge, and agents need answers in milliseconds, offline, with
  readiness computed from links GitHub does not have. An export to
  GitHub can come later as its own tool.
- **Runs that can only claim, file and check.** Safer, but every close
  would wait on the person, and beads' agents work without that.

## Consequences

- Runs can keep and share a plan longer than a turn, and the person can
  steer it from a phone.
- A run's claim ending with the run means no stale claims, at the cost
  of reopening an issue the run half did; the comment says what it did.
- The plugin needs no new point: it uses `SIDEBAR_REPO`, a page,
  `COMPOSER_BAND`, `CARD`, `RUN_ROW` and the `graph` widget. It needs
  one new widget, for the kanban: `board`, columns of cards with drag
  between them as an action, joins the kernel widgets of 0045.
- Issues are per repository; an issue across repositories is two
  issues with a `related` link.
