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
//!   that fit the run's context; [`tau_codemode::description`] is the
//!   tool's text.
//! - [`tau_codemode::live`] is what a call reports while its script
//!   runs: its Jev requests, which make no run events of their own.
//! - [`tau_codemode::store`] folds the store's records and keeps a
//!   script's writes.
//! - [`Codemode`] is the plugin, and [`CodemodeTool`] its tool.
//! - [`tau_codemode::format::formatted`] is a script as the cards show
//!   it, and [`tau_codemode::outline`] its output.
//! - [`CodemodeHost`] is its host half (ADR 0030): the tool for each
//!   run, its entry on the Plugins screen, and what its card asks. Its
//!   card and the records it folds are `tau-codemode`'s.

mod engine;
mod half;
pub mod host;
pub mod inference;
pub mod inference_budget;
pub mod jev;
pub mod json;
mod module_tests;
mod module_tools;
pub mod options;
pub mod plugin;
pub mod repository_modules;
pub mod search;
pub mod signature;
pub mod value;

pub use engine::{
    MAX_OUTPUT_BYTES,
    MEMORY_LIMIT,
    Request,
    YIELD_EVERY,
    error_text,
    run,
};
pub use half::CodemodeHost;
pub use host::{Host, Namespace, ToolCall, ToolEntry, ToolReply};
pub use options::{Options, Source, SourceError};
pub use plugin::{Codemode, CodemodeTool};
pub use signature::describe;
// What a script returns, as the interface half reads it.
pub use tau_codemode::{
    CallRow,
    CallStatus,
    Failure,
    Item,
    Outcome,
    PLUGIN,
    Rendered,
};
use tau_codemode::{
    description,
    image,
    inference_trace,
    live,
    modules,
    promotion,
    result,
    store,
};
pub use tokio_util::sync::CancellationToken;
