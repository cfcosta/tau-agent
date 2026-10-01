# Version-control tools (`tau-vcs`, optional)

- Status: implemented in `crates/plugins/vcs`, on `jj-lib` 0.45.1.
- Design study: [jj-lib.md](../research/jj-lib.md). Decisions:
  [0009](../decisions/0009-child-runs-land-on-their-parent.md) (child
  runs land on their parent) and
  [0014](../decisions/0014-the-model-commits-and-runs-land-as-stacked-diffs.md)
  (the model commits; runs land as stacked diffs).

The model reaches version control only through these tools. It never
runs `jj` or `git` in a shell. The tools work on a jj repository, so
there is no staging area, and file edits made with `write`, `edit` or
`bash` are recorded when the next vcs tool snapshots the working copy.

## Setup

```rust
use tau_vcs::{Identity, Vcs, VcsPlugin};

// An existing jj workspace:
let vcs = Vcs::open("/path/to/workspace", Identity::default())?;
// Or a new repository with an internal Git store (`jj git init`):
let vcs = Vcs::init("/path/to/new", Identity::default())?;

let agent = Agent::new(llm).plugin(VcsPlugin::new(vcs));
```

- `Identity` is the author and committer of the commits and operations
  the tools write. The default is `tau <tau@localhost>`.
- `Vcs::open` and `Vcs::init` block while the workspace loads. Call them
  when you set up the agent, not inside a tool.
- `Vcs::init` makes a non-colocated repository: the Git store is in
  `.jj/repo/store/git`, and there is no `.git` beside `.jj`.
- `VcsPlugin::new(vcs)` adds all nine tools. `read_only()` keeps only
  `vcs_status`, `vcs_diff`, `vcs_log` and `vcs_show`.
- The plugin keeps no state per run. Every run of the agent works in
  the one workspace that `vcs` opened.
- To use the coding tools on the same files, root `CodingTools` at the
  workspace directory.

## Threading

- jj-lib's futures are not `Send`. Much of their work is blocking file
  and object I/O. So each `Vcs` has one thread that owns the workspace
  and runs every job in order. The thread drives jj-lib's futures with
  `pollster`.
- A tool future only sends a job and awaits the reply on a oneshot
  channel. It never holds jj-lib types across an await.
- A panic in jj-lib is caught on the thread. The tool returns
  `jj-lib panicked: <message>` as its error, and the thread loads the
  workspace again before the next job.
- Clones of a `Vcs` share the thread. The thread stops when the last
  clone is dropped.
- All tools are `ExecutionMode::Sequential`. A batch of calls runs in
  order, so `vcs_commit` and then `vcs_log` in one turn see each other.

## Scoping rules

- **Ids only.** Arguments that name a change take a change id or a
  commit id, or a unique prefix of either:
  - change ids use jj's letters `k` to `z`;
  - commit ids use hex digits.
  - Revsets, `@`, bookmark names and tags are refused. A model cannot
    write a query that is arbitrarily expensive.
- **Snapshot first.** Every tool snapshots the working copy before it
  reads or writes. The snapshot is its own operation, marked as a
  snapshot, and only happens when files changed. A call refused after
  its snapshot keeps it.
- **The working copy only.** The write tools change only the
  working-copy change (`@`) of this workspace, and they only add new
  changes on top of it.
- **Immutable commits are refused.** A commit is immutable if it is the
  root commit, or an ancestor of a tag or of a remote bookmark (all
  remotes except the internal `git` one). A write tool fails with
  `The working-copy commit <id> is immutable` when `@` is one of them.
- **Tagged operations.** Each write is one operation, described as
  `tau vcs: <tool>`. It carries the workspace name and the operation
  attribute `tau.vcs.tool = <tool>`. `vcs_undo` uses these tags.
