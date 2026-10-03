# 0025: Agent commands run in an environment plugins give

- Status: accepted. Amends [tools.md](../reference/tools.md)'s `bash`,
  whose commands inherited tau-ui's environment and nothing else: they
  now start through what plugins give the repository. tau-direnv is the
  first plugin to give one. Adds the `REPO_MENU` extension point.
  Reference: [environment.md](../reference/environment.md).
- Date: 2026-10-03

## Context

`bash` ran its commands with tau-ui's own environment. A repository
that sets up its toolchain with direnv (tau-agent's `.envrc` says `use
flake`) had none of it unless tau-ui itself was started from `nix
develop` in that repository, which works for one repository at a time
and not at all for tau started from a desktop launcher. The person's
shell loads the `.envrc` as they `cd`; the agent's did not.

direnv is the person's own setup, and loading an `.envrc` runs code
from the repository. The person allows it once per `.envrc` with
`direnv allow`; tau's workspaces are new directories every chat, so
that allow never covers them, and writing to the person's allow
database on their behalf would be tau deciding for them.

## Decision

### An extension point for commands' environment

- `tau_agent::launch`: a `Launch` (the words before the program, the
  variables over its environment) and a `Launcher` that answers one for
  a directory and may wait first.
- `UiPlugin::launcher(host, repo, settings)` gives a repository's. The
  host joins every plugin's in the registry's order and hands the
  result to `bash` (pipes and terminal; tau-terminal takes a generic
  `Command::through`) and, through `RunCtx::services` (`RepoLauncher`),
  to tau-mcp, whose stdio servers of the repository start through it in
  the main workspace. With no plugin giving one, nothing changes.
- A command waits for its launcher within its own timeout; a cancel
  ends the wait. Nothing starts until the launcher answers.
- `points::REPO_MENU` takes a plugin's entries on a repository's menu.

### tau-direnv, the first provider

- When direnv is on `PATH` and a run's workspace has an `.envrc`, the
  person is asked once per repository, in the composer's place, as
  tau-ask asks ([0019](0019-ask-the-person.md)): "Load it" or "Run
  without it". It is the person's decision, never the model's. The
  answer is the plugin's setting, per repository, and changes on the
  repository's menu.
- Allowed, each workspace loads in the background (`direnv export
json`); a card shows while it loads, and one if it failed, with Try
  again. Commands then start as `direnv exec <workspace> <shell> -c
<command>`; a failed load runs them without it.
- tau points `DIRENV_CONFIG` at its own directory: the person's
  `direnv.toml` with the allowed repositories' directories added to
  `[whitelist] prefix`, and links to their `lib/` and `direnvrc`. The
  person's allow and deny records are untouched, and a `direnv deny`
  still holds.
- Without direnv the plugin is off, and the menu says direnv is not
  installed. tau does not bundle direnv.

## Consequences

- A repository like tau-agent gets its toolchain in every chat without
  tau being started from `nix develop`.
- The first command of each workspace waits for its load: seconds when
  a dev shell is built already, longer when it must build. Each command
  after costs a `direnv exec` (tens of milliseconds with nix-direnv's
  cache).
- `.direnv/` goes to tau's data, not the workspace (`direnv_layout_dir`),
  since tau commits everything a workspace holds.
- A change to the flake inside a chat is picked up by `direnv exec` on
  the next command. If it breaks the environment, those commands fail
  with direnv's error until it is fixed or the person stops loading it.
- The repository's MCP servers start in the main workspace's
  environment: before main's first turn, that workspace may not have
  the `.envrc` yet, and they start without it.
