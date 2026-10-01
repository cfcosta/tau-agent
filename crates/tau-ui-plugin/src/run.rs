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

/// What a context rewrite left of a tool call in the model's context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Dropped {
    /// The call stays, its result is gone.
    Result,
    /// Neither stays.
    Call,
}

/// A result a plugin cut as it arrived: the lines the model saw of the
/// whole output, and the file holding the whole of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputCut {
    pub kept: usize,
    pub lines: usize,
    pub archive: String,
    /// Estimated tokens of the whole output, and of what the model saw.
    pub tokens_before: u64,
    pub tokens_after: u64,
    /// What cutting it cost, in US dollars.
    pub cost: f64,
}

impl OutputCut {
    /// `kept 212 of 4,810 lines · 12k → 900 tokens`.
    pub fn label(&self) -> String {
        use tau_ui_kit::format::{grouped, tokens};
        format!(
            "kept {} of {} lines · {} → {} tokens",
            grouped(self.kept),
            grouped(self.lines),
            tokens(self.tokens_before),
            tokens(self.tokens_after)
        )
    }
}

/// The run a plugin folds a record into.
pub trait RunCx {
    /// Places the plugin's anchor `key` at the current point of the
    /// transcript. The plugin draws it at the transcript's point.
    fn transcript(&mut self, key: &str);
    /// Attaches the plugin's anchor `key` to the card of `call_id`; false
    /// when the run has no such card (yet).
    fn attach(&mut self, call_id: &str, key: &str) -> bool;
    /// Marks what the plugin's context rewrite left of the call
    /// `call_id`; false when the run has no such card.
    fn dropped(&mut self, call_id: &str, dropped: Dropped) -> bool;
    /// Says the call `call_id`'s result reached the model cut; false when
    /// the run has no such card.
    fn cut(&mut self, call_id: &str, cut: OutputCut) -> bool;
    /// Names one of the plugin's context rewrites `key`, for it to draw
    /// at [`crate::points::REWRITE`]: the rewrite it is about to make, or,
    /// folding a stored rewrite's details ([`crate::REWRITE`]), the one
    /// history placed.
    fn rewrite(&mut self, key: &str);
    /// The run's tool calls so far, in order.
    fn cards(&self) -> Vec<CardInfo>;
    /// The assistant's last text, if any.
    fn last_text(&self) -> Option<String>;
    /// The turn the run is in.
    fn turn(&self) -> u32;
}
