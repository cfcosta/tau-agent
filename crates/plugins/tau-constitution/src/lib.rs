//! tau-constitution (docs/reference/constitution.md, ADR 0050): checks a
//! run's tool calls and final answer against the repository's rules,
//! asking Jev how likely each rule is broken. A call past a rule's block
//! mark is refused with the rule, one between its marks runs, flagged
//! for a person, and a final answer that breaks a rule goes back. It is
//! a Luau plugin tau comes with: its code is the Luau under `plugins/tau-constitution/`,
//! which tau-luau-plugins loads as always active, and its rules live in
//! its store in each repository; this crate carries the files, what the
//! demo shows, and the tests that check them.

/// The plugin's name, and its folder's.
pub const NAME: &str = "tau-constitution";

/// Its files, by their paths in its folder.
pub const FILES: &[(&str, &str)] = &[
    (
        "tau.toml",
        include_str!("../../../../plugins/tau-constitution/tau.toml"),
    ),
    (
        "src/plugin.luau",
        include_str!("../../../../plugins/tau-constitution/src/plugin.luau"),
    ),
    (
        "src/rules.luau",
        include_str!("../../../../plugins/tau-constitution/src/rules.luau"),
    ),
    (
        "src/checks.luau",
        include_str!("../../../../plugins/tau-constitution/src/checks.luau"),
    ),
    (
        "src/page.luau",
        include_str!("../../../../plugins/tau-constitution/src/page.luau"),
    ),
    (
        "tests/rules.luau",
        include_str!("../../../../plugins/tau-constitution/tests/rules.luau"),
    ),
    (
        "tests/checks.luau",
        include_str!("../../../../plugins/tau-constitution/tests/checks.luau"),
    ),
    (
        "tests/page.luau",
        include_str!("../../../../plugins/tau-constitution/tests/page.luau"),
    ),
];

/// What the demo's run shows of its checks: the state the plugin leaves
/// and the view it draws from it, as JSON (`[{ state, view }]`), after
/// each check the run makes: an edit refused for R2, an edit that
/// passes, a command flagged for R1, the final answer sent back for R6,
/// and the answer that follows it. The demo cannot run Luau;
/// `tests/luau.rs` checks each is what the plugin leaves and draws.
pub const DEMO_RUN: &str =
    include_str!("../../../../plugins/tau-constitution/fixtures/run.json");

/// The rules page of the demo's tau-agent repository, as the plugin
/// draws it from its store, by the demo screen that shows it
/// (`{ name: tree }`): the rules and their counts, a rule being written
/// and tried, one saved without a place, rules that cannot be read, and
/// none, with no TypeSafe key.
pub const DEMO_PAGES: &str =
    include_str!("../../../../plugins/tau-constitution/fixtures/pages.json");

/// What the plugin declares, as the demo lists it among tau's Luau
/// plugins.
pub const DEMO_DECLARATION: &str = include_str!(
    "../../../../plugins/tau-constitution/fixtures/declaration.json"
);
