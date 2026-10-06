# tau-ui

tau on the computer: the host that runs coding agents in repositories
cloned from GitHub, drives `tau-ui-remote`'s interface, and serves the
phones paired with it. The interface itself (the workspace, its screens,
the plugins' views) lives in `tau-ui-remote`; this crate is everything
behind it that runs agents, signs in, clones, lands, pushes and notifies.
It is a library: the `tau` app (`crates/tau`) is the binary.

## What it provides

| Module              | What it holds                                                                                     |
| ------------------- | ------------------------------------------------------------------------------------------------- |
| `host`              | `Host`, the wiring for a real coding agent, `HostConfig`, and `onboard` when no one is signed in  |
| `host::queue`       | A main chat's landing queue: `Lane` decides, the host acts                                        |
| `hosted`            | The plugins' host halves: `halves` gives them to `tau-ui-remote`'s plugins, `install` once        |
| `accounts`          | Sign in with ChatGPT: `Credentials` (where sign-ins are kept) and `SignIns`                       |
| `github`            | GitHub through tau's GitHub App: device sign-in or a token, the account, the repositories (`Api`) |
| `phone_server`      | The computer's side of phones: `serve`, once "Allow phones" is on                                 |
| `notify`            | Desktop notifications over D-Bus while the window is not focused: `Notifier`, `Desktop`           |
| `metered`           | Jev requests counted for the Plugins screen: `Metered`                                            |
| `interface_runtime` | A tokio runtime for the interface's own async work, such as signing in                            |
| `demo`              | A scripted session, for running the interface without an agent                                    |

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
| `$XDG_DATA_HOME/tau/repos`            | The projects                                          |
| `$XDG_DATA_HOME/tau/repos.json`       | The repositories tau lists                            |
| `$XDG_CONFIG_HOME/tau/models.json`    | The person's model choices                            |
| `$XDG_CONFIG_HOME/tau/interface.json` | Interface settings (`reduce_motion`, `notifications`) |
| `~/.agents/skills`                    | The person's skills (tau-skills)                      |

`$XDG_DATA_HOME` falls back to `~/.local/share`. ChatGPT sign-ins sit in
`$XDG_CONFIG_HOME/tau/chatgpt`, and the GitHub token in `github.json` beside
them.

## How it fits

It builds on `tau-agent`, `tau-ai`, `tau-store` and `tau-store-sqlite`,
`tau-remote`, `tau-ui-kit`, `tau-ui-plugin`, `tau-ui-remote`, and on every
plugin. It is the host half's user: `hosted::halves` gives each plugin
`tau-ui-remote` lists its host half (`tau-tools-host`, `tau-vcs-host`,
`tau-memory-host`, `tau-constitution-host`, `tau-mcp-host`, and the light
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
- `tests/github.rs`: GitHub sign-in against a fake GitHub.
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
