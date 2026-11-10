# tau-canvas

What a run makes for the person to look at, in a pane beside its
transcript (ADR 0040). The model shows an image file with
`canvas_show`, writes a Markdown document with `canvas_write`, revises
one with `canvas_edit`, and reads one back with `canvas_read`. Each is
an item on the run's canvas, numbered from 1, with every revision kept.
It is a Luau plugin tau comes with (ADR 0050): this folder is its
package, with the tests and the fixtures the demo shows of it. Its
reference is [canvas.md](../../docs/reference/canvas.md).

## What it provides

| File                | What it is                                                            |
| ------------------- | --------------------------------------------------------------------- |
| `src/plugin.luau`   | The four tools, the pane's actions, and the view: pane, cards, status |
| `src/canvas.luau`   | The canvas: items and revisions, edits, what the pane shows           |
| `tests/plugin.luau` | The tools, the pane, its picks and the cards, as cases                |
| `tests/canvas.luau` | Edits and the canvas, as cases and properties                         |
| `fixtures/`         | What the demo shows of it, and what it declares                       |

## How it fits

The canvas is the run's state. An image's bytes are a blob of the
repository's (`ctx.blobs.put_file`), which the interface asks the host
for once; the file may change or go after. `uses.files` is the run's
workspace and the home folder, read-only: a path past them is refused.

## Testing

```sh
target/release/tau-plugin test plugins/tau-canvas   # its Luau tests, no compiling
target/release/tau-plugin test plugins/tau-canvas --bless   # writes its fixtures anew
cargo nextest run --release -p tau-ui -E 'test(/^canvas::/)'
```

The first runs its Luau tests as tau loads it, and checks the fixtures
`tests/fixtures.luau` makes are those in `fixtures/`; `tests/loop.luau`
shows an image in a real run: read from the workspace, kept as a blob,
and read back whole. The third runs tau-ui's pane on the plugin.
