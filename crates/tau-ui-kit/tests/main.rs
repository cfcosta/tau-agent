//! Every integration test of tau-ui-kit, in one binary: linking one
//! rather than one per file is most of what building the tests
//! costs (docs/reference/testing.md, "One binary per crate").

mod design;
mod diff;
mod format;
mod select;
mod syntax;
