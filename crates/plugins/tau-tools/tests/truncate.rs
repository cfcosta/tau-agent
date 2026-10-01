//! Truncation (`tau_tools::truncate`), against the rules in
//! `docs/reference/tools.md` and pi's `truncate.ts`.

use hegel::{TestCase, generators as gs};
use tau_tools::truncate::{
    CUT,
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
fn text(tc: &TestCase) -> String {
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
    // Sometimes one byte short of the whole text, which is where only a
    // trailing newline is over the limit.
    let max_bytes = if tc.draw(gs::weighted_booleans(0.2)) {
        text.len().saturating_sub(1).max(1)
    } else {
        max_bytes
    };
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
    } else {
        // Every line kept: only the trailing newline was over the byte
        // limit, which pi reports as a cut by lines.
        tc.event("only the trailing newline cut");
        assert_eq!(result.by, Some(Limit::Lines));
    }
}

/// `truncate_tail` keeps the longest suffix of whole lines within both
/// limits; when even the last line is over the byte limit, it keeps the
/// cut marker and the longest end of that line that fits after it (with
/// no marker when the limit cannot hold one), which is valid UTF-8
/// because it is a `String`, and starts on a character boundary of the
/// original.
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
    assert_eq!(result.total_lines, all.len());
    assert_eq!(result.total_bytes, text.len());
    if result.last_line_partial {
        tc.event("partial last line");
        let last = all.last().unwrap();
        assert!(last.len() > max_bytes);
        let marker = if max_bytes >= CUT.len() { CUT } else { "" };
        let end = result
            .content
            .strip_prefix(marker)
            .expect("a cut line starts with the marker");
        assert!(last.ends_with(end));
        // The longest end that fits: one more character would not.
        let start = last.len() - end.len();
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
        // Reaching the line limit, or keeping every line (only the
        // trailing newline was over), reports `Lines`; otherwise a line
        // was left out for its bytes.
        let by = if k >= max_lines || k == all.len() {
            Limit::Lines
        } else {
            Limit::Bytes
        };
        assert_eq!(result.by, Some(by));
    }
}

/// A last line over the byte limit keeps its end after `…`, the marker
/// counted in the limit; a limit too small for the marker keeps the end
/// unmarked.
#[test]
fn a_cut_last_line_starts_with_the_marker() {
    let line = "abcdefghij\n";
    assert_eq!(truncate_tail(line, 10, 6).content, "…hij");
    assert_eq!(truncate_tail(line, 10, 3).content, "…");
    assert_eq!(truncate_tail(line, 10, 2).content, "ij");
    let result = truncate_tail("abc\nabcdefghij", 10, 6);
    assert_eq!(result.content, "…hij");
    assert!(result.last_line_partial);
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

/// `truncate_line` keeps a line of at most `max_chars` characters as it
/// is, and cuts a longer one to its first `max_chars` characters (never
/// inside a character) followed by the marker.
#[hegel::test(test_cases = 500)]
fn truncate_line_keeps_the_first_characters(tc: TestCase) {
    let line = tc.draw(gs::text().alphabet("aé€😀 ").max_size(20));
    let max_chars = tc.draw(gs::integers::<usize>().max_value(25));
    let (out, cut) = truncate_line(&line, max_chars);
    let count = line.chars().count();
    assert_eq!(cut, count > max_chars);
    if cut {
        tc.event("cut");
        let kept: String = line.chars().take(max_chars).collect();
        assert_eq!(out, format!("{kept}... [truncated]"));
    } else {
        assert_eq!(out, line);
    }
}

/// `format_size` prints bytes below 1 KiB as an integer count, then
/// KiB below 1 MiB, then MiB, each to one decimal and within half a
/// tenth of the exact value.
#[hegel::test(test_cases = 500)]
fn format_size_is_the_size_to_one_decimal(tc: TestCase) {
    let bytes = tc.draw(hegel::one_of!(
        gs::sampled_from(vec![
            0usize,
            1023,
            1024,
            1024 * 1024 - 1,
            1024 * 1024
        ]),
        gs::integers::<usize>().max_value(2048),
        gs::integers::<usize>().max_value(8 * 1024 * 1024),
    ));
    let text = format_size(bytes);
    let (number, unit) = if let Some(n) = text.strip_suffix("MB") {
        (n, 1024.0 * 1024.0)
    } else if let Some(n) = text.strip_suffix("KB") {
        (n, 1024.0)
    } else {
        let n = text.strip_suffix('B').expect("a unit");
        assert_eq!(n, bytes.to_string());
        assert!(bytes < 1024);
        return;
    };
    let expected_unit = if bytes < 1024 * 1024 {
        1024.0
    } else {
        1024.0 * 1024.0
    };
    assert!(bytes >= 1024);
    assert_eq!(unit, expected_unit, "{text}");
    let (_, decimals) = number.split_once('.').expect("one decimal");
    assert_eq!(decimals.len(), 1, "{text}");
    let value: f64 = number.parse().unwrap();
    assert!(
        (value - bytes as f64 / unit).abs() <= 0.05 + 1e-9,
        "{bytes} printed as {text}"
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
