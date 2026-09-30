# 0015: A main chat per repository, and repositories from GitHub

- Status: accepted. Amends [0014](0014-the-model-commits-and-runs-land-as-stacked-diffs.md):
  what a session is a child of.
- Date: 2026-09-30

## Context

[0014](0014-the-model-commits-and-runs-land-as-stacked-diffs.md) made
runs one tree rooted at trunk, with each session a child of trunk.
Sessions then have no shared conversation: every chat starts from
nothing, and what one chat learned about the repository never reaches
the next one. Nothing in the sidebar says where the tree starts either.

tau also listed the directory it started in, and let a local path be
added. Runs in such a repository worked on a copy of the checkout, or
on the checkout itself when copying failed. That second path breaks
everything built on projects (forks, landing, snapshots), and a
checkout has no remote that pull requests and updates can go to.

## Decision

### Repositories come from GitHub

- tau lists only repositories cloned from GitHub. The directory tau
  starts in is not listed, and a local path cannot be added: "Add
  repository" opens the GitHub picker.
- A clone that cannot be made a project is an error. Runs refuse to
  start in it, and the status bar names it. Runs never work in a
  checkout.

### Each repository has a main chat

- **Main is made with the repository**, when it is cloned or first
  listed: a root run that starts empty and finished, titled `main`.
  `repos.json` keeps its id.
- **Main stays open.** It cannot be closed, and it heads its
  repository in the sidebar, before anything else.
- **A message to main goes on with it**, like any finished chat.
- **Every new chat is a fork of main**: at main's latest turn, with
  its conversation and on that turn's code, or at its start, on trunk,
  while main has no turn. The sidebar lists the chats under main,
  newest first, with the older ones behind "Show older runs".
- **Chats land on main** the way forks land on their run (0009).
  Main that has not taken a turn gets its workspace, on trunk, from
  the first chat that lands on it.

The tree is now trunk, then main, then the chats and their own forks
and sub-agents. What 0014 says about sessions and trunk (Merge, the
session resolving its conflicts) applies to main.

## Alternatives considered

- **One main chat for all repositories.** One place to talk, but a
  chat works in one repository, and main's code has to be that
  repository's.
- **Chats listed under main without its conversation.** Cheaper
  context, and chats that do not inherit what main discussed. The
  point of main is that its conversation reaches every chat, so they
  fork it.
- **Keep local checkouts beside GitHub.** Local-only repositories
  would keep working, but on a second path through the host that
  forks, landing and pull requests do not cover.

## Consequences

- `HostConfig` loses `root`, and tau-ui its `--root` flag. Hosts built
  elsewhere, as in tests, list a project with `Host::with_repo`.
- `Host::start` forks main; `Host::main_of` makes main the first time.
- Each chat's context starts with main's whole conversation, so a long
  main makes every new chat more expensive. Compaction applies as
  anywhere else.
- History always loads each main, however old.
- A run's title placeholder comes from its own first message, not one
  it inherited: `Store::first_prompt`.
