//! tau-agents' host half
//! ([ADR 0037](../../../docs/decisions/0037-agents.md), ADR 0030):
//! each agent's `agent.luau`, read from its repository's trunk and
//! tested, and what wakes it. What every interface shows of an agent is
//! `tau-agents`'.
//!
//! - [`Agents`]: the agents on this host: their events, queued and
//!   delivered to their conversations, and what the interface reads.
//! - `registry`: each agent's versions, and the one that is live.
//! - `schedule`: when a schedule comes due.
//! - `poll`: what a poll fires, once per new item.
//! - `stream`: a stream's connection and its frames.
//! - `state`: each agent's queue and schedules, kept across restarts.
//! - [`Plan`]: what an agent schedules for itself from its conversation.
//! - `tools`: what an agent's runs have to reach the person and other
//!   agents.
//! - `artifacts`: an event's data too large for its message.
//! - `env`: what the agents reach of the host.
//! - `sandbox`: where an agent's commands run.
//! - `clock`: the host's time, and its local days.
//! - `skill`: the `tau-agents` skill tau ships.

mod agents;
pub mod artifacts;
pub mod env;
mod half;
pub mod poll;
pub mod registry;
pub mod resource;
pub mod sandbox;
pub mod schedule;
pub mod skill;
pub mod state;
pub mod stream;
pub mod tools;

pub use agents::{
    Agents,
    SAVE_SOON,
    scheduled::{MAX_SCHEDULED, Made, Plan},
    talk::{MAX_HOPS, MAX_MESSAGE, MAX_TEXT, hops_of},
};
pub use half::AgentsHost;
pub use tau_agents::{NAME, ROOT, SKILL};
use tau_luau_plugins_host::runtime::Root;
/// The host's clock and local days, which tau-schedule keeps for chats
/// and agents alike.
pub use tau_schedule::clock;

/// An agent's repository as a folder: `agent.luau`, required as
/// `agent`, with its modules under `lib/` and its tests under `tests/`.
pub const AGENT: Root = Root {
    file: tau_agents::ENTRY,
    module: "agent",
    subject: "_agent",
};

/// Writes `text` to `path` whole or not at all: to a file beside it,
/// then over it. It blocks: call it from `spawn_blocking`.
pub(crate) fn write_atomic(
    path: &std::path::Path,
    text: &str,
) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let partial = path.with_extension("partial");
    std::fs::write(&partial, text)?;
    std::fs::rename(partial, path)
}
