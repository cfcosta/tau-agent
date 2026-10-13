//! tau-canvas in the workspace: the run's pane opens as something new
//! lands on its canvas, stays closed for a run first drawn with a canvas
//! until a plugin opens it, and an image on it is asked of the host
//! once.

use std::sync::Arc;

use gpui::{Entity, TestAppContext, VisualTestContext};
use tau_agent::tool::RunId;
use tau_canvas::{Blob, Content, ImageRef, Record};
use tau_ui::demo;
use tau_ui_remote::{
    Workspace,
    WorkspaceEvent,
    catalog::Catalog,
    route::Route,
    update::HostUpdate,
};

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

fn document(call: &str, item: u32, text: &str) -> HostUpdate {
    fold(revised(call, item, text))
}

fn revised(call: &str, item: u32, text: &str) -> Record {
    Record::Revised {
        call: call.into(),
        item,
        title: "Plan".into(),
        content: Content::Document { text: text.into() },
    }
}

fn image(call: &str) -> HostUpdate {
    fold(Record::Revised {
        call: call.into(),
        item: 9,
        title: "Chart".into(),
        content: Content::Image {
            image: ImageRef {
                blob: Blob {
                    id: "0190f0c0-0000-7000-8000-000000000001".into(),
                    digest: "ab".repeat(32),
                    size_bytes: 10,
                },
                mime: "image/png".into(),
                width: Some(4),
                height: Some(3),
                path: "chart.png".into(),
            },
            caption: String::new(),
        },
    })
}

fn fold(record: Record) -> HostUpdate {
    HostUpdate::PluginFold {
        run: demo::run_id(),
        plugin: tau_canvas::NAME.into(),
        body: serde_json::to_value(record).unwrap(),
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
            matches!(event, WorkspaceEvent::PluginAct { plugin, action }
                if plugin == tau_canvas::NAME && action["action"] == "fetch")
        })
        .count()
}

#[gpui::test]
fn the_pane_opens_as_something_new_lands_while_the_run_is_in_view(
    cx: &mut TestAppContext,
) {
    let (workspace, mut cx, events) = open(cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Run(demo::run_id()), cx)
    });
    cx.run_until_parked();
    assert!(
        !open_pane(&workspace, &mut cx),
        "an empty canvas has no pane"
    );

    workspace.update(&mut cx, |ws, cx| ws.apply(document("a", 1, "one"), cx));
    cx.run_until_parked();
    assert!(open_pane(&workspace, &mut cx), "a new document opens it");

    // Closed, it stays closed until the next revision.
    workspace.update(&mut cx, |ws, cx| ws.toggle_pane(cx));
    cx.run_until_parked();
    assert!(!open_pane(&workspace, &mut cx));
    workspace.update(&mut cx, |ws, cx| ws.apply(document("b", 1, "two"), cx));
    cx.run_until_parked();
    assert!(
        open_pane(&workspace, &mut cx),
        "a new revision opens it again"
    );

    // An image shows at once in the pane and its card, and is asked of
    // the host once for both, however often they draw.
    workspace.update(&mut cx, |ws, cx| ws.apply(image("c"), cx));
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
        view.fold(
            tau_canvas::NAME,
            &serde_json::to_value(revised("a", 1, "one")).unwrap(),
        );
        vec![view]
    });
    cx.run_until_parked();
    workspace
        .update(&mut cx, |ws, cx| ws.navigate(Route::Run(other.clone()), cx));
    cx.run_until_parked();
    assert!(!pane_of(&workspace, &other, &mut cx));

    // A card's click opens it, through the plugin's handle.
    workspace.update(&mut cx, |ws, cx| {
        ws.plugin_handle(tau_canvas::NAME).open_pane(cx)
    });
    cx.run_until_parked();
    assert!(pane_of(&workspace, &other, &mut cx));
}

#[gpui::test]
fn what_landed_while_the_run_was_away_opens_its_pane_on_return(
    cx: &mut TestAppContext,
) {
    let (workspace, mut cx, _) = open(cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Run(demo::run_id()), cx)
    });
    cx.run_until_parked();
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::History, cx);
        ws.apply(document("a", 1, "one"), cx);
    });
    cx.run_until_parked();
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Run(demo::run_id()), cx)
    });
    cx.run_until_parked();
    assert!(open_pane(&workspace, &mut cx));
}
