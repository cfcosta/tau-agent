//! Keys plugins declare (`Manifest::key`): bound in the workspace, each
//! acting in the repository the person is in. A key tau binds stays
//! tau's, one an earlier plugin declared stays that plugin's, and one
//! that is not a key is dropped.

use std::{cell::RefCell, rc::Rc, sync::Once};

use gpui::{TestAppContext, VisualTestContext, size};
use tau_ui_plugin::{Manifest, UiPlugin};
use tau_ui_remote::{
    Workspace,
    catalog::{Catalog, Repo},
    plugins,
    route::{Route, Tab},
    workspace::WorkspaceEvent,
};

const REPO: &str = "tau-agent";

/// A plugin after memory that declares tau's ctrl-1, memory's ctrl-2, a
/// key gpui cannot read and a key of its own, each asking its host half
/// to say which.
struct Keys;

impl UiPlugin for Keys {
    type State = ();
    type Data = ();
    type RepoData = ();
    type Settings = ();
    type Ui = ();

    fn name(&self) -> &'static str {
        "keys"
    }

    fn manifest(&self) -> Manifest<Self> {
        let press = |key: &'static str| {
            move |at: &tau_ui_plugin::KeyAt,
                  view: &mut tau_ui_plugin::ViewCx<'_, Keys>| {
                view.handle.act(
                    serde_json::json!({ "key": key, "repo": at.repo }),
                    view.cx,
                );
            }
        };
        Manifest::new()
            .key("ctrl-1", press("ctrl-1"))
            .key("ctrl-2", press("ctrl-2"))
            .key("ctrl-5", press("ctrl-5"))
            .key("hyper-x", press("bad"))
    }
}

/// The app's plugins and the one above: a process has one registry.
fn install() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| plugins::install(plugins::plugins().with(Keys)));
}

#[gpui::test]
fn ctrl_2_opens_the_repositorys_memory_and_conflicting_keys_are_dropped(
    cx: &mut TestAppContext,
) {
    install();
    cx.update(tau_ui_remote::init);
    let window = cx.add_window(|window, cx| {
        let catalog = Catalog {
            repos: vec![Repo::new(REPO, "~/tau-agent")],
            open_repos: vec![REPO.into()],
            ..Default::default()
        };
        Workspace::new("tau", Vec::new(), catalog, window, cx)
    });
    let workspace = window.root(cx).unwrap();
    let events: Rc<RefCell<Vec<WorkspaceEvent>>> = Rc::default();
    let seen = events.clone();
    cx.update(|cx| {
        cx.subscribe(&workspace, move |_, event: &WorkspaceEvent, _| {
            seen.borrow_mut().push(event.clone())
        })
        .detach()
    });
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.simulate_resize(size(gpui::px(1200.), gpui::px(800.)));
    workspace.update_in(&mut cx, |ws, window, cx| {
        window.focus(&gpui::Focusable::focus_handle(ws, cx), cx)
    });
    cx.run_until_parked();
    let pressed = |events: &Rc<RefCell<Vec<WorkspaceEvent>>>| -> Vec<String> {
        events
            .borrow()
            .iter()
            .filter_map(|event| match event {
                WorkspaceEvent::PluginAct { plugin, action }
                    if plugin == "keys" =>
                {
                    Some(format!("{} {}", action["key"], action["repo"]))
                }
                _ => None,
            })
            .collect()
    };

    // Memory's: the repository's notes, as the old ctrl-2 opened them.
    cx.simulate_keystrokes("ctrl-2");
    cx.run_until_parked();
    workspace.read_with(&cx, |ws, _| {
        assert_eq!(
            *ws.route(),
            Route::Plugin {
                plugin: tau_memory::NAME.into(),
                page: "notes".into(),
                params: [("repo".to_owned(), REPO.to_owned())].into(),
            }
        )
    });

    // tau's own: the Runs tab.
    cx.simulate_keystrokes("ctrl-1");
    cx.run_until_parked();
    workspace
        .read_with(&cx, |ws, _| assert_eq!(*ws.route(), Tab::Runs.route()));

    // The plugin's own key, in the repository the person is in.
    cx.simulate_keystrokes("ctrl-5");
    cx.run_until_parked();
    assert_eq!(pressed(&events), [format!("\"ctrl-5\" \"{REPO}\"")]);
}
