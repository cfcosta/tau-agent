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

/// What a note's line is written in: letters, digits, punctuation and
/// characters of two, three and four bytes, so a count of bytes and a
/// count of characters differ.
const ALPHABET: &str = "abcXYZ019 `,;_-é漢🦀";

/// `count` characters a note's line may hold before its period: no white
/// space at either end (the parser trims it), and never the word `none`,
/// which is the reply that has nothing to say.
fn body(tc: &TestCase, count: usize) -> String {
    let mut chars: Vec<char> = tc
        .draw(
            gs::text()
                .alphabet(ALPHABET)
                .min_size(count)
                .max_size(count),
        )
        .chars()
        .collect();
    for at in [0, chars.len().saturating_sub(1)] {
        if chars.get(at) == Some(&' ') {
            chars[at] = 'a';
        }
    }
    let body: String = chars.into_iter().collect();
    if body.eq_ignore_ascii_case("none") {
        format!("x{}", &body[1..])
    } else {
        body
    }
}

/// A line `count` characters long, its period included.
fn line_of(tc: &TestCase, count: usize) -> String {
    format!("{}.", body(tc, count - 1))
}

/// A line one row long that ends in a period, within the limit: its
/// shortest and longest lengths drawn as often as the rest.
#[hegel::composite]
fn line(tc: &TestCase) -> String {
    let count = tc.draw(hegel::one_of!(
        gs::sampled_from(vec![1, 2, MAX_LINE - 1, MAX_LINE]),
        gs::integers::<usize>().min_value(1).max_value(MAX_LINE),
    ));
    line_of(tc, count)
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

/// White space a reply may come wrapped in.
fn padding() -> impl gs::PrintableGenerator<String> {
    gs::sampled_from(vec!["", "\n", "  ", "\n\n", " \n "]).map(str::to_owned)
}

/// A note written the way the parser reads it, with or without an
/// explanation and wrapped in white space, reads back as written, up to
/// a line of [`MAX_LINE`] characters however many bytes they take.
#[hegel::test(test_cases = 200)]
fn a_written_note_reads_back(tc: TestCase) {
    let line = tc.draw(line());
    let tag = tc.draw(gs::sampled_from(Tag::ALL.to_vec()).print_as_debug());
    let explain = tc.draw(gs::optional(explanation().print_as_debug()));
    let (before, after) = (tc.draw(padding()), tc.draw(padding()));
    let text = format!(
        "{before}{}{after}",
        reply::write(tag, &line, explain.as_ref())
    );
    assert_eq!(parse(&text), Ok(Reply::Note { tag, line, explain }));
}

/// An explanation's lines as [`reply::write`] puts them, after the
/// `explain:` head: the bold title, then a bullet each.
fn explain_lines(explain: &Explain) -> Vec<String> {
    std::iter::once(format!("**{}**", explain.title))
        .chain(explain.bullets.iter().map(|bullet| format!("- {bullet}")))
        .collect()
}

/// A note written as the parser reads it, then broken in one way, is
/// refused for that fault and no other: no `tag:` line; a tag that is
/// not one of the two; a line without its period; a line longer than
/// [`MAX_LINE`] characters, counted as characters; `none` with more
/// after it; and an explanation that is not a bold title and 3 to 5
/// bullets.
#[hegel::test(test_cases = 300)]
fn a_note_with_one_fault_is_refused_for_it(tc: TestCase) {
    let tag = tc.draw(gs::sampled_from(Tag::ALL.to_vec()).print_as_debug());
    let line = tc.draw(line());
    let explain = tc.draw(gs::optional(explanation().print_as_debug()));
    // What follows the tag line when the explanation is kept whole.
    let tail: Vec<String> = explain
        .as_ref()
        .map(|explain| {
            std::iter::once("explain:".to_owned())
                .chain(explain_lines(explain))
                .collect()
        })
        .unwrap_or_default();
    let tag_line = format!("tag: {}", tag.label());
    let note = |line: &str, tag_line: Option<&str>, tail: &[String]| {
        std::iter::once(format!("learn: {line}"))
            .chain(tag_line.map(str::to_owned))
            .chain(tail.iter().cloned())
            .collect::<Vec<_>>()
            .join("\n")
    };
    let (text, expected) = match tc.draw(gs::integers::<u8>().max_value(8)) {
        0 => (note(&line, None, &tail), Unreadable::NoTag),
        1 => {
            let word = tc.draw(gs::sampled_from(vec![
                "FYI",
                "heads up",
                "HEADS UP",
                "Heads-up",
                "You should know!",
                "Note",
            ]));
            (
                note(&line, Some(&format!("tag: {word}")), &tail),
                Unreadable::UnknownTag(word.to_owned()),
            )
        }
        2 => {
            let count = tc
                .draw(gs::integers::<usize>().min_value(1).max_value(MAX_LINE));
            let bare = body(&tc, count);
            (note(&bare, Some(&tag_line), &tail), Unreadable::NoPeriod)
        }
        3 => {
            let count = tc.draw(hegel::one_of!(
                gs::just(MAX_LINE + 1),
                gs::integers::<usize>()
                    .min_value(MAX_LINE + 1)
                    .max_value(MAX_LINE + 40),
            ));
            let long = line_of(&tc, count);
            (
                note(&long, Some(&tag_line), &tail),
                Unreadable::TooLong(count),
            )
        }
        4 => {
            let none = tc.draw(gs::sampled_from(vec!["none", "None.", "NONE"]));
            let written = note(&line, Some(&tag_line), &tail);
            (format!("learn: {none}\n{written}"), Unreadable::ManyLines)
        }
        // The explanation broken: too few or too many bullets, a title
        // that is not bold, a line that is not a bullet, or no
        // `explain:` head.
        fault => {
            let explain = explain
                .unwrap_or_else(|| tc.draw(explanation().print_as_debug()));
            let mut lines = explain_lines(&explain);
            match fault {
                5 => {
                    let bullets =
                        tc.draw(gs::sampled_from(vec![0_usize, 1, 2, 6, 7]));
                    lines.truncate(1);
                    lines.extend((0..bullets).map(|n| format!("- bullet {n}")));
                    lines.insert(0, "explain:".into());
                }
                6 => {
                    lines[0] = explain.title.clone();
                    lines.insert(0, "explain:".into());
                }
                7 => {
                    let at = tc.draw(
                        gs::integers::<usize>()
                            .min_value(1)
                            .max_value(lines.len()),
                    );
                    lines.insert(at, "and then some".into());
                    lines.insert(0, "explain:".into());
                }
                _ => {}
            }
            (note(&line, Some(&tag_line), &lines), Unreadable::BadExplain)
        }
    };
    assert_eq!(parse(&text), Err(expected), "{text:?}");
}
