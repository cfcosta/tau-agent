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
//! - `state`: each agent's queue and schedules, kept across restarts.
//! - `env`: what the agents reach of the host.
//! - `clock`: the host's time, and its local days.
//! - `skill`: the `tau-agents` skill tau ships.

mod agents;
pub mod clock;
pub mod env;
mod half;
pub mod registry;
pub mod schedule;
pub mod skill;
pub mod state;

pub use agents::Agents;
pub use half::AgentsHost;
pub use tau_agents::{NAME, ROOT, SKILL};
use tau_luau_plugins_host::runtime::Root;

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
