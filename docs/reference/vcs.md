# Version-control tools (`tau-vcs`, optional)

- Status: implemented in `crates/plugins/tau-vcs-host`, on `jj-lib` 0.46.0,
  with its cards in `crates/plugins/tau-vcs`.
- Design study: [jj-lib.md](../research/jj-lib.md). Decisions:
  [0009](../decisions/0009-child-runs-land-on-their-parent.md) (child
  runs land on their parent),
  [0014](../decisions/0014-the-model-commits-and-runs-land-as-stacked-diffs.md)
  (the model commits; runs land as stacked diffs) and
  [0023](../decisions/0023-main-pushes-with-git-chat-prs-replay-onto-origin.md)
  (main pushes with git; chat pull requests replay onto origin).

The model reaches version control only through these tools. It never
runs `jj` or `git` in a shell. The tools work on a jj repository, so
there is no staging area, and file edits made with `write`, `edit` or
`exec_command` are recorded when the next vcs tool snapshots the working copy.

## Setup

```rust
use tau_vcs_host::{Identity, Vcs, VcsPlugin};

// An existing jj workspace:
let vcs = Vcs::open("/path/to/workspace", Identity::default()).await?;
// Or a new repository with an internal Git store (`jj git init`):
let vcs = Vcs::init("/path/to/new", Identity::default()).await?;

let agent = Agent::new(llm).plugin(VcsPlugin::new(vcs));
```

- `Identity` says who the commits and operations the tools write are
  by, and where the jj config they follow is (see "Identity and jj
  config"). The default is `tau <tau@localhost>` and reads no config.
- `Vcs::open` and `Vcs::init` are async: they load the workspace on
  tokio's blocking pool, so they need a runtime. Call them when you set
  up the agent, not inside a tool.
- `Vcs::init` makes a non-colocated repository: the Git store is in
  `.jj/repo/store/git`, and there is no `.git` beside `.jj`.
- `VcsPlugin::new(vcs)` adds all ten tools. `read_only()` keeps only
  `vcs_status`, `vcs_diff`, `vcs_log` and `vcs_show`.
- The plugin keeps no state per run. Every run of the agent works in
  the one workspace that `vcs` opened.
- To use the coding tools on the same files, root `CodingTools` at the
  workspace directory.

## Identity and jj config

tau follows the person's jj config exactly as the jj CLI (0.46.0) would
in the same workspace. tau-vcs-host's `config` module ports jj-cli's
loading:

- Layers, lowest first: jj-lib's defaults; tau's `Identity` (`user.name`
  and `user.email`); the system's `/etc/jj/config.toml` and
  `/etc/jj/conf.d/`; the variables the person's config overrides
  (`operation.hostname`, `operation.username`, `NO_COLOR`, `VISUAL`,
  `EDITOR`); the person's config; the repository's; the workspace's;
  then the variables that override every file (`JJ_USER`, `JJ_EMAIL`,
  `JJ_TIMESTAMP`, `JJ_RANDOMNESS_SEED`, `JJ_OP_TIMESTAMP`,
  `JJ_OP_HOSTNAME`, `JJ_OP_USERNAME`, `JJ_EDITOR`, `JJ_PAGER`). So tau's
  identity names a commit only when nothing else sets `user.name` or
  `user.email`.
- The person's config is `~/.jjconfig.toml` when it exists, then
  `config.toml` and the `.toml` files of `conf.d/` (in name order) in
  `$XDG_CONFIG_HOME/jj` or `~/.config/jj`. `JJ_CONFIG`, a list of
  files and directories, replaces all of them, and the system's too.
- The repository's and the workspace's configs are where jj 0.46 keeps
  them, outside the repository: `repos/<id>/config.toml` and
  `workspaces/<id>/config.toml` in jj's config directory, named by
  `.jj/repo/config-id` and `.jj/workspace-config-id`. tau reads them
  with jj-lib's `SecureConfig::maybe_load_config`, as any jj command
  does, and makes a config id only where `jj git clone` would: when a
  clone or an import writes the repository's `trunk()` (see
  "Projects").
- `--when` scopes resolve once the repository is known, against the
  canonical workspace and repository paths, the home directory (for
  `~`), the host name and the variables. tau runs no jj command, so a
  `--when.commands` scope never holds.
- Settings are built when a workspace loads, in its job: a `Vcs` finds
  its workspace's repository first (`WorkspaceLoader::repo_path`), as
  jj does, then loads it with the settings for that repository and
  workspace. A `ProjectRepo` builds its main workspace's settings when
  it opens, and a clone or an import again once it has written the
  repository's config. A run workspace that `add_workspace` makes is written with
  the main workspace's settings, as `jj workspace add` does, then opened
  again with its own. Config changes reach a `Vcs` or `ProjectRepo`
  opened after them.
- jj-lib builds the commit signer from the same settings, so with the
  person's `signing.*` tau's commits are signed as jj would sign them.
  `~/` at the start of the paths it reads (`signing.key`,
  `signing.backends.ssh.allowed-signers` and `revocation-list`) is the
  `JjEnv`'s home directory, as jj expands it with its own.
- tau adds jj-cli's defaults (from its `src/config/misc.toml`) for the
  jj-cli settings it reads, just above jj-lib's: the person's config
  overrides them as it would the CLI's.
- Snapshots follow jj-cli's snapshot options: `snapshot.auto-track`, a
  fileset read from the workspace root with the person's
  `fileset-aliases` (a later layer's alias replacing an earlier one's);
  `snapshot.max-new-file-size`, in bytes or as `<n><unit>` (`1MiB`),
  where 0 means no limit; and the ignore patterns jj applies before the
  workspace's `.gitignore` files: Git's `core.excludesFile` (a relative
  path from the workspace root, `~/` for the home), or when it is unset
  `git/ignore` under `$XDG_CONFIG_HOME` or `~/.config`, then the Git
  store's `info/exclude`. Git's config is read as jj's gix reads it,
  from the `JjEnv`, never the process: the system's `gitconfig`
  (`GIT_CONFIG_SYSTEM`, `GIT_CONFIG_NOSYSTEM`), `git/config` under the
  config directory and `~/.gitconfig` (or `GIT_CONFIG_GLOBAL`), the Git
  store's `config`, includes, then `GIT_CONFIG_COUNT`'s variables. A
  malformed `snapshot.auto-track` or `snapshot.max-new-file-size` fails
  the snapshot with an error naming the setting (and the file that set
  it), so every tool fails until it is fixed, as every jj command
  would. A project's Git store is tau's own, so its `info/exclude` is
  not the source checkout's.
