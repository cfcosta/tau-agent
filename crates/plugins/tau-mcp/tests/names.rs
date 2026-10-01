//! Tool names, as properties (`docs/reference/mcp.md`, "Tests"): every
//! name matches `^[A-Za-z0-9_]{1,64}$`; distinct (server, tool) pairs get
//! distinct names; the names do not depend on the order tools are
//! listed.

use std::collections::{BTreeMap, BTreeSet};

use hegel::{TestCase, generators as gs};
use tau_mcp::names::{base_name, namespace, tool_names};

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

/// A pair whose plain name is short and shared by no other keeps it.
#[hegel::test(test_cases = 300)]
fn unshared_short_names_stay_plain(tc: TestCase) {
    let pairs = tc.draw(pairs());
    let names = names_of(&pairs);
    for ((server, tool), name) in pairs.iter().zip(&names) {
        let base = base_name(server, tool);
        let shared = pairs
            .iter()
            .any(|(s, t)| (s, t) != (server, tool) && base_name(s, t) == base);
        let taken = names.iter().any(|other| *other == base && other != name);
        if base.len() <= 64 && !shared && !taken {
            assert_eq!(*name, base);
        }
    }
}

#[test]
fn namespaces_join_dashes_and_underscores() {
    assert_eq!(namespace("dev-radius"), namespace("dev_radius"));
    assert_eq!(namespace("dev-radius"), "mcp__dev_radius");
}
