//! tau-fast-compaction's Luau loads as a plugin and passes its own tests, as
//! tau loads it; and what the demo lists of it is what it declares.

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
        tau_luau_plugins::names::FAST_COMPACTION,
    )
    .unwrap();
    block_on_io(load(tau_luau_plugins::names::FAST_COMPACTION, files)).unwrap()
}

#[test]
fn the_plugin_loads_and_passes_its_tests() {
    let loaded = loaded();
    assert_eq!(
        loaded.declaration.name,
        tau_luau_plugins::names::FAST_COMPACTION
    );
    let tests = block_on_io(loaded.test(CancellationToken::new()));
    assert!(tests.len() > 15, "{tests:?}");
    for test in tests {
        assert!(test.passed, "{test:?}");
    }
}

/// Writes `drawn` as the fixture `name` with `BLESS_FIXTURES=1`, else
/// checks it is `fixture`.
fn bless_or_check(name: &str, fixture: &str, drawn: &Value) {
    if std::env::var_os("BLESS_FIXTURES").is_some() {
        let path = crate::fixture_path("tau-fast-compaction", name);
        let text = serde_json::to_string_pretty(drawn).unwrap() + "\n";
        std::fs::write(path, text).unwrap();
        return;
    }
    let fixture: Value = serde_json::from_str(fixture).unwrap();
    assert_eq!(&fixture, drawn, "{name}: run with BLESS_FIXTURES=1");
}

/// What the demo lists of the plugin is what it declares.
/// `BLESS_FIXTURES=1` writes it anew from the plugin.
#[test]
fn the_demo_lists_what_the_plugin_declares() {
    let declared = serde_json::to_value(&loaded().declaration).unwrap();
    bless_or_check(
        "declaration",
        crate::fixture("tau-fast-compaction", "declaration"),
        &declared,
    );
}

