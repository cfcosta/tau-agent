//! What tau-watcher publishes, as its agent half writes it, its UI
//! writes it when the person acts, and every interface folds it.

use serde::{Deserialize, Serialize};

/// The name the plugin goes by in events and records.
pub const NAME: &str = "tau-watcher";

/// What kind of note it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tag {
    /// How something works.
    YouShouldKnow,
    /// About this session's work.
    HeadsUp,
}

impl Tag {
    pub const ALL: [Self; 2] = [Self::YouShouldKnow, Self::HeadsUp];

    /// How the model spells it, and the annotation says it.
    pub fn label(self) -> &'static str {
        match self {
            Self::YouShouldKnow => "You should know",
            Self::HeadsUp => "Heads up",
        }
    }
}

/// A note's longer form: what "Learn more" shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Explain {
    /// The takeaway, in a few words.
    pub title: String,
    pub bullets: Vec<String>,
}

/// What the person did with a note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    /// Opened the explanation.
    Learned,
    /// Said they knew it: the model is told not to say it again.
    Knew,
    /// Took it to the composer.
    Chatted,
    Dismissed,
}

/// What tau-watcher publishes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Record {
    /// The check found something to say.
    Noted {
        /// The step the check ran at: requests of the conversation so far.
        step: u32,
        tag: Tag,
        line: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        explain: Option<Explain>,
    },
    /// The check's reply could not be used: nothing is shown.
    Dropped { step: u32, reason: String },
    /// A message was written while a note waited unanswered.
    TypedPast,
    /// The person acted on the note anchored at `key`.
    Answered { key: String, answer: Answer },
}
