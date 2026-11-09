//! tau-tree-compaction's Luau loads as a plugin and passes its own tests, as
//! tau loads it; and what the demo lists of it is what it declares.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use serde_json::Value;
use tau_luau_plugins_host::runtime::{Loaded, load};
use tau_testing::block_on_io;
use tokio_util::sync::CancellationToken;

fn loaded() -> Loaded {
    let files = tau_luau_plugins_host::built_in::files(
        tau_luau_plugins::names::TREE_COMPACTION,
    )
    .unwrap();
    block_on_io(load(tau_luau_plugins::names::TREE_COMPACTION, files)).unwrap()
}

#[test]
fn the_plugin_loads_and_passes_its_tests() {
    let loaded = loaded();
    assert_eq!(
        loaded.declaration.name,
        tau_luau_plugins::names::TREE_COMPACTION
    );
    let tests = block_on_io(loaded.test(CancellationToken::new()));
    assert!(tests.len() > 20, "{tests:?}");
    for test in tests {
        assert!(test.passed, "{test:?}");
    }
}

/// What the demo lists of the plugin is what it declares.
/// `BLESS_FIXTURES=1` writes it anew from the plugin.
#[test]
fn the_demo_lists_what_the_plugin_declares() {
    let declared = serde_json::to_value(&loaded().declaration).unwrap();
    if std::env::var_os("BLESS_FIXTURES").is_some() {
        let path = crate::fixture_path("tau-tree-compaction", "declaration");
        let text = serde_json::to_string_pretty(&declared).unwrap() + "\n";
        std::fs::write(path, text).unwrap();
        return;
    }
    let fixture: Value = serde_json::from_str(crate::fixture(
        "tau-tree-compaction",
        "declaration",
    ))
    .unwrap();
    assert_eq!(fixture, declared, "run with BLESS_FIXTURES=1");
}
