//! tau-luau-plugins' host half
//! ([ADR 0027](../../../docs/decisions/0027-luau-plugins.md),
//! ADR 0030): reading a plugins folder, loading each plugin, and
//! running its hooks in a fresh codemode VM. What a plugin declares, and
//! how every interface draws it, is `tau-luau-plugins`'.
//!
//! - `runtime`: reading a folder, loading it, calling its hooks.
//! - `agent`: the active plugins, as one agent plugin of a run.
//! - `registry`: the repository's plugins, as the host keeps them.

pub mod agent;
pub mod archive;
pub mod built_in;
pub mod chats;
pub mod files;
pub mod forms;
mod half;
pub mod hegel;
pub mod infer;
pub mod jobs;

pub mod parity;
pub mod registry;
pub mod rewrite;
pub mod runtime;
pub mod skill;
pub mod slots;
pub mod store;
pub mod testing;

pub use half::LuauPluginsHost;
// What a plugin declares, from the interface half, by the paths the
// host half has always used.
use tau_luau_plugins::{
    ActionDone,
    Declaration,
    Entry,
    NAME,
    Overview,
    REPO,
    Record,
    SKILL,
    SettingsPage,
    Standing,
    TAU_MODULE,
    TestResult,
    ToolSpec,
    settings,
};
