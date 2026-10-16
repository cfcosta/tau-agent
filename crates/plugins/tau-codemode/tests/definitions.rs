//! A module definition's version is the digest of its content: the same
//! content has the same version, any change has another, and a definition
//! read back from storage with its content or version altered does not
//! verify. Names and versions have exact shapes.

use std::collections::BTreeMap;

use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_codemode::modules::{Definition, valid_version, validate_name};

fn name_ok(name: &str) -> bool {
    let bytes = name.as_bytes();
    (1..=64).contains(&bytes.len())
        && (bytes[0].is_ascii_alphabetic() || bytes[0] == b'_')
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'_')
}

const IDENT_START: &str =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz_";
const IDENT_REST: &str =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_";

/// An identifier of exactly `len` ASCII characters (`len >= 1`).
#[hegel::composite]
fn identifier(tc: &TestCase, len: usize) -> String {
    let first: String =
        tc.draw(gs::text().alphabet(IDENT_START).min_size(1).max_size(1));
    let rest: String = tc.draw(
        gs::text()
            .alphabet(IDENT_REST)
            .min_size(len - 1)
            .max_size(len - 1),
    );
    format!("{first}{rest}")
}

/// Valid names, and the ways a name goes wrong: empty, too long, a
/// digit first, a character outside the identifier set, a non-ASCII
/// letter. Lengths are drawn at the 64-byte limit and either side of
/// it as well as across the range, so a name of 64 and one of 65
/// identifier characters are both common.
#[hegel::composite]
fn name(tc: &TestCase) -> String {
    let len = tc.draw(hegel::one_of!(
        gs::sampled_from(vec![1_usize, 63, 64, 65]),
        gs::integers::<usize>().min_value(1).max_value(70),
    ));
    let valid = tc.draw(identifier(len));
    match tc.draw(gs::integers::<u8>().max_value(5)) {
        0 => String::new(),
        1 => format!(
            "{}{}",
            tc.draw(gs::from_regex("[0-9]").fullmatch(true)),
            &valid[1..]
        ),
        2 => {
            let at = tc.draw(gs::integers::<usize>().max_value(valid.len()));
            let bad =
                tc.draw(gs::sampled_from(vec!["-", " ", ".", "é", "雪", "/"]));
            format!("{}{bad}{}", &valid[..at], &valid[at..])
        }
        3 => format!("é{}", &valid[1..]),
        _ => valid,
    }
}

#[hegel::test(test_cases = 500)]
fn a_module_name_is_a_short_ascii_identifier(tc: TestCase) {
    let name = tc.draw(name());
    assert_eq!(validate_name(&name).is_ok(), name_ok(&name), "{name:?}");
}

/// A version is exactly 64 lowercase hex digits.
#[hegel::test(test_cases = 500)]
fn a_version_is_sixty_four_lowercase_hex_digits(tc: TestCase) {
    let text = tc.draw(hegel::one_of!(
        gs::from_regex("[0-9a-f]{64}").fullmatch(true),
        gs::from_regex("[0-9a-fA-F]{60,68}").fullmatch(true),
        gs::from_regex("[0-9a-g ]{0,70}").fullmatch(true),
    ));
    let want = text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    assert_eq!(valid_version(&text), want, "{text:?}");
}

#[hegel::composite]
fn content(tc: &TestCase) -> (String, String, Value, BTreeMap<String, String>) {
    let signatures = Value::Object(
        tc.draw(
            gs::hashmaps(
                gs::from_regex("[a-z]{1,4}").fullmatch(true),
                gs::sampled_from(vec![
                    json!("string"),
                    json!(1),
                    json!(null),
                    json!({"a": [1, 2]}),
                ]),
            )
            .max_size(4),
        )
        .into_iter()
        .collect(),
    );
    let dependencies = tc
        .draw(
            gs::hashmaps(
                gs::from_regex("[a-z_]{1,6}").fullmatch(true),
                gs::from_regex("[0-9a-f]{64}").fullmatch(true),
            )
            .max_size(3),
        )
        .into_iter()
        .collect();
    (
        tc.draw(gs::from_regex("[a-z_][a-z0-9_]{0,10}").fullmatch(true)),
        tc.draw(gs::text().max_size(50)),
        signatures,
        dependencies,
    )
}

fn build(c: &(String, String, Value, BTreeMap<String, String>)) -> Definition {
    Definition::new(c.0.clone(), c.1.clone(), c.2.clone(), c.3.clone()).unwrap()
}

