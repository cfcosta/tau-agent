//! Every integration test of tau-ui-plugin, in one binary: linking one
//! rather than one per file is most of what building the tests
//! costs (docs/reference/testing.md, "One binary per crate").

mod board;
mod outside_resources;
mod outside_tools;
