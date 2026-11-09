//! Every integration test of tau-remote, in one binary: linking one
//! rather than one per file is most of what building the tests
//! costs (docs/reference/testing.md, "One binary per crate").

mod feed;
mod network_model;
mod remote;
