//! Codemode (`docs/reference/codemode.md`): a tool whose input is a
//! Luau script. The script runs in a sandbox in the harness and calls
//! the run's other tools, and Jev, as functions.
//!
//! - [`options::parse`] splits the `-- @options:` line off the source.
//! - [`value`] maps JSON to Luau and back.
//! - [`store`] folds the store's records and keeps a script's writes.
//! - [`result`] turns what a script left into the result the model
//!   reads.
//! - [`signature`] renders tools as Luau signatures and picks those
//!   that fit the run's context; [`description`] is the tool's text.

pub mod description;
pub mod host;
pub mod image;
pub mod options;
pub mod result;
pub mod signature;
pub mod store;
pub mod value;

pub use host::{Host, Namespace, ToolCall, ToolEntry};
pub use options::{Options, Source, SourceError};
pub use result::{CallRow, CallStatus, Failure, Item, Outcome, Rendered};
pub use signature::describe;
