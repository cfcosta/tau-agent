//! tau-goal's Luau loads as a plugin and passes its own tests, as tau
//! loads it; and the goals the demo shows are what the plugin leaves and
//! draws.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use serde_json::{Value, json};
use tau_jev::{Answer, Jev, fake::FakeJev};
use tau_luau_plugins_host::runtime::{Context, Hook, Loaded, Reach, load};
use tau_testing::block_on_io;
use tokio_util::sync::CancellationToken;

fn loaded() -> Loaded {
    let files =
        tau_luau_plugins_host::built_in::files(tau_luau_plugins::names::GOAL)
            .unwrap();
    block_on_io(load(tau_luau_plugins::names::GOAL, files)).unwrap()
}

#[test]
fn the_plugin_loads_and_passes_its_tests() {
    let loaded = loaded();
    assert_eq!(loaded.declaration.name, tau_luau_plugins::names::GOAL);
    assert_eq!(loaded.declaration.continues, Some(20));
    let tests = block_on_io(loaded.test(CancellationToken::new()));
    assert!(tests.len() > 10, "{tests:?}");
    for test in tests {
        assert!(test.passed, "{test:?}");
    }
}

/// A Jev that answers each check with the next of `answers`.
fn jev(answers: &'static [f64]) -> Arc<dyn Jev> {
    let answers = Arc::new(Mutex::new(answers.iter().copied()));
    Arc::new(FakeJev::new(move |request| {
        let p = answers.lock().unwrap().next().expect("an answer left");
        let answers =
            BTreeMap::from([("met".to_owned(), Answer::Noul { noul: p })]);
        Ok(tau_jev::fake::response(answers, request))
    }))
}

struct WithJev(Arc<dyn Jev>);

impl Reach for WithJev {
    fn jev(&self) -> Option<Arc<dyn Jev>> {
        Some(self.0.clone())
    }
}

/// `hook` with `input`, from `state`: the state it leaves.
fn call(
    loaded: &Loaded,
    hook: Hook,
    input: Value,
    state: &Value,
    jev: &Arc<dyn Jev>,
) -> Value {
    let context = Context {
        run: json!({ "id": "demo", "kind": "main" }),
        now: json!({}),
        settings: json!({}),
        state: state.clone(),
        secrets: None,
        store: None,
    };
    let outcome = block_on_io(loaded.call(
        &hook,
        input,
        &context,
        Arc::new(WithJev(jev.clone())),
        CancellationToken::new(),
    ));
    assert_eq!(outcome.error, None);
    outcome.state
}

/// A run on `/goal <limits> <condition>` that stops after each of
/// `turns` with Jev's answers, then ends: the state it leaves.
fn goal_run(
    loaded: &Loaded,
    command: &str,
    answers: &'static [f64],
    turns: &[u32],
) -> Value {
    let jev = jev(answers);
    let mut state = call(
        loaded,
        Hook::Start,
        json!({ "input": command }),
        &json!({}),
        &jev,
    );
    for turn in turns {
        let ended =
            json!({ "turn": turn, "text": "", "calls": [], "cost": 0.12 });
        state = call(loaded, Hook::TurnEnd, ended, &state, &jev);
        let stop = json!({ "text": "done", "turn": turn, "reason": "stop" });
        state = call(loaded, Hook::BeforeStop, stop, &state, &jev);
    }
    let end = json!({ "stop": "Stop", "turns": 0, "cost": 0, "text": "done" });
    call(loaded, Hook::RunEnd, end, &state, &jev)
}

/// The states the demo shows, as the plugin leaves them.
fn demo_states(loaded: &Loaded) -> Vec<(&'static str, &'static str, Value)> {
    let goal = "All tests in tau-ai pass and cargo clippy is clean";
    vec![
        (
            "working",
            crate::fixture("tau-goal", "working"),
            goal_run(loaded, &format!("/goal {goal}"), &[0.03, 0.08], &[8, 10]),
        ),
        (
            "met",
            crate::fixture("tau-goal", "met"),
            goal_run(
                loaded,
                &format!("/goal {goal}"),
                &[0.03, 0.08, 0.96],
                &[8, 10, 11],
            ),
        ),
        (
            "stopped",
            crate::fixture("tau-goal", "stopped"),
            goal_run(
                loaded,
                &format!("/goal --continuations 2 {goal}"),
                &[0.12, 0.11, 0.11],
                &[9, 11, 13],
            ),
        ),
        (
            "triage",
            crate::fixture("tau-goal", "triage"),
            goal_run(
                loaded,
                "/goal Every mutant in retry.rs is caught",
                &[0.2, 0.96],
                &[6, 9],
            ),
        ),
        (
            "lanes",
            crate::fixture("tau-goal", "lanes"),
            goal_run(
                loaded,
                "/goal --continuations 2 Every lane has an owner in lanes.toml",
                &[0.12, 0.11, 0.11],
                &[9, 11, 13],
            ),
        ),
    ]
}

/// What the plugin's `view` draws from `state`.
fn view_of(loaded: &Loaded, state: &Value) -> Value {
    let context = Context {
        run: json!({ "id": "demo", "kind": "main" }),
        now: json!({}),
        settings: json!({}),
        state: state.clone(),
        secrets: None,
        store: None,
    };
    let outcome = block_on_io(loaded.call(
        &Hook::View,
        Value::Null,
        &context,
        Arc::new(tau_luau_plugins_host::runtime::NoReach),
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
            let path = crate::fixture_path("tau-goal", name);
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
        let path = crate::fixture_path("tau-goal", "declaration");
        let text = serde_json::to_string_pretty(&declared).unwrap() + "\n";
        std::fs::write(path, text).unwrap();
        return;
    }
    let fixture: Value =
        serde_json::from_str(crate::fixture("tau-goal", "declaration"))
            .unwrap();
    assert_eq!(fixture, declared, "run with BLESS_FIXTURES=1");
}
