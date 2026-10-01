//! Connects a tau agent to MCP servers and adds their tools
//! (`docs/reference/mcp.md`).
//!
//! - [`config`]: the `mcpServers` format and where servers come from.
//! - [`names`]: the tools' names and the servers' namespaces.
//! - [`results`]: what a call's result becomes for the model and for
//!   scripts.
//! - [`connection`]: one connection per server, shared by every run;
//!   only its `client` module touches `rmcp`.
//! - [`resources`]: servers' resources as `list_mcp_resources`,
//!   `list_mcp_resource_templates` and `read_mcp_resource`.
//! - [`pool`]: connections kept across changes to the servers, for the
//!   host.
//! - [`ui`]: the plugin with its UI, [`McpUi`] (ADR 0017): the Servers
//!   page, MCP tools' cards, and one plugin per repository on the host.

pub mod config;
pub mod names;
pub mod results;

mod client;
pub mod connection;
mod plugin;
pub mod pool;
pub mod resources;
pub mod tool;
pub mod ui;

pub use plugin::{
    DESCRIPTION_LIMIT,
    McpPlugin,
    McpPluginBuilder,
    NAME,
    SERVERS_INTRO,
    SERVERS_LIMIT,
    STARTUP_WAIT,
    servers_block,
};
pub use ui::McpUi;
