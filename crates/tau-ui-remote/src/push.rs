//! Pushing a repository's main chat to GitHub (ADR 0023): what the
//! host reports, and where a push stands in the interface.

use serde::{Deserialize, Serialize};

/// A change a push took to GitHub: its change id and its description's
/// first line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushedChange {
    pub change_id: String,
    pub title: String,
}

/// What a push did: the branch, GitHub's commit before and after (full
/// ids), and the changes it took, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pushed {
    pub branch: String,
    pub from: Option<String>,
    pub to: String,
    pub changes: Vec<PushedChange>,
}

/// Why a push did not go.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PushFailure {
    /// GitHub's `branch` moved since the last fetch: nothing was pushed,
    /// and `ahead` changes still wait. Fetching puts them on top, and
    /// the push can go again.
    Moved { branch: String, ahead: u32 },
    /// Anything else, in words.
    Failed(String),
}

/// Where a repository's push stands, for its main chat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PushState {
    /// Pushing, or fetching first.
    Pushing {
        fetching: bool,
    },
    Pushed(Pushed),
    Moved {
        branch: String,
        ahead: u32,
    },
}

impl Pushed {
    /// A commit id as the cards show it: its first seven digits.
    pub fn short(id: &str) -> &str {
        id.get(..7).unwrap_or(id)
    }
}
