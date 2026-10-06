# tau-codemode

The interface half of Codemode. Codemode is a tool whose input is a Luau
script: the script calls the run's other tools as functions. This crate
holds what every interface needs to show those calls, and the records
the plugin writes. It runs no Luau. A phone links it and draws the same
cards the desktop does.

## What it provides

| Item                    | What it is                                                    |
| ----------------------- | ------------------------------------------------------------- |
| `CodemodeUi`            | The `UiPlugin`: the card, the inspector, the plugin-list line |
| `PLUGIN`                | The plugin's name, `tau-codemode`                             |
| `Outcome`, `Rendered`   | What a script returned, and what the model reads              |
| `CallRow`, `CallStatus` | One nested call on the card, and its status                   |
| `Failure`, `Item`       | Why a script failed; one output item                          |
| `description`           | The `codemode` tool's fixed description text                  |
| `store`                 | Values scripts keep: `Store`, `Writes`, `fold`                |
| `modules`               | Module definitions, selections, tests and pins                |
| `promotion`             | Requests to promote a module, and their decisions             |
| `inference_trace`       | Records of nested `infer` calls; `restore_budget`             |
| `live`                  | What a running call reports: `JevUpdate`, `InferUpdate`       |
| `format::formatted`     | A script formatted by StyLua, for the cards only              |
| `outline`               | A script's output on its card, one line per item              |
| `image`                 | `image()` payloads, typed by their bytes (`parse`, `sniff`)   |

The records and their folds are plain data. Live and stored runs fold
them the same way, so a card drawn while a script runs matches the one
drawn from the stored run.

## How it fits

This is the interface half (decisions 0006, 0017 and 0030). Its partner
is `tau-codemode-host`, which runs the scripts in a Luau sandbox, writes
these records and re-exports `Outcome`, `CallRow` and the other shapes
its API returns.

It builds on `tau-agent` (with `serde`), `tau-ai`, `tau-ui-plugin` and
`tau-ui-kit`, and draws with gpui. `stylua` formats scripts for the
cards.

Used by:

- `tau-codemode-host`, the host half;
- `tau-luau-plugins-host`, which loads a Luau plugin's files as codemode
  module definitions;
- `tau-ui-remote`, which registers `CodemodeUi` in the plugin list;
- `tau-ui`, whose demo draws codemode cards;
- `tau-codemode-eval` (`crates/evals`).

`CodemodeUi` is registered last in `tau_ui_remote::plugins`. Codemode's
`start` lists the signatures of the tools in the run's plan, so a plugin
that adds tools in its own `start`, such as tau-mcp, must come before it.

## Testing

```sh
cargo nextest run --release -p tau-codemode
```

`tests/store.rs`, `tests/modules.rs` and `tests/repository_pins.rs` are
Hegel property tests. They check the folds against an independent model
at every prefix of generated record histories. The workspace's
`hegel.toml` sets the case counts. `tests/format.rs` checks StyLua
formatting. Nothing needs network or credentials.

## Further reading

- [Codemode reference](../../../docs/reference/codemode.md)
- [Module records](../../../docs/reference/codemode-modules-records.md)
- [Modules in the run inspector](../../../docs/reference/codemode-modules-ui.md)
- [Module promotion](../../../docs/reference/codemode-module-promotion.md)
- [Inference traces](../../../docs/reference/codemode-inference-traces.md)
- [0017: Plugins bring their own UI](../../../docs/decisions/0017-plugins-bring-their-ui.md)
- [0018: MCP servers and Codemode, as two plugins](../../../docs/decisions/0018-codemode-and-mcp.md)
- [0030: Host halves are crates](../../../docs/decisions/0030-host-halves-are-crates.md)
