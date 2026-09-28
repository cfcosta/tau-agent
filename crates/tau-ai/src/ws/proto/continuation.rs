//! The delta rule: when a request can continue from the previous response
//! on its lane instead of resending the whole transcript.
//!
//! Ported from pi's `getCachedWebSocketInputDelta`
//! (`packages/ai/src/api/openai-codex-responses.ts:1438`). The rule is
//! specified in `docs/reference/openai-websocket.md` ("The delta rule").
//!
//! After a response completes, its lane records the request's fields
//! (every field but `input`, `previous_response_id` and `generate`), a
//! *baseline* (the request's input followed by the response's output
//! items, tool outputs excluded), and the response id. The next request
//! may send only the items after the baseline, with
//! `previous_response_id`, if its fields are otherwise identical and its
//! input starts with the baseline. Otherwise it is sent in full.
//!
//! Items and fields are compared as JSON values, so the order of keys
//! inside an object does not matter, as it does not to the server.
//!
//! ## Sharing
//!
//! A [`Body`] holds its fields and each input item behind an [`Arc`], so
//! a lane keeps its baseline without copying the transcript, and a
//! request built from the same items (see
//! [`InputCache`](crate::responses::input::InputCache)) matches the
//! baseline item by item on pointer identity: `Arc<Value>` equality
//! compares pointers before contents. Only items that were built anew,
//! such as the last response's output, are compared by value. The rule
//! is the same either way; sharing only makes it cheap.

use std::sync::Arc;

use serde_json::{Map, Value};

/// The fields of a `response.create` message other than `input` and
/// `previous_response_id`.
pub type Fields = Map<String, Value>;

const INPUT: &str = "input";
const PREVIOUS_RESPONSE_ID: &str = "previous_response_id";
/// A warm-up's `generate: false` switches one request off, not a setting
/// of the lane, so a real turn may continue from a warm-up.
const GENERATE: &str = "generate";

/// A `response.create` request. Cloning one copies no item.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Body {
    /// Every field but `input` and `previous_response_id`. A session
    /// builds them once and shares them across its requests.
    pub fields: Arc<Fields>,
    pub input: Vec<Arc<Value>>,
    pub previous_response_id: Option<String>,
}

impl Body {
    pub fn new(fields: Arc<Fields>, input: Vec<Arc<Value>>) -> Self {
        Self {
            fields,
            input,
            previous_response_id: None,
        }
    }

    /// The body as one JSON object.
    pub fn to_map(&self) -> Map<String, Value> {
        let mut map = (*self.fields).clone();
        if let Some(id) = &self.previous_response_id {
            map.insert(PREVIOUS_RESPONSE_ID.into(), Value::String(id.clone()));
        }
        map.insert(
            INPUT.into(),
            Value::Array(
                self.input.iter().map(|item| (**item).clone()).collect(),
            ),
        );
        map
    }

    /// The body serialized as a frame, with `extra` fields added (such as
    /// the lane's `stream_id`), without building it as one JSON value
    /// first.
    pub fn to_frame(&self, extra: &[(&str, &Value)]) -> String {
        let mut out = Vec::with_capacity(256);
        out.push(b'{');
        let mut first = true;
        let mut key = |out: &mut Vec<u8>, key: &str| {
            if !first {
                out.push(b',');
            }
            first = false;
            write_json(out, key);
            out.push(b':');
        };
        for (name, value) in self.fields.iter() {
            if extra.iter().any(|(k, _)| k == name) {
                continue;
            }
            key(&mut out, name);
            write_json(&mut out, value);
        }
        for (name, value) in extra {
            key(&mut out, name);
            write_json(&mut out, value);
        }
        if let Some(id) = &self.previous_response_id {
            key(&mut out, PREVIOUS_RESPONSE_ID);
            write_json(&mut out, id);
        }
        key(&mut out, INPUT);
        out.push(b'[');
        for (index, item) in self.input.iter().enumerate() {
            if index > 0 {
                out.push(b',');
            }
            write_json(&mut out, &**item);
        }
        out.extend_from_slice(b"]}");
        String::from_utf8(out).expect("serde_json writes UTF-8")
    }
}

/// Splits `input` and `previous_response_id` out of a JSON object.
impl From<Map<String, Value>> for Body {
    fn from(mut map: Map<String, Value>) -> Self {
        let input = match map.remove(INPUT) {
            Some(Value::Array(items)) => {
                items.into_iter().map(Arc::new).collect()
            }
            _ => Vec::new(),
        };
        let previous_response_id = match map.remove(PREVIOUS_RESPONSE_ID) {
            Some(Value::String(id)) => Some(id),
            _ => None,
        };
        Self {
            fields: Arc::new(map),
            input,
            previous_response_id,
        }
    }
}

fn write_json(out: &mut Vec<u8>, value: &(impl serde::Serialize + ?Sized)) {
    serde_json::to_writer(out, value).expect("JSON values serialize");
}

/// What a lane remembers about its last completed response.
#[derive(Debug, Clone, PartialEq)]
pub struct Continuation {
    fields: Arc<Fields>,
    baseline: Vec<Arc<Value>>,
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
        let mut baseline =
            Vec::with_capacity(full_body.input.len() + output_items.len());
        baseline.extend(full_body.input.iter().cloned());
        baseline.extend(output_items.into_iter().map(Arc::new));
        Self {
            fields: full_body.fields.clone(),
            baseline,
            response_id,
        }
    }

    /// The items to send after the baseline, if `full_body` can continue
    /// from this response.
    pub fn delta<'a>(&self, full_body: &'a Body) -> Option<&'a [Arc<Value>]> {
        if self.response_id.is_empty()
            || !same_fields(&self.fields, &full_body.fields)
        {
            return None;
        }
        full_body.input.strip_prefix(self.baseline.as_slice())
    }
}

/// Whether two sets of fields match, `generate` aside.
fn same_fields(a: &Arc<Fields>, b: &Arc<Fields>) -> bool {
    fn compared(fields: &Fields) -> impl Iterator<Item = (&String, &Value)> {
        fields.iter().filter(|(key, _)| key.as_str() != GENERATE)
    }
    Arc::ptr_eq(a, b)
        || (compared(a).count() == compared(b).count()
            && compared(a).all(|(key, value)| b.get(key) == Some(value)))
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
    full_body: &Body,
) -> Prepared {
    match continuation.and_then(|c| Some((c, c.delta(full_body)?))) {
        Some((continuation, items)) => Prepared {
            body: Body {
                fields: full_body.fields.clone(),
                input: items.to_vec(),
                previous_response_id: Some(continuation.response_id.clone()),
            },
            kind: RequestKind::Delta,
        },
        None => Prepared {
            body: Body {
                fields: full_body.fields.clone(),
                input: full_body.input.clone(),
                previous_response_id: None,
            },
            kind: RequestKind::Full,
        },
    }
}
