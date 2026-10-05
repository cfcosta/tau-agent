//! Codemode's interface half (`docs/reference/codemode.md`): what a
//! `codemode` call's card shows, and the records it folds. The tool
//! itself, which runs Luau scripts in a sandbox, is
//! `tau-codemode-host`'s; a phone draws its cards without it (ADR 0030).
//!
//! - [`live`] is what a call reports while its script runs: its Jev
//!   requests, which make no run events of their own.
//! - [`store`] folds the store's records and keeps a script's writes.
//! - [`Outcome`] is what a script returned, as its card reads it.
//! - [`format::formatted`] is a script as the cards show it, and
//!   [`outline`] its output.
//! - [`CodemodeUi`] is the plugin with its UI (ADR 0017): its card and
//!   its store in the inspector.

pub mod description;
pub mod format;
pub mod image;
pub mod inference_trace;
pub mod live;
pub mod modules;
pub mod outline;
pub mod promotion;
pub mod result;
pub mod store;
pub mod ui;

pub use description::PLUGIN;
pub use result::{CallRow, CallStatus, Failure, Item, Outcome, Rendered};
pub use ui::CodemodeUi;
