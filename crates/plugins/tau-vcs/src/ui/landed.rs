//! A child run's landing on its parent (ADR 0009): what it brought, in
//! the parent's chat and on the `wait` card that landed it.

use gpui::{Div, div, prelude::*};
use serde::{Deserialize, Serialize};
use tau_agent::tool::RunId;
use tau_ui_kit::theme::{Design as _, Theme, Type, sp};

use super::{change_log::Change, log_card::stack_rows};

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
}

/// What a landing brought: its changes as they sit on the stack, and
/// the files left with conflict markers.
pub fn landed_body(card: &LandedCard, t: &Theme, compact: bool) -> Div {
    div()
        .flex()
        .flex_col()
        .when(!card.changes.is_empty(), |column| {
            column.child(
                div()
                    .flex()
                    .flex_col()
                    .py(sp(1.5))
                    .border_t_1()
                    .border_color(t.border)
                    .children(stack_rows(&card.changes, t, compact)),
            )
        })
        .when(!card.conflicts.is_empty(), |column| {
            column.child(
                div()
                    .px(sp(3.))
                    .pb(sp(2.))
                    .typeset(Type::CAPTION)
                    .text_color(t.red)
                    .child(format!(
                        "Conflicts in {}: the next turn resolves them.",
                        card.conflicts.join(", ")
                    )),
            )
        })
}
