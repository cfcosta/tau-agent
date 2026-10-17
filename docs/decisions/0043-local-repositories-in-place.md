# 0043: Local repositories work in place, beside the person's own work

- Status: proposed. Amends
  [0015](0015-a-main-chat-per-repository.md): a repository can come
  from this computer again, and is worked in where it is. Amends
  [0023](0023-main-pushes-with-git-chat-prs-replay-onto-origin.md): a
  local repository pushes only where the person chose, and fetches
  only when asked. Builds on [0014](0014-the-model-commits-and-runs-land-as-stacked-diffs.md)
  and [0042](0042-sub-agents-can-start-fresh-and-keep-their-work-aside.md).
- Date: 2026-10-17

## Context

Repositories come from GitHub alone since change `zynomvoz`. Before it,
tau copied a local checkout into its own storage, and when the copy
failed, ran in the checkout without a jj project, which broke forks,
landing and snapshots. People keep work on their machine that is not
on GitHub, or want tau's work to show up in the folder they already
use.

This time the folder is the project. tau sets up jj in it, colocated
with Git, and gives runs workspaces of their own outside it. That puts
tau and the person in one repository, writing at the same time, with
two tools (jj-lib in tau, the jj or git CLI for the person). Most of
this decision is about keeping each out of the other's way.

The UI was iterated on a design canvas (entry A, one Add screen with
GitHub and "On this computer"; entry B, the sidebar's + menu with a
small dialog).

## Decision

### Adding

- A local repository is a Git or jj repository on this computer,
  added from the Add screen or the sidebar's + menu, on the desktop
  only (phones list it, and say folders are added at the computer).
- Adding shows what tau found (repository kind, checked-out branch,
  remote, uncommitted files) and what it checked, then sets up jj
  beside Git with progress, all or nothing: a failed setup removes
  the `.jj` tau made, and the folder is as before.
- tau refuses a repository that uses Git LFS or submodules: runs would
  see pointer files or empty folders. Git hooks and clean/smudge
  filters are ignored: they do not run on tau's commits.
- tau refuses a Git worktree (it offers the main checkout) and a folder
  inside a repository (it offers the repository).
- tau follows the person's jj config, global and the repository's,
  as the jj CLI does, in every repository, clones included. Not doing
  so is a bug. This was fixed before local repositories came back:
  config layers and `--when` scopes, commit author and signing,
  `immutable_heads()` and the revset aliases, snapshot settings
  (`snapshot.auto-track`, `max-new-file-size`, Git's excludes),
  conflict markers, diff context, `git.sign-on-push`,
  `git.private-commits`, and the `trunk()` alias a clone writes.
  `docs/reference/vcs.md` lists the settings tau leaves aside, each
  with its reason (fetch auto-tracking, `snapshot.auto-update-stale`,
  presentation settings, and others).
- jj version skew between tau's jj-lib and the person's jj is not
  checked: jj's own compatibility rules apply.

### Where things live

- The project root is the person's folder. jj's `default` workspace is
  the person's.
- The main chat works in a named workspace of its own, in every
  repository, clones included: `default` is never tau's. A local
  repository can instead put the main chat in the person's checkout,
  where the two share one working copy.
- Runs work in workspaces under tau's data directory, one per run.
  Each is also a Git worktree of the repository, so `git`, nix flakes
  and build scripts that ask Git work in runs; this holds for clones
  too.
- The person names untracked paths (for example `.env`, `.direnv`)
  that tau links into each run's workspace. Nothing by default.
- direnv trust follows the repository: allowing it in the folder
  allows it in that repository's workspaces.
- Plugin data (memory, settings) keys on a stable repository id, not
  on the path.

### Staying out of the person's way

- tau never moves Git's `HEAD` or index in a local repository. It
  exports refs only. When trunk is about to move while the person has
  it checked out in Git (`HEAD` attached to trunk), tau detaches
  `HEAD` on its current commit first, as jj does, and the status bar
  says trunk moved.
- Run heads live in a private namespace, `refs/tau/*`, not as
  branches: `git branch` and `git push --all` never see them.
- tau sets the repository's default log revset to leave out run
  commits that have not landed. Its operations stay in the operation
  log, each described `tau: …`.
- When the person and tau both move trunk, the person's commit wins:
  tau rebases what it landed on top, as a landing onto a moved trunk
  does, and conflicts go to the main chat.
- tau watches the operation log for operations it did not write, and
  reloads: trunk, counts, landing forecasts and the sidebar follow the
  person's commits, fetches and rebases.
- When the person removes tau's state (a workspace forgotten, a run's
  head abandoned, a `refs/tau/` ref deleted), that run is recorded as
  dropped. The startup sweep removes only what it still finds.

### Remote

- Where pushes go is chosen per repository as it is added: GitHub when
  `origin` is on GitHub and tau's GitHub App can push there (checked
  at add time; an SSH origin maps to HTTPS), else nowhere, which hides
  Push and pull requests until a remote is set.
- A local repository fetches only when asked: from the repository's
  menu, or by the main chat with a new tool, `vcs_pull`. There is no
  startup fetch, and landing never pulls.
- `vcs_pull` (main chat only; chats and sub-agents get a refusal, as
  `vcs_land` on main does) fetches origin and moves trunk to it as a
  fast-forward, or rebases main's own unpushed commits on top;
  conflicts land for the main chat to resolve.

### When the folder moves or goes

- A repository whose folder is missing (moved, renamed, deleted, on a
  disk not mounted) is marked missing; runs refuse to start. Relink
  points it at the new path and repairs the workspaces' paths; a
  folder that comes back is picked up again.

### Removing

- Remove from tau closes the repository's chats and takes away what
  tau added: its workspaces (and their folders), `refs/tau/*`, and the
  log revset it set. The folder, its branches and its commits stay,
  and the operation log keeps what tau did. Removing `.jj` too is a
  separate choice.

## Consequences

- One rule for every repository: the main chat in a workspace of its
  own and run workspaces that are Git worktrees change clones as well,
  and remove the code paths that assume the main chat is `default`
  (`DEFAULT_WORKSPACE`, 22 places).
- jj-lib 0.45.1 has no API for a jj workspace that is also a Git
  worktree. tau creates the worktree itself with gix (there is no git
  binary) and keeps its `HEAD` and index in step, as `colocate.rs`
  does for `main/` today.
- tau's commits carry the person's name and are signed when their
  config signs. A signing setup that prompts would block tau's
  commits; it surfaces as a failed commit.
- What is immutable is what jj says: the person's `immutable_heads()`.
  A tracked remote bookmark that is not trunk no longer protects its
  commits, and a broken definition stops tau's writes with an error
  naming it, as it stops jj's.
- Files outside `snapshot.auto-track` stay out of runs' commits, as
  ignored files do, and are lost with a run's workspace.
- The person and tau share one operation log; `jj undo` by the person
  can undo a tau landing. The op log watcher sees it, and tau reloads.
- Local repositories never push or fetch on their own, so they can
  fall behind origin until someone pulls.
