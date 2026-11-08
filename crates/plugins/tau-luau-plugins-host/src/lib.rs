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
pub mod chats;
mod half;
pub mod hegel;
pub mod jobs;

/// The Luau plugins tau comes with (ADR 0048, 0050).
pub const BUILT_IN: &[registry::BuiltIn] = &[
    registry::BuiltIn {
        name: tau_todo::NAME,
        files: tau_todo::FILES,
    },
    registry::BuiltIn {
        name: tau_titles::NAME,
        files: tau_titles::FILES,
    },
    registry::BuiltIn {
        name: tau_compaction_chain::NAME,
        files: tau_compaction_chain::FILES,
    },
    registry::BuiltIn {
        name: tau_watcher::NAME,
        files: tau_watcher::FILES,
    },
    registry::BuiltIn {
        name: tau_reasoning::NAME,
        files: tau_reasoning::FILES,
    },
    registry::BuiltIn {
        name: tau_goal::NAME,
        files: tau_goal::FILES,
    },
];

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
