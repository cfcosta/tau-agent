//! The reply parser: `learn: none`, or a note read strictly.

use hegel::{TestCase, generators as gs, generators::Generator as _};
use tau_watcher::{
    record::{Explain, Tag},
    reply::{self, MAX_LINE, Reply, Unreadable, parse},
};

const NOTE: &str = "learn: `cargo test` rebuilds in debug, so each run takes minutes.\n\
tag: Heads up\n\
explain:\n\
**Debug tests rebuild everything**\n\
- `cargo test` builds in its own profile.\n\
- Nothing from `--release` is reused.\n\
- Each run recompiles every crate.";

fn explain() -> Explain {
    Explain {
        title: "Debug tests rebuild everything".into(),
        bullets: vec![
            "`cargo test` builds in its own profile.".into(),
            "Nothing from `--release` is reused.".into(),
            "Each run recompiles every crate.".into(),
        ],
    }
}

#[test]
fn none_is_nothing_however_it_is_spelled() {
    for text in [
        "learn: none",
        "  learn: none\n",
        "Learn: None.",
        "learn:none\n\n",
    ] {
        assert_eq!(parse(text), Ok(Reply::Nothing), "{text:?}");
    }
}

#[test]
fn none_with_more_after_it_is_not_a_reply() {
    assert_eq!(
        parse("learn: none\nbut also this"),
        Err(Unreadable::ManyLines)
    );
}

#[test]
fn a_note_with_an_explanation_reads() {
    assert_eq!(
        parse(NOTE),
        Ok(Reply::Note {
            tag: Tag::HeadsUp,
            line: "`cargo test` rebuilds in debug, so each run takes minutes."
                .into(),
            explain: Some(explain()),
        })
    );
}

#[test]
fn a_note_may_leave_the_explanation_out() {
    let text = "learn: The key expires tomorrow.\ntag: You should know\n";
    assert_eq!(
        parse(text),
        Ok(Reply::Note {
            tag: Tag::YouShouldKnow,
            line: "The key expires tomorrow.".into(),
            explain: None,
        })
    );
}

#[test]
fn a_note_without_a_tag_is_not_read() {
    assert_eq!(parse("learn: The key expires."), Err(Unreadable::NoTag));
    assert_eq!(
        parse("learn: The key expires.\nexplain:\n**x**\n- a\n- b\n- c"),
        Err(Unreadable::NoTag)
    );
}

#[test]
fn an_unknown_tag_is_not_read() {
    assert_eq!(
        parse("learn: The key expires.\ntag: FYI"),
        Err(Unreadable::UnknownTag("FYI".into()))
    );
}

#[test]
fn a_line_over_the_limit_is_not_read() {
    let line = format!("{}.", "a".repeat(MAX_LINE));
    assert_eq!(
        parse(&format!("learn: {line}\ntag: Heads up")),
        Err(Unreadable::TooLong(MAX_LINE + 1))
    );
    let line = format!("{}.", "a".repeat(MAX_LINE - 1));
    assert!(parse(&format!("learn: {line}\ntag: Heads up")).is_ok());
}

#[test]
fn a_line_without_a_period_is_not_read() {
    assert_eq!(
        parse("learn: The key expires\ntag: Heads up"),
        Err(Unreadable::NoPeriod)
    );
}

#[test]
fn a_quoted_or_fenced_reply_is_not_read() {
    for text in [
        "```\nlearn: none\n```",
        "> learn: none",
        "\"learn: none\"",
        "`learn: none`",
    ] {
        assert_eq!(parse(text), Err(Unreadable::Quoted), "{text:?}");
    }
}

#[test]
fn prose_and_blanks_are_not_read() {
    assert_eq!(parse(""), Err(Unreadable::Empty));
    assert_eq!(parse("  \n "), Err(Unreadable::Empty));
    assert_eq!(parse("Sure! Here is a note."), Err(Unreadable::NoLearn));
    assert_eq!(parse("note: something."), Err(Unreadable::NoLearn));
    assert_eq!(parse("learn:\ntag: Heads up"), Err(Unreadable::NoLine));
}

#[test]
fn a_broken_explanation_drops_the_note() {
    let head = "learn: A thing.\ntag: Heads up\nexplain:\n";
    for tail in [
        "**t**\n- a\n- b",
        "**t**\n- a\n- b\n- c\n- d\n- e\n- f",
        "t\n- a\n- b\n- c",
        "**t**\n- a\n- b\nc",
        "- a\n- b\n- c",
    ] {
        assert_eq!(
            parse(&format!("{head}{tail}")),
            Err(Unreadable::BadExplain),
            "{tail:?}"
        );
    }
    assert_eq!(
        parse("learn: A thing.\ntag: Heads up\nand then some"),
        Err(Unreadable::BadExplain)
    );
}

/// A line one row long that ends in a period, within the limit.
#[hegel::composite]
fn line(tc: &TestCase) -> String {
    let body = tc.draw(gs::from_regex("[A-Za-z`][A-Za-z0-9 `,;_-]{0,200}"));
    format!("{}.", body.trim_end().trim_end_matches('.'))
}

#[hegel::composite]
fn explanation(tc: &TestCase) -> Explain {
    let word =
        || gs::from_regex("[A-Za-z`][A-Za-z0-9 `,._-]{0,40}[A-Za-z0-9`]");
    Explain {
        title: tc.draw(word()),
        bullets: tc.draw(gs::vecs(word()).min_size(3).max_size(5)),
    }
}

/// A note written the way the parser reads it reads back as written.
#[hegel::test(test_cases = 200)]
fn a_written_note_reads_back(tc: TestCase) {
    let line = tc.draw(line());
    let tag = tc.draw(gs::sampled_from(Tag::ALL.to_vec()).print_as_debug());
    let explain = tc.draw(gs::optional(explanation().print_as_debug()));
    let text = reply::write(tag, &line, explain.as_ref());
    assert_eq!(parse(&text), Ok(Reply::Note { tag, line, explain }));
}

/// No text makes the parser panic, and whatever it reads as a note has
/// a line within the limit that ends in a period.
#[hegel::test(test_cases = 300)]
fn any_text_reads_or_is_refused(tc: TestCase) {
    let text = tc.draw(gs::text().max_size(400));
    let text = if tc.draw(gs::booleans()) {
        format!("learn: {text}")
    } else {
        text
    };
    if let Ok(Reply::Note { line, .. }) = parse(&text) {
        assert!(line.chars().count() <= MAX_LINE);
        assert!(line.ends_with('.'));
        assert!(!line.contains('\n'));
    }
}
