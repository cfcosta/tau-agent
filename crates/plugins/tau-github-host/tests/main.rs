//! Every integration test of tau-github-host, in one binary: linking one
//! rather than one per file is most of what building the tests
//! costs (docs/reference/testing.md, "One binary per crate").

mod support;

mod api;
mod sign_in;
mod url;