- **Everything is tracked but large new files.** The snapshot tracks
  every new file, so there is no staging and no list of untracked
  files. The one exception is a new file larger than 1 MiB
  (`MAX_NEW_FILE_SIZE`, jj's default): it stays out of `@`, and
  `vcs_status` names it under `Left out of @` with its size.
- **Stale working copies.** When an operation in another workspace
  rewrote this workspace's commit, as the main chat's catch-up with
  trunk does to the commits its chats stand on, the next tool or turn
  first moves the files to the rewritten commit, as jj's
  `workspace update-stale` does, with what was edited on disk since the
  last snapshot merged on top, as a rebase would: an edit to a file the
  rewrite changed too becomes a conflict. When a concurrent operation
  forked the operation log instead, the tools fail with
  `The working copy is stale: ...` and ask for the user to update the
  workspace.

## Results

Every tool returns compact text for the model and a JSON `details`
value for callers. The model never sees `details`.

A change appears in text as one line:

```
<change id, 12 letters> <commit id, 12 digits> [@] [(empty)] [(conflict)] [(divergent)] [(immutable)] [[bookmarks]] <first line of the description>
```

An empty description shows as `(no description set)`. Bookmarks are
the commit's local bookmarks, such as `[main]`, in brackets and
separated by commas. In `details`,
a change is a `ChangeInfo`:

| Field          | Meaning                                     |
| -------------- | ------------------------------------------- |
| `change_id`    | full change id                              |
| `commit_id`    | full commit id                              |
| `description`  | full description, ending in a newline       |
| `empty`        | the change touches no files                 |
| `conflict`     | the change has unresolved conflicts         |
| `immutable`    | see "Scoping rules"                         |
| `working_copy` | the change is this workspace's working copy |
| `divergent`    | other visible commits share its change id   |
| `bookmarks`    | its local bookmarks, sorted                 |

A changed path is a `FileChange`:
`{ "path": "src/lib.rs", "kind": "added" | "modified" | "removed" }`.

## Read tools

### vcs_status: `{}`

- Text: the working-copy line, one `Parent (@-):` line for each parent,
  then either `The working copy has no changes.` or
  `Working copy changes:` with one `A`, `M` or `D` line for each path.
  Then conflicted paths, and new files too large to snapshot, if there
  are any.
- Details: `working_copy`, `parents`, `changes`, `conflicts` (paths),
  `too_large` (`{ "path", "size" }`, the size in bytes), `diff` (`@`'s
  diff as `vcs_diff` gives it, which the text leaves out) and
  `truncated`.
- Conflicts are data. jj keeps them in commits, and the files hold
  conflict markers that the model edits like any other text.

### vcs_diff: `{ change?, paths? }`

- `change` defaults to the working copy. The diff is against the
  change's parent, or against the merge of its parents.
- `paths` limits the diff to those files and directories. Each path is
  relative to the workspace root. An absolute path must be inside the
  workspace.
- Text: Git-style unified diff with three lines of context. It includes
  `new file mode`, `deleted file mode`, mode changes, and
  `Binary files ... differ`. Conflicted files show jj's conflict
  markers.
- A change with no diff returns `No changes in <change line>.`
- The output is cut to 50 KiB (`MAX_DIFF_BYTES`) at a line boundary,
  with a note to pass `paths`.
- Details: `change`, `files`, `diff` (the diff as in the text, without
  the note), `truncated`.

### vcs_log: `{ limit? }`

- Lists the working copy and its ancestors, newest first, without the
  root commit. There is one change line for each row.
- `limit` defaults to 10 and is clamped to 1..=100. When there are more
  changes, the text ends with
  `[Showing the newest N changes. Use limit=M for more]`.
- Details: `changes`, `more`.

### vcs_show: `{ change }`

- Text: `Change ID`, `Commit ID`, `Author`, one `Parent:` line for each
  parent, then `Flags:` when any flag is set, then the full description
  indented by four spaces, then the diff, as for `vcs_diff`.
- Details: `change`, `parents`, `author` (`name`, `email`), `files`,
  `diff`, `truncated`.

## Write tools

### vcs_describe: `{ message }`

- Replaces the working-copy change's description. The change id stays
  the same.
- Trailing whitespace is trimmed and one newline is added, as jj
  stores descriptions. An empty message clears the description.

### vcs_commit: `{ message }`

- Does what `jj commit -m` does: describes the working-copy change, and
  then starts a new empty change on top of it as the new `@`. Files do
  not change.
- An empty message is refused: `The description must not be empty`.
- Details: `committed` and `working_copy`.
- Nothing is committed for the model (ADR 0014): its commits are how a
  run's work is reviewed, landed and pushed, so the description asks
  for a Conventional Commits message at each boundary a reviewer would
  want, and everything committed before the run finishes.

### vcs_land: `{}`

- Only with `VcsPlugin::landing()`: for chats that land on their
  repository's main chat when they finish. Sub-agents land as they
  return and do not get it.
- Proposes landing the run's commits. It moves nothing: it refuses
  while `@` holds changes (`Your working copy has uncommitted changes
(…). Commit your work with vcs_commit first.`), and otherwise returns
  `{ "proposed": true, "head": <@'s parent> }` for the host, which
  shows the person what would land and waits for them to confirm.

### vcs_new: `{ message? }`

- Starts a new empty change on top of the working copy, with an
  optional description. The old change keeps its description. Files do
  not change.

### vcs_restore: `{ paths, from? }`

- Makes `paths` in the working copy match `from`. By default `from` is
  the working copy's parent. Files that `from` does not have are
  deleted. Other paths are not changed.
- At least one path is required. `.` restores everything.
- The files on disk are updated. Details: `restored` (the paths that
  changed) and `working_copy`.

### vcs_undo: `{}`

- Undoes the newest operation that these tools made in this workspace,
  in the way that `jj undo` does. It merges the parent's view over the
  current one, so file edits made since that operation are kept.
- Snapshots are skipped, and so are operations that an earlier
  `vcs_undo` already undid. Calling it again undoes the operation
  before that one.
- It refuses when the newest operation that is not a snapshot was not
  made by these tools in this workspace. For example, the user's own
  `jj` commands, another workspace, or the initial repository setup.
  So a run never undoes work that it did not do.
- The undo records `tau.vcs.undo = <operation id>` on its own
  operation.
- Details: `operation` (the id that was undone), `tool`,
  `working_copy`.

## ls

The plugin also marks what `@` changes in each result of the coding
tools' `ls` (`tools.md`, "ls"). After an `ls` of a directory inside the
workspace, it snapshots the working copy and gives each entry that
differs from `@`'s parents a `change` in the result's details: a file's
`added` or `modified`, and `modified` for a directory with any change
under it. The model's text stays as `ls` wrote it. When the snapshot
fails, or the directory is outside the workspace, the listing goes on
unmarked.

## Error strings

In these messages, `<rev>` and `<path>` stand for the argument as the
model gave it, and the real message puts it in backquotes.

| Tool          | Condition                          | Message                                                                                         |
| ------------- | ---------------------------------- | ----------------------------------------------------------------------------------------------- |
| all           | cancelled before starting          | `Operation aborted`                                                                             |
| all           | jj-lib panicked                    | `jj-lib panicked: <message>`                                                                    |
| all           | stale working copy                 | `The working copy is stale: another process changed this workspace's commit. ...`               |
| ids           | not an id                          | `<rev> is not a change id or a commit id. ... revsets are not accepted.`                        |
| ids           | unknown change or commit           | `No change matches <rev>`, `No commit matches <rev>`                                            |
| ids           | ambiguous prefix                   | `Change id prefix <rev> is ambiguous; give more of it` (or `Commit id prefix`)                  |
| ids           | divergent change                   | `Change <rev> is divergent; pass a commit id instead`                                           |
| ids           | abandoned change                   | `Change <rev> is hidden (abandoned)`                                                            |
| paths         | absolute, outside the workspace    | `<path> is outside the repository`                                                              |
| paths         | `..` or not a valid path           | `<path> is not a path inside the repository`                                                    |
| write tools   | `@` is immutable                   | `The working-copy commit <id> is immutable`                                                     |
| `vcs_commit`  | empty message                      | `The description must not be empty`                                                             |
| `vcs_restore` | no paths                           | `Name at least one path to restore ("." restores everything)`                                   |
| `vcs_undo`    | newest operation is not the tools' | `The last operation was not made by the vcs tools in this workspace ("<description>"); ...`     |
| `vcs_undo`    | concurrent operations              | `The operation log has concurrent operations here; ask the user to undo from the operation log` |

## Projects

A `Project` is a repository tau owns, in a directory of its own
(the host uses `$XDG_DATA_HOME/tau/repos/<name>/`). Runs never work in
the user's checkout.

| Path           | What it is                                                             |
| -------------- | ---------------------------------------------------------------------- |
| `git/`         | A bare copy of the source's Git store; jj's Git store.                 |
| `main/`        | The jj repository (`main/.jj/repo`). Its own working copy stays empty. |
| `runs/<name>/` | One jj workspace per run, named by the host.                           |

- `Project::open_or_import(source, root, identity)` opens the project at
  `root`, or makes it from the local repository at `source` (a checkout,
  a linked worktree or a bare repository) and imports every branch as a
  bookmark. No `git` is needed: the object files are hard-linked (they
  never change once written), and the refs, `HEAD`, config and
  `shallow` (where a shallow clone's history starts) copied. Cloning
  from a URL is not supported yet. jj-lib's fetch and push run
  `git` as a subprocess, so the GitHub side will need it, or a fetch
  through gix and a push of our own.
- `clone_bare(url, token, into)` clones a remote into a bare
  repository with gix, keeping every branch and tag under its own
  name, as `git clone --bare` does; `open_or_import` takes that clone
  as its source.
- `update(from)` brings in what changed at the source since the import
  or the last update: from the checkout (`UpdateFrom::Checkout`) or
  from a remote (`UpdateFrom::Remote`, a fetch through gix). The
  copy's branches and tags become the source's: those the source
  deleted go. A remote's or a bare repository's `HEAD` comes too, so
  trunk follows a new default branch. A checkout's does not: it names
  the branch checked out there, so trunk stays on the branch the
  import's `HEAD` named. It returns trunk before and after (`Updated`). Runs keep their
  workspaces and commits.
- `trunk()` is the commit new runs start from: the branch the copy's
  `HEAD` names, else `main`, `master` or `trunk`, else the root commit.
- `add_workspace(name, base)` makes `runs/<name>` on a new empty commit
  on top of `base` and checks out its files. `forget_workspace(name)`
  drops it from the view and deletes the directory; its commits stay.
- These calls block. Call them off the async executor.

## Runs and turns

`RunWorkspace` is a plugin for one run: build one per run, point the
run's coding tools at `RunWorkspace::dir()`, and give `VcsPlugin` its
`vcs()` (it loads on first use, after the run has made it).

- **At start** it makes the run's workspace: from the snapshot of the
  turn a fork inherits (see below), on the commit a link names (a
  landed change), on `with_base` (a sub-agent), else on `trunk()`.
- **The model makes the commits**
  ([ADR 0014](../decisions/0014-the-model-commits-and-runs-land-as-stacked-diffs.md)).
  A turn commits nothing.
- **After each turn** (`TurnEnd`) it snapshots `@` and points the run's
  local bookmark, `tau/<run id>`, at the run's newest commit (`@`'s
  parent), so the run's work stays findable by name after its
  workspace is gone. Then it stores a `Link` record under the plugin
  name `workspace`:

  ```json
  {
    "turn": 2,
    "workspace": "0192…",
    "commit_id": "…",
    "change_id": "…",
    "changed": true,
    "from": null,
    "snapshot": true
  }
  ```

  `commit_id` is the snapshot: `@` as the turn left it. Later snapshots
  rewrite `@` under the same change id, so a snapshot link is found by
  its commit id, and `Project::current` leaves it as it is. `changed`
  says whether the turn changed files since the turn before, or, for a
  run's first turn, since the run started. Links a
  landing stores (`from` set, `snapshot` false) name changes, and
  `Project::current` moves them to where their change is now. A failed
  snapshot stores `{ "turn": n, "error": "…" }` and the run goes on.
  Observers given with `on_turn` hear each `TurnSnapshot`, with the
  paths the turn changed.

- **Before it stops** with changes in `@`, the run is held once, with
  `COMMIT_FIRST` and the paths as the next user message.
- **When it finishes** normally or at a limit with changes still in
  `@`, they are committed (`Vcs::commit_all`), with a message the run's
  model writes from the diff and the run's task (its own input, not the
  first message of a transcript it forked): one short `PluginCtx::ask`.
  tau never writes a commit message itself; without an answer, the work
  stays uncommitted. A failed or cancelled run keeps its work as it is.
