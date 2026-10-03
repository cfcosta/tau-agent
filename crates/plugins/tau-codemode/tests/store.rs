//! The store's records.
//!
//! | Property | Oracle |
//! | --- | --- |
//! | store and UI folds match applying script operations to a map at every prefix | model |
//! | records of another shape are skipped without increasing UI write count | model |

use std::collections::BTreeMap;

use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_codemode::{
    CodemodeUi,
    PLUGIN,
    store::{self, Store, Writes},
    ui::State,
};

/// `writes` as the record tau-codemode publishes.
fn record(writes: &Writes) -> Value {
    serde_json::to_value(store::Record::Store(writes.clone())).unwrap()
}

#[derive(Debug, Clone)]
enum Op {
    Set(String, i64),
    Delete(String),
}
hegel::pretty_print_as_debug!(Op);

#[hegel::composite]
fn op(tc: &TestCase) -> Op {
    let key = tc
        .draw(gs::sampled_from(vec!["a", "b", "c", "d"]))
        .to_owned();
    if tc.draw(gs::booleans()) {
        Op::Set(key, tc.draw(gs::integers::<i64>()))
    } else {
        Op::Delete(key)
    }
}

fn assert_store_prefix_matches_model(
    records: &[Value],
    ui_value: &tau_ui_plugin::PluginValue,
    model: &BTreeMap<String, Value>,
    valid_store_records: usize,
) {
    let folded = store::fold(records);
    assert_eq!(&folded, model);

    let state = ui_value.get::<State>();
    assert_eq!(&state.store, model);
    assert_eq!(state.writes, valid_store_records);
}

// Property inventory: `store::fold` and the registered UI fold must match an
// independently updated BTreeMap after every batch and junk-record prefix.
// Generated input has at most 6 batches of at most 6 operations, keys from a
// 4-key alphabet, i64 values, and 3 strings of at most 16 characters wrapped as
// unknown-kind records; shrinking removes generated batches, operations, and text
// while retaining the fixed absent-delete, same-key, and empty-batch examples.
#[hegel::test(test_cases = 200)]
fn store_and_ui_folds_match_the_operation_model_at_each_prefix(tc: TestCase) {
    let generated_batches: Vec<Vec<Op>> =
        tc.draw(gs::vecs(gs::vecs(op()).max_size(6)).max_size(6));
    let junk: Vec<String> =
        tc.draw(gs::vecs(gs::text().max_size(16)).max_size(3));
    let mut batches = vec![
        vec![Op::Delete("absent".into()), Op::Set("same".into(), 1)],
        vec![Op::Set("same".into(), 2), Op::Delete("same".into())],
        vec![],
    ];
    batches.extend(generated_batches);

    let mut model: BTreeMap<String, Value> = BTreeMap::new();
    let mut records = Vec::new();
    let registry = tau_ui_plugin::Registry::new().with(CodemodeUi);
    let plugin = registry.get(PLUGIN).unwrap();
    let mut ui_value = tau_ui_plugin::PluginValue::default();
    let mut valid_store_records = 0;

    for (i, batch) in batches.iter().enumerate() {
        // Each batch is one successful script.
        let mut store = Store::new(model.clone());
        for op in batch {
            match op {
                Op::Set(key, n) => {
                    store.store(key, Some(json!(n))).unwrap();
                    model.insert(key.clone(), json!(n));
                }
                Op::Delete(key) => {
                    store.store(key, None).unwrap();
                    model.remove(key);
                }
            }
        }
        let body = record(store.writes());
        records.push(body.clone());
        plugin.apply(
            &mut ui_value,
            &body,
            &mut tau_ui_plugin::testing::FakeRun::default(),
        );
        valid_store_records += 1;
        assert_store_prefix_matches_model(
            &records,
            &ui_value,
            &model,
            valid_store_records,
        );

        // A record of another shape lands after each batch and changes
        // neither the independent model nor the UI's valid Store write count.
        let junk_record = json!({"kind": "other"});
        records.push(junk_record.clone());
        plugin.apply(
            &mut ui_value,
            &junk_record,
            &mut tau_ui_plugin::testing::FakeRun::default(),
        );
        assert_store_prefix_matches_model(
            &records,
            &ui_value,
            &model,
            valid_store_records,
        );

        if let Some(value) = junk.get(i) {
            let junk_record = json!({"kind": "other", "value": value});
            records.push(junk_record.clone());
            plugin.apply(
                &mut ui_value,
                &junk_record,
                &mut tau_ui_plugin::testing::FakeRun::default(),
            );
            assert_store_prefix_matches_model(
                &records,
                &ui_value,
                &model,
                valid_store_records,
            );
        }
    }
}

#[test]
fn malformed_store_records_are_skipped() {
    let records = [
        json!({ "kind": "store", "set": { "a": 1 } }),
        json!({ "kind": "store", "set": 3 }),
        json!({ "kind": "store", "delete": [1] }),
        json!("text"),
        json!({ "kind": "store", "set": { "b": 2 }, "delete": ["a"] }),
    ];
    let registry = tau_ui_plugin::Registry::new().with(CodemodeUi);
    let plugin = registry.get(PLUGIN).unwrap();
    let mut ui_value = tau_ui_plugin::PluginValue::default();
    let a = BTreeMap::from([("a".to_owned(), json!(1))]);
    let b = BTreeMap::from([("b".to_owned(), json!(2))]);
    for (index, body) in records.iter().enumerate() {
        plugin.apply(
            &mut ui_value,
            body,
            &mut tau_ui_plugin::testing::FakeRun::default(),
        );
        assert_store_prefix_matches_model(
            &records[..=index],
            &ui_value,
            if index == 4 { &b } else { &a },
            if index == 4 { 2 } else { 1 },
        );
    }
}
