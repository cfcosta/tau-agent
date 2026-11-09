//! Every integration test of tau-memory-e2e, in one binary: linking one
//! rather than one per file is most of what building the tests
//! costs (docs/reference/testing.md, "One binary per crate").

mod access;
mod metrics;
mod runner;
mod scenarios;
