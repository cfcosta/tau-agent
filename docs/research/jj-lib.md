# Research: version control through jj-lib

- Status: research, no decision yet
- Date: 2026-09-28
- Checked against: `jj-lib` 0.45.1 (released 2026-09-03)

This note studies how the tau app could own its repositories and do all
version control through `jj-lib`, the library behind Jujutsu. The goal
is that code and chats are both versioned, that every turn is a point
you can check out, fork from and compare, and that the model reaches
version control only through tools, never through `jj` or `git` in a
shell.

Items marked **(unverified)** were not confirmed against the docs or
source and need a spike before a decision.

## Summary

- **Recommended shape.** One jj repository per project, with an
  internal (non-colocated) Git store, under
  `$XDG_DATA_HOME/tau/repos/<name>`. One jj workspace per run.
- **Every turn is a snapshot.** At each turn end the plugin snapshots
  the run's working copy. The snapshot's commit id is the code state of
  that turn. The model decides where change boundaries go, with a
  `vcs_commit` tool, so the history pushed to GitHub stays readable.
- **SQLite stays the source of truth for runs.** Transcripts, costs,
  forks and checkpoints stay in `tau-store`. The link from a turn to a
  commit is a plugin record (`kind = 'plugin'`), which already follows
  the fork chain and the fork cutoff.
- **Chats go into jj as a second, unpushed lineage** in the same repo,
  in a later stage. One transaction writes the code snapshot and the
  chat commit, so both land in one operation.
- **The model gets about twelve tools.** Fetch, push, clone, op-log
  restore and branch cleanup stay in the UI.

## jj-lib today

| Fact            | Value                                                                              |
| --------------- | ---------------------------------------------------------------------------------- |
| Latest version  | 0.45.1, 2026-09-03                                                                 |
| Release cadence | Monthly, one minor per month since at least 0.34.0 (2025-10-01)                    |
| License         | Apache-2.0 (also `jj-core`, which it now depends on)                               |
| MSRV            | 1.89                                                                               |
| Features        | `default = ["git"]`, `git = ["dep:gix"]`, `watchman`, `testing`                    |
| Git library     | gitoxide (`gix` 0.87) for objects and refs; no `git2`, no libgit2                  |
| Fetch and push  | Spawn the `git` binary (`GitSubprocessOptions`)                                    |
| Async           | Most repo, workspace and transaction calls are `async fn`; fetch and push are sync |
| Runtime         | No tokio by default (only with `watchman`); uses `pollster` and `rayon`            |
| Dependency tree | 271 crates with default features, 84 of them `gix-*`                               |
| Docs coverage   | 57% of `jj_lib` items documented on docs.rs                                        |

The tree was measured with `cargo tree -e normal` on a fresh crate that
depends only on `jj-lib = "0.45.1"`. Licenses in that tree are
permissive (MIT, Apache-2.0, BSD, Zlib, Unlicense), plus one MPL-2.0
crate. `deny.toml` will need that MPL-2.0 entry checked. tau-agent does
not have `gix` or `prost` in its lock file yet, so all of it is new
weight, but `rayon` 1.12 is already there.

### Stability

- jj is pre-1.0. Every monthly release may break the library API, and
  the CLI is the only consumer the project tests against.
