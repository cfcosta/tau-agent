//! A plugin's actions in a repository page's header
//! (`points::REPO_ACTIONS`): drawn beside New run, each for the
//! repository the page is about, and doing what the plugin says when
//! clicked.

use std::{cell::RefCell, rc::Rc, sync::Once};

use gpui::{
    AnyElement,
    InteractiveElement as _,
    IntoElement as _,
    Modifiers,
    ParentElement as _,
    StatefulInteractiveElement as _,
    TestAppContext,
    VisualTestContext,
    div,
    size,
};
use tau_ui_kit::components::{ButtonKind, button};
use tau_ui_plugin::{Manifest, UiPlugin, ViewCx, points};
use tau_ui_remote::{
    Workspace,
    catalog::{Catalog, Repo},
    plugins,
    route::Route,
    workspace::WorkspaceEvent,
};

const REPO: &str = "tau-agent";

/// A plugin with one action for a repository: it asks its host half to
/// tidy that repository.
struct Tidy;

impl UiPlugin for Tidy {
    type State = ();
    type Data = ();
    type RepoData = ();
    type Settings = ();
    type Ui = ();

    fn name(&self) -> &'static str {
        "tidy"
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new().contribute(points::REPO_ACTIONS, action)
    }
}

fn action(
    at: &points::AtRepo,
    view: &mut ViewCx<'_, Tidy>,
) -> Option<AnyElement> {
    let (handle, repo) = (view.handle.clone(), at.repo.clone());
    Some(
        div()
            .id("tidy-action")
            .debug_selector(|| "tidy-action".to_owned())
            .child(button("Tidy", ButtonKind::Secondary, view.theme()))
            .on_click(move |_, _, cx| {
                handle.act(serde_json::json!({ "tidy": repo }), cx)
            })
            .into_any_element(),
    )
}

/// The app's plugins and the one above: a process has one registry.
fn install() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| plugins::install(plugins::plugins().with(Tidy)));
}

#[gpui::test]
fn a_plugin_s_action_is_drawn_on_the_repository_page_and_does_its_work(
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
    assert!(cx.debug_bounds("tidy-action").is_none(), "not on Home");

    workspace
        .update(&mut cx, |ws, cx| ws.navigate(Route::Repo(REPO.into()), cx));
    cx.run_until_parked();
    let action = cx.debug_bounds("tidy-action").expect("in the header");
    cx.simulate_click(action.center(), Modifiers::none());
    cx.run_until_parked();
    assert!(
        events.borrow().iter().any(|event| matches!(
            event,
            WorkspaceEvent::PluginAct { plugin, action }
                if plugin == "tidy" && action["tidy"] == REPO
        )),
        "{:?}",
        events.borrow()
    );
}
