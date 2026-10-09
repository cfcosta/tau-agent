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

/// Valid names, and the ways a name goes wrong: empty, too long, a
/// digit first, a character outside the identifier set, a non-ASCII
/// letter.
#[hegel::composite]
fn name(tc: &TestCase) -> String {
    let valid =
        tc.draw(gs::from_regex("[A-Za-z_][A-Za-z0-9_]{0,63}").fullmatch(true));
    match tc.draw(gs::integers::<u8>().max_value(6)) {
        0 => String::new(),
        1 => format!("{valid}{valid}"),
        2 => format!(
            "{}{valid}",
            tc.draw(gs::from_regex("[0-9]").fullmatch(true))
        ),
        3 => format!(
            "{valid}{}",
            tc.draw(gs::sampled_from(vec!["-", " ", ".", "é", "雪", "/"]))
        ),
        4 => format!("é{valid}"),
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

/// Quotas: a source past 64 KiB or more than 32 dependencies is
/// refused, and one dependency on a bad version names it.
#[hegel::test(test_cases = 50)]
fn definitions_past_their_quotas_are_refused(tc: TestCase) {
    let (name, _, signatures, _) = tc.draw(content());
    let big = "x".repeat(64 * 1024 + 1);
    assert!(
        Definition::new(name.clone(), big, signatures.clone(), BTreeMap::new())
            .is_err()
    );
    let exactly = "x".repeat(64 * 1024);
    assert!(
        Definition::new(
            name.clone(),
            exactly,
            signatures.clone(),
            BTreeMap::new()
        )
        .is_ok()
    );

    let deps = |n: usize| -> BTreeMap<String, String> {
        (0..n)
            .map(|i| (format!("dep{i}"), "a".repeat(64)))
            .collect()
    };
    assert!(
        Definition::new(
            name.clone(),
            String::new(),
            signatures.clone(),
            deps(32)
        )
        .is_ok()
    );
    assert!(
        Definition::new(
            name.clone(),
            String::new(),
            signatures.clone(),
            deps(33)
        )
        .is_err()
    );
    let bad = BTreeMap::from([("dep".to_owned(), "short".to_owned())]);
    let error =
        Definition::new(name, String::new(), signatures, bad).unwrap_err();
    assert!(error.contains("dep"), "{error}");
}