- Files are written and shown as jj writes them. jj-lib's working copy
  reads `ui.conflict-marker-style`, `working-copy.eol-conversion` and
  `working-copy.exec-bit-change` from the workspace's settings, so
  checkouts, restores and landings write conflicts, line ends and
  executable bits as the config says. `vcs_diff`, `vcs_status`'s diff
  and a project's diff show a conflicted file's markers in
  `ui.conflict-marker-style` too, merged with the store's `merge.*`
  options, as `jj diff --git` does, with `diff.git.context` lines of
  context (three by default).
- jj-cli settings tau does not follow, each because tau's own design
  decides what they would: `remotes.<name>.auto-track-bookmarks` and
  `git.track-default-bookmark-on-clone` (a project tracks every
  upstream branch, so trunk follows upstream, ADR 0015);
  `git.abandon-unreachable-commits` and
  `git.record-synthetic-predecessors` on an update (an update keeps
  what upstream dropped, and catch-ups leave it behind, so a chat's
  `@` is never abandoned under it);
  `snapshot.auto-update-stale` (tau always moves a stale workspace's
  files, as `jj workspace update-stale` does, since the main chat's
  catch-up restacks its chats); `git.colocate` (a project's layout is
  tau's; jj 0.46 also reads it in `jj workspace add`, which makes the
  new workspace a Git worktree when the current one is colocated, but
  tau's run workspaces are not Git worktrees yet: ADR 0043 makes them
  ones, with gix, since jj-lib's `create_worktree` runs `git`);
  `git.push` and `revsets.git-push` (a push names its remote, `origin`,
  and the one branch it sends, as `jj git push --remote origin
  --bookmark <branch>` would, so neither the default remotes nor the
  default revset applies); `aliases.*` (tau runs no jj command);
  `experimental-advance-branches` (tau moves its trunk and run
  bookmarks itself); `diff.git.show-path-prefix` (tau's diff readers
  need the `a/` and `b/` prefixes); `revsets.short-prefixes` and
  `revsets.log` (tau shows 12-character ids and resolves prefixes in
  the whole repository); `templates.*`, `ui.*` presentation, editors
  and pagers, which a model has no use for, including
  `templates.commit_trailers`, which needs jj-cli's template
  language.
- The environment is a value, `JjEnv`: config directory, home
  directory, system directory, host name, user name and variables.
  Only the app reads the process's (`Identity::from_environment`, which
  `tau`'s `main` puts in `HostConfig::identity`); everything else,
  tests included, gets the empty default and reads no file or
  variable. The host hands the same `Identity` to plugin hosts as a
  service, for the plugins and agents repositories.

## Threading

- jj-lib's futures are not `Send`. Much of their work is blocking file
  and object I/O. So each `Vcs` keeps its workspace behind an async
  lock, and each job takes the lock and runs on tokio's
  `spawn_blocking`, which drives jj-lib's futures with `pollster`
  ([ADR 0028](../decisions/0028-async-all-the-way-blocking-only-in-spawn-blocking.md)).
  Jobs on one workspace run one at a time, in the order they asked, and
  no thread waits between them.
- A tool future only awaits its job. It never holds jj-lib types across
  an await.
- A panic in jj-lib is caught in the job. The tool returns
  `jj-lib panicked: <message>` as its error, and the next job loads the
  workspace again.
- Clones of a `Vcs` share the workspace and its lock.
- A `Project` is the repository's handle for async code: `run` takes
  the repository's turn, which every handle on it shares, and runs the
  job on `ProjectRepo`, the blocking API, in `spawn_blocking`.
  `ProjectRepo` is reached only there, and in tests.
- All tools are `ExecutionMode::Sequential`. A batch of calls runs in
  order, so `vcs_commit` and then `vcs_log` in one turn see each other.
- Every operation that writes to a repository takes the repository's
  lock first, and holds it until the operation is written and the
  files are checked out: each tool's snapshot and transaction, and
  each `ProjectRepo` write (`update`, `add_workspace`,
  `add_workspace_from_snapshot`, `forget_workspace`, `abandon_between`,
  `remove_bookmark`, `push_trunk`, `push_branch`). Runs work in
  workspaces of one repository at the same time, and two operations
  that started from the same one would fork jj's operation log: a
  commit two of them rewrote
  would be divergent, and a bookmark two of them moved conflicted.
  Under the lock they happen one after another. The lock is
  `.jj/repo/tau.lock`, held with `flock`, so it holds between threads
  and between processes. Reads take no lock.

## Scoping rules

- **Ids only.** Arguments that name a change take a change id or a
  commit id, or a unique prefix of either:
  - change ids use jj's letters `k` to `z`;
  - commit ids use hex digits.
  - Revsets, `@`, bookmark names and tags are refused. A model cannot
    write a query that is arbitrarily expensive.
- **Snapshot first.** Every tool snapshots the working copy before it
  reads or writes. The snapshot is its own operation, marked as a
  snapshot, and only happens when files changed. It stands when the
  tool then fails or refuses.
- **The working copy only.** The write tools change only the
  working-copy change (`@`) of this workspace, and they only add new
  changes on top of it.
- **Immutable commits are refused.** A commit is immutable as jj says:
  the root commit, or an ancestor of what the config's
  `immutable_heads()` names. jj's default is `trunk()` (`main`,
  `master` or `trunk` on `origin` or `upstream`, or the default
  branch a clone wrote into the repository's config), tags, and
  untracked remote bookmarks and tags (the internal `git` remote left
  out); the person's `revset-aliases` can say otherwise. A `trunk()`
  that names something the repository lacks, as a bookmark it does not
  have, is `root()` instead, as jj makes it after warning; tau has no
  one to warn. One that does not parse fails the tools that read it. A write tool fails with
  `The working-copy commit <id> is immutable` when `@` is one of them,
  and one that cannot read `immutable_heads()` fails naming it.
- **Tagged operations.** Each write is one operation, described as
  `tau vcs: <tool>`. It carries the workspace name and the operation
  attribute `tau.vcs.tool = <tool>`. `vcs_undo` uses these tags.
- **New files are tracked as jj would track them.** The snapshot
  takes jj's snapshot options from the config, as the jj CLI does (see
  "Identity and jj config"). With jj's defaults it tracks every new file
  that no ignore file names, so there is no staging, except a new file
  larger than `snapshot.max-new-file-size` (1 MiB by default,
  `MAX_NEW_FILE_SIZE`): it stays out of `@`, and `vcs_status` names it
  under `Left out of @ (new files over <size> are not snapshotted)`
  with its size, in jj's format (`1.0MiB`). A new file that
  `snapshot.auto-track` does not match stays out too, as the person
  asked, and is not listed, as an ignored file is not.
