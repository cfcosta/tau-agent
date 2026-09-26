//! The delta rule (`tau_ai::ws::proto::continuation`), checked against a
//! model of the server's per-connection response cache.

use std::collections::HashMap;

use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_ai::ws::proto::continuation::{Continuation, RequestKind, prepare};
use tau_testing::generators::{self, lane::LaneHistory};

/// The server side of one connection: the full item list behind every
/// response it holds, as OpenAI keeps them in memory.
#[derive(Default)]
struct Server {
    responses: HashMap<String, Vec<Value>>,
}

impl Server {
    /// The input the server sees for a request: the held items of
    /// `previous_response_id`, then the request's own input.
    fn rebuild(&self, body: &serde_json::Map<String, Value>) -> Vec<Value> {
        let input = body["input"].as_array().unwrap().clone();
        match body.get("previous_response_id") {
            Some(Value::String(id)) => {
                let mut items = self.responses[id].clone();
                items.extend(input);
                items
            }
            _ => input,
        }
    }
}

/// Runs a lane over a clean history and returns the kind of each request.
/// Asserts, turn by turn, that the server rebuilds the full input.
fn run_lane(
    history: &LaneHistory,
    perturb: impl Fn(usize, &mut serde_json::Map<String, Value>),
) -> Vec<RequestKind> {
    let mut server = Server::default();
    let mut continuation: Option<Continuation> = None;
    let mut kinds = Vec::new();
    for (i, turn) in history.turns.iter().enumerate() {
        let mut full = history.full_body(i);
        perturb(i, &mut full);
        let prepared = prepare(continuation.as_ref(), full.clone());
        let seen = server.rebuild(&prepared.body);
        assert_eq!(seen, full["input"].as_array().unwrap().clone(), "turn {i}");
        let mut held = seen;
        held.extend(turn.output_items.iter().cloned());
        server.responses.insert(turn.response_id.clone(), held);
        continuation = Some(Continuation::record(
            &full,
            turn.output_items.clone(),
            turn.response_id.clone(),
        ));
        kinds.push(prepared.kind);
    }
    kinds
}

/// On a clean history, every turn after the first is a delta, and the
/// input the server rebuilds from its cache plus the delta equals the full
/// input of that turn.
#[hegel::test(test_cases = 500)]
fn delta_reconstructs_full_input(tc: TestCase) {
    let history = tc.draw(generators::lane::lane_history());
    let kinds = run_lane(&history, |_, _| {});
    assert_eq!(kinds[0], RequestKind::Full);
    assert!(
        kinds[1..].iter().all(|k| *k == RequestKind::Delta),
        "{kinds:?}"
    );
}

/// Changing any request field other than `input` at some turn forces that
/// turn to a full resend; the server still sees the full input.
#[hegel::test(test_cases = 500)]
fn settings_change_forces_full_resend(tc: TestCase) {
    let history = tc.draw(generators::lane::lane_history());
    let at =
        tc.draw(gs::integers::<usize>().max_value(history.turns.len() - 1));
    let field = tc.draw(gs::sampled_from(vec![
        "instructions",
        "model",
        "tools",
        "reasoning",
    ]));
    let kinds = run_lane(&history, |i, body| {
        if i >= at {
            // An array: no generated setting is one, so this always differs.
            body.insert(field.into(), json!(["changed"]));
        }
    });
    assert_eq!(kinds[at], RequestKind::Full);
    for (i, kind) in kinds.iter().enumerate() {
        let expected = if i == 0 || i == at {
            RequestKind::Full
        } else {
            RequestKind::Delta
        };
        assert_eq!(*kind, expected, "turn {i}");
    }
}

/// Changing an item the server already holds (in the baseline) forces a
/// full resend; the server still sees the full input.
#[hegel::test(test_cases = 500)]
fn baseline_change_forces_full_resend(tc: TestCase) {
    let history = tc.draw(generators::lane::lane_history());
    tc.assume(history.turns.len() > 1); // one turn in six-sized histories
    let at = tc.draw(
        gs::integers::<usize>()
            .min_value(1)
            .max_value(history.turns.len() - 1),
    );
    let baseline_len = history.full_input(at - 1).len()
        + history.turns[at - 1].output_items.len();
    let index = tc.draw(gs::integers::<usize>().max_value(baseline_len - 1));
    let kinds = run_lane(&history, |i, body| {
        if i == at {
            body["input"][index]["text"] = json!("rewritten ☃");
        }
    });
    assert_eq!(kinds[at], RequestKind::Full);
}

/// A full request is the body unchanged, minus any stray
/// `previous_response_id`; a delta request is the body with only the new
/// items and the previous response id, every other field untouched.
#[hegel::test]
fn prepared_bodies_keep_other_fields(tc: TestCase) {
    let history = tc.draw(generators::lane::lane_history());
    let mut full = history.full_body(0);
    full.insert("previous_response_id".into(), json!("resp_stale"));
    let first = prepare(None, full.clone());
    full.remove("previous_response_id");
    assert_eq!(first.body, full);

    let turn = &history.turns[0];
    let continuation = Continuation::record(
        &full,
        turn.output_items.clone(),
        turn.response_id.clone(),
    );
    let mut next = full.clone();
    let mut input = history.full_input(0);
    input.extend(turn.output_items.iter().cloned());
    let added = tc.draw(gs::vecs(generators::lane::item()).max_size(3));
    input.extend(added.iter().cloned());
    next.insert("input".into(), Value::Array(input));
    let prepared = prepare(Some(&continuation), next.clone());
    assert_eq!(prepared.kind, RequestKind::Delta);
    assert_eq!(prepared.body["input"], Value::Array(added));
    assert_eq!(
        prepared.body["previous_response_id"],
        json!(turn.response_id)
    );
    for (key, value) in &next {
        if key != "input" {
            assert_eq!(&prepared.body[key], value, "{key}");
        }
    }
}

/// Input shorter than the baseline cannot continue.
#[test]
fn shorter_input_forces_full_resend() {
    let mut full = serde_json::Map::new();
    full.insert("input".into(), json!([{"type": "message", "text": "a"}]));
    let continuation = Continuation::record(
        &full,
        vec![json!({"type": "message", "text": "b"})],
        "resp_1".into(),
    );
    assert_eq!(prepare(Some(&continuation), full).kind, RequestKind::Full);
}

/// Key order inside items does not matter, as it does not to the server.
#[test]
fn key_order_does_not_matter() {
    let mut full = serde_json::Map::new();
    full.insert("input".into(), json!([{"type": "message", "text": "a"}]));
    let continuation = Continuation::record(&full, vec![], "resp_1".into());
    let mut next = serde_json::Map::new();
    next.insert("input".into(), serde_json::from_str(r#"[{"text": "a", "type": "message"}, {"type": "message", "text": "c"}]"#).unwrap());
    assert_eq!(prepare(Some(&continuation), next).kind, RequestKind::Delta);
}
