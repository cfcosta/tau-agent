# tau-ui-kit

tau's design language for GPUI: the theme's tokens, the bundled icons and
fonts, the shared components, the text field, and marked-up text. The
interface and every plugin's UI take their look from here, so a screen never
writes its own sizes or colors. Changing the look is a change to this crate.

## What it provides

| Module       | What it holds                                                                                  |
| ------------ | ---------------------------------------------------------------------------------------------- |
| `theme`      | `Theme` (every color; `Theme::tokyo_night` is the default), `Type` sizes, `sp` spacing, `Tone` |
| `components` | Text, buttons, badges, chips, cards, notices, dialogs, tables, code blocks, QR codes and more  |
| `assets`     | `Icon`, the `Assets` source GPUI loads SVGs from, `Brand` marks, and `load_fonts`              |
| `input`      | `TextInput`, a text field that works with input methods; one line, or `multiline`              |
| `select`     | Text the person can select and copy: `selection_scope`, `selectable`                           |
| `markdown`   | A model's reply parsed into `Block`s with `pulldown-cmark` (`blocks`)                          |
| `prose`      | Marked-up text: `rich`, `prose`, and `markdown`, which draws a reply                           |
| `syntax`     | Tree-sitter highlighting: `Lang`, `Kind`, `highlight`, `highlight_lines`                       |
| `diff`       | Unified diffs: `parse`, `stat`, and `view` to draw one                                         |
| `format`     | Numbers as the interface writes them: `grouped`, `tokens`, `usd`, `clock`                      |
| `design`     | `check`, which finds raw design values in a crate's sources                                    |

`init` sets up what the kit needs once per app: it loads the fonts, sets
`Theme::tokyo_night` as the global theme, binds the text field's keys, and
binds copying selected text.

The fonts are Geist, Geist Mono and Newsreader, under the SIL Open Font
License (`assets/fonts/`). Syntax highlighting covers Bash, Go, JavaScript,
JSON, Luau, Nix, Python, Rust, TOML, TSX and TypeScript; the Luau
highlight query is in `queries/luau.scm`.

`build.rs` embeds OpenAI's and GitHub's marks when their approved files are
in `assets/brand/` (`chatgpt-mark.svg`, `github-mark.svg`). tau does not
ship them. Without them, `Brand::svg` is `None` and a neutral placeholder
stands in.

## How it fits

It builds on GPUI and on `tau-terminal`, whose palette the theme carries
(`Theme::term`). `tau-ui-plugin`, `tau-ui-remote`, `tau-ui` and every
plugin under `crates/plugins` depend on it.

## Usage

Call `init` once, then draw with the theme and the components:

```rust
use gpui::{App, Div, ParentElement as _};
use tau_ui_kit::{
    assets::Icon,
    components::{ButtonKind, button, card, notice, text},
    theme::{Type, theme},
};

fn waiting(cx: &App) -> Div {
    let t = theme(cx);
    card(t)
        .child(text("Cloning tau-agent", Type::BODY, t.text))
        .child(notice(
            Icon::Spinner,
            "Waiting for GitHub",
            t.blue,
            Type::SMALL,
            t,
        ))
        .child(button("Cancel", ButtonKind::Secondary, t))
}
```

Keep a crate in the design language with a test:

```rust
#[test]
fn only_the_kit_holds_design_values() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let found = tau_ui_kit::design::check(&src, &[]);
    assert!(found.is_empty(), "{}", found.join("\n"));
}
```

The second argument lists files (relative to `src`) that may hold raw
values. `tau-ui-remote` allows only `ui/components.rs`.

## Testing

```sh
cargo nextest run --release -p tau-ui-kit
```

`tests/design.rs` checks the kit's own sources and every plugin's under
`crates/plugins` for raw design values. `tests/syntax.rs` and
`tests/diff.rs` hold Hegel property tests; `tests/select.rs` drives text
selection in GPUI's test app.

## Further reading

- [docs/decisions/0017-plugins-bring-their-ui.md](../../docs/decisions/0017-plugins-bring-their-ui.md)
- [docs/decisions/0007-gpui-interface.md](../../docs/decisions/0007-gpui-interface.md)