- **Stale working copies.** When an operation in another workspace
  rewrote this workspace's commit, as the main chat's catch-up with
  trunk does to the commits its chats stand on, the next tool or turn
  first moves the files to the rewritten commit, as jj's
  `workspace update-stale` does, with what was edited on disk since the
  last snapshot merged on top, as a rebase would: an edit to a file the
  rewrite changed too becomes a conflict. tau-vcs never forks the
  operation log itself (see "Threading"). Another program can, as a
  `jj` command run by hand can. When that leaves the working copy on an
  operation the repository's head does not descend from, or leaves
  `@`'s change divergent (two operations rewrote `@` at once, and jj
  merged both), the tools fail with `The working copy is stale: ...`
  and ask for the user to update the workspace: either side of a
  divergent `@` may hold the run's work, so they keep both.

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
- Conflicts are structured jj data. Some materialize without markers,
  including empty-v-deleted files. Unchanged contents, including a
  same-byte rewrite, remain conflicted. Edits can remove markers;
  `vcs_restore` can select a committed side; `vcs_resolve` can explicitly
  accept a markerless file's current native materialization.
- Cancelling redundant native merge terms does not list an untouched
  path as changed, in status, diff, commits, or turn snapshots.
- The host reads the same conflicted paths with `Vcs::conflicts`
  (after a snapshot): `@` sits on the run's newest commit, so they are
  what the run's stack leaves in conflict.

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

### vcs_commit: `{ message, paths? }`

- Does what `jj commit -m` does: describes the working-copy change, and
  then starts a new empty change on top of it as the new `@`. Files do
  not change.
- With `paths` (files or directories, relative to the repository root,
  `.` for everything), it does what `jj commit -m <message> <paths>`
  does: the described change keeps `@`'s change id but only what `@`
  changes under those paths, its parent's version of the rest; the new
  `@` on top holds the rest of the work, so files still do not change.
  This lets the model commit unrelated edits apart.
- An empty message is refused: `The description must not be empty`. So
  are empty `paths` (`Name at least one path …`), and paths `@` does
  not change, naming what it does change.
- Details: `committed`, `working_copy`, and `left`: the paths still
  changed in the new `@`, in path order (empty without `paths`).
  For the card, also the committed change's `parent`, its `files`,
  `diff` and `truncated` against that parent (as `vcs_show` has them),
  and `left_files`, `left_diff` and `left_truncated`: what the new `@`
  holds on top of it.
- The card draws the stack the commit made: closed, a small graph and
  the subject; open, `@` with what stayed in it, the new change with its
  message and files, and the parent below.
- Nothing is committed for the model (ADR 0014): its commits are how a
  run's work is reviewed, landed and pushed, so the description asks
  for a Conventional Commits message at each boundary a reviewer would
  want, and everything committed before the run finishes.

### vcs_land: `{}`

- With `VcsPlugin::landing()`: for chats that land on their
  repository's main chat when they finish. Main chats and sub-agents
  have nothing to land (a main chat commits on trunk, a sub-agent lands
  as it returns), but declare it all the same, through
  `VcsPlugin::refusing_landing(why)`, so every run of a repository
  sends the same tools and reads the same prompt cache (ADR 0022); a
  call there fails with `why`.
- Proposes landing the run's commits. It moves nothing: it refuses
  while `@` holds changes (`Your working copy has uncommitted changes
(…). Commit your work with vcs_commit first.`), and otherwise returns
  `{ "proposed": true, "head": <@'s parent> }` for the host, which
  shows the person what would land and waits for them to confirm.
- tau-ui opens the landing card once the run stops, if a `vcs_land`
  went through during it: one the model called, or one a tool called
  through the loop at any depth, such as a codemode script's
  (`RunView::proposes_landing`). It knows a nested one from its
  `ToolEnd`, while the model's call runs, or from that call's
  `details.calls` once it ended, the only record a stored run has.
  However many times the run proposed, the card opens once.

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

### vcs_resolve: `{ paths }`

- Explicitly accepts the current native materialization of selected
  markerless file conflicts, including empty and binary files. File
  presence and executable mode follow jj-lib's checkout materialization.
- At least one path is required. `.` matches everything; directories
  select their conflicted descendants. Paths already resolved are no-ops.
- A marked or non-file conflict refuses the entire selected resolution.
  Its markers require an edit, or a committed-side selection through
  `vcs_restore`. Nothing else is implicitly accepted.
- Details: `resolved` (accepted paths) and `working_copy`. The resolution
  remains in `@` until `vcs_commit`; `vcs_undo` can undo the resolution.