- **Forking at a turn**: read the run's links with
  `Store::plugin_entries(run, "workspace")`, take the `seq` of the
  turn's link, and fork with `Checkpoint::at(run, seq)` and a new
  `RunWorkspace`. The fork inherits the transcript and links up to that
  turn, and `Project::add_workspace_from_snapshot` starts it on a new
  change holding the snapshot's files, uncommitted, on the snapshot's
  parent as it is now: if a landing restacked that parent since, its
  new files and the turn's work are merged. A fork never stands on
  another workspace's working copy: when the parent's change is one now
  (the run undid that commit with `vcs_undo`), the fork starts on that
  parent's parent, with the turn's files merged as above, and the undone
  commit's description is not the fork's.
- `Project::stack(head)` lists a run's commits, oldest first: what
  `head` has that trunk lacks. Pull requests push those, one GitHub
  commit per commit, with the model's message.

## Landing a child run

A child run (a fork, or a sub-agent) lands on its parent by restacking
([ADR 0009](../decisions/0009-child-runs-land-on-their-parent.md)).
`Vcs::land(child_head, bookmark, confirm)` runs on the parent's `Vcs`:

- The child's changes are what its head has that the parent's newest
  commit lacks. Their root is rebased onto that commit, the rest
  follow, and each keeps its change id.
