//! tau-ask's Luau loads as a plugin and passes its own tests, as tau
//! loads it; and the questions the demo shows are what the plugin
//! leaves and draws.

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
    let files =
        tau_luau_plugins_host::built_in::files(tau_luau_plugins::names::ASK)
            .unwrap();
    block_on_io(load(tau_luau_plugins::names::ASK, files)).unwrap()
}

#[test]
fn the_plugin_loads_and_passes_its_tests() {
    let loaded = loaded();
    assert_eq!(loaded.declaration.name, tau_luau_plugins::names::ASK);
    let tests = block_on_io(loaded.test(CancellationToken::new()));
    assert!(tests.len() > 10, "{tests:?}");
    let failed: Vec<_> = tests.iter().filter(|test| !test.passed).collect();
    assert!(failed.is_empty(), "{failed:#?}");
}

/// `hook` with `input` on `state` in the run `run`: its value and the
/// state it leaves.
fn call(
    loaded: &Loaded,
    hook: Hook,
    input: Value,
    run: &str,
    call_id: &str,
    state: Value,
) -> (Value, Value) {
    let context = Context {
        run: json!({ "id": run, "kind": "main", "call": call_id, "live": true }),
        now: json!({}),
        settings: json!({}),
        state,
        secrets: None,
        store: None,
    };
    let outcome = block_on_io(loaded.call(
        &hook,
        input,
        &context,
        Arc::new(NoReach),
        CancellationToken::new(),
    ));
    assert_eq!(outcome.error, None, "{hook:?}");
    (outcome.value, outcome.state)
}

/// The questions the demo's run waits on.
fn demo_questions() -> Value {
    json!({ "questions": [
        {
            "question": "How should the ask tool wait for your answer?",
            "header": "Waiting",
            "options": [
                {
                    "label": "Hold the call open (Recommended)",
                    "description": "The tool blocks on a channel; your answer comes back as its result and the turn goes on.",
                    "preview": "let (tx, rx) = oneshot::channel();\nself.pending.insert(ctx.call_id, tx);\nselect! {\n    a = rx => Ok(a?),\n    _ = ctx.cancel => Err(Cancelled),\n}"
                },
                {
                    "label": "End the turn",
                    "description": "Like vcs_land: the run stops, and your answer arrives as the next message.",
                    "preview": "ctx.publish(&asked)?;\nOk(ToolOutput::text(\"Asked; waiting.\"))"
                },
                {
                    "label": "Hold, then time out",
                    "description": "Block for a while; if nobody answers, stop the run and keep the panel open.",
                    "preview": "select! {\n    a = rx => Ok(a?),\n    _ = sleep(timeout) => stop(),\n}"
                }
            ]
        },
        {
            "question": "Where else should a pending question show up?",
            "header": "Surfaces",
            "multi_select": true,
            "options": [
                { "label": "Run list badge", "description": "An amber dot on the run in the sidebar until you answer." },
                { "label": "Desktop notification", "description": "Only when the window is not focused." },
                { "label": "Parent run", "description": "A child run's question surfaces on the run that spawned it." }
            ]
        },
        {
            "question": "How long should a question hold the run?",
            "header": "Timeout",
            "options": [
                { "label": "5 minutes", "description": "Short; the run stops soon and resumes when you answer." },
                { "label": "Never", "description": "Hold until you answer or cancel the run." }
            ]
        }
    ]})
}

/// What run `run`'s call `call_id` asking `asked` leaves after `steps`,
/// each an action and its arguments, and the view drawn from it.
fn demo_of(
    loaded: &Loaded,
    run: &str,
    call_id: &str,
    asked: Value,
    steps: &[(&str, Value)],
) -> Value {
    let (_, mut state) = call(
        loaded,
        Hook::Tool("ask".into()),
        asked,
        run,
        call_id,
        json!({}),
    );
    for (action, args) in steps {
        state = call(
            loaded,
            Hook::Action((*action).into()),
            args.clone(),
            run,
            call_id,
            state,
        )
        .1;
    }
    let (view, _) =
        call(loaded, Hook::View, Value::Null, run, call_id, state.clone());
    json!({ "state": state, "view": view })
}

/// The run the demo shows waiting, as the plugin leaves and draws it:
/// untouched, a checklist's note being written, and on the review; and
/// another chat waiting on one question.
fn demos(loaded: &Loaded) -> Vec<(&'static str, &'static str, Value)> {
    let key = |key: &str| ("key", json!({ "key": key }));
    let picked = [key("1"), key("1"), key("3")];
    let note = (
        "note-done",
        json!({ "text": "Only when the run has waited a while." }),
    );
    let mut writing = picked.to_vec();
    writing.extend([key("n"), note.clone(), key("n")]);
    let mut review = picked.to_vec();
    review.extend([key("n"), note, key("right"), key("2")]);
    let sweep = json!({ "questions": [{
        "question": "Delete the 3 workspaces with no run, or keep them?",
        "header": "Workspaces",
        "options": [
            { "label": "Delete them", "description": "No run uses them." },
            { "label": "Keep them", "description": "Look at them first." }
        ]
    }]});
    let run = "retry-after";
    vec![
        (
            "ask",
            crate::fixture("tau-ask", "ask"),
            demo_of(loaded, run, "call_ask", demo_questions(), &[]),
        ),
        (
            "note",
            crate::fixture("tau-ask", "note"),
            demo_of(loaded, run, "call_ask", demo_questions(), &writing),
        ),
        (
            "review",
            crate::fixture("tau-ask", "review"),
            demo_of(loaded, run, "call_ask", demo_questions(), &review),
        ),
        (
            "sweep",
            crate::fixture("tau-ask", "sweep"),
            demo_of(loaded, "sweep-workspaces", "call_sweep", sweep, &[]),
        ),
    ]
}

/// Each state the demo shows is what the plugin leaves, and each view
/// what it draws from it. `BLESS_FIXTURES=1` writes them anew from the
/// plugin.
#[test]
fn the_demo_is_what_the_plugin_leaves_and_draws() {
    let loaded = loaded();
    for (name, text, drawn) in demos(&loaded) {
        if std::env::var_os("BLESS_FIXTURES").is_some() {
            let path = crate::fixture_path("tau-ask", name);
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
        let path = crate::fixture_path("tau-ask", "declaration");
        let text = serde_json::to_string_pretty(&declared).unwrap() + "\n";
        std::fs::write(path, text).unwrap();
        return;
    }
    let fixture: Value =
        serde_json::from_str(crate::fixture("tau-ask", "declaration")).unwrap();
    assert_eq!(fixture, declared, "run with BLESS_FIXTURES=1");
}
