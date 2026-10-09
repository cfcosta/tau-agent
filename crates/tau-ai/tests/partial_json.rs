//! `PartialJson`: an incremental parser for streamed tool-call arguments.
//!
//! See `docs/reference/testing.md#tau-ai` for the property inventory this
//! file implements: the differential oracle against `serde_json`, the
//! prefix-consistency and monotonicity invariants over the partial view,
//! the re-chunking metamorphic relation, and pi's leniency rules.

use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_ai::partial_json::{PartialJson, PartialJsonError};
use tau_testing::generators;

/// `sub` is a prefix-consistent snapshot of `full`: every key an object
/// shows is in the final object with a `⊑` value, every array is no
/// longer than its final form with `⊑` elements, every string is a
/// prefix of its final text, and any other scalar is exactly equal.
/// This is the oracle for prefix consistency and monotonicity below, so
/// it stays obviously correct rather than clever.
fn le(sub: &Value, full: &Value) -> bool {
    match (sub, full) {
        (Value::Object(s), Value::Object(f)) => s
            .iter()
            .all(|(k, sv)| f.get(k).is_some_and(|fv| le(sv, fv))),
        (Value::Array(s), Value::Array(f)) => {
            s.len() <= f.len()
                && s.iter().zip(f.iter()).all(|(sv, fv)| le(sv, fv))
        }
        (Value::String(s), Value::String(f)) => f.starts_with(s.as_str()),
        (a, b) => a == b,
    }
}

/// A generated object, its compact JSON text, and one random chunking of
/// that text at character boundaries.
fn arrange(tc: &TestCase) -> (Value, Vec<String>) {
    let value = Value::Object(tc.draw(generators::json_object(3)));
    let text =
        serde_json::to_string(&value).expect("a JSON value always serializes");
    let chunks = tc.draw(generators::char_chunks(text));
    (value, chunks)
}

/// Differential: fed the arguments in any chunking (of compact or
/// pretty-printed text), `PartialJson` agrees with `serde_json` on the
/// complete text.
#[hegel::test(test_cases = 500)]
fn agrees_with_serde_json_for_any_chunking(tc: TestCase) {
    let value = Value::Object(tc.draw(generators::json_object(3)));
    let text = if tc.draw(gs::booleans()) {
        serde_json::to_string_pretty(&value)
            .expect("a JSON value always serializes")
    } else {
        serde_json::to_string(&value).expect("a JSON value always serializes")
    };
    tc.note(&text);
    let chunks = tc.draw(generators::char_chunks(text.clone()));

    let mut parser = PartialJson::new();
    for chunk in &chunks {
        parser.push(chunk);
    }
    let parsed = parser.finish().expect("generated JSON is always valid");

    assert_eq!(parsed, value);
    assert_eq!(
        parsed,
        serde_json::from_str::<Value>(&text)
            .expect("generated JSON is always valid")
    );
}

/// A strict prefix of an object's text, however it is chunked, is not a
/// complete object: `finish` fails, as `serde_json` does on the same
/// text, and the partial view is still consistent with the final value.
#[hegel::test(test_cases = 500)]
fn strict_prefix_fails_to_finish(tc: TestCase) {
    let value = Value::Object(tc.draw(generators::json_object(3)));
    let text = if tc.draw(gs::booleans()) {
        serde_json::to_string_pretty(&value)
            .expect("a JSON value always serializes")
    } else {
        serde_json::to_string(&value).expect("a JSON value always serializes")
    };
    let chars = text.chars().count();
    let cut = tc.draw(gs::integers::<usize>().max_value(chars - 1));
    let prefix: String = text.chars().take(cut).collect();
    tc.note(&prefix);
    let chunks = tc.draw(generators::char_chunks(prefix.clone()));

    let mut parser = PartialJson::new();
    for chunk in &chunks {
        parser.push(chunk);
    }
    assert!(le(parser.value(), &value), "{}", parser.value());
    assert!(serde_json::from_str::<Value>(&prefix).is_err());
    assert!(parser.finish().is_err(), "{prefix:?} finished");
}

/// Each partial parse is consistent with the final value: no field seen
/// early is later changed or dropped.
#[hegel::test(test_cases = 500)]
fn partial_view_is_consistent_with_the_final_value(tc: TestCase) {
    let (value, chunks) = arrange(&tc);

    let mut parser = PartialJson::new();
    for chunk in &chunks {
        parser.push(chunk);
        assert!(
            le(parser.value(), &value),
            "{:?} is not a prefix of the final {value:?}",
            parser.value()
        );
    }
}

