//! An agent's polls (ADR 0037): a tool or an HTTP request, made every
//! interval, that fires once per item whose key it has not seen.
//!
//! A poll keeps the keys it saw, the last [`KEYS`]. Its first poll only
//! records them, so a new poll does not fire once for everything already
//! there. Every poll runs at once when tau starts. One that fails fires
//! nothing and says why on the agent's page; after [`BACK_OFF_AFTER`]
//! failures in a row it waits four times as long, at most an hour.

use std::collections::{BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tau_agents::{
    TriggerKind,
    event::{Event, Kind},
};
use tau_tools_host::{http, secrets::SecretStore};
use tokio_util::sync::CancellationToken;

use crate::schedule::{self, SHORTEST};

/// How many keys a poll keeps: the oldest go first.
pub const KEYS: usize = 10_000;

/// How many failures in a row make a poll wait longer.
pub const BACK_OFF_AFTER: u32 = 3;

/// The longest a poll that keeps failing waits, unless its own interval
/// is longer.
const BACK_OFF_MAX: i64 = 3600;

/// `every` as seconds, or why a poll cannot go by it: a poll costs a
/// request each time, and at least a minute goes between two.
pub fn every(every: &str) -> Result<i64, String> {
    let seconds = schedule::duration(every)
        .ok_or_else(|| format!("`{every}` is not a duration, like \"15m\""))?;
    if seconds < SHORTEST {
        return Err(format!(
            "`{every}` is shorter than a minute, the shortest a poll may be"
        ));
    }
    Ok(seconds)
}

/// How long a poll that failed `failures` times in a row waits.
pub fn wait(every: i64, failures: u32) -> i64 {
    if failures >= BACK_OFF_AFTER {
        (every * 4).min(BACK_OFF_MAX.max(every))
    } else {
        every
    }
}

/// What tau keeps of one poll across restarts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Polled {
    /// What it polls: a poll that changes starts over.
    pub kind: TriggerKind,
    /// When it polls next.
    pub next: i64,
    /// The keys it saw, oldest first.
    #[serde(default)]
    pub seen: VecDeque<String>,
    /// Whether it polled once: until then, keys are only recorded.
    #[serde(default)]
    pub primed: bool,
    /// Failures in a row.
    #[serde(default)]
    pub failures: u32,
    /// Why it last failed, while it does.
    #[serde(default)]
    pub error: Option<String>,
}

impl Polled {
    /// A poll of `kind` not polled yet, due at `next`.
    pub fn new(kind: TriggerKind, next: i64) -> Self {
        Self {
            kind,
            next,
            seen: VecDeque::new(),
            primed: false,
            failures: 0,
            error: None,
        }
    }

    /// Takes one result's `items` (key and item), and gives the events
    /// of those it had not seen; the first result gives none. `now` is
    /// when the result came; it polls next `every` from then.
    pub fn found(
        &mut self,
        trigger: &str,
        items: Vec<(String, Value)>,
        every: i64,
        now: i64,
    ) -> Vec<Event> {
        let mut known: BTreeSet<String> = self.seen.iter().cloned().collect();
        let mut events = Vec::new();
        for (key, item) in items {
            if !known.insert(key.clone()) {
                continue;
            }
            self.seen.push_back(key.clone());
            if self.primed {
                events.push(Event {
                    trigger: trigger.to_owned(),
                    kind: Kind::Poll,
                    id: format!("{trigger}:{key}"),
                    at: now,
                    data: item,
                });
            }
        }
        while self.seen.len() > KEYS {
            self.seen.pop_front();
        }
        self.primed = true;
        self.failures = 0;
        self.error = None;
        self.next = now + every;
        events
    }

    /// Notes that a poll failed with `error` at `now`.
    pub fn failed(&mut self, error: String, every: i64, now: i64) {
        self.failures += 1;
        self.error = Some(error);
        self.next = now + wait(every, self.failures);
    }
}

/// Makes the request a poll names, its secrets from `store`, and gives
/// what its `key` and `items` read: the answer as JSON when it is, else
/// its text. An answer that is not a success, is cut, or is too long to
/// read is a failure; every error is scrubbed of the secrets' values.
pub async fn fetch(store: &SecretStore, spec: &Value) -> Result<Value, String> {
    let fetched =
        http::fetch(&http::client(), store, spec, &CancellationToken::new())
            .await?;
    if !(200..300).contains(&fetched.status) {
        let body: String = fetched.body.chars().take(200).collect();
        return Err(format!("the answer was {}: {body}", fetched.status));
    }
    if fetched.cut || fetched.body.len() > http::MAX_JSON_BYTES {
        return Err(format!(
            "the answer is longer than a poll reads ({} KiB)",
            http::MAX_JSON_BYTES / 1024
        ));
    }
    Ok(fetched.json.unwrap_or(Value::String(fetched.body)))
}

