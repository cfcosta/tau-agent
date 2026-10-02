//! Property inventory: run-owner pin lookup matches an independent record-prefix
//! table; canonical digest matches direct SHA-256 of the specified JSON tuple.
//! Generator plan: small indexed definitions and owner sequences are valid by
//! construction. Vec length and byte indices shrink toward short histories and
//! simple selections. Workspace hegel.toml supplies development and CI counts.
//! On CI Hegel uses its shipped CI profile: deterministic cases, no example
//! database, and the default TooSlow suppression.

use std::collections::BTreeMap;

use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tau_codemode::{
    modules::{self, Definition, RepositoryPin},
    store,
};

fn definition(name: &str, number: u8) -> Definition {
    Definition::new(
        name.into(),
        format!("return {{ n = {number} }}"),
        json!({"call": {"z": number, "a": true}}),
        BTreeMap::new(),
    )
    .unwrap()
}

#[hegel::test]
fn owned_pin_follows_every_record_prefix(tc: TestCase) {
    let selections: Vec<(u8, u8)> = tc.draw(
        gs::vecs(hegel::tuples!(gs::integers::<u8>(), gs::integers::<u8>()))
            .max_size(24),
    );
    let candidates = [
        definition("alpha", 0),
        definition("alpha", 1),
        definition("beta", 2),
    ];
    let mut records = Vec::<Value>::new();
    let mut expected = BTreeMap::<String, (String, String)>::new();
    for (owner_index, selection_index) in selections {
        let owner = format!("run-{}", owner_index % 4);
        let selected = &candidates[selection_index as usize % candidates.len()];
        // A run's first pin wins. Later records for the same owner use the
        // same immutable pin; this models a resume, not a new activation.
        let (name, version) = expected
            .entry(owner.clone())
            .or_insert_with(|| {
                (selected.name().to_owned(), selected.version().to_owned())
            })
            .clone();
        let definition = candidates
            .iter()
            .find(|item| item.version() == version)
            .unwrap();
        let pin = RepositoryPin {
            owner: owner.clone(),
            selected: BTreeMap::from([(name, version.clone())]),
            versions: BTreeMap::from([(version, definition.clone())]),
        };
        records.push(
            serde_json::to_value(store::Record::RepositoryPin(pin)).unwrap(),
        );
        for (owner, (name, version)) in &expected {
            let actual =
                modules::pin_for_run(&records, owner).unwrap().unwrap();
            assert_eq!(actual.selected.get(name), Some(version));
            assert_eq!(actual.owner, *owner);
        }
        assert!(
            modules::pin_for_run(&records, "not-a-run")
                .unwrap()
                .is_none()
        );
    }
}

#[hegel::test]
fn definition_digest_matches_independent_canonical_hash(tc: TestCase) {
    let source_number: u8 = tc.draw(gs::integers());
    let signature_number: u8 = tc.draw(gs::integers());
    let dependency_number: u8 = tc.draw(gs::integers());
    let name = "entry";
    let source = format!("return {{ n = {source_number} }}");
    let signatures = json!({"call": {"z": signature_number, "a": true}});
    let dependency = definition("base", dependency_number);
    let dependencies =
        BTreeMap::from([("base".to_owned(), dependency.version().to_owned())]);
    let actual = Definition::new(
        name.into(),
        source.clone(),
        signatures.clone(),
        dependencies.clone(),
    )
    .unwrap();
    // Independent oracle: specify the known sorted nested object directly,
    // without using the production signature normalization routine.
    let sorted_signatures = BTreeMap::from([(
        "call",
        BTreeMap::from([("a", json!(true)), ("z", json!(signature_number))]),
    )]);
    let canonical =
        serde_json::to_vec(&(name, &source, &sorted_signatures, &dependencies))
            .unwrap();
    let expected: String = Sha256::digest(canonical)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(actual.version(), expected);
    let pin = RepositoryPin {
        owner: "run".into(),
        selected: BTreeMap::from([(name.into(), actual.version().into())]),
        versions: BTreeMap::from([
            (actual.version().into(), actual.clone()),
            (dependency.version().into(), dependency),
        ]),
    };
    pin.verify().unwrap();
    let mut tampered = serde_json::to_value(&pin).unwrap();
    tampered["versions"][actual.version()]["source"] = json!("return 999");
    let corrupt: RepositoryPin = serde_json::from_value(tampered).unwrap();
    assert!(corrupt.verify().is_err());
}

#[test]
fn corrupt_owned_pin_fails_without_falling_back_to_inherited_pin() {
    let definition = definition("alpha", 1);
    let parent = RepositoryPin {
        owner: "parent".into(),
        selected: BTreeMap::from([(
            "alpha".into(),
            definition.version().into(),
        )]),
        versions: BTreeMap::from([(
            definition.version().into(),
            definition.clone(),
        )]),
    };
    let mut child = parent.clone();
    child.owner = "child".into();
    let mut corrupt =
        serde_json::to_value(store::Record::RepositoryPin(child)).unwrap();
    corrupt["versions"][definition.version()]["source"] = json!("return 999");
    let records = [
        serde_json::to_value(store::Record::RepositoryPin(parent)).unwrap(),
        corrupt,
    ];
    assert!(modules::pin_for_run(&records, "child").is_err());
}
