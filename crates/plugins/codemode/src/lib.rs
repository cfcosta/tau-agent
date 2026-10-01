//! Codemode (`docs/reference/codemode.md`): a tool whose input is a
//! Luau script. The script runs in a sandbox in the harness and calls
//! the run's other tools, and Jev, as functions.
//!
//! - [`options::parse`] splits the `-- @options:` line off the source.

pub mod options;

pub use options::{Options, Source, SourceError};
