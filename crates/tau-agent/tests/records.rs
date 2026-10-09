//! Records a plugin keeps in a transcript outlive the code that wrote
//! them. Reading them skips what no longer parses, keeps the rest in
//! order, and never stops a run.
//!
//! Properties: `read_records` agrees with parsing each body on its own;
//! a record written by `serde_json` reads back whatever surrounds it.

use hegel::{TestCase, generators as gs};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tau_agent::plugin::{read_record, read_records};
use tau_testing::generators;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Note {
    id: u32,
    text: String,
    #[serde(default)]
    tags: Vec<String>,
}

hegel::pretty_print_as_debug!(Note);

#[hegel::composite]
fn note(tc: &TestCase) -> Note {
    Note {
        id: tc.draw(gs::integers()),
        text: tc.draw(gs::text().max_size(20)),
        tags: tc.draw(gs::vecs(gs::text().max_size(5)).max_size(3)),
    }
}

/// A body: a record, one with a field of the wrong type, a record of an
/// older shape (no `text`), or any JSON at all.
#[hegel::composite]
fn body(tc: &TestCase) -> Value {
    match tc.draw(gs::integers::<u8>().max_value(3)) {
        0 => json!(tc.draw(note())),
        1 => json!({"id": tc.draw(gs::text().max_size(3)), "text": "x"}),
        2 => json!({"id": tc.draw(gs::integers::<u32>())}),
        _ => Value::Object(tc.draw(generators::json_object(2))),
    }
}

/// Reading many is reading each, with the unreadable left out and the
/// order kept.
#[hegel::test(test_cases = 300)]
fn reading_many_is_reading_each_in_order(tc: TestCase) {
    let bodies = tc.draw(gs::vecs(body()).max_size(10));
    let want: Vec<Note> = bodies
        .iter()
        .filter_map(|body| serde_json::from_value(body.clone()).ok())
        .collect();
    assert_eq!(read_records::<Note>("test", &bodies), want);
    for body in &bodies {
        assert_eq!(
            read_record::<Note>("test", body),
            serde_json::from_value(body.clone()).ok()
        );
    }
}

/// What a plugin wrote, it reads back, however much unreadable data
/// sits between its records.
#[hegel::test(test_cases = 300)]
fn written_records_read_back_between_unreadable_ones(tc: TestCase) {
    let notes = tc.draw(gs::vecs(note()).max_size(6));
    let mut bodies = Vec::new();
    for note in &notes {
        for _ in 0..tc.draw(gs::integers::<u8>().max_value(2)) {
            bodies.push(json!({"unrelated": tc.draw(gs::text().max_size(5))}));
        }
        bodies.push(json!(note));
    }
    assert_eq!(read_records::<Note>("test", &bodies), notes);
}
