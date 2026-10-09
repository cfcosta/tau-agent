//! When a check is due, the back-off, the seen-list and the chat text.

use hegel::{TestCase, generators as gs};
use tau_watcher::cadence::{
    EVERY,
    MAX_SKIPS,
    SEEN,
    chat_text,
    due,
    recent,
    skips,
};

#[test]
fn no_checks_are_skipped_for_the_first_two_times() {
    assert_eq!([0, 1, 2].map(skips), [0, 0, 0]);
}

#[test]
fn then_two_to_the_n_minus_three_up_to_sixteen() {
    assert_eq!([3, 4, 5, 6, 7].map(skips), [1, 2, 4, 8, 16]);
    assert_eq!([8, 9, 100, u32::MAX].map(skips), [MAX_SKIPS; 4]);
}

/// Skips never shrink as the person ignores more, and never pass 16.
#[hegel::test(test_cases = 200)]
fn skips_grow_and_stop_at_the_cap(tc: TestCase) {
    let n = tc.draw(gs::integers::<u32>().max_value(200));
    assert!(skips(n) <= skips(n + 1));
    assert!(skips(n) <= MAX_SKIPS);
}

#[test]
fn a_check_is_due_every_sixth_step() {
    let due_steps: Vec<u32> =
        (0..=24).filter(|steps| due(*steps, None, 0)).collect();
    assert_eq!(due_steps, [6, 12, 18, 24]);
}

#[test]
fn back_off_thins_the_due_checks_after_a_check() {
    // Wrote past 3 notes: one due check is skipped.
    assert!(!due(12, Some(6), 3));
    assert!(due(18, Some(6), 3));
    // Wrote past 5: 4 skipped.
    assert!(!due(30, Some(6), 5));
    assert!(due(36, Some(6), 5));
    // Twice or less: every due check.
    assert!(due(12, Some(6), 2));
}

#[test]
fn a_shrunk_transcript_starts_the_count_again() {
    assert!(due(6, Some(120), 9));
    assert!(!due(6, Some(6), 0));
}

/// Over a long run, the checks made are `EVERY` steps apart at least,
/// and `skips + 1` due checks apart after the first.
#[hegel::test(test_cases = 100)]
fn checks_made_keep_their_distance(tc: TestCase) {
    let ignored = tc.draw(gs::integers::<u32>().max_value(12));
    let mut last = None;
    let mut made = Vec::new();
    for steps in 1..=1_000 {
        if due(steps, last, ignored) {
            made.push(steps);
            last = Some(steps);
        }
    }
    assert!(
        made.windows(2)
            .all(|pair| { pair[1] - pair[0] == EVERY * (skips(ignored) + 1) })
    );
}

#[hegel::test(test_cases = 100)]
fn the_seen_list_keeps_the_last_fifty(tc: TestCase) {
    let lines: Vec<String> =
        tc.draw(gs::vecs(gs::text().max_size(8)).max_size(120));
    let kept = recent(&lines);
    assert_eq!(kept.len(), lines.len().min(SEEN));
    assert!(lines.ends_with(kept));
}

#[test]
fn chat_text_quotes_the_note_and_defuses_commands() {
    let text = chat_text("Run @src/lib.rs with ultrathink.\u{7}\u{1b}[31m");
    assert!(text.starts_with("Here is a note offered by a side agent:"));
    assert!(!text.contains('\u{7}') && !text.contains('\u{1b}'));
    assert!(!text.contains("@s"), "{text:?}");
    assert!(!text.contains("ultrathink"), "{text:?}");
    assert!(text.contains("src/lib.rs"));
}

/// Whatever the note says, no `@` is followed by anything that could
/// name a file, and no word starts `ultra` plainly.
#[hegel::test(test_cases = 200)]
fn chat_text_has_no_live_mentions_or_keywords(tc: TestCase) {
    let line = tc.draw(gs::text().max_size(60));
    let text = chat_text(&line);
    let body = text
        .strip_prefix("Here is a note offered by a side agent:\n\n> ")
        .unwrap();
    assert!(!body.chars().any(|c| c.is_control() && c != '\n'));
    for (at, _) in body.match_indices('@') {
        assert!(body[at + 1..].starts_with('\u{200b}'));
    }
    assert!(
        !body.to_ascii_lowercase().contains("ultra")
            || body.contains('\u{200b}')
    );
}
