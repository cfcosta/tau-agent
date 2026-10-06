# tau-direnv

Runs an agent's commands in the repository's direnv environment, as
they would run in the person's shell. When a workspace has an `.envrc`,
the person is asked once per repository whether tau loads it. Once
allowed, each workspace's environment loads in the background, and
`bash`'s commands and the repository's MCP servers start through
`direnv exec <workspace>`. tau never runs `direnv allow` or edits the
person's own direnv files: it writes a configuration of its own that
whitelists the repositories the person allowed.

## What it provides

| Item                 | What it is                                                                                                 |
| -------------------- | ---------------------------------------------------------------------------------------------------------- |
| `DirenvUi`           | The plugin's `UiPlugin`: the question, loading and failure cards, menu toggle, settings pane               |
| `Record`             | What the plugin publishes about a run's workspace: `Asked`, `Loading`, `Loaded`, `Failed`, `Denied`, `Off` |
| `Settings`           | The person's answer per repository (`repos`)                                                               |
| `RepoData`           | Whether a repository has an `.envrc`, and whether direnv is on `PATH`                                      |
| `Act`                | What the UI asks the host half: `Decide` for a repository, `Reload` a failed load                          |
| `NAME`               | `"tau-direnv"`, the name both halves go by                                                                 |
| `DirenvHost`         | The `HostHalf`: the agent plugin and the launcher (feature `host`)                                         |
| `host::Host`         | The host state: direnv, the answers, each workspace's status (feature `host`)                              |
| `launch::Direnv`     | The `direnv` program and where tau keeps its files; `Direnv::find` looks on `PATH`                         |
| `launch::Status`     | Where a workspace's environment stands                                                                     |
| `launch::launch_for` | Pure: how a command in a workspace starts, given direnv, the answer and the status                         |
| `config`             | tau's `direnv.toml`: the person's, with allowed repositories in `[whitelist] prefix`                       |

## How it fits

tau-direnv is a single crate holding both halves. The interface half
(records, their fold, the views) always builds. The host half sits
behind the default `host` feature, so `tau-ui-remote` and a phone build
it with `default-features = false` (decisions 0017, 0030).

- Builds on `tau-agent` (`launch::Launch`, `Launcher`, `Plugin`),
  `tau-ui-plugin` and `tau-ui-kit`.
- Used by `tau-ui`, which registers `tau_direnv::DirenvHost`, and
  `tau-ui-remote`, which lists `tau_direnv::DirenvUi`.

The host half gives a repository a `tau_agent::launch::Launcher` through
`HostHalf::launcher`. The host hands it to tau-tools' `bash`
(`CodingTools::with_launcher`) and to tau-mcp's stdio servers. Commands
wait while the person is asked and while the environment loads. A
failed load, a `direnv deny` or a "no" runs commands as they are.
Sub-agents' commands go through it too, but they get no cards.

## Usage

`launch_for` decides how one command starts. Only an installed direnv,
an allowed repository and a loaded workspace go through `direnv exec`:

```rust
use std::path::Path;
use tau_direnv::launch::{Direnv, Status, launch_for};

let direnv = Direnv::find(Path::new("/path/to/tau-data/plugins/tau-direnv"));
let workspace = Path::new("/path/to/workspace");
let launch = launch_for(direnv.as_ref(), Some(true), &Status::Ready, workspace);
let (program, args) = launch.argv("/bin/sh".as_ref(), ["-c", "cargo build"]);
```

In tau, `DirenvHost` does this for every command.

## Features

| Feature | Default | Effect                                                                                |
| ------- | ------- | ------------------------------------------------------------------------------------- |
| `host`  | on      | The host half: `DirenvHost`, `host`, `launch` and `config`, with tokio, sha2 and toml |

## Testing

```sh
cargo nextest run --release -p tau-direnv
```

- `tests/host.rs` runs the host half against a fake direnv,
  `tests/fixtures/fake-direnv.sh` (a POSIX shell script that uses
  `sha256sum`). It is Unix only.
- `tests/real_direnv.rs` uses the real direnv when it is on `PATH`, and
  skips itself otherwise. direnv's data directory is the test's own, so
  the person's allow and deny records are never touched.
- `config`'s unit tests are Hegel property tests of the `direnv.toml`
  merge.

## Further reading

- [Agent commands' environment](../../../docs/reference/environment.md)
- [Plugins](../../../docs/reference/plugins.md)
- [0025: Agent commands run in an environment plugins give](../../../docs/decisions/0025-agent-commands-run-in-an-environment-plugins-give.md)
- [0029: Every plugin's settings in a pane of their own](../../../docs/decisions/0029-every-plugins-settings-in-a-pane.md)
- [0030: Host halves are crates](../../../docs/decisions/0030-host-halves-are-crates.md)
