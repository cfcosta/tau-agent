# heiko

heiko is a coding agent that runs on your Linux desktop and that you can
also steer from an Android phone. It works in your GitHub repositories:
each chat gets its own jj workspace, the model commits its own work, and
that work lands as stacked changes you can push to GitHub. It runs on
your ChatGPT plan through Sign in with ChatGPT, so there are no API keys
and the interface shows no dollar amounts.

The name is 並行 (heikō), "in parallel": several agents work side by
side.

![A chat in heiko: the transcript with its tool calls, beside the sidebar's rail and the repository's chats](docs/screenshots/run.png)

The screenshots here were taken before the interface was redrawn
([ADR 0057](docs/decisions/0057-the-interface-is-flat-and-printed.md))
and show the earlier look.

## A tour

### Home and the sidebar

Home is where a run starts: what to do, where it starts and on which
model, and Start. Beside it is a list of what happened since you last
looked, across every repository and agent: a chat that asks you
something, one ready to land, one that failed, changes not pushed yet.
Each has the one thing to do about it.

The sidebar is a rail of tiles, one per repository and one per agent,
and beside it the tree of the one you chose: its chats, with their
forks and sub-agents under them, and what plugins keep for it. A small
mark at a row's end says what state it is in, and a tile shows when
something inside needs you.

In a chat, what you wrote and what the model answered lead. A tool call
is one line, with its name, its arguments and what came back, and it
opens to show the rest.

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
tests and code. The model reaches version control only through
its own tools, whose calls you can open in the chat.

<p>
  <img src="docs/screenshots/compare.png" alt="Two forks compared side by side" width="49%">
  <img src="docs/screenshots/vcs.png" alt="A finished run with its version-control calls" width="49%">
</p>

### GitHub

Main pushes to GitHub. A chat can open a pull request, with a
description written from the run.

<p>
  <img src="docs/screenshots/pull-request.png" alt="Opening a pull request from a chat" width="49%">
  <img src="docs/screenshots/pushed.png" alt="A push to origin/main, in its chat" width="49%">
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
can rewrite its own `src/agent.luau` as it learns. It leaves you questions
and notes on the Agents page.

<p>
  <img src="docs/screenshots/agents.png" alt="The Agents page" width="49%">
  <img src="docs/screenshots/agent.png" alt="An agent's conversation, with what wakes it" width="49%">
</p>

A plain chat can schedule itself to wake up once, later, with a note to
itself.

![A chat that scheduled itself a look at a pull request](docs/screenshots/scheduled.png)

### On your phone

The phone runs no agent. It pairs with the heiko on your computer by QR
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

