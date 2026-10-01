//! A diff's lines: read from a unified diff, counted, and drawn.

use gpui::{Div, div, prelude::*, px};
use serde::{Deserialize, Serialize};

use crate::theme::{Design as _, MONO, Theme, Type, sp};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffLine {
    pub kind: DiffKind,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffKind {
    Context,
    Added,
    Removed,
}

/// Reads a unified diff into lines, dropping the file and hunk headers.
pub fn parse(diff: &str) -> Vec<DiffLine> {
    const HEADERS: [&str; 9] = [
        "---",
        "+++",
        "@@",
        "diff --git ",
        "new file mode",
        "deleted file mode",
        "old mode",
        "new mode",
        "\\ No newline",
    ];
    diff.lines()
        .filter(|line| !HEADERS.iter().any(|header| line.starts_with(header)))
        .map(|line| {
            let (kind, rest) = match line.chars().next() {
                Some('+') => (DiffKind::Added, &line[1..]),
                Some('-') => (DiffKind::Removed, &line[1..]),
                Some(' ') => (DiffKind::Context, &line[1..]),
                _ => (DiffKind::Context, line),
            };
            DiffLine {
                kind,
                text: rest.to_owned(),
            }
        })
        .collect()
}

/// `+12 −3`.
pub fn stat(lines: &[DiffLine]) -> String {
    let added = lines.iter().filter(|l| l.kind == DiffKind::Added).count();
    let removed = lines.iter().filter(|l| l.kind == DiffKind::Removed).count();
    format!("+{added} −{removed}")
}

/// The lines, each on its own row, added and removed ones tinted.
pub fn view(lines: &[DiffLine], t: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .py(sp(1.5))
        .font_family(MONO)
        .typeset(Type::CAPTION)
        .line_height(px(20.))
        .children(lines.iter().map(|line| {
            let (sign, color, bg) = match line.kind {
                DiffKind::Added => {
                    ("+ ", t.added_text, Some(t.green.opacity(0.12)))
                }
                DiffKind::Removed => {
                    ("- ", t.removed_text, Some(t.red.opacity(0.12)))
                }
                DiffKind::Context => ("  ", t.dim, None),
            };
            div()
                .px(sp(3.))
                .text_color(color)
                .whitespace_nowrap()
                .overflow_hidden()
                .when_some(bg, |row, bg| row.bg(bg))
                .child(format!("{sign}{}", line.text))
        }))
}
