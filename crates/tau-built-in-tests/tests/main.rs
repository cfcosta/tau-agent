//! The tests of tau's built-in Luau plugins that need Rust (ADR 0051, ADR
//! 0053): every plugin's own tests and fixtures as `tau-plugin test` runs
//! them, and what Luau cannot say, such as a real loop, the real host and
//! the store under a fuse. One crate, one binary. Everything is read from
//! the folder at run time, so a Luau edit compiles nothing.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

mod binaries;
mod built_ins;
mod constitution;
mod package;
mod reasoning;
mod schedule;
mod todo;
