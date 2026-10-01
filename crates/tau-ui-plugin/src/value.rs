//! A plugin's value with its type erased: its state in a run, its data,
//! its settings. The interface holds it as the plugin's own type, and
//! turns it into JSON only to send or store it.

use std::{
    any::{Any, type_name},
    collections::BTreeSet,
    fmt,
    sync::{Arc, Mutex, OnceLock},
};

use serde::{
    Deserialize,
    Deserializer,
    Serialize,
    Serializer,
    de::DeserializeOwned,
};
use serde_json::Value;

/// A plugin's value: typed once a plugin reads it, JSON while it
/// travels. Clones share the typed value until one changes it.
#[derive(Clone, Default)]
pub struct PluginValue {
    json: OnceLock<Value>,
    typed: OnceLock<Typed>,
}

#[derive(Clone)]
struct Typed {
    value: Arc<dyn Any + Send + Sync>,
    encode: fn(&(dyn Any + Send + Sync)) -> Value,
}

fn encode<T: Serialize + 'static>(value: &(dyn Any + Send + Sync)) -> Value {
    let value = value
        .downcast_ref::<T>()
        .expect("a typed value encodes as its own type");
    serde_json::to_value(value).unwrap_or_default()
}

impl Typed {
    fn new<T: Serialize + Send + Sync + 'static>(value: T) -> Self {
        Self {
            value: Arc::new(value),
            encode: encode::<T>,
        }
    }
}

/// `value` as a `T`, or `T`'s default when it is not one, said once per
/// type: a value the plugin can no longer read must not blank the rest.
pub(crate) fn decode<T: DeserializeOwned + Default>(value: &Value) -> T {
    match serde_json::from_value(value.clone()) {
        Ok(typed) => typed,
        Err(_) if value.is_null() => T::default(),
        Err(error) => {
            static LOGGED: Mutex<BTreeSet<&'static str>> =
                Mutex::new(BTreeSet::new());
            if LOGGED
                .lock()
                .expect("not poisoned")
                .insert(type_name::<T>())
            {
                eprintln!(
                    "tau-ui-plugin: a {} could not be read, so it is \
                     empty: {error}",
                    type_name::<T>()
                );
            }
            T::default()
        }
    }
}

impl PluginValue {
    pub fn from_json(json: Value) -> Self {
        Self {
            json: OnceLock::from(json),
            typed: OnceLock::new(),
        }
    }

    pub fn typed<T: Serialize + Send + Sync + 'static>(value: T) -> Self {
        Self {
            json: OnceLock::new(),
            typed: OnceLock::from(Typed::new(value)),
        }
    }

    /// The value as JSON, as it travels.
    pub fn json(&self) -> &Value {
        self.json.get_or_init(|| {
            let typed = self.typed.get().expect("a value is typed or JSON");
            (typed.encode)(&*typed.value)
        })
    }

    /// Whether nothing was ever set.
    pub fn is_null(&self) -> bool {
        self.typed.get().is_none() && self.json.get().is_none_or(Value::is_null)
    }

    /// The value as `T`, read from its JSON the first time.
    pub fn get<T>(&self) -> &T
    where
        T: Serialize + DeserializeOwned + Default + Send + Sync + 'static,
    {
        let typed = self.typed.get_or_init(|| {
            Typed::new(decode::<T>(self.json.get().unwrap_or(&Value::Null)))
        });
        typed
            .value
            .downcast_ref()
            .expect("a plugin's value is always its own type")
    }

    /// The value as `T`, to change. Clones holding it keep the old one.
    pub fn get_mut<T>(&mut self) -> &mut T
    where
        T: Serialize
            + DeserializeOwned
            + Default
            + Clone
            + Send
            + Sync
            + 'static,
    {
        self.get::<T>();
        self.json.take();
        let typed = self.typed.get_mut().expect("read just now");
        if Arc::get_mut(&mut typed.value).is_none() {
            let copy: T = typed
                .value
                .downcast_ref::<T>()
                .expect("a plugin's value is always its own type")
                .clone();
            typed.value = Arc::new(copy);
        }
        Arc::get_mut(&mut typed.value)
            .expect("unshared just now")
            .downcast_mut()
            .expect("a plugin's value is always its own type")
    }
}

impl From<Value> for PluginValue {
    fn from(json: Value) -> Self {
        Self::from_json(json)
    }
}

impl PartialEq for PluginValue {
    fn eq(&self, other: &Self) -> bool {
        self.json() == other.json()
    }
}

impl fmt::Debug for PluginValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.json(), f)
    }
}

impl Serialize for PluginValue {
    fn serialize<S: Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        self.json().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PluginValue {
    fn deserialize<D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        Value::deserialize(deserializer).map(Self::from_json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A value read as its type, changed, and sent reads back the same;
    /// a clone taken before the change keeps the old value.
    #[hegel::test(test_cases = 100)]
    fn a_value_changes_alone_and_travels_whole(tc: hegel::TestCase) {
        use hegel::generators as gs;
        let first: Vec<u32> =
            tc.draw(gs::vecs(gs::integers::<u32>()).max_size(5));
        let more: u32 = tc.draw(gs::integers());
        let from_json: bool = tc.draw(gs::booleans());
        let mut value = if from_json {
            PluginValue::from_json(serde_json::to_value(&first).unwrap())
        } else {
            PluginValue::typed(first.clone())
        };
        let before = value.clone();
        value.get_mut::<Vec<u32>>().push(more);
        let mut expected = first.clone();
        expected.push(more);
        assert_eq!(value.get::<Vec<u32>>(), &expected);
        assert_eq!(before.get::<Vec<u32>>(), &first);
        let sent: PluginValue =
            serde_json::from_str(&serde_json::to_string(&value).unwrap())
                .unwrap();
        assert_eq!(sent.get::<Vec<u32>>(), &expected);
        assert_eq!(sent, value);
    }

    /// JSON a plugin cannot read is its type's default, not an error.
    #[test]
    fn unreadable_json_is_the_default() {
        let value =
            PluginValue::from_json(serde_json::json!({"not": "a list"}));
        assert!(value.get::<Vec<u32>>().is_empty());
        assert!(PluginValue::default().is_null());
    }
}