Most of heiko is plugins. Each has a half every screen draws (the phone
links it) and a half on the computer that does the work, and each has
its settings in a pane on the Plugins page. You can write your own in
Luau: `/plugin` and what it should do, in any chat. heiko keeps them in a
repository of its own, tests them, and loads each change once it lands.
A plugin's notes in a chat can draw small graphics from the
interface's own pieces: ticks, marks, stamps, squares and slips.

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

  ![heiko asking whether to load a repository's .envrc](docs/screenshots/direnv.png)

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

History lists every run in every repository, by day, and
search reaches any run, chat or action.

<p>
  <img src="docs/screenshots/history.png" alt="History" width="49%">
  <img src="docs/screenshots/search.png" alt="Search" width="49%">
</p>

## Requirements

- Linux, on x86_64 or aarch64. Building the phone app also needs
  x86_64.
- A ChatGPT plan that allows use through Sign in with ChatGPT.
- A GitHub account. heiko works only in repositories cloned from GitHub.
- For the plugins that ask Jev: `TYPESAFE_API_KEY` in the environment.
- `git` on `PATH`, since pushes go through it. The Nix package brings
  its own.

## Running it

From a checkout:

```sh
nix run                                  # the packaged app
cargo run --release -p heiko               # the same, inside `nix develop`
```

heiko is not one file: it reads its Luau plugins at startup from
`share/heiko/plugins` beside the binary, or from `plugins/` in the checkout it
was built from, or from `HEIKO_PLUGINS_DIR` (ADR 0051). Without them it starts
no agents, and says why. `heiko-plugin test plugins/heiko-goal` runs one's tests
with nothing to compile, and `heiko-plugin check` loads them all (ADR 0053).

On first start, heiko asks you to sign in with ChatGPT and with GitHub.
After that, "Add repository" picks a repository from GitHub to clone.

<p>
  <img src="docs/screenshots/welcome.png" alt="The first screen of onboarding" width="49%">
  <img src="docs/screenshots/model.png" alt="Connecting heiko to ChatGPT" width="49%">
</p>

`--model <id>` picks the model (gpt-6.1-sol by default), and
`--prompt <text>` starts a chat with that task. The demo replays a
scripted session with no sign-in and no agent:

```sh
cargo run --release -p heiko -- --demo
cargo run --release -p heiko -- --demo --phone         # the phone layout
cargo run --release -p heiko -- --demo --open canvas   # one screen
```

[crates/heiko/README.md](crates/heiko/README.md) lists every flag. The
screenshots here come from the demo: `scripts/readme-screenshots.sh`
takes them again.

### On a phone

```sh
nix build .#heiko-phone-apk      # result/heiko-phone-debug.apk
nix run .#heiko-phone-install    # installs it on the phone adb sees
```

Turn on "Allow phones" on the computer's Phones screen, then scan the QR
code it shows from the phone. [crates/heiko-phone/README.md](crates/heiko-phone/README.md)
has the details, including how to build without Nix.

### Where heiko keeps things

| Path                       | What                                              |
| -------------------------- | ------------------------------------------------- |
| `~/.local/share/heiko/repos` | The clones, with a jj workspace per chat          |
| `~/.local/share/heiko/`      | The run store, uploads, and the plugins' own data |
| `~/.config/heiko/`           | Sign-ins, model choices and interface settings    |
| `~/.agents/skills`         | Your skills, shared with other agents             |

These follow `$XDG_DATA_HOME` and `$XDG_CONFIG_HOME` when they're set.
[crates/heiko-ui/README.md](crates/heiko-ui/README.md) has the full list.

## Development

The workspace pins a nightly toolchain and uses Rust edition 2024.
`nix develop` provides it, along with the native libraries GPUI and the
terminal need.

```sh
nix develop
cargo run --release -p heiko
cargo nextest run --release    # the tests: no sign-in or network needed
```

The tests drive the real host and interface with scripted models and
fake OpenAI, ChatGPT and GitHub servers. The evaluations under
`crates/evals/` are separate, and most of them make real model calls.

### How it fits together

| Layer         | Crates                                                                                |
| ------------- | ------------------------------------------------------------------------------------- |
| The app       | `heiko` (desktop), `heiko-phone` (Android)                                                |
| The interface | `heiko-ui-remote` (every screen, shared by desktop and phone), `heiko-ui-kit` (the look)  |
| The host      | `heiko-ui`: runs agents, clones, lands, pushes, notifies, serves phones                 |
| Plugins       | `crates/plugins/`: `heiko-<name>` for what screens draw, `heiko-<name>-host` for the work |
| Plugin seams  | `heiko-ui-plugin`: `UiPlugin`, `HostHalf`, extension points                             |
| Phones        | `heiko-remote`: pairing, the TLS identity, the wire                                     |
| The agent     | `heiko-agent` (the loop, tools, plugins), `heiko-ai` (OpenAI over WebSocket)              |
| Storage       | `heiko-store`, `heiko-store-sqlite`, `heiko-artifacts`                                      |
| Support       | `heiko-terminal` (command output in a real terminal), `heiko-testing`                     |

Each crate has a README. [docs/architecture.md](docs/architecture.md)
covers how the crates connect, [docs/decisions/](docs/decisions/) records
why each fixed decision was made, and [docs/reference/](docs/reference/)
documents each part's exact behaviour.

The agent loop, its event model and tool validation are ported from
[pi](https://github.com/earendil-works/pi). The command tools follow
Codex's unified exec.

## Status

heiko is in active development and has no release yet.

## License

heiko is under the [MIT license](LICENSE). Parts of it come from pi and
other projects, under their own licenses:
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) lists them.
