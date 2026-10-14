# tau

tau is a coding agent that runs on your Linux desktop and that you can
also steer from an Android phone. It works in your GitHub repositories:
each chat gets its own jj workspace, the model commits its own work, and
that work lands as stacked changes you can push to GitHub. It runs on
your ChatGPT plan through Sign in with ChatGPT, so there are no API keys.

## What it does

- **A main chat per repository, which leads.** Main splits a request
  into tasks, spawns up to 8 sub-agents to work on them side by side,
  and checks each one as it lands. Every other chat is a fork of main,
  with main's conversation and code.
- **Work lands as stacked diffs.** Each chat works in its own jj
  workspace and commits with its own messages. When it finishes, its
  commits are rebased onto its parent, waiting in a queue while the
  parent is still working. Main pushes to GitHub, and a chat can open
  a pull request.
- **The phone is a second screen.** The phone runs no agent. It pairs
  with the tau on your computer by QR code, over your LAN or a VPN such
  as Tailscale, and shows and steers the same runs, with the same
  screens.
- **Show and tell.** The agent can ask you structured questions, and
  can show you images and Markdown documents in a canvas beside the
  chat. You can send it images: paste them, drop them, or attach them.
- **Agents that keep going.** An agent is a conversation that never
  ends, in a repository of its own. Schedules, polls and streams wake
  it, and it can rewrite its own `agent.luau`. A plain chat can also
  schedule itself to wake up once, later.
- **Plugins.** Most of tau is plugins. Each one has a half every screen
  draws (a phone links it) and a half on the computer that does the
  work. You can write your own in Luau: tau keeps them in a repository
  of its own and loads each change once it lands.

The plugins tau ships with:

| Area               | Plugins                                                                                                   |
| ------------------ | --------------------------------------------------------------------------------------------------------- |
| Tools              | Coding tools (`read`, `edit`, `exec_command`, …), version control on jj, Codemode (Luau that calls tools) |
| Talking to you     | `ask` for questions, the canvas, the watcher's notes on what you likely missed                            |
| Context            | Fast compaction, tree compaction, summarizing compaction, long-term memory                                |
| Keeping runs going | `/goal`, chats that wake later, agents                                                                    |
| The repository     | Its rules (the constitution), MCP servers, direnv, skills from `~/.agents/skills`                         |
| Tuning             | Picks each message's reasoning effort                                                                     |

Several plugins ask Jev, TypeSafe's System One model, for small and
cheap decisions.

## Requirements

- Linux, on x86_64 or aarch64. Building the phone app also needs
  x86_64.
- A ChatGPT plan that allows use through Sign in with ChatGPT.
- A GitHub account. tau works only in repositories cloned from GitHub.
- For the plugins that ask Jev: `TYPESAFE_API_KEY` in the environment.
- `git` on `PATH`, since pushes go through it. The Nix package brings
  its own.

## Running it

From a checkout:

```sh
nix run                                  # the packaged app
cargo run --release -p tau               # the same, inside `nix develop`
```

On first start, tau asks you to sign in with ChatGPT and with GitHub.
After that, "Add repository" picks a repository from GitHub to clone.

`--model <id>` picks the model (gpt-6.1-sol by default), and
`--prompt <text>` starts a chat with that task. The demo replays a
scripted session with no sign-in and no agent:

```sh
cargo run --release -p tau -- --demo
cargo run --release -p tau -- --demo --phone         # the phone layout
cargo run --release -p tau -- --demo --open canvas   # one screen
```

[crates/tau/README.md](crates/tau/README.md) lists every flag.

### On a phone

```sh
nix build .#tau-phone-apk      # result/tau-phone-debug.apk
nix run .#tau-phone-install    # installs it on the phone adb sees
```

Turn on "Allow phones" on the computer's Phones screen, then scan the QR
code it shows from the phone. [crates/tau-phone/README.md](crates/tau-phone/README.md)
has the details, including how to build without Nix.

### Where tau keeps things

| Path                       | What                                              |
| -------------------------- | ------------------------------------------------- |
| `~/.local/share/tau/repos` | The clones, with a jj workspace per chat          |
| `~/.local/share/tau/`      | The run store, uploads, and the plugins' own data |
| `~/.config/tau/`           | Sign-ins, model choices and interface settings    |
| `~/.agents/skills`         | Your skills, shared with other agents             |

These follow `$XDG_DATA_HOME` and `$XDG_CONFIG_HOME` when they're set.
[crates/tau-ui/README.md](crates/tau-ui/README.md) has the full list.

## Development

The workspace pins a nightly toolchain and uses Rust edition 2024.
`nix develop` provides it, along with the native libraries GPUI and the
terminal need.

```sh
nix develop
cargo run --release -p tau
cargo nextest run --release    # the tests: no sign-in or network needed
```

The tests drive the real host and interface with scripted models and
fake OpenAI, ChatGPT and GitHub servers. The evaluations under
`crates/evals/` are separate, and most of them make real model calls.

### How it fits together

| Layer         | Crates                                                                                |
| ------------- | ------------------------------------------------------------------------------------- |
| The app       | `tau` (desktop), `tau-phone` (Android)                                                |
| The interface | `tau-ui-remote` (every screen, shared by desktop and phone), `tau-ui-kit` (the look)  |
| The host      | `tau-ui`: runs agents, clones, lands, pushes, notifies, serves phones                 |
| Plugins       | `crates/plugins/`: `tau-<name>` for what screens draw, `tau-<name>-host` for the work |
| Plugin seams  | `tau-ui-plugin`: `UiPlugin`, `HostHalf`, extension points                             |
| Phones        | `tau-remote`: pairing, the TLS identity, the wire                                     |
| The agent     | `tau-agent` (the loop, tools, plugins), `tau-ai` (OpenAI over WebSocket)              |
| Storage       | `tau-store`, `tau-store-sqlite`, `tau-artifacts`                                      |
| Support       | `tau-terminal` (command output in a real terminal), `tau-testing`                     |

Each crate has a README. [docs/architecture.md](docs/architecture.md)
covers how the crates connect, [docs/decisions/](docs/decisions/) records
why each fixed decision was made, and [docs/reference/](docs/reference/)
documents each part's exact behaviour.

The agent loop, its event model and tool validation are ported from
[pi](https://github.com/earendil-works/pi). The command tools follow
Codex's unified exec.

## Status

tau is in active development and has no release yet. No license has been
chosen.
