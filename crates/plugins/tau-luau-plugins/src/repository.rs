//! tau's own plugins repository (ADR 0027): where it lives, and what it
//! starts with.

/// The plugins repository's directory under tau's data directory.
pub const ROOT: &str = "luau-plugins";

/// What the repository starts with: what it is for, and the instructions
/// runs in it read.
pub fn first_files() -> Vec<(String, String)> {
    vec![
        ("README.md".into(), README.into()),
        ("AGENTS.md".into(), AGENTS.into()),
    ]
}

const README: &str = "# tau's plugins\n\n\
    Plugins written in Luau, one folder each: `plugin.luau`, modules under \
    `lib/`, tests under `tests/`, and a README. A plugin works in the chat \
    that writes it as soon as its tests pass, and everywhere once it lands \
    on `main`; one that reaches further than the version you allowed \
    waits for you on the Plugins screen.\n";

const AGENTS: &str = "# Writing tau's plugins\n\n\
    This repository holds tau's Luau plugins. Load the `tau-plugins` skill \
    before you write or change one: it has the interface, the view pieces \
    and how to test. Run a plugin's tests with `plugin_test` before you \
    commit. A plugin is active once its commit is on `main`.\n";
