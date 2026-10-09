//! When to check, and how long to stay quiet: Claude Code's "You should
//! know" cadence (ADR 0032).

/// A check is due every this many steps.
pub const EVERY: u32 = 6;

/// How many note lines the prompt carries, so a topic does not return.
pub const SEEN: usize = 50;

/// A note stays a band above the composer until the person has written
/// past it this many times.
pub const BAND_LIMIT: u32 = 2;

/// The most due checks that are skipped in a row.
pub const MAX_SKIPS: u32 = 16;

/// How many due checks to skip after the person wrote past `typed_past`
/// notes unanswered: none for the first 2, then 2^(n-3), at most 16.
pub fn skips(typed_past: u32) -> u32 {
    match typed_past.checked_sub(3) {
        None => 0,
        Some(power) if power >= MAX_SKIPS.ilog2() => MAX_SKIPS,
        Some(power) => 1 << power,
    }
}

/// Whether the request at step `steps` (1 for the conversation's first)
/// gets a check. `last` is the step of the last check made, if any:
/// checks come every [`EVERY`] steps, and after the person wrote past
/// notes, fewer.
pub fn due(steps: u32, last: Option<u32>, typed_past: u32) -> bool {
    if steps == 0 || steps % EVERY != 0 {
        return false;
    }
    match last {
        Some(last) if last == steps => false,
        Some(last) if last < steps => {
            (steps - last) / EVERY > skips(typed_past)
        }
        // None yet, or a transcript that shrank (compaction): the count
        // starts again.
        _ => true,
    }
}

/// The last [`SEEN`] of `lines`, oldest first.
pub fn recent(lines: &[String]) -> &[String] {
    &lines[lines.len().saturating_sub(SEEN)..]
}

/// `line` as the composer quotes it back to the model, with the note
/// framed as offered by a side agent: control characters stripped, `@`
/// mentions and `ultra` keywords (which the composer reads as commands)
/// broken up with a zero-width space.
pub fn chat_text(line: &str) -> String {
    let mut quoted = String::new();
    let mut word = String::new();
    let flush = |word: &mut String, quoted: &mut String| {
        if word.to_ascii_lowercase().starts_with("ultra") {
            quoted.push_str(&word[..5]);
            quoted.push('\u{200b}');
            quoted.push_str(&word[5..]);
        } else {
            quoted.push_str(word);
        }
        word.clear();
    };
    for c in line.chars().filter(|c| !c.is_control() || *c == ' ') {
        if c.is_alphanumeric() {
            word.push(c);
            continue;
        }
        flush(&mut word, &mut quoted);
        quoted.push(c);
        if c == '@' {
            quoted.push('\u{200b}');
        }
    }
    flush(&mut word, &mut quoted);
    format!("Here is a note offered by a side agent:\n\n> {quoted}\n\n")
}
