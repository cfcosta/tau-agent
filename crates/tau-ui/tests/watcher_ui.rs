//! tau-watcher in the workspace: a note is a line above the composer
//! while it is new, and an annotation in the transcript for good. The
//! watcher is Luau (ADR 0050): its views, drawn by the plugin from each
//! state, reach the workspace as tau-luau-plugins' records.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::sync::Arc;

use gpui::{Entity, TestAppContext, VisualTestContext};
use serde_json::{Value, json};
use tau_demo as demo;
use tau_luau_plugins_host::runtime::{Context, Hook, Loaded, NoReach, load};
use tau_ui_plugin::points::{self, AtAnchor, AtRun};
use tau_ui_remote::{
    Workspace,
    catalog::{Catalog, Repo},
    route::Route,
    update::HostUpdate,
};
use tokio_util::sync::CancellationToken;

fn open(cx: &mut TestAppContext) -> (Entity<Workspace>, VisualTestContext) {
    tau::install();
    cx.update(tau_ui_remote::init);
    let window = cx.add_window(|window, cx| {
        let repo = Repo {
            name: "tau-agent".into(),
            main: Some(demo::run_id()),
            ..Default::default()
        };
        let catalog = Catalog {
            repos: vec![repo],
            ..Default::default()
        };
        Workspace::new("tau", vec![demo::retry_after()], catalog, window, cx)
    });
    let workspace = window.root(cx).unwrap();
    (workspace, VisualTestContext::from_window(window.into(), cx))
}

fn watcher() -> Loaded {
    let files = tau_luau_plugins_host::built_in::files(
        tau_luau_plugins::names::WATCHER,
    )
    .unwrap();
    tau_testing::block_on_io(load(tau_luau_plugins::names::WATCHER, files))
        .unwrap()
}

/// A run's watcher state with one note, `status`, written past
/// `typed_past` times.
fn state(status: &str, typed_past: u32) -> Value {
    json!({
        "notes": [{
            "key": "n0", "step": 6, "tag": "Heads up",
            "line": "The key expires tomorrow.",
            "status": status, "typed_past": typed_past,
        }],
        "ignored": typed_past, "dropped": 0, "last_check": 6,
    })
}

/// The view the watcher draws from `state`, folded into the run as the
/// host stores it.
fn fold(
    workspace: &Entity<Workspace>,
    plugin: &Loaded,
    state: &Value,
    cx: &mut VisualTestContext,
) {
    let context = Context {
        run: json!({ "id": demo::run_id().0.as_ref(), "kind": "main" }),
        now: json!({}),
        settings: json!({ "enabled": true, "family": "chat" }),
        state: state.clone(),
        secrets: None,
        store: None,
        job: None,
    };
    let view = tau_testing::block_on_io(plugin.call(
        &Hook::View,
        Value::Null,
        &context,
        Arc::new(NoReach),
        CancellationToken::new(),
    ));
    assert_eq!(view.error, None);
    let record = tau_luau_plugins::Record::View {
        plugin: tau_luau_plugins::names::WATCHER.into(),
        view: view.value,
    };
    let update = HostUpdate::PluginFold {
        run: demo::run_id(),
        plugin: tau_luau_plugins::NAME.into(),
        body: serde_json::to_value(record).unwrap(),
    };
    workspace.update(cx, |ws, cx| ws.apply(update, cx));
    cx.run_until_parked();
}

/// Contributions in the band above the composer, and in the transcript
/// at the note's anchor.
fn drawn(
    workspace: &Entity<Workspace>,
    cx: &mut VisualTestContext,
) -> (usize, usize) {
    workspace.update(cx, |ws, cx| {
        let run = ws.run(&demo::run_id()).unwrap().info();
        let anchor = AtAnchor {
            run: run.clone(),
            key: format!("note-{}-n0", tau_luau_plugins::names::WATCHER),
            index: 0,
        };
        (
            ws.drawn_of(
                tau_luau_plugins::NAME,
                points::COMPOSER_BAND,
                &AtRun { run },
                cx,
            )
            .len(),
            ws.drawn_of(
                tau_luau_plugins::NAME,
                points::TRANSCRIPT,
                &anchor,
                cx,
            )
            .len(),
        )
    })
}

#[gpui::test]
fn a_note_is_a_band_while_new_and_an_annotation_always(
    cx: &mut TestAppContext,
) {
    let (workspace, mut cx) = open(cx);
    let plugin = watcher();
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Run(demo::run_id()), cx)
    });
    assert_eq!(drawn(&workspace, &mut cx), (0, 0), "nothing before a note");
    // New: both. Written past once: still a band. Twice: only the
    // annotation.
    for (typed_past, expected) in [(0, (1, 1)), (1, (1, 1)), (2, (0, 1))] {
        fold(&workspace, &plugin, &state("new", typed_past), &mut cx);
        assert_eq!(drawn(&workspace, &mut cx), expected, "{typed_past}");
    }
}

#[gpui::test]
fn acting_on_a_note_takes_the_band_away(cx: &mut TestAppContext) {
    let (workspace, mut cx) = open(cx);
    let plugin = watcher();
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Run(demo::run_id()), cx)
    });
    fold(&workspace, &plugin, &state("new", 0), &mut cx);
    assert_eq!(drawn(&workspace, &mut cx), (1, 1));
    fold(&workspace, &plugin, &state("knew", 0), &mut cx);
    assert_eq!(drawn(&workspace, &mut cx), (0, 1));
}
