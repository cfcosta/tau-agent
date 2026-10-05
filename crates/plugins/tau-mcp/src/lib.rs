//! tau-mcp's interface half (`docs/reference/mcp.md`): the Servers page,
//! MCP tools' cards, and what they read. Connecting to servers and adding
//! their tools to a run is `tau-mcp-host`'s (ADR 0030).
//!
//! - [`config`]: the `mcpServers` format and where servers come from.
//! - [`names`]: the tools' names and the servers' namespaces.
//! - [`prompts`]: servers' prompts as composer commands.
//! - [`ui`]: the plugin with its UI, [`McpUi`] (ADR 0017): the Servers
//!   page and MCP tools' cards.

pub mod config;
#[cfg(feature = "demo")]
pub mod demo;
pub mod info;
pub mod names;
pub mod prompts;
pub mod ui;

pub use ui::McpUi;

/// The name the plugin goes by in events and errors.
pub const NAME: &str = "tau-mcp";
