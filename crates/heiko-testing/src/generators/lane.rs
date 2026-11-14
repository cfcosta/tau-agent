//! Lane histories for the delta rule.

use hegel::{TestCase, generators as gs};
use serde_json::{Map, Value, json};

use super::{id, text};

/// One turn on a lane: the items the caller adds (a user message or tool
/// outputs), then the output items of the response.
#[derive(Debug, Clone, PartialEq, hegel::PrettyPrintable)]
pub struct Turn {
    pub new_items: Vec<Value>,
    pub output_items: Vec<Value>,
    pub response_id: String,
}

/// A clean lane history: fixed request settings and a series of turns,
/// each of which extends the previous turn's input with that turn's
/// output items and its own new items.
#[derive(Debug, Clone, PartialEq, hegel::PrettyPrintable)]
pub struct LaneHistory {
    /// Every request field except `input`: model, instructions, tools.
    pub settings: Map<String, Value>,
    pub turns: Vec<Turn>,
}

impl LaneHistory {
    /// The full `input` of turn `index`.
    pub fn full_input(&self, index: usize) -> Vec<Value> {
        let mut input = Vec::new();
        for (i, turn) in self.turns.iter().enumerate().take(index + 1) {
            input.extend(turn.new_items.iter().cloned());
            if i < index {
                input.extend(turn.output_items.iter().cloned());
            }
        }
        input
    }

    /// The full request body of turn `index`.
    pub fn full_body(&self, index: usize) -> Map<String, Value> {
        let mut body = self.settings.clone();
        body.insert("input".into(), Value::Array(self.full_input(index)));
        body
    }
}

/// A small input or output item.
#[hegel::composite]
pub fn item(tc: &TestCase) -> Value {
    let kind = tc.draw(gs::sampled_from(vec![
        "message",
        "reasoning",
        "function_call",
        "function_call_output",
    ]));
    json!({ "type": kind, "text": tc.draw(text(8)) })
}

#[hegel::composite]
pub fn lane_history(tc: &TestCase) -> LaneHistory {
    let mut settings = Map::new();
    settings.insert("type".into(), json!("response.create"));
    settings.insert("model".into(), json!("gpt-5.5"));
    settings.insert("store".into(), json!(false));
    settings.insert("instructions".into(), json!(tc.draw(text(16))));
    let turn_count = tc.draw(gs::integers::<usize>().min_value(1).max_value(6));
    let turns = (0..turn_count)
        .map(|i| Turn {
            new_items: tc.draw(gs::vecs(item()).min_size(1).max_size(3)),
            output_items: tc.draw(gs::vecs(item()).max_size(3)),
            response_id: format!("{}_{i}", tc.draw(id("resp_"))),
        })
        .collect();
    LaneHistory { settings, turns }
}