### vcs_undo: `{}`

- Undoes the newest operation that these tools made in this workspace,
  in the way that `jj undo` does. It merges the parent's view over the
  current one, so file edits made since that operation are kept.
- Snapshots are skipped, and so are operations that an earlier
  `vcs_undo` already undid. Calling it again undoes the operation
  before that one.
- It refuses when the newest operation that is not a snapshot was not
  made by these tools in this workspace. For example, the user's own
  `jj` commands, another workspace, the initial repository setup, or
  what the host does in this workspace: a turn's checkpoint, a
  catch-up with trunk (`Vcs::move_onto`) or a landing (`Vcs::land`).
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

| Tool            | Condition                          | Message                                                                                                                                                |
| --------------- | ---------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| all             | cancelled before starting          | `Operation aborted`                                                                                                                                    |
| all             | jj-lib panicked                    | `jj-lib panicked: <message>`                                                                                                                           |
| all             | stale working copy                 | `The working copy is stale: another process changed this workspace's commit. ...`                                                                      |
| ids             | not an id                          | `<rev> is not a change id or a commit id. ... revsets are not accepted.`                                                                               |
| ids             | unknown change or commit           | `No change matches <rev>`, `No commit matches <rev>`                                                                                                   |
| ids             | ambiguous prefix                   | `Change id prefix <rev> is ambiguous; give more of it` (or `Commit id prefix`)                                                                         |
| ids             | divergent change                   | `Change <rev> is divergent; pass a commit id instead`                                                                                                  |
| ids             | abandoned change                   | `Change <rev> is hidden (abandoned)`                                                                                                                   |
| paths           | absolute, outside the workspace    | `<path> is outside the repository`                                                                                                                     |
| paths           | `..` or not a valid path           | `<path> is not a path inside the repository`                                                                                                           |
| write tools     | `@` is immutable                   | `The working-copy commit <id> is immutable`                                                                                                            |
| `vcs_commit`    | empty message                      | `The description must not be empty`                                                                                                                    |
| `vcs_commit`    | `paths` that `@` does not change   | `The working copy changes none of <paths>. It changes: <paths, or nothing>`                                                                            |
| restore/resolve | no paths                           | `Name at least one path ("." matches everything)`                                                                                                      |
| `vcs_resolve`   | marked or non-file conflict        | `<path> has markers or a non-file conflict. Edit it or use vcs_restore to select a committed side; vcs_resolve accepts only markerless file contents.` |
| `vcs_undo`      | newest operation is not the tools' | `The last operation was not made by the vcs tools in this workspace ("<description>"); ...`                                                            |
| `vcs_undo`      | concurrent operations              | `The operation log has concurrent operations here; ask the user to undo from the operation log`                                                        |

## Projects

A `Project` is a repository tau owns, in a directory of its own
(the host uses `$XDG_DATA_HOME/tau/repos/github/<owner>/<name>/` for a
clone from GitHub). Runs never work in
the user's checkout.

| Path           | What it is                                                                     |
| -------------- | ------------------------------------------------------------------------------ |
| `main/`        | The jj repository, colocated with its Git store: `main/.jj` beside `main/.git` |
| `runs/<name>/` | One jj workspace per run, named by the host.                                   |

`main/` is colocated as `jj git init --colocate` makes a repository, so
`jj` and `git` both work in it:

- Upstream's branches are remote-tracking branches,
  `refs/remotes/origin/<branch>`, which jj imports as `<branch>@origin`,
  each tracked by the bookmark of the same name. Its tags are tags, and
  `refs/remotes/origin/HEAD` names its default branch. `origin` holds
  upstream's URL (or, for a local source, its path) and no fetch
  refspec.
- Every operation tau writes exports the bookmarks and tags to Git's
  `refs/heads/` and `refs/tags/`, and, when the main workspace's `@`
  moved, points Git's `HEAD` at `@-` and resets the index to it, as
  the jj CLI does after each command. `git status` in `main/` shows
  `@`'s changes.
- Run workspaces are plain jj workspaces, with no `.git`: jj-lib cannot
  make one a Git worktree.

- `Project::clone(url, token, root, identity)` makes the project at
  `root` by fetching every branch and tag of `url` with gix, as
  `jj git clone --colocate` would. The token answers the server's
  request for credentials and is not written anywhere. A failed clone
  removes `root`. As `jj git clone` does, when `origin/HEAD` names a
  branch the clone has, the repository's config (`repos/<id>/config.toml`
  in the person's jj config directory, made when there is none, with
  what it held kept) gets `revset-aliases."trunk()" =
  "<branch>@origin"`, so `trunk()` and `immutable_heads()` name
  upstream's default branch whatever it is called. Nothing is written
  without a jj config directory. An import writes it the same way, the
  source's default branch standing for the remote's. An update leaves
  it as it is, as `jj git fetch` does, though trunk follows a new
  default branch. jj 0.46.0 refuses to clone when the person's own
  `trunk()` names a remote bookmark: it resolves `immutable_heads()`
  in the still-empty repository, without the fallback to `root()` its
  other commands have. tau clones, and falls back as those commands
  do.
