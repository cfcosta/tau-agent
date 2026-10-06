# tau-terminal

A terminal for tool output. It emulates a terminal with libghostty-vt,
runs a command under a pseudo-terminal, and draws the result in a GPUI
view. tau uses it so that `bash` output looks like a real terminal (colors,
progress bars, redraws) in the interface, while the model reads plain text.
The crate depends on no other tau crate, so any GPUI app can use it.

## What it provides

| Item                                 | What it is                                                                                   |
| ------------------------------------ | -------------------------------------------------------------------------------------------- |
| `Terminal`                           | The emulator. Feed bytes with `write`; read `text` (plain), `vt` (replayable) or `snapshot`  |
| `Options`, `Size`, `Scroll`          | How to create a `Terminal`, its size in cells (`Size::TOOL` is 120×40), and viewport scrolls |
| `Screen`, `Line`, `TextRun`, `Style` | A styled snapshot of the terminal, as plain data that crosses threads                        |
| `Command`                            | A builder that runs a program under a pseudo-terminal; `spawn` returns a `Run`               |
| `Run`, `Event`, `Finished`, `Replay` | A running command, its ordered events (`Output`, `Text`, `Screen`, `Exit`) and how it ended  |
| `Killer`                             | Kills a run's whole process group, from any thread                                           |
| `TerminalView`, `ViewOptions`        | A GPUI entity: feed it bytes while a program runs, `freeze` it when it ends                  |
| `TerminalEvent`, `view::bind_keys`   | What the view emits, and the key bindings for copy                                           |
| `Palette`                            | The colors the view draws in                                                                 |
| `Scroller`, `ScrollTo`               | Scroll math for the view                                                                     |
| `Selection`, `Point`, `text_of`      | Selecting cells and reading their text                                                       |
| `glyph`, `layout`                    | Pure drawing helpers: box and block glyphs as shapes, and a row's runs                       |
| `Error`                              | Why a call failed: libghostty-vt, I/O, or a write to a frozen view                           |

`Terminal` wraps libghostty-vt, which is not thread-safe, so it is neither
`Send` nor `Sync`. Create it on the thread that uses it. `Screen` and
everything a `Run` produces are plain data. The runner keeps its own
`Terminal` on a thread of its own.

## How it fits

No tau crate is below it. `tau-tools` and `tau-tools-host` use it behind
their `terminal` feature: `bash` runs commands through `Command`, and its
card draws them with `TerminalView`. `tau-ui-kit` carries the terminal
palette in its theme, and `tau-ui-remote` draws `bash` cards with the view.

libghostty-vt comes from the Nix flake through pkg-config, never from git
and Zig at build time. Build inside the dev shell (`nix develop`).

## Usage

Run a command and collect its plain text:

```rust
use tau_terminal::{Command, Event, Size};

let mut run = Command::new("ls")
    .arg("-la")
    .arg("--color=always")
    .size(Size::TOOL)
    .spawn()?;
let mut text = String::new();
while let Some(event) = run.next().await {
    match event {
        Event::Text(rows) => text.push_str(&rows),
        Event::Exit(Ok(finished)) => text.push_str(&finished.text),
        Event::Exit(Err(error)) => eprintln!("{error}"),
        Event::Output(_) | Event::Screen(_) => {}
    }
}
```

`Command::spawn` needs a tokio runtime. Each `Event::Output` holds the exact
bytes the program wrote; write them to a `TerminalView` to show the run
live, and call `freeze` on `Event::Exit`.

The example opens a window with a live terminal:

```sh
cargo run -p tau-terminal --example run -- ls -la --color=always
```

With no command, it runs a short demo of colors and box drawing.

## Testing

```sh
cargo nextest run --release -p tau-terminal
```

`tests/terminal.rs` and `tests/drawing.rs` hold Hegel property tests (plain
output round trips through `Terminal::text`, recording keeps every row) and
example tests for redraws, colors, wide characters, scrollback and replay.
`tests/run.rs` runs real processes under a pseudo-terminal (Unix only).
`tests/view.rs` drives `TerminalView` in GPUI's test app.

## Further reading

- [docs/reference/terminal.md](../../docs/reference/terminal.md): the view
  and its pure parts
- [docs/reference/tools.md](../../docs/reference/tools.md): `bash` in
  terminal mode
- [docs/decisions/0010-terminal-rendering.md](../../docs/decisions/0010-terminal-rendering.md)
