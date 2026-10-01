# 0010: bash output is a terminal, emulated by libghostty-vt

- Status: accepted
- Date: 2026-09-29
- Research: [terminal-rendering.md](../research/terminal-rendering.md),
  which recommended `alacritty_terminal` and `vt100` instead

## Context

`bash` ran commands on pipes. Programs saw no terminal, so they printed
no colors and no progress bars, and tau-ui showed their output as plain
lines. We want tau-ui to show a command's output as a real terminal
screen: a fixed grid, the program's colors, scrollback, and a way to
compare it with the text the model saw.

That needs three things: a pseudo-terminal for the command, a terminal
emulator that turns its bytes into a screen, and a record of the bytes
so a reopened run can draw the same screen.

## Decision

- **libghostty-vt is the emulator**, through the `libghostty-vt` crate
  (0.2.2) of Uzaaft/libghostty-rs. It is Ghostty's own parser and
  screen state: the most complete emulation available (grapheme
  clusters, wide characters, reflow, current OSC and Kitty sequences),
  used every day in Ghostty. Its formatter gives plain text, its render
  state gives styled cells, and its tracked grid references let us read
  every row once as it scrolls away. The model's text and tau-ui's
  screen come from the same engine, so they cannot disagree about what
  the output was.
- **A crate of its own, `tau-terminal`** (`crates/tau-terminal`), depending
  on no other tau crate, so any GPUI app can use it:
  - `Terminal`: write bytes, read plain text, a VT replay, and a styled
    `Screen` snapshot (runs of cells with colors, attributes, wide
    characters and the cursor) that a renderer draws from;
  - `Command`: runs a program under a `rustix` pseudo-terminal of a fixed
    120 by 40 cells, with stdin on `/dev/null`, its own session with the
    terminal as controlling terminal, `TERM=xterm-256color` and every
    pager set to `cat`. It streams the bytes in order, feeds a
    `Terminal` on a thread of its own, and reports the text and the exit
    status;
  - `view`: the GPUI side. `tau-terminal` always depends on `gpui`.
- **`tau-tools` uses it behind a cargo feature, `terminal`.** With the
  feature off, `bash` is exactly the pipe implementation, and neither
  libghostty nor GPUI is built ([0001](0001-library-not-product.md),
  [0007](0007-gpui-interface.md)). With it on, `bash` runs under a
  terminal by default (`Bash::with_terminal(false)` goes back to pipes)
  and adapts tau-terminal's runner to the tool contract. **tau-ui turns
  the feature on**: it is the product, and it draws the terminal.
- **The model's text keeps pi's limits.** It is the terminal's plain
  text, fed through the same accumulator as pipe mode: `truncate_tail`
  to 2000 lines or 50 KiB, and a spill file of plain text for longer
  output. Plain output (no escapes, no `\r`) reads exactly as it does
  with pipes; a property test holds that.
- **The bytes travel in `details.term`** of the progress updates and of
  the result ([tools.md](../reference/tools.md), "bash", "Terminal").
  The result's details are stored with the tool result, so a reopened
  run rebuilds its terminal from the store alone. A failed command keeps
  them too: `ToolError::Output` carries a whole output, details
  included.
- **The Nix flake provides libghostty.** It has the Ghostty flake as an
  input, pinned at the commit `libghostty-vt-sys` 0.2.2 builds against
  (`a887df42c56f6de86c0fe6da9c4eeca37931e083`), and puts its
  `libghostty-vt` package in the dev shell and the package build. The
  `-sys` crate's `pkg-config` feature finds it there and links it
  statically, so cargo never runs git or Zig.

## Consequences

- **The API is pre-1.0.** The crate's README and Ghostty's C API docs
  both expect breaking changes. Every libghostty call is inside
  `tau-terminal`, so an upgrade touches one crate.
- **The Ghostty input and the crate move together.** A crate version
  that pins a new Ghostty commit needs the flake input at that commit in
  the same change, or the library and the bindings disagree. Comments
  in `flake.nix` and the workspace `Cargo.toml` say so.
- **The first build compiles Ghostty.** ghostty.cachix.org and nixpkgs
  do not have this commit's `libghostty-vt`, so Nix builds it with Zig:
  about a minute, after about 280 MiB of downloads (1.7 GiB unpacked).
  Outside Nix, the `-sys` crate falls back to cloning Ghostty with git
  and building it with Zig 0.15.
- **`Terminal` is `!Send`.** libghostty-vt is not thread-safe. The
  runner keeps each command's terminal on a thread of its own and sends
  plain data out; tau-ui keeps its terminals on the GPUI foreground
  thread. `Screen` snapshots are plain data and cross threads.
- **Terminal behavior reaches the model.** A tab becomes spaces, a `\r`
  redraw leaves only its last frame, and what a program drew on the
  alternate screen is gone when it leaves it, as a person would see.
- **Process behavior changes a little.** A command that reads
  `/dev/tty` gets end of file at once, instead of failing to open it.
  Commands start with `SIGHUP` ignored, so a background job outlives the
  shell as it did with pipes.
- **Stored results grow.** A result keeps up to 4 MiB of raw output
  (base64 in its details); past that, a VT snapshot of the final
  screen and scrollback.
