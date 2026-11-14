# heiko-tools

The interface half of the coding tools. It draws the cards that `read`,
`exec_command`, `write_stdin`, `edit`, `write`, `grep`, `find` and `ls`
leave in a run's
transcript, and it holds the shapes those cards read. It does not run
any tool. A phone links this crate to draw the cards of a run that a
desktop host is running.

## What it provides

| Item                          | What it is                                                                     |
| ----------------------------- | ------------------------------------------------------------------------------ |
| `ToolsUi`                     | The plugin's `UiPlugin`: the cards of the eight tools                          |
| `ui::NAME`                    | `"heiko-tools"`, the name both halves go by                                      |
| `ui::State`                   | The fold of the run's artifact grants, the same live and reloaded              |
| `ui::Action`, `ActionReply`   | What a card asks the host half (a range of a granted artifact), and the answer |
| `ui::ReadView`                | A `read` card's view of a call: path, range and lines                          |
| `ui::artifact_status`         | Whether a finished call left an artifact, with its metadata, or why not        |
| `ui::diff_of`                 | The diff an `edit` or `write` call made                                        |
| `ui::output_lines`            | A call's output lines: its result, or the output so far                        |
| `ui::listing`, `listing_card` | `DirListing`, the parsed `ls` details, and its card                            |
| `ui::term`                    | `TermOutput`: a command call's terminal stream, as the card reads it           |
| `ui::term_card`               | `TermCards`: command output drawn as a terminal (feature `terminal`)           |
| `details`                     | `Listing`, `Entry`, `EntryKind`: what `ls` puts in its result's details        |
| `artifact_grant`              | `ArtifactGrant`, `ArtifactMetadata`, `ArtifactRecord` and `fold_grants`        |

Command cards show the call's terminal, or its last lines. `edit` and
`write` cards show the diff they made. `ls` cards show the directory.

## How it fits

This is the interface half (decisions 0006, 0017, 0030). Its partner is
[heiko-tools-host](../heiko-tools-host), which builds the tools on a run's
workspace and re-exports `details` and `artifact_grant`.

- Builds on `heiko-agent`, `heiko-ai`, `heiko-ui-plugin` (the `UiPlugin` and
  `Fold` traits), `heiko-ui-kit` (theme, diffs, syntax) and, with the
  `terminal` feature, `heiko-terminal`.
- Used by `heiko-ui-remote`, which lists `heiko_tools::ui::ToolsUi` among
  the plugins a phone runs, by `heiko-ui`, and by `heiko-tools-host`.

## Usage

A client that only draws runs registers the UI without a host half:

```rust
use heiko_ui_plugin::Registry;

let plugins = Registry::new().with(heiko_tools::ui::ToolsUi);
```

A host then gives it its half, in its place, with
`.host(heiko_tools_host::ToolsHost)`.

## Features

| Feature    | Default | Effect                                                                                     |
| ---------- | ------- | ------------------------------------------------------------------------------------------ |
| `terminal` | off     | Draws command output as a terminal through `heiko-terminal` (libghostty-vt): `ui::term_card` |

Without `terminal`, command cards show the last lines of the output and
`heiko-terminal` is not built. `heiko-ui` and `heiko-ui-remote` turn it on.

## Testing

```sh
cargo nextest run --release -p heiko-tools
cargo nextest run --release -p heiko-tools --features terminal
```

`tests/ui.rs` checks what the cards read from a call: diffs, `read`
line numbers, output so far, and the artifact grant fold. Several are
Hegel property tests.

## Further reading

- [Coding tools reference](../../../docs/reference/tools.md)
- [Plugins: a plugin's UI](../../../docs/reference/plugins.md)
- [Code Mode artifact ranges](../../../docs/reference/codemode-artifact-ranges.md)
- [0004: Coding tools live in an optional crate](../../../docs/decisions/0004-coding-tools-optional.md)
- [0010: command output is a terminal, emulated by libghostty-vt](../../../docs/decisions/0010-terminal-rendering.md)
- [0017: Plugins bring their own UI](../../../docs/decisions/0017-plugins-bring-their-ui.md)
- [0030: Host halves are crates](../../../docs/decisions/0030-host-halves-are-crates.md)
