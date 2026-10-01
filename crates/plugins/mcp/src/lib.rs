//! Connects a tau agent to MCP servers and adds their tools
//! (`docs/reference/mcp.md`).
//!
//! - [`config`]: the `mcpServers` format and where servers come from.
//! - [`names`]: the tools' names and the servers' namespaces.
//! - [`results`]: what a call's result becomes for the model and for
//!   scripts.

pub mod config;
pub mod names;
pub mod results;
