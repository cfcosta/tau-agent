//! What a plugin's fold reaches of a run: where to place anchors, and
//! what the run holds so far. `tau-ui`'s run view implements it.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_agent::tool::ToolOutput;
use tau_ai::message::InputBlock;

/// A tool call the run made, as a fold reads it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CardInfo {
    pub call_id: String,
    pub tool: String,
    /// The arguments the model sent.
    pub args: Value,
    /// The argument worth reading at a glance.
    pub summary: String,
    /// Characters the call and its result take in the context.
    pub size: usize,
    /// The turn it was made in.
    pub turn: u32,
}

/// What a tool call sent, as its card shows it: its arguments, what it
/// reported while it ran, and its result.
///
/// A tool can call tools through the loop (`ToolCtx::call`, ADR 0018).
/// Those nested calls never reach the transcript, so they get no card of
/// their own: they fold into the card of the model's call they came
/// from, however deep, as [`Self::nested`] and [`Self::nested_marks`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CallData {
    pub args: Value,
    /// Each update's details while it ran, oldest first.
    pub updates: Vec<Value>,
    /// The text of the latest update.
    pub partial: Option<String>,
    /// Its result, once it ended.
    pub result: Option<CallResult>,
    /// The calls it made while it runs, in the order they started.
    /// Live only: they come from run events, which a stored run does not
    /// keep, so [`Self::end`] empties them and a live card and a stored
    /// one hold the same thing once the call ends. A tool whose calls
    /// should outlast it lists them in its result's details, as
    /// codemode's `calls` does, and its card draws them from there.
    #[serde(default)]
    pub nested: Vec<NestedCall>,
    /// What plugins decided about its nested calls, in the order they
    /// came. Kept once the call ends: plugins' records place them, live
    /// and in history alike.
    #[serde(default)]
    pub nested_marks: Vec<NestedMark>,
}

/// The most characters a nested call's text keeps: its latest update,
/// or its result. A card shows a line of it; a script's `read` of a
/// large file should not grow the run's view by as much.
pub const NESTED_TEXT_CHARS: usize = 2_000;

/// A call a tool made through the loop, as its card holds it while it
/// runs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NestedCall {
    /// `<parent>/<n>`.
    pub id: String,
    /// The call that made it: the card's own call, or another nested
    /// call.
    pub parent: String,
    pub tool: String,
    /// The arguments as the loop ran it, repaired.
    pub args: Value,
    /// Updates it reported.
    pub updates: usize,
    /// The text of the latest update, cut at [`NESTED_TEXT_CHARS`].
    pub partial: Option<String>,
    /// Its result once it ended: its text blocks joined with nothing
    /// between them, as a calling tool reads a failure's text, cut at
    /// [`NESTED_TEXT_CHARS`]; without details.
    pub result: Option<CallResult>,
}

/// What `plugin` decided about the nested call `id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NestedMark {
    pub id: String,
    pub plugin: String,
    pub mark: CardMark,
}

impl CallData {
    /// The nested call `id` started, made by `parent`.
    pub fn nested_start(
        &mut self,
        id: &str,
        parent: &str,
        tool: &str,
        args: &Value,
    ) {
        if self.nested.iter().any(|call| call.id == id) {
            return;
        }
        self.nested.push(NestedCall {
            id: id.to_owned(),
            parent: parent.to_owned(),
            tool: tool.to_owned(),
            args: args.clone(),
            ..NestedCall::default()
        });
    }

    /// The nested call `id` reported `partial`.
    pub fn nested_update(&mut self, id: &str, partial: &ToolOutput) {
        if let Some(call) = self.nested.iter_mut().find(|call| call.id == id) {
            call.updates += 1;
            call.partial = Some(nested_text(partial));
        }
    }

    /// The nested call `id` ended with `output`.
    pub fn nested_end(&mut self, id: &str, output: &ToolOutput, error: bool) {
        if let Some(call) = self.nested.iter_mut().find(|call| call.id == id) {
            call.result = Some(CallResult {
                text: nested_text(output),
                details: None,
                error,
            });
        }
    }

    /// Keeps what `plugin` decided about the nested call `id`; a later
    /// mark from the same plugin replaces it.
    pub fn mark_nested(&mut self, id: &str, plugin: &str, mark: CardMark) {
        self.nested_marks
            .retain(|kept| !(kept.id == id && kept.plugin == plugin));
        self.nested_marks.push(NestedMark {
            id: id.to_owned(),
            plugin: plugin.to_owned(),
            mark,
        });
    }

    /// The marks on the nested call `id`.
    pub fn marks_of<'a>(
        &'a self,
        id: &'a str,
    ) -> impl Iterator<Item = &'a NestedMark> {
        self.nested_marks.iter().filter(move |mark| mark.id == id)
    }

    /// The call ended: its nested calls go, as a stored run never had
    /// them (see [`Self::nested`]).
    pub fn end(&mut self) {
        self.nested = Vec::new();
    }
}

/// `output`'s text blocks joined with nothing between them, cut at
/// [`NESTED_TEXT_CHARS`].
fn nested_text(output: &ToolOutput) -> String {
    output
        .content
        .iter()
        .filter_map(|block| match block {
            InputBlock::Text(text) => Some(text.text.as_str()),
            InputBlock::Image(_) => None,
        })
        .flat_map(str::chars)
        .take(NESTED_TEXT_CHARS)
        .collect()
}

/// Whether `id` is a call nested under `call_id`, at any depth: nested
/// ids are `<parent>/<n>`.
pub fn nested_under(id: &str, call_id: &str) -> bool {
    id.strip_prefix(call_id)
        .is_some_and(|rest| rest.starts_with('/'))
}

/// What a tool returned.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CallResult {
    pub text: String,
    pub details: Option<Value>,
    pub error: bool,
}

/// What a plugin decided about a tool call, for its card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CardMark {
    /// Refused; `reason` went back to the model.
    Blocked { reason: String },
    /// It ran, and waits for a person to look at it.
    Flagged,
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
///
/// Card lookups take any call id the run's events or a plugin's hooks
/// name, nested calls' included (ADR 0018). A nested call has no card:
///
/// - [`Self::attach`] lands on the card of the model's call it came
///   from, so what a plugin draws for it shows there;
/// - [`Self::mark`] keeps the mark on that card's
///   [`CallData::nested_marks`], for the row the card draws for it, and
///   leaves the card's own state alone: a script that catches a blocked
///   call still succeeds;
/// - [`Self::dropped`] and [`Self::cut`] are ignored and answer false: a
///   nested result never reaches the model's context, so no context
///   rewrite drops it and no cut changes what the model saw.
pub trait RunCx {
    /// Places the plugin's anchor `key` at the current point of the
    /// transcript. The plugin draws it at the transcript's point.
    fn transcript(&mut self, key: &str);
    /// Attaches the plugin's anchor `key` to the card of `call_id`; false
    /// when the run has no such card (yet).
    fn attach(&mut self, call_id: &str, key: &str) -> bool;
    /// Marks what the plugin decided about the call `call_id`; false when
    /// the run has no such card.
    fn mark(&mut self, call_id: &str, mark: CardMark) -> bool;
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
