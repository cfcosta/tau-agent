//! tau-tools' interface half: the cards of its coding tools in a run's
//! transcript, and the shapes they read (`docs/reference/tools.md`). The
//! tools themselves are `tau-tools-host`'s; a phone draws their cards
//! without them (ADR 0030).
//!
//! The `terminal` feature draws `bash`'s output as a terminal, through
//! tau-terminal (libghostty-vt)
//! (`docs/decisions/0010-terminal-rendering.md`).

pub mod artifact_grant;
pub mod details;
pub mod ui;

pub use ui::ToolsUi;
