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

fn pin_record(owner: &str, definition: &Definition) -> Value {
    let pin = RepositoryPin {
        owner: owner.to_owned(),
        selected: BTreeMap::from([(
            definition.name().to_owned(),
            definition.version().to_owned(),
        )]),
        versions: BTreeMap::from([(
            definition.version().to_owned(),
            definition.clone(),
        )]),
    };
    serde_json::to_value(store::Record::RepositoryPin(pin)).unwrap()
}

/// A run's pin is the one its records hold. A later record for the same
/// owner repeating that pin is a resume and changes nothing; a later
/// record holding a different pin is a conflict, and the lookup for that
/// owner fails, while every other owner's lookup is unaffected. The
/// model is a table of each owner's pin and whether it conflicted.
#[hegel::test]
fn owned_pin_follows_every_record_prefix(tc: TestCase) {
    let steps: Vec<(u8, u8, bool)> = tc.draw(
        gs::vecs(hegel::tuples!(
            gs::integers::<u8>(),
            gs::integers::<u8>(),
            gs::weighted_booleans(0.2)
        ))
        .max_size(24),
    );
    let candidates = [
        definition("alpha", 0),
        definition("alpha", 1),
        definition("beta", 2),
    ];
    let mut records = Vec::<Value>::new();
    // Owner to (index of its pinned candidate, whether it conflicted).
    let mut expected = BTreeMap::<String, (usize, bool)>::new();
    for (owner_index, selection_index, conflict) in steps {
        let owner = format!("run-{}", owner_index % 4);
        let chosen = match expected.get_mut(&owner) {
            None => {
                let index = selection_index as usize % candidates.len();
                expected.insert(owner.clone(), (index, false));
                index
            }
            Some((pinned, conflicted)) if conflict => {
                // Any candidate but the pinned one: a different pin.
                let offset = 1 + selection_index as usize % 2;
                *conflicted = true;
                (*pinned + offset) % candidates.len()
            }
            Some((pinned, _)) => *pinned,
        };
        records.push(pin_record(&owner, &candidates[chosen]));
        for (owner, (pinned, conflicted)) in &expected {
            let looked_up = modules::pin_for_run(&records, owner);
            if *conflicted {
                assert!(
                    looked_up
                        .as_ref()
                        .is_err_and(|error| error
                            .contains("conflicting repository pins")),
                    "{owner}: {looked_up:?}"
                );
                continue;
            }
            let actual = looked_up.unwrap().unwrap();
            let definition = &candidates[*pinned];
            assert_eq!(actual.owner, *owner);
            assert_eq!(
                actual.selected,
                BTreeMap::from([(
                    definition.name().to_owned(),
                    definition.version().to_owned()
                )])
            );
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

#[test]
fn missing_or_empty_pin_owner_fails_closed_on_resume() {
    let definition = definition("alpha", 1);
    let pin = RepositoryPin {
        owner: "run".into(),
        selected: BTreeMap::from([(
            "alpha".into(),
            definition.version().into(),
        )]),
        versions: BTreeMap::from([(definition.version().into(), definition)]),
    };
    let original =
        serde_json::to_value(store::Record::RepositoryPin(pin)).unwrap();
    for malformed in [None, Some(json!("")), Some(json!(7))] {
        let mut record = original.clone();
        match malformed {
            None => {
                record.as_object_mut().unwrap().remove("owner");
            }
            Some(owner) => {
                record["owner"] = owner;
            }
        }
        assert!(modules::pin_for_run(&[record.clone()], "run").is_err());
        assert!(modules::pin_for_run(&[record], "fresh-fork").is_err());
    }
    assert!(
        modules::pin_for_run(&[original], "fresh-fork")
            .unwrap()
            .is_none()
    );
}
