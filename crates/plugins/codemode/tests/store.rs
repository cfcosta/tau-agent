//! The store's records.
//!
//! | Property | Oracle |
//! | --- | --- |
//! | folding records equals applying each script's writes to a map | model |
//! | records of another shape are skipped | model |

use std::collections::BTreeMap;

use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_codemode::store::{self, Store, Writes};

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

#[hegel::test]
fn store_fold_equals_applying_the_writes(tc: TestCase) {
    let batches: Vec<Vec<Op>> =
        tc.draw(gs::vecs(gs::vecs(op()).max_size(6)).max_size(6));
    let junk: Vec<Value> =
        tc.draw(gs::vecs(hegel::extras::serde_json::values()).max_size(3));
    let mut model: BTreeMap<String, Value> = BTreeMap::new();
    let mut records = Vec::new();
    for (i, batch) in batches.iter().enumerate() {
        // Each batch is one successful script.
        let mut store = Store::new(store::fold(&records));
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
        records.push(store.writes().to_record());
        // Records of another shape land anywhere and change nothing.
        if let Some(junk) = junk.get(i)
            && Writes::from_record(junk).is_none()
        {
            records.push(junk.clone());
        }
    }
    assert_eq!(store::fold(&records), model);
}

#[test]
fn malformed_store_records_are_skipped() {
    let records = [
        json!({ "store": { "set": { "a": 1 } } }),
        json!({ "store": { "set": 3 } }),
        json!({ "store": { "delete": [1] } }),
        json!("text"),
        json!({ "store": { "set": { "b": 2 }, "delete": ["a"] } }),
    ];
    assert_eq!(store::fold(&records), [("b".to_owned(), json!(2))].into());
}
