//! Marked-up text: prose with `code` and **bold**, and a model's reply
//! drawn from its markdown ([`crate::markdown`] parses it).

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

use crate::{
    components::*,
    theme::{MONO, SANS, Theme, weight},
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
            t.roles.link
        } else if span.code {
            t.roles.code
        } else if span.bold {
            t.text
        } else {
            color
        };
        runs.push(TextRun {
            len: body.len(),
            font: face,
            color: tint,
            background_color: None,
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
                Mark::Code if chips => (font(MONO), t.roles.code, None),
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

#[cfg(test)]
mod tests {
    use hegel::generators as gs;

    use super::*;

    const MARKUP_ALPHABET: &str = "abcdefghijklmnopqrstuvwxyz é雪😀";

    #[derive(Debug, Clone, Copy)]
    enum MarkedKind {
        Bold,
        Code,
    }

    #[derive(Debug)]
    struct MarkedSection {
        kind: MarkedKind,
        body: String,
    }

    #[derive(Debug)]
    struct MarkupDescription {
        leading_plain: Option<String>,
        sections: Vec<MarkedSection>,
        interstitial_plain: Option<String>,
        trailing_plain: Option<String>,
    }

    fn marked_body() -> impl hegel::PrintableGenerator<String> {
        gs::text().alphabet(MARKUP_ALPHABET).min_size(1).max_size(8)
    }

    fn plain_fragment() -> impl hegel::PrintableGenerator<String> {
        gs::text().alphabet(MARKUP_ALPHABET).max_size(8)
    }

    fn append_expected_plain(spans: &mut Vec<(String, Mark)>, text: &str) {
        if text.is_empty() {
            return;
        }
        if let Some((previous, Mark::Plain)) = spans.last_mut() {
            previous.push_str(text);
        } else {
            spans.push((text.to_owned(), Mark::Plain));
        }
    }

    /// Prints a semantic description and builds its expected spans directly
    /// from that description, without asking `spans` to define the oracle.
    fn print_markup(
        description: &MarkupDescription,
    ) -> (String, Vec<(String, Mark)>) {
        let mut rendered = String::new();
        let mut expected = Vec::new();

        if let Some(leading) = &description.leading_plain {
            rendered.push_str(leading);
            append_expected_plain(&mut expected, leading);
        }

        for (index, section) in description.sections.iter().enumerate() {
            if index > 0 {
                rendered.push_str(" / ");
                append_expected_plain(&mut expected, " / ");
                if let Some(interstitial) = &description.interstitial_plain {
                    rendered.push_str(interstitial);
                    append_expected_plain(&mut expected, interstitial);
                }
            }

            match section.kind {
                MarkedKind::Bold => {
                    rendered.push_str("**");
                    rendered.push_str(&section.body);
                    rendered.push_str("**");
                    expected.push((section.body.clone(), Mark::Bold));
                }
                MarkedKind::Code => {
                    rendered.push('`');
                    rendered.push_str(&section.body);
                    rendered.push('`');
                    expected.push((
                        format!("\u{2009}{}\u{2009}", section.body),
                        Mark::Code,
                    ));
                }
            }
        }

        if let Some(trailing) = &description.trailing_plain {
            rendered.push_str(trailing);
            append_expected_plain(&mut expected, trailing);
        }

        (rendered, expected)
    }

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
    fn arbitrary_text_spans_write_back_without_losing_text(
        tc: hegel::TestCase,
    ) {
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

    /// Property inventory: `spans_recognize_generated_bold_and_code_sections`
    /// compares parsed spans with the independent `MarkupDescription` AST
    /// printer and exact expected-span oracle. Bodies and optional plain
    /// fragments use at most 8 characters from a bounded alphabet without
    /// markup delimiters; code also has a fixed `**` variant. Every AST has
    /// Bold and Code witnesses, while 0..5 optional marks and optional plain
    /// fragments shrink away to those mandatory witnesses. Adjacent expected
    /// plain fragments merge.
    #[hegel::test(test_cases = 500)]
    fn spans_recognize_generated_bold_and_code_sections(tc: hegel::TestCase) {
        let leading_plain: Option<String> =
            tc.draw(gs::optional(plain_fragment()));
        let bold_body: String = tc.draw(marked_body());
        let code_body: String =
            tc.draw(hegel::one_of!(gs::just("**".to_owned()), marked_body(),));
        let optional_marks: Vec<(bool, String)> = tc.draw(
            gs::vecs(hegel::tuples!(gs::booleans(), marked_body())).max_size(5),
        );
        let interstitial_plain: Option<String> =
            tc.draw(gs::optional(plain_fragment()));
        let trailing_plain: Option<String> =
            tc.draw(gs::optional(plain_fragment()));

        let mut sections = vec![
            MarkedSection {
                kind: MarkedKind::Bold,
                body: bold_body,
            },
            MarkedSection {
                kind: MarkedKind::Code,
                body: code_body,
            },
        ];
        sections.extend(optional_marks.into_iter().map(|(is_bold, body)| {
            MarkedSection {
                kind: if is_bold {
                    MarkedKind::Bold
                } else {
                    MarkedKind::Code
                },
                body,
            }
        }));

        let description = MarkupDescription {
            leading_plain,
            sections,
            interstitial_plain,
            trailing_plain,
        };
        let (rendered, expected) = print_markup(&description);
        let found = spans(&rendered);

        assert_eq!(found, expected, "description: {description:?}");
        assert_eq!(rewrite(&found), rendered, "description: {description:?}");
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