/// Monotonicity: consecutive snapshots of the partial view only grow.
#[hegel::test(test_cases = 500)]
fn partial_view_only_grows(tc: TestCase) {
    let (_value, chunks) = arrange(&tc);

    let mut parser = PartialJson::new();
    let mut previous = parser.value().clone();
    for chunk in &chunks {
        parser.push(chunk);
        let current = parser.value().clone();
        assert!(
            le(&previous, &current),
            "{previous:?} is not a prefix of the next snapshot {current:?}"
        );
        previous = current;
    }
}

/// Metamorphic: re-chunking the same text at different character
/// boundaries never changes the parser's snapshot, nor its final value.
#[hegel::test(test_cases = 500)]
fn rechunking_does_not_change_the_result(tc: TestCase) {
    let value = Value::Object(tc.draw(generators::json_object(3)));
    let text =
        serde_json::to_string(&value).expect("a JSON value always serializes");
    let chunks_a = tc.draw(generators::char_chunks(text.clone()));
    let chunks_b = tc.draw(generators::char_chunks(text));

    let mut parser_a = PartialJson::new();
    for chunk in &chunks_a {
        parser_a.push(chunk);
    }
    let mut parser_b = PartialJson::new();
    for chunk in &chunks_b {
        parser_b.push(chunk);
    }
    assert_eq!(
        parser_a.value(),
        parser_b.value(),
        "different chunkings of the same text disagree before finish()"
    );
    assert_eq!(
        parser_a.finish().expect("generated JSON is always valid"),
        parser_b.finish().expect("generated JSON is always valid")
    );
}

/// A faithful port of pi's `repairJson`
/// (`packages/ai/src/utils/json-parse.ts`): escapes a raw control
/// character inside a string, and turns an invalid escape into a
/// doubled backslash (which a standard JSON parser then reads back as a
/// single literal backslash followed by the original character). This
/// is the independent oracle for the fuzz property below: it must not
/// share any code with `PartialJson`'s own string lexer.
///
/// One divergence from a spec-compliant repair, kept because it is what
/// pi's source actually does: a `\u` escape is accepted here as soon as
/// its next character is present, even when the following 4 characters
/// are not all hex digits. Passing those characters straight through
/// then reproduces the original (still invalid) text unchanged, so this
/// repair is a no-op for a malformed `\u` escape rather than a fix —
/// see `has_malformed_unicode_escape` and its use below.
fn repair(json: &str) -> String {
    const VALID_ESCAPES: &[char] =
        &['"', '\\', '/', 'b', 'f', 'n', 'r', 't', 'u'];
    let chars: Vec<char> = json.chars().collect();
    let mut repaired = String::new();
    let mut in_string = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if !in_string {
            repaired.push(c);
            if c == '"' {
                in_string = true;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            repaired.push(c);
            in_string = false;
            i += 1;
            continue;
        }
        if c == '\\' {
            let Some(&next) = chars.get(i + 1) else {
                repaired.push_str("\\\\");
                i += 1;
                continue;
            };
            if next == 'u' {
                let digits: String = chars.iter().skip(i + 2).take(4).collect();
                let valid = digits.len() == 4
                    && digits.chars().all(|d| d.is_ascii_hexdigit());
                if valid {
                    repaired.push_str("\\u");
                    repaired.push_str(&digits);
                    i += 6;
                    continue;
                }
                // Falls through: pi treats `u` as a valid escape
                // character on its own, regardless of what follows.
            }
            if VALID_ESCAPES.contains(&next) {
                repaired.push('\\');
                repaired.push(next);
                i += 2;
                continue;
            }
            repaired.push_str("\\\\");
            i += 1;
            continue;
        }
        if (c as u32) <= 0x1F {
            repaired.push_str(&escape_control_character(c));
        } else {
            repaired.push(c);
        }
        i += 1;
    }
    repaired
}

/// Port of pi's `escapeControlCharacter`.
fn escape_control_character(c: char) -> String {
    match c {
        '\u{8}' => "\\b".to_owned(),
        '\u{c}' => "\\f".to_owned(),
        '\n' => "\\n".to_owned(),
        '\r' => "\\r".to_owned(),
        '\t' => "\\t".to_owned(),
        other => format!("\\u{:04x}", other as u32),
    }
}

