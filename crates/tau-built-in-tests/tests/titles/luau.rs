//! tau-titles's Luau loads as a plugin and passes its own tests, as tau loads
//! it.

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
        tau_luau_plugins_host::built_in::files(tau_luau_plugins::names::TITLES)
            .unwrap();
    let loaded =
        block_on_io(load(tau_luau_plugins::names::TITLES, files)).unwrap();
    assert_eq!(loaded.declaration.name, tau_luau_plugins::names::TITLES);
    let tests = block_on_io(loaded.test(CancellationToken::new()));
    assert!(!tests.is_empty());
    for test in tests {
        assert!(test.passed, "{test:?}");
    }
}
