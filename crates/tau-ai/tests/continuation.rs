//! The delta rule (`tau_ai::ws::proto::continuation`), checked against a
//! model of the server's per-connection response cache.

use std::collections::HashMap;

use hegel::{TestCase, generators as gs};
use serde_json::{Map, Value, json};
use tau_ai::ws::proto::continuation::{self, Body, Continuation, RequestKind};
use tau_testing::generators::{
    self,
    lane::{LaneHistory, Turn, item, lane_history},
};

/// A request prepared from a JSON body, as a JSON body again.
struct Prepared {
    body: Map<String, Value>,
    kind: RequestKind,
}

fn prepare(
    continuation: Option<&Continuation>,
    full: Map<String, Value>,
) -> Prepared {
    let prepared = continuation::prepare(continuation, &Body::from(full));
    Prepared {
        body: prepared.body.to_map(),
        kind: prepared.kind,
    }
}

fn record(
    full: &Map<String, Value>,
    output_items: Vec<Value>,
    response_id: String,
) -> Continuation {
    Continuation::record(&Body::from(full.clone()), output_items, response_id)
}

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
        continuation = Some(record(
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
    delta_reconstructs_full_input_body(tc)
}

/// [`delta_reconstructs_full_input`] with more cases, for the nightly tier.
#[hegel::test(profile = "nightly")]
#[ignore = "nightly"]
fn delta_reconstructs_full_input_nightly(tc: TestCase) {
    delta_reconstructs_full_input_body(tc)
}

fn delta_reconstructs_full_input_body(tc: TestCase) {
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
    settings_change_forces_full_resend_body(tc)
}

/// [`settings_change_forces_full_resend`] with more cases, for the nightly tier.
#[hegel::test(profile = "nightly")]
#[ignore = "nightly"]
fn settings_change_forces_full_resend_nightly(tc: TestCase) {
    settings_change_forces_full_resend_body(tc)
}

fn settings_change_forces_full_resend_body(tc: TestCase) {
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

/// On a model that keeps its cache across efforts, a new reasoning
/// effort still continues; on any other model it resends in full.
#[hegel::test(test_cases = 200)]
fn an_effort_change_continues_only_where_the_cache_survives(tc: TestCase) {
    let history = tc.draw(generators::lane::lane_history());
    let at =
        tc.draw(gs::integers::<usize>().max_value(history.turns.len() - 1));
    let model = tc.draw(gs::sampled_from(vec![
        "gpt-6-sol",
        "gpt-6-astra",
        "gpt-5.6-terra",
        "gpt-6-luna",
    ]));
    let kinds = run_lane(&history, |i, body| {
        body.insert("model".into(), json!(model));
        let effort = if i >= at { "high" } else { "low" };
        body.insert(
            "reasoning".into(),
            json!({ "effort": effort, "summary": "auto" }),
        );
    });
    let keeps = tau_ai::model::effort_keeps_cache(model);
    for (i, kind) in kinds.iter().enumerate() {
        let expected = if i == 0 || (i == at && !keeps) {
            RequestKind::Full
        } else {
            RequestKind::Delta
        };
        assert_eq!(*kind, expected, "turn {i} on {model}");
    }
}

/// A lane history with two turns or more: [`lane_history`], with a turn
/// added when it drew only one.
#[hegel::composite]
fn at_least_two_turns(tc: &TestCase) -> LaneHistory {
    let mut history = tc.draw(lane_history());
    if history.turns.len() < 2 {
        let response_id = format!("{}_next", history.turns[0].response_id);
        history.turns.push(Turn {
            new_items: tc.draw(gs::vecs(item()).min_size(1).max_size(3)),
            output_items: tc.draw(gs::vecs(item()).max_size(3)),
            response_id,
        });
    }
    history
}

/// Changing an item the server already holds (in the baseline) forces a
/// full resend; the server still sees the full input.
#[hegel::test(test_cases = 500)]
fn baseline_change_forces_full_resend(tc: TestCase) {
    baseline_change_forces_full_resend_body(tc)
}

/// [`baseline_change_forces_full_resend`] with more cases, for the nightly tier.
#[hegel::test(profile = "nightly")]
#[ignore = "nightly"]
fn baseline_change_forces_full_resend_nightly(tc: TestCase) {
    baseline_change_forces_full_resend_body(tc)
}

fn baseline_change_forces_full_resend_body(tc: TestCase) {
    let history = tc.draw(at_least_two_turns());
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
    let continuation =
        record(&full, turn.output_items.clone(), turn.response_id.clone());
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
    let continuation = record(
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
    let continuation = record(&full, vec![], "resp_1".into());
    let mut next = serde_json::Map::new();
    next.insert("input".into(), serde_json::from_str(r#"[{"text": "a", "type": "message"}, {"type": "message", "text": "c"}]"#).unwrap());
    assert_eq!(prepare(Some(&continuation), next).kind, RequestKind::Delta);
}

/// A warm-up (`generate: false`, no input) is a continuation the first
/// real turn picks up: the turn goes as a delta carrying its whole
/// input, the server rebuilds exactly that input, and `generate` itself
/// never decides whether bodies match.
#[hegel::test(test_cases = 200)]
fn a_real_turn_continues_from_a_warm_up(tc: TestCase) {
    let history = tc.draw(generators::lane::lane_history());
    let first = history.full_body(0);
    let mut warm_up = first.clone();
    warm_up.insert("input".into(), json!([]));
    warm_up.insert("generate".into(), json!(false));
    let mut server = Server::default();
    server.responses.insert("resp_warm".into(), Vec::new());
    let continuation = record(&warm_up, Vec::new(), "resp_warm".into());
    let prepared = prepare(Some(&continuation), first.clone());
    assert_eq!(prepared.kind, RequestKind::Delta);
    assert_eq!(prepared.body["previous_response_id"], json!("resp_warm"));
    assert_eq!(prepared.body["input"], first["input"]);
    assert_eq!(
        server.rebuild(&prepared.body),
        first["input"].as_array().unwrap().clone()
    );
    assert!(prepared.body.get("generate").is_none());
}

/// A body serialized as a frame is the body as one JSON object, plus
/// the extra fields, which replace fields of the same name.
#[hegel::test(test_cases = 200)]
fn a_frame_is_the_body_as_json(tc: TestCase) {
    let history = tc.draw(generators::lane::lane_history());
    let index =
        tc.draw(gs::integers::<usize>().max_value(history.turns.len() - 1));
    let mut full = history.full_body(index);
    if tc.draw(gs::booleans()) {
        full.insert("previous_response_id".into(), json!("resp_1"));
    }
    if tc.draw(gs::booleans()) {
        full.insert("stream_id".into(), json!("stale"));
    }
    let body = Body::from(full);
    let stream_id = json!("tau-7");
    let frame: Value =
        serde_json::from_str(&body.to_frame(&[("stream_id", &stream_id)]))
            .unwrap();
    let mut expected = body.to_map();
    expected.insert("stream_id".into(), stream_id);
    assert_eq!(frame, Value::Object(expected));
}

/// A JSON body splits into fields, input and `previous_response_id`, and
/// joins back unchanged.
#[hegel::test(test_cases = 200)]
fn a_json_body_round_trips(tc: TestCase) {
    let history = tc.draw(generators::lane::lane_history());
    let mut full = history.full_body(0);
    let previous = tc.draw(gs::booleans());
    if previous {
        full.insert("previous_response_id".into(), json!("resp_1"));
    }
    let body = Body::from(full.clone());
    assert_eq!(body.previous_response_id.is_some(), previous);
    assert!(!body.fields.contains_key("previous_response_id"));
    assert!(!body.fields.contains_key("input"));
    assert_eq!(body.to_map(), full);
}
