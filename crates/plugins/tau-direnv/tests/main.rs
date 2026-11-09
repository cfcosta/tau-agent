//! Every integration test of tau-direnv, in one binary: linking one
//! rather than one per file is most of what building the tests
//! costs (docs/reference/testing.md, "One binary per crate").

mod host;
mod real_direnv;
