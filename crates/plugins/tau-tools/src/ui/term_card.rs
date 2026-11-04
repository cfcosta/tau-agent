//! A command card's terminal, as a view tree (ADR 0045): its screen
//! under a strip. The interface keeps the screen, live while the command
//! runs and frozen once it ends, fed the bytes so far. When a plugin cut
//! the output, the card holds two tabs: the terminal, and the text the
//! model saw.

use tau_ui_kit::format::grouped;
use tau_ui_plugin::{
    OutputCut,
    tree::{Kind, Node, Tab, Tone},
};

use super::term::{SeenLine, TermOutput};

/// The action of the button that opens a cut's full output, with
/// `{ "path": archive }`.
pub const OPEN: &str = "open";

/// The action of the button that copies the command's text, with
/// `{ "text": ... }`.
pub const COPY: &str = "copy";

/// The card's body: the strip, then the screen; on a cut card, the
/// terminal and the text the model saw, as tabs. `id` names the card
/// among the plugin's: its run and call.
pub fn body(id: &str, term: &TermOutput, cut: Option<&OutputCut>) -> Node {
    let live = term.end.is_none();
    let screen = Node::Terminal {
        id: format!("term-{id}"),
        text: String::from_utf8_lossy(&term.bytes).into_owned(),
        cols: term.cols,
        rows: term.rows,
        live,
    };
    let Some(cut) = cut else {
        return Node::stack([strip(term, live), screen]);
    };
    Node::stack([
        Node::Tabs {
            id: format!("term-tabs-{id}"),
            tabs: vec![
                Tab {
                    label: format!("terminal · {} lines", grouped(cut.lines)),
                    children: vec![strip(term, live), screen],
                },
                Tab {
                    label: format!("model saw · {} lines", grouped(cut.kept)),
                    children: vec![seen(term)],
                },
            ],
        },
        Node::Button {
            label: "open full output".into(),
            action: OPEN.into(),
            args: serde_json::json!({ "path": cut.archive }),
            kind: Kind::Secondary,
            icon: None,
        },
    ])
}

/// The terminal's size and how it stands: `120×40 · xterm-256color` and
/// `live` while it runs, `no output` once it ended without any.
fn strip(term: &TermOutput, live: bool) -> Node {
    let label = if live {
        format!("{} · xterm-256color", term.size_label())
    } else if term.bytes.is_empty() {
        format!("{} · no output", term.size_label())
    } else {
        term.size_label()
    };
    let mut row = vec![Node::Mono { text: label }, Node::Spacer];
    if live {
        row.push(Node::Pill {
            text: "live".into(),
            tone: Tone::Info,
        });
    }
    // Its text, without the terminal's colors and moves.
    let text = term.seen.trim_end();
    if !text.is_empty() {
        row.push(Node::Button {
            label: String::new(),
            action: COPY.into(),
            args: serde_json::json!({ "text": text }),
            kind: Kind::Secondary,
            icon: Some("copy".into()),
        });
    }
    Node::row(row)
}

/// The text the model saw, with the cut's marks: omitted lines as small
/// badges, its header and footer as quiet lines.
fn seen(term: &TermOutput) -> Node {
    let mut pieces = Vec::new();
    let mut text = Vec::new();
    let flush = |text: &mut Vec<String>, pieces: &mut Vec<Node>| {
        if !text.is_empty() {
            pieces.push(Node::Code {
                lang: None,
                text: text.join("\n"),
            });
            text.clear();
        }
    };
    for line in term.seen_lines() {
        match line {
            SeenLine::Text(line) => text.push(line),
            SeenLine::Note(line) => {
                flush(&mut text, &mut pieces);
                pieces.push(Node::Mono { text: line });
            }
            SeenLine::Omitted(count) => {
                flush(&mut text, &mut pieces);
                pieces.push(Node::Badge {
                    text: format!("{} lines omitted", grouped(count)),
                    tone: Tone::Info,
                });
            }
        }
    }
    flush(&mut text, &mut pieces);
    Node::Scroll {
        height: Some(24.),
        children: pieces,
    }
}
