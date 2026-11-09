//! Every integration test of tau-agents-host, in one binary: linking one
//! rather than one per file is most of what building the tests
//! costs (docs/reference/testing.md, "One binary per crate").

mod agents;
mod registry;
mod resources;
mod sandbox;
mod skill;
mod streams;
mod talk;
