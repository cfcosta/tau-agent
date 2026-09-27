//! The delta rule: when a request can continue from the previous response
//! on its lane instead of resending the whole transcript.
//!
//! Ported from pi's `getCachedWebSocketInputDelta`
//! (`packages/ai/src/api/openai-codex-responses.ts:1438`). The rule is
//! specified in `docs/reference/openai-websocket.md` ("The delta rule").
//!
//! After a response completes, its lane records the request body without
//! `input`, `previous_response_id` and `generate`, a *baseline* (the request's input
//! followed by the response's output items, tool outputs excluded), and
//! the response id. The next request may send only the items after the
//! baseline, with `previous_response_id`, if its body is otherwise
//! identical and its input starts with the baseline. Otherwise it is sent
//! in full.
//!
//! Items and bodies are compared as JSON values, so the order of keys
//! inside an object does not matter, as it does not to the server.

use serde_json::{Map, Value};

/// A request body: the fields of a `response.create` message.
pub type Body = Map<String, Value>;

const INPUT: &str = "input";
const PREVIOUS_RESPONSE_ID: &str = "previous_response_id";
/// A warm-up's `generate: false` switches one request off, not a setting
/// of the lane, so a real turn may continue from a warm-up.
const GENERATE: &str = "generate";

/// What a lane remembers about its last completed response.
#[derive(Debug, Clone, PartialEq)]
pub struct Continuation {
    body_sans_input: Body,
    baseline: Vec<Value>,
    response_id: String,
}

impl Continuation {
    /// Records a completed response.
    ///
    /// `full_body` is the request as it would be sent in full, and
    /// `output_items` are the response's output items converted the same
    /// way the next request will convert them, without tool outputs.
    pub fn record(
        full_body: &Body,
        output_items: Vec<Value>,
        response_id: String,
    ) -> Self {
        let mut baseline = input_of(full_body).to_vec();
        baseline.extend(output_items);
        Self {
            body_sans_input: without_input(full_body),
            baseline,
            response_id,
        }
    }

    /// The items to send after the baseline, if `full_body` can continue
    /// from this response.
    pub fn delta<'a>(&self, full_body: &'a Body) -> Option<&'a [Value]> {
        if without_input(full_body) != self.body_sans_input {
            return None;
        }
        input_of(full_body)
            .strip_prefix(self.baseline.as_slice())
            .filter(|_| !self.response_id.is_empty())
    }
}

/// A request ready to send, and whether it continues a previous response.
#[derive(Debug, Clone, PartialEq)]
pub struct Prepared {
    pub body: Body,
    pub kind: RequestKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RequestKind {
    /// The whole input, with no `previous_response_id`.
    Full,
    /// Only the new items, with `previous_response_id`.
    Delta,
}

/// Turns `full_body` into the request to send on a lane whose last
/// completed response is `continuation`, if any.
///
/// A full request is `full_body` unchanged, except that a stray
/// `previous_response_id` is removed: a full resend must never continue
/// anything.
pub fn prepare(
    continuation: Option<&Continuation>,
    full_body: Body,
) -> Prepared {
    let delta = continuation.and_then(|c| {
        c.delta(&full_body)
            .map(|d| (c.response_id.clone(), d.to_vec()))
    });
    match delta {
        Some((response_id, items)) => {
            let mut body = full_body;
            body.insert(INPUT.to_owned(), Value::Array(items));
            body.insert(
                PREVIOUS_RESPONSE_ID.to_owned(),
                Value::String(response_id),
            );
            Prepared {
                body,
                kind: RequestKind::Delta,
            }
        }
        None => {
            let mut body = full_body;
            body.remove(PREVIOUS_RESPONSE_ID);
            Prepared {
                body,
                kind: RequestKind::Full,
            }
        }
    }
}

fn input_of(body: &Body) -> &[Value] {
    match body.get(INPUT) {
        Some(Value::Array(items)) => items,
        _ => &[],
    }
}

fn without_input(body: &Body) -> Body {
    let mut rest = body.clone();
    rest.remove(INPUT);
    rest.remove(PREVIOUS_RESPONSE_ID);
    rest.remove(GENERATE);
    rest
}
