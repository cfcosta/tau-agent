//! A diff's lines: read from a unified diff, counted, and drawn.

use gpui::{Div, div, prelude::*, rems};
use serde::{Deserialize, Serialize};

use crate::{
    syntax::{self, Lang},
    theme::{Design as _, MONO, Theme, Type, sp},
};

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
    read(diff).into_iter().map(|(line, _)| line).collect()
}

/// The number of each line [`parse`] reads, in its own version of the
/// file: an added or unchanged line's in the new file, a removed line's
/// in the old one.
pub fn line_numbers(diff: &str) -> Vec<u32> {
    read(diff).into_iter().map(|(_, number)| number).collect()
}

/// The diff's lines, each with its number in its version of the file.
fn read(diff: &str) -> Vec<(DiffLine, u32)> {
    let mut lines = Vec::new();
    // Old and new lines the current hunk has left to give, and the
    // number of the next one on each side.
    let (mut old, mut new) = (0usize, 0usize);
    let (mut old_at, mut new_at) = (0u32, 0u32);
    for line in diff.lines() {
        if old == 0 && new == 0 {
            if line.starts_with("@@") {
                ((old_at, old), (new_at, new)) = hunk_ranges(line);
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
        let number = match kind {
            DiffKind::Added => {
                new = new.saturating_sub(1);
                new_at += 1;
                new_at - 1
            }
            DiffKind::Removed => {
                old = old.saturating_sub(1);
                old_at += 1;
                old_at - 1
            }
            DiffKind::Context => {
                old = old.saturating_sub(1);
                new = new.saturating_sub(1);
                old_at += 1;
                new_at += 1;
                new_at - 1
            }
        };
        lines.push((
            DiffLine {
                kind,
                text: text.to_owned(),
            },
            number,
        ));
    }
    lines
}

/// Where the old and new sides of `@@ -38,7 +38,12 @@` start and how
/// many lines each holds. A side with no count (`-38`) holds one line.
fn hunk_ranges(header: &str) -> ((u32, usize), (u32, usize)) {
    let range = |sign: char| {
        header
            .split_whitespace()
            .find_map(|part| part.strip_prefix(sign))
            .map_or((0, 0), |range| match range.split_once(',') {
                Some((start, count)) => {
                    (start.parse().unwrap_or(0), count.parse().unwrap_or(0))
                }
                None => (range.parse().unwrap_or(0), 1),
            })
    };
    (range('-'), range('+'))
}

/// How strongly unchanged lines around a change are colored.
pub const CONTEXT_OPACITY: f32 = 0.7;

/// Each diff line's highlighted parts. The old side (context and
/// removed lines) and the new side (context and added lines) are each
/// parsed as code of their own, so a line is colored as it reads in
/// its version of the file.
pub fn colors(lines: &[(DiffKind, &str)], lang: Lang) -> syntax::Lines {
    let side = |drop: DiffKind| {
        let texts: Vec<&str> = lines
            .iter()
            .filter(|(kind, _)| *kind != drop)
            .map(|(_, text)| *text)
            .collect();
        syntax::highlight_lines(lang, &texts).into_iter()
    };
    let (mut old, mut new) = (side(DiffKind::Added), side(DiffKind::Removed));
    lines
        .iter()
        .map(|(kind, _)| {
            match kind {
                DiffKind::Removed => old.next(),
                DiffKind::Added => new.next(),
                DiffKind::Context => {
                    old.next();
                    new.next()
                }
            }
            .unwrap_or_default()
        })
        .collect()
}

/// `+12 −3`.
pub fn stat(lines: &[DiffLine]) -> String {
    let (added, removed) = counts(lines);
    format!("+{added} −{removed}")
}

/// How many lines the diff adds and removes.
pub fn counts(lines: &[DiffLine]) -> (usize, usize) {
    let added = lines.iter().filter(|l| l.kind == DiffKind::Added).count();
    let removed = lines.iter().filter(|l| l.kind == DiffKind::Removed).count();
    (added, removed)
}

/// The squares of a diffstat.
pub const SQUARES: usize = 5;

/// A diffstat's [`SQUARES`] squares: how many are green and how many
/// red, by the share of lines added and removed. A side with any lines
/// keeps a square; a diff that changes none has no squares lit.
pub fn squares(added: usize, removed: usize) -> (usize, usize) {
    let total = added + removed;
    if total == 0 {
        return (0, 0);
    }
    if removed == 0 {
        return (SQUARES, 0);
    }
    if added == 0 {
        return (0, SQUARES);
    }
    let green = ((added * SQUARES + total / 2) / total).clamp(1, SQUARES - 1);
    (green, SQUARES - green)
}

/// The lines, each a band across: added ones on green, removed ones on
/// red.
pub fn view(lines: &[DiffLine], lang: Option<Lang>, t: &Theme) -> Div {
    numbered(lines, None, lang, t)
}

/// [`view`], each line after its number in `numbers` ([`line_numbers`]),
/// muted.
pub fn numbered(
    lines: &[DiffLine],
    numbers: Option<&[u32]>,
    lang: Option<Lang>,
    t: &Theme,
) -> Div {
    let colors = lang.map(|lang| {
        colors(
            &lines
                .iter()
                .map(|line| (line.kind, line.text.as_str()))
                .collect::<Vec<_>>(),
            lang,
        )
    });
    let context = t.syntax.faded(CONTEXT_OPACITY);
    // The widest number's digits, so the code lines up.
    let digits = numbers
        .and_then(|numbers| numbers.iter().max())
        .map_or(0, |widest| widest.to_string().len());
    div()
        .w_full()
        .flex()
        .flex_col()
        .py(sp(2.))
        .bg(t.card)
        .font_family(MONO)
        .typeset(Type::CAPTION)
        .line_height(rems(1.25))
        .children(lines.iter().enumerate().map(|(i, line)| {
            let (color, bg) = match line.kind {
                DiffKind::Added => (t.added_text, Some(t.green.opacity(0.12))),
                DiffKind::Removed => {
                    (t.removed_text, Some(t.red.opacity(0.12)))
                }
                DiffKind::Context => (t.text_soft, None),
            };
            let number =
                numbers.and_then(|numbers| numbers.get(i)).map(|number| {
                    div()
                        .flex_shrink_0()
                        .pr(sp(3.))
                        .text_color(t.dim)
                        .child(format!("{number:>digits$}"))
                });
            div()
                .w_full()
                .px(sp(3.5))
                .text_color(color)
                .whitespace_nowrap()
                .overflow_hidden()
                .when_some(bg, |row, bg| row.bg(bg))
                .flex()
                .children(number)
                .child(syntax::styled(
                    &line.text,
                    colors.as_ref().map(|colors| colors[i].as_slice()),
                    color,
                    if line.kind == DiffKind::Context {
                        &context
                    } else {
                        &t.syntax
                    },
                ))
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each line is numbered in its own version of the file: a removed
    /// line in the old one, the others in the new one.
    #[test]
    fn lines_are_numbered_in_their_version_of_the_file() {
        let diff = "--- a\n+++ b\n@@ -14,3 +14,4 @@\n keep\n-old\n+new\n+more\n                     end\n@@ -40 +41 @@\n-x\n+y\n";
        assert_eq!(line_numbers(diff), [14, 15, 15, 16, 17, 40, 41]);
        assert_eq!(line_numbers(diff).len(), parse(diff).len());
    }

    /// The squares follow the share of lines added, and a side with any
    /// lines keeps one.
    #[test]
    fn a_diffstat_has_five_squares_by_share() {
        assert_eq!(squares(0, 0), (0, 0));
        assert_eq!(squares(38, 0), (5, 0));
        assert_eq!(squares(0, 2), (0, 5));
        assert_eq!(squares(3, 1), (4, 1));
        assert_eq!(squares(1, 1), (3, 2));
        assert_eq!(squares(500, 1), (4, 1));
        assert_eq!(squares(1, 500), (1, 4));
    }
}
