//! Every integration test of tau-luau-plugins-host, in one binary: linking one
//! rather than one per file is most of what building the tests
//! costs (docs/reference/testing.md, "One binary per crate").

mod agent;
mod agents;
mod desktop;
mod dispatch;
mod files;
mod forge_urls;
mod forms;
mod infer;
mod install;
mod machines;
mod package;
mod parity;
mod properties;
mod registry;
mod rewrite;
mod runtime;
mod skill;
mod store;
mod writes;