- `Project::open_or_import(source, root, identity)` opens the project at
  `root`, or makes it from the local repository at `source` (a checkout,
  a linked worktree or a bare repository) as if cloned from it. No
  `git` is needed: the object files are hard-linked (they never change
  once written), the branches and tags written as a fetch would write
  them, the branch `HEAD` names made `origin/HEAD`, and `shallow`
  (where a shallow clone's history starts) copied. Cloning and
  fetching go through gix; only pushing runs `git`, through jj-lib
  (see "Pushing").
- `update(from)` brings in what changed at the source since the import
  or the last update: from the checkout (`UpdateFrom::Checkout`) or
  from a remote (`UpdateFrom::Remote`, a fetch through gix). The
  remote-tracking branches and tags become the source's: those the
  source deleted go. A remote's or a bare repository's `HEAD` comes too,
  so trunk follows a new default branch; a detached one leaves no
  default branch. A checkout's does not: it names the branch checked
  out there, so trunk stays on the branch the import's `HEAD` named. It
  returns trunk before and after (`Updated`). Runs keep their
  workspaces and commits.
- `trunk()` is the commit new runs start from: the bookmark of the
  branch `origin/HEAD` names, else `main`, `master` or `trunk`, else
  the root commit.
- `add_workspace(name, base)` makes `runs/<name>` on a new empty commit
  on top of `base` and checks out its files. When `base` was rewritten
  since the caller read it, as the main chat's catch-up rewrites its
  commits, the workspace starts on the commit `base`'s change names
  now, so the old commit does not come back beside the new one. `forget_workspace(name)`
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
  name `tau-vcs`:

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
  paths the turn changed. They count from the turn before's snapshot,
  with what each landing and catch-up since did to the run's newest
  commit applied on top: what a landing stacked on the run, or a
  catch-up brought by restacking the commit the run stands on, is not
  the turn's. Nothing else is followed: the run's own commits, undos and
  recommits are the turn's work. A confirmed landing tags its operation
  with the change ids it landed (`tau.vcs.landed`), and a catch-up
  (`Vcs::move_onto`, or the step a run's commit, turn end or landing
  takes after an update) with the commit it moved onto
  (`tau.vcs.caught_up`). The turn walks the operation log back to its
  snapshot, and for each tagged operation applies the change of the
  run's newest commit (its working copy's parent) from before to after
  it, whichever workspace made it: a catch-up of the main chat moves
  the chats standing on its commits too. The landing's own links record
  what landed.

- **Before it stops** with changes in `@`, the run is held once, with
  `COMMIT_FIRST` and the paths as the next user message.
- **When it finishes** normally or at a limit with changes still in
  `@`, they are committed (`Vcs::commit_all`), with a message the run's
  model writes from the diff and the run's task (its own input, not the
  first message of a transcript it forked): one short `PluginCtx::ask`.
  tau never writes a commit message itself. An empty or failed answer,
  a failed commit, or oversized untracked files leave the work uncommitted
  and record a finalization error. A delegating caller verifies the native
  working copy before handing it off; manual landing also refuses
  remaining edits. A failed or cancelled run keeps its work as it is.
- **Forking at a turn**: read the run's links with
  `Store::plugin_entries(run, "tau-vcs")`, take the `seq` of the
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
  `head` has that trunk lacks. A pull request replays those onto
  GitHub's trunk (see "Pull requests").

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
- A child that already sits on the parent's head (the parent has not
  moved since it started) is not rewritten.
- A `child_head` that is no longer visible, because something rewrote
  or abandoned it after the caller read it (the main chat's catch-up
  restacks the chats on its commits), is refused with
  `VcsError::HiddenHead`, naming it: landing it would bring the old
  copies back beside the new ones. Read the child's bookmark again.
  The head is checked as the landing finds the repository: when the
  landing's first step moves the parent onto trunk after an update
  (`land::follow_bookmark`), the child's head moves with it, and lands
  where it went.
- The parent's uncommitted work stays uncommitted: its working copy
  moves onto the landed changes. Landing is between the parent's turns.
- The host's `Host::land` records each landed change as a `Link` in
  the parent, at the parent's latest turn, with `from` naming the
  child, so forks, the compare view and pull requests read them as the
  parent's own. Then it closes the child: its workspace is forgotten
  and its bookmark removed. Both runs must be idle.
- `Host::land` survives tau closing at any step. It first stores a
  `landing-intent` record in the parent (the child, its head, both
  workspaces). The restack is one jj operation, which records what it
  did under the attribute `tau.vcs.landing` (`{ child_head, landing }`),
  so `Project::landed(child_head)` reads the `Landing` back from the
  operation log (the newest `LANDING_LOOKBACK` operations). The links
  and the `landing` record go in one store write; forgetting the
  workspace and removing the bookmark find nothing to do when done
  before. A restack that fails stores the intent again, `cancelled`.
  At start, `Host::recover` finishes each intent with no `landing`
  record: a restack the operations show done is recorded, and the
  child closed; one not done lands now. Their records say `recovered`,
  and the parent's card says tau finished the landing at start. A
  landing whose record was stored is done but for the child's
  workspace and bookmark, which the sweep takes. A host test cuts a
  landing off after each step (`Host::cut_landing_after`) and checks
  that the next start leaves what a whole landing leaves.
- `Host::drop_child` closes a child without landing it: its own
  changes (what its head has that its parent's lacks) are abandoned
  with `Project::abandon_between`, and its workspace and bookmark go.
  A main chat catches up with trunk first, and what it keeps is what
  its working copy stands on, not trunk's bookmark: an update may have
  moved trunk past the main chat's own commits under the child.
  `Host::land` catches a main chat up first too, and reads the child's
  head after, since the catch-up restacks the child.
  The operation log keeps what was abandoned. It stores a `dropped`
  record on the child first, so a drop cut off by tau closing still
  counts.
- A chat that landed (its parent holds a `landing` record from it, or
  links whose `from` names it) or was dropped (it holds a `dropped`
  record) ended for good (`Host::ending_of`, `RunView::ending`). The
  host refuses to resume or steer it, saying why, so no workspace or
  `tau/<run>` bookmark comes back for it; it cannot land or be dropped
  again. Its screen is read-only: the composer's place says why and
  offers a new chat from main, on the desktop and on a phone. History
  still loads it.
- A child cannot land or be dropped while it has children still open:
  running, or holding changes it does not have. They land or are
  dropped first, one level at a time.
- Land in the interface does not call `Host::land` directly: it puts
  the chat in its main chat's landing queue
  ([ADR 0024](../decisions/0024-landings-queue-while-the-parent-works.md)),
  `Host::queue_landing`, which lands it at once when main is idle and
  nothing waits before it. The queue is `host::queue::Lane`, a pure
  state machine; the host carries out the `Action`s it returns. Each
  change is a `landing-queue` record on the main chat (`Record`:
  `queued`, `previewed`, `left`, `resolving`, `conflicted`,
  `dismissed`, `clean`), and `Lane::restore` folds them back, so a
  restart keeps the queue. A queued chat (`queue::Waiting`) carries the
  files its last preview found in conflict and the files the person
  confirmed: those of the preview they saw before clicking Land
  (`Host::preview_landing` keeps it), or, for a chat queued already,
  what its last preview in the queue found.
- `queue::drain` lands what may land: never while main runs, while
  tau's resolving turn is about to start, or while main is marked
  conflicted. It reads main's stack first (`Host::main_conflicts`:
  catch up, then `Vcs::conflicts`), then previews the first chat again.
  Clean, or conflicting only in confirmed files, it lands through
  `Host::land`; conflicts start main's resolving turn and stop the drain
  until that turn ends. New conflicts stop the drain and leave the chat
  first, needing confirmation. A chat that landed or was dropped
  meanwhile leaves the queue; one that cannot land leaves it, and its
  card says why. The host drains after Land, Unqueue and Drop, after each
  turn of main (`Host::main_turn_ended`), and at start
  (`Host::drain_all`), one drain at a time.
