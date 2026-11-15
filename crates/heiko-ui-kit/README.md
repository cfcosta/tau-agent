# heiko-ui-kit

heiko's design language for GPUI: the theme's tokens, the bundled icons and
fonts, the shared components, the text field, and marked-up text. The
interface and every plugin's UI take their look from here, so a screen never
writes its own sizes or colors. Changing the look is a change to this crate.

## What it provides

| Module       | What it holds                                                                                  |
| ------------ | ---------------------------------------------------------------------------------------------- |
| `theme`      | `Theme` (every color: `Theme::tokyo_night`), `Type` sizes and faces, `sp` spacing, `Tone`      |
| `components` | Rules, section heads, actions, marks, stamps, ticks, slips, fields, dialogs, tables: see below |
| `assets`     | `Icon`, `State` marks, the `Assets` source GPUI loads SVGs from, `Brand` marks, `load_fonts`   |
| `input`      | `TextInput`, a text field that works with input methods; one line, or `multiline`              |
| `select`     | Text the person can select and copy: `selection_scope`, `selectable`                           |
| `markdown`   | A model's reply parsed into `Block`s with `pulldown-cmark` (`blocks`)                          |
| `prose`      | Marked-up text: `rich`, `prose`, and `markdown`, which draws a reply                           |
| `syntax`     | Tree-sitter highlighting: `Lang`, `Kind`, `highlight`, `highlight_lines`                       |
| `diff`       | Unified diffs: `parse`, `stat`, and `view` to draw one                                         |
| `format`     | Numbers as the interface writes them: `grouped`, `tokens`, `usd`, `clock`                      |
| `design`     | `check`, which finds raw design values in a crate's sources                                    |

The look is flat and printed
([ADR 0057](../../docs/decisions/0057-the-interface-is-flat-and-printed.md)):
no gradients, soft shadows, rounded boxes or pills, and no warm color
as a fill. Structure comes from rules, the one decoration is the hard
offset print, and state is shown with marks, stamps, squares, slips
and ticks.

## Components

| What                | Components                                                                                         |
| ------------------- | -------------------------------------------------------------------------------------------------- |
| Rules and sections  | `rule` (a hairline), `heavy_rule`, `section_header` (a title in the display face over the rule)    |
| Text                | `text`, `mono`, `title`, `screen_title`, `lead`, `heading`, `label`, `big_figure`, `stat`, `empty` |
| The primary action  | `primary_block` with `BlockSize::{Small, Medium, Large}`: a blue block over a pink offset          |
| Other actions       | `link_action` (an underlined word), `text_link`, `link`, `hot_action` (one that needs the person)  |
| Buttons             | `button` and `big_button` with `ButtonKind`, `square_button`, `icon_button`                        |
| State               | `mark` and `counted_mark` with `State` (working, asks, failing, met, limit, cap, idle)             |
| Stamps              | `stamp`, `stamp_in`: a word or two in mono, outlined: `exit 0`, `queued`                           |
| Squares             | `square` with `SquareStyle::{Quiet, Outlined, Filled}`: a step, a check, a place in a queue        |
| Slips               | `slip`, `dashed_slip`, `slip_in`: a note or an id on an outlined label over an offset              |
| Quantities          | `ticks`, `marked_ticks` (thresholds in white), `colored_ticks`, `bar` (a share as ticks)           |
| Names and values    | `chip` (a word the person can change), `tag`, `key_values`, `repo_mark`, `unread_count`            |
| Blocks              | `card` and `panel` (under a hairline, no fill or outline), `band` (a result's flat plane), `rail`  |
| The brand           | `wordmark` (並行 and heiko, overprinted), `logo` (並 alone)                                        |
| News and dialogs    | `notice`, `note` with `NoteHead` (a plugin's note), `dialog`, `modal`, `tile`                      |
| Inputs              | `field`, `switch`, `checkbox`, `hints`, `key_hint`                                                 |
| A reply's blocks    | `table`, `weighted_table`, `code_block`, `numbered_code_block`, `marked_line`, `quote`, `list_item`, `md_heading` |
| Screens             | `screen`, `phone_body`, `qr_code`, `icon`, `dot`, `live_dot`, `status_mark`                        |
| Surfaces            | The `Material` trait on any element: `printed` (the offset), `pressed`, `chrome`, `lifted`, `sunk` |

- Use `primary_block` once or twice a screen, for its main action, and
  `link_action` for the rest.
- A quantity is ticks, never a smooth bar.
- `card`, `panel`, `link` and `bar` keep their names from the earlier
  look and draw in this one. New code uses `section_header`, `rule`,
  `link_action` and `ticks`.

`init` sets up what the kit needs once per app: it loads the fonts, sets
`Theme::tokyo_night` as the global theme, binds the text field's keys, and
binds copying selected text.

The fonts are Geist, Geist Mono, Newsreader and RocknRoll One (the display
face), under the SIL Open Font License (`assets/fonts/`). Syntax highlighting covers Bash, Go, JavaScript,
JSON, Luau, Nix, Python, Rust, TOML, TSX and TypeScript; the Luau
highlight query is in `queries/luau.scm`.

`build.rs` embeds OpenAI's and GitHub's marks when their approved files are
in `assets/brand/` (`chatgpt-mark.svg`, `github-mark.svg`). heiko does not
ship them. Without them, `Brand::svg` is `None` and a neutral placeholder
stands in.

## How it fits

It builds on GPUI and on `heiko-terminal`, whose palette the theme carries
(`Theme::term`). `heiko-ui-plugin`, `heiko-ui-remote`, `heiko-ui` and every
plugin under `crates/plugins` depend on it.

## Usage

Call `init` once, then draw with the theme and the components:

```rust
use gpui::{App, Div, ParentElement as _};
use heiko_ui_kit::{
    assets::State,
    components::{
        BlockSize, link_action, mark, primary_block, section_header, text,
        ticks,
    },
    theme::{Type, theme},
};

fn cloning(cx: &App) -> Div {
    let t = theme(cx);
    gpui::div()
        .child(section_header("Cloning", Some("3 of 8".into()), t))
        .child(mark(State::Working, t))
        .child(text("heiko-agent", Type::BODY, t.text))
        .child(ticks(3, 8, t.blue, t))
        .child(primary_block("Open", BlockSize::Small, t))
        .child(link_action("cancel", t))
}
```

Keep a crate in the design language with a test:

```rust
#[test]
fn only_the_kit_holds_design_values() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let found = heiko_ui_kit::design::check(&src, &[]);
    assert!(found.is_empty(), "{}", found.join("\n"));
}
```

The second argument lists files (relative to `src`) that may hold raw
values. `heiko-ui-remote` allows only `ui/components.rs`.

## Testing

```sh
cargo nextest run --release -p heiko-ui-kit
```

`tests/design.rs` checks the kit's own sources and every plugin's under
`crates/plugins` for raw design values. `tests/syntax.rs` and
`tests/diff.rs` hold Hegel property tests; `tests/select.rs` drives text
selection in GPUI's test app.

## Further reading

- [docs/decisions/0017-plugins-bring-their-ui.md](../../docs/decisions/0017-plugins-bring-their-ui.md)
- [docs/decisions/0007-gpui-interface.md](../../docs/decisions/0007-gpui-interface.md)
