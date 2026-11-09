//! tau-todo's Luau loads as a plugin and passes its own tests, as tau
//! loads it: its examples, and the issue graph's laws, which are
//! properties on hegel's engine (ADR 0048, "1. The issue graph"; ADR
//! 0049).

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use tau_luau_plugins_host::runtime::load;
use tau_testing::block_on_io;
use tokio_util::sync::CancellationToken;

#[test]
fn the_plugin_loads_and_passes_its_tests() {
    let files =
        tau_luau_plugins_host::built_in::files(tau_luau_plugins::names::TODO)
            .unwrap();
    let loaded =
        block_on_io(load(tau_luau_plugins::names::TODO, files)).unwrap();
    assert_eq!(loaded.declaration.name, tau_luau_plugins::names::TODO);
    let tests = block_on_io(loaded.test(CancellationToken::new()));
    assert!(!tests.is_empty());
    for test in &tests {
        assert!(test.passed, "{test:?}");
    }
    let graph: Vec<&str> = tests
        .iter()
        .filter(|test| test.file == "graph")
        .map(|test| test.name.as_str())
        .collect();
    assert_eq!(
        graph,
        [
            "every move is in the table",
            "ready and blocked are the model's",
            "a blocks cycle is refused with its path",
            "an epic counts its children",
            "ids never repeat",
            "an export round trips",
        ]
    );
}
