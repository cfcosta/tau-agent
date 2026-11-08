//! tau-ask (docs/reference/ask.md, ADR 0019, ADR 0050): the agent asks
//! the person one to four questions and waits for the answers. It is a
//! Luau plugin tau comes with: its code is the Luau under `luau/`, which
//! tau-luau-plugins loads as always active; this crate carries the files,
//! what the demo shows of it, and the tests that check them.

/// The plugin's name, and its folder's.
pub const NAME: &str = "tau-ask";

/// Its files, by their paths in its folder.
pub const FILES: &[(&str, &str)] = &[
    ("plugin.luau", include_str!("../luau/plugin.luau")),
    ("lib/ask.luau", include_str!("../luau/lib/ask.luau")),
    ("lib/panel.luau", include_str!("../luau/lib/panel.luau")),
    ("lib/escaped.luau", include_str!("../luau/lib/escaped.luau")),
    ("tests/ask.luau", include_str!("../luau/tests/ask.luau")),
    (
        "tests/plugin.luau",
        include_str!("../luau/tests/plugin.luau"),
    ),
];

/// What the demo shows of tau-ask: a run's state and the view the plugin
/// draws from it, as JSON (`{ state, view }`). The demo's run waiting on
/// three questions, untouched, with a checklist's note being written,
/// and on the review; and another chat waiting on one question. The
/// demo cannot run Luau; `tests/luau.rs` checks each is what the plugin
/// leaves and draws.
pub const DEMO_ASK: &str = include_str!("../fixtures/ask.json");
pub const DEMO_NOTE: &str = include_str!("../fixtures/note.json");
pub const DEMO_REVIEW: &str = include_str!("../fixtures/review.json");
pub const DEMO_SWEEP: &str = include_str!("../fixtures/sweep.json");

/// What the plugin declares, as the demo lists it among tau's Luau
/// plugins.
pub const DEMO_DECLARATION: &str = include_str!("../fixtures/declaration.json");