/// Detects the one documented divergence between `repair` and
/// `PartialJson` (see `repair`'s doc comment): a `\u` escape inside a
/// string that is not followed by exactly 4 hex digits. `repair`
/// reproduces it unchanged, so re-parsing the repaired text still
/// fails; `PartialJson` instead decodes it as literal text, by design
/// (`incomplete_unicode_escape_is_kept_literally`). The fuzz property
/// below skips generated text that hits this narrow case.
fn has_malformed_unicode_escape(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let mut in_string = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if !in_string {
            if c == '"' {
                in_string = true;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_string = false;
            i += 1;
            continue;
        }
        if c == '\\' {
            match chars.get(i + 1) {
                Some('u') => {
                    let digits: String =
                        chars.iter().skip(i + 2).take(4).collect();
                    let valid = digits.len() == 4
                        && digits.chars().all(|d| d.is_ascii_hexdigit());
                    if !valid {
                        return true;
                    }
                    i += 6;
                }
                Some(_) => i += 2,
                None => i += 1,
            }
            continue;
        }
        i += 1;
    }
    false
}

/// Differential (possibly invalid JSON): a valid object's text, mutated
/// by 1-3 small edits and then chunked arbitrarily, agrees with pi's
/// `repairJson` followed by a standard parse — whether that succeeds or
/// fails, and, when it succeeds, on the resulting value. This is the
/// property that actually exercises structural error handling (an
/// unmutated, always-valid text never reaches most of it).
#[hegel::test(test_cases = 500)]
fn agrees_with_pis_repair_for_mutated_text(tc: TestCase) {
    let value = Value::Object(tc.draw(generators::json_object(3)));
    let text =
        serde_json::to_string(&value).expect("a JSON value always serializes");
    let mutated = tc.draw(generators::mutate_text(text));
    tc.note(&mutated);

    // See `has_malformed_unicode_escape`'s doc comment: this is a
    // narrow, documented divergence, not a bug, and it needs an actual
    // `\u` escape to already exist and then be corrupted in its digit
    // run, so the rejection rate stays low.
    tc.assume(!has_malformed_unicode_escape(&mutated));

    let mut parser = PartialJson::new();
    for chunk in tc.draw(generators::char_chunks(mutated.clone())) {
        parser.push(&chunk);
        assert!(
            parser.value().is_object(),
            "the partial view must always be an object, got {:?}",
            parser.value()
        );
    }
    let ours = parser.finish();
    let oracle = serde_json::from_str::<Value>(&repair(&mutated))
        .ok()
        .filter(Value::is_object);

    match (ours, oracle) {
        (Ok(v), Some(v2)) => {
            assert_eq!(v, v2, "agree that it parses, but not on the value")
        }
        (Err(_), None) => {}
        (ours, oracle) => panic!(
            "disagreement on {mutated:?}: ours = {ours:?}, pi's repair = {oracle:?}"
        ),
    }
}

