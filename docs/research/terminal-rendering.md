# Research: rendering tool output as a terminal

- Status: decided in
  [0010](../decisions/0010-terminal-rendering.md): libghostty-vt,
  against this note's recommendation
- Date: 2026-09-28

## Question

`tau-ui` shows `bash` results as plain text lines. Can it always show
them as a real terminal view instead, with ANSI colors, cursor
movement, progress bars and wide characters? Is libghostty the right
engine for that, and what else would have to change?

This note covers libghostty as it is in September 2026, how it would
fit a Rust and GPUI app, what the `bash` tool would need, and the Rust
alternatives. It ends with a recommendation and a staged plan.

## What tau does today

- `tau_tools_host::bash` (`crates/plugins/tau-tools-host/src/bash.rs`) runs
  `$SHELL -c <command>` with `stdin` set to null and `stdout`/`stderr`
  on pipes. The child sees no TTY, so most programs turn colors and
  progress bars off.
- Both pipes feed one `Accumulator`. While the command runs, the tool
  sends a full `truncate_tail` snapshot as a `ToolUpdate`, at most once
  per throttle window. The last update and the final result are text
  only.
- The model sees the last 2000 lines or 50 KiB. The spec in
  `docs/reference/tools.md` says limits and strings follow pi, and any
  change is a behaviour change.
- `tau-ui` (`src/view.rs`) keeps the last 6 lines of each update and
  the last 4 lines of the result, as `ToolBody::Output(Vec<String>)`.
  `ui/transcript.rs::output` draws each line as a monospace `div`, with
  one special case: lines starting with `PASS` are green.

So there are two separate gaps. The tool never produces terminal output,
and the UI has nothing that could interpret it.

## libghostty today

### What it is

Mitchell Hashimoto announced libghostty in September 2025 as a family
of embeddable libraries split out of Ghostty. The first, and so far
the only one with a public API, is **libghostty-vt**: the VT parser
and terminal state. Planned later pieces are input encoding (now
partly inside libghostty-vt), a GPU renderer (OpenGL or Metal), and
GTK and Swift widgets.