- The upstream issue on panics in the library (#5685) is still the
  reference: a panic in jj-lib is a crash of the host. The tau process
  runs agents and the UI, so jj calls should run where a panic can be
  caught (`catch_unwind` around a blocking task) **(unverified that
  jj-lib state stays usable after a caught panic; assume it does not
  and reload the repo)**.
- The async move is recent. `Workspace::init_*`, `RepoLoader::load_at_head`,
  `Transaction::commit`, `CommitBuilder::write`,
  `MutableRepo::rebase_descendants`, `check_out` and `edit` are async in
  0.45.1. Whether the returned futures are `Send` is **unverified**.
- Pin an exact version (`=0.45.1`) and upgrade on purpose, one release
  at a time, with the plugin's tests as the check.

## Where things live

```
$XDG_DATA_HOME/tau/
  tau.db                      # tau-store: runs, messages, costs
  repos/<name>/
    .jj/repo/                 # the one jj repo: op log, index, git store
      store/git/              # internal Git store (non-colocated)
    main/                     # the default workspace, what the user edits
    runs/<run-id>/            # one workspace per run
```

- **Non-colocated.** The model never runs Git and the user reaches the
  repo through the app, so a `.git` beside `.jj` buys little. It costs a
  Git HEAD and index sync on every operation. `Workspace::init_internal_git`
  creates this layout. A user who wants to open the repo in another tool
  can still point `jj` at it, since it is a normal jj repo.
- **Colocated as an option.** `init_colocated_git` is one call away if
  users ask for `git` in the `main` workspace. Only the default workspace
  can be colocated.
- **The repo name** comes from the GitHub path (`owner/repo` becomes
  `owner--repo`), and the UI can rename it.

## Designs for storing chats

The loop reads transcripts from SQLite, and the core cannot depend on
a plugin ([0006](../decisions/0006-plugin-crates.md)). Any design keeps
SQLite as what the loop reads. The question is what jj holds besides
code.

| Design                               | Code history on GitHub  | Atomic with code | Checkout gives chat | Cost                                   |
| ------------------------------------ | ----------------------- | ---------------- | ------------------- | -------------------------------------- |
| A. Transcript files in the code tree | Polluted unless removed | Yes              | Yes                 | Rewrites on push, merge conflicts      |
| B. A separate chat repo              | Clean                   | No               | Via a lookup        | Two op logs, two repos to keep in sync |
| C. Trailers or metadata on commits   | Trailers leak on push   | Yes              | No                  | No custom headers in `CommitBuilder`   |
| D. A second lineage in the same repo | Clean                   | Yes              | Via a lookup        | A tree builder, a bookmark namespace   |
| E. SQLite only, jj holds code        | Clean                   | n/a              | Via a lookup        | Nothing new                            |

- **A** makes every fork diff show transcript churn, and two forks that
  touch the same run file conflict when merged. It also needs a rewrite
  before every push. Rejected.
- **B** is simple to reason about, but the chat commit and the code
  commit land in two operations in two repos. A crash between them
  leaves them out of step.
- **C** does not work as metadata. `CommitBuilder` offers description,
  parents, tree, author, committer and change id, and nothing else.
  Trailers in the description (`Tau-Run:`, `Tau-Seq:`) do work, and
  `jj_lib::trailer` parses them, but they are pushed with the commit.
  Operation metadata is the better place for tags: `Transaction::set_attribute`
  stores key-value pairs in `OperationMetadata::attributes`, and those
  never reach GitHub.
- **D** stores each run's transcript as files (`run.jsonl`,
  `records.jsonl`) in commits whose history starts at the root commit
  and never meets the code history. One `Transaction` writes the code
  snapshot and the chat commit, then commits once, so the two cannot
  drift. The chat DAG has the same shape as the run tree: a fork's first
  chat commit has the fork point's chat commit as parent. Bookmarks live
  under `tau/chat/<run-id>` and are never pushed unless the user asks,
  for example to a private remote to sync machines.
- **E** is what tau has today. Forks already share messages by
  reference, and a checkpoint is `(run, seq)`.

**Proposal:** start with E plus the link records below, and add D in a
later stage when there is a use for chats inside jj: syncing a project
between machines, or reviewing a run's transcript next to its diff in
one history. D is a mirror written by the plugin. SQLite stays the
source of truth.

## Mapping tau's model to jj

