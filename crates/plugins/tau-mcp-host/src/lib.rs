//! Connects a tau agent to MCP servers and adds their tools
//! (`docs/reference/mcp.md`): tau-mcp's host half. Its page, cards and
//! config format are `tau-mcp`'s (ADR 0030).
//!
//! - [`results`]: what a call's result becomes for the model and for
//!   scripts.
//! - [`connection`]: one connection per server, shared by every run;
//!   only its `client` module and [`auth`] touch `rmcp`.
//! - [`auth`]: signing in to servers with OAuth, and the grants file.
//! - [`resources`]: servers' resources as `list_mcp_resources`,
//!   `list_mcp_resource_templates` and `read_mcp_resource`.
//! - [`pool`]: connections kept across changes to the servers, for the
//!   host.
//! - [`McpHost`]: the plugin's host half: one plugin per repository on
//!   the host, and what the Servers page asks.

pub mod auth;
mod client;
pub mod connection;
mod half;
mod plugin;
pub mod pool;
pub mod prompts;
pub mod resources;
pub mod results;
pub mod tool;

pub use half::{Host, McpHost, RunServers, apply};
pub use plugin::{
    DESCRIPTION_LIMIT,
    McpPlugin,
    McpPluginBuilder,
    SERVERS_INTRO,
    SERVERS_LIMIT,
    STARTUP_WAIT,
    servers_block,
};
// The interface half's shapes, by the paths the host half has always
// used.
pub use tau_mcp::NAME;
use tau_mcp::{config, info, names};
