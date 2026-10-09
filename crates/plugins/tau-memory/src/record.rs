//! What tau-memory publishes, as its host half writes it and every
//! interface folds it.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The name the plugin goes by in events and records.
pub const NAME: &str = "tau-memory";

/// What tau-memory publishes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Record {
    /// The notes found for the run's task as it started.
    Recalled {
        notes: Vec<Recalled>,
    },
    /// What the model wrote or linked from a conversation about to be
    /// compacted, or at the end of a run: one entry per tool call.
    Saved {
        calls: Vec<Saved>,
    },
    Error {
        message: String,
    },
    /// What a run put in the conversation as it started, stored, never
    /// shown: the notes by id, and the index notes' text when they went
    /// in (they changed since they last did). The next run of the
    /// conversation leaves out what it already has.
    Given {
        notes: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<String>,
    },
    /// The conversation earlier runs gave notes to was compacted away:
    /// the next run gives them again. Stored, never shown.
    Forgotten,
    /// What the interface folds as a run starts, never stored: how many
    /// notes the repository has.
    Starting {
        notes: usize,
    },
}

/// A note found for a run's task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recalled {
    pub id: String,
    pub title: String,
}

/// One memory tool call made while saving, and what it gave.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    pub tool: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The prefix of ids from the user's scope.
pub const USER: &str = "user:";
