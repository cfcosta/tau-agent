# 0055: gpui is the interface's alone

- Status: proposed. Amends [0045](0045-everything-is-a-plugin.md): the
  view tree's renderer is a crate of its own, and nothing a plugin crate
  needs pulls gpui in. Amends [0053](0053-a-cli-for-plugins.md):
  `tau-plugin` says it has no gpui, and after this it has none. Follows
  [0030](0030-host-halves-are-crates.md): a crate boundary keeps a
  dependency out of a build, a feature does not.
- Date: 2026-11-14

## Context

0045 says no plugin draws gpui: a plugin's UI is a `Node` tree, and the
kernel draws it with `tau-ui-kit`. The crates did not follow. Measured on
2026-11-14 (release, 32 cores):

- **31 of our 41 crates depend on gpui**, and none of the 24 that do not
  draw calls it. They reach it through `tau-ui-plugin` (23 crates depend
  on it) and `tau-ui-kit` (14). `tau-plugin`, the headless command line,
  links gpui through `tau-luau-plugins-host`; so do the evals.
- **What they take is plain values.** From `tau-ui-kit`: `theme::Tone`
  (17 uses), `format::{grouped, usd, fine_usd}`, `assets::Icon`,
  `diff::{parse, DiffKind}` and `syntax::Lang`. From `tau-terminal`
  (only `tau-tools-host`): `Command` and `Error`. In `tau-ui-plugin`,
  gpui is only in `tree.rs`: `Draw`, the renderer (about 1,200 of its
  2,100 lines), `DraggedCard`, and the callback types that name gpui
  (`OnKey`, `OnSubmit`, `OnAct`, `Widgets`, `Inputs`, `Fetches`). The
  plugin traits (`UiPlugin`, `HostHalf`, `Fold`, the manifest, the wire)
  name no gpui type. In `tau-terminal`, gpui is only in `view.rs`.
- **Every UI edit is a full rebuild.** Touching a crate and rebuilding
  `--workspace --all-targets`:

  | Touched | Libs rebuilt | Targets rebuilt | Unit-seconds |
  | --- | --- | --- | --- |
  | `tau-ui-kit` | 30 | 31 | 1,617 |
  | `tau-terminal` | 31 | 32 | 1,624 |
  | `tau-ui-plugin` | 29 | 30 | 1,604 |
  | every crate of ours | 41 | — | 1,796 |

  A change to a widget, a colour or the terminal's view recompiles the
  host crates, the plugins and their tests, which use none of it, and
  links gpui into each of their test binaries again.

## Decision

Only crates that draw depend on gpui: `tau-ui-kit`, the renderer below,
`tau-ui`, `tau-ui-remote`, `tau-demo`, `tau` and `tau-phone`. Every
other crate reaches gpui through no normal or build dependency.

### `tau-ui-data`: what the interface and plugins share, without gpui

A new crate holds the values both sides name: `Tone`, `Icon` (its names;
the SVGs and the `AssetSource` stay in `tau-ui-kit`), `format`,
`diff::parse` with `DiffLine` and `DiffKind`, `syntax::Lang` and
`ticker::Count`. What turns them into gpui (a tone's colour, an icon's
path as an asset, highlighting) stays in `tau-ui-kit`, as functions or
an extension trait, since a crate cannot add inherent methods to another
crate's type.

`tau-ui-kit` depends on `tau-ui-data` and re-exports each value under
its path today (`tau_ui_kit::theme::Tone` and so on), so the drawing
crates keep their imports. Plugin crates import from `tau-ui-data`.

### `tau-ui-plugin` without gpui

It keeps the plugin interface and the view tree as data: `Node`, `Act`,
the specs, the keys. It depends on `tau-ui-data`, not on `tau-ui-kit`,
and not on gpui. The `From<Tone> for theme::Tone` conversion goes with
the renderer.

### `tau-ui-tree`: the renderer

A new crate, on `tau-ui-plugin` and `tau-ui-kit`, holds `Draw`,
`DraggedCard` and the callback types. `tau-ui`, `tau-ui-remote` and
`tau-demo` depend on it. It is not in `tau-ui`, since `tau-ui-remote`
draws the phone and must not depend on the desktop interface.

### The terminal's view in `tau-ui-kit`

`tau-terminal/src/view.rs` moves into `tau-ui-kit`, as 0045 already
plans for the terminal and diff views. `tau-terminal` keeps the
emulator, the pseudo-terminal and the screen, with no gpui, so
`tau-tools-host` runs commands without the toolkit.

### A test keeps it so

A test reads `cargo metadata` and fails when a crate outside the
drawing list reaches `gpui-pre` through a normal or build dependency,
naming the path that does. A crate's tests may still reach gpui through
a dev-dependency when they draw; the test lists those, so each one is a
choice.

## Alternatives considered

- **A `gpui` feature on `tau-ui-plugin` and `tau-ui-kit`.** Cargo turns
  a feature on for the whole build when any crate asks for it (0030),
  and `tau-ui` would ask. Workspace builds, where the time goes, would
  not change.
- **The renderer in `tau-ui`.** `tau-ui-remote` would then depend on the
  desktop interface.
- **Lower optimisation or incremental release builds for our crates.**
  Declined on 2026-10-05; this decision removes work instead.
- **Leave it.** Every edit to a widget stays a near-full rebuild of our
  code.

## Consequences

- An edit to `tau-ui-kit`, the renderer or the terminal's view rebuilds
  the drawing crates only. With today's unit times that is about 400
  unit-seconds (`tau-ui` 121, `tau-ui-remote` 101, `tau` 68, `tau-demo`
  58, then `tau-ui-kit`, `tau-ui-tree` and `tau-phone`), not 1,617. This
  is an estimate; the steps end by measuring it.
- Edits to `tau-ui-plugin`'s data, `tau-ui-data` and the bottom crates
  (`tau-ai`, `tau-store`, `tau-agent`) still rebuild about 30 crates.
  They are under every plugin by design.
- 24 crates' test binaries stop linking gpui and its platform crates,
  which also shrinks `target/`. `tau-plugin` and the evals are headless,
  as 0053 says.
- In a clean build the host crates stop waiting for gpui (done about
  70 s in). The critical path stays Luau's C++ through `mlua`.
- About 35 imports in plugin crates change from `tau_ui_kit::…` to
  `tau_ui_data::…`. No plugin's behaviour or UI changes.

## Steps

1. `tau-ui-data`: move the values, re-export them from `tau-ui-kit`,
   and switch the plugin crates' imports.
2. The terminal's view into `tau-ui-kit`; `tau-terminal` without gpui.
3. `tau-ui-tree`: `Draw`, `DraggedCard` and the callback types;
   `tau-ui-plugin` drops gpui and `tau-ui-kit`.
4. The test that keeps gpui to the drawing crates.
5. Measure again, touching `tau-ui-kit`, `tau-terminal` and
   `tau-ui-plugin` as in the table, and record it here under "As built".
