//! The store: JSON values a conversation's scripts keep for later
//! scripts.
//!
//! A successful script's writes become one record,
//! `{ "kind": "store", "set": { key: value }, "delete": [key] }`. Each
//! run folds the records along its fork chain, oldest first, into the
//! snapshot the script starts from. Records that do not parse are
//! skipped.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The most JSON text one stored value may take, in bytes.
pub const MAX_VALUE_BYTES: usize = 256 * 1024;

/// The most JSON text all stored values may take together, in bytes.
pub const MAX_TOTAL_BYTES: usize = 1024 * 1024;

/// The store's values, by key.
pub type Snapshot = BTreeMap<String, Value>;

/// One script's writes, in the order they apply: sets, then deletes.
/// A key is in at most one of the two.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Writes {
    pub set: BTreeMap<String, Value>,
    pub delete: Vec<String>,
}

/// What tau-codemode publishes: a script's store writes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Record {
    Store(Writes),
    Module(crate::modules::Record),
    RepositoryPin(crate::modules::RepositoryPin),
    Inference(crate::inference_trace::Record),
}

impl Writes {
    pub fn is_empty(&self) -> bool {
        self.set.is_empty() && self.delete.is_empty()
    }

    pub fn apply(&self, snapshot: &mut Snapshot) {
        for (key, value) in &self.set {
            snapshot.insert(key.clone(), value.clone());
        }
        for key in &self.delete {
            snapshot.remove(key);
        }
    }
}

/// Folds `records`, oldest first, into a snapshot. Records of another
/// shape are skipped.
pub fn fold(records: &[Value]) -> Snapshot {
    let mut snapshot = Snapshot::new();
    for record in records.iter().filter(|record| {
        matches!(
            record.get("kind").and_then(Value::as_str),
            Some("store" | "module")
        )
    }) {
        if let Some(Record::Store(writes)) =
            tau_agent::plugin::read_record(crate::PLUGIN, record)
        {
            writes.apply(&mut snapshot);
        }
    }
    snapshot
}

/// A script's view of the store: the snapshot it started from with its
/// own writes applied, and those writes.
#[derive(Debug, Clone, Default)]
pub struct Store {
    values: Snapshot,
    sizes: BTreeMap<String, usize>,
    total: usize,
    writes: Writes,
}

impl Store {
    pub fn new(snapshot: Snapshot) -> Self {
        let sizes: BTreeMap<String, usize> = snapshot
            .iter()
            .map(|(key, value)| (key.clone(), value.to_string().len()))
            .collect();
        let total = sizes.values().sum();
        Self {
            values: snapshot,
            sizes,
            total,
            writes: Writes::default(),
        }
    }

    pub fn load(&self, key: &str) -> Option<&Value> {
        self.values.get(key)
    }

    /// Keeps `value` under `key`, or removes the key for `None`.
    pub fn store(
        &mut self,
        key: &str,
        value: Option<Value>,
    ) -> Result<(), String> {
        let old = self.sizes.get(key).copied().unwrap_or(0);
        match value {
            None => {
                self.values.remove(key);
                self.sizes.remove(key);
                self.total -= old;
                self.writes.set.remove(key);
                if !self.writes.delete.iter().any(|k| k == key) {
                    self.writes.delete.push(key.to_owned());
                }
            }
            Some(value) => {
                let size = value.to_string().len();
                if size > MAX_VALUE_BYTES {
                    return Err(format!(
                        "store: the value for `{key}` is {size} bytes of \
                         JSON; one value may be at most {MAX_VALUE_BYTES}"
                    ));
                }
                let total = self.total - old + size;
                if total > MAX_TOTAL_BYTES {
                    return Err(format!(
                        "store: keeping `{key}` would make the store {total} \
                         bytes of JSON; it may be at most {MAX_TOTAL_BYTES}"
                    ));
                }
                self.total = total;
                self.sizes.insert(key.to_owned(), size);
                self.values.insert(key.to_owned(), value.clone());
                self.writes.delete.retain(|k| k != key);
                self.writes.set.insert(key.to_owned(), value);
            }
        }
        Ok(())
    }

    pub fn writes(&self) -> &Writes {
        &self.writes
    }

    pub fn into_writes(self) -> Writes {
        self.writes
    }
}
