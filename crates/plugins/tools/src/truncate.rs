//! Output truncation shared by the tools (`docs/reference/tools.md`),
//! ported from pi's `truncate.ts`.
//!
//! Two limits, lines and bytes; whichever is hit first wins. Head
//! truncation keeps whole lines from the start. Tail truncation keeps
//! whole lines from the end, except that a last line longer than the
//! byte limit keeps its end, cut at a UTF-8 boundary.
//!
//! Lines split on `\n`; a trailing `\n` does not start another line.

/// The most lines a tool result shows.
pub const MAX_LINES: usize = 2000;
/// The most bytes a tool result shows: 50 KiB.
pub const MAX_BYTES: usize = 50 * 1024;
/// The most characters of one `grep` match line.
pub const GREP_MAX_LINE: usize = 500;

/// Which limit cut the content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    Lines,
    Bytes,
}

/// The result of a truncation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Truncation {
    pub content: String,
    /// `None` when nothing was cut.
    pub by: Option<Limit>,
    pub total_lines: usize,
    pub total_bytes: usize,
    /// Lines in `content`, a partial one included.
    pub output_lines: usize,
    pub output_bytes: usize,
    /// Tail only: the kept last line is the end of a longer line.
    pub last_line_partial: bool,
    /// Head only: the first line alone is over the byte limit, so
    /// nothing is kept.
    pub first_line_exceeds_limit: bool,
}

impl Truncation {
    pub fn truncated(&self) -> bool {
        self.by.is_some()
    }

    fn whole(content: &str, lines: usize) -> Self {
        Self {
            content: content.to_owned(),
            by: None,
            total_lines: lines,
            total_bytes: content.len(),
            output_lines: lines,
            output_bytes: content.len(),
            last_line_partial: false,
            first_line_exceeds_limit: false,
        }
    }
}

/// The lines of `content`, as pi counts them: none for an empty string,
/// and no empty last line after a trailing `\n`.
fn lines(content: &str) -> Vec<&str> {
    if content.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<&str> = content.split('\n').collect();
    if content.ends_with('\n') {
        lines.pop();
    }
    lines
}

/// Keeps whole lines from the start, within `max_lines` and `max_bytes`.
/// If the first line alone is over `max_bytes`, keeps nothing.
pub fn truncate_head(
    content: &str,
    max_lines: usize,
    max_bytes: usize,
) -> Truncation {
    let all = lines(content);
    if all.len() <= max_lines && content.len() <= max_bytes {
        return Truncation::whole(content, all.len());
    }
    let mut result = Truncation {
        content: String::new(),
        by: Some(Limit::Lines),
        total_lines: all.len(),
        total_bytes: content.len(),
        output_lines: 0,
        output_bytes: 0,
        last_line_partial: false,
        first_line_exceeds_limit: false,
    };
    if all[0].len() > max_bytes {
        result.by = Some(Limit::Bytes);
        result.first_line_exceeds_limit = true;
        return result;
    }
    let mut kept = 0;
    let mut bytes = 0;
    for (i, line) in all.iter().enumerate().take(max_lines) {
        let cost = line.len() + usize::from(i > 0);
        if bytes + cost > max_bytes {
            result.by = Some(Limit::Bytes);
            break;
        }
        bytes += cost;
        kept += 1;
    }
    if kept >= max_lines {
        result.by = Some(Limit::Lines);
    }
    result.content = all[..kept].join("\n");
    result.output_lines = kept;
    result.output_bytes = result.content.len();
    result
}

/// Keeps whole lines from the end, within `max_lines` and `max_bytes`.
/// If the last line alone is over `max_bytes`, keeps its end, starting
/// at a character boundary.
pub fn truncate_tail(
    content: &str,
    max_lines: usize,
    max_bytes: usize,
) -> Truncation {
    let all = lines(content);
    if all.len() <= max_lines && content.len() <= max_bytes {
        return Truncation::whole(content, all.len());
    }
    let mut result = Truncation {
        content: String::new(),
        by: Some(Limit::Lines),
        total_lines: all.len(),
        total_bytes: content.len(),
        output_lines: 0,
        output_bytes: 0,
        last_line_partial: false,
        first_line_exceeds_limit: false,
    };
    let mut kept: Vec<&str> = Vec::new();
    let mut bytes = 0;
    for line in all.iter().rev() {
        if kept.len() >= max_lines {
            break;
        }
        let cost = line.len() + usize::from(!kept.is_empty());
        if bytes + cost > max_bytes {
            result.by = Some(Limit::Bytes);
            if kept.is_empty() {
                let tail = end_within(line, max_bytes);
                kept.push(tail);
                bytes = tail.len();
                result.last_line_partial = true;
            }
            break;
        }
        kept.push(line);
        bytes += cost;
    }
    if kept.len() >= max_lines && bytes <= max_bytes {
        result.by = Some(Limit::Lines);
    }
    kept.reverse();
    result.content = kept.join("\n");
    result.output_lines = kept.len();
    result.output_bytes = result.content.len();
    result
}

/// The longest end of `line` within `max_bytes`, starting at a character
/// boundary.
fn end_within(line: &str, max_bytes: usize) -> &str {
    let mut start = line.len().saturating_sub(max_bytes);
    while !line.is_char_boundary(start) {
        start += 1;
    }
    &line[start..]
}

/// Cuts a line to `max_chars` characters, marking the cut with
/// `... [truncated]`. Returns the line and whether it was cut. (pi
/// counts UTF-16 units; this counts characters.)
pub fn truncate_line(line: &str, max_chars: usize) -> (String, bool) {
    match line.char_indices().nth(max_chars) {
        None => (line.to_owned(), false),
        Some((end, _)) => (format!("{}... [truncated]", &line[..end]), true),
    }
}

/// A byte count for people: `512B`, `1.5KB`, `2.0MB`.
pub fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}
