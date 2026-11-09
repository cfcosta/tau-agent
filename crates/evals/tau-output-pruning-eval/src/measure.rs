//! The eval's measuring stick: how the plugin splits an output into
//! lines and chunks, renders what stays, and estimates tokens.
//!
//! These are ports of the Rust tau-fast-compaction had before it became
//! a Luau plugin (ADR 0050); its Luau (`luau/lib/output.luau`,
//! `luau/lib/estimate.luau`) does the same. The eval measures with its
//! own copy, so a trial's layout and token counts do not depend on the
//! plugin under test. The tests check that the two agree.

use std::ops::Range;

/// Lines longer than this, in characters, are split first.
pub const MAX_LINE_CHARS: usize = 2_000;

/// Chunks per output, at most: groups of lines merge to stay under.
pub const MAX_CHUNKS: usize = 200;

/// A line of the output, or a piece of one longer than
/// [`MAX_LINE_CHARS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Line<'a> {
    pub text: &'a str,
    /// Whether its line ends here: false on every piece of a long line
    /// but the last.
    pub ends: bool,
}

/// The output's lines, split at each `\n`, with lines over
/// [`MAX_LINE_CHARS`] characters split into pieces of that many.
pub fn lines(output: &str) -> Vec<Line<'_>> {
    let mut lines = Vec::new();
    for line in output.split('\n') {
        let mut rest = line;
        loop {
            let cut = rest
                .char_indices()
                .nth(MAX_LINE_CHARS)
                .map_or(rest.len(), |(at, _)| at);
            let (piece, after) = rest.split_at(cut);
            lines.push(Line {
                text: piece,
                ends: after.is_empty(),
            });
            if after.is_empty() {
                break;
            }
            rest = after;
        }
    }
    lines
}

/// The chunks of `count` lines: runs of `chunk_lines` lines, merged so
/// there are at most [`MAX_CHUNKS`].
pub fn chunks(count: usize, chunk_lines: usize) -> Vec<Range<usize>> {
    let per = chunk_lines.max(1).max(count.div_ceil(MAX_CHUNKS));
    (0..count)
        .step_by(per)
        .map(|start| start..(start + per).min(count))
        .collect()
}

/// The text of `range` of `lines`, as it is in the output.
pub fn text_of(lines: &[Line<'_>], range: Range<usize>) -> String {
    let mut text = String::new();
    for (index, line) in lines[range.clone()].iter().enumerate() {
        text.push_str(line.text);
        if line.ends && range.start + index + 1 < range.end {
            text.push('\n');
        }
    }
    text
}

/// The id Jev's questions give chunk `index`.
pub fn chunk_id(index: usize) -> String {
    format!("c{}", index + 1)
}

/// What a pruned output reads as: the header, the kept chunks' lines,
/// each run of dropped lines as one marker, and a footer naming
/// `archive`.
pub fn render(
    lines: &[Line<'_>],
    chunks: &[Range<usize>],
    keep: &[bool],
    archive: &str,
) -> String {
    let mut body = String::new();
    // What came last: a line (and whether its line ended), or a marker.
    let mut last: Option<Option<bool>> = None;
    let mut omitted = 0usize;
    let mut push = |body: &mut String, text: &str, ends: Option<bool>| {
        match last {
            Some(Some(false)) if ends.is_some() => {}
            Some(_) => body.push('\n'),
            None => {}
        }
        body.push_str(text);
        last = Some(ends);
    };
    for (range, kept) in chunks.iter().zip(keep) {
        for line in &lines[range.clone()] {
            if !kept {
                omitted += 1;
                continue;
            }
            if omitted > 0 {
                push(&mut body, &format!("[{omitted} lines omitted]"), None);
                omitted = 0;
            }
            push(&mut body, line.text, Some(line.ends));
        }
    }
    if omitted > 0 {
        push(&mut body, &format!("[{omitted} lines omitted]"), None);
    }
    format!(
        "{}\n{body}\n\n[full output: {archive} (read or grep it if needed)]",
        tau_luau_plugins::names::PRUNED_HEADER
    )
}

/// Whether `c` is a space as JavaScript's `\s` sees it.
fn js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n'
            | '\u{b}'
            | '\u{c}'
            | '\r'
            | ' '
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

/// A token count without a tokenizer, jev-pruner's `estimateTokens`, in
/// tenths and rounded up once: a word of ASCII letters is one token per
/// six letters, rounded up; a digit half a token; any other character
/// that is not a space nine tenths of a token per UTF-16 unit.
pub fn estimate_tokens(text: &str) -> usize {
    let mut tenths = 0usize;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_alphabetic() {
            let mut len = 1usize;
            while chars.next_if(char::is_ascii_alphabetic).is_some() {
                len += 1;
            }
            tenths += 10 * (1 + (len - 1) / 6);
        } else if c.is_ascii_digit() {
            tenths += 5;
        } else if !js_space(c) {
            tenths += 9 * c.len_utf16();
        }
    }
    tenths.div_ceil(10)
}
