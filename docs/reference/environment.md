# Agent commands' environment

What agent commands start through, and tau-direnv, the plugin that runs
them in the repository's direnv environment. Crates: `tau-agent`
(`launch`), `crates/plugins/tau-direnv`. Decision:
[0025](../decisions/0025-agent-commands-run-in-an-environment-plugins-give.md).

## The extension point

- `tau_agent::launch::Launch { prefix, env }`: the process started is
  `prefix… program args…`, with `env` set over what it inherits.
  `Launch::default()` starts the program as it is.
- `Launcher::launch(dir)` answers the `Launch` for a command in `dir`.
  It may wait; callers bound the wait. `Launchers` joins several: their
  words in order, their variables in order (a later one wins).
- `UiPlugin::launcher(host, repo, settings)` gives a repository's.
  tau-ui's host joins every plugin's in the registry's order
  (`Host::launcher_of`).
- Consumers:
  - `bash` (`CodingTools::with_launcher`), with pipes and under a
    terminal (`tau_terminal::Command::through`). The command's timeout
    counts the wait: past it, the result is `timed_out` with "Command
    timed out waiting for the repository's environment to load", and
    nothing ran. A cancel while waiting is `Command aborted`.
  - tau-mcp: the run's services carry the repository's `RepoLauncher`
    (the joined launcher and the main workspace); the repository
    scope's pool keeps it, and each stdio server starts through it, in
    the main workspace. The entry's own `env` comes after the
    launcher's.
- `RunCtx::services` also carries the run's `WorkspaceDir`, and
  `RepoCtx::workspaces` is the directory that holds a repository's
  workspaces.

## tau-direnv

### When it applies

- direnv is on tau's `PATH` (`Direnv::find`). Without it the plugin
  gives no launcher, and the repository menu's entry says "direnv is
  not installed".
- The workspace a command runs in has an `.envrc`. Without one,
  commands start as they are and nothing waits.

### Asking

- The first time a run's workspace in a repository needs it and the
  person has not decided, the run hears `asked { repo, envrc }`: its
  composer's place shows "Load <repo>'s .envrc for agent commands?",
  the `.envrc` (its first 4 KiB), "Load it" and "Run without it".
  Commands wait for the answer.
- The answer is the plugin's setting `{ "repos": { "<repo>": true } }`,
  changed from the panel or the repository menu's toggle "Load .envrc
  for agent commands" (`Act::Decide { repo, load }`). Turning it off
  stops loading every workspace of the repository (`off`); on loads
  them.

### Loading

- A workspace loads once a session, in the background, on the host's
  runtime: `direnv status --json` (a `foundRC.allowed` of 2, a denied
  `.envrc`, is `denied`; 1 is a failure, "direnv did not allow it"),
  then `direnv export json`.
- The runs in it hear `loading { since }` (a card, "Loading the
  repository's environment · .envrc · m:ss", "commands wait for it"),
  then `loaded`, or `failed { status, output }`: "direnv exited N" and
  direnv's last 12 lines, escapes removed, with Try again
  (`Act::Reload { run }`). Commands run without it until it loads.
- Records are stored with the run and pushed to the interface at once
  (`HostCx::publish`), from the run's first turn: no run event flows
  while a command waits.

### Commands

- Loaded and allowed (`launch_for`): `direnv exec <workspace> <shell>
-c <command>`, with `DIRENV_CONFIG=<tau's>`, `DIRENV_LOG_FORMAT=`
  (direnv's own lines silenced; the `.envrc`'s output still shows) and
  `direnv_layout_dir=<tau's data>/plugins/tau-direnv/layouts/<hash>`.
  Anything else: as it is.

### tau's direnv configuration

`<tau's data>/plugins/tau-direnv/config`, written at start and on
every decision:

- `direnv.toml`: the person's (`$DIRENV_CONFIG`, else
  `$XDG_CONFIG_HOME/direnv`, else `~/.config/direnv`; `direnv.toml`,
  else `config.toml`) with every key kept and each allowed
  repository's workspaces directory added once to `[whitelist]
prefix`. A file that does not parse fails the loads.
- `lib` and `direnvrc`: links to the person's, when they have them, so
  nix-direnv still loads.
- The person's allow and deny records (`$XDG_DATA_HOME/direnv`) are
  read by direnv as always and never written by tau.

## Tests

- `tau-agent`: `Launch::argv`, and `Launchers` joining in order.
- `tau-terminal`: a launcher's words run before the program.
- `tau-tools` (`tests/bash_launch.rs`): the words and variables reach
  the command, pipes and terminal; no launcher, or a direct one,
  changes nothing; a held command starts once answered; the wait
  counts against the timeout; a cancel ends it.
- `tau-mcp` (`tests/stdio.rs`): a stdio server starts through the
  launcher, asked for the main workspace.
- `tau-direnv`: the `direnv.toml` merge (every key kept, the person's
  prefixes then each root once, idempotent), the `launch_for` table,
  the host against a fake direnv (`tests/fixtures/fake-direnv.sh`), and
  the real direnv when it is on `PATH`, with its data in a temporary
  directory.
- `tau-ui`: `tests/direnv.rs` (the host with the fake first on `PATH`)
  and `tests/direnv_ui.rs` (the question, cards and menu entry).
