//! Notes what you missed (`docs/reference/watcher.md`, ADR 0032): every
//! 6th step of a run, a side request reads the transcript and answers
//! `learn: none`, or offers one short note the person likely missed. The
//! note is a record, drawn under the step that asked for it and, while
//! it is new, as a line above the composer.
//!
//! The reply parser, the cadence and the seen-list are plain functions
//! here; [`WatcherUi`] draws, and [`WatcherHost`] asks (ADR 0030: a host
//! half this light stays in the plugin's crate).

pub mod cadence;
pub mod host;
pub mod prompt;
pub mod record;
pub mod reply;
pub mod state;
pub mod ui;

pub use host::{WatcherHost, WatcherPlugin};
pub use record::{NAME, Record};
pub use state::State;
pub use ui::WatcherUi;
