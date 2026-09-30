# 0009: Child runs land on their parent's stack, then close

- Status: accepted. Amended by
  [0014](0014-the-model-commits-and-runs-land-as-stacked-diffs.md):
  the model makes the commits (no commit per turn), tau starts the
  parent's turn that resolves a landing's conflicts, and sessions
  merge into trunk.
- Date: 2026-09-29
- Options, with commit graphs:
  [Landing Child Runs](https://claude.ai/artifact/XhxBri4hxYF34fAprH98hC);
  the jj groundwork is in [jj-lib.md](../research/jj-lib.md)

## Context

A run can have children in two ways today, and neither child's work
ever comes back to its parent:

- **Forks.** The user forks a turn. The fork gets its own jj workspace
  on the commit that turn's `Link` names, and its own stack of turn
  commits (`tau: run <id> turn <n>`). Those commits have no bookmark
  and stay in the project. "Keep branch" only forgets the other
  workspaces. A fork's pull request pushes its own turns and never
  touches the parent.
- **Sub-agents.** `Agent::as_tool` runs an agent as a tool call. The
  caller waits and gets the child's last text back. The child
  inherits no transcript and no records. tau-ui never builds one, so
  a sub-agent has no workspace. Given a `RunWorkspace`, it would start
  on trunk and not see its caller's edits.

For the user, the two are the same thing. A child is a run with its
own chat that works on a copy of its parent's code. When it is done,
its changes should join the parent's history as stacked commits, and
the child should close. jj suits this: changes keep their ids when
rewritten, conflicts are data inside commits, and every step is one
operation that can be undone.

Nothing needed for this exists yet:

- no bookmarks;
- no rebase, squash or abandon;
- `Link`s point at commit ids, which change when commits are
  rewritten.

## Decision

### One kind of child run

Forks and sub-agents become one kind of run, a **child run**. They
differ only in who starts one and whether the parent waits:

|           | Started by                | Parent waits | Starts with                    |
| --------- | ------------------------- | ------------ | ------------------------------ |
| Fork      | the user, at a turn       | no           | the transcript up to that turn |
| Sub-agent | the model, as a tool call | yes          | its task                       |

Every child run has:

- its own chat, listed under its parent;
- its own jj workspace, on the parent's work at the point it started:
  that turn's commit for a fork, the parent's `@` for a sub-agent;
- a bookmark, `tau/<run>`, moved to its head at every turn commit.

A child run can be opened, steered and forked like any run.

### Landing

When a child stops, its changes **land** on its parent's stack, by
one move: **restack**.

- The child's changes, from where it started to its head, are rebased
  onto the parent's head with `set_parents` and `rebase_descendants`.
  Each change keeps its change id, so every turn of the child stays a
  change of its own on the parent's stack. The parent's `@` starts
  again on top.
- A child the parent waited on (a sub-agent call) is the easy case.
  At the call, the parent's `@` is committed and the child's
  workspace starts on that commit. The parent has not moved when the
  child returns, so the restack rewrites nothing and cannot conflict:
  the parent's new `@` simply starts on the child's head.
- **One level at a time.** A child lands in its direct parent only.
  Landing that parent in turn is a separate choice, from its own
  card, so each step can be reviewed.
- **Several children** land one after another, in the order they
  finished. This includes several sub-agents called in one batch;
  the ones after the first restack onto the one before.
- **A landing that would conflict asks first.** The landing card
  lists the files that would conflict before anything changes. The
  user confirms or leaves the child open. A confirmed
  landing puts the conflicts in as jj conflicts: the parent's next
  `vcs_status` shows them, and its model edits the markers out like
  any other file. A child the parent waited on cannot conflict, so it
  never asks.
- **Idle runs only.** The host rewrites only runs that are idle, lands
  only between turns, and updates each workspace it touched before
  that workspace's next tool call.
- **The landing is recorded** under the parent's `workspace` plugin
  records as `{ from, changes }`. The transcript draws it as a
  card, forks of the parent inherit it, and the operation-log undo
  can name it.

### Closing

- A child closes once it has landed, or once it is dropped:
  - its workspace is forgotten;
  - its chat becomes read-only, under its parent in history;
  - its bookmark is removed: its changes now live on the parent's
    stack, under the same change ids.
- A dropped child's changes are abandoned.
- A child that has open children of its own cannot close until they
  have landed or been dropped.

### Groundwork, first

1. `tau/<run>` bookmarks, set in `RunWorkspace`.
2. `Link`s resolved by change id. A change id that has diverged is
   reported, not guessed at.
3. The landing record, and the host's rule of rewriting only idle
   runs.
4. Children of the host's runs: sub-agent workspaces started from the
   caller's `@`, and a child's chat in the sidebar.

## Alternatives considered

- **Fold: squash the child into one change.** A three-way merge of
  the fork point, the parent's head and the child's head, described
  with the child's summary. It keeps the parent's stack short, but
  hides the child's turns from it, and it is a second way to land
  that the user would have to choose between. Restack keeps each turn
  reviewable, and a child's turns can be squashed later if needed.
- **Work in the stack as a rule of its own.** A child the parent waits
  on builds on the parent's committed work. That is what restack
  already does when the parent has not moved, so it needs no separate
  rule.
- **Join: a merge change whose parents are both heads.** It rewrites
  nothing, lets a child keep running, and lands parallel children in
  one step. It is left out because merge commits break the linear
  stacks that the PR push (one commit per turn, chained) and
  `vcs_log` (first parent only) assume.
- **Keep children apart and open a pull request per child.** This is
  what forks do today. The parent never sees its children's work, and
  a sub-agent's changes are useless to the run that asked for them.
- **Share the caller's workspace with a sub-agent.** The jj study's
  simpler idea. Two runs would then snapshot one working copy, and
  the sub-agent's turns would mix into the caller's commits.

## Consequences

- A sub-agent finally has a workspace, and it sees its caller's
  unsaved edits.
- The parent's history stays a linear stack of changes, so the
  pull-request push stays as it is. A PR from the root includes every
  landed child's changes.
- Restacking rewrites the child's commits. Forks must be idle to
  land, and anything that stores commit ids (links, compare views)
  resolves them by change id.
- A parent's turn that calls a sub-agent splits into two commits
  around the call.
- The sidebar grows a tree of child chats. Closing keeps it from
  growing without bound.
- A deep tree of children lands one review at a time, level by level.
- Every turn of every landed child is a change on its parent's
  stack, so a parent with many children has a tall stack.
- New jj-lib calls:
  - `set_parents`, for the restack (jj merges the trees as it
    rebases);
  - bookmark set and remove;
  - `record_abandoned_commit`, for dropped children.
