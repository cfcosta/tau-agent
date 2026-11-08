//! tau-goal (docs/reference/goal.md, ADR 0050): keeps a conversation
//! going until its `/goal` holds, checked with Jev each time the model
//! would stop, while its continuations and budget last. It is a Luau
//! plugin tau comes with: its code is the Luau under `plugins/tau-goal/`, which
//! tau-luau-plugins loads as always active; this crate carries the
//! files, the goals the demo shows, and the tests that check them.

/// The plugin's name, and its folder's.
pub const NAME: &str = "tau-goal";

/// Its files, by their paths in its folder.
pub const FILES: &[(&str, &str)] = &[
    (
        "tau.toml",
        include_str!("../../../../plugins/tau-goal/tau.toml"),
    ),
    (
        "src/plugin.luau",
        include_str!("../../../../plugins/tau-goal/src/plugin.luau"),
    ),
    (
        "src/goal.luau",
        include_str!("../../../../plugins/tau-goal/src/goal.luau"),
    ),
    (
        "tests/command.luau",
        include_str!("../../../../plugins/tau-goal/tests/command.luau"),
    ),
    (
        "tests/plugin.luau",
        include_str!("../../../../plugins/tau-goal/tests/plugin.luau"),
    ),
];

/// The goal of the demo's run.
pub const DEMO_GOAL: &str =
    "All tests in tau-ai pass and cargo clippy is clean";

/// What the demo shows of a goal: a run's state, and the view the plugin
/// draws from it, as JSON (`{ state, view }`). [`DEMO_GOAL`] working
/// after two checks that did not hold, met at the third, and stopped out
/// of continuations; and two stored chats' goals, met and stopped. The
/// demo cannot run Luau; `tests/luau.rs` checks each is what the plugin
/// leaves and draws.
pub const DEMO_WORKING: &str =
    include_str!("../../../../plugins/tau-goal/fixtures/working.json");
pub const DEMO_MET: &str =
    include_str!("../../../../plugins/tau-goal/fixtures/met.json");
pub const DEMO_STOPPED: &str =
    include_str!("../../../../plugins/tau-goal/fixtures/stopped.json");
pub const DEMO_TRIAGE: &str =
    include_str!("../../../../plugins/tau-goal/fixtures/triage.json");
pub const DEMO_LANES: &str =
    include_str!("../../../../plugins/tau-goal/fixtures/lanes.json");

/// What the plugin declares, as the demo lists it among tau's Luau
/// plugins.
pub const DEMO_DECLARATION: &str =
    include_str!("../../../../plugins/tau-goal/fixtures/declaration.json");