| tau                       | jj                                                                  |
| ------------------------- | ------------------------------------------------------------------- |
| Project                   | One repo under `repos/<name>`                                       |
| `RunId`                   | Workspace `run-<uuid>` and bookmark `tau/<short-id>`                |
| Turn end                  | A snapshot: one operation tagged `tau.run`, `tau.seq`, `tau.turn`   |
| Code state at a turn      | The working-copy commit id after that snapshot                      |
| A model-chosen commit     | A change: `vcs_commit` describes it and starts a new one on top     |
| `Checkpoint { run, seq }` | The latest link record with `seq <= checkpoint.seq`                 |
| Fork                      | A new workspace whose working-copy commit is a child of that commit |
| Sub-agent run             | Shares its caller's workspace, or none; it records no links         |
| Compare two runs          | Diff between their heads, or between each head and the fork point   |
| `KeepBranch` (UI)         | Move `tau/<run>` onto the kept head, abandon the siblings' changes  |
| Undo                      | Restore the view of an earlier operation                            |

### Change ids and commit ids

- A **change id** survives rewrites. It names "the thing the model is
  working on", and is what the model sees in `vcs_log` and passes back.
- A **commit id** names one exact snapshot. Links from turns to code
  store commit ids, since the change keeps moving as later turns amend
  it.
- Each turn's snapshot amends the working-copy change, so a run with
  twelve turns and two `vcs_commit` calls has three changes and up to
  twelve snapshot commits. The older snapshots are predecessors in the
  change's evolution log (`jj_lib::evolution`). They are hidden, not
  lost: creating a child of a hidden commit makes it visible again,
  which is how a fork from turn 5 works.
- The alternative, one change per turn, gives a DAG with one node per
  turn and nothing to decide, but pushes dozens of "turn 7" commits.
  The evolution log already keeps every turn, so the model-chosen
  boundary costs nothing in addressability.

### The link record

The plugin writes one record per turn that changed or might have
changed files, through `PluginCtx::record`. `Store::records` already
walks the fork chain and respects `fork_seq`, so a fork's `RunPlan`
receives exactly the links up to its fork point.

```rust
/// Stored with `PluginCtx::record` by the `vcs` plugin.
#[derive(Serialize, Deserialize)]
pub struct Link {
    /// The turn the snapshot closes.
    pub turn: u32,
    /// The working-copy commit after the snapshot.
    pub commit: String,  // hex CommitId
    /// Its change, for display.
    pub change: String,  // reverse-hex ChangeId
    /// The jj operation that recorded it, for undo and audit.
    pub op: String,      // hex OperationId
    /// Set when the chat lineage (design D) is on.
    pub chat_commit: Option<String>,
}
```

- **What stays in SQLite:** runs, messages, costs, token counts,
  statuses, workflow ids, and these links. Everything the UI filters or
  sums.
- **What lives in jj:** file contents, the commit DAG, bookmarks,
  remotes, the operation log, and (with D) a copy of each transcript.
- **Reverse lookups** (which run made this commit) are not served by
  `Store::records`, which reads per run. Two options: the op attribute
  `tau.run` on the operation that wrote the commit, found by walking
  the op log, or a small index table owned by the plugin in its own
  SQLite file. Adding a table to `tau-store` for one plugin goes against
  [0006](../decisions/0006-plugin-crates.md).

## Concurrency: one workspace per run

- jj's operation log is lock-free. Each process or task loads the repo
  at the latest operation, works on a snapshot of the view, and writes a
  new operation. If two operations share a parent, the next load merges
  their views with a 3-way merge, and a bookmark moved two ways becomes
  a bookmark conflict, not an error.
- Each workspace has its own working-copy directory and state, and the
  view records one working-copy commit per workspace. Two runs editing
  files in two workspaces never touch each other's files.
- **A fork always starts a new change.** It must never `edit` a commit
  another run still works on, or both runs rewrite one change and jj
  marks it divergent.
- **Stale working copies.** If the UI rebases a run's change while the
  run is idle, the run's workspace becomes stale. Before its next
  snapshot the plugin must detect this (the working copy's recorded
  operation is not an ancestor of the head) and update the files, as
  `jj workspace update-stale` does **(unverified which jj-lib call the
  CLI uses; it lives in `jj-cli`, not in the library)**. The UI should
  only rewrite a run's commits while that run is not running.
