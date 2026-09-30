//! A model's reply as blocks of styled text, read with `pulldown-cmark`:
//! CommonMark with GitHub's tables, strikethrough and task lists.
//! [`crate::ui::markdown`] draws it.
//!
//! Soft line breaks read as spaces, as CommonMark has it; a hard break
//! (two trailing spaces or a backslash) is a new line. HTML is shown as
//! written. A table still streaming in reads as a paragraph until its
//! delimiter row arrives.

use pulldown_cmark::{
    Alignment,
    CodeBlockKind,
    Event,
    HeadingLevel,
    Options,
    Parser,
    Tag,
    TagEnd,
};
use serde::{Deserialize, Serialize};

/// How a table column's cells line up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// A run of text with one style.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    /// Where the text links to.
    pub link: Option<String>,
}

impl Span {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }

    fn same_style(&self, other: &Self) -> bool {
        (self.bold, self.italic, self.code, self.strike, &self.link)
            == (
                other.bold,
                other.italic,
                other.code,
                other.strike,
                &other.link,
            )
    }
}

/// Text in spans; adjacent spans of one style are merged.
pub type Inline = Vec<Span>;

/// The text of `inline`, without its styles.
pub fn plain(inline: &[Span]) -> String {
    inline.iter().map(|span| span.text.as_str()).collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Block {
    Paragraph(Inline),
    /// Level 1 to 6.
    Heading(u8, Inline),
    Code {
        lang: Option<String>,
        text: String,
    },
    /// Numbered from `start`, or bulleted when `None`.
    List {
        start: Option<u64>,
        items: Vec<Vec<Block>>,
    },
    Quote(Vec<Block>),
    Table {
        align: Vec<Align>,
        head: Vec<Inline>,
        rows: Vec<Vec<Inline>>,
    },
    Rule,
}

/// `text` as blocks, in order.
pub fn blocks(text: &str) -> Vec<Block> {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS;
    let mut events = Parser::new_ext(text, options).peekable();
    read_blocks(&mut events, None)
}

type Events<'a> = std::iter::Peekable<Parser<'a>>;

/// Blocks until `end` closes the container they are in, or the text
/// ends.
fn read_blocks(events: &mut Events, end: Option<TagEnd>) -> Vec<Block> {
    let mut blocks = Vec::new();
    while let Some(event) = events.peek() {
        if let (Event::End(closing), Some(end)) = (event, end)
            && *closing == end
        {
            events.next();
            break;
        }
        // Text straight in a container, as in a tight list's items, is
        // a paragraph of its own.
        if is_inline(event) {
            let inline = read_inline(events, None);
            if !inline.is_empty() {
                blocks.push(Block::Paragraph(inline));
            }
            continue;
        }
        let Some(event) = events.next() else { break };
        match event {
            Event::Start(Tag::Paragraph) => {
                let inline = read_inline(events, Some(TagEnd::Paragraph));
                blocks.push(Block::Paragraph(inline));
            }
            Event::Start(Tag::Heading { level, .. }) => {
                let inline = read_inline(events, Some(TagEnd::Heading(level)));
                blocks.push(Block::Heading(heading(level), inline));
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(lang) if !lang.is_empty() => {
                        Some(lang.to_string())
                    }
                    _ => None,
                };
                let mut text = String::new();
                for event in events.by_ref() {
                    match event {
                        Event::Text(chunk) => text.push_str(&chunk),
                        Event::End(TagEnd::CodeBlock) => break,
                        _ => {}
                    }
                }
                // The fence's closing new line is not part of the code.
                if text.ends_with('\n') {
                    text.pop();
                }
                blocks.push(Block::Code { lang, text });
            }
            Event::Start(Tag::List(start)) => {
                let mut items = Vec::new();
                while let Some(event) = events.next() {
                    match event {
                        Event::Start(Tag::Item) => {
                            items.push(read_blocks(events, Some(TagEnd::Item)))
                        }
                        Event::End(TagEnd::List(_)) => break,
                        _ => {}
                    }
                }
                blocks.push(Block::List { start, items });
            }
            Event::Start(Tag::BlockQuote(kind)) => {
                let quoted =
                    read_blocks(events, Some(TagEnd::BlockQuote(kind)));
                blocks.push(Block::Quote(quoted));
            }
            Event::Start(Tag::Table(alignment)) => {
                blocks.push(read_table(events, &alignment));
            }
            Event::Start(Tag::HtmlBlock) => {
                let mut html = String::new();
                for event in events.by_ref() {
                    match event {
                        Event::Html(chunk) | Event::Text(chunk) => {
                            html.push_str(&chunk)
                        }
                        Event::End(TagEnd::HtmlBlock) => break,
                        _ => {}
                    }
                }
                blocks
                    .push(Block::Paragraph(vec![Span::plain(html.trim_end())]));
            }
            Event::Rule => blocks.push(Block::Rule),
            // A container these options should not start: read what is
            // in it.
            Event::Start(tag) => {
                let end = tag.to_end();
                blocks.extend(read_blocks(events, Some(end)));
            }
            _ => {}
        }
    }
    blocks
}