/// Same content, same version, whichever order the signature keys
/// were written in; every field takes part in the digest.
#[hegel::test(test_cases = 300)]
fn the_version_is_the_digest_of_the_content(tc: TestCase) {
    let content = tc.draw(content());
    let definition = build(&content);
    assert!(valid_version(definition.version()));
    definition.verify().unwrap();
    assert_eq!(build(&content).version(), definition.version());

    // Keys reversed: the same signatures.
    let reordered = match &content.2 {
        Value::Object(map) => Value::Object(
            map.iter()
                .rev()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        ),
        other => other.clone(),
    };
    let mut again = content.clone();
    again.2 = reordered;
    assert_eq!(build(&again).version(), definition.version());

    let mut other = content.clone();
    match tc.draw(gs::integers::<u8>().max_value(3)) {
        0 => other.0.push('x'),
        1 => other.1.push('x'),
        2 => other.2 = json!({"changed": other.2}),
        _ => {
            other.3.insert("added_dep".into(), "0".repeat(64));
        }
    }
    assert_ne!(build(&other).version(), definition.version());
}

/// A definition read from storage is only as good as its digest: with
/// the source or the version altered, it does not verify.
#[hegel::test(test_cases = 300)]
fn a_stored_definition_that_was_altered_does_not_verify(tc: TestCase) {
    let definition = build(&tc.draw(content()));
    let stored = serde_json::to_value(&definition).unwrap();
    let back: Definition = serde_json::from_value(stored.clone()).unwrap();
    assert_eq!(back, definition);
    back.verify().unwrap();

    let mut altered = stored.clone();
    match tc.draw(gs::integers::<u8>().max_value(2)) {
        0 => altered["source"] = json!(format!("{}x", definition.source())),
        1 => altered["version"] = json!("0".repeat(64)),
        _ => altered["version"] = json!(definition.version().to_uppercase()),
    }
    let altered: Definition = serde_json::from_value(altered).unwrap();
    // `0`×64 could be the real version only by a 2^-256 accident; an
    // uppercase digest is never a valid version, letters or not.
    if altered != definition {
        assert!(altered.verify().is_err());
    }
}

/// Quotas: a source is taken up to 64 KiB and refused past it; a
/// definition takes up to 32 dependencies; a dependency on anything but
/// a 64-digit lowercase hex version is refused, and the error names that
/// dependency. Lengths and counts are drawn at their bounds as well as
/// across the range.
#[hegel::test(test_cases = 100)]
fn definitions_past_their_quotas_are_refused(tc: TestCase) {
    const KIB_64: usize = 64 * 1024;
    let (name, _, signatures, _) = tc.draw(content());
    let source_len: usize = tc.draw(hegel::one_of!(
        gs::sampled_from(vec![0, 1, KIB_64 - 1, KIB_64, KIB_64 + 1]),
        gs::integers::<usize>().max_value(KIB_64 + 4096),
    ));
    let sourced = Definition::new(
        name.clone(),
        "x".repeat(source_len),
        signatures.clone(),
        BTreeMap::new(),
    );
    assert_eq!(sourced.is_ok(), source_len <= KIB_64, "{source_len} bytes");

    let count: usize = tc.draw(hegel::one_of!(
        gs::sampled_from(vec![0, 1, 31, 32, 33]),
        gs::integers::<usize>().max_value(40),
    ));
    let deps: BTreeMap<String, String> = (0..count)
        .map(|i| (format!("dep{i}"), "a".repeat(64)))
        .collect();
    let counted =
        Definition::new(name.clone(), String::new(), signatures.clone(), deps);
    assert_eq!(counted.is_ok(), count <= 32, "{count} dependencies");

    // `zq` appears nowhere in the error's own words, so finding the key
    // there means the error names it.
    let key: String = tc.draw(gs::from_regex("zq[a-z_]{1,6}").fullmatch(true));
    let version: String = tc.draw(hegel::one_of!(
        gs::from_regex("[0-9a-f]{0,63}").fullmatch(true),
        gs::from_regex("[0-9a-f]{65,70}").fullmatch(true),
        gs::from_regex("[0-9a-f]{0,62}[A-F][0-9a-f]{1}").fullmatch(true),
        gs::from_regex("[0-9a-f]{63}[g-z ]").fullmatch(true),
    ));
    let bad = BTreeMap::from([(key.clone(), version.clone())]);
    let error = Definition::new(name, String::new(), signatures, bad)
        .expect_err(&version);
    assert!(error.contains(&key), "{error} for {key}");
}
