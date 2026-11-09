//! tau-watcher's Luau loads as a plugin and passes its own tests, as tau
//! loads it; and the views the demo shows are what the plugin draws.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::sync::Arc;

use serde_json::{Value, json};
use tau_luau_plugins_host::runtime::{Context, Hook, Loaded, NoReach, load};
use tau_testing::block_on_io;
use tokio_util::sync::CancellationToken;

fn loaded() -> Loaded {
    let files = tau_luau_plugins_host::built_in::files(
        tau_luau_plugins::names::WATCHER,
    )
    .unwrap();
    block_on_io(load(tau_luau_plugins::names::WATCHER, files)).unwrap()
}

#[test]
fn the_plugin_loads_and_passes_its_tests() {
    let loaded = loaded();
    assert_eq!(loaded.declaration.name, tau_luau_plugins::names::WATCHER);
    let tests = block_on_io(loaded.test(CancellationToken::new()));
    assert!(tests.len() > 10, "{tests:?}");
    for test in tests {
        assert!(test.passed, "{test:?}");
    }
}

/// What the plugin's `view` draws from `state`.
fn view_of(loaded: &Loaded, state: &Value) -> Value {
    let context = Context {
        run: json!({ "id": "demo", "kind": "main" }),
        now: json!({}),
        settings: json!({ "enabled": true, "family": "chat" }),
        state: state.clone(),
        secrets: None,
        store: None,
        job: None,
    };
    let outcome = block_on_io(loaded.call(
        &Hook::View,
        Value::Null,
        &context,
        Arc::new(NoReach),
        CancellationToken::new(),
    ));
    assert_eq!(outcome.error, None);
    outcome.value
}

/// Each view the demo shows is what the plugin draws from its state.
/// `BLESS_FIXTURES=1` writes them anew from the plugin.
#[test]
fn the_demo_views_are_what_the_plugin_draws() {
    let loaded = loaded();
    for (name, text) in [
        ("new", crate::fixture("tau-watcher", "new")),
        ("learned", crate::fixture("tau-watcher", "learned")),
        ("knew", crate::fixture("tau-watcher", "knew")),
    ] {
        let mut fixture: Value = serde_json::from_str(text).unwrap();
        let drawn = view_of(&loaded, &fixture["state"]);
        if std::env::var_os("BLESS_FIXTURES").is_some() {
            fixture["view"] = drawn;
            let path = crate::fixture_path("tau-watcher", name);
            let text = serde_json::to_string_pretty(&fixture).unwrap() + "\n";
            std::fs::write(path, text).unwrap();
            continue;
        }
        assert_eq!(fixture["view"], drawn, "{name}: run with BLESS_FIXTURES=1");
    }
}
