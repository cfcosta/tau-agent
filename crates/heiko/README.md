# heiko

heiko on the computer: the desktop app. It opens `heiko-ui-remote`'s interface,
starts `heiko-ui`'s host behind it, and links every plugin's host half. With a
ChatGPT sign-in that allows plan use, it runs a real coding agent in the
repositories cloned from GitHub. Without one, it opens onboarding to sign in.

## What it provides

| Item        | What it is                                                                 |
| ----------- | -------------------------------------------------------------------------- |
| `heiko` (bin) | The app: parses its flags, starts a `Host` when signed in, opens a window  |
| `plugins`   | Every plugin `heiko-ui-remote` lists, each with its host half, as a Registry |
| `install`   | Installs `plugins` as the process's, once, before anything starts a host   |

The binary's flags:

| Flag              | What it does                                                          |
| ----------------- | --------------------------------------------------------------------- |
| `--model <id>`    | The model; gpt-6.1-sol by default                                     |
| `--prompt <text>` | Starts a run with this task right away                                |
| `--demo`          | Replays the scripted session instead of running agents                |
| `--finished`      | Opens the demo already done                                           |
| `--open <screen>` | A demo screen from `heiko_ui::demo::SCREENS`, or `page:<plugin>/<page>` |
| `--phone`         | The phone layout in a 390×844 frame                                   |
| `--frame <w>x<h>` | Lays out at exactly that size, to compare with the designs            |
| `--steps <n>`     | Stops the demo script after its first `n` updates                     |
| `--reduce-motion` | No looping motion, and transitions as plain fades                     |

With a sign-in, `--open` takes only a plugin's page, such as
`page:heiko-mcp/servers?repo=` for the MCP servers outside a repository.
`HEIKO_REDUCE_MOTION=1` asks for reduced motion too (`0` asks for all of it),
then `"reduce_motion": true` in `interface.json`, then the desktop's
`enable-animations` setting.

## How it fits

It is the top of the stack and nothing depends on it. It builds on
`heiko-ui` (the host), `heiko-ui-remote` (the interface), `heiko-ui-plugin` (the
registry) and GPUI. `heiko_ui::hosted::halves` gives most plugins their host
halves. This crate adds the two Luau ones, `heiko-codemode-host` and
`heiko-luau-plugins-host`, which are linked here and nowhere above, so
`heiko-ui` builds while Luau's C++ does (ADR 0030).

The phone build is a separate crate, `heiko-phone`. It brings its own
platform, so the desktop windowing dependency is left out on Android.

## Usage

Run from the repository's root:

```sh
cargo run --release -p heiko                        # sign in, then run agents
cargo run --release -p heiko -- --demo              # the scripted session
cargo run --release -p heiko -- --demo --open plugins
cargo run --release -p heiko -- --demo --phone      # the phone layout
nix run                                           # the packaged app
```

The nix package wraps the binary with GPUI's libraries on its library path
and `git` on its `PATH`, since jj-lib's push runs `git` (ADR 0023). Under
plain `cargo run`, `git` must be on `PATH` for pushes.

## Testing

The crate has no tests of its own. The host and the interface are tested in
`heiko-ui` and `heiko-ui-remote`:

```sh
cargo nextest run --release -p heiko-ui -p heiko-ui-remote
```

## Further reading

- [crates/heiko-ui/README.md](../heiko-ui/README.md)
- [crates/heiko-ui-remote/README.md](../heiko-ui-remote/README.md)
- [docs/decisions/0007-gpui-interface.md](../../docs/decisions/0007-gpui-interface.md)
- [docs/decisions/0023-main-pushes-with-git-chat-prs-replay-onto-origin.md](../../docs/decisions/0023-main-pushes-with-git-chat-prs-replay-onto-origin.md)
- [docs/decisions/0030-host-halves-are-crates.md](../../docs/decisions/0030-host-halves-are-crates.md)
