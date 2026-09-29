//! The pieces screens are built from. [`components`] holds the shared
//! components, which take their look from the tokens in
//! [`crate::theme`]; this module adds marked-up text and how a run's
//! status reads. The workspace decides where screens go for the window's
//! width.

pub mod chrome;
pub mod components;
pub mod inspector;
pub mod screens;
pub mod transcript;

pub use components::*;
use gpui::{
    Hsla,
    IntoElement,
    SharedString,
    StyledText,
    TextRun,
    div,
    font,
    prelude::*,
    px,
};
use tau_agent::event::StopReason;

use crate::{
    assets::Icon,
    theme::{IconSize, MONO, SANS, Theme, weight},
    view::{RunStatus, RunView},
};

/// Prose in `color`, with the two marks models use most: `code` in the
/// monospace face on a chip, and `**bold**`.
pub fn rich(text: &str, color: Hsla, t: &Theme) -> StyledText {
    rich_in(text, SANS, color, t)
}

/// A model's reply: its prose with [`rich`] marks, and its pipe tables
/// drawn as tables.
pub fn markdown(text: &str, color: Hsla, t: &Theme) -> gpui::Div {
    use crate::markdown::{Block, blocks};
    div().flex().flex_col().gap(crate::theme::sp(3.)).children(
        blocks(text).into_iter().filter_map(|block| match block {
            Block::Prose(prose) if prose.trim().is_empty() => None,
            Block::Prose(prose) => Some(
                div()
                    .child(rich(prose.trim_matches('\n'), color, t))
                    .into_any_element(),
            ),
            Block::Table { align, head, rows } => Some(
                table(&align, &head, &rows, t, |text, color| {
                    rich(text, color, t).into_any_element()
                })
                .into_any_element(),
            ),
        }),
    )
}

/// [`rich`] in another body face, such as the serif of a note.
pub fn rich_in(
    text: &str,
    family: &'static str,
    color: Hsla,
    t: &Theme,
) -> StyledText {
    marked(text, family, color, true, t)
}

/// Prose where `code` is only set in the monospace face, with no chip:
/// for paths and names inside a sentence.
pub fn prose(text: &str, color: Hsla, t: &Theme) -> StyledText {
    marked(text, SANS, color, false, t)
}

fn marked(
    text: &str,
    family: &'static str,
    color: Hsla,
    chips: bool,
    t: &Theme,
) -> StyledText {
    let spans: Vec<(String, Mark)> = spans(text)
        .into_iter()
        .map(|(span, mark)| match mark {
            Mark::Code if !chips => {
                (span.trim_matches('\u{2009}').to_owned(), mark)
            }
            _ => (span, mark),
        })
        .collect();
    let plain: String = spans.iter().map(|(span, _)| span.as_str()).collect();
    let runs = spans
        .iter()
        .map(|(span, mark)| {
            let base = font(family);
            let (font, color, background) = match mark {
                Mark::Plain => (base, color, None),
                Mark::Bold => (
                    gpui::Font {
                        weight: weight::STRONG,
                        ..base
                    },
                    t.text,
                    None,
                ),
                Mark::Code if chips => (font(MONO), t.text, Some(t.raised)),
                Mark::Code => (font(MONO), color, None),
            };
            TextRun {
                len: span.len(),
                font,
                color,
                background_color: background,
                underline: None,
                strikethrough: None,
            }
        })
        .filter(|run| run.len > 0)
        .collect();
    StyledText::new(plain).with_runs(runs)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mark {
    Plain,
    Bold,
    Code,
}

/// Splits text into plain, `**bold**` and `` `code` `` spans. Marks
/// inside code stay literal; an unclosed mark is plain text.
fn spans(text: &str) -> Vec<(String, Mark)> {
    let mut spans = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let code = rest.find('`');
        let bold = rest.find("**");
        let (at, mark, open, close) = match (code, bold) {
            (Some(c), Some(b)) if b < c => (b, Mark::Bold, 2, "**"),
            (Some(c), _) => (c, Mark::Code, 1, "`"),
            (None, Some(b)) => (b, Mark::Bold, 2, "**"),
            (None, None) => break,
        };
        let inner = &rest[at + open..];
        let Some(end) = inner.find(close) else { break };
        if at > 0 {
            spans.push((rest[..at].to_owned(), Mark::Plain));
        }
        // Code chips get a thin space of padding on each side.
        let body = &inner[..end];
        spans.push(match mark {
            Mark::Code => (format!("\u{2009}{body}\u{2009}"), Mark::Code),
            other => (body.to_owned(), other),
        });
        rest = &inner[end + close.len()..];
    }
    if !rest.is_empty() {
        spans.push((rest.to_owned(), Mark::Plain));
    }
    spans
}

/// The dot color and words a run's status reads as.
pub fn status_look(status: &RunStatus, t: &Theme) -> (Hsla, SharedString) {
    match status {
        RunStatus::Planning => (t.blue, "planning".into()),
        RunStatus::Running => (t.accent, "running".into()),
        RunStatus::Finished(stop) => stop_look(stop, t),
    }
}

pub fn stop_look(stop: &StopReason, t: &Theme) -> (Hsla, SharedString) {
    match stop {
        // A conversation that stopped is not over: it waits for the user.
        StopReason::Stop => (t.muted, "your turn".into()),
        StopReason::Limit(kind) => {
            (t.red, format!("limit · {kind:?}").to_lowercase().into())
        }
        StopReason::Cancelled => (t.muted, "cancelled".into()),
        StopReason::Error(_) => (t.red, "error".into()),
    }
}

/// The icon a run shows in lists.
pub fn status_icon(
    run: &RunView,
    t: &Theme,
    size: IconSize,
) -> gpui::AnyElement {
    match &run.status {
        RunStatus::Planning | RunStatus::Running => {
            dot(t.accent, 8.).into_any_element()
        }
        // Waiting for the user, whether it stopped or was stopped.
        RunStatus::Finished(StopReason::Stop | StopReason::Cancelled) => div()
            .size(px(size.0))
            .flex()
            .items_center()
            .justify_center()
            .child(dot(t.border_strong, 6.))
            .into_any_element(),
        RunStatus::Finished(_) => {
            icon(Icon::Warning, size, t.red).into_any_element()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_split_into_spans() {
        let found = spans("see `a**b` and **bold** then `x");
        let marks: Vec<Mark> = found.iter().map(|(_, m)| *m).collect();
        assert_eq!(
            marks,
            [
                Mark::Plain,
                Mark::Code,
                Mark::Plain,
                Mark::Bold,
                Mark::Plain
            ]
        );
        assert_eq!(found[1].0, "\u{2009}a**b\u{2009}");
        assert_eq!(found[3].0, "bold");
        assert_eq!(found[4].0, " then `x");
        assert_eq!(spans("plain").len(), 1);
        assert!(spans("").is_empty());
    }
}
