//! Property inventory:
//! - `slug_matches_the_independent_whole_word_prefix` compares generated
//!   semantic ASCII words with a test-owned lowercase/prefix oracle. It draws
//!   1..=6 words of 1..=70 ASCII alphanumeric bytes, with the first word
//!   starting with `x` and bounded to 64 so every case has an ordinary,
//!   nonfallback result;
//!   separators include punctuation, whitespace, `é`, and `雪`. Shrinking
//!   shortens the word vector and words while preserving the input grammar.
//! - `slug_obeys_64_byte_whole_word_boundaries_and_fallback` fixes lengths 63,
//!   64, and 65, no-ASCII fallback, an overlong first word followed by a short
//!   word, and prefixes where a later whole word does not fit. Expected slugs
//!   are explicit table values; `is_id` and idempotence remain secondary laws.
//!
//! Existing note round trips and plain ASCII wiki-link properties are retained.

mod common;

use common::{id, note};
use hegel::{TestCase, generators as gs, generators::Generator as _};
use tau_memory_host::note::{Link, LinkType, Note, is_id, slug, wiki_links};

const ASCII_WORDS: &str =
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

#[hegel::composite]
fn generated_ordinary_title(tc: &TestCase) -> (Vec<String>, String) {
    let first_suffix: String =
        tc.draw(gs::text().alphabet(ASCII_WORDS).max_size(63));
    let first = format!("x{first_suffix}");
    let remaining: Vec<String> = tc.draw(
        gs::vecs(gs::text().alphabet(ASCII_WORDS).min_size(1).max_size(70))
            .max_size(5),
    );
    let words: Vec<String> = std::iter::once(first).chain(remaining).collect();
    let separator: &str = tc.draw(gs::sampled_from(vec![
        " ", "-", "_", ".", "/", ":", "é", "雪", "\t",
    ]));
    let title = words.join(separator);
    (words, title)
}

fn expected_slug(words: &[String]) -> String {
    let mut expected = String::new();
    for word in words {
        let separator_bytes = usize::from(!expected.is_empty());
        if expected.len() + separator_bytes + word.len() > 64 {
            break;
        }
        if separator_bytes == 1 {
            expected.push('-');
        }
        expected.push_str(&word.to_ascii_lowercase());
    }
    if expected.is_empty() {
        "note".into()
    } else {
        expected
    }
}

#[hegel::test(test_cases = 300)]
fn a_note_reads_back_as_written(tc: TestCase) {
    let note = tc.draw(note());
    note.validate().unwrap();
    let text = note.render();
    assert_eq!(Note::parse(&text).unwrap(), note);
    // And its file does not change on a second trip.
    assert_eq!(Note::parse(&text).unwrap().render(), text);
}

#[hegel::test]
fn a_bare_link_in_the_body_relates(tc: TestCase) {
    let mut note = tc.draw(note());
    let target = tc.draw(id());
    note.body =
        format!("See [[{target}]] and [[{target}]], not [[Not An Id]].");
    let links = note.all_links();
    let bare: Vec<&Link> = links.iter().filter(|l| l.to == target).collect();
    // Once, and as listed when the front matter names it already.
    assert_eq!(bare.len(), 1);
    match note.links.iter().find(|l| l.to == target) {
        Some(listed) => assert_eq!(bare[0], listed),
        None => assert_eq!(bare[0].kind, LinkType::Relates),
    }
    assert!(links.iter().all(|l| l.to != "Not An Id"));
}

#[hegel::test]
fn slug_matches_the_independent_whole_word_prefix(tc: TestCase) {
    let (words, title) = tc.draw(generated_ordinary_title());
    let expected = expected_slug(&words);
    assert_ne!(expected, "note", "generated first word must not fall back");
    let actual = slug(&title);
    assert_eq!(actual, expected, "from {title:?} and words {words:?}");

    // These remain secondary invariants; the expected slug above is computed
    // without either production helper.
    assert!(is_id(&actual), "{actual:?} from {title:?}");
    assert_eq!(slug(&actual), actual);
}

