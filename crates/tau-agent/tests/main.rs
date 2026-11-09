//! Every integration test of tau-agent, in one binary: linking one
//! rather than one per file is most of what building the tests
//! costs (docs/reference/testing.md, "One binary per crate").

mod common;

mod agent_loop;
mod compile;
mod context;
mod driver;
mod examples;
mod limits;
mod loop_model;
mod nested;
mod output;
mod plugin;
mod records;
mod retry;
mod schema;
mod store_failure;
mod tool;
mod validation;
mod workflow;
