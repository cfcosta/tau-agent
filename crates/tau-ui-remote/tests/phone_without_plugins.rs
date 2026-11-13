//! A phone with no plugins of its own (ADR 0046): it learns the
//! computer's from its catalog, draws their views as the computer sends
//! them, sends their clicks to the computer, carries out what comes back
//! for it, and folds none of their records. A process of its own, as a
//! phone is: it installs no plugins, so it has none.

use std::{cell::RefCell, rc::Rc};

use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext, size};
use serde_json::json;
use tau_agent::tool::RunId;
use tau_ui_plugin::{
    PluginStatus,
    PointCx as _,
    Request,
    points::{self, AtRun},
    tree::{Kind, Node},
    wire::{Built, Contributed, View},
};
use tau_ui_remote::{
    Workspace,
    catalog::{Catalog, PluginInfo, Repo},
    plugins,
    route::Route,
    update::HostUpdate,
    view::RunView,
    workspace::WorkspaceEvent,
};

const REPO: &str = "tau-agent";
const GOAL: &str = "tau-goal";

fn run_id() -> RunId {
    RunId("main".into())
}

fn catalog() -> Catalog {
    Catalog {
        plugins: vec![PluginInfo {
            name: GOAL.into(),
            description: "Goals".into(),
            reach: tau_ui_plugin::Reach {
                seams: Vec::new(),
                ..Default::default()
            },
            spend: 0.0,
            page: None,
            group: Default::default(),
            note: None,
            entries: Vec::new(),
            settings: false,
            ui: true,
            rewrites_keep_transcript: false,
        }],
        repos: vec![Repo {
            name: REPO.into(),
            main: Some(run_id()),
            ..Default::default()
        }],
        ..Default::default()
    }
}

type Events = Rc<RefCell<Vec<WorkspaceEvent>>>;