/// The items `found` holds, as `tau._poll_items` gives them.
pub fn items(found: Value) -> Result<Vec<(String, Value)>, String> {
    let found = match found {
        // An empty table reads as an object.
        Value::Object(map) if map.is_empty() => return Ok(Vec::new()),
        Value::Array(found) => found,
        other => {
            return Err(format!("the poll's items are not a list: {other}"));
        }
    };
    found
        .into_iter()
        .map(|entry| {
            let key = entry
                .get("key")
                .and_then(Value::as_str)
                .ok_or("an item has no key")?
                .to_owned();
            Ok((key, entry.get("item").cloned().unwrap_or(json!(null))))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use hegel::generators as gs;

    use super::*;

    fn poll() -> Polled {
        Polled::new(
            TriggerKind::Poll {
                source: "s".into(),
                every: "10m".into(),
                tool: Some("t".into()),
                args: None,
                http: None,
            },
            0,
        )
    }

    fn keyed(keys: &[&str]) -> Vec<(String, Value)> {
        keys.iter()
            .map(|key| ((*key).to_owned(), json!({ "id": key })))
            .collect()
    }

    /// A poll's first result only records its keys; each later one fires,
    /// in order, the items whose key it does not hold, once each, and it
    /// holds the last [`KEYS`] keys it saw, so a key it let go fires
    /// again. Every result sets the next poll an interval on. Model: the
    /// keys held, as a queue, oldest first. The first result may fill the
    /// queue nearly to its bound, so later ones push keys out.
    #[hegel::test(test_cases = 100)]
    fn a_poll_fires_each_key_it_does_not_hold_once(tc: hegel::TestCase) {
        let fill = tc.draw(hegel::one_of!(
            gs::integers::<usize>().max_value(8),
            gs::integers::<usize>().min_value(KEYS - 8).max_value(KEYS),
        ));
        let results: Vec<Vec<&str>> = tc.draw(
            gs::vecs(
                // `f0` and `f1` are the oldest of the first result's
                // fillers: the first to be let go.
                gs::vecs(gs::sampled_from(vec!["a", "b", "c", "f0", "f1"]))
                    .max_size(6),
            )
            .min_size(1)
            .max_size(5),
        );
        let fillers: Vec<String> = (0..fill).map(|n| format!("f{n}")).collect();
        let mut polled = poll();
        let mut held: VecDeque<String> = VecDeque::new();
        for (n, result) in results.iter().enumerate() {
            let mut keys: Vec<&str> = result.clone();
            if n == 0 {
                keys.splice(0..0, fillers.iter().map(String::as_str));
            }
            let now = 1_000 * n as i64;
            let events = polled.found("r", keyed(&keys), 600, now);
            let mut fired = Vec::new();
            if n == 0 {
                // Distinct, and the first held: each is new.
                held.extend(fillers.iter().cloned());
            }
            for key in result {
                if held.iter().any(|kept| kept.as_str() == *key) {
                    continue;
                }
                held.push_back((*key).to_owned());
                if n > 0 {
                    fired.push(*key);
                }
            }
            while held.len() > KEYS {
                held.pop_front();
            }
            if fill > 8 && fired.iter().any(|key| key.starts_with('f')) {
                tc.event("a key let go fires again");
            }
            let ids: Vec<String> =
                fired.iter().map(|key| format!("r:{key}")).collect();
            assert_eq!(
                events.iter().map(|e| e.id.clone()).collect::<Vec<_>>(),
                ids
            );
            for (event, key) in events.iter().zip(&fired) {
                assert_eq!(event.kind, Kind::Poll);
                assert_eq!(event.at, now);
                assert_eq!(event.data, json!({ "id": key }));
            }
            assert_eq!(polled.seen, held);
            assert_eq!(polled.next, now + 600);
        }
    }

    /// After [`BACK_OFF_AFTER`] failures in a row a poll waits four times
    /// its interval, at most an hour, unless its interval is longer, which
    /// it keeps; before that, its interval. (The failures themselves, and a
    /// success starting over, are in `tests/agents.rs`.) Intervals reach
    /// both sides of the hour and of a quarter of it.
    #[hegel::test(test_cases = 500)]
    fn a_failing_poll_waits_four_intervals_at_most_an_hour(
        tc: hegel::TestCase,
    ) {
        let every = tc.draw(hegel::one_of!(
            gs::sampled_from(vec![60i64, 899, 900, 901, 3_599, 3_600, 3_601]),
            gs::integers::<i64>().min_value(60).max_value(3 * 86_400),
        ));
        let failures = tc.draw(gs::integers::<u32>().max_value(6));
        let expected = if failures < 3 {
            every
        } else if every >= 3_600 {
            tc.event("an interval of an hour or more");
            every
        } else {
            (4 * every).min(3_600)
        };
        assert_eq!(wait(every, failures), expected, "{every} after {failures}");
    }

    #[test]
    fn items_read_as_luau_gives_them() {
        assert_eq!(items(json!({})), Ok(Vec::new()));
        assert_eq!(
            items(json!([{ "key": "1", "item": { "n": 1 } }])),
            Ok(vec![("1".to_owned(), json!({ "n": 1 }))])
        );
        assert!(items(json!("x")).is_err());
    }
}
