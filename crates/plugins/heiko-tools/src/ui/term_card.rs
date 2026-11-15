//! A command card's terminal, as a view tree (ADR 0045): its screen,
//! flat, and the button that copies its text. The interface keeps the
//! screen, live while the command runs and frozen once it ends, fed the
//! bytes so far; how the command stands is the card's stamp. When a
//! plugin cut the output, the card holds two tabs: the terminal, and the
//! text the model saw.

use heiko_ui_kit::format::grouped;
use heiko_ui_plugin::{
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

/// The card's body: the screen, then the button that copies its text;
/// on a cut card, the terminal and the text the model saw, as tabs.
/// `id` names the card among the plugin's: its run and call.
pub fn body(id: &str, term: &TermOutput, cut: Option<&OutputCut>) -> Node {
    let screen = Node::Terminal {
        id: format!("term-{id}"),
        text: String::from_utf8_lossy(&term.bytes).into_owned(),
        cols: term.cols,
        rows: term.rows,
        live: term.end.is_none(),
        // A long output folds to its last rows; the person opens it.
        fold: true,
    };
    let mut terminal = vec![screen];
    terminal.extend(copy(term));
    let Some(cut) = cut else {
        return Node::stack(terminal);
    };
    Node::stack([
        Node::Tabs {
            id: format!("term-tabs-{id}"),
            tabs: vec![
                Tab {
                    label: format!("terminal · {} lines", grouped(cut.lines)),
                    children: terminal,
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
            size: Default::default(),
            wide: false,
            detail: None,
        },
    ])
}

/// The button that copies the command's text, without the terminal's
/// colors and moves; none while there is no text.
fn copy(term: &TermOutput) -> Option<Node> {
    let text = term.seen.trim_end();
    (!text.is_empty()).then(|| Node::Button {
        label: "copy".into(),
        action: COPY.into(),
        args: serde_json::json!({ "text": text }),
        kind: Kind::Secondary,
        icon: None,
        size: Default::default(),
        wide: false,
        detail: None,
    })
}

/// The text the model saw, with the cut's marks: omitted lines and its
/// header and footer as quiet lines.
fn seen(term: &TermOutput) -> Node {
    let mut pieces = Vec::new();
    let mut text = Vec::new();
    let flush = |text: &mut Vec<String>, pieces: &mut Vec<Node>| {
        if !text.is_empty() {
            pieces.push(Node::Code {
                lang: None,
                text: text.join("\n"),
                first_line: None,
            });
            text.clear();
        }
    };
    for line in term.seen_lines() {
        match line {
            SeenLine::Text(line) => text.push(line),
            SeenLine::Note(line) => {
                flush(&mut text, &mut pieces);
                pieces.push(quiet(line));
            }
            SeenLine::Omitted(count) => {
                flush(&mut text, &mut pieces);
                pieces
                    .push(quiet(format!("… {} lines omitted", grouped(count))));
            }
        }
    }
    flush(&mut text, &mut pieces);
    Node::Scroll {
        height: Some(24.),
        children: pieces,
    }
}

/// A line that is not the command's own.
fn quiet(text: String) -> Node {
    Node::Toned {
        text,
        tone: Tone::Neutral,
        mono: true,
    }
}
