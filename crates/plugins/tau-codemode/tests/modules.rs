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
    ui::State,
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

#[test]
fn digest_uses_canonical_content_and_every_field() {
    let first = Definition::new(
        "m".into(),
        "return 1".into(),
        json!({"z": {"b": 1, "a": 2}, "a": 3}),
        BTreeMap::new(),
    )
    .unwrap();
    let reordered = Definition::new(
        "m".into(),
        "return 1".into(),
        serde_json::from_str("{\"a\":3,\"z\":{\"a\":2,\"b\":1}}").unwrap(),
        BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(first.version(), reordered.version());
    assert_ne!(first.version(), definition("m", "return 2").version());
    assert_ne!(
        first.version(),
        Definition::new(
            "m".into(),
            "return 1".into(),
            json!({"a": 4}),
            BTreeMap::new()
        )
        .unwrap()
        .version()
    );
    let dependencies = BTreeMap::from([("dep".into(), "a".repeat(64))]);
    assert_ne!(
        first.version(),
        Definition::new(
            "m".into(),
            "return 1".into(),
            first.signatures().clone(),
            dependencies
        )
        .unwrap()
        .version()
    );
}

#[test]
fn invalid_content_and_versions_cannot_change_selection() {
    for name in ["", "../x", "a/b", "9a", "a-b", &"x".repeat(65)] {
        assert!(
            Definition::new(name.into(), "".into(), json!({}), BTreeMap::new())
                .is_err()
        );
    }
    assert!(
        Definition::new(
            "m".into(),
            "x".repeat(65537),
            json!({}),
            BTreeMap::new()
        )
        .is_err()
    );
    assert!(
        Definition::new(
            "m".into(),
            "".into(),
            json!({"s": "x".repeat(16384)}),
            BTreeMap::new()
        )
        .is_err()
    );
    assert!(
        Definition::new(
            "m".into(),
            "".into(),
            json!({}),
            BTreeMap::from([("dep".into(), "A".repeat(64))])
        )
        .is_err()
    );
    assert!(
        Definition::new(
            "m".into(),
            "".into(),
            json!({}),
            (0..33).map(|i| (format!("d{i}"), "a".repeat(64))).collect()
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

#[test]
fn old_versions_support_rollback_and_forked_prefixes() {
    let first = definition("m", "one");
    let second = definition("m", "two");
    let prefix = vec![outer(modules::Record::Define {
        definition: first.clone(),
    })];
    let mut main = prefix.clone();
    main.push(outer(modules::Record::Define {
        definition: second.clone(),
    }));
    main.push(outer(modules::Record::Select {
        name: "m".into(),
        version: first.version().into(),
    }));
    let mut fork = prefix.clone();
    fork.push(outer(modules::Record::Define {
        definition: definition("m", "fork"),
    }));
    assert_eq!(modules::fold(&prefix).selected()["m"], first.version());
    assert_eq!(modules::fold(&main).selected()["m"], first.version());
    assert_eq!(modules::fold(&main).versions().len(), 2);
    assert_ne!(
        modules::fold(&fork).selected()["m"],
        modules::fold(&main).selected()["m"]
    );
}

#[test]
fn quotas_and_json_roundtrip() {
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
    assert_eq!(
        serde_json::from_value::<Library>(
            serde_json::to_value(&library).unwrap()
        )
        .unwrap(),
        library
    );
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
    let state = State {
        modules: library.clone(),
        ..State::default()
    };
    assert_eq!(
        serde_json::from_value::<State>(serde_json::to_value(state).unwrap())
            .unwrap()
            .modules,
        library
    );
    assert_eq!(
        serde_json::from_value::<State>(json!({})).unwrap().modules,
        Library::default()
    );
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
        // Every prefix is a resume point; a fork starts from precisely this state.
        let resumed = modules::fold(&records);
        assert_eq!(resumed.versions(), &versions);
        assert_eq!(resumed.selected(), &aliases);
        assert_eq!(modules::fold(&records[..records.len()]), resumed);
    }
}
