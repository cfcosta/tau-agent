//! A child run's landing on its parent (ADR 0009): the changes it
//! brings, as the kernel's landing preview and landed card show them.

use tau_ui_plugin::{ViewCx, points::AtChanges, tree::Node};

use super::{VcsUi, change_log::Change, log_card::stack};

/// The changes, as they sit on the stack.
pub fn changes(at: &AtChanges, view: &mut ViewCx<'_, VcsUi>) -> Option<Node> {
    let changes: Vec<Change> =
        at.changes.iter().cloned().map(Change::new).collect();
    Some(stack(&changes, view.compact))
}
