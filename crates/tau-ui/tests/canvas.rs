//! tau-canvas in the workspace: the run's pane opens as something new
//! lands on its canvas, stays closed for a run first drawn with a canvas
//! until a plugin opens it, and an image on it is asked of the host
//! once. tau-canvas is Luau (ADR 0050): its state, and the view it draws
//! from it, fold into the run as the host publishes them.

use std::sync::Arc;

use gpui::{Entity, TestAppContext, VisualTestContext};
use serde_json::{Value, json};
use tau_agent::tool::RunId;
use tau_demo as demo;
use tau_luau_plugins::Record;
use tau_luau_plugins_host::runtime::{Context, Hook, Loaded, NoReach, load};
use tau_testing::block_on_io;
use tau_ui_remote::{
    Workspace,
    WorkspaceEvent,
    catalog::Catalog,
    route::Route,
    update::HostUpdate,
};
use tokio_util::sync::CancellationToken;

type Events = std::rc::Rc<std::cell::RefCell<Vec<WorkspaceEvent>>>;

fn open(
    cx: &mut TestAppContext,
) -> (Entity<Workspace>, VisualTestContext, Events) {
    open_with(cx, Vec::new)
}

/// The workspace on the demo run, with `more` runs beside it.
fn open_with(
    cx: &mut TestAppContext,
    more: impl FnOnce() -> Vec<tau_ui_remote::view::RunView> + 'static,
) -> (Entity<Workspace>, VisualTestContext, Events) {
    tau_ui::hosted::install();
    cx.update(tau_ui_remote::init);
    let window = cx.add_window(|window, cx| {
        let catalog = Catalog {
            repos: vec![tau_ui_remote::catalog::Repo {
                name: "tau-agent".into(),
                main: Some(demo::run_id()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let runs = std::iter::once(demo::retry_after()).chain(more()).collect();
        Workspace::new("tau", runs, catalog, window, cx)
    });
    let workspace = window.root(cx).unwrap();
    let events = Events::default();
    let seen = events.clone();
    cx.update(|cx| {
        cx.subscribe(&workspace, move |_, event: &WorkspaceEvent, _| {
            seen.borrow_mut().push(event.clone())
        })
        .detach()
    });
    (
        workspace,
        VisualTestContext::from_window(window.into(), cx),
        events,
    )
}

/// tau-canvas's Luau, as tau loads it.
fn plugin() -> Loaded {
    let files =
        tau_luau_plugins_host::built_in::files(tau_luau_plugins::names::CANVAS)
            .unwrap();
    block_on_io(load(tau_luau_plugins::names::CANVAS, files)).unwrap()
}

/// A run's canvas holding `revisions`, each `(call, item, content)`, in
/// order, as the plugin keeps it.
fn canvas(revisions: &[(&str, u32, Value)]) -> Value {
    let mut items: Vec<Value> = Vec::new();
    for (call, item, content) in revisions {
        let revision = json!({ "call": call, "content": content });
        match items.iter_mut().find(|found| found["id"] == json!(item)) {
            Some(found) => {
                found["revisions"].as_array_mut().unwrap().push(revision)
            }
            None => items.push(json!({
                "id": item,
                "title": "Plan",
                "revisions": [revision],
            })),
        }
    }
    json!({ "canvas": {
        "items": items,
        "fresh": revisions.len(),
        "last": revisions.last().map(|(_, item, _)| *item),
    } })
}

fn text(text: &str) -> Value {
    json!({ "type": "document", "text": text })
}

fn chart() -> Value {
    json!({ "type": "image", "caption": "", "image": {
        "blob": {
            "id": "0190f0c0-0000-7000-8000-000000000001",
            "digest": "ab".repeat(32),
            "size_bytes": 10,
        },
        "mime": "image/png",
        "width": 4,
        "height": 3,
        "path": "chart.png",
    } })
}

/// The run `run` with `state`, and the view the plugin draws from it, as
/// the host publishes them.
fn records(loaded: &Loaded, run: &RunId, state: Value) -> [Value; 2] {
    let context = Context {
        run: json!({ "id": run.0.as_ref(), "kind": "main", "repo": "tau-agent" }),
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
    [
        Record::State {
            plugin: tau_luau_plugins::names::CANVAS.into(),
            state,
        },
        Record::View {
            plugin: tau_luau_plugins::names::CANVAS.into(),
            view: outcome.value,
        },
    ]
    .map(|record| serde_json::to_value(record).unwrap())
}

/// Folds the canvas of `revisions` into the demo run.
fn publish(
    ws: &mut Workspace,
    loaded: &Loaded,
    revisions: &[(&str, u32, Value)],
    cx: &mut gpui::Context<Workspace>,
) {
    for body in records(loaded, &demo::run_id(), canvas(revisions)) {
        ws.apply(
            HostUpdate::PluginFold {
                run: demo::run_id(),
                plugin: tau_luau_plugins::NAME.into(),
                body,
            },
            cx,
        );
    }
}

fn open_pane(
    workspace: &Entity<Workspace>,
    cx: &mut VisualTestContext,
) -> bool {
    pane_of(workspace, &demo::run_id(), cx)
}

fn pane_of(
    workspace: &Entity<Workspace>,
    run: &RunId,
    cx: &mut VisualTestContext,
) -> bool {
    workspace.read_with(cx, |ws, _| ws.pane_is_open(run))
}

/// The images the canvas asked the host for.
fn fetches(events: &Events) -> usize {
    events
        .borrow()
        .iter()
        .filter(|event| {
            // The pane's and the card's image are the interface's, which
            // fetches it as a picture (ADR 0045).
            matches!(event, WorkspaceEvent::PluginFetch { plugin, action, .. }
                if plugin == tau_luau_plugins::NAME && action["act"] == "blob")
        })
        .count()
}

#[gpui::test]
fn the_pane_opens_as_something_new_lands_while_the_run_is_in_view(
    cx: &mut TestAppContext,
) {
    let (workspace, mut cx, events) = open(cx);
    let loaded = plugin();
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Run(demo::run_id()), cx)
    });
    cx.run_until_parked();
    assert!(
        !open_pane(&workspace, &mut cx),
        "an empty canvas has no pane"
    );

    let mut landed = vec![("a", 1, text("one"))];
    workspace.update(&mut cx, |ws, cx| publish(ws, &loaded, &landed, cx));
    cx.run_until_parked();
    assert!(open_pane(&workspace, &mut cx), "a new document opens it");

    // Closed, it stays closed until the next revision.
    workspace.update(&mut cx, |ws, cx| ws.toggle_pane(cx));
    cx.run_until_parked();
    assert!(!open_pane(&workspace, &mut cx));
    landed.push(("b", 1, text("two")));
    workspace.update(&mut cx, |ws, cx| publish(ws, &loaded, &landed, cx));
    cx.run_until_parked();
    assert!(
        open_pane(&workspace, &mut cx),
        "a new revision opens it again"
    );

    // An image shows at once in the pane and its card, and is asked of
    // the host once for both, however often they draw.
    landed.push(("c", 9, chart()));
    workspace.update(&mut cx, |ws, cx| publish(ws, &loaded, &landed, cx));
    cx.run_until_parked();
    workspace.update(&mut cx, |_, cx| cx.notify());
    cx.run_until_parked();
    assert_eq!(fetches(&events), 1);
}

#[gpui::test]
fn a_run_first_drawn_with_a_canvas_keeps_its_pane_closed_until_asked(
    cx: &mut TestAppContext,
) {
    let other = RunId(Arc::from("canvas-run"));
    let id = other.clone();
    let (workspace, mut cx, _) = open_with(cx, move || {
        let mut view = tau_ui_remote::view::RunView::new(
            id,
            "canvas-run",
            "coder",
            "gpt-5.5",
        )
        .in_repo("tau-agent");
        let loaded = plugin();
        for body in
            records(&loaded, &view.id.clone(), canvas(&[("a", 1, text("one"))]))
        {
            view.fold(tau_luau_plugins::NAME, &body);
        }
        vec![view]
    });
    cx.run_until_parked();
    workspace
        .update(&mut cx, |ws, cx| ws.navigate(Route::Run(other.clone()), cx));
    cx.run_until_parked();
    assert!(!pane_of(&workspace, &other, &mut cx));

    // A card's click opens it, through the plugins' handle.
    workspace.update(&mut cx, |ws, cx| {
        ws.plugin_handle(tau_luau_plugins::NAME).open_pane();
        ws.drain_plugin_requests(cx)
    });
    cx.run_until_parked();
    assert!(pane_of(&workspace, &other, &mut cx));
}

#[gpui::test]
fn what_landed_while_the_run_was_away_opens_its_pane_on_return(
    cx: &mut TestAppContext,
) {
    let (workspace, mut cx, _) = open(cx);
    let loaded = plugin();
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Run(demo::run_id()), cx)
    });
    cx.run_until_parked();
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::History, cx);
        publish(ws, &loaded, &[("a", 1, text("one"))], cx);
    });
    cx.run_until_parked();
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Run(demo::run_id()), cx)
    });
    cx.run_until_parked();
    assert!(open_pane(&workspace, &mut cx));
}
