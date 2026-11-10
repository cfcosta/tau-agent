# tau-ui

tau on the computer: the host that runs coding agents in repositories
cloned from the forge (tau-github fills its slot), drives `tau-ui-remote`'s interface, and mirrors it to
the phones tau-phones pairs with it. The interface itself (the workspace, its screens,
the plugins' views) lives in `tau-ui-remote`; this crate is everything
behind it that runs agents, signs in, clones, lands, pushes and notifies.
It is a library: the `tau` app (`crates/tau`) is the binary.

## What it provides

| Module        | What it holds                                                                                          |
| ------------- | ------------------------------------------------------------------------------------------------------ |
| `host`        | `Host`, the wiring for a real coding agent, and `HostConfig`; it starts signed out or in               |
| `host::queue` | A main chat's landing queue: `Lane` decides, the host acts                                             |
| `hosted`      | The plugins' host halves: `halves` gives them to `tau-ui-remote`'s plugins, `install` once             |
| `accounts`    | `Credentials`: where sign-ins are kept; tau-chatgpt signs in                                           |
| `mirror`      | The window as phones mirror it: what crosses, what a phone may ask, the `tau.app` service              |
| `notify`      | When to notify while the window is not focused (`Notifier`), through the `tau.notify` slot (`Slotted`) |
| `metered`     | Jev requests counted for the Plugins screen: `Metered`                                                 |

`Host` owns a tokio runtime beside GPUI's executor. A new run starts on that
runtime; a task reads its events and sends them to the workspace on the UI
thread. Each repository is cloned into a project under
`$XDG_DATA_HOME/tau/repos`, with a main chat per repository, and each run
gets a jj workspace there with a commit per turn. Runs land on their parent
as stacked diffs, queue while the parent works, and push to GitHub. The
host adds the repository's `AGENTS.md` to a run's instructions.

Where it keeps its files (`HostConfig`):

| Path                                  | What                                                  |
| ------------------------------------- | ----------------------------------------------------- |
| `$XDG_DATA_HOME/tau/runs.db`          | The run store                                         |
| `$XDG_DATA_HOME/tau/uploads`          | The images the person sent, by key, for interfaces    |
| `$XDG_DATA_HOME/tau/repos`            | The projects                                          |
| `$XDG_DATA_HOME/tau/repos.json`       | The repositories tau lists                            |
| `$XDG_CONFIG_HOME/tau/models.json`    | The person's model choices                            |
| `$XDG_CONFIG_HOME/tau/interface.json` | Interface settings (`reduce_motion`, `notifications`) |
| `~/.agents/skills`                    | The person's skills (tau-skills, a Luau package)      |

`$XDG_DATA_HOME` falls back to `~/.local/share`. ChatGPT sign-ins sit in
`$XDG_CONFIG_HOME/tau/chatgpt`, and tau-github, a Luau plugin, keeps the
GitHub token as its `github` secret. Signing in to GitHub is tau-github's: a setup
step before the repositories and a section of the Models screen; the host
lists the forge's repositories for setup whenever the catalog says the forge
is newly signed in.

## How it fits

It builds on `tau-agent`, `tau-ai`, `tau-store` and `tau-store-sqlite`,
`tau-remote`, `tau-ui-kit`, `tau-ui-plugin`, `tau-ui-remote`, and on every
plugin. It is the host half's user: `hosted::halves` gives each plugin
`tau-ui-remote` lists its host half (`tau-tools-host`, `tau-vcs-host`,
`tau-mcp-host`, and the light
ones in the plugins' own crates). The Luau plugins' halves
(`tau-codemode-host`, `tau-luau-plugins-host`) are left out so this crate
builds while Luau's C++ does; the `tau` app adds them.

It turns on the `terminal` feature of `tau-tools` and `tau-tools-host`, so
`bash` runs under a pseudo-terminal.

Only `crates/tau` depends on it.

## Usage

Run the app from the repository's root:

```sh
cargo run --release -p tau                       # sign in, then run agents
cargo run --release -p tau -- --demo             # the scripted session
cargo run --release -p tau -- --demo --open plugins
cargo run --release -p tau -- --demo --phone     # the phone layout
```

With a ChatGPT sign-in that allows plan use, `tau` runs real agents;
without one, it opens onboarding. `--open` takes a screen name from
`tau_ui::demo::SCREENS`. The other flags (`--model`, `--prompt`,
`--finished`, `--frame`, `--steps`, `--reduce-motion`) are documented at
the top of `crates/tau/src/main.rs`. `nix run` builds and runs the same
app.

Pushing runs `git` through jj-lib, so `git` must be on `PATH`.

## Testing

```sh
cargo nextest run --release -p tau-ui
```

The tests drive the real host and the workspace in GPUI's test app, with
scripted models, so they need no sign-in and no network:

- `tests/host.rs`, `tests/landing.rs`, `tests/queue.rs`,
  `tests/forecast.rs` and `tests/restart.rs`: runs, forks, landing, the
  landing queue and restarts, on real jj repositories in temporary
  directories.
- `tests/push.rs`: pushing and pull requests, against a bare repository
  through a `file://` URL and a fake GitHub API. It runs `git`.
- `tests/direnv.rs`: tau-direnv with a fake `direnv` first on `PATH`; it
  sets `PATH` and the XDG directories once for the whole binary.
- `tests/phones.rs` and `tests/sync_model.rs`: a computer and phones in one
  process; `sync_model` is a Hegel model test of what every interface must
  show.
- `tests/queue_model.rs` and `tests/notify.rs`: Hegel tests of the landing
  queue and of when tau notifies.
- `tests/workspace.rs`, `tests/ask.rs`, `tests/direnv_ui.rs` and
  `tests/update.rs`: onboarding, pull requests, plugin flows, and round
  trips of what host and interface send each other.

## Further reading

- [docs/reference/chatgpt-sign-in.md](../../docs/reference/chatgpt-sign-in.md)
- [docs/reference/attention.md](../../docs/reference/attention.md)
- [docs/reference/vcs.md](../../docs/reference/vcs.md)
- [docs/reference/environment.md](../../docs/reference/environment.md)
- [docs/reference/plugins.md](../../docs/reference/plugins.md)
- [docs/decisions/0007-gpui-interface.md](../../docs/decisions/0007-gpui-interface.md)
- [docs/decisions/0013-phones-connect-to-a-running-tau.md](../../docs/decisions/0013-phones-connect-to-a-running-tau.md)
- [docs/decisions/0014-the-model-commits-and-runs-land-as-stacked-diffs.md](../../docs/decisions/0014-the-model-commits-and-runs-land-as-stacked-diffs.md)
- [docs/decisions/0015-a-main-chat-per-repository.md](../../docs/decisions/0015-a-main-chat-per-repository.md)
- [docs/decisions/0020-the-repository-s-agents-file.md](../../docs/decisions/0020-the-repository-s-agents-file.md)
- [docs/decisions/0023-main-pushes-with-git-chat-prs-replay-onto-origin.md](../../docs/decisions/0023-main-pushes-with-git-chat-prs-replay-onto-origin.md)
- [docs/decisions/0024-landings-queue-while-the-parent-works.md](../../docs/decisions/0024-landings-queue-while-the-parent-works.md)
- [docs/decisions/0026-sub-agents-run-detached.md](../../docs/decisions/0026-sub-agents-run-detached.md)
- [docs/decisions/0028-async-all-the-way-blocking-only-in-spawn-blocking.md](../../docs/decisions/0028-async-all-the-way-blocking-only-in-spawn-blocking.md)
- [docs/decisions/0030-host-halves-are-crates.md](../../docs/decisions/0030-host-halves-are-crates.md)
- [docs/decisions/0031-the-main-chat-orchestrates.md](../../docs/decisions/0031-the-main-chat-orchestrates.md)
