# tau-vcs

The interface half of the version-control tools. It draws the cards
that `vcs_status`, `vcs_diff`, `vcs_show`, `vcs_log`, `vcs_commit`,
`spawn` and `wait` leave in a run's transcript, and the pieces tau's
landings draw changes with. It also holds the shapes the tools put in
their results. It does not touch a repository: jj-lib lives in the host
half, so a phone can draw the cards without it.

## What it provides

| Item                                                      | What it is                                                            |
| --------------------------------------------------------- | --------------------------------------------------------------------- |
| `VcsUi`                                                   | The plugin's `UiPlugin`: the tools' cards                             |
| `ChangeInfo`, `ChangeKind`, `FileChange`                  | One change, and one file's change, as the tools describe them         |
| `Landing`, `TooLarge`                                     | What landing a child did; a new file a snapshot left out for its size |
| `details::STATUS`, `DIFF`, `LOG`, `SHOW`, `COMMIT`        | The tool names the cards match on                                     |
| `details::SPAWN`, `WAIT`                                  | The sub-agent tool names                                              |
| `ui::change_status`                                       | `ChangeStatus`: the parsed `vcs_status` details                       |
| `ui::change_diff`                                         | `ChangeDiff`: a parsed `vcs_diff` or `vcs_show`, with files and hunks |
| `ui::change_log`                                          | `ChangeLog`: the parsed `vcs_log`, the run's stack and trunk          |
| `ui::commit_card`, `diff_card`, `log_card`, `status_card` | The cards' bodies and summaries                                       |
| `ui::landed`                                              | `LandingRecord` and `LandedCard`: what a landing shows                |

## How it fits

This is the interface half (decisions 0006, 0017, 0030). Its partner is
[tau-vcs-host](../tau-vcs-host), which runs the tools on jj-lib and
re-exports the shapes above.

- Builds on `tau-agent`, `tau-ui-plugin` and `tau-ui-kit`. It has no
  jj-lib or gix dependency.
- Used by `tau-ui-remote` (which lists `tau_vcs::ui::VcsUi` among the
  plugins a phone runs), `tau-ui` and `tau-vcs-host`.

## Usage

A client that only draws runs registers the UI without a host half:

```rust
use tau_ui_plugin::Registry;

let plugins = Registry::new().with(tau_vcs::ui::VcsUi);
```

A host then gives it its half, in its place, with
`.host(tau_vcs_host::VcsHost)`.

## Testing

```sh
cargo nextest run --release -p tau-vcs
```

`tests/ui.rs` is a Hegel property test: the cards keep the files a
person opened and the change they picked, over any clicks.

## Further reading

- [Version-control tools reference](../../../docs/reference/vcs.md)
- [Plugins: a plugin's UI](../../../docs/reference/plugins.md)
- [0014: The model commits, and every run lands as a stacked diff](../../../docs/decisions/0014-the-model-commits-and-runs-land-as-stacked-diffs.md)
- [0017: Plugins bring their own UI](../../../docs/decisions/0017-plugins-bring-their-ui.md)
- [0030: Host halves are crates](../../../docs/decisions/0030-host-halves-are-crates.md)
