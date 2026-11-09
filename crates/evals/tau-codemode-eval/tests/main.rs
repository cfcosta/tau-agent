//! Every integration test of tau-codemode-eval, in one binary: linking one
//! rather than one per file is most of what building the tests
//! costs (docs/reference/testing.md, "One binary per crate").

mod fixtures;
mod matrix;
mod runner;
