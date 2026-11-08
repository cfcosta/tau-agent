//! tau-reasoning (docs/reference/plugins.md, ADR 0050): on auto, Jev
//! picks each message's reasoning effort as the run starts and, with
//! `redecide`, again when the effort's lease ends. An effort someone
//! chose stands, and a long conversation keeps the effort it last ran
//! at, since a change resends its prefix uncached. It is a Luau plugin
//! tau comes with: its code is the Luau under `luau/`, which
//! tau-luau-plugins loads as always active; this crate carries the
//! files, the views the demo draws, the replay example, and the tests
//! that check them.

/// The plugin's name, and its folder's.
pub const NAME: &str = "tau-reasoning";

/// Its files, by their paths in its folder.
pub const FILES: &[(&str, &str)] = &[
    ("plugin.luau", include_str!("../luau/plugin.luau")),
    ("lib/effort.luau", include_str!("../luau/lib/effort.luau")),
    (
        "tests/reasoning.luau",
        include_str!("../luau/tests/reasoning.luau"),
    ),
];

/// What the demo shows of a run's effort: its state, and the view the
/// plugin draws from it, as JSON (`{ state, view }`): high picked as
/// the run starts, and medium kept over Jev's high for a 48k-token
/// prefix. The demo cannot run Luau; `tests/luau.rs` checks each is what
/// the plugin leaves and draws.
pub const DEMO_CHOSE: &str = include_str!("../fixtures/chose.json");
pub const DEMO_KEPT: &str = include_str!("../fixtures/kept.json");

/// What the plugin declares, as the demo lists it among tau's Luau
/// plugins.
pub const DEMO_DECLARATION: &str = include_str!("../fixtures/declaration.json");
