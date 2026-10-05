//! Chats that wait to land on a busy main chat, and conflicts a
//! resolving turn left on main (ADR 0024).
//!
//! The host decides when a queued chat lands; the interface shows what
//! it says, on the main chat's view ([`RunView::landing_queue`],
//! [`RunView::main_conflicts`]), so a phone shows the same.
//!
//! [`RunView::landing_queue`]: crate::view::RunView::landing_queue
//! [`RunView::main_conflicts`]: crate::view::RunView::main_conflicts

use serde::{Deserialize, Serialize};

/// A finished chat waiting to land on its repository's main chat.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Waiting {
    /// The chat's run id.
    pub run: String,
    pub title: String,
    /// The changes it brings, as last previewed.
    pub changes: usize,
    /// The files its landing would conflict in, as last previewed.
    pub conflicts: Vec<String>,
    /// The conflicting files the person saw and confirmed landing with.
    pub confirmed: Vec<String>,
    /// A sub-agent that ended with nobody waiting for it (ADR 0026),
    /// and how. It lands whatever it conflicts in, and tau's turn after
    /// the drain reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub_agent: Option<SubAgentEnd>,
}

/// How a sub-agent waiting in the queue ended.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubAgentEnd {
    /// The limit that cut it short, as `tau_vcs` names it, if one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<String>,
    /// Why it has nothing to land, if it has not: it failed, or its work
    /// could not be checked. It only gets reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<String>,
}

impl Waiting {
    /// It would conflict in a file the person did not confirm: it waits
    /// for them before it lands. A sub-agent never waits: main asked for
    /// its work.
    pub fn needs_confirmation(&self) -> bool {
        self.sub_agent.is_none()
            && self
                .conflicts
                .iter()
                .any(|file| !self.confirmed.contains(file))
    }
}

/// Conflicts left on a main chat's stack after its turn ended: nothing
/// lands on it, and no chat forks it, until they are resolved.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MainConflicts {
    /// The files in conflict, in path order.
    pub files: Vec<String>,
    /// The chat whose landing brought them, when tau knows it.
    pub from: Option<String>,
    /// The message tau's resolving turn started with: Resolve again
    /// sends it again.
    pub prompt: String,
    /// The person said they will write to main: the card goes, the
    /// mark stays until main is clean.
    pub dismissed: bool,
}

/// What a resolving turn is told when it would stop with conflicts
/// still on main, and what Resolve again asks when tau knows no better.
pub fn conflicts_remain(files: &[String]) -> String {
    format!(
        "Conflicts remain in {}. Resolve them and commit.",
        files.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chat_needs_confirmation_only_for_files_not_confirmed() {
        let waiting = |conflicts: &[&str], confirmed: &[&str]| Waiting {
            conflicts: conflicts.iter().map(|f| f.to_string()).collect(),
            confirmed: confirmed.iter().map(|f| f.to_string()).collect(),
            ..Waiting::default()
        };
        assert!(!waiting(&[], &[]).needs_confirmation());
        assert!(!waiting(&["a.rs"], &["a.rs", "b.rs"]).needs_confirmation());
        assert!(waiting(&["a.rs", "c.rs"], &["a.rs"]).needs_confirmation());
    }

    #[test]
    fn the_hold_names_the_files() {
        assert_eq!(
            conflicts_remain(&["a.rs".into(), "b.rs".into()]),
            "Conflicts remain in a.rs, b.rs. Resolve them and commit."
        );
    }
}
