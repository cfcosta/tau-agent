//! The side request's reply, read strictly: `learn: none`, or
//!
//! ```text
//! learn: <one line, at most 240 characters, ending in a period>
//! tag: You should know | Heads up
//! explain:
//! **title**
//! - 3 to 5 bullets
//! ```
//!
//! The `explain:` part may be left out. Anything else is an error, and
//! the caller drops the reply: a note the model got slightly wrong is
//! worse than none.

use crate::record::{Explain, Tag};

/// The longest line a note may have.
pub const MAX_LINE: usize = 240;

/// The fewest and most bullets an explanation has.
pub const BULLETS: std::ops::RangeInclusive<usize> = 3..=5;

/// What the model said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// Nothing the person would pay to have missed.
    Nothing,
    Note {
        tag: Tag,
        line: String,
        explain: Option<Explain>,
    },
}

/// Why a reply could not be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Unreadable {
    #[error("the reply was empty")]
    Empty,
    #[error("the reply was quoted or fenced")]
    Quoted,
    #[error("the reply did not start with `learn:`")]
    NoLearn,
    #[error("the note's line was empty")]
    NoLine,
    #[error("the note's line has {0} characters, over {MAX_LINE}")]
    TooLong(usize),
    #[error("the note's line did not end with a period")]
    NoPeriod,
    #[error("the note's line spans several lines")]
    ManyLines,
    #[error("the reply had no `tag:` line")]
    NoTag,
    #[error("the tag was `{0}`, not `You should know` or `Heads up`")]
    UnknownTag(String),
    #[error("the explanation was not a `**title**` and 3 to 5 bullets")]
    BadExplain,
}

/// Reads `text` as the model's reply.
pub fn parse(text: &str) -> Result<Reply, Unreadable> {
    let text = text.trim();
    if text.is_empty() {
        return Err(Unreadable::Empty);
    }
    if text.starts_with("```")
        || text.starts_with('>')
        || text.starts_with('"')
        || text.starts_with('\'')
        || text.starts_with('`')
    {
        return Err(Unreadable::Quoted);
    }
    let mut lines = text.lines();
    let first = lines.next().unwrap_or_default();
    let line = field(first, "learn").ok_or(Unreadable::NoLearn)?;
    if line.trim_end_matches('.').eq_ignore_ascii_case("none") {
        // The whole reply, not a note that starts with the word.
        return if lines.all(|rest| rest.trim().is_empty()) {
            Ok(Reply::Nothing)
        } else {
            Err(Unreadable::ManyLines)
        };
    }
    if line.is_empty() {
        return Err(Unreadable::NoLine);
    }
    let length = line.chars().count();
    if length > MAX_LINE {
        return Err(Unreadable::TooLong(length));
    }
    if !line.ends_with('.') {
        return Err(Unreadable::NoPeriod);
    }
    let tag = lines
        .next()
        .and_then(|line| field(line, "tag"))
        .ok_or(Unreadable::NoTag)?;
    let tag = Tag::ALL
        .into_iter()
        .find(|known| known.label() == tag)
        .ok_or_else(|| Unreadable::UnknownTag(tag.to_owned()))?;
    let rest: Vec<&str> = lines.collect();
    let explain = explanation(&rest)?;
    Ok(Reply::Note {
        tag,
        line: line.to_owned(),
        explain,
    })
}

/// The value of the line `key: value`, trimmed.
fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let (name, value) = line.split_once(':')?;
    name.trim().eq_ignore_ascii_case(key).then(|| value.trim())
}

/// The lines after `tag:`: nothing, or `explain:`, a bold title and
/// bullets.
fn explanation(lines: &[&str]) -> Result<Option<Explain>, Unreadable> {
    let mut lines = lines
        .iter()
        .map(|line| line.trim())
        .skip_while(|l| l.is_empty());
    let Some(head) = lines.next() else {
        return Ok(None);
    };
    if !head.eq_ignore_ascii_case("explain:") {
        return Err(Unreadable::BadExplain);
    }
    let title = lines
        .by_ref()
        .find(|line| !line.is_empty())
        .and_then(|line| line.strip_prefix("**")?.strip_suffix("**"))
        .map(str::trim)
        .filter(|title| !title.is_empty() && !title.contains("**"))
        .ok_or(Unreadable::BadExplain)?;
    let mut bullets = Vec::new();
    for line in lines.filter(|line| !line.is_empty()) {
        let bullet = line
            .strip_prefix("- ")
            .or_else(|| line.strip_prefix("* "))
            .map(str::trim)
            .filter(|bullet| !bullet.is_empty())
            .ok_or(Unreadable::BadExplain)?;
        bullets.push(bullet.to_owned());
    }
    if !BULLETS.contains(&bullets.len()) {
        return Err(Unreadable::BadExplain);
    }
    Ok(Some(Explain {
        title: title.to_owned(),
        bullets,
    }))
}

/// A reply written the way [`parse`] reads it.
pub fn write(tag: Tag, line: &str, explain: Option<&Explain>) -> String {
    let mut text = format!("learn: {line}\ntag: {}", tag.label());
    if let Some(explain) = explain {
        text.push_str(&format!("\nexplain:\n**{}**", explain.title));
        for bullet in &explain.bullets {
            text.push_str(&format!("\n- {bullet}"));
        }
    }
    text
}
