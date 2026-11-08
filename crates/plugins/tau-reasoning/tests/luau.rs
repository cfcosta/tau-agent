//! tau-reasoning's Luau loads as a plugin and passes its own tests, as
//! tau loads it; and what the demo shows is what the plugin leaves and
//! draws.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{collections::BTreeMap, sync::Arc};

use serde_json::{Value, json};
use tau_jev::{Answer, Jev, fake::FakeJev};
use tau_luau_plugins_host::runtime::{
    Context,
    Files,
    Hook,
    Loaded,
    PLUGIN,
    Reach,
    load,
};
use tau_testing::block_on_io;
use tokio_util::sync::CancellationToken;

fn loaded() -> Loaded {
    let paths: Vec<(String, Vec<u8>)> = tau_reasoning::FILES
        .iter()
        .map(|(path, source)| ((*path).to_owned(), source.as_bytes().to_vec()))
        .collect();
    let files = Files::from_paths(&paths, PLUGIN).unwrap();
    block_on_io(load(tau_reasoning::NAME, files)).unwrap()
}

#[test]
fn the_plugin_loads_and_passes_its_tests() {
    let loaded = loaded();
    assert_eq!(loaded.declaration.name, tau_reasoning::NAME);
    let tests = block_on_io(loaded.test(CancellationToken::new()));
    assert!(tests.len() > 10, "{tests:?}");
    for test in tests {
        assert!(test.passed, "{test:?}");
    }
}

/// gpt-5.5's levels.
const EFFORTS: [&str; 5] = ["none", "low", "medium", "high", "xhigh"];

/// A Jev `confidence` sure of `probabilities`, one per level.
fn jev(probabilities: [f64; 5], confidence: f64) -> Arc<dyn Jev> {
    Arc::new(FakeJev::new(move |request| {
        let answers = BTreeMap::from([(
            "effort".to_owned(),
            Answer::Score {
                score: 0.0,
                probabilities: probabilities
                    .iter()
                    .enumerate()
                    .map(|(n, p)| (n.to_string(), *p))
                    .collect(),
                confidence,
            },
        )]);
        Ok(tau_jev::fake::response(answers, request))
    }))
}

struct WithJev(Arc<dyn Jev>);

impl Reach for WithJev {
    fn jev(&self) -> Option<Arc<dyn Jev>> {
        Some(self.0.clone())
    }
}

fn context(run: &str, state: &Value) -> Context {
    Context {
        run: json!({ "id": run, "kind": "main" }),
        now: json!({}),
        settings: json!({ "redecide": false, "threshold": 0.7 }),
        state: state.clone(),
        secrets: None,
        store: None,
    }
}

/// `hook` with `input` on `run`, from `state`: the state it leaves.
fn call(
    loaded: &Loaded,
    hook: Hook,
    input: Value,
    run: &str,
    state: &Value,
    jev: Arc<dyn Jev>,
) -> Value {
    let outcome = block_on_io(loaded.call(
        &hook,
        input,
        &context(run, state),
        Arc::new(WithJev(jev)),
        CancellationToken::new(),
    ));
    assert_eq!(outcome.error, None);
    outcome.state
}

/// A message coming in on auto, with `prefix` tokens before it.
fn message(task: &str, prefix: u64) -> Value {
    json!({
        "input": task, "instructions": "You are a coding agent.",
        "model": "gpt-5.5", "efforts": EFFORTS, "prefix_tokens": prefix,
    })
}

/// The states the demo shows, as the plugin leaves them: high picked as
/// the run starts; then a later message of a 48k-token conversation
/// that keeps medium over Jev's high.
fn demo_states(loaded: &Loaded) -> [(&'static str, &'static str, Value); 2] {
    let high = jev([0.01, 0.02, 0.09, 0.84, 0.04], 0.84);
    let chose = call(
        loaded,
        Hook::Start,
        message("Fix the flaky retry test", 0),
        "demo",
        &json!({}),
        high,
    );
    let medium = jev([0.01, 0.03, 0.9, 0.05, 0.01], 0.9);
    let earlier = call(
        loaded,
        Hook::Start,
        message("Design the retry pool", 0),
        "earlier",
        &json!({}),
        medium,
    );
    let earlier = call(
        loaded,
        Hook::RunEnd,
        json!({ "stop": "Stop", "turns": 3, "cost": 0, "text": "Done." }),
        "earlier",
        &earlier,
        jev([0.0; 5], 0.0),
    );
    let high = jev([0.01, 0.03, 0.11, 0.81, 0.04], 0.81);
    let kept = call(
        loaded,
        Hook::Start,
        message("Fix the flaky retry test", 48_213),
        "demo",
        &earlier,
        high,
    );
    [
        ("chose", tau_reasoning::DEMO_CHOSE, chose),
        ("kept", tau_reasoning::DEMO_KEPT, kept),
    ]
}

/// What the plugin's `view` draws from `state`.
fn view_of(loaded: &Loaded, state: &Value) -> Value {
    let outcome = block_on_io(loaded.call(
        &Hook::View,
        Value::Null,
        &context("demo", state),
        Arc::new(WithJev(jev([0.0; 5], 0.0))),
        CancellationToken::new(),
    ));
    assert_eq!(outcome.error, None);
    outcome.value
}

/// Each state the demo shows is what the plugin leaves, and each view
/// what it draws from it. `BLESS_FIXTURES=1` writes them anew from the
/// plugin.
#[test]
fn the_demo_is_what_the_plugin_leaves_and_draws() {
    let loaded = loaded();
    for (name, text, state) in demo_states(&loaded) {
        let drawn = json!({ "state": state, "view": view_of(&loaded, &state) });
        if std::env::var_os("BLESS_FIXTURES").is_some() {
            let path = format!(
                "{}/../../../plugins/tau-reasoning/fixtures/{name}.json",
                env!("CARGO_MANIFEST_DIR")
            );
            let text = serde_json::to_string_pretty(&drawn).unwrap() + "\n";
            std::fs::write(path, text).unwrap();
            continue;
        }
        let fixture: Value = serde_json::from_str(text).unwrap();
        assert_eq!(fixture, drawn, "{name}: run with BLESS_FIXTURES=1");
    }
}

/// What the demo lists of the plugin is what it declares.
/// `BLESS_FIXTURES=1` writes it anew from the plugin.
#[test]
fn the_demo_lists_what_the_plugin_declares() {
    let declared = serde_json::to_value(&loaded().declaration).unwrap();
    if std::env::var_os("BLESS_FIXTURES").is_some() {
        let path = format!(
            "{}/../../../plugins/tau-reasoning/fixtures/declaration.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let text = serde_json::to_string_pretty(&declared).unwrap() + "\n";
        std::fs::write(path, text).unwrap();
        return;
    }
    let fixture: Value =
        serde_json::from_str(tau_reasoning::DEMO_DECLARATION).unwrap();
    assert_eq!(fixture, declared, "run with BLESS_FIXTURES=1");
}
