//! Every integration test of tau-memory-host, in one binary: linking one
//! rather than one per file is most of what building the tests
//! costs (docs/reference/testing.md, "One binary per crate").

mod common;

mod colbert;
mod eval;
mod import;
mod importers;
mod memory;
mod note;
mod plugin;
mod recall;
mod safety;
mod store;
mod ui;
