//! What a plugin lists on Home of the selected repository
//! (`points::HOME`): a titled section of rows under the new run's
//! composer, a row writing in the composer or carrying out the plugin's
//! action in the repository; no section for a list without rows.

use std::{cell::RefCell, rc::Rc, sync::Once};

use gpui::{Modifiers, TestAppContext, VisualTestContext, size};
use heiko_ui_plugin::{
    Manifest,
    UiPlugin,
    ViewCx,
    points::{self, HomeAction, HomeList, HomeRow},
};
use heiko_ui_remote::{
    Workspace,
    catalog::{Catalog, Repo},
    plugins,
    workspace::WorkspaceEvent,
};

const REPO: &str = "heiko-agent";
const EMPTY: &str = "docbert";

/// A plugin that lists two issues of [`REPO`], and none of any other
/// repository.
struct Issues;

impl UiPlugin for Issues {
    type State = ();
    type Data = ();
    type RepoData = ();
    type Settings = ();

    fn name(&self) -> &'static str {
        "issues"
    }

    fn declaration(&self) -> heiko_ui_plugin::Declaration {
        heiko_ui_plugin::Declaration::new()
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new().contribute(points::HOME, list)
    }

    /// Starting an issue asks the host, with the repository it is of.
    fn on_action(
        &self,
        action: &str,
        args: &serde_json::Value,
        view: &mut ViewCx<'_, Self>,
    ) {
        if action == "start" {
            view.handle.act(serde_json::json!({
                "issue": args["issue"],
                "repo": view.params.get("repo"),
            }));
        }
    }
}

fn list(at: &points::AtRepo, _: &mut ViewCx<'_, Issues>) -> Option<HomeList> {
    let rows = if at.repo == REPO {
        vec![
            HomeRow {
                id: "T-34".into(),
                title: "Log malformed headers once per run".into(),
                detail: "P3".into(),
                action: HomeAction::Write("Work on T-34.".into()),
            },
            HomeRow {
                id: "T-36".into(),
                title: "Jitter on top of the hint".into(),
                detail: "P2".into(),
                action: HomeAction::Act {
                    action: "start".into(),
                    args: serde_json::json!({ "issue": "T-36" }),
                },
            },
        ]
    } else {
        Vec::new()
    };
    Some(HomeList {
        title: "Ready to pick up".into(),
        meta: Some("this repository's issues".into()),
        rows,
    })
}

/// The app's plugins and the one above: a process has one registry.
fn install() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| plugins::install(heiko_plugins::plugins().with(Issues)));
}

fn workspace(
    repo: &str,
    cx: &mut TestAppContext,
) -> (gpui::Entity<Workspace>, gpui::AnyWindowHandle) {
    install();
    cx.update(heiko_ui_remote::init);
    let repo = repo.to_owned();
    let window = cx.add_window(|window, cx| {
        let catalog = Catalog {
            repos: vec![Repo::new(repo.clone(), format!("~/{repo}"))],
            open_repos: vec![repo.clone()],
            ..Default::default()
        };
        Workspace::new("heiko", Vec::new(), catalog, window, cx)
    });
    (window.root(cx).unwrap(), window.into())
}

#[gpui::test]
fn a_plugin_s_list_is_on_home_and_its_rows_do_what_they_say(
    cx: &mut TestAppContext,
) {
    let (workspace, window) = workspace(REPO, cx);
    let events: Rc<RefCell<Vec<WorkspaceEvent>>> = Rc::default();
    let seen = events.clone();
    cx.update(|cx| {
        cx.subscribe(&workspace, move |_, event: &WorkspaceEvent, _| {
            seen.borrow_mut().push(event.clone())
        })
        .detach()
    });
    let mut cx = VisualTestContext::from_window(window, cx);
    cx.simulate_resize(size(gpui::px(1200.), gpui::px(800.)));
    cx.run_until_parked();

    // The first row writes in the composer.
    let first = cx.debug_bounds("home-row:T-34").expect("its first row");
    let second = cx.debug_bounds("home-row:T-36").expect("its second row");
    assert!(first.origin.y < second.origin.y, "in the plugin's order");
    cx.simulate_click(first.center(), Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        workspace.read_with(&cx, |ws, cx| ws.composer_text(cx).to_owned()),
        "Work on T-34."
    );

    // The second is the plugin's action, in the repository.
    cx.simulate_click(second.center(), Modifiers::none());
    cx.run_until_parked();
    assert!(
        events.borrow().iter().any(|event| matches!(
            event,
            WorkspaceEvent::PluginAct { plugin, action, .. }
                if plugin == "issues"
                    && action["issue"] == "T-36"
                    && action["repo"] == REPO
        )),
        "{:?}",
        events.borrow()
    );
}

#[gpui::test]
fn a_list_without_rows_has_no_section(cx: &mut TestAppContext) {
    let (_workspace, window) = workspace(EMPTY, cx);
    let mut cx = VisualTestContext::from_window(window, cx);
    cx.simulate_resize(size(gpui::px(1200.), gpui::px(800.)));
    cx.run_until_parked();
    assert!(cx.debug_bounds("home-row:T-34").is_none());
}
