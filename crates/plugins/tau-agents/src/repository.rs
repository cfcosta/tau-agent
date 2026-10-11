//! An agent's repository (ADR 0037): where agents live, and what each
//! starts with.

/// The agents' directory under tau's data directory: one repository
/// per agent, named for it.
pub const ROOT: &str = "agents";

/// What an agent's repository starts with: instructions it replaces
/// with its own once it knows what it is for.
pub fn first_files() -> Vec<(String, String)> {
    vec![("AGENTS.md".into(), AGENTS.into())]
}

const AGENTS: &str = "# An agent\n\n\
    This repository is an agent's own. Its first message says what it is \
    for; it then writes these instructions, and the `agent.luau` that says \
    what wakes it, itself.\n";
