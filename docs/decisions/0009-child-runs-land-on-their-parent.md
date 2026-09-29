# 0009: Child runs land on their parent's stack, then close

- Status: proposed
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

When a child stops, its changes **land** on its parent's stack.

- **The parent waited (a sub-agent call): work in the stack.**
  1. At the call, the parent's `@` is committed.
  2. The child's workspace starts on that commit, and the child's
     turns commit on top of it.
  3. When the child returns, the parent's new `@` starts on the
     child's head.

  Nothing is rewritten and nothing can conflict: the parent was not
  changing anything.

- **The parent went on (a fork, or a sub-agent left running):
  restack.** The child's changes, from its start to its head, are
  rebased onto the parent's head with `set_parents` and
  `rebase_descendants`. Each change keeps its change id. The parent's
  `@` starts again on top.
- **Fold, on request.** The user may land a child as one change
  instead: a three-way merge of the fork point, the parent's head and
  the child's head, described with the child's summary. The child's
  own commits stay under its bookmark.
- **Several children** land one after another, in the order they
  finished. This includes several sub-agents called in one batch;
  the ones after the first restack onto the one before.
- **Conflicts land as jj conflicts.** The parent's next `vcs_status`
  shows them, and the parent's model edits the markers out like any
  other file. The landing card shows the conflicts before the user
  confirms.
- **Idle runs only.** The host rewrites only runs that are idle, lands
  only between turns, and updates each workspace it touched before
  that workspace's next tool call.
- **The landing is recorded** under the parent's `workspace` plugin
  records as `{ from, mode, changes }`. The transcript draws it as a
  card, forks of the parent inherit it, and the operation-log undo
  can name it.

### Closing

- A child closes once it has landed, or once it is dropped:
  - its workspace is forgotten;
  - its chat becomes read-only, under its parent in history;
  - its bookmark is removed, since its changes now live on the
    parent's stack.
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
- New jj-lib calls:
  - `set_parents`;
  - bookmark set and remove;
  - `record_abandoned_commit`;
  - a three-way tree merge for folding.
- Open questions:
  - whether a conflicting landing asks the user first or goes
    straight to the parent's model;
  - whether a folded child keeps its bookmark;
  - whether landing a grandchild offers to land its parent too.
