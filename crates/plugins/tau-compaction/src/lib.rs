//! tau-compaction (docs/reference/compaction.md, ADR 0050): when a run's
//! context nears the model's window, the older messages are summarized
//! and replaced by the summary, the newest kept as they are; an overflow
//! summarizes at once, and an idle chat is summarized in its own
//! conversation before its prompt cache lapses. It is a port of pi's
//! coding-agent compaction, and a Luau plugin tau comes with: its code
//! is the Luau under `luau/`, which tau-luau-plugins loads as always
//! active; this crate carries the files and the tests that check them.

/// The plugin's name, and its folder's.
pub const NAME: &str = "tau-compaction";

/// Its files, by their paths in its folder.
pub const FILES: &[(&str, &str)] = &[
    ("plugin.luau", include_str!("../luau/plugin.luau")),
    (
        "lib/compaction.luau",
        include_str!("../luau/lib/compaction.luau"),
    ),
    (
        "tests/plugin.luau",
        include_str!("../luau/tests/plugin.luau"),
    ),
    ("tests/rules.luau", include_str!("../luau/tests/rules.luau")),
];

/// What the plugin declares, as the demo lists it among tau's Luau
/// plugins.
pub const DEMO_DECLARATION: &str = include_str!("../fixtures/declaration.json");
