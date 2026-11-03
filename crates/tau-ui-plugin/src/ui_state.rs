//! The interface's state for plugins, as data (ADR 0045): which section
//! is open, the tab chosen, a draft. It changes only by [`UiEvent`]s,
//! which every interface of one tau applies alike, so the computer and
//! its phones show the same, and the host saves it across restarts.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Every plugin's UI state: by plugin, a value for each key the plugin
/// names.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UiState(BTreeMap<String, BTreeMap<String, Value>>);

/// A change to one key of a plugin's UI state: a new value, or none to
/// forget it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiEvent {
    pub plugin: String,
    pub key: String,
    pub value: Option<Value>,
}

/// No keys, for a plugin that has none.
static NONE: BTreeMap<String, Value> = BTreeMap::new();

impl UiState {
    /// Applies `event`. A key set to `null` or to nothing is forgotten,
    /// and a plugin left with no keys is too, so two states that hold
    /// the same values are equal however they got there.
    pub fn apply(&mut self, event: &UiEvent) {
        match event.value.clone().filter(|value| !value.is_null()) {
            Some(value) => {
                self.0
                    .entry(event.plugin.clone())
                    .or_default()
                    .insert(event.key.clone(), value);
            }
            None => {
                if let Some(keys) = self.0.get_mut(&event.plugin) {
                    keys.remove(&event.key);
                    if keys.is_empty() {
                        self.0.remove(&event.plugin);
                    }
                }
            }
        }
    }

    /// `plugin`'s keys.
    pub fn of(&self, plugin: &str) -> &BTreeMap<String, Value> {
        self.0.get(plugin).unwrap_or(&NONE)
    }

    pub fn get(&self, plugin: &str, key: &str) -> Option<&Value> {
        self.of(plugin).get(key)
    }
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};
    use serde_json::json;

    use super::*;

    fn event(tc: &TestCase) -> UiEvent {
        let value = match tc.draw(gs::integers::<u8>().max_value(3)) {
            0 => None,
            1 => Some(Value::Null),
            2 => Some(json!(tc.draw(gs::integers::<i8>()))),
            _ => Some(json!(
                tc.draw(gs::from_regex("[a-c]{0,2}").fullmatch(true))
            )),
        };
        UiEvent {
            plugin: tc
                .draw(gs::sampled_from(vec!["a".to_owned(), "b".to_owned()])),
            key: tc
                .draw(gs::sampled_from(vec!["x".to_owned(), "y".to_owned()])),
            value,
        }
    }

    /// The state is what the last event for each key said, and nothing
    /// more: it equals the state made from those last values alone, and
    /// reads back from JSON the same.
    #[hegel::test(test_cases = 200)]
    fn the_last_value_of_each_key_is_the_state(tc: TestCase) {
        let events: Vec<UiEvent> = (0..tc
            .draw(gs::integers::<usize>().max_value(12)))
            .map(|_| event(&tc))
            .collect();
        let mut state = UiState::default();
        for event in &events {
            state.apply(event);
        }
        let mut last: BTreeMap<(String, String), &UiEvent> = BTreeMap::new();
        for event in &events {
            last.insert((event.plugin.clone(), event.key.clone()), event);
        }
        let mut expected = UiState::default();
        for event in last.values() {
            expected.apply(event);
        }
        assert_eq!(state, expected);
        for ((plugin, key), event) in &last {
            assert_eq!(
                state.get(plugin, key),
                event.value.as_ref().filter(|value| !value.is_null())
            );
        }
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(serde_json::from_value::<UiState>(json).unwrap(), state);
    }

    /// Forgetting every key leaves the empty state.
    #[test]
    fn forgetting_leaves_nothing() {
        let mut state = UiState::default();
        let set = |value| UiEvent {
            plugin: "p".into(),
            key: "open".into(),
            value,
        };
        state.apply(&set(Some(json!(true))));
        assert_eq!(state.get("p", "open"), Some(&json!(true)));
        state.apply(&set(None));
        assert_eq!(state, UiState::default());
    }
}