- The parent's working copy starts again on the child's new head, and
  the parent's bookmark moves there, in the same operation (tagged
  `land`), so the parent's files follow.
- A child that already sits on the parent's head (the parent waited on
  it) is not rewritten.
- The parent's uncommitted work stays uncommitted: its working copy
  moves onto the landed changes. Landing is between the parent's turns.
- The host's `Host::land` records each landed change as a `Link` in
  the parent, at the parent's latest turn, with `from` naming the
  child, so forks, the compare view and pull requests read them as the
  parent's own. Then it closes the child: its workspace is forgotten
  and its bookmark removed. Both runs must be idle.
- `Host::drop_child` closes a child without landing it: its own
  changes (what its head has that its parent's lacks) are abandoned
  with `Project::abandon_between`, and its workspace and bookmark go.
  The operation log keeps what was abandoned.
- A child cannot land or be dropped while it has children still open:
  running, or holding changes it does not have. They land or are
  dropped first, one level at a time.
- With `confirm` off, nothing changes. The `Landing` it returns says
  what would happen: the changes as they would be (`changes`, newest
  first), the paths that would hold conflict markers in the new head
  (`conflicts`), and the new head. Confirmed, conflicts land as jj
  conflicts for the parent's next turn to resolve.

## Moving onto trunk

`Vcs::move_onto(trunk, bookmark, confirm)`, on a run's `Vcs`, rebases
the run's changes, up to `@`, onto trunk's newest commit, and points
`bookmark` at the run's newest commit there. The run's changes are
what `@` has that neither trunk nor the commit the workspace last moved
onto has. The workspace records that commit (in `.jj/tau-moved-onto`)
after each move, unless it was one of the run's own commits. So
upstream's commits under the run never count as the run's: once
upstream drops them (a reset, an amend, a force-push), they stay
dropped. Each change keeps its
change id, and the run's files follow. With `confirm` off it changes
nothing and returns what it would do, as a `Landing`; its `conflicts`
include any in `@`. The main chat catches up with trunk this way (see
below).

## A repository's main chat

Each repository has a main chat, a top-level run that every other chat
forks from. It works in the repository's own checkout, jj's `default`
workspace under `main/` (`DEFAULT_WORKSPACE`), never in one of its own,
and `forget_workspace` never removes it. It commits on trunk: its
`RunWorkspace` is built with `commits_to(project.trunk_name())`, so its
commits, its turns'
snapshots, the chats that land on it and its sub-agents all move
trunk's bookmark, not `tau/<run>`. It has nothing to land: it does
not get `vcs_land`, and its bar offers no landing. It is the only
top-level run (ADR 0016), so nothing merges into trunk: a chat lands on
the main chat, which moves trunk.

Trunk can move without it, when an update brings commits from GitHub.
When the main chat has moved trunk too, `Project::update` takes
upstream's trunk. So it does for any bookmark of the source's that it
leaves with two targets: one the main chat moved, then upstream moved,
renamed or deleted, goes where upstream has it, or goes. Before each of its turns, and before a chat lands on
it, the host moves its workspace onto trunk's head (`Vcs::move_onto`):
its own commits go on top, keeping their change ids, and so does its
work in `@`; commits upstream dropped do not come back. Its next commit moves trunk forward rather
than aside.

## Delegating to a sub-agent

`Delegate` is the `delegate` tool (`{ task, model?, effort? }`): a run
hands a task to a sub-agent, a child run in a chat of its own that
forks the caller's conversation
([ADR 0009](../decisions/0009-child-runs-land-on-their-parent.md),
[ADR 0015](../decisions/0015-delegates-fork-their-caller.md)). Build it
on the run's `RunWorkspace`, with the model ids a call may pick from,
and a closure that builds the sub-agent's `Agent` around the
sub-agent's own `RunWorkspace` on the `ChildModel` its call asked for.
tau-ui builds it on the model and effort asked for, or the caller's,
and refuses an effort the model does not take.

1. The caller's work must be committed (ADR 0014): with changes in its
   `@`, the tool refuses and says to commit with `vcs_commit` first.
   The sub-agent's workspace, `<caller's workspace>-sub-<12 random hex
digits>`, so that no process reuses one an earlier one left, starts
   on the caller's newest commit
   (`RunWorkspace::with_base`), so it sees the caller's work.
2. The sub-agent runs through `Agent::as_tool(…).forking()`: a
   `Subagent` run of the caller with a `fork_seq`, its events forwarded
   to the caller's. It is asked on the caller's transcript, the turn
   that made the call with an output for each call in it, and then its
   task. It commits its work on its own stack, under its own bookmark.
3. The tool is `ExecutionMode::Grouped`: the calls in one batch run side
   by side, up to `MAX_RUNNING` (4) at once, and the batch's other tools
   run before or after them, never while a landing moves the caller's
   working copy.
4. As each finishes, its changes land on the caller with `Vcs::land`,
   one landing at a time, in the order they finish. The first cannot
   conflict. A later one whose changes clash with an earlier one's
   lands its conflicts, and its result names the files this landing
   left in conflict, not those the caller's head held already, for the
   caller to resolve. The tool's text is the sub-agent's answer and a
   line on what landed; its details hold `run`, the `landing`, the
   `conflicts` it brought, and the `limit` that cut it short, if any.
5. A sub-agent stopped by a limit committed what it left at its end, as
   any run at a limit does, and lands like one that finished: its text
   is its last message, and the line on what landed starts by saying
   which limit stopped it (`It stopped at its turn limit.`).
6. When it fails, or the caller is cancelled before it lands, its
   changes are abandoned and the caller gets the error.
7. Either way the sub-agent closes: its workspace is forgotten and its
   bookmark removed.

The caller's links record what came to its stack during the turn: each
landed change with `from` naming the sub-agent, then the turn's
snapshot. Only a top-level run (a repository's main chat) gets
`delegate`: runs nest one level
([ADR 0016](../decisions/0016-runs-nest-one-level.md)).

## Left to the host and the UI

These operations change shared state, use the network, or throw work
away. The model does not get them. The host calls jj-lib (or later
`tau-vcs` APIs) for them when the user asks
([jj-lib.md](../research/jj-lib.md), "Operations the user triggers from
the UI"):

- **Fetch and push.** Updating trunk and pushing a run's bookmark.
  These need credentials.
- **Pull requests.** Opening a PR after a push.
- **Bookmarks, rebase, abandon and squash.** The study's
  `vcs_abandon`, `vcs_squash`, `vcs_cat` and named revisions (`trunk`,
  `fork-point`) are not built.
- **Operation log.** Browsing it, restoring an arbitrary operation, and
  updating a workspace a concurrent operation left stale.
- **Guarding the shell.** A `before_tool` hook that blocks `git` and
  `jj` in `bash` is not built.
