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
///
/// Each `@@ -a,b +c,d @@` header says how many old and new lines its
/// hunk holds, and only those lines are read as the diff's own. So a
/// removed `-- comment` (written `--- comment`) or an added `++x`
/// (written `+++x`) stays a line, not a header.
pub fn parse(diff: &str) -> Vec<DiffLine> {
    let mut lines = Vec::new();
    // Old and new lines the current hunk has left to give.
    let (mut old, mut new) = (0usize, 0usize);
    for line in diff.lines() {
        if old == 0 && new == 0 {
            if line.starts_with("@@") {
                (old, new) = hunk_lengths(line);
            }
            continue;
        }
        let (kind, text) = match line.split_at_checked(1) {
            Some(("+", text)) => (DiffKind::Added, text),
            Some(("-", text)) => (DiffKind::Removed, text),
            Some((" ", text)) => (DiffKind::Context, text),
            // `\ No newline at end of file` belongs to the line before.
            Some(("\\", _)) => continue,
            // An empty context line an editor trimmed.
            _ => (DiffKind::Context, ""),
        };
        match kind {
            DiffKind::Added => new = new.saturating_sub(1),
            DiffKind::Removed => old = old.saturating_sub(1),
            DiffKind::Context => {
                old = old.saturating_sub(1);
                new = new.saturating_sub(1);
            }
        }
        lines.push(DiffLine {
            kind,
            text: text.to_owned(),
        });
    }
    lines
}

/// The old and new line counts of `@@ -38,7 +38,12 @@`. A side with no
/// count (`-38`) holds one line.
fn hunk_lengths(header: &str) -> (usize, usize) {
    let length = |sign: char| {
        header
            .split_whitespace()
            .find_map(|part| part.strip_prefix(sign))
            .map_or(0, |range| match range.split_once(',') {
                Some((_, count)) => count.parse().unwrap_or(0),
                None => 1,
            })
    };
    (length('-'), length('+'))
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
