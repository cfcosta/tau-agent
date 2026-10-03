//! Tool names, as properties (`docs/reference/mcp.md`, "Tests"): every
//! name matches `^[A-Za-z0-9_]{1,64}$`; distinct (server, tool) pairs get
//! distinct names; the names do not depend on the order tools are
//! listed.

use std::collections::{BTreeMap, BTreeSet};

use hegel::{TestCase, generators as gs};
use sha2::{Digest, Sha256};
use tau_mcp::names::{namespace, tool_names};

/// Pairs that often collide once sanitized or cut: short names over a
/// small alphabet with separators, and some long ones.
#[hegel::composite]
fn pairs(tc: &TestCase) -> Vec<(String, String)> {
    let pairs: BTreeSet<(String, String)> = tc.draw(
        gs::btree_sets(hegel::tuples!(
            gs::from_regex("[ab_-]{1,3}|[a-z0-9_-]{50,70}"),
            gs::from_regex("[ab_.]{1,4}|[a-zé/ ._]{0,70}"),
        ))
        .max_size(12),
    );
    pairs.into_iter().collect()
}

#[hegel::composite]
fn short_ascii_component(tc: &TestCase) -> String {
    tc.draw(gs::from_regex("[A-Za-z0-9]{1,8}"))
}

/// Property inventory for the constructed-name laws below:
/// - `short_ascii_pairs_keep_exact_plain_names`: exact-format oracle; one
///   generated server/tool pair of 1..8 ASCII alphanumerics, shrinking toward
///   shorter valid components.
/// - `colliding_sanitized_bases_get_full_hash_names`: independent sanitizer and
///   SHA-256 oracle and a known sanitized literal; exactly two fixed servers
///   and one generated short tool.
/// - `names_at_63_and_64_bytes_stay_plain_while_65_bytes_is_hashed`: explicit
///   expected strings for three ASCII inputs at the length boundary.
/// - `unicode_and_punctuation_map_to_the_known_plain_name`: literal output
///   oracle for one isolated pair containing known sanitizer replacements.
fn names_of(pairs: &[(String, String)]) -> Vec<String> {
    let refs: Vec<(&str, &str)> = pairs
        .iter()
        .map(|(server, tool)| (server.as_str(), tool.as_str()))
        .collect();
    tool_names(&refs)
}

#[hegel::test(test_cases = 500)]
fn every_name_is_a_valid_identifier(tc: TestCase) {
    let pairs = tc.draw(pairs());
    for name in names_of(&pairs) {
        assert!(
            !name.is_empty()
                && name.len() <= 64
                && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'),
            "{name}"
        );
    }
}

#[hegel::test(test_cases = 500)]
fn distinct_pairs_get_distinct_names(tc: TestCase) {
    let pairs = tc.draw(pairs());
    let names = names_of(&pairs);
    let unique: BTreeSet<&String> = names.iter().collect();
    assert_eq!(unique.len(), pairs.len(), "{names:?}");
}

#[hegel::test(test_cases = 500)]
fn names_do_not_depend_on_order(tc: TestCase) {
    let pairs = tc.draw(pairs());
    let shuffled = tc.draw(gs::permutations(pairs.clone()));
    let by_pair =
        |pairs: &[(String, String)]| -> BTreeMap<(String, String), String> {
            pairs.iter().cloned().zip(names_of(pairs)).collect()
        };
    assert_eq!(by_pair(&pairs), by_pair(&shuffled));
}

#[hegel::test(test_cases = 300)]
fn short_ascii_pairs_keep_exact_plain_names(tc: TestCase) {
    let server = tc.draw(short_ascii_component());
    let tool = tc.draw(short_ascii_component());
    let names = names_of(&[(server.clone(), tool.clone())]);

    assert_eq!(names, [format!("mcp__{server}__{tool}")]);
}

fn expected_digest_prefix(server: &str, tool: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(server.as_bytes());
    hasher.update([0]);
    hasher.update(tool.as_bytes());
    let digest = hasher.finalize();

    format!(
        "{:02x}{:02x}{:02x}{:02x}",
        digest[0], digest[1], digest[2], digest[3]
    )
}

#[hegel::test(test_cases = 300)]
fn colliding_sanitized_bases_get_full_hash_names(tc: TestCase) {
    let tool = tc.draw(short_ascii_component());
    let pairs = [("a-b", tool.as_str()), ("a_b", tool.as_str())];
    // The generated tool needs no sanitization; these two server spellings
    // have the independently known sanitized form `a_b`.
    let expected_base = format!("mcp__a_b__{tool}");
    let expected = [
        format!("{expected_base}_{}", expected_digest_prefix("a-b", &tool)),
        format!("{expected_base}_{}", expected_digest_prefix("a_b", &tool)),
    ];
    assert_eq!(tool_names(&pairs), expected);
}

#[test]
fn names_at_63_and_64_bytes_stay_plain_while_65_bytes_is_hashed() {
    let pairs = [
        ("s".to_owned(), "t".repeat(55)),
        ("s".to_owned(), "t".repeat(56)),
        ("s".to_owned(), "t".repeat(57)),
    ];
    let base_63 = format!("mcp__s__{}", "t".repeat(55));
    let base_64 = format!("mcp__s__{}", "t".repeat(56));
    let base_65 = format!("mcp__s__{}", "t".repeat(57));
    let expected_65 = format!(
        "{}_{}",
        &base_65[..55],
        expected_digest_prefix("s", &pairs[2].1)
    );

    assert_eq!([base_63.len(), base_64.len(), base_65.len()], [63, 64, 65]);
    assert_eq!(names_of(&pairs), [base_63, base_64, expected_65]);
}

#[test]
fn unicode_and_punctuation_map_to_the_known_plain_name() {
    // The server characters a, é, -, ! sanitize to a___; x, ø, ., / sanitize
    // to x___. Construct those inputs only after recording the expected maps.
    let server: String = ['a', 'é', '-', '!'].into_iter().collect();
    let tool: String = ['x', 'ø', '.', '/'].into_iter().collect();

    assert_eq!(names_of(&[(server, tool)]), ["mcp__a_____x___"]);
}

#[test]
fn namespaces_join_dashes_and_underscores() {
    assert_eq!(namespace("dev-radius"), namespace("dev_radius"));
    assert_eq!(namespace("dev-radius"), "mcp__dev_radius");
}
