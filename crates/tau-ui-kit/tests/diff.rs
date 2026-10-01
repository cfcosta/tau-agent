//! Reading unified diffs back into lines.

use hegel::generators as gs;
use similar::TextDiff;
use tau_ui_kit::diff::{DiffKind, DiffLine, parse, stat};

/// Lines that look like diff headers once a sign is put in front.
const LINES: [&str; 8] = ["-- a", "--", "++b", "+", "@@ x", "a", "", "\\ c"];

fn text(lines: &[usize]) -> String {
    lines.iter().map(|&i| format!("{}\n", LINES[i])).collect()
}

fn side(lines: &[DiffLine], drop: DiffKind) -> String {
    lines
        .iter()
        .filter(|line| line.kind != drop)
        .map(|line| format!("{}\n", line.text))
        .collect()
}

/// With the whole file as context, the old side is every line that was
/// not added and the new side every line that was not removed, however
/// much a line looks like a header.
/// (With no change there is no diff at all.)
#[hegel::test(test_cases = 200)]
fn a_whole_file_diff_gives_back_both_sides(tc: hegel::TestCase) {
    let line = || gs::integers::<usize>().max_value(LINES.len() - 1);
    let old: Vec<usize> = tc.draw(gs::vecs(line()).max_size(8));
    let new: Vec<usize> = tc.draw(gs::vecs(line()).max_size(8));
    let (old, new) = (text(&old), text(&new));
    let diff = TextDiff::from_lines(&old, &new)
        .unified_diff()
        .context_radius(usize::MAX / 2)
        .header("a", "b")
        .to_string();
    let lines = parse(&diff);
    if old == new {
        // Nothing changed: no hunk, no lines.
        assert!(lines.is_empty(), "{diff}");
        return;
    }
    assert_eq!(side(&lines, DiffKind::Added), old, "{diff}");
    assert_eq!(side(&lines, DiffKind::Removed), new, "{diff}");
}

#[test]
fn a_removed_comment_is_a_line_not_a_header() {
    let diff = "--- a\n+++ b\n@@ -1,2 +1,2 @@\n--- note\n+++x\n a\n";
    let lines = parse(diff);
    assert_eq!(stat(&lines), "+1 −1");
    assert_eq!(lines[0].text, "-- note");
    assert_eq!(lines[1].text, "++x");
}