/// A string containing raw control characters (leniency, matching pi's
/// `repairJson`) comes back unchanged.
#[hegel::test(test_cases = 500)]
fn raw_control_characters_in_a_string_are_kept(tc: TestCase) {
    let raw: String = tc
        .draw(
            gs::vecs(hegel::one_of!(
                gs::characters().min_codepoint(0).max_codepoint(0x1F),
                gs::characters().min_codepoint(0x20).max_codepoint(0x7E),
            ))
            .max_size(12),
        )
        .into_iter()
        .collect();
    let text = format!(r#"{{"k":{}}}"#, raw_string_literal(&raw));
    tc.note(&text);

    let mut parser = PartialJson::new();
    parser.push(&text);
    let value = parser.finish().expect("only \" and \\ are escaped");

    assert_eq!(value["k"], Value::String(raw));
}

/// Builds a JSON string literal for `s` that escapes only `"` and `\`,
/// leaving every other character — including raw control characters —
/// exactly as given. This is deliberately more lenient than a
/// spec-compliant JSON writer, to build the inputs `PartialJson`'s own
/// leniency is meant to accept.
fn raw_string_literal(s: &str) -> String {
    let mut out = String::from('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// Leniency (pi's `repairJson`): a backslash followed by a character
/// that is not a valid JSON escape is kept literally.
///
/// From `packages/ai/src/utils/json-parse.ts` in pi.
#[test]
fn invalid_escape_is_kept_literally() {
    let mut parser = PartialJson::new();
    parser.push(r#"{"p":"C:\q"}"#);
    let value = parser
        .finish()
        .expect("leniency accepts the invalid escape");
    assert_eq!(value["p"], "C:\\q");
}

/// Leniency (pi's `repairJson`): `\u` not followed by 4 hex digits keeps
/// the backslash literally, digits and all.
///
/// From `packages/ai/src/utils/json-parse.ts` in pi.
#[test]
fn incomplete_unicode_escape_is_kept_literally() {
    let mut parser = PartialJson::new();
    parser.push(r#"{"p":"\u12"}"#);
    let value = parser
        .finish()
        .expect("leniency accepts the incomplete escape");
    assert_eq!(value["p"], "\\u12");
}

/// Known case: a string holding U+2028/U+2029 and an escaped surrogate
/// pair, with the pair's own escape split across chunks (`\ud83` ends
/// one chunk, `e\udd80` starts the next). The pair must still combine
/// into the single scalar it encodes.
#[test]
fn surrogate_pair_and_line_separators_split_across_chunks() {
    let mut parser = PartialJson::new();
    parser.push("{\"emoji\":\"\u{2028}\\ud83");
    parser.push("e\\udd80\u{2029}\"}");
    let value = parser.finish().expect("a complete, valid surrogate pair");
    assert_eq!(value["emoji"], "\u{2028}\u{1f980}\u{2029}");
}

/// Truncated input never reaches a complete value.
#[test]
fn truncated_input_is_an_error() {
    let mut parser = PartialJson::new();
    parser.push(r#"{"a":"#);
    assert!(parser.finish().is_err());
}

/// The root value must be a JSON object.
#[test]
fn non_object_root_is_an_error() {
    for text in [r#"[1]"#, "1"] {
        let mut parser = PartialJson::new();
        parser.push(text);
        assert!(
            parser.finish().is_err(),
            "{text:?} is not an object and should not parse"
        );
    }
}

/// Nothing may follow the root object but whitespace: any whitespace
/// is fine and leaves the object as it was, and any other character is
/// an error, however the text is chunked.
#[hegel::test(test_cases = 300)]
fn only_whitespace_may_follow_the_root_object(tc: TestCase) {
    let value = Value::Object(tc.draw(generators::json_object(3)));
    let text = serde_json::to_string(&value).unwrap();
    let blanks = tc.draw(gs::text().alphabet(" \t\r\n").max_size(4));
    let mut parser = PartialJson::new();
    for chunk in tc.draw(generators::char_chunks(format!("{text}{blanks}"))) {
        parser.push(&chunk);
    }
    assert_eq!(parser.finish().expect("trailing whitespace"), value);

    let stray =
        tc.draw(gs::characters().exclude_categories(&["Z", "Cc", "Cs"]));
    let mut parser = PartialJson::new();
    for chunk in
        tc.draw(generators::char_chunks(format!("{text}{blanks}{stray}")))
    {
        parser.push(&chunk);
    }
    assert!(parser.finish().is_err(), "{text}{blanks}{stray:?}");
}

/// Before anything is pushed, the partial view is an empty object.
#[test]
fn empty_input_value_is_an_empty_object() {
    let parser = PartialJson::new();
    assert_eq!(parser.value(), &json!({}));
}

/// A valid escape decodes to its single character, not the two
/// characters of its own text: `\/` is `/`, not `\` followed by `/`.
#[test]
fn escaped_forward_slash_decodes_to_a_slash() {
    let mut parser = PartialJson::new();
    parser.push(r#"{"p":"a\/b"}"#);
    let value = parser.finish().expect("`/` is a valid JSON escape");
    assert_eq!(value["p"], "a/b");
}

/// A character that cannot start or continue a number is reported
/// precisely, with the number's text so far — not folded into some
/// other error by treating it as a stray terminator.
#[test]
fn invalid_character_after_a_number_is_reported_precisely() {
    let mut parser = PartialJson::new();
    parser.push(r#"{"a":1x}"#);
    assert_eq!(
        parser.finish(),
        Err(PartialJsonError::InvalidNumber("1".to_owned()))
    );
}

/// Closing the root object is enough to finish, without any further
/// input: the frame it closes must actually be popped.
#[test]
fn closing_the_root_object_finishes_without_more_input() {
    let mut parser = PartialJson::new();
    parser.push("{}");
    assert_eq!(parser.finish().unwrap(), json!({}));
}

/// Every error variant renders a non-empty message that names its own
/// specifics, not a shared or blank string.
#[test]
fn each_error_displays_a_specific_message() {
    let cases: [(PartialJsonError, &str); 6] = [
        (PartialJsonError::RootNotObject, "object"),
        (PartialJsonError::TrailingGarbage, "after the root object"),
        (PartialJsonError::UnexpectedEof, "end of input"),
        (PartialJsonError::UnexpectedChar('x'), "'x'"),
        (PartialJsonError::InvalidNumber("1x".to_owned()), "1x"),
        (PartialJsonError::LoneSurrogate, "surrogate"),
    ];
    let mut seen = std::collections::HashSet::new();
    for (err, must_contain) in cases {
        let message = err.to_string();
        assert!(!message.is_empty(), "{err:?} displayed an empty message");
        assert!(
            message.contains(must_contain),
            "{err:?} displayed {message:?}, expected it to mention {must_contain:?}"
        );
        assert!(
            seen.insert(message.clone()),
            "{err:?} displayed {message:?}, which another variant already used"
        );
    }
}