fn is_inline(event: &Event) -> bool {
    match event {
        Event::Text(_)
        | Event::Code(_)
        | Event::InlineHtml(_)
        | Event::InlineMath(_)
        | Event::DisplayMath(_)
        | Event::FootnoteReference(_)
        | Event::SoftBreak
        | Event::HardBreak
        | Event::TaskListMarker(_) => true,
        Event::Start(tag) => matches!(
            tag,
            Tag::Emphasis
                | Tag::Strong
                | Tag::Strikethrough
                | Tag::Superscript
                | Tag::Subscript
                | Tag::Link { .. }
                | Tag::Image { .. }
        ),
        _ => false,
    }
}

fn is_inline_end(event: &Event) -> bool {
    matches!(
        event,
        Event::End(
            TagEnd::Emphasis
                | TagEnd::Strong
                | TagEnd::Strikethrough
                | TagEnd::Superscript
                | TagEnd::Subscript
                | TagEnd::Link
                | TagEnd::Image
        )
    )
}

fn heading(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Spans until `end`, or, with no `end`, until the next event that is
/// not inline.
fn read_inline(events: &mut Events, end: Option<TagEnd>) -> Inline {
    let mut inline: Inline = Vec::new();
    let mut links: Vec<String> = Vec::new();
    let (mut bold, mut italic, mut strike) = (0u32, 0u32, 0u32);
    while let Some(event) = events.peek() {
        match (end, event) {
            (Some(end), Event::End(closing)) if *closing == end => {
                events.next();
                break;
            }
            (None, event) if !is_inline(event) && !is_inline_end(event) => {
                break;
            }
            _ => {}
        }
        let Some(event) = events.next() else { break };
        let style = Span {
            text: String::new(),
            bold: bold > 0,
            italic: italic > 0,
            code: false,
            strike: strike > 0,
            link: links.last().cloned(),
        };
        let text = match event {
            Event::Text(text)
            | Event::InlineHtml(text)
            | Event::Html(text)
            | Event::InlineMath(text)
            | Event::DisplayMath(text) => text.to_string(),
            Event::Code(text) => {
                push(
                    &mut inline,
                    Span {
                        text: text.to_string(),
                        code: true,
                        ..style
                    },
                );
                continue;
            }
            Event::FootnoteReference(name) => format!("[^{name}]"),
            Event::SoftBreak => " ".to_owned(),
            Event::HardBreak => "\n".to_owned(),
            Event::TaskListMarker(done) => {
                if done { "☑ " } else { "☐ " }.to_owned()
            }
            Event::Start(Tag::Strong) => {
                bold += 1;
                continue;
            }
            Event::End(TagEnd::Strong) => {
                bold = bold.saturating_sub(1);
                continue;
            }
            Event::Start(Tag::Emphasis) => {
                italic += 1;
                continue;
            }
            Event::End(TagEnd::Emphasis) => {
                italic = italic.saturating_sub(1);
                continue;
            }
            Event::Start(Tag::Strikethrough) => {
                strike += 1;
                continue;
            }
            Event::End(TagEnd::Strikethrough) => {
                strike = strike.saturating_sub(1);
                continue;
            }
            // An image reads as its alt text, linking to it.
            Event::Start(
                Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. },
            ) => {
                links.push(dest_url.to_string());
                continue;
            }
            Event::End(TagEnd::Link | TagEnd::Image) => {
                links.pop();
                continue;
            }
            _ => continue,
        };
        push(&mut inline, Span { text, ..style });
    }
    inline
}

/// Adds `span` to `inline`, merged into the last span when they share a
/// style.
fn push(inline: &mut Inline, span: Span) {
    if span.text.is_empty() {
        return;
    }
    match inline.last_mut() {
        Some(last) if last.same_style(&span) => last.text.push_str(&span.text),
        _ => inline.push(span),
    }
}

fn read_table(events: &mut Events, alignment: &[Alignment]) -> Block {
    let align = alignment
        .iter()
        .map(|align| match align {
            Alignment::Center => Align::Center,
            Alignment::Right => Align::Right,
            Alignment::Left | Alignment::None => Align::Left,
        })
        .collect();
    let mut head = Vec::new();
    let mut rows: Vec<Vec<Inline>> = Vec::new();
    let mut in_head = false;
    while let Some(event) = events.next() {
        match event {
            Event::Start(Tag::TableHead) => in_head = true,
            Event::End(TagEnd::TableHead) => in_head = false,
            Event::Start(Tag::TableRow) => rows.push(Vec::new()),
            Event::Start(Tag::TableCell) => {
                let cell = read_inline(events, Some(TagEnd::TableCell));
                match rows.last_mut() {
                    Some(row) if !in_head => row.push(cell),
                    _ => head.push(cell),
                }
            }
            Event::End(TagEnd::Table) => break,
            _ => {}
        }
    }
    Block::Table { align, head, rows }
}
