//! The pieces screens are built from. Each function draws from a
//! [`RunView`](crate::view::RunView) and the [`Theme`]; the workspace
//! decides where they go for the window's width.

pub mod chrome;
pub mod inspector;
pub mod screens;
pub mod transcript;

use gpui::{
    Div,
    FontWeight,
    Hsla,
    IntoElement,
    SharedString,
    Styled,
    StyledText,
    Svg,
    TextRun,
    div,
    font,
    prelude::*,
    px,
    svg,
};
use tau_agent::event::StopReason;

use crate::{
    assets::Icon,
    theme::{MONO, SANS, Theme},
    view::{RunStatus, RunView},
};

pub fn icon(icon: Icon, size: f32, color: Hsla) -> Svg {
    svg()
        .path(icon.path())
        .size(px(size))
        .flex_shrink_0()
        .text_color(color)
}

/// Monospace text.
pub fn mono(text: impl Into<SharedString>, size: f32, color: Hsla) -> Div {
    div()
        .font_family(MONO)
        .text_size(px(size))
        .text_color(color)
        .child(text.into())
}

/// A small uppercase section heading.
pub fn heading(text: &str, t: &Theme) -> Div {
    div()
        .text_size(px(11.))
        .text_color(t.dim)
        .child(text.to_uppercase())
}

/// Prose in `color`, with the two marks models use most: `code` in the
/// monospace face on a chip, and `**bold**`.
pub fn rich(text: &str, color: Hsla, t: &Theme) -> StyledText {
    rich_in(text, SANS, color, t)
}

/// [`rich`] in another body face, such as the serif of a note.
pub fn rich_in(
    text: &str,
    family: &'static str,
    color: Hsla,
    t: &Theme,
) -> StyledText {
    let spans = spans(text);
    let plain: String = spans.iter().map(|(span, _)| span.as_str()).collect();
    let runs = spans
        .iter()
        .map(|(span, mark)| {
            let base = font(family);
            let (font, color, background) = match mark {
                Mark::Plain => (base, color, None),
                Mark::Bold => (
                    gpui::Font {
                        weight: FontWeight::SEMIBOLD,
                        ..base
                    },
                    t.text,
                    None,
                ),
                Mark::Code => (font(MONO), t.text, Some(t.raised)),
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

/// A rounded badge of text.
pub fn pill(text: impl Into<SharedString>, color: Hsla, bg: Hsla) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .px(px(8.))
        .py(px(3.))
        .rounded(px(10.))
        .bg(bg)
        .text_color(color)
        .text_size(px(12.))
        .child(dot(color, 6.))
        .child(text.into())
}

pub fn dot(color: Hsla, size: f32) -> Div {
    div()
        .size(px(size))
        .flex_shrink_0()
        .rounded(px(size / 2.))
        .bg(color)
}

/// A horizontal meter: `share` of the track filled.
pub fn bar(share: f32, height: f32, fill: Hsla, track: Hsla) -> Div {
    div()
        .relative()
        .h(px(height))
        .w_full()
        .rounded(px(height / 2.))
        .bg(track)
        .child(
            div()
                .h(px(height))
                .w(gpui::relative(share.clamp(0.0, 1.0)))
                .rounded(px(height / 2.))
                .bg(fill),
        )
}

/// A labelled button outline; callers add the id and the click.
pub fn button(label: &'static str, t: &Theme) -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .h(px(30.))
        .px(px(10.))
        .rounded(px(6.))
        .border_1()
        .border_color(t.border)
        .text_color(t.text_soft)
        .text_size(px(12.))
        .cursor_pointer()
        .hover(|style| style.bg(gpui::white().opacity(0.04)))
        .child(label)
}

pub fn primary_button(label: impl Into<SharedString>, t: &Theme) -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .h(px(30.))
        .px(px(14.))
        .rounded(px(6.))
        .bg(t.accent)
        .text_color(t.bg)
        .text_size(px(12.))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .hover(|style| style.opacity(0.9))
        .child(label.into())
}

/// Two columns of name and value: names in a fixed column, values
/// left-aligned beside them.
pub fn key_values(
    rows: impl IntoIterator<Item = (SharedString, Div)>,
    t: &Theme,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .children(rows.into_iter().map(|(key, value)| {
            div()
                .flex()
                .items_start()
                .gap(px(12.))
                .child(mono(key, 12., t.muted).w(px(128.)).flex_shrink_0())
                .child(div().flex_1().min_w(px(0.)).child(value))
        }))
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
        StopReason::Stop => (t.green, "finished".into()),
        StopReason::Limit(kind) => {
            (t.red, format!("limit · {kind:?}").to_lowercase().into())
        }
        StopReason::Cancelled => (t.muted, "cancelled".into()),
        StopReason::Error(_) => (t.red, "error".into()),
    }
}

/// The icon a run shows in lists.
pub fn status_icon(run: &RunView, t: &Theme, size: f32) -> gpui::AnyElement {
    match &run.status {
        RunStatus::Planning | RunStatus::Running => {
            dot(t.accent, 8.).into_any_element()
        }
        RunStatus::Finished(StopReason::Stop) => {
            icon(Icon::Check, size, t.green).into_any_element()
        }
        RunStatus::Finished(StopReason::Cancelled) => {
            icon(Icon::Stop, size, t.muted).into_any_element()
        }
        RunStatus::Finished(_) => {
            icon(Icon::Warning, size, t.red).into_any_element()
        }
    }
}

/// Text that navigates somewhere; callers add the id and the click.
pub fn link(label: impl Into<SharedString>, t: &Theme) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(4.))
        .flex_shrink_0()
        .text_size(px(12.))
        .text_color(t.blue)
        .cursor_pointer()
        .hover(|style| style.underline())
        .child(label.into())
        .child(icon(Icon::Chevron, 11., t.blue))
}

/// A bordered box.
pub fn card(t: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .border_1()
        .border_color(t.border)
        .rounded(px(8.))
        .overflow_hidden()
}

/// A small labelled number.
pub fn stat(label: &str, value: impl Into<SharedString>, t: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .px(px(12.))
        .py(px(10.))
        .rounded(px(6.))
        .bg(t.panel)
        .child(mono(label.to_owned(), 12., t.dim))
        .child(mono(value, 15., t.text))
}

/// A screen's title and the sentence under it.
pub fn screen_title(
    title: impl Into<SharedString>,
    subtitle: impl Into<SharedString>,
    t: &Theme,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(
            div()
                .text_size(px(20.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title.into()),
        )
        .child(
            div()
                .max_w(px(760.))
                .text_color(t.muted)
                .line_height(gpui::relative(1.5))
                .child(subtitle.into()),
        )
}

/// A scrolling screen body with the layout's padding.
pub fn screen(
    id: &'static str,
    compact: bool,
    content: impl IntoElement,
) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .flex_1()
        .min_h(px(0.))
        .overflow_y_scroll()
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(18.))
                .px(px(if compact { 16. } else { 32. }))
                .py(px(if compact { 16. } else { 24. }))
                .child(content),
        )
}

/// An empty state.
pub fn empty(text: impl Into<SharedString>, t: &Theme) -> Div {
    div()
        .flex()
        .justify_center()
        .py(px(48.))
        .text_color(t.muted)
        .child(text.into())
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
