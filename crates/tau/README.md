# tau

tau on the computer: the desktop app. It opens `tau-ui-remote`'s interface,
starts `tau-ui`'s host behind it, and links every plugin's host half. With a
ChatGPT sign-in that allows plan use, it runs a real coding agent in the
repositories cloned from GitHub. Without one, it opens onboarding to sign in.

## What it provides

| Item        | What it is                                                                 |
| ----------- | -------------------------------------------------------------------------- |
| `tau` (bin) | The app: parses its flags, starts a `Host` when signed in, opens a window  |
| `plugins`   | Every plugin `tau-ui-remote` lists, each with its host half, as a Registry |
| `install`   | Installs `plugins` as the process's, once, before anything starts a host   |

The binary's flags:

| Flag              | What it does                                                          |
| ----------------- | --------------------------------------------------------------------- |
| `--model <id>`    | The model; gpt-6.1-sol by default                                     |
| `--prompt <text>` | Starts a run with this task right away                                |
| `--demo`          | Replays the scripted session instead of running agents                |
| `--finished`      | Opens the demo already done                                           |
| `--open <screen>` | A demo screen from `tau_ui::demo::SCREENS`, or `page:<plugin>/<page>` |
| `--phone`         | The phone layout in a 390×844 frame                                   |
| `--frame <w>x<h>` | Lays out at exactly that size, to compare with the designs            |
| `--steps <n>`     | Stops the demo script after its first `n` updates                     |
| `--reduce-motion` | No looping motion, and transitions as plain fades                     |

With a sign-in, `--open` takes only a plugin's page, such as
`page:tau-mcp/servers?repo=` for the MCP servers outside a repository.
`TAU_REDUCE_MOTION=1` asks for reduced motion too (`0` asks for all of it),
then `"reduce_motion": true` in `interface.json`, then the desktop's
`enable-animations` setting.

## How it fits

It is the top of the stack and nothing depends on it. It builds on
`tau-ui` (the host), `tau-ui-remote` (the interface), `tau-ui-plugin` (the
registry) and GPUI. `tau_ui::hosted::halves` gives most plugins their host
halves. This crate adds the two Luau ones, `tau-codemode-host` and
`tau-luau-plugins-host`, which are linked here and nowhere above, so
`tau-ui` builds while Luau's C++ does (ADR 0030).

The phone build is a separate crate, `tau-phone`. It brings its own
platform, so the desktop windowing dependency is left out on Android.

## Usage

Run from the repository's root:

```sh
cargo run --release -p tau                        # sign in, then run agents
cargo run --release -p tau -- --demo              # the scripted session
cargo run --release -p tau -- --demo --open plugins
cargo run --release -p tau -- --demo --phone      # the phone layout
nix run                                           # the packaged app
```

The nix package wraps the binary with GPUI's libraries on its library path
and `git` on its `PATH`, since jj-lib's push runs `git` (ADR 0023). Under
plain `cargo run`, `git` must be on `PATH` for pushes.

## Testing

The crate has no tests of its own. The host and the interface are tested in
`tau-ui` and `tau-ui-remote`:

```sh
cargo nextest run --release -p tau-ui -p tau-ui-remote
```

## Further reading

- [crates/tau-ui/README.md](../tau-ui/README.md)
- [crates/tau-ui-remote/README.md](../tau-ui-remote/README.md)
- [docs/decisions/0007-gpui-interface.md](../../docs/decisions/0007-gpui-interface.md)
- [docs/decisions/0023-main-pushes-with-git-chat-prs-replay-onto-origin.md](../../docs/decisions/0023-main-pushes-with-git-chat-prs-replay-onto-origin.md)
- [docs/decisions/0030-host-halves-are-crates.md](../../docs/decisions/0030-host-halves-are-crates.md)