fn open(
    cx: &mut TestAppContext,
) -> (Entity<Workspace>, VisualTestContext, Events) {
    cx.update(tau_ui_remote::init);
    let run = RunView::new(run_id(), "main", "coder", "gpt-5.5").in_repo(REPO);
    let window = cx.add_window(|window, cx| {
        Workspace::new("tau", vec![run], catalog(), window, cx)
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
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.simulate_resize(size(gpui::px(1440.), gpui::px(900.)));
    workspace.update(&mut cx, |ws, cx| {
        ws.draw_remote_views();
        ws.navigate(Route::Run(run_id()), cx);
    });
    cx.run_until_parked();
    (workspace, cx, events)
}

fn status() -> PluginStatus {
    PluginStatus {
        name: GOAL.into(),
        state: "it ships · 0 of 3".into(),
        tone: Default::default(),
    }
}

/// What the computer would answer for each view the phone asked about:
/// goal's line and its banner with a Pause button, nothing elsewhere.
fn answer(asked: &[tau_ui_plugin::wire::ViewKey]) -> HostUpdate {
    let answers = asked
        .iter()
        .map(|key| {
            let out = match &key.view {
                View::Point { point, .. } if point == points::STATUS.name => {
                    Some(serde_json::to_value(status()).unwrap())
                }
                View::Point { point, .. }
                    if point == points::RUN_BANNER.name =>
                {
                    Some(
                        serde_json::to_value(Node::Button {
                            label: "Pause".into(),
                            action: "act".into(),
                            args: json!("Pause"),
                            kind: Kind::Secondary,
                            icon: None,
                        })
                        .unwrap(),
                    )
                }
                _ => None,
            };
            let outs = out
                .map(|out| Contributed {
                    plugin: GOAL.into(),
                    out,
                })
                .into_iter()
                .collect();
            (key.clone(), Built::Outs { outs })
        })
        .collect();
    HostUpdate::Views { answers }
}

#[gpui::test]
fn a_phone_without_plugins_draws_and_acts_through_the_computer(
    cx: &mut TestAppContext,
) {
    let (workspace, mut cx, events) = open(cx);
    let asked: Vec<_> = events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            WorkspaceEvent::AskViews(keys) => Some(keys.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    let at = |ws: &Workspace| AtRun {
        run: ws.run(&run_id()).unwrap().info(),
    };
    let (place, layout) = workspace
        .read_with(&cx, |ws, _| (at(ws).wire().unwrap(), ws.view_layout()));
    let point = |point: &str| tau_ui_plugin::wire::ViewKey {
        view: View::Point {
            point: point.to_owned(),
            at: place.clone(),
        },
        layout,
    };
    assert!(
        asked.contains(&point(points::RUN_BANNER.name)),
        "it asks for the run's banners: {asked:?}"
    );

    // The status line, drawn without an answer, is asked for once the
    // frame ends: the phone keeps answers only to what it asked.
    events.borrow_mut().clear();
    workspace.update(&mut cx, |ws, cx| {
        let at = at(ws);
        ws.contributions_named(points::STATUS, |_| Some(&at));
        ws.ask_for_views(cx);
    });
    let mut keys = asked.clone();
    keys.extend(
        events
            .borrow()
            .iter()
            .filter_map(|event| match event {
                WorkspaceEvent::AskViews(keys) => Some(keys.clone()),
                _ => None,
            })
            .flatten(),
    );
    assert!(keys.contains(&point(points::STATUS.name)), "{keys:?}");

    // The computer's answer is what it draws, under plugins it has no
    // code of.
    workspace.update(&mut cx, |ws, cx| ws.apply(answer(&keys), cx));
    cx.run_until_parked();
    let lines = workspace.read_with(&cx, |ws, _| {
        let at = at(ws);
        ws.contributions_named(points::STATUS, |_| Some(&at))
    });
    assert_eq!(lines, [(GOAL, status())]);

    // Its banner's button goes to the computer, as goal's action.
    events.borrow_mut().clear();
    let pause = cx.debug_bounds("button:Pause").expect("the banner draws");
    cx.simulate_click(pause.center(), Modifiers::none());
    cx.run_until_parked();
    let call = events.borrow().iter().find_map(|event| match event {
        WorkspaceEvent::PluginCall {
            id,
            plugin,
            origin,
            call,
            ..
        } => Some((*id, plugin.clone(), origin.clone(), call.clone())),
        _ => None,
    });
    let Some((id, plugin, origin, call)) = call else {
        panic!("no call went: {:?}", events.borrow());
    };
    assert_eq!(plugin, GOAL);
    assert_eq!(origin.run, Some(run_id()));
    assert_eq!(
        call,
        plugins::PluginCall::Action {
            action: "act".into(),
            args: json!("Pause"),
        }
    );

    // What comes back for its call is carried out here; another's is not.
    let back = |id| HostUpdate::Called {
        id,
        plugin: GOAL.into(),
        requests: vec![Request::Composer("/goal it ships".into())],
    };
    workspace.update(&mut cx, |ws, cx| ws.apply(back(id.wrapping_add(1)), cx));
    assert_eq!(
        workspace.read_with(&cx, |ws, cx| ws.composer_text(cx).to_owned()),
        ""
    );
    workspace.update(&mut cx, |ws, cx| ws.apply(back(id), cx));
    assert_eq!(
        workspace.read_with(&cx, |ws, cx| ws.composer_text(cx).to_owned()),
        "/goal it ships"
    );

    // It folds none of the plugin's records: the computer has its state.
    workspace.update(&mut cx, |ws, cx| {
        ws.apply(
            HostUpdate::PluginFold {
                run: run_id(),
                plugin: GOAL.into(),
                body: json!({ "kind": "paused" }),
            },
            cx,
        )
    });
    let states = workspace
        .read_with(&cx, |ws, _| ws.run(&run_id()).unwrap().plugin_states.len());
    assert_eq!(states, 0);
}

/// A phone that reaches the computer again asks again for every view it
/// draws: what the computer pushed while it was away went to nobody,
/// and a phone that came back from its last message gets no snapshot
/// to ask after. Found by `tau-ui`'s plugins model.
#[gpui::test]
fn a_phone_that_comes_back_asks_again_for_what_it_draws(
    cx: &mut TestAppContext,
) {
    use tau_ui_remote::pairing::{Computer, PairingUpdate};

    let (workspace, mut cx, events) = open(cx);
    let asked: Vec<_> = events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            WorkspaceEvent::AskViews(keys) => Some(keys.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    assert!(!asked.is_empty(), "it asks for what it draws");
    workspace.update(&mut cx, |ws, cx| ws.apply(answer(&asked), cx));
    cx.run_until_parked();
    events.borrow_mut().clear();

    let computer = Computer {
        name: "desk".into(),
        address: tau_remote::Address {
            host: "100.84.12.7".into(),
            port: tau_remote::Address::DEFAULT_PORT,
        },
        fingerprint: tau_remote::Fingerprint([7; 32]),
        via: None,
    };
    workspace.update(&mut cx, |ws, cx| {
        ws.update_pairing(PairingUpdate::Connected(computer), cx)
    });
    cx.run_until_parked();
    let again: Vec<_> = events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            WorkspaceEvent::AskViews(keys) => Some(keys.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    for key in &asked {
        assert!(again.contains(key), "{key:?} is not asked again: {again:?}");
    }
}