- **In-process serialization.** Within the tau process, one blocking
  worker per repo runs all jj calls for that repo in order. That turns
  most concurrent operations into a linear op log, which is easier to
  show and undo. Other processes (the user's own `jj`) still work,
  through the op-heads merge.

## Undo through the operation log

- Every change to the repo is an operation, with a description,
  timestamps, the workspace name and free attributes. The plugin sets
  `tau.run`, `tau.seq`, `tau.tool` on each one.
- **Restore** (the UI): load the repo at the chosen operation
  (`RepoLoader::load_operation`, `load_at`), start a transaction on the
  current head, and `set_view` to the old view. The working copies of
  affected workspaces then need a checkout. This is `jj op restore`.
- **Undo one operation** (the model, scoped): only if the last
  operation on the head was made by this run, revert it by merging the
  operation's parent view over it, as `jj undo` does. Otherwise refuse,
  so a run can never undo another run's work.
- `op_walk::walk_ancestors` gives the log for the UI's history panel.
- Hidden commits stay in the Git store while an operation that can see
  them is kept. `jj util gc` with a short expiry would drop the
  snapshots old checkpoints point at. The app should not expire
  operations younger than the oldest run it keeps **(unverified how jj
  0.45 protects commits from Git GC; older versions wrote
  `refs/jj/keep/*`)**.

## The jj-lib calls behind each step

Paths are in `jj_lib::`. Signatures are from docs.rs for 0.45.1.

| Step                 | Calls                                                                                                                        |
| -------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| Settings             | `config::StackedConfig`, `settings::UserSettings::from_config` (needs `user.name`, `user.email`)                             |
| Clone                | `Workspace::init_internal_git`, `git::add_remote`, `GitFetch::new`, `GitFetch::fetch`, `GitFetch::import_refs`               |
| Load                 | `Workspace::load(settings, path, &StoreFactories, &WorkingCopyFactories)`, then `repo_loader().load_at_head()`               |
| Add a run workspace  | `Workspace::init_workspace_with_existing_repo`, then `MutableRepo::check_out(name, &parent)` in a transaction                |
| Snapshot             | `Workspace::start_working_copy_mutation`, `LockedWorkingCopy::snapshot(&SnapshotOptions)`, `rewrite_commit(wc).set_tree(..)` |
| Finish a snapshot    | `rebase_descendants`, `Transaction::commit`, `LockedWorkspace::finish(op_id)`                                                |
| Describe             | `rewrite_commit(&c).set_description(..).write()`                                                                             |
| Commit (`jj commit`) | describe the working-copy commit, then `new_commit(vec![id], tree)` and `set_wc_commit`                                      |
| Check out            | `MutableRepo::check_out` (new child) or `edit` (the commit itself), then `Workspace::check_out`                              |
| Rebase               | `rewrite_commit(..).set_parents(..)`, `rebase_descendants_with_options`; helpers in `rewrite`                                |
| Abandon              | `record_abandoned_commit`, `rebase_descendants`                                                                              |
| Bookmarks            | `set_local_bookmark_target`, `get_local_bookmark`, `track_remote_bookmark`                                                   |
| Diff                 | `MergedTree` diff streams, `diff_presentation` for unified output, `copies` for renames                                      |
| Log                  | `revset` parse and evaluate against the repo; `id_prefix` for short ids                                                      |
| Push                 | `git::push_refs(mut_repo, GitSubprocessOptions, remote, &GitPushRefTargets, callback, &GitPushOptions)` (sync)               |
| Op log               | `op_walk::walk_ancestors`, `RepoLoader::load_operation`, `load_at`, `MutableRepo::set_view`                                  |
| Tag an operation     | `Transaction::set_attribute(key, value)`, `set_workspace_name`                                                               |

- **Fetch and push shell out.** `GitSubprocessOptions` carries the path
  to `git` and the environment. Credentials go through that environment:
  `GIT_ASKPASS` pointing at a small helper, or `GIT_CONFIG_COUNT` with a
  `credential.helper` that reads a token the app holds. The packaged app
  must ship `git`; the Nix package for `tau-ui` should add it to the
  wrapper's `PATH`.
- **Snapshot limits.** `SnapshotOptions::max_new_file_size` (jj's
  default is 1 MiB) refuses large new files with `NewFileTooLarge`. The
  plugin should report this to the model as a tool error, naming the
  file, and not fail the run.
- **The revset and template languages** are in the library, but the
  template engine is in `jj-cli`. Output formats for tools are written by
  the plugin.

### Running jj-lib inside tokio

tau is tokio-based. jj-lib's `async fn`s do not need tokio, but much of
their work is synchronous file and object I/O, hashing and `rayon`
parallelism. A sketch:

```rust
/// One per repo. All jj calls for the repo run on its thread, in order.
pub struct RepoWorker {
    tx: mpsc::Sender<Job>,
}

type Job = Box<dyn FnOnce(&mut RepoState) + Send>;

struct RepoState {
    settings: UserSettings,
    root: PathBuf,
    workspaces: HashMap<RunId, Workspace>,
}

impl RepoWorker {
    pub fn spawn(root: PathBuf, settings: UserSettings) -> Self {
        let (tx, mut rx) = mpsc::channel::<Job>(64);
        std::thread::spawn(move || {
            let mut state = RepoState { settings, root, workspaces: HashMap::new() };
            while let Some(job) = rx.blocking_recv() {
                // A panic in jj-lib drops this job's reply channel; the
                // caller sees an error and the worker reloads the repo.
                let _ = std::panic::catch_unwind(AssertUnwindSafe(|| job(&mut state)));
            }
        });
        Self { tx }
    }

    /// Runs `f` on the worker and awaits its result.
    pub async fn call<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut RepoState) -> anyhow::Result<T> + Send + 'static,
    ) -> anyhow::Result<T>;
}
```

Inside a job, async jj-lib calls are driven with `pollster::block_on`,
which jj-lib already depends on. This sidesteps the `Send` question and
keeps jj work off tokio's worker threads.

## The `tau-vcs` plugin

A crate at `crates/plugins/tau-vcs`, named `tau-vcs`, per
[0006](../decisions/0006-plugin-crates.md). It depends on `tau-agent`
and `jj-lib`, and nothing depends on it except the app.

```rust
pub struct Vcs {
    repos: Arc<Repos>,       // RepoWorker per repo, opened lazily
    repo: RepoName,          // the project this agent works in
    policy: Policy,          // which tools, bookmark prefix, size limits
}

#[async_trait]
impl Plugin for Vcs {
    fn name(&self) -> &str { "vcs" }
    fn tools(&self) -> Vec<Arc<dyn AgentTool>>;       // the table below
    async fn start(&self, plan: &mut RunPlan, ctx: &PluginCtx)
        -> anyhow::Result<Box<dyn PluginRun>>;
}
```

- **`start`** reads the inherited links from `plan.records()`. A root
  run gets a new workspace on top of `trunk()` (or a revision the host
  passes). A fork gets a new workspace whose working-copy commit is a
  child of the last inherited link's commit. It adds one line to
  `plan.context`: the workspace path and the current change id.
- **Turn end** (`on_event(TurnEnd)`): snapshot, tag the operation, write
  a `Link` record. A turn with no file changes writes no operation and
  links the same commit again.
- **`finish`**: a last snapshot, then point `tau/<short-id>` at the
  head so the run's work stays visible after its workspace is removed.
- **Coding tools.** `tau-tools` stays unaware of jj. The host roots
  `CodingTools` at the run's workspace path, which the plugin exposes
  to the host (for example through `Repos::workspace_path(run)`), since
  plugin `start` cannot change another plugin's root **(open: needs a
  way for the host to learn the path before the run starts, or a
  plugin that wraps `CodingTools`)**.
