//! What a plugin's fold reaches of a run: where to place anchors, and
//! what the run holds so far. `tau-ui`'s run view implements it.

use serde::{Deserialize, Serialize};

/// A tool call the run made, as a fold reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardInfo {
    pub call_id: String,
    pub tool: String,
    /// The argument worth reading at a glance.
    pub summary: String,
    /// Characters the call and its result take in the context.
    pub size: usize,
    /// The turn it was made in.
    pub turn: u32,
}

/// The run a plugin folds a record into.
pub trait RunCx {
    /// Places the plugin's anchor `key` at the current point of the
    /// transcript. The plugin draws it at the transcript's point.
    fn transcript(&mut self, key: &str);
    /// Attaches the plugin's anchor `key` to the card of `call_id`; false
    /// when the run has no such card (yet).
    fn attach(&mut self, call_id: &str, key: &str) -> bool;
    /// The run's tool calls so far, in order.
    fn cards(&self) -> Vec<CardInfo>;
    /// The assistant's last text, if any.
    fn last_text(&self) -> Option<String>;
    /// The turn the run is in.
    fn turn(&self) -> u32;
}
