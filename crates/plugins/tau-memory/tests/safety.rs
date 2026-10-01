//! Secrets never reach a note, plain prose passes untouched, and text
//! that addresses the model is refused.

use hegel::{TestCase, generators as gs, generators::Generator as _};
use tau_memory::safety::{REDACTED, redact, refusal};

/// Prose from words that are neither secrets nor instructions.
#[hegel::composite]
fn prose(tc: &TestCase) -> String {
    tc.draw(
        gs::vecs(gs::sampled_from(vec![
            "the",
            "retry",
            "loop",
            "reads",
            "header",
            "cargo",
            "test",
            "src/lib.rs",
            "failed",
            "because",
            "42",
            "ms",
            "fix:",
            "use",
            "jitter.",
            "(see",
            "docs)",
        ]))
        .max_size(30),
    )
    .join(" ")
}

/// `text` with each letter's case drawn: the patterns ignore case where
/// they say so.
#[hegel::composite]
fn cased(tc: &TestCase, text: &'static str) -> String {
    text.chars()
        .map(|c| {
            if tc.draw(gs::booleans()) {
                c.to_ascii_uppercase()
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect()
}

/// A secret's body. A `-`, `.` or `=` inside it is followed by a digit,
/// so no other kind of secret can start within it.
fn body(chars: &str, min: usize, max: usize) -> gs::RegexGenerator {
    gs::from_regex(&format!(
        "[{chars}]{{{min},{max}}}(?:-[0-9][{chars}]{{0,6}}){{0,2}}"
    ))
}

/// One secret of each shape `safety` knows, bodies with `_` and `-`,
/// any case where the pattern ignores it.
#[hegel::composite]
fn secret(tc: &TestCase) -> String {
    let kind = tc.draw(gs::integers::<u8>().max_value(8));
    match kind {
        0 => {
            let block = tc.draw(gs::sampled_from(vec![
                "RSA PRIVATE KEY",
                "OPENSSH PRIVATE KEY",
                "EC PRIVATE KEY",
                "PRIVATE KEY",
            ]));
            let lines: String =
                tc.draw(gs::from_regex(r"(?:[A-Za-z0-9+/=]{1,64}\n){1,4}"));
            format!("-----BEGIN {block}-----\n{lines}-----END {block}-----")
        }
        1 => {
            let prefix = tc.draw(gs::sampled_from(vec!["", "ant-", "proj-"]));
            format!("sk-{prefix}{}", tc.draw(body("A-Za-z0-9_", 20, 40)))
        }
        2 => tc.draw(gs::from_regex("gh[pousr]_[A-Za-z0-9]{30,40}")),
        3 => tc.draw(gs::from_regex("github_pat_[A-Za-z0-9_]{30,40}")),
        4 => tc.draw(gs::from_regex("(?:AKIA|ASIA)[A-Z0-9]{16}")),
        5 => format!(
            "{}{}",
            tc.draw(gs::from_regex("xox[abprs]-")),
            tc.draw(body("A-Za-z0-9", 10, 30))
        ),
        6 => tc.draw(gs::from_regex(
            "AIza(?:[A-Za-z0-9_]{35}|[A-Za-z0-9_]{20}-[0-9][A-Za-z0-9_]{13})",
        )),
        7 => format!(
            "{}{}{}",
            tc.draw(cased("bearer")),
            tc.draw(gs::from_regex("[ \t]{1,3}")),
            tc.draw(gs::from_regex(
                "[A-Za-z0-9_]{20,30}(?:[.=][0-9][A-Za-z0-9_]{0,6}){0,2}"
            )),
        ),
        _ => {
            let key = tc.draw(gs::sampled_from(vec![
                "password", "passwd", "secret", "token", "api_key", "api-key",
                "apikey",
            ]));
            format!(
                "{}{}{}",
                tc.draw(cased(key)),
                tc.draw(gs::from_regex(r#" {0,2}[:=] {0,2}['"]?"#)),
                tc.draw(body("A-Za-z0-9_", 8, 20)),
            )
        }
    }
}

/// A secret in prose is replaced by the redaction mark, once, and the
/// prose around it is left exactly as it was.
#[hegel::test(test_cases = 300)]
fn a_secret_never_survives(tc: TestCase) {
    let secret = tc.draw(secret());
    let (a, b) = (tc.draw(prose()), tc.draw(prose()));
    let text = format!("{a} {secret} {b}");
    assert_eq!(redact(&text), (format!("{a} {REDACTED} {b}"), 1), "{text}");
}

#[hegel::test(test_cases = 300)]
fn plain_prose_passes_untouched(tc: TestCase) {
    let text = tc.draw(prose());
    assert_eq!(redact(&text), (text.clone(), 0));
    assert_eq!(refusal(&text), None);
}

/// A run of whitespace, as `\s+` matches it.
#[hegel::composite]
fn gap(tc: &TestCase) -> String {
    tc.draw(gs::from_regex("[ \t\n]{1,3}"))
}

/// A phrase from the grammar of the instruction patterns, in any case,
/// with any whitespace between its words.
#[hegel::composite]
fn instruction(tc: &TestCase) -> String {
    let optional = |tc: &TestCase, words: Vec<&'static str>| -> String {
        if tc.draw(gs::booleans()) {
            let word = tc.draw(gs::sampled_from(words));
            format!("{}{}", tc.draw(cased(word)), tc.draw(gap()))
        } else {
            String::new()
        }
    };
    let one = |tc: &TestCase, words: Vec<&'static str>| -> String {
        tc.draw(cased(tc.draw(gs::sampled_from(words))))
    };
    let kind = tc.draw(gs::integers::<u8>().max_value(5));
    match kind {
        0 => format!(
            "{}{}{}{}{}{}{}",
            tc.draw(cased("ignore")),
            tc.draw(gap()),
            optional(tc, vec!["all", "any"]),
            optional(tc, vec!["the"]),
            one(tc, vec!["previous", "prior", "above", "earlier"]),
            tc.draw(gap()),
            one(tc, vec!["instructions", "messages", "rules"]),
        ),
        1 => format!(
            "{}{}{}{}{}",
            tc.draw(cased("disregard")),
            tc.draw(gap()),
            optional(tc, vec!["all", "any"]),
            optional(tc, vec!["the"]),
            one(tc, vec!["previous", "prior", "above", "system"]),
        ),
        2 => format!(
            "{}{}{}{}{}{}{}",
            tc.draw(cased("you")),
            tc.draw(gap()),
            tc.draw(cased("are")),
            tc.draw(gap()),
            tc.draw(cased("now")),
            tc.draw(gap()),
            one(tc, vec!["a", "an", "in"]),
        ),
        3 => format!(
            "{}{}{}{}{}",
            tc.draw(cased("new")),
            tc.draw(gap()),
            tc.draw(cased("system")),
            tc.draw(gap()),
            tc.draw(cased("prompt")),
        ),
        4 => format!(
            "<{}{}{}{}>",
            tc.draw(gs::sampled_from(vec!["", "|"])),
            tc.draw(gs::from_regex("[ \t]{0,2}")),
            one(tc, vec!["im_start", "im_end", "system"]),
            tc.draw(gs::from_regex("[ \t]{0,2}\\|?")),
        ),
        _ => format!(
            "[{}{}{}]{}:",
            tc.draw(gs::from_regex("[ \t]{0,2}")),
            tc.draw(cased("system")),
            tc.draw(gs::from_regex("[ \t]{0,2}")),
            tc.draw(gs::from_regex("[ \t]{0,2}")),
        ),
    }
}

#[hegel::test(test_cases = 300)]
fn instructions_to_the_model_are_refused(tc: TestCase) {
    let phrase = tc.draw(instruction());
    let text = format!("{} {} {}", tc.draw(prose()), phrase, tc.draw(prose()));
    let why = refusal(&text).unwrap_or_else(|| panic!("{text:?}"));
    assert!(why.contains("instruction"), "{why}");
}

/// Phrases that share an instruction's first words, and characters
/// right beside the hidden ranges, are prose.
#[hegel::test]
fn near_misses_are_not_refused(tc: TestCase) {
    let near = tc.draw(hegel::one_of!(
        gs::sampled_from(vec![
            "ignore the noise".to_owned(),
            "you are now done".to_owned(),
            "disregard it".to_owned(),
            "a new system".to_owned(),
        ]),
        gs::sampled_from(vec![
            '\u{200A}', '\u{2010}', '\u{2029}', '\u{205F}', '\u{2065}',
            '\u{206A}', '\u{FEFE}', '\u{FF00}',
        ])
        .map(String::from),
    ));
    let text = format!("{} {near} {}", tc.draw(prose()), tc.draw(prose()));
    assert_eq!(refusal(&text), None, "{text:?}");
}

/// A character from each range that hides text.
#[hegel::composite]
fn hidden(tc: &TestCase) -> char {
    let range = tc.draw(gs::sampled_from(vec![
        (0x200B, 0x200F),
        (0x202A, 0x202E),
        (0x2060, 0x2064),
        (0x2066, 0x2069),
        (0xFEFF, 0xFEFF),
    ]));
    let code =
        tc.draw(gs::integers::<u32>().min_value(range.0).max_value(range.1));
    char::from_u32(code).unwrap()
}

/// A hidden character anywhere is refused, and the refusal names it.
#[hegel::test(test_cases = 200)]
fn a_hidden_character_is_refused(tc: TestCase) {
    let hidden = tc.draw(hidden());
    let before = tc.draw(prose());
    let text = format!("{before}{hidden}{}", tc.draw(prose()));
    let why = refusal(&text).unwrap();
    assert!(why.contains("invisible"), "{why}");
    assert!(
        why.contains(&format!("U+{:04X}", u32::from(hidden))),
        "{why}"
    );
}
