//! The pieces screens are built from. [`components`] holds the shared
//! components, which take their look from the tokens in
//! [`crate::theme`]; this module adds marked-up text and how a run's
//! status reads. The workspace decides where screens go for the window's
//! width.

pub mod chrome;
pub mod components;
pub mod diff_card;
pub mod inspector;
pub mod landing;
pub mod log_card;
pub mod screens;
pub mod status_card;
pub mod term_card;
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

/// A model's reply, drawn from its markdown: paragraphs, headings, lists,
/// quotes, code blocks, tables and rules, with bold, italics, code,
/// strikethrough and links that open in the browser. `id` tells this
/// reply's links apart from other replies'.
pub fn markdown(id: &str, text: &str, color: Hsla, t: &Theme) -> gpui::Div {
    let blocks = crate::markdown::blocks(text);
    div()
        .flex()
        .flex_col()
        .gap(crate::theme::sp(3.))
        .children(draw_blocks(id, &blocks, color, t))
}

fn draw_blocks(
    id: &str,
    blocks: &[crate::markdown::Block],
    color: Hsla,
    t: &Theme,
) -> Vec<gpui::AnyElement> {
    use crate::markdown::Block;
    blocks
        .iter()
        .enumerate()
        .map(|(n, block)| {
            let id = format!("{id}.{n}");
            match block {
                Block::Paragraph(inline) => styled_spans(&id, inline, color, t),
                Block::Heading(level, inline) => {
                    md_heading(*level, styled_spans(&id, inline, t.text, t), t)
                        .into_any_element()
                }
                Block::Code { lang, text } => {
                    code_block(lang.as_deref(), text, t).into_any_element()
                }
                Block::List { start, items } => div()
                    .flex()
                    .flex_col()
                    .gap(crate::theme::sp(1.5))
                    .children(items.iter().enumerate().map(|(k, item)| {
                        let marker = match start {
                            Some(first) => format!("{}.", first + k as u64),
                            None => "•".to_owned(),
                        };
                        let id = format!("{id}.{k}");
                        list_item(marker, draw_blocks(&id, item, color, t), t)
                    }))
                    .into_any_element(),
                Block::Quote(quoted) => {
                    quote(draw_blocks(&id, quoted, t.muted, t), t)
                        .into_any_element()
                }
                Block::Table { align, head, rows } => {
                    let cell = |at: String, inline| {
                        styled_spans(&at, inline, color, t)
                    };
                    table(
                        align,
                        head.iter()
                            .enumerate()
                            .map(|(k, inline)| {
                                cell(format!("{id}.h{k}"), inline)
                            })
                            .collect(),
                        rows.iter()
                            .enumerate()
                            .map(|(r, row)| {
                                row.iter()
                                    .enumerate()
                                    .map(|(k, inline)| {
                                        cell(format!("{id}.{r}.{k}"), inline)
                                    })
                                    .collect()
                            })
                            .collect(),
                        t,
                    )
                    .into_any_element()
                }
                Block::Rule => rule(t).into_any_element(),
            }
        })
        .collect()
}

