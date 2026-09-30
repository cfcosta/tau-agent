# 0014: The model commits, and every run lands as a stacked diff

- Status: accepted. Amends [0009](0009-child-runs-land-on-their-parent.md):
  its commit per turn, who resolves a landing's conflicts, and where a
  top-level run lands.
- Date: 2026-09-30

## Context

[0009](0009-child-runs-land-on-their-parent.md) made forks and
sub-agents one kind of run, a child, whose changes land on its
parent's stack by restacking. Three things in it do not hold up:

- **A commit per turn.** `RunWorkspace` commits whatever each turn
  changed, as `tau: run <id> turn <n>`. A turn is when the model
  stopped talking, not a unit of work: one change can take five turns,
  and one turn can make three unrelated changes. Landed children bring
  every turn along, so a parent with a few children gets a tall stack
  of commits no reviewer would have made.
- **Nobody runs the conflict fix.** A confirmed landing leaves jj
  conflicts in the parent for "its next turn" to resolve, but nothing
  starts that turn. The parent sits idle with conflicts in its stack
  until the person writes to it.
- **The tree stops short.** A top-level run leaves only as a GitHub
  pull request. Its changes never reach the repository's own `main`,
  so the work of a session and all its children has no way home that
  does not go through GitHub.

The idea behind the fix is the one stacked diffs rest on: a stack of
reviewable changes, each made on purpose, that moves up a tree until
it reaches trunk.

## Decision

### Runs form one tree, rooted at trunk

- A repository's **trunk** (its local `main`, as `Project::trunk()`
  finds it) is the root.
- A **session**, a top-level run, is a child of trunk.
- **Forks and sub-agents** are children of the run they came from, as
  in 0009.

Every child lands on its parent the same way, by restack (0009,
unchanged): its changes are rebased onto the parent's head, and each
keeps its change id.

### The model makes the commits

- **No commit per turn.** A run's working copy (`@`) carries its work
  across turns until the model commits it.
- **`vcs_commit` is how work becomes a change.** The model commits
  where a reviewer would want a boundary, with its own description.
  The tool exists already; its description says so, and the system
  prompt says that commits are how the run's work is seen and landed.
- **Every message is written by a model.** Commits carry what a
  reviewer reads, so tau never describes one itself: no
  `tau: run <id> turn <n>`, and no run's answer pasted in as a
  description.
- **For sure at the end.** A run that would stop with uncommitted
  changes is held once: tau-vcs keeps it going (as a plugin holds a
  stop now) with "Commit your work with `vcs_commit` before you
  finish." If it stops again with changes left, tau still commits
  them, so no work is left outside a change, but the message comes
  from a model: one short call with the diff and the run's task, on
  the run's own model, asked for a Conventional Commits message. The
  call's cost is the run's.
- **Delegating needs a clean working copy.** The sub-agent starts on
  the caller's newest commit and must see its work. `delegate`
  refuses while the caller's `@` holds changes and says to commit
  first, rather than committing for it.

### Turns keep snapshots, not commits

Forking at a turn, the compare view, and memory's stale notes need to
know the code at each turn. jj already records it: every snapshot of
the working copy is a commit, rewritten in place under the same change
id, and the old one stays in the operation log.

- At each `TurnEnd`, `RunWorkspace` snapshots `@` and stores a `Link`
  from the turn to that snapshot's **commit id**. Nothing new appears
  in the stack.
- A snapshot's commit id is what a link resolves to. Its change id is
  `@`'s, which moves on, so links to snapshots are not resolved by
  change id. Links to the model's commits still are (0009's rule).
- **Forking at a turn** starts the fork's workspace on a new change
  whose parent is the snapshot's parent and whose files are the
  snapshot's. The fork's own commits then stack on that.
- The operation log's snapshots must not be collected while a run
  links to them. tau never runs `jj util gc`; a later decision can
  add a collection that keeps linked commits.

### Landing

- **Sub-agents land by themselves**, when they return, as 0009 says:
  their caller waited, so nothing is rewritten and nothing conflicts.
- **Forks land when the person clicks Land.** Forks are often
  competing tries at one thing; landing all of them would be wrong.
- **Sessions merge into trunk when the person clicks Merge**, beside
  opening a pull request, which stays for repositories reviewed on
  GitHub.
- What lands is the child's commits. Uncommitted work in a finished
  child cannot exist (see "For sure at the end").

### Conflicts: the parent resolves, or the child for trunk

- **When the parent is a run**, a landing that conflicts asks first,
  as in 0009, and once confirmed, **tau starts the parent's next turn
  itself**: "Landing `<child title>` left conflicts in `a.rs` and
  `b.rs`. Resolve them, and commit the resolution." The parent knows
  its own stack, and resolving is part of taking the child's work.
  The landing card shows that turn running.
- **When the parent is trunk**, there is no model on that side. The
  session resolves instead: Merge first restacks the session onto
  trunk's head. If that conflicts, tau starts the session's next turn
  with the same message, and Merge finishes once that turn ends with
  no conflicts left. Then trunk's bookmark moves to the session's head,
  a fast-forward, and the session closes.
- A session merged into trunk is pushed only if the person pushes;
  merging is local.

## Alternatives considered

- **Squash each child into one change as it lands.** One reviewable
  diff per child, whatever it did inside. It hides a child's own
  commit boundaries, and once the model chooses them, those are the
  boundaries worth keeping. A reviewer can still ask for a squash.
- **Keep a commit per turn, and let the model describe them.** Turns
  would still be the unit, and the stack would still be as tall.
- **Always let the child resolve.** One rule everywhere, and the
  child knows what its changes meant. But landing is the parent taking
  work in, and the parent knows what else is on its stack; only trunk,
  which has no model, needs the child to do it.
- **Land every child when it finishes.** Simpler, and undoable from the
  operation log, but wrong for competing forks and too quiet for a
  merge into `main`.
- **Leave sessions to pull requests.** The tree would stop at the
  session, and local-only repositories would have no way to take a
  session's work.

## Consequences

- `RunWorkspace` stops committing at `TurnEnd`: it snapshots and links
  instead, and holds a stop with uncommitted changes once.
- `Link` gains a kind: a turn's snapshot, resolved by commit id, or a
  change (the model's commit, or one a landing brought), resolved by
  change id.
- Pull requests push the run's commits, not its turns. "Keep pushing"
  pushes after each commit rather than after each turn.
- `delegate` refuses with uncommitted changes, which costs the model a
  `vcs_commit` before delegating.
- The host starts a turn on its own: the conflict-resolving turn,
  after a landing or during a merge. It is the first turn tau starts
  that the person did not ask for.
- tau-ui grows a Merge action on sessions, and a way to show that a
  landing or merge waits on a resolving turn.
- A run whose model never commits still ends with one change, made at
  the end, with a message a model wrote from its diff.
- The reference (`docs/reference/vcs.md`, "Runs and turns", "Landing a
  child run", "Delegating to a sub-agent") describes the code as it
  is; it changes with the code.

## Order of work

1. Snapshot links at `TurnEnd` in place of commits; forks start from a
   snapshot. Pull requests push commits.
2. The held stop, and the commit at the end; `delegate`'s clean-copy
   rule; `vcs_commit`'s description and the system prompt.
3. The conflict-resolving turn after a confirmed landing.
4. Sessions as children of trunk: Merge, with the session resolving.

Sub-agents need nothing new: tau-ui's runs already `delegate`, and
their sub-agents already land as they return.
