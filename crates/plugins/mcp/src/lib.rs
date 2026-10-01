//! Connects a tau agent to MCP servers and adds their tools
//! (`docs/reference/mcp.md`).
//!
//! - [`config`]: the `mcpServers` format and where servers come from.
//! - [`names`]: the tools' names and the servers' namespaces.
//! - [`results`]: what a call's result becomes for the model and for
//!   scripts.
//! - [`connection`]: one connection per server, shared by every run;
//!   only its `client` module touches `rmcp`.

pub mod config;
pub mod names;
pub mod results;

mod client;
pub mod connection;