- A preview works while main runs: it reads main as it is, without
  catching it up or making workspaces. `Host::land` holds the start
  lock, so no turn of main starts in the middle of a landing.
- tau's resolving turn (`Host::start_resolving`) gets a plugin that
  holds its stop once while main's working copy still has conflicts:
  `Conflicts remain in a.rs, b.rs. Resolve them and commit.` A main turn
  that ends with conflicts marks main (`Record::Conflicted`), and the
  host calls the hook given to `Host::on_conflicts_on_main`. While
  marked, nothing lands on main, and `Host::start` and `Host::fork`
  refuse with `main has conflicts in a.rs; resolve them first`. The mark
  goes once main's stack is clean, checked at each turn end of main and
  each drain. Resolve again starts tau's turn again with the last
  resolving message; "I'll write to main" stores `dismissed`, which
  hides the card and keeps the mark.
- The interface shows the queue and the mark on the main chat's view
  (`RunView::landing_queue`, `RunView::main_conflicts`), sent as
  `HostUpdate::LandingQueue`; `Workspace::queued` gives a chat's place.
  Preview, Land, Unqueue and Drop run on the host's blocking pool and
  answer with `HostUpdate`s.
- With `confirm` off, nothing changes. The `Landing` it returns says
  what would happen: the changes as they would be (`changes`, newest
  first), the unresolved paths in the new head (`conflicts`), whether
  their native materialization has markers or not, and the new head.
  Confirmed, conflicts land as jj
  conflicts for the parent's next turn to resolve.
- `Host::forecast_landing` reads a landing as the preview does
  (`Host::land_dry`, the same `LandingPlan` and `Vcs::land` with
  `confirm` off), but with `Reading::Forecast`: it writes nothing,
  neither catching the parent up nor making workspaces, so it can run
  in the background, and it keeps nothing for Land to confirm. The
  sidebar's "Ready to land" and "Would conflict" come from it
  ([attention.md](attention.md)).

## Sweeping at start

Nothing cleans a run's workspace and bookmark once the process that ran
it is gone: a sub-agent cut off mid-call, a chat whose landing or drop
tau closed in the middle of, a workspace whose run never reached the
store. Left there, they take disk space, and the main chat's catch-ups
restack their commits every time.

`tau_vcs_host::sweep::plan(owners, workspaces, bookmarks)` is the rule, a
pure function. Each `Owner` is a run with the workspaces it worked in
and its `Standing`:

- `Kept`: its workspaces and bookmark stay.
- `Landed`: they go; its commits, on its parent now, stay.
- `Discarded`: they go, and first its own commits, what its bookmark
  and its workspaces' working copies have that its `Keep` (its parent's
  bookmark, or the main chat's working copy) lacks, as a drop does.

A workspace goes unless a kept run worked in it, and `default`, the
repository's checkout, never goes. A `tau/<run>` bookmark goes unless
that run is kept; other bookmarks are not runs' and stay. What no
owner names belongs to a run that is gone, and goes. `Project::sweep`
carries a plan out: abandons, then forgets workspaces (their directories
go), then removes bookmarks. Each step is idempotent, so a sweep cut
off is finished by the next, and a done sweep plans nothing.