#[test]
fn slug_obeys_64_byte_whole_word_boundaries_and_fallback() {
    let exactly_fits = format!("{} {}", "A".repeat(30), "B".repeat(33));
    let exactly_fits_prefix = format!("{}-{}", "a".repeat(30), "b".repeat(33));
    let overflow_after_prefix =
        format!("{} {} {}", "A".repeat(20), "B".repeat(20), "C".repeat(25));
    let cases = [
        ("A".repeat(63), "a".repeat(63)),
        ("B".repeat(64), "b".repeat(64)),
        ("C".repeat(65), "note".to_owned()),
        (format!("{} short", "D".repeat(65)), "note".to_owned()),
        (exactly_fits, exactly_fits_prefix),
        (
            overflow_after_prefix,
            format!("{}-{}", "a".repeat(20), "b".repeat(20)),
        ),
        ("é 雪".to_owned(), "note".to_owned()),
    ];

    for (title, expected) in cases {
        let actual = slug(&title);
        assert_eq!(actual, expected, "from {title:?}");
        assert!(is_id(&actual), "{actual:?} from {title:?}");
        assert_eq!(slug(&actual), actual);
    }
}

/// `[[id]]` links come back in order, each once; brackets around what
/// is not an id, and an unclosed pair, give nothing.
#[hegel::test]
fn wiki_links_are_the_ids_in_brackets(tc: TestCase) {
    let parts: Vec<(String, Option<String>)> = tc.draw(
        gs::vecs(hegel::one_of!(
            id().map(|id| (format!("[[{id}]]"), Some(id))),
            gs::from_regex("[a-z .]{0,8}").map(|text| (text, None)),
            gs::sampled_from(vec!["[[Not An Id]]", "[[]]", "[[a b]]"])
                .map(|text| (text.to_owned(), None)),
        ))
        .max_size(8),
    );
    let tail = tc.draw(gs::sampled_from(vec!["", "[[x", "[[y]"]));
    let text: String = parts
        .iter()
        .map(|(text, _)| text.as_str())
        .chain([tail])
        .collect();
    let mut want: Vec<String> = Vec::new();
    for id in parts.into_iter().filter_map(|(_, id)| id) {
        if !want.contains(&id) {
            want.push(id);
        }
    }
    assert_eq!(wiki_links(&text), want, "{text:?}");
}

#[test]
fn wiki_links_skip_what_is_not_an_id() {
    assert_eq!(wiki_links("[[a]] [[B]] [[a]] [[c-2]] [[x"), ["a", "c-2"]);
}

/// A line break in any front-matter field would let a value close the
/// front matter or start another key, so `validate` refuses it in every
/// field, whichever break it is and wherever in the value it falls.
#[hegel::test(test_cases = 300)]
fn a_line_break_in_any_front_matter_field_is_refused(tc: TestCase) {
    let mut note = tc.draw(note());
    note.validate().unwrap();
    let broken = format!(
        "{}{}{}",
        tc.draw(common::line()),
        tc.draw(gs::sampled_from(vec!["\n", "\r", "\r\n", "\n+++\n"])),
        tc.draw(gs::text().max_size(10)),
    );
    let link = Link {
        to: tc.draw(id()),
        kind: LinkType::Relates,
        why: None,
    };
    match tc.draw(gs::integers::<u8>().max_value(8)) {
        0 => note.title = broken,
        1 => note.description = broken,
        2 => note.tags.push(broken),
        3 => note.stale = Some(broken),
        4 => note.source.run = Some(broken),
        5 => note.source.commit = Some(broken),
        6 => note.source.files.push(broken),
        7 => note.links.push(Link { to: broken, ..link }),
        _ => note.links.push(Link {
            why: Some(broken),
            ..link
        }),
    }
    assert!(note.validate().is_err(), "{note:?}");
}

/// The fields that name the note cannot be blank either.
#[hegel::test(test_cases = 100)]
fn a_blank_title_or_description_is_refused(tc: TestCase) {
    let mut note = tc.draw(note());
    let blank: String = tc.draw(gs::text().alphabet(" \t").max_size(5));
    if tc.draw(gs::booleans()) {
        note.title = blank;
    } else {
        note.description = blank;
    }
    assert!(note.validate().is_err(), "{note:?}");
}
