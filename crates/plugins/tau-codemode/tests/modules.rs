//! Property inventory: generated define/select histories match an independent
//! BTreeMap version/alias oracle at every resume prefix and fork boundary.
//! The generator builds valid definitions and selects from already seen
//! versions by index, so it rejects no cases and shrinks toward short runs.
//! Workspace hegel.toml supplies local and CI case counts and seed policy.

use std::collections::BTreeMap;

use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_codemode::{
    modules::{self, Definition, Library},
    store,
};

fn definition(name: &str, source: &str) -> Definition {
    Definition::new(
        name.into(),
        source.into(),
        json!({"run": {"arg": "string"}}),
        BTreeMap::new(),
    )
    .unwrap()
}

fn outer(record: modules::Record) -> Value {
    serde_json::to_value(store::Record::Module(record)).unwrap()
}

/// Signatures past their size limit are refused (names, sources and
/// dependencies are `definitions.rs`'s properties), and records that do
/// not hold a verified definition or a known version leave the selection
/// as it was.
#[test]
fn invalid_content_and_versions_cannot_change_selection() {
    assert!(
        Definition::new(
            "m".into(),
            "".into(),
            json!({"s": "x".repeat(16384)}),
            BTreeMap::new()
        )
        .is_err()
    );

    let good = definition("m", "one");
    let mut records = vec![outer(modules::Record::Define {
        definition: good.clone(),
    })];
    let mut altered = records[0].clone();
    altered["definition"]["source"] = json!("two");
    records.push(altered);
    records.push(outer(modules::Record::Select {
        name: "other".into(),
        version: good.version().into(),
    }));
    records.push(outer(modules::Record::Select {
        name: "m".into(),
        version: "A".repeat(64),
    }));
    records.push(json!({"kind":"module","op":"define","definition":42}));
    records.push(json!({"kind":"other","op":"define"}));
    let library = modules::fold(&records);
    assert_eq!(
        library.selected().get("m").map(String::as_str),
        Some(good.version())
    );
    assert_eq!(library.versions().len(), 1);
    assert_eq!(store::fold(&records), Default::default());
}

/// Past 128 versions, or past the byte quota, a definition is refused and
/// the library stays as it was.
#[test]
fn version_and_byte_quotas_refuse_without_changing_the_library() {
    let mut library = Library::default();
    for i in 0..128 {
        library
            .apply(&modules::Record::Define {
                definition: definition("m", &format!("{i}")),
            })
            .unwrap();
    }
    let before = library.clone();
    assert!(
        library
            .apply(&modules::Record::Define {
                definition: definition("m", "overflow")
            })
            .is_err()
    );
    assert_eq!(library, before);
    let one = definition("large", &"x".repeat(60_000));
    let mut bytes = Library::default();
    for i in 0..30 {
        let value = definition("large", &format!("{i}{}", one.source()));
        let result =
            bytes.apply(&modules::Record::Define { definition: value });
        if bytes
            .versions()
            .values()
            .map(|d| serde_json::to_vec(d).unwrap().len())
            .sum::<usize>()
            > modules::MAX_REGISTERED_BYTES
        {
            panic!("byte quota exceeded");
        }
        if result.is_err() {
            assert!(i >= 16);
            break;
        }
    }
    assert!(bytes.versions().len() < 30);
}

#[hegel::test]
fn generated_histories_match_the_version_and_alias_oracle(tc: TestCase) {
    let steps: Vec<(bool, u8, u8)> = tc.draw(
        gs::vecs(hegel::tuples!(
            gs::booleans(),
            gs::integers::<u8>(),
            gs::integers::<u8>()
        ))
        .max_size(20),
    );
    let mut records = Vec::new();
    let mut versions = BTreeMap::<String, Definition>::new();
    let mut aliases = BTreeMap::<String, String>::new();
    // The model's state after each prefix, the empty one first.
    let mut states = vec![(versions.clone(), aliases.clone())];
    for (define, name_index, source_index) in steps {
        let name = ["alpha", "beta", "gamma"][(name_index as usize) % 3];
        if define || versions.is_empty() {
            let source = format!("return {source_index}");
            let value = definition(name, &source);
            aliases.insert(name.into(), value.version().into());
            versions.insert(value.version().into(), value.clone());
            records.push(outer(modules::Record::Define { definition: value }));
        } else {
            let known: Vec<_> = versions.values().collect();
            let value = known[(source_index as usize) % known.len()];
            aliases.insert(value.name().into(), value.version().into());
            records.push(outer(modules::Record::Select {
                name: value.name().into(),
                version: value.version().into(),
            }));
        }
        // Every prefix is a resume point.
        let resumed = modules::fold(&records);
        assert_eq!(resumed.versions(), &versions);
        assert_eq!(resumed.selected(), &aliases);
        states.push((versions.clone(), aliases.clone()));
    }
    // A fork starts from the state at its prefix: records written after
    // the fork point in the parent never reach it, and its own records
    // land on that state.
    let at = tc.draw(gs::integers::<usize>().max_value(records.len()));
    let (mut versions, mut aliases) = states[at].clone();
    let own = definition("gamma", "return 'fork'");
    let mut forked = records[..at].to_vec();
    forked.push(outer(modules::Record::Define {
        definition: own.clone(),
    }));
    aliases.insert(own.name().into(), own.version().into());
    versions.insert(own.version().into(), own);
    let fork = modules::fold(&forked);
    assert_eq!(fork.versions(), &versions);
    assert_eq!(fork.selected(), &aliases);
}
