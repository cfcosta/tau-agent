//! tau-titles's Luau loads as a plugin and passes its own tests, as tau loads
//! it.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use tau_luau_plugins_host::runtime::{Files, PLUGIN, load};
use tau_testing::block_on_io;
use tokio_util::sync::CancellationToken;

#[test]
fn the_plugin_loads_and_passes_its_tests() {
    let paths: Vec<(String, Vec<u8>)> = tau_titles::FILES
        .iter()
        .map(|(path, source)| ((*path).to_owned(), source.as_bytes().to_vec()))
        .collect();
    let files = Files::from_paths(&paths, PLUGIN).unwrap();
    let loaded = block_on_io(load(tau_titles::NAME, files)).unwrap();
    assert_eq!(loaded.declaration.name, tau_titles::NAME);
    let tests = block_on_io(loaded.test(CancellationToken::new()));
    assert!(!tests.is_empty());
    for test in tests {
        assert!(test.passed, "{test:?}");
    }
}
