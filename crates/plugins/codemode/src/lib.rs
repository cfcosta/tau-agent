//! Codemode (`docs/reference/codemode.md`): a tool whose input is a
//! Luau script. The script runs in a sandbox in the harness and calls
//! the run's other tools, and Jev, as functions.
//!
//! - [`options::parse`] splits the `-- @options:` line off the source.
//! - [`value`] maps JSON to Luau and back.
//! - [`store`] folds the store's records and keeps a script's writes.
//! - [`result`] turns what a script left into the result the model
//!   reads.

pub mod image;
pub mod options;
pub mod result;
pub mod store;
pub mod value;

pub use options::{Options, Source, SourceError};
pub use result::{CallRow, CallStatus, Failure, Item, Outcome, Rendered};
