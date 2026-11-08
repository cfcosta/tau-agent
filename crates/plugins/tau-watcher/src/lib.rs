//! tau-watcher (docs/reference/watcher.md, ADR 0032, 0050): every 6th
//! step of a run, a side request reads the conversation and answers
//! `learn: none`, or offers one short note the person likely missed. It
//! is a Luau plugin tau comes with: its code is the Luau under `plugins/tau-watcher/`,
//! which tau-luau-plugins loads as always active; this crate carries the
//! files, the views the demo draws, and the tests that check them.

/// The plugin's name, and its folder's.
pub const NAME: &str = "tau-watcher";

/// Its files, by their paths in its folder.
pub const FILES: &[(&str, &str)] = &[
    (
        "tau.toml",
        include_str!("../../../../plugins/tau-watcher/tau.toml"),
    ),
    (
        "src/plugin.luau",
        include_str!("../../../../plugins/tau-watcher/src/plugin.luau"),
    ),
    (
        "src/watch.luau",
        include_str!("../../../../plugins/tau-watcher/src/watch.luau"),
    ),
    (
        "tests/watch.luau",
        include_str!("../../../../plugins/tau-watcher/tests/watch.luau"),
    ),
    (
        "tests/plugin.luau",
        include_str!("../../../../plugins/tau-watcher/tests/plugin.luau"),
    ),
];

/// What the demo shows of a note: a run's state, and the view the plugin
/// draws from it, as JSON (`{ state, view }`), new, opened and known.
/// The demo cannot run Luau; `tests/luau.rs` checks each view is what the
/// plugin draws.
pub const DEMO_NEW: &str =
    include_str!("../../../../plugins/tau-watcher/fixtures/new.json");
pub const DEMO_LEARNED: &str =
    include_str!("../../../../plugins/tau-watcher/fixtures/learned.json");
pub const DEMO_KNEW: &str =
    include_str!("../../../../plugins/tau-watcher/fixtures/knew.json");
