//! Every integration test of tau-mcp-host, in one binary: linking one
//! rather than one per file is most of what building the tests
//! costs (docs/reference/testing.md, "One binary per crate").

mod common;

mod auth;
mod connection;
mod lifecycle_model;
mod oauth;
mod plugin;
mod pool;
mod prompts;
mod resources;
mod results;
mod subscriptions;
mod ui;
