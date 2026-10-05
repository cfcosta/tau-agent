//! tau-vcs's interface half: the cards of its tools in a run's transcript,
//! and the shapes they read (`docs/reference/vcs.md`). The tools
//! themselves, on jj-lib, are `tau-vcs-host`'s; a phone draws their
//! cards without them (ADR 0030).

pub mod details;
pub mod ui;

pub use details::{ChangeInfo, ChangeKind, FileChange, Landing, TooLarge};
pub use ui::VcsUi;
