# tau

tau is a coding agent that runs on your Linux desktop and that you can
also steer from an Android phone. It works in your GitHub repositories:
each chat gets its own jj workspace, the model commits its own work, and
that work lands as stacked changes you can push to GitHub. It runs on
your ChatGPT plan through Sign in with ChatGPT, so there are no API keys.

![A chat in tau: the transcript with its tool cards, beside the repositories and their chats](docs/screenshots/run.png)

## A tour

### The main chat leads

Each repository has a main chat. Main splits a request into tasks,
spawns up to 8 sub-agents to work on them side by side, and checks each
one as it lands. Its crew sits above the composer while they work. Every
other chat is a fork of main, with main's conversation and code.

![Main's crew: two sub-agents at work above the composer](docs/screenshots/crew.png)

### Work lands as stacked diffs

Each chat works in its own jj workspace and the model commits its own
work, with its own messages. A finished chat lands on its parent: its
commits are rebased on top, and a conflict goes to the parent's next
turn to resolve. While the parent is working, landings wait in a queue.

<p>
  <img src="docs/screenshots/land.png" alt="A fork landing on its parent, with a conflict" width="49%">
  <img src="docs/screenshots/land-queue.png" alt="Two chats waiting to land on main" width="49%">
</p>

Forks of the same checkpoint can be compared side by side: turns, tokens,
cost, tests and code. The model reaches version control only through
its own tools, which leave cards you can open.

<p>
  <img src="docs/screenshots/compare.png" alt="Two forks compared side by side" width="49%">
  <img src="docs/screenshots/vcs.png" alt="A finished run with its version-control cards" width="49%">
</p>

### GitHub

Main pushes to GitHub. A chat can open a pull request, with a
description written from the run.

<p>
  <img src="docs/screenshots/pull-request.png" alt="Opening a pull request from a chat" width="49%">
  <img src="docs/screenshots/pushed.png" alt="The card of a push to origin/main" width="49%">
</p>

### Talking with the agent

The agent can ask you structured questions, with a preview for each
choice. You can always write your own answer instead.

![A question from the agent, with previews, in place of the composer](docs/screenshots/ask.png)

It can show you images and Markdown documents on a canvas beside the
chat, and every revision of a document is kept. You can send it images
too: paste them, drop them on the composer, or attach them.

<p>
  <img src="docs/screenshots/canvas-image.png" alt="An image on the canvas" width="49%">
  <img src="docs/screenshots/canvas-document.png" alt="A document on the canvas, at an earlier revision" width="49%">
</p>

![An image sent to the agent, and another waiting in the composer](docs/screenshots/uploads.png)

### Agents that keep going

An agent is a conversation that never ends, in a repository of its own.
Schedules, polls and streams wake it, it keeps to a daily budget, and it
can rewrite its own `agent.luau` as it learns. It leaves you questions
and notes on the Agents page.

<p>
  <img src="docs/screenshots/agents.png" alt="The Agents page" width="49%">
  <img src="docs/screenshots/agent.png" alt="An agent's conversation, with what wakes it" width="49%">
</p>

A plain chat can schedule itself to wake up once, later, with a note to
itself.

![A chat that scheduled itself a look at a pull request](docs/screenshots/scheduled.png)

### On your phone

The phone runs no agent. It pairs with the tau on your computer by QR
code, over your LAN or a VPN such as Tailscale, and shows and steers the
same runs with the same screens.

![The computer's Phones screen, with its pairing code](docs/screenshots/phones.png)

<p>
  <img src="docs/screenshots/phone-agents.png" alt="The runs and agents on a phone" width="19%">
  <img src="docs/screenshots/phone-run.png" alt="A chat on a phone" width="19%">
  <img src="docs/screenshots/phone-ask.png" alt="A question from the agent on a phone" width="19%">
  <img src="docs/screenshots/phone-canvas.png" alt="The canvas on a phone" width="19%">
  <img src="docs/screenshots/phone-pair.png" alt="Pairing a phone by QR code" width="19%">
</p>

### Plugins

Most of tau is plugins. Each has a half every screen draws (the phone
links it) and a half on the computer that does the work, and each has
its settings in a pane on the Plugins page. You can write your own in
Luau: `/plugin` and what it should do, in any chat. tau keeps them in a
repository of its own, tests them, and loads each change once it lands.

<p>
  <img src="docs/screenshots/plugins.png" alt="The built-in plugins and their settings" width="49%">
  <img src="docs/screenshots/plugins-yours.png" alt="Your own plugins, one waiting for your approval" width="49%">
</p>

What the built-in plugins add:

- **Codemode.** A tool whose input is a Luau script that calls the
  run's other tools as functions, to chain calls, run them in parallel
  and filter large results before the model reads them.

  ![A codemode call, with each tool it called and its output](docs/screenshots/codemode.png)

- **Long-term memory.** A Zettelkasten of typed, linked Markdown notes
  that runs search and write. A run starts with the notes that fit its
  task. What Claude Code and Codex learned on this computer can come
  along: memory reads what they saved and their chats, and proposes
  notes you keep or drop.

  ![A repository's memory notes](docs/screenshots/memory.png)

- **The constitution.** A repository's rules. Jev checks every tool
  call and final answer against them, and a call that breaks one is
  blocked and sent back to the model, as in the first screenshot.

  ![A repository's rules](docs/screenshots/constitution.png)

- **MCP.** Connects runs to MCP servers and adds their tools. A
  repository's own servers wait for your approval.

  ![A repository's MCP servers](docs/screenshots/mcp.png)

- **Fast compaction.** Prunes large tool outputs and stale tool history
  with Jev, keeping what stays word for word, and shows what it dropped
  and why. Tree compaction and summarizing compaction take over when
  pruning is not enough.

  ![Fast compaction's ledger for a run](docs/screenshots/ledger.png)

- **direnv.** Runs the agent's commands in the repository's direnv
  environment, once you allow it.

  ![tau asking whether to load a repository's .envrc](docs/screenshots/direnv.png)

- **Skills.** Instructions from `~/.agents/skills`, which the model
  loads when a task calls for one.

  ![A skill the model loaded](docs/screenshots/skill.png)

- **`/goal`.** Keeps a run going until a condition holds, checked with
  Jev, within a budget you set.

  ![A goal on a run, not met yet](docs/screenshots/goal.png)

- **The watcher.** Off by default. Now and then, a short note on what
  you likely missed.

  ![The watcher's note on what changed](docs/screenshots/watcher.png)

- **Reasoning.** Picks each message's reasoning effort from its task,
  with Jev.

  ![The reasoning effort picked for a message, and the memory notes recalled for it](docs/screenshots/reasoning.png)

Several plugins ask Jev, TypeSafe's System One model, for small and
cheap decisions.

### Finding your way

History lists every run in every repository, with what it cost, and
search reaches any run, chat or action.

<p>
  <img src="docs/screenshots/history.png" alt="History" width="49%">
  <img src="docs/screenshots/search.png" alt="Search" width="49%">
</p>

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

<p>
  <img src="docs/screenshots/welcome.png" alt="The first screen of onboarding" width="49%">
  <img src="docs/screenshots/model.png" alt="Connecting tau to ChatGPT" width="49%">
</p>

`--model <id>` picks the model (gpt-6.1-sol by default), and
`--prompt <text>` starts a chat with that task. The demo replays a
scripted session with no sign-in and no agent:

```sh
cargo run --release -p tau -- --demo
cargo run --release -p tau -- --demo --phone         # the phone layout
cargo run --release -p tau -- --demo --open canvas   # one screen
```

[crates/tau/README.md](crates/tau/README.md) lists every flag. The
screenshots here come from the demo: `scripts/readme-screenshots.sh`
takes them again.

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

tau is in active development and has no release yet.

## License

tau is under the [MIT license](LICENSE). Parts of it come from pi and
other projects, under their own licenses:
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) lists them.
