//! Every integration test of tau-codemode, in one binary: linking one
//! rather than one per file is most of what building the tests
//! costs (docs/reference/testing.md, "One binary per crate").

mod definitions;
mod format;
mod modules;
mod repository_pins;
mod result;
mod store;
