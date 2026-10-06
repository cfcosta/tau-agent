# tau-vcs-host

Version-control tools for tau agents, backed by jj-lib. The model
reaches version control only through these tools and never runs `jj` or
`git` in a shell. The crate also holds what tau does with repositories
around a run: a project cloned from the user's repository, a jj
workspace per run, landing a run's commits on its parent, sub-agents,
sweeping what old runs left, and pushing.

## What it provides

| Item                                       | What it is                                                                                     |
| ------------------------------------------ | ---------------------------------------------------------------------------------------------- |
| `Vcs`                                      | A handle on one jj workspace: `Vcs::open`, `Vcs::init` (internal Git store), `Vcs::lazy`       |
| `Identity`                                 | The author of the commits and operations; defaults to `tau <tau@localhost>`                    |
| `VcsPlugin`                                | The tools on one `Vcs`, as a `Plugin` for `Agent::plugin`                                      |
| `tools`                                    | One `TypedTool` per tool (`Status`, `Diff`, `Commit`, `Land`, ...)                             |
| `ProjectRepo`, `Project`                   | A repository tau owns, with a workspace per run; `Project` runs its jobs off the async runtime |
| `RunWorkspace`, `Link`                     | A run's own workspace, snapshotted after each turn so forks start from that turn's code        |
| `Spawn`, `Wait`, `SubAgents`               | The `spawn` and `wait` tools, and the sub-agents running beside their caller                   |
| `RefusingSpawn`, `RefusingWait`            | The same tools on runs below the main chat: declared so caches match, refusing every call      |
| `sweep`                                    | Which workspaces and bookmarks of finished runs go (`plan`, `ProjectRepo::sweep`)              |
| `Remote`, `Pushed`                         | Where `ProjectRepo::push_trunk` and `push_branch` push, and what they pushed                   |
| `VcsError`, `CloneError`, `TransferError`  | What goes wrong; `VcsError` is what the tools hand the model                                   |
| `VcsHost`                                  | tau-vcs's `HostHalf`, for tau's plugin registry                                                |
| `ChangeInfo`, `FileChange`, `Landing`, ... | The result shapes, re-exported from `tau-vcs`                                                  |

`VcsPlugin::new(vcs)` adds ten tools:

- Reading: `vcs_status`, `vcs_diff`, `vcs_log`, `vcs_show`.
- Writing: `vcs_describe`, `vcs_commit`, `vcs_new`, `vcs_restore`,
  `vcs_resolve`, `vcs_undo`.

`read_only()` keeps the four reading tools. `landing()` adds
`vcs_land`, for a run that lands on its parent or on trunk when it
finishes. `refusing_landing(refusal)` declares `vcs_land` but fails
every call, so a run's tools match those of runs that land. The plugin
also marks what `@` changes in each `ls` listing.

The tools take change ids and commit ids, never revsets. Every tool
snapshots the working copy first, so edits made with other tools are
never lost. The writing tools refuse immutable commits. New files over
`MAX_NEW_FILE_SIZE` (1 MiB) stay out of the snapshot.

## How it fits

This is the host half (decisions 0017, 0030): the behaviour that runs
inside the agent. Its partner is [tau-vcs](../tau-vcs), which draws the
tools' cards and owns the shapes they return.

- Builds on `tau-agent`, `tau-ai`, `tau-ui-plugin`, `tau-vcs`, jj-lib,
  and gix (HTTPS clones without the `git` binary).
- Used by `tau-ui` and `tau-luau-plugins-host`.

`VcsHost` adds no agent plugin itself. The host builds `VcsPlugin` with
each run's workspace.

Cloning and fetching over HTTPS run in process, with gix. Pushing goes
through jj-lib, which runs `git`; this is the one place tau runs it. The
push token reaches `git` only through the `TAU_GIT_TOKEN` variable of
the child process, read by a credential helper.

## Usage

```rust
use tau_agent::agent::Agent;
use tau_tools_host::{path::Root, plugin::CodingTools};
use tau_vcs_host::{Identity, Vcs, VcsPlugin};

// An existing jj workspace. `Vcs::init` makes a new repository instead.
let dir = "/path/to/workspace";
let vcs = Vcs::open(dir, Identity::default()).await?;

let agent = Agent::new(llm)
    .plugin(CodingTools::new(Root::new(dir)))
    .plugin(VcsPlugin::new(vcs));
```

Root the coding tools at the same directory so the model edits the
files the vcs tools snapshot. Every run of this agent works in the one
workspace `vcs` opened. tau itself gives each run its own workspace
with `ProjectRepo` and `RunWorkspace`.

## Testing

```sh
cargo nextest run --release -p tau-vcs-host
```

- The tests build fixture repositories with the `git` binary
  (`tau_testing::git`), so `git` must be on `PATH`. They never read the
  user's Git settings.
- Many suites are Hegel model-based tests (`model`, `runs_model`,
  `workspaces_model`, `update_model`, `concurrent_runs`, and more).
  Their long variants are `#[ignore = "nightly"]`.
- One test in `tests/project.rs` clones a public repository from GitHub
  and is `#[ignore = "needs the network"]`.
- `tests/agent.rs` runs the tools inside a real run, with
  `ScriptedModel` choosing the calls.

Run the ignored tests with:

```sh
cargo nextest run --release -p tau-vcs-host --run-ignored only
```

## Further reading

- [Version-control tools reference](../../../docs/reference/vcs.md)
- [VCS handoff and conflict properties](../../../docs/reference/vcs-hardening-tests.md)
- [0009: Child runs land on their parent's stack, then close](../../../docs/decisions/0009-child-runs-land-on-their-parent.md)
- [0014: The model commits, and every run lands as a stacked diff](../../../docs/decisions/0014-the-model-commits-and-runs-land-as-stacked-diffs.md)
- [0015: A main chat per repository, and repositories from GitHub](../../../docs/decisions/0015-a-main-chat-per-repository.md)
- [0015: Delegates fork their caller, and run side by side](../../../docs/decisions/0015-delegates-fork-their-caller.md)
- [0016: Runs nest one level](../../../docs/decisions/0016-runs-nest-one-level.md)
- [0023: Main pushes with git; chat pull requests replay onto origin](../../../docs/decisions/0023-main-pushes-with-git-chat-prs-replay-onto-origin.md)
- [0024: Landings queue while the parent works](../../../docs/decisions/0024-landings-queue-while-the-parent-works.md)
- [0026: Sub-agents run detached: spawn and wait](../../../docs/decisions/0026-sub-agents-run-detached.md)
- [0028: Async all the way; blocking only in spawn_blocking](../../../docs/decisions/0028-async-all-the-way-blocking-only-in-spawn-blocking.md)
- [0030: Host halves are crates](../../../docs/decisions/0030-host-halves-are-crates.md)
