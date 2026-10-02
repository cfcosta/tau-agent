//! Deterministic Codemode workloads, independent of any implementation.
//!
//! Files are inputs; `expected` is the answer the evaluator must check.
//! Nothing here calls a provider. Large logs place evidence at the start,
//! middle, and end so a head/tail display cannot substitute for full access.

pub mod fixtures;
pub mod matrix;
pub mod runner;