| Part                                | Library today?     | Notes                                                                |
| ----------------------------------- | ------------------ | -------------------------------------------------------------------- |
| VT parser (CSI, ESC, DCS, OSC, APC) | Yes, libghostty-vt | C API added in March 2026 (discussion #11348).                       |
| Terminal state: grid, scrollback    | Yes, libghostty-vt | Reflow on resize, alternate screen, Kitty graphics state.            |
| Render state (viewport, dirty rows) | Yes, libghostty-vt | Built for custom renderers; iterators over rows and cells.           |
| Formatter (plain text, VT, HTML)    | Yes, libghostty-vt | Useful for the model-facing text.                                    |
| Key, mouse, focus, paste encoding   | Yes, libghostty-vt | Not needed for read-only output.                                     |
| GPU renderer (Metal, OpenGL)        | No                 | Still inside the Ghostty app. No stable embedding API.               |
| Full embedding API (`ghostty.h`)    | Internal           | Used by the macOS app. cmux ships on it (unverified). Not stable.    |
| PTY management                      | No                 | Out of scope by design: "libghostty-vt doesn't create/manage a pty". |

### Language, ABI and build

- Written in Zig. It exposes a Zig module and a C API (`ghostty/vt.h`).
  C API docs are on a Doxygen site (libghostty.tip.ghostty.org).
- Zero dependencies, not even libc. It targets macOS, Linux, Windows
  and WebAssembly.
- It builds with `zig build`. The current Rust bindings need Zig
  0.16.x on `PATH`.
- License: MIT (Ghostty repository).

### Maturity

- The Ghostty README says the functionality is "extremely stable",
  because the Ghostty app has used it for a long time, but "the API
  signatures are still in flux".
- The C API docs say: "the API is not yet stable. Breaking changes are
  expected in future versions."
- There is no tagged libghostty release. The Rust bindings pin a
  Ghostty commit.
- The C API is still moving. The daily tip changelog shows new
  render-state fields and callbacks in September 2026 (overscan on
  2026-09-25, a `render_hold` callback on 2026-09-20).

### Rust bindings

- `libghostty-vt-sys` (raw FFI, generated from `vt.h`) and
  `libghostty-vt` (safe API), from `github.com/Uzaaft/libghostty-rs`.
  Mitchell Hashimoto describes them as made by Ghostty maintainers.
- `libghostty-vt` 0.2.1, released 2026-07-18. Versions so far: 0.1.0
  and 0.1.1 (March 2026), 0.2.0 (June), 0.2.1 (July). License
  `MIT OR Apache-2.0`. The README says pre-1.0 and that breaking
  changes are expected.
- Types such as `Terminal` and `RenderState` are `!Send` and `!Sync`.
  All calls must come from one thread.
- The repository has a port of Ghostling (a minimal terminal) on
  macroquad. It is the closest worked example of a custom renderer.
- There are forks (`aislopware/libghostty-rs`, which carries a
  clipboard soundness fix from upstream issue #75, and others). Use
  the Uzaaft crate.

The safe API, from the crate docs and source:

- `Terminal::new(TerminalOptions)` with columns, rows and scrollback
  size; `vt_write(&[u8])` feeds bytes; `resize(cols, rows, cell_w_px,
cell_h_px)`; `cursor_x`, `cursor_y`, `scrollback_rows`,
  `total_rows`, `scroll_viewport`, `active_screen`, `title`.
- Callbacks run inside `vt_write`: `on_pty_write` (replies to device
  queries), `on_bell`, `on_title_changed`, `on_pwd_changed`.
- `RenderState::update(&terminal)` returns a `Snapshot`. From it,
  `dirty()`, `colors()`, `cursor_viewport()`, `cursor_visual_style()`,
  and a `RowIterator` that yields a `CellIterator` per row. Each cell
  has `graphemes_utf8`, `style()`, `fg_color()`, `bg_color()` and
  `is_selected()`.
- `fmt::Formatter` writes the terminal as plain text, VT or HTML, with
  options such as `with_unwrap` and `with_trim`.

### Building it under Nix

The `-sys` build script clones Ghostty at the pinned commit with `git`
and runs `zig build`, which also fetches Zig packages. Neither works in
the Nix sandbox. There are three ways around it:

1. **pkg-config (preferred).** Add the Ghostty flake as an input at the
   commit `libghostty-vt-sys` pins, take
   `ghostty.packages.${system}.libghostty-vt`, add it to
   `buildInputs`, and build with the `libghostty-vt-sys/pkg-config`
   feature. The libghostty-rs flake does exactly this. No Zig in tau's
   own build.
2. **Build from a local source.** Set `GHOSTTY_SOURCE_DIR` to the
   Ghostty flake input, `GHOSTTY_ZIG_SYSTEM_DIR` to pre-fetched Zig
   packages, and provide Zig 0.16 (nixpkgs has 0.16.0, or
   `mitchellh/zig-overlay`). More moving parts.
3. **Ghostty's binary cache.** `ghostty.cachix.org` serves Ghostty
   flake outputs, which saves the Zig build for common systems.
   Unverified whether it has `libghostty-vt` for every system tau
   builds.

The Ghostty input and the crate version must move together. A crate
bump that pins a new Ghostty commit needs a flake input bump in the
same commit.

## Fitting it into GPUI

### Option A: libghostty-vt state, GPUI drawing

This is the only route that exists today.

1. Bytes arrive from the tool (see below) on the async runtime.
2. The `tau-ui` view that owns the card calls `vt_write` on the GPUI
   main thread. This matches the `!Send` rule, because GPUI entities
   already live on the foreground thread.
3. On each frame that has dirty rows, call `RenderState::update` and
   walk rows and cells.
4. Group adjacent cells with the same style into one text run. Draw
   backgrounds as rectangles, then text with `shape_line`, then the
   cursor. Skip the spacer cell after a wide character.

Zed's terminal is the reference for step 4.
`crates/terminal_view/src/terminal_element.rs` builds
`BatchedTextRun`s from adjacent cells with equal style (`can_append`
compares font, colors, underline and strikethrough), paints
`LayoutRect` backgrounds, text runs, and a `CursorLayout`. It also
draws box-drawing and block characters as rectangles, so they line up
with no gaps, and runs `ensure_minimum_contrast` on colors. The code is
GPL-3.0 (Zed's license for that crate, unverified), so read it for
the approach and do not copy it.

Cell metrics: take the advance of one glyph of `theme::MONO` at the
card's text size as the cell width, and the line height as the cell
height. Pass both to `resize`.

### Option B: Ghostty's own renderer

This does not fit, now or soon:

- There is no public renderer library. The planned one targets
  OpenGL and Metal and expects to own a surface.
- GPUI owns its GPU backend (Metal on macOS, Vulkan on Linux, DirectX
  on Windows) and its own scene graph. Sharing a surface would mean
  a native child window per card, which does not work for many small
  cards in a scrolling transcript.
- Ghostty's renderer brings its own font stack (CoreText, FreeType,
  HarfBuzz), which would not match GPUI's text.

Option B is worth a look only for a full-screen interactive terminal
pane, and only after libghostty ships a renderer API.

### Cost per card

A transcript can hold hundreds of `bash` cards. Keep one live
`Terminal` per running card only. When a command ends, freeze the card
into a plain value (rows of styled runs, capped to the lines shown) and
drop the `Terminal`. Old runs loaded from `tau-store` start frozen, so
the UI never replays their bytes unless the card is expanded.

## Changes to the `bash` tool

The UI can only show what the tool captures. Four changes are needed,
and each can ship alone.

### 1. Keep the raw bytes

The model-facing text is a truncated tail. It cannot drive a terminal,
because a cut can land inside an escape sequence and the lost prefix
may have set modes or moved the cursor.

- Send each chunk, in order, as its own update. Use
  `ToolOutput::details` so the model never sees it. For example:
  `{"term": {"seq": 12, "bytes": "<base64>"}}`. The loop already
  forwards `details` in `ToolUpdate` and `ToolEnd`.
- Updates are dropped after the tool future resolves
  (`ToolUpdates::close`), so the last chunks must be flushed before
  the tool returns.
- The throttle applies to the text snapshot only. Byte chunks can be
  coalesced (for example, every 16 ms or 64 KiB) but must not be
  dropped.
- Cap what the UI keeps. Past a few MiB, keep only the VT state.

### 2. Run under a PTY

Without a TTY most programs print no colors or progress bars. Options:

| Approach                                                                  | Effect                                                             | Cost                                                          |
| ------------------------------------------------------------------------- | ------------------------------------------------------------------ | ------------------------------------------------------------- |
| Pipes plus `CLICOLOR_FORCE=1`, `FORCE_COLOR=1`, `CARGO_TERM_COLOR=always` | Colors from tools that honour them. No progress bars, no `isatty`. | Trivial. Model text gets escape codes unless stripped.        |
| `portable-pty` 0.9 (wezterm)                                              | Real TTY, cross-platform, including ConPTY.                        | Blocking reader; needs a thread per command.                  |
| `rustix::pty::openpt` or `openpty`, own spawn                             | Real TTY on Unix, fits the current tokio code.                     | We handle `setsid`, `TIOCSCTTY` and process groups ourselves. |

`bash` is already unix-only, so a `rustix` PTY fits best. `rustix` is
already in `Cargo.lock`. Points to handle:

- **stdin.** Today it is null, so commands that prompt fail at once.
  With a PTY slave as stdin, a prompt hangs until the timeout. Keep
  stdin on `/dev/null` and give only stdout and stderr the slave. The
  child still needs the slave as its controlling terminal, or job
  control and `/dev/tty` break.
- **Pagers.** A TTY makes `git log`, `man` and others start `less`.
  Set `PAGER=cat`, `GIT_PAGER=cat`, `MANPAGER=cat` and `LESS=-FRX`.
- **Size.** Fix the size when the command starts (for example 120
  columns, 40 rows). Do not follow the UI width: the model-facing text
  must not depend on window size, or runs stop being reproducible.
- **`TERM`.** `xterm-256color` is the safe choice. `xterm-ghostty`
  needs Ghostty's terminfo on the machine.
- **Line endings.** The tty layer turns `\n` into `\r\n`. The
  model-facing text must turn it back.
- **Process group and kill.** Keep `process_group(0)` and kill the
  group on timeout and cancel as today. Closing the master sends
  `SIGHUP` to the session.
- **stderr.** It merges with stdout, which matches today's documented
  arrival-order merge.

Ship it behind an option (`Bash::with_pty(bool)`) and keep pipes as
the default until the model text is shown to be unchanged for plain
output.

### 3. Plain text for the model

With a PTY, the raw stream has escapes, `\r` redraws and cursor moves.
The model must still get plain text, and the limits must stay pi's.

- Run the bytes through a VT emulator in the tool and emit its text.
  A progress bar then shows only its final frame, which is also what a
  person saw.
- Keep the scrollback large enough for `MAX_LINES` (2000) plus the
  screen, then apply `truncate_tail` to the result.
- For output with no escapes and no `\r`, the text must be
  byte-identical to today's. Add a property test for that
  (`rust-proptest`).
- Watch for full-screen programs that switch to the alternate screen.
  Their final text is whatever the primary screen shows after exit,
  which is usually right.

This emulator runs in `tau-tools`, a library crate. Adding libghostty
there puts a Zig build on every user of the coding tools, against
[0001](../decisions/0001-library-not-product.md) and the "users do not
pay for what they do not use" rule in
[0007](../decisions/0007-gpui-interface.md). Use a pure-Rust emulator
in the tool (`vt100` or `alacritty_terminal`), or put the emulator
behind a cargo feature.

### 4. Record it

Store the raw bytes, or a compact VT snapshot, with the tool result in
`tau-store`, so a reopened run shows the same terminal. libghostty-vt
has a "Terminal Snapshot" API for this, but its format is not stable.
Raw bytes (capped) are the safer format.

## Alternatives

| Crate                | Version, date      | License                          | Scope                                               | Pure Rust | Used by                      | Fit for tau                                                                                   |
| -------------------- | ------------------ | -------------------------------- | --------------------------------------------------- | --------- | ---------------------------- | --------------------------------------------------------------------------------------------- |
| `libghostty-vt`      | 0.2.1, 2026-07-18  | MIT OR Apache-2.0 (Ghostty: MIT) | Parser, state, render state, formatter, encoders    | No (Zig)  | cmux, Ghostling ports        | Best emulation. Unstable API, Zig build, `!Send`.                                             |
| `alacritty_terminal` | 0.26.0, 2026-04-06 | Apache-2.0                       | Parser, grid, scrollback, selection, PTY event loop | Yes       | Alacritty, Zed terminal      | Mature. Proven with GPUI in Zed. Built for interactive terminals.                             |
| `vt100`              | 0.16.2, 2025-07-12 | MIT                              | Parser and screen, `contents()`, cell attrs         | Yes       | many TUI tests and recorders | Small and simple. Good for the model-facing text. Less complete emulation.                    |
| `termwiz`            | 0.23.3, 2025-03-20 | MIT                              | Escape parser, surfaces, terminal I/O               | Yes       | wezterm                      | No full emulator on its own. That is `wezterm-term`, not published on crates.io (unverified). |
| `vte`                | 0.15.0, 2025-02-02 | Apache-2.0 OR MIT                | Parser only                                         | Yes       | Alacritty                    | We would write the grid ourselves. No.                                                        |
| `portable-pty`       | 0.9.0, 2025-02-11  | MIT                              | PTY only                                            | Yes       | wezterm                      | PTY side only; pairs with any of the above.                                                   |

Notes:

- Zed's terminal uses `alacritty_terminal` for state and draws with
  GPUI in `terminal_view`. That is the same split as Option A, and it
  already works with GPUI.
- `alacritty_terminal` expects an `EventListener` and a `Term<T>` it
  locks with a `FairMutex`. It can be fed bytes directly through its
  `vte` processor with no PTY.
- `vt100::Parser::process` plus `screen().contents()` gives the plain
  text in two calls. `rows_formatted` and per-cell attributes are
  enough for a basic styled view.
- libghostty-vt has more complete emulation than the others (Kitty
  graphics, grapheme clustering, reflow, many OSC sequences). Most tool
  output needs little of it: SGR colors, `\r`, erase line and cursor
  up. All of the crates above handle those.

## Recommendation

- Do not depend on libghostty now. Its API is pre-1.0 and still
  changing. It adds a Zig build and a pinned Ghostty input to the Nix
  flake, and its types cannot cross threads. The emulation gain over
  `alacritty_terminal` does not matter much for command output.
- Use `alacritty_terminal` in `tau-ui` for the terminal view, and draw
  with GPUI following Zed's approach. It is pure Rust, stable and
  already proven inside GPUI.
- Use `vt100` in `tau-tools` for the model-facing text. It is small,
  pure Rust, and its `contents()` is exactly the plain text needed.
  (`alacritty_terminal` would also work; `vt100` keeps the library
  lighter.)
- Hide the emulator behind a small trait in `tau-ui`
  (`write(&[u8])`, `resize`, `rows() -> styled runs`, `cursor`), so
  libghostty-vt can replace `alacritty_terminal` later without touching
  the drawing code.
- Look at libghostty again when it has a tagged release and a stable
  C API, or when a renderer library ships.

## Staged plan

1. **Styled text without a PTY.** In `tau-ui`, feed the existing text
   through a VT emulator and draw colored runs instead of the `PASS`
   special case. Set `CLICOLOR_FORCE=1`, `FORCE_COLOR=1` and
   `CARGO_TERM_COLOR=always` in the tool, and strip escapes from the
   model text with the emulator. Model text for plain output stays the
   same.
2. **Raw byte stream.** Send ordered byte chunks in `details.term`,
   flush them before the tool returns, and make `tau-ui` replay them
   into a live emulator per running card. Freeze cards on `ToolEnd`.
3. **PTY behind an option.** Add a `rustix` PTY mode with fixed size,
   null stdin, pager variables and CRLF handling. Add property tests
   that plain output gives the same model text in both modes. Then
   make it the default and record the change in `tools.md`.
4. **Terminal element.** Replace `ToolBody::Output(Vec<String>)` with
   a grid element: runs, backgrounds, wide characters, cursor while
   running, expand to full scrollback, text selection and copy.
5. **Persistence.** Store capped raw bytes with the tool result in
   `tau-store`, so reopened runs render the same.
6. **Re-evaluate libghostty.** Try `libghostty-vt` behind the trait
   from the recommendation, built through the Ghostty flake and
   pkg-config. Switch only if it is stable and shows a visible gain.

## Open questions

- Which terminal size should the model see? A fixed 120 by 40 is
  proposed. Wider sizes cost tokens on redrawn lines.
- Should the UI show the full scrollback or a capped tail when a card
  is expanded?
- Do we need input (typing into a running command)? Not today: tools
  are non-interactive. If ever, it belongs to a separate terminal pane,
  not to tool cards.

## Sources

- [Libghostty Is Coming, Mitchell Hashimoto (2025-09-22)](https://mitchellh.com/writing/libghostty-is-coming)
- [ghostty-org/ghostty README](https://github.com/ghostty-org/ghostty)
- [Add Parser and Terminal C API to libghostty-vt, discussion #11348](https://github.com/ghostty-org/ghostty/discussions/11348)
- [libghostty-vt C API docs](https://libghostty.tip.ghostty.org/)
- [Ghostty tip changelog, 2026-09-25](https://github.com/dive/ghostty-tip-changelog/releases/tag/daily-2026-09-25)
- [Ghostty tip changelog, 2026-09-20](https://github.com/dive/ghostty-tip-changelog/releases/tag/daily-2026-09-20)
- [Uzaaft/libghostty-rs](https://github.com/Uzaaft/libghostty-rs/) (build script, flake and source read at `main`, 2026-09-28)
- [libghostty-vt on crates.io](https://crates.io/crates/libghostty-vt)
- [libghostty-vt on docs.rs](https://docs.rs/libghostty-vt/latest/libghostty_vt/)
- [libghostty-vt-sys on crates.io](https://crates.io/crates/libghostty-vt-sys)
- [aislopware/libghostty-rs fork](https://github.com/aislopware/libghostty-rs)
- [Mitchell Hashimoto on the Rust bindings](https://x.com/mitchellh/status/2037966943282696213)
- [Zed terminal_view source](https://github.com/zed-industries/zed/tree/main/crates/terminal_view/src)
- [Zed terminal crate Cargo.toml](https://github.com/zed-industries/zed/blob/main/crates/terminal/Cargo.toml)
- [alacritty_terminal on crates.io](https://crates.io/crates/alacritty_terminal)
- [vt100 on crates.io](https://crates.io/crates/vt100)
- [termwiz on crates.io](https://crates.io/crates/termwiz)
- [vte on crates.io](https://crates.io/crates/vte)
- [portable-pty on crates.io](https://crates.io/crates/portable-pty)
- [zig in nixpkgs](https://mynixos.com/nixpkgs/package/zig)
- [Pane vs Ghostty (cmux on libghostty, May 2026)](https://runpane.com/compare/ghostty)

Unverified items are marked in the text: the license of Zed's
`terminal_view` crate, whether `wezterm-term` is on crates.io, and
whether Ghostty's Cachix has `libghostty-vt` for every system. The
GPUI backend list comes from
[0007](../decisions/0007-gpui-interface.md), not from GPUI docs.