- **Guarding the shell.** The model has `bash` when `CodingTools`
  includes it. A `before_tool` in the vcs plugin blocks commands whose
  first word is `git` or `jj`, with a message naming the vcs tools. This
  is a courtesy, not a sandbox: `sh -c` and scripts get around it.

### Tools for the model

All tools act on the run's own workspace. Revisions are change ids,
commit ids, or a fixed set of names: `@` (the working copy), `@-`,
`trunk`, `fork-point` (this run's first link), and `tau/*` bookmarks.
Full revsets are not accepted from the model, so a query cannot be
made arbitrarily expensive.

| Tool           | Arguments                                    | Result                                                                    | jj-lib                                                     |
| -------------- | -------------------------------------------- | ------------------------------------------------------------------------- | ---------------------------------------------------------- |
| `vcs_status`   | none                                         | change id, description, parent, changed paths with kind, conflicted paths | snapshot, then diff `@-`..`@`, `MergedTree` conflict check |
| `vcs_diff`     | `from?`, `to?`, `paths?`, `stat?: bool`      | unified diff or stat, truncated as `read` truncates                       | tree diff stream, `diff_presentation`                      |
| `vcs_log`      | `from?` (default `@`), `limit?` (default 20) | rows: change, commit, first line, author, time, bookmarks, run and turn   | revset `::from`, limited; links for run and turn           |
| `vcs_show`     | `rev`                                        | full description, parents, diff stat                                      | `Commit`, tree diff                                        |
| `vcs_cat`      | `rev`, `path`                                | file contents at that revision, truncated                                 | `MergedTree::path_value`, read the file                    |
| `vcs_describe` | `message`                                    | change id, new commit id                                                  | `rewrite_commit(@).set_description`                        |
| `vcs_commit`   | `message`                                    | the finished change id, the new empty change id                           | describe `@`, `new_commit([@])`, `set_wc_commit`           |
| `vcs_new`      | `parents: [rev]`, `message?`                 | new change id; files now match the parents                                | `new_commit`, `check_out`, `Workspace::check_out`          |
| `vcs_restore`  | `paths`, `from?` (default `@-`)              | restored paths                                                            | tree merge of paths from `from` into `@`, then check out   |
| `vcs_abandon`  | `rev`                                        | abandoned change, rebased descendants count                               | `record_abandoned_commit`, `rebase_descendants`            |
| `vcs_squash`   | `into?` (default `@-`)                       | the target change id                                                      | rewrite target with `@`'s tree, abandon `@`                |
| `vcs_undo`     | none                                         | the operation undone                                                      | revert the head op if its `tau.run` is this run            |

- **Every write is scoped.** `vcs_describe`, `vcs_abandon`,
  `vcs_squash` and `vcs_undo` refuse commits that are not this run's
  (not reachable from the run's head and not after its fork point) and
  commits that are immutable (`trunk()`, anything on a remote).
- **Every write snapshots first**, so edits made with `write` or `bash`
  are never lost to a checkout.
- **Results are text for the model** and a JSON `details` value for the
  UI, the way `tau-tools` returns them.
- **Conflicts are data.** jj keeps conflicts in commits. `vcs_status`
  lists conflicted paths, and the files hold conflict markers the model
  edits like any text.
- **Execution mode.** All vcs tools are `ExecutionMode::Sequential`, so
  a batch of calls runs in order.

### Operations the user triggers from the UI

These change shared state, reach the network, or throw work away. They
go through `WorkspaceEvent`s to the host, which calls `tau-vcs`
directly, not through the model.

| Operation         | What it does                                                                             |
| ----------------- | ---------------------------------------------------------------------------------------- |
| Add project       | Clone a GitHub repo into `repos/<name>`: init, add remote, fetch, import, check out main |
| Fetch             | `GitFetch` for the remote, import refs; update `trunk`                                   |
| Push              | Push `tau/<run>` (or a renamed bookmark) to `origin`; optionally open a PR               |
| Check out a point | Open a read-only workspace at a link's commit, or show its tree without a workspace      |
| Fork from a point | `WorkspaceEvent::Fork` at any turn, not only the latest: a checkpoint at that link's seq |
| Compare           | Diff two runs' heads, or each against the fork point                                     |
| Keep branch       | `KeepBranch`: keep one fork's bookmark, abandon the siblings' changes, remove workspaces |
| Rebase onto trunk | Rebase an idle run's changes onto the fetched trunk                                      |
| Operation log     | Browse, restore to an operation, undo                                                    |
| Resolve conflicts | Show conflicted files; resolution is a normal edit                                       |
| Remove a project  | Delete the repo directory after confirming                                               |

`WorkspaceEvent` in `tau-ui` has `Fork { run }` and `KeepBranch { run }`
today. A fork at an earlier turn needs a `seq` field, and the other
operations need new variants, all handled by the host
([0007](../decisions/0007-gpui-interface.md)).

## Risks

- **API churn.** A minor release every month, and 0.x semantics. The
  plugin should confine jj-lib to one module behind its own small
  `Repo` type, so an upgrade touches one place.
- **Panics in the library.** A jj-lib panic inside the app process takes
  the UI down unless caught. The worker thread design limits this to a
  failed job.
- **Weight.** About 270 crates, mostly gitoxide. Build time and
  `cargo deny` exceptions grow. Only the app and `tau-vcs` pay for it.
- **A runtime `git` dependency** for fetch and push, and credential
  handling through environment variables.
- **Record timing.** A link written at `TurnEnd` gets a `seq` after the
  turn's messages. A checkpoint taken at the turn's last message would
  then miss that turn's link and fork from the previous one **(open:
  check where `Outcome::last_seq` and `TurnEnd` fall relative to plugin
  records; the fix may be to write the link from `after_tool`, or to
  let the loop store records with the turn)**.
- **Snapshots and large repos.** A snapshot walks the working copy. On
  large repos, doing this every turn may cost seconds. The `watchman`
  feature exists but pulls tokio and a watchman daemon.
- **Ignored and huge files.** Build outputs the model creates are
  tracked unless `.gitignore` covers them. The 1 MiB new-file limit
  stops the worst cases.
- **Hidden commits and GC.** Checkpoints point at hidden snapshot
  commits. A GC policy that expires operations can make old forks
  impossible.
- **Divergence.** A bug that lets two runs rewrite one change creates
  divergent changes the model cannot reason about. The scope checks
  on write tools are the guard.
- **The shell escape.** Blocking `git` and `jj` in `bash` is best
  effort. A model that runs `git commit` in a workspace confuses the
  snapshot, but the next snapshot picks up the file state anyway.

## Open questions

- Can `Workspace` and `ReadonlyRepo` futures run on tokio directly
  (are they `Send`), or is the worker thread required?
- How does the host learn a run's workspace path before `CodingTools`
  is built? Options: the host creates the workspace before starting the
  run and passes it to both plugins, or `tau-vcs` wraps `CodingTools`.
- Should a sub-agent that edits code get its own workspace (a fork of
  its caller's state) or share its caller's? Sharing is simpler, but two
  runs then snapshot one working copy.
- Does the chat lineage (design D) belong in the first release, or only
  once cross-machine sync is wanted?
- Is `git.change-id` (the `change-id` Git header jj writes since 0.30)
  something we want on pushed commits? It keeps change ids across
  clones, and GitHub keeps the header **(unverified for GitHub)**.
- GitHub auth: reuse `gh`'s token, a GitHub App, or a token the user
  pastes into the UI?
- Should `tau-vcs` also work without jj for users who only want Git?
  This note assumes no.

## Staged plan

1. **Spike (a few days).** A throwaway binary on jj-lib 0.45.1: clone a
   GitHub repo with an internal Git store, add two workspaces, write
   files in both, snapshot both, check that the op log merges, fork a
   third workspace from a hidden snapshot, push a bookmark. Answer the
   `Send` and stale-workspace questions.
2. **`tau-vcs` read path.** The crate, the repo worker, `Repos` under
   the XDG dir, and the read tools: `vcs_status`, `vcs_diff`, `vcs_log`,
   `vcs_show`, `vcs_cat`. Tests on temporary repos, no network.
3. **Snapshots and links.** Workspace per run, snapshot at turn end,
   `Link` records, forks from links, op attributes. Settle the record
   timing question. Property tests: a fork from any link sees exactly
   that tree.
4. **Write tools.** `vcs_describe`, `vcs_commit`, `vcs_new`,
   `vcs_restore`, `vcs_abandon`, `vcs_squash`, `vcs_undo`, with the scope
   checks, plus the `bash` guard.
5. **UI operations.** Add project, fetch, push, fork at a turn,
   compare, keep branch, op log. New `WorkspaceEvent` variants in
   `tau-ui`, handled by the host. `git` in the Nix package.
6. **Chat lineage (optional).** Design D: chat commits written in the
   same transaction as the snapshot, `tau/chat/*` bookmarks, an optional
   private remote for sync.
7. **Decision record.** Once the spike and stage 3 hold up, write
   `docs/decisions/0008-...` with what was chosen.

## Sources

- jj-lib on crates.io (versions, license, features, dependencies):
  <https://crates.io/crates/jj-lib>
- jj-lib 0.45.1 API: <https://docs.rs/jj-lib/0.45.1/jj_lib/>
  - `git` module: <https://docs.rs/jj-lib/0.45.1/jj_lib/git/index.html>
  - `git::push_refs`: <https://docs.rs/jj-lib/0.45.1/jj_lib/git/fn.push_refs.html>
  - `git::GitFetch`: <https://docs.rs/jj-lib/0.45.1/jj_lib/git/struct.GitFetch.html>
  - `git::GitSubprocessOptions`: <https://docs.rs/jj-lib/0.45.1/jj_lib/git/struct.GitSubprocessOptions.html>
  - `workspace::Workspace`: <https://docs.rs/jj-lib/0.45.1/jj_lib/workspace/struct.Workspace.html>
  - `repo::MutableRepo`: <https://docs.rs/jj-lib/0.45.1/jj_lib/repo/struct.MutableRepo.html>
  - `repo::RepoLoader`: <https://docs.rs/jj-lib/0.45.1/jj_lib/repo/struct.RepoLoader.html>
  - `transaction::Transaction`: <https://docs.rs/jj-lib/0.45.1/jj_lib/transaction/struct.Transaction.html>
  - `commit_builder::CommitBuilder`: <https://docs.rs/jj-lib/0.45.1/jj_lib/commit_builder/struct.CommitBuilder.html>
  - `working_copy::LockedWorkingCopy`: <https://docs.rs/jj-lib/0.45.1/jj_lib/working_copy/trait.LockedWorkingCopy.html>
  - `working_copy::SnapshotOptions`: <https://docs.rs/jj-lib/0.45.1/jj_lib/working_copy/struct.SnapshotOptions.html>
  - `op_store::OperationMetadata`: <https://docs.rs/jj-lib/0.45.1/jj_lib/op_store/struct.OperationMetadata.html>
  - `op_walk`: <https://docs.rs/jj-lib/0.45.1/jj_lib/op_walk/index.html>
  - `settings::UserSettings`: <https://docs.rs/jj-lib/0.45.1/jj_lib/settings/struct.UserSettings.html>
- jj-core 0.45.1: <https://docs.rs/jj-core/0.45.1/jj_core/>
- Jujutsu concurrency design: <https://docs.jj-vcs.dev/latest/technical/concurrency/>
- Jujutsu architecture: <https://docs.jj-vcs.dev/latest/technical/architecture/>
- Jujutsu configuration (git subprocess, colocation, snapshot limits):
  <https://docs.jj-vcs.dev/latest/config/>
- Panics in the library, issue #5685: <https://github.com/jj-vcs/jj/issues/5685>
- The `change-id` Git header, PR #6162: <https://github.com/jj-vcs/jj/pull/6162>
- Dependency count and licenses: measured locally with `cargo tree` and
  `cargo metadata` on a crate depending only on `jj-lib = "0.45.1"`.
