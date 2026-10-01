//! Codemode (`docs/reference/codemode.md`): a tool whose input is a
//! Luau script. The script runs in a sandbox in the harness and calls
//! the run's other tools, and Jev, as functions: it chains calls, runs
//! them in parallel, and filters large results before the model reads
//! them.
//!
//! The engine reaches the run only through [`Host`], so tests can wire
//! it to fakes; [`plugin`] wires it to `ToolCtx::call` as the
//! [`Codemode`] plugin.
//!
//! - [`options::parse`] splits the `-- @options:` line off the source.
//! - [`run`] runs a script in a fresh VM and returns an [`Outcome`].
//! - [`Outcome::render`] turns it into the result the model reads.
//! - [`signature`] renders tools as Luau signatures and picks those
//!   that fit the run's context; [`description`] is the tool's text.
//! - [`live`] is what a call reports while its script runs: its Jev
//!   requests, which make no run events of their own.
//! - [`store`] folds the store's records and keeps a script's writes.
//! - [`Codemode`] is the plugin, and [`CodemodeTool`] its tool.
//! - [`CodemodeUi`] is the plugin with its UI (ADR 0017): its card, its
//!   store in the inspector, and its entry on the Plugins screen.

pub mod description;
mod engine;
pub mod host;
pub mod image;
pub mod jev;
pub mod live;
pub mod options;
pub mod plugin;
pub mod result;
pub mod search;
pub mod signature;
pub mod store;
pub mod ui;
pub mod value;

pub use engine::{
    MAX_OUTPUT_BYTES,
    MEMORY_LIMIT,
    Request,
    YIELD_EVERY,
    error_text,
    run,
};
pub use host::{Host, Namespace, ToolCall, ToolEntry};
pub use options::{Options, Source, SourceError};
pub use plugin::{Codemode, CodemodeTool, PLUGIN};
pub use result::{CallRow, CallStatus, Failure, Item, Outcome, Rendered};
pub use signature::describe;
pub use tokio_util::sync::CancellationToken;
pub use ui::CodemodeUi;
