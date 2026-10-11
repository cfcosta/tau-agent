//! tau-agents: agents, each a conversation that never ends in a
//! repository of its own, woken by the events it declares
//! ([ADR 0037](../../../docs/decisions/0037-agents.md)). This is its
//! interface half: what every interface shows of an agent. Loading an
//! agent's `agent.luau`, firing its triggers and starting its turns is
//! `tau-agents-host`'s (ADR 0030).

pub mod repository;

/// The directory under tau's data directory where agents live, one
/// repository each.
pub use repository::ROOT;

/// Whether `name` can name an agent, and so its repository: lower-case
/// letters, digits and dashes, starting with a letter, at most 40.
pub fn valid_name(name: &str) -> bool {
    (1..=40).contains(&name.len())
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}