The host sweeps each listed repository as it starts (`Host::recover`,
off the interface's thread, before updates). A run of this session, or
a sub-agent under one, is kept, and so is a workspace named after one
of their workspaces with `-sub-`, which a `spawn` may be making; no run
starts while the sweep reads what the projects hold. A chat that landed
is landed, one dropped is discarded, and any other chat is open and
kept, whatever its status: a chat that finished but has not landed,
failed, or was cut off can go on. A sub-agent that failed, was
cancelled or was cut off is discarded, as its end would have done; one
that finished keeps what it has, to land, or because it could not be
finalized.

## Moving onto trunk

`Vcs::move_onto(trunk, bookmark, confirm)`, on a run's `Vcs`, rebases
the run's changes, up to `@`, onto trunk's newest commit, and points
`bookmark` at the run's newest commit there. A run with no commit of
its own, moved onto the root commit (upstream deleted every trunk
branch), has none, and the bookmark goes: no bookmark names the root
commit. The run's changes are
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
trunk's bookmark, not `tau/<run>`. It has nothing to land: its
`vcs_land` refuses ("The main chat commits straight to trunk, so it has
nothing to land: …"), and its bar offers no landing. It is the only
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

An update can also land while the main chat's turn runs. So when an
update has moved a run's bookmark (it names upstream's commit, as
`<bookmark>@origin` does, which a run's own commits never do) to a commit
the run's newest commit does not have, `commit_all`, `end_turn` and
`Vcs::land` first move the run's changes, up to `@`, onto it, as a
catch-up does, in the same operation: trunk moves forward, never aside.
And `move_onto` goes onto that commit instead of the one it was given
when the update came after the caller read trunk.

## Sub-agents: spawn

Only the main chat starts sub-agents (ADR 0016). Chats and sub-agents
declare `spawn` all the same, as `RefusingSpawn` with the same
description and arguments, so their tools match main's and their first
request reads main's prompt cache
(ADR 0022). A call fails with `ONLY_MAIN_SPAWNS`: "Only the main chat
starts sub-agents; do this work here or ask the person to start a
chat."

A sub-agent is a child run in a chat of its own that forks the
caller's conversation, unless it is `fresh`, and runs beside it
([ADR 0009](../decisions/0009-child-runs-land-on-their-parent.md),
[ADR 0015](../decisions/0015-delegates-fork-their-caller.md),
[ADR 0026](../decisions/0026-sub-agents-run-detached.md),
[ADR 0031](../decisions/0031-the-main-chat-orchestrates.md),
[ADR 0042](../decisions/0042-sub-agents-can-start-fresh-and-keep-their-work-aside.md)). Main
orchestrates: each of its runs starts with context that says to split
the request into tasks, spawn a sub-agent for each, and end its turn
while they work. Nothing waits for a sub-agent. A caller's
sub-agents live in a `SubAgents`, which outlives the caller's turns:
tau-ui keeps one per repository's main chat, which sends the
sub-agents' events on with the host's own. `Spawn` is built per run on
the run's `RunWorkspace` and the caller's `SubAgents`.
`Spawn` also takes the model ids a call may pick from and a closure
that builds the sub-agent's `Agent` around the sub-agent's own
`RunWorkspace` on the `ChildModel` its call asked for. tau-ui builds it
on the model and effort asked for, or the caller's, and refuses an
effort the model does not take.

1. `spawn` (`{ task, model?, effort?, fresh?, land? }`) needs the caller's work
   committed (ADR 0014): with changes in its `@`, it refuses and says to
   commit with `vcs_commit` first. With `MAX_RUNNING` (8) running
   already, it refuses and says to end the turn and spawn again when
   tau reports one. The sub-agent's
   workspace, `<caller's workspace>-sub-<12 random hex digits>`, so that
   no process reuses one an earlier one left, starts on the caller's
   newest commit (`RunWorkspace::with_base`), so it sees the caller's
   work.
2. The sub-agent starts through `Agent::as_tool(…).forking().spawn(…)`:
   a `Subagent` run of the caller with a `fork_seq`, with its own events
   and its own cancel token. It is asked on the caller's transcript, the
   turn that made the call with an output for each call in it, and then
   its task. With `fresh: true` it starts without `forking()`: its first
   message is its task alone, so a reviewer or a judge does not carry
   the caller's leanings. It commits its work on its own stack, under its own
   bookmark. `spawn` answers at once with its run id (`details.run`);
   the caller goes on, and cancelling the caller does not stop it.
   `spawn` is `ExecutionMode::Grouped`, so a `vcs_commit` in its batch
   runs before or after it, never while it reads the working copy.
3. As it ends, `SubAgents` checks its work. One that finished, or that
   a limit stopped, can land (`Ending::Done`), once a verified committed
   working copy shows its finish hook committed everything; otherwise
   it is `Ending::Retained`: nothing lands, and its workspace and
   bookmark stay for recovery. One that failed (`Ending::Failed`) or
   that the person stopped (`Ending::Stopped`) has its changes abandoned,
   and its workspace and bookmark go. One spawned with `land: false`
   that finished is `Ending::Kept` instead of `Done`: its workspace
   closes, its commits stay under its bookmark, and the ending lists its
   own changes (`ProjectRepo::changes_between` its base and its head).
4. tau-ui takes each ended sub-agent (`SubAgents::take`) and queues it
   on the main chat's landing queue (ADR 0024) with how it ended
   (`Waiting::sub_agent`). It lands whatever it conflicts in. A drain
   lands queued sub-agents one after another, and reports one with
   nothing to land (`Waiting::reports_only`: it failed, or was kept).
   A kept one is recorded as dropped as it is queued, so the next
   start's sweep abandons its commits; until then its report names its
   changes (`kept_note`), for main to read with `vcs_show` and take
   files from with `vcs_restore` `from`; after the drain, tau starts main's turn with each
   sub-agent's title, answer and landing note, which starts by saying
   which limit stopped it, if one did (`It stopped at its turn limit.`),
   and the conflicts to resolve, if any. The turn tells main to check
   what landed, resolve conflicts, and spawn what comes next. A
   sub-agent the person stopped is not reported. A main turn the person
   stopped drains nothing (`Host::main_turn_stopped`): what waits lands
   after main's next turn, or when another sub-agent ends.

Behavioral properties and replay instructions are in
[VCS handoff and conflict properties](vcs-hardening-tests.md).

A landed sub-agent records its links on main as a chat's landing does.
A call a tool makes through the loop, such as a codemode script's, is
a call like the model's: in tau-ui a `spawn` it makes gets a chat on
its task (`RunView::call` finds a nested call while the model's call
runs).

## Pushing

[ADR 0023](../decisions/0023-main-pushes-with-git-chat-prs-replay-onto-origin.md).
A project pushes to the repository it was cloned from with jj-lib's
`push_updates`, which runs `git push` on the project's Git store. This
is the one place tau runs `git`; the Nix package puts it on `tau-ui`'s
`PATH`, and `git.executable-path` in the person's jj config names
another, as for jj. `Remote { url, token }` says where:

- The token reaches `git` through the child's environment alone:
  `GIT_CONFIG_COUNT` sets `credential.helper` to an empty value, which
  drops the person's own helpers so none stores the token, then to a
  shell helper that answers `get` with `x-access-token` and
  `$TAU_GIT_TOKEN`. `GIT_TERMINAL_PROMPT=0` keeps `git` from asking.
  The token is never written to a file or put on a command line.
- The Git store's `origin` is rewritten before each push to hold the
  URL and nothing else. Without a fetch refspec, `git push` writes no
  remote-tracking ref of its own: a pull request's branch stays out of
  the project until a fetch brings it.
- `Project::upstream()` is GitHub's trunk as the last fetch or push
  left it: what `<trunk>@origin` names.
- `Project::unpushed()` lists trunk's commits that `upstream()` lacks,
  oldest first: the main chat's work waiting to go.
- `Project::push_trunk(remote)` pushes trunk to the remote's branch of
  the same name, with a lease on `upstream()`: the commits themselves,
  so commit and change ids stay. Nothing to push returns an empty
  `Pushed`. A commit with conflicts refuses the push. A remote whose
  branch moved since the last fetch refuses it (`git` rejects the lease
  or a non-fast-forward): `VcsError::PushRejected`. A remote that
  refuses for its own reasons, such as a hook or a protected branch, is
  `VcsError::PushRefused` with its reason. Once pushed, the Git store's
  remote-tracking branch names the pushed commit and an import moves
  `<trunk>@origin` there, as a fetch would, so nothing is ahead. The
  pushed commits are then immutable as upstream's are, when the
  config's `immutable_heads()` names `<trunk>@origin`, as jj's default
  does for `main`, `master` and `trunk`, and for the default branch
  through the `trunk()` a clone writes (see "Scoping rules").
- `Project::replay(commits, onto)` copies commits (oldest first, each on
  the one before) onto `onto`: each copy's tree is jj's three-way merge
  of the copy before (`onto` for the first), the commit's parent tree,
  and the commit's tree. Copies keep the description and author and get
  new change ids, so they never make the originals divergent once a
  fetch brings them back. They are written in a transaction that is
  dropped, so no operation records them and they stay hidden; the Git
  store keeps them under jj's `refs/jj/keep/`. The first copy that would
  hold a conflict refuses with `VcsError::WouldConflict(paths)`.
- `Project::push_branch(remote, branch, expected, head)` points the
  remote's `branch` at `head` if the remote has it at `expected` (absent
  for `None`), else `PushRejected`. No bookmark changes. It returns the
  head it pushed: `head`, unless signing on push made copies of it.
- The person's jj config applies as `jj git push` applies it (jj-cli
  0.46.0's `commands/git/push.rs`), with jj-cli's defaults
  (`git.private-commits = "none()"`, `git.sign-on-push = false`, and
  its `revset-aliases`):
  - `git.private-commits` is a revset, parsed with the person's
    `revset-aliases` and `fileset-aliases`, their email for `mine()`,
    and the main workspace for `@`. `push_trunk` refuses, before
    anything is sent, when a commit it would send (trunk's ancestors
    that neither `<bookmark>@origin` nor `immutable_heads()` reaches)
    is in it: `VcsError::PrivateCommit`, naming the newest such commit
    (change id, commit id, first line) and the revset. A pull request
    sends copies, so `replay` refuses the same way for the commits it
    copies, before making any copy. The revset is evaluated whole, as
    jj 0.46 evaluates it, so an `at_operation()` in it that names an
    operation off the repository's line (a concurrent or unpublished
    one) sees that operation's commits.
  - With `git.sign-on-push`, `push_trunk` first signs each commit it
    would send that is mutable (not in `immutable_heads()`), unsigned,
    and the person's own (its author's email is `user.email`), with
    `signing.*`, as jj's `signing.behavior = "own"` would, and rebases
    what stands on them, in one operation; trunk then names the signed
    commits, so their commit ids change and their change ids stay.
    `push_branch` signs the copies under `head` that `replay` made in
    the same way, as new hidden copies.
  - A revset in the config that does not parse or resolve fails the
    push with an error naming the setting (`VcsError::ConfigRevset`).