/// What the plugin's `view` draws from `state`.
fn view_of(loaded: &Loaded, state: &Value) -> Value {
    let context = Context {
        run: json!({ "id": "demo", "kind": "main", "repo": "tau-agent", "jev": true }),
        now: json!({}),
        settings: json!({}),
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

/// The demo run's states, in the shape the plugin keeps: it starts with
/// a key, cuts the test run's output as it arrives, then prunes the
/// history between turns.
fn demo_states() -> [Value; 3] {
    let started = json!({ "on": true });
    let archive = "/home/you/.local/share/tau/repos/tau-agent-3f2a91c0/\
                   archive/tau-output-41822-1790716482-0.txt";
    let mut cut = started.clone();
    cut["outputs"] = json!([{
        "call_id": "c7", "lines": 4810, "chunks": 241, "kept": 11,
        "dropped_lines": 4598, "segments": 1, "requests": 2,
        "tokens_before": 14_200, "tokens_after": 1_100, "pruned": true,
        "archive": archive, "cost": 0.0006,
    }]);
    let decision =
        |call_id: &str, tool: &str, action: &str, odds: (f64, f64)| {
            json!({
                "call_id": call_id, "tool": tool, "action": action,
                "keep_call": odds.0, "keep_result": odds.1,
            })
        };
    let decided = [
        ("c2", "grep", "keep", (0.64, 0.22)),
        ("c3", "read", "drop_result", (0.71, 0.18)),
        ("c5", "edit", "keep", (0.94, 0.88)),
        ("c6", "exec_command", "keep", (0.18, 0.04)),
        ("c7", "exec_command", "keep", (0.62, 0.03)),
        ("c8", "read", "drop_result", (0.52, 0.09)),
        ("c9", "read", "drop_result", (0.58, 0.11)),
    ];
    // Every call before the rewrite, as the ledger page lists it: the
    // first ones pinned with the prompt, the rest judged.
    let calls = [
        (
            "c1",
            "memory_read",
            r#"{"id":"n-0388","title":"Retry policy honors server hints"}"#,
            40,
        ),
        ("c1a", "ls", r#"{"path":"crates/tau-ai/src"}"#, 90),
        (
            "c2",
            "grep",
            r#"{"path":"crates/tau-ai/src","pattern":"retry_after|RetryPolicy"}"#,
            120,
        ),
        (
            "c3",
            "read",
            r#"{"path":"crates/tau-ai/src/retry.rs"}"#,
            3_100,
        ),
        (
            "c4",
            "edit",
            r#"{"edits":[{"newText":"retry_after","oldText":"delay"}],"path":"crates/tau-ai/src/retry.rs"}"#,
            210,
        ),
        (
            "c5",
            "edit",
            r#"{"edits":[{"newText":"server_hint","oldText":"backoff"}],"path":"crates/tau-ai/src/retry.rs"}"#,
            640,
        ),
        (
            "c5t",
            "exec_command",
            r#"{"cmd":"cargo nextest run -p tau-ai"}"#,
            1_900,
        ),
        (
            "c6",
            "exec_command",
            r#"{"cmd":"rm -rf target/debug/incremental"}"#,
            20,
        ),
        (
            "c7",
            "exec_command",
            r#"{"cmd":"cargo nextest run --workspace"}"#,
            1_100,
        ),
        (
            "c8",
            "read",
            r#"{"path":"crates/tau-ai/src/client.rs"}"#,
            7_600,
        ),
        (
            "c9",
            "read",
            r#"{"path":"crates/tau-testing/src/fake_openai.rs"}"#,
            5_000,
        ),
    ];
    let ledger: Vec<Value> = calls
        .iter()
        .map(|(call_id, tool, input, tokens)| {
            let judged = decided.iter().find(|(id, ..)| id == call_id);
            json!({
                "call_id": call_id, "tool": tool, "input": input,
                "tokens": tokens,
                "matters": judged.map(|(.., odds)| odds.0),
                "verbatim": judged.map(|(.., odds)| odds.1),
                "decision": judged.map_or("pinned", |(_, _, action, _)| action),
            })
        })
        .collect();
    let mut pruned = cut.clone();
    pruned["decisions"] = decided
        .iter()
        .map(|(call_id, tool, action, odds)| {
            decision(call_id, tool, action, *odds)
        })
        .collect();
    pruned["ledger"] = Value::Array(ledger);
    pruned["last_pass"] = json!(172_000);
    pruned["passes"] = json!([{
        "before": 172_000, "after": 81_000, "results_dropped": 3,
        "calls_dropped": 0, "reduction_ratio": 0.53,
        "detail": "7 calls judged in 1 Jev request · 3 results cut, 0 calls \
                   dropped · −53% · Jev read 18k tokens of history, whole · \
                   $0.0011",
        "window": 272_000,
    }]);
    [started, cut, pruned]
}

/// The demo's run is what the plugin draws at each step: started, the
/// output cut, the history pruned. `BLESS_FIXTURES=1` writes it anew.
#[test]
fn the_demo_is_what_the_plugin_draws() {
    let loaded = loaded();
    let steps: Vec<Value> = demo_states()
        .into_iter()
        .map(|state| {
            let view = view_of(&loaded, &state);
            json!({ "state": state, "view": view })
        })
        .collect();
    let [started, cut, pruned] = &steps[..] else {
        unreachable!("three steps")
    };
    let status = |step: &Value| step["view"]["status"].to_string();
    assert!(status(started).contains("watching the window"), "{started}");
    assert_eq!(cut["view"]["calls"]["c7"]["cut"]["lines"], 4810);
    assert!(status(pruned).contains("3 pruned · −53%"), "{pruned}");
    for id in ["c3", "c8", "c9"] {
        assert_eq!(pruned["view"]["calls"][id]["dropped"], "result", "{id}");
    }
    assert!(pruned["view"]["notes"]["pass-1"].is_object());
    bless_or_check(
        "run",
        crate::fixture("tau-fast-compaction", "run"),
        &Value::Array(steps.clone()),
    );
}
