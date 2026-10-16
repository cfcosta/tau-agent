//! When a check is due, the back-off, the seen-list and the chat text.

use hegel::{TestCase, generators as gs, generators::Generator as _};
use tau_watcher::cadence::{
    EVERY,
    MAX_SKIPS,
    SEEN,
    chat_text,
    due,
    recent,
    skips,
};

/// The skips the back-off makes after `typed_past` notes were written
/// past, written out as the rule says it: none for the first two, one
/// for the third, then twice as many for each note more, up to
/// [`MAX_SKIPS`].
fn expected_skips(typed_past: u32) -> u32 {
    let mut skipped = 0;
    for n in 3..=typed_past.min(64) {
        skipped = if n == 3 {
            1
        } else {
            (skipped * 2).min(MAX_SKIPS)
        };
    }
    skipped
}

/// Every count of notes written past to 200, and the largest, skips as
/// the rule says.
#[test]
fn skips_double_from_the_third_note_up_to_the_cap() {
    for typed_past in (0..=200).chain([u32::MAX]) {
        assert_eq!(
            skips(typed_past),
            expected_skips(typed_past),
            "{typed_past}"
        );
    }
    assert_eq!(MAX_SKIPS, 16);
}

/// However many notes were written past, the checks a long run makes
/// are the first due one, at step [`EVERY`], and then every
/// `skips + 1`th due one after it, and no others.
#[test]
fn checks_come_every_sixth_step_thinned_by_the_back_off() {
    for typed_past in 0..=12 {
        let mut last = None;
        let mut made = Vec::new();
        for steps in 0..=1_000 {
            if due(steps, last, typed_past) {
                made.push(steps);
                last = Some(steps);
            }
        }
        let apart = EVERY * (expected_skips(typed_past) + 1);
        let expected: Vec<u32> =
            (EVERY..=1_000).step_by(apart as usize).collect();
        assert_eq!(made, expected, "{typed_past} written past");
    }
}

#[test]
fn a_shrunk_transcript_starts_the_count_again() {
    assert!(due(6, Some(120), 9));
    assert!(!due(6, Some(6), 0));
}

#[hegel::test(test_cases = 100)]
fn the_seen_list_keeps_the_last_fifty(tc: TestCase) {
    let lines: Vec<String> =
        tc.draw(gs::vecs(gs::text().max_size(8)).max_size(120));
    let kept = recent(&lines);
    assert_eq!(kept.len(), lines.len().min(SEEN));
    assert!(lines.ends_with(kept));
}

/// What the chat gets for `line`: the note quoted after a fixed lead,
/// with every control character but the space dropped, a zero-width
/// space after each `@` so it names no file, and one after the `ultra`
/// of each word that starts so (in any case) so it is no keyword;
/// nothing else changes.
fn check_chat_text(line: &str) {
    let text = chat_text(line);
    let body = text
        .strip_prefix("Here is a note offered by a side agent:\n\n> ")
        .and_then(|rest| rest.strip_suffix("\n\n"))
        .unwrap_or_else(|| panic!("{text:?}"));
    let kept: String = line
        .chars()
        .filter(|c| !c.is_control() || *c == ' ')
        .collect();
    assert_eq!(body.replace('\u{200b}', ""), kept, "{line:?}");
    for (at, _) in body.match_indices('@') {
        assert!(body[at + 1..].starts_with('\u{200b}'), "{body:?}");
    }
    // Each run of letters and digits that starts `ultra` is cut right
    // after it: `ultra`, then a zero-width space.
    let mut rest = body;
    while let Some(start) = rest.find(char::is_alphanumeric) {
        let run = &rest[start..];
        let end = run
            .find(|c: char| !c.is_alphanumeric())
            .unwrap_or(run.len());
        if run[..end].to_ascii_lowercase().starts_with("ultra") {
            assert_eq!(end, 5, "{:?} in {body:?}", &run[..end]);
            assert!(run[end..].starts_with('\u{200b}'), "{body:?}");
        }
        rest = &run[end..];
    }
}

/// A piece of a note: ordinary text, or what the chat must defuse.
fn piece() -> impl gs::PrintableGenerator<String> {
    hegel::one_of!(
        gs::text().exclude_characters("\u{200b}").max_size(8),
        gs::sampled_from(vec![
            "@",
            "@src/lib.rs",
            "a@b",
            "ultrathink",
            "ULTRATHINK",
            "Ultra",
            "ultra",
            "xultra",
            "\u{7}",
            "\u{1b}[31m",
            "\n",
            "\t",
            " ",
            "é",
        ])
        .map(str::to_owned),
    )
}

/// Whatever the note says, the chat gets it whole, with no live mention
/// or keyword in it.
#[hegel::test(test_cases = 300)]
fn chat_text_has_no_live_mentions_or_keywords(tc: TestCase) {
    let line: String = tc.draw(gs::vecs(piece()).max_size(12)).concat();
    check_chat_text(&line);
    // Notes that once came through live.
    check_chat_text("Run @src/lib.rs with ultrathink.\u{7}\u{1b}[31m");
    check_chat_text("Ultraultrathink");
}