The host's `Host::push_main(repo, fetch)` pushes a repository's trunk
with the GitHub sign-in's token. With `fetch` it first updates from
GitHub and moves the main chat onto trunk (`Vcs::move_onto`), which
puts the main chat's commits on top of GitHub's new ones; the main chat
must not be running then. The catalog's `Repo::unpushed` counts
`unpushed()` for each repository from GitHub; the main chat's header
and sidebar row show it with a Push button, and its chat ends with a
card for the last push, or for GitHub's moved branch with Fetch and
push (`WorkspaceEvent::Push`, `HostUpdate::Pushed`).

## Pull requests

A chat's pull request carries only the chat's commits:

1. Preparing it fetches from GitHub, then replays the chat's stack onto
   `upstream()`. A replay that would conflict refuses the pull request:
   `would conflict on origin/<trunk>: a.rs, b.rs`. The draft lists each
   copy with its line counts.
2. Creating it replays again and pushes the last copy to the draft's
   branch with `push_branch` (expecting no branch there), then opens
   the pull request and asks for reviews through GitHub's API.
3. Keep pushing replays the chat's commits after the last one pushed
   onto the branch's commit, and pushes with a lease on it: a
   fast-forward. A chat whose last pushed change is gone from its
   stack replays all of it onto `upstream()` again.

The main chat has no pull request: it pushes.

## Left to the host and the UI

These operations change shared state, use the network, or throw work
away. The model does not get them. The host calls jj-lib (or later
`tau-vcs` APIs) for them when the user asks
([jj-lib.md](../research/jj-lib.md), "Operations the user triggers from
the UI"):

- **Fetch and push.** Updating trunk (`Project::update`) and pushing
  (see "Pushing"). These need credentials.
- **Pull requests.** Opening a PR after a push (see "Pull requests").
- **Bookmarks, rebase, abandon and squash.** The study's
  `vcs_abandon`, `vcs_squash`, `vcs_cat` and named revisions (`trunk`,
  `fork-point`) are not built.
- **Operation log.** Browsing it, restoring an arbitrary operation, and
  updating a workspace a concurrent operation left stale.
- **Guarding the shell.** A `before_tool` hook that blocks `git` and
  `jj` in `exec_command` is not built.
