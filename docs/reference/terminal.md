# tau-terminal: the terminal view

`tau-terminal` (`crates/terminal`) draws a terminal in any GPUI app. It
depends on no other tau crate
([0010](../decisions/0010-terminal-rendering.md)). This page covers the
view and its pure parts. The emulator (`Terminal`) and the runner
(`Command`) are covered in the crate docs and in
[tools.md](tools.md), "bash: terminal mode".

Try it with the example, which runs a command under a pseudo-terminal
and shows it:

```sh
cargo run -p tau-terminal --example run -- ls -la --color=always
```

## `TerminalView`

`TerminalView` is a GPUI entity (`Render`, `Focusable`,
`EventEmitter<TerminalEvent>`). It is live or frozen:

- **Live:** it owns a libghostty `Terminal`. Feed it output with
  `write`. The terminal is `!Send`, and like every entity it stays on
  the GPUI thread.
- **Frozen:** it holds only styled rows (`Vec<Line>`). `freeze` drops
  the terminal and keeps the rows up to the last one with something on
  it. After that, `write` fails with `Error::Frozen`.

```rust
use tau_terminal::{TerminalView, ViewOptions, ScrollTo, view};

view::bind_keys(cx); // once: secondary-c / ctrl-shift-c copy, secondary-a selects all

let term = cx.new(|cx| TerminalView::new(ViewOptions::default(), cx))?; // live, empty
term.update(cx, |t, cx| t.write(bytes, cx))?;  // program output, split anywhere
term.update(cx, |t, cx| t.freeze(cx))?;        // the program ended

// Or frozen at once, from stored output or a `Terminal::vt` rendering:
let old = cx.new(|cx| TerminalView::replay(bytes, options, cx))?;
```

| Method                                                       | What it does                                                                                                    |
| ------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------- |
| `new(options, cx)`                                           | A live view of an empty `options.size` terminal.                                                                |
| `frozen(lines, cols, options, cx)`                           | A frozen view of given rows.                                                                                    |
| `replay(bytes, options, cx)`                                 | `new`, then `write`, then `freeze`.                                                                             |
| `write(bytes, cx)` / `freeze(cx)`                            | Feed output / drop the terminal.                                                                                |
| `is_frozen()`, `cols()`                                      |                                                                                                                 |
| `total_rows()`                                               | Rows to show. Frozen: every row. Live: the scrollback, then the screen down to its last used row or the cursor. |
| `visible_range()`                                            | The rows shown, out of `total_rows()`.                                                                          |
| `set_visible_rows(Some(n) \| None, cx)`                      | Show at most `n` rows and scroll through the rest. `None` shows every row.                                      |
| `scroll_to(ScrollTo::{Top, Bottom, Row(n)})`                 | Scrolls the window. `Bottom` also turns following back on.                                                      |
| `scroll_by(rows, cx)`, `follows_end()`, `scrolls()`          |                                                                                                                 |
| `selection()`, `set_selection(..)`                           | A `Selection` from an anchor `Point { row, col }` to a head.                                                    |
| `selected_text()`, `text()`                                  | The selection's text, or all the text.                                                                          |
| `rows(range)`                                                | Styled rows (`Rc<Vec<Line>>`).                                                                                  |
| `set_palette(..)`, `set_font(family, size, ..)`, `options()` |                                                                                                                 |

