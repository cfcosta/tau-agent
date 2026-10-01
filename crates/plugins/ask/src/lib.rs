//! tau-ask: the agent asks the person questions and waits for the
//! answers.
//!
//! The `ask` tool takes one to four questions, each with two to four
//! choices, one or several to pick, and a preview per choice. The person
//! can always write their own answer instead, and add a note to any
//! answer. The call stays open until the answers come, the person
//! declines, or the run is cancelled; the answers are the call's result.
//!
//! - [`ask`]: the questions, the answers, how they are checked, and what
//!   the model reads back.
//! - [`Record`]: what the plugin publishes, folded by its UI.
//! - [`ui`]: the panel that takes the composer's place while a question
//!   waits, and the card of an `ask` call.
//! - `host` (feature `host`): the tool and the agent plugin.

pub mod ask;
#[cfg(feature = "host")]
pub mod host;
pub mod ui;

pub use ask::{Answer, Ask, Choice, Question, Reply};
use serde::{Deserialize, Serialize};
use serde_json::Value;
pub use ui::AskUi;

/// The plugin's name, for both its halves.
pub const NAME: &str = "tau-ask";

/// The tool's name, as the model calls it.
pub const TOOL: &str = "ask";

/// Everything tau-ask publishes about a call, as stored and reported.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Record {
    /// The call waits for the person to answer `ask`.
    Asked { call: String, ask: Ask },
    /// The person answered, or declined.
    Answered { call: String, reply: Reply },
    /// It stopped waiting without an answer: the run was cancelled, or
    /// it ended while the call waited.
    Closed { call: String },
}

/// Whether an undecodable record was logged already: once is enough.
impl Record {
    /// The record `body` holds, or none, said the first time.
    pub fn parse(body: &Value) -> Option<Self> {
        tau_agent::plugin::read_record(NAME, body)
    }

    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("a record serializes")
    }

    /// The call it is about.
    pub fn call(&self) -> &str {
        match self {
            Self::Asked { call, .. }
            | Self::Answered { call, .. }
            | Self::Closed { call } => call,
        }
    }
}
