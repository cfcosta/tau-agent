//! Truncation (`tau_tools::truncate`), against the rules in
//! `docs/reference/tools.md` and pi's `truncate.ts`.

use hegel::{TestCase, generators as gs};
use tau_tools::truncate::{
    GREP_MAX_LINE,
    Limit,
    MAX_BYTES,
    MAX_LINES,
    format_size,
    truncate_head,
    truncate_line,
    truncate_tail,
};

/// Text of up to a few dozen short lines, with multi-byte characters
/// and sometimes a trailing newline, so both limits and every UTF-8
/// boundary get hit with small limits.
#[hegel::composite]
fn text(tc: TestCase) -> String {
    let line = gs::text().alphabet("ab \té€😀").max_size(30);
    let lines: Vec<String> = tc.draw(gs::vecs(line).max_size(30));
    let mut text = lines.join("\n");
    if tc.draw(gs::booleans()) {
        text.push('\n');
    }
    text
}

/// The lines of `text` as the tools count them.
fn lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<&str> = text.split('\n').collect();
    if text.ends_with('\n') {
        lines.pop();
    }
    lines
}

fn limits(tc: &TestCase) -> (usize, usize) {
    (
        tc.draw(gs::integers::<usize>().min_value(1).max_value(12)),
        tc.draw(gs::integers::<usize>().min_value(1).max_value(200)),
    )
}

/// `truncate_head` keeps the longest prefix of whole lines within both
/// limits: its output is the first `k` lines joined, within the limits,
/// and one more line would break one of them. When the first line
/// alone is over the byte limit it keeps nothing and says so.
#[hegel::test(test_cases = 500)]
fn head_keeps_the_longest_prefix_of_whole_lines(tc: TestCase) {
    let text = tc.draw(text());
    let (max_lines, max_bytes) = limits(&tc);
    let result = truncate_head(&text, max_lines, max_bytes);
    let all = lines(&text);
    if !result.truncated() {
        assert!(all.len() <= max_lines && text.len() <= max_bytes);
        assert_eq!(result.content, text);
        return;
    }
    let k = result.output_lines;
    assert_eq!(result.content, all[..k].join("\n"));
    assert!(k <= max_lines);
    assert!(result.content.len() <= max_bytes);
    assert_eq!(result.output_bytes, result.content.len());
    assert_eq!(result.total_lines, all.len());
    assert_eq!(result.total_bytes, text.len());
    if result.first_line_exceeds_limit {
        assert_eq!(k, 0);
        assert!(all[0].len() > max_bytes);
        assert_eq!(result.by, Some(Limit::Bytes));
    } else if k < all.len() {
        let longer = all[..=k].join("\n");
        assert!(k + 1 > max_lines || longer.len() > max_bytes);
        let by = if k >= max_lines {
            Limit::Lines
        } else {
            Limit::Bytes
        };
        assert_eq!(result.by, Some(by));
    }
}

/// `truncate_tail` keeps the longest suffix of whole lines within both
/// limits; when even the last line is over the byte limit, it keeps the
/// longest end of that line that fits, which is valid UTF-8 because it
/// is a `String`, and starts on a character boundary of the original.
#[hegel::test(test_cases = 500)]
fn tail_keeps_the_longest_suffix(tc: TestCase) {
    let text = tc.draw(text());
    let (max_lines, max_bytes) = limits(&tc);
    let result = truncate_tail(&text, max_lines, max_bytes);
    let all = lines(&text);
    if !result.truncated() {
        assert!(all.len() <= max_lines && text.len() <= max_bytes);
        assert_eq!(result.content, text);
        return;
    }
    assert!(result.output_lines <= max_lines);
    assert!(result.content.len() <= max_bytes);
    assert_eq!(result.output_bytes, result.content.len());
    if result.last_line_partial {
        let last = all.last().unwrap();
        assert!(last.len() > max_bytes);
        assert!(last.ends_with(&result.content));
        // The longest end that fits: one more character would not.
        let start = last.len() - result.content.len();
        let before = last[..start].chars().next_back();
        assert!(before.is_none_or(|c| result.content.len() + c.len_utf8() > max_bytes));
        // pi's rule: reaching the line limit reports `Lines`, even when
        // the only kept line is a partial one.
        let by = if max_lines == 1 {
            Limit::Lines
        } else {
            Limit::Bytes
        };
        assert_eq!(result.by, Some(by));
    } else {
        let k = result.output_lines;
        assert_eq!(result.content, all[all.len() - k..].join("\n"));
        if k < all.len() {
            let longer = all[all.len() - k - 1..].join("\n");
            assert!(k + 1 > max_lines || longer.len() > max_bytes);
        }
    }
}

/// Truncation is idempotent: truncating the output again cuts nothing.
#[hegel::test(test_cases = 500)]
fn truncation_is_idempotent(tc: TestCase) {
    let text = tc.draw(text());
    let (max_lines, max_bytes) = limits(&tc);
    for truncate in [truncate_head, truncate_tail] {
        let once = truncate(&text, max_lines, max_bytes).content;
        let twice = truncate(&once, max_lines, max_bytes);
        assert!(!twice.truncated());
        assert_eq!(twice.content, once);
    }
}

/// pi's edge: content whose only excess is its trailing newline is
/// reported as cut by lines, as pi's `truncatedBy` starts at "lines".
#[test]
fn a_trailing_newline_over_the_byte_limit() {
    let result = truncate_head("a\n", 10, 1);
    assert_eq!(result.content, "a");
    assert_eq!(result.by, Some(Limit::Lines));
}

/// A grep line is cut at 500 characters (not bytes) and marked.
#[test]
fn lines_are_cut_by_characters() {
    let short = "é".repeat(GREP_MAX_LINE);
    assert_eq!(truncate_line(&short, GREP_MAX_LINE), (short.clone(), false));
    let long = format!("{short}xyz");
    assert_eq!(
        truncate_line(&long, GREP_MAX_LINE),
        (format!("{short}... [truncated]"), true)
    );
}

/// Sizes read as pi prints them.
#[test]
fn sizes_read_as_pi_prints_them() {
    assert_eq!(format_size(0), "0B");
    assert_eq!(format_size(1023), "1023B");
    assert_eq!(format_size(1024), "1.0KB");
    assert_eq!(format_size(1536), "1.5KB");
    assert_eq!(format_size(1024 * 1024 - 1), "1024.0KB");
    assert_eq!(format_size(1024 * 1024), "1.0MB");
    assert_eq!(format_size(5 * 1024 * 1024 / 2), "2.5MB");
}

/// The limits are the spec's (`tools.md`, "The shared limits").
#[test]
fn the_limits_are_the_spec() {
    assert_eq!(MAX_LINES, 2000);
    assert_eq!(MAX_BYTES, 51_200);
    assert_eq!(GREP_MAX_LINE, 500);
}