/// Styled spans as one wrapping text: bold and italics in the body
/// face, code on a chip in the monospace face, links in blue, underlined,
/// opening in the browser.
fn styled_spans(
    id: &str,
    inline: &[crate::markdown::Span],
    color: Hsla,
    t: &Theme,
) -> gpui::AnyElement {
    let mut text = String::new();
    let mut runs = Vec::new();
    let mut links: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    for span in inline {
        // Code chips get a thin space of padding on each side.
        let body = if span.code {
            format!("\u{2009}{}\u{2009}", span.text)
        } else {
            span.text.clone()
        };
        let start = text.len();
        text.push_str(&body);
        let mut face = font(if span.code { MONO } else { SANS });
        if span.bold {
            face.weight = weight::STRONG;
        }
        if span.italic {
            face.style = gpui::FontStyle::Italic;
        }
        let tint = if span.link.is_some() {
            t.blue
        } else if span.code || span.bold {
            t.text
        } else {
            color
        };
        runs.push(TextRun {
            len: body.len(),
            font: face,
            color: tint,
            background_color: span.code.then_some(t.raised),
            underline: span.link.as_ref().map(|_| gpui::UnderlineStyle {
                color: Some(t.blue),
                thickness: px(1.),
                wavy: false,
            }),
            strikethrough: span.strike.then_some(gpui::StrikethroughStyle {
                color: Some(color),
                thickness: px(1.),
            }),
        });
        if let Some(url) = &span.link {
            links.push((start..text.len(), url.clone()));
        }
    }
    let styled = StyledText::new(text).with_runs(runs);
    if links.is_empty() {
        return styled.into_any_element();
    }
    let (ranges, urls): (Vec<_>, Vec<_>) = links.into_iter().unzip();
    gpui::InteractiveText::new(SharedString::from(id.to_owned()), styled)
        .on_click(ranges, move |at, _, cx| cx.open_url(&urls[at]))
        .into_any_element()
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

/// The text [`rich`] draws and its runs.
fn marked_runs(
    text: &str,
    family: &'static str,
    color: Hsla,
    chips: bool,
    t: &Theme,
) -> (String, Vec<TextRun>) {
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
    (plain, runs)
}

fn marked(
    text: &str,
    family: &'static str,
    color: Hsla,
    chips: bool,
    t: &Theme,
) -> StyledText {
    let (plain, runs) = marked_runs(text, family, color, chips, t);
    StyledText::new(plain).with_runs(runs)
}

/// How wide [`rich`] draws `text` on one line, at the window's current
/// text size: the widest of its lines.
///
/// For a box that shrinks to its text: given this width, GPUI measures
/// the text at the width it draws it. Left to find the width itself, it
/// can size the box for fewer lines than it then draws.
pub fn rich_width(
    text: &str,
    t: &Theme,
    window: &gpui::Window,
) -> gpui::Pixels {
    let size = window.text_style().font_size.to_pixels(window.rem_size());
    text.split('\n')
        .map(|line| {
            let (plain, runs) = marked_runs(line, SANS, t.text, true, t);
            window
                .text_system()
                .shape_line(plain.into(), size, &runs, None)
                .width
        })
        .fold(px(0.), gpui::Pixels::max)
        .ceil()
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
    use hegel::generators as gs;

    use super::*;

    /// Spans written back with their marks, the chips' padding taken
    /// off, are the text they came from.
    fn rewrite(spans: &[(String, Mark)]) -> String {
        spans
            .iter()
            .map(|(text, mark)| match mark {
                Mark::Plain => text.clone(),
                Mark::Bold => format!("**{text}**"),
                Mark::Code => {
                    let inner = text
                        .strip_prefix('\u{2009}')
                        .and_then(|text| text.strip_suffix('\u{2009}'))
                        .expect("a code chip is padded");
                    format!("`{inner}`")
                }
            })
            .collect()
    }

    /// Any text splits into spans that write back to it: no text is
    /// lost or added, no span is empty but a mark's, a bold span holds
    /// no `**` and a code span no backtick, and two plain spans never
    /// sit side by side.
    #[hegel::test(test_cases = 500)]
    fn spans_write_back_to_their_text(tc: hegel::TestCase) {
        let text: String = tc
            .draw(
                gs::vecs(gs::sampled_from(vec![
                    "a", "b c", "*", "**", "`", " ", "\u{2009}", "é",
                ]))
                .max_size(16),
            )
            .concat();
        let found = spans(&text);
        assert_eq!(rewrite(&found), text);
        for (body, mark) in &found {
            match mark {
                Mark::Plain => assert!(!body.is_empty(), "{found:?}"),
                Mark::Bold => assert!(!body.contains("**"), "{found:?}"),
                Mark::Code => assert!(!body.contains('`'), "{found:?}"),
            }
        }
        assert!(
            found
                .windows(2)
                .all(|pair| !(pair[0].1 == Mark::Plain
                    && pair[1].1 == Mark::Plain)),
            "{found:?}"
        );
    }

    /// Text with no backtick and no `**` is one plain span.
    #[hegel::test]
    fn unmarked_text_is_one_plain_span(tc: hegel::TestCase) {
        let text: String = tc.draw(gs::text().min_size(1).max_size(40));
        tc.assume(!text.contains('`') && !text.contains("**"));
        assert_eq!(spans(&text), [(text.clone(), Mark::Plain)]);
    }

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