`TerminalEvent::Scrolled` fires when the rows shown change: after a
scroll, after new rows while the window follows the end, after a
freeze, or after a change to the visible rows. `TerminalEvent::Selected`
fires when the selection changes. A host that shows the visible range
(as tau-ui's strip does) subscribes to these.

### `ViewOptions`

| Field          | Default                             | Meaning                                                   |
| -------------- | ----------------------------------- | --------------------------------------------------------- |
| `size`         | 120×40 (`Size::TOOL`)               | The live terminal's grid; frozen views keep their `cols`. |
| `scrollback`   | 64 MiB                              | The live terminal's scrollback, in bytes.                 |
| `font_family`  | Menlo / Consolas / DejaVu Sans Mono | A monospace family.                                       |
| `font_size`    | 12 px                               |                                                           |
| `line_height`  | 1.5                                 | A multiple of the font size, rounded to whole pixels.     |
| `palette`      | `Palette::default()`                | See below.                                                |
| `visible_rows` | `Some(24)`                          | `None` shows every row.                                   |
| `min_rows`     | 1                                   | The view's height in rows, at least.                      |

### How it draws

- **Cells.** A cell is as wide as one glyph of the font (`m`) and as
  tall as the line height. The grid is `cols` wide. A line wider than
  the view is clipped, and a horizontal wheel scrolls it sideways.
- **Each row is drawn in this order:**
  1. the backgrounds that differ from the palette's, merged when
     adjacent;
  2. the selection;
  3. box-drawing and block characters, drawn as rectangles snapped to
     device pixels so they meet their neighbors;
  4. the text: each run of one style is shaped as one line, with every
     glyph placed at its cell's column. A wide character is a run of
     its own, two cells wide, and its spacer cell is never drawn.
- **Then the cursor**, only while the view is live and the program
  shows it: block (with the character under it redrawn in the ground's
  color), hollow block, bar, or underline.
- **Then the scrollbar thumb**, when there are more rows than the view
  shows.
- Rows outside the window's content mask are not shaped. A view that
  shows every row inside a scrolled list costs only what is on screen.
- **Styles:** bold, italic, faint, single or curly underline,
  strikethrough, inverse, and invisible.

### Input

- The wheel scrolls rows. It stops propagating only while the view
  moved, so at an edge it scrolls whatever holds the view.
- Dragging selects in reading order. The view scrolls when the drag
  goes past its top or bottom. Shift-click extends the selection. A
  click with no drag clears it.
- `Copy` copies the selection, or all the text when nothing is
  selected. Rows join with `\n`, soft-wrapped rows join into one line,
  and trailing blanks are dropped.

## The pure parts

These are testable without a window:

- `Palette`: the 16 ANSI colors, the default foreground and background,
  the cursor, the selection and the scrollbar. `indexed(n)` maps the 16
  to the palette and 16–255 to the xterm color cube and gray ramp.
  `colors(&Style)` resolves a run's colors, with inverse applied.
  Snapshots keep palette entries as `Color::Palette(n)`, so the same
  output takes any palette.
- `layout::layout(&Line, &Palette) -> RowLayout`: a row's fills, texts
  and shapes, as described above.
- `glyph::glyph(char) -> Option<Glyph>`: the characters drawn as shapes.
  These are light, heavy and double straight lines, their corners, tees
  and crosses of one weight, rounded corners (drawn square), half
  lines, and all block elements. Dashed lines and the other double
  forms are left to the font.
- `Scroller`: the window of rows. It never starts past the last full
  page, and it follows the end exactly when it is at the end. It also
  gives the scrollbar's thumb.
- `Selection`, `text_of`: the text of a span of rows.
- `Terminal::lines(range)`, `total_rows()`, `history_rows()`: styled
  rows of the whole scrollback. Freezing is built from these.

## In tau-ui: the `bash` card

A `bash` result with `term` details (see [tools.md](tools.md)) is a
`ToolBody::Terminal(TermOutput)` in the view model (`view.rs`):

- While the command runs, each progress update's chunk is appended in
  `seq` order. Repeated or early chunks are dropped.
- The result replaces the chunks with its replay bytes and records how
  the command ended (`exit N`, `timed out` or `cancelled`, shown in red
  on the card).
- A run reopened from the store builds the same body from the stored
  result's details.
- Results without `term` details (pipe mode) keep the plain lines.

`ui/term_card.rs` keeps one `TerminalView` per card that has been shown:

- A running card's view is live. It is fed from the model as events
  arrive (`Workspace::apply_event`), and the cursor is shown.
- A finished card's view is frozen. A stream replay is written and then
  frozen. A snapshot replay (for output over 4 MiB) replaces the live
  view.
- A card first seen finished, such as one from history, is built
  frozen from its replay.

The card is option B of the design:

- an inset screen in its own ground and a Ghostty-like palette
  (`Theme::term`), 12 rows when closed, scrolled to the end, with a
  scrollbar;
- a strip on top:
  - while running: `120×40 · xterm-256color` and a live dot;
  - when finished: `120×40 · N lines · a–b` (the rows in view), with
    `copy` and `expand`. `expand` shows every row.
- When fast compaction pruned the output, the strip holds two tabs:
  - `terminal · N lines`;
  - `model saw · K lines`: the text the model got, with each
    `[N lines omitted]` drawn as a small blue chip and the header and
    footer dimmed;
  - an `open full output` link, which opens the archive.
