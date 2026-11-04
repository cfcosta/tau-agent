//! A child run's landing on its parent (ADR 0009): what it brought, in
//! the parent's chat.

use serde::{Deserialize, Serialize};
use tau_agent::tool::RunId;
use tau_ui_plugin::tree::{Node, Tone};

use super::{change_log::Change, log_card::stack};

/// The record a landing leaves in its parent:
/// enough to draw its card again when the parent comes back from
/// history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LandingRecord {
    pub from: String,
    pub title: String,
    pub landing: crate::Landing,
    /// tau closed in the middle of the landing, and finished it at its
    /// next start.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub recovered: bool,
    /// The chat landed what it had and went on (ADR 0034): it did not
    /// close.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub kept: bool,
}

impl LandedCard {
    pub fn from_record(record: LandingRecord) -> Self {
        Self {
            from: RunId(record.from.into()),
            title: record.title,
            changes: record
                .landing
                .changes
                .into_iter()
                .map(Change::new)
                .collect(),
            conflicts: record.landing.conflicts,
            recovered: record.recovered,
            kept: record.kept,
        }
    }
}

/// A child run that landed on this run: what it brought.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LandedCard {
    pub from: RunId,
    /// The child's title, as the run list called it.
    pub title: String,
    /// Its changes on this run's stack, newest first.
    pub changes: Vec<Change>,
    /// Paths left with conflict markers for this run's next turn.
    pub conflicts: Vec<String>,
    /// tau finished the landing at start, after closing in its middle.
    #[serde(default)]
    pub recovered: bool,
    /// The chat went on after it landed (ADR 0034).
    #[serde(default)]
    pub kept: bool,
}

/// What a landing brought: its changes as they sit on the stack, and
/// the files left with conflict markers. Core draws it in the parent's
/// chat.
pub fn landed_body(card: &LandedCard, compact: bool) -> Node {
    let mut column = Vec::new();
    if !card.changes.is_empty() {
        column.push(stack(&card.changes, compact));
    }
    if !card.conflicts.is_empty() {
        column.push(Node::Toned {
            text: format!(
                "Conflicts in {}: the next turn resolves them.",
                card.conflicts.join(", ")
            ),
            tone: Tone::Bad,
            mono: false,
        });
    }
    Node::stack(column)
}
