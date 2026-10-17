//! The status bar under the desktop: the jobs plugins run, each with its
//! progress, opening the page it names when clicked; several collapse to
//! a count; and what a plugin adds there from its data
//! (`points::ACTIVITY`).

use std::sync::Once;

use gpui::{
    AnyElement,
    Entity,
    InteractiveElement as _,
    IntoElement as _,
    Modifiers,
    ParentElement as _,
    TestAppContext,
    VisualTestContext,
    div,
    size,
};
use tau_ui_plugin::{
    JobInfo,
    Link,
    Manifest,
    Page,
    PluginValue,
    UiPlugin,
    ViewCx,
    points,
};
use tau_ui_remote::{
    Workspace,
    catalog::{Catalog, PluginInfo, Repo},
    plugins,
    route::Route,
};

const NAME: &str = "worker";
const REPO: &str = "tau-agent";

/// A plugin that runs jobs, has a review page and a page of its own, and
/// says in the status bar what its data holds.
struct Worker;

impl UiPlugin for Worker {
    type State = ();
    /// What it has to say in the status bar, if anything.
    type Data = String;
    type RepoData = ();
    type Settings = ();
    type Ui = ();

    fn name(&self) -> &'static str {
        NAME
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
            .page(Page::new("review", |_| div().into_any_element()))
            .page(Page::new("main", |_| div().into_any_element()))
            .contribute(points::ACTIVITY, activity)
    }
}

fn activity(
    _: &points::AtApp,
    view: &mut ViewCx<'_, Worker>,
) -> Option<AnyElement> {
    if view.data.is_empty() {
        return None;
    }
    Some(
        div()
            .debug_selector(|| "worker-activity".to_owned())
            .child(view.data.clone())
            .into_any_element(),
    )
}

/// The app's plugins and the one above: a process has one registry.
fn install() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| plugins::install(plugins::plugins().with(Worker)));
}

fn job(name: &str, page: Option<Link>) -> JobInfo {
    JobInfo {
        plugin: NAME.into(),
        name: name.into(),
        label: "Importing notes".into(),
        fraction: 0.4,
        line: "Reading ~/.codex/sessions".into(),
        page,
    }
}

/// A desktop window whose catalog lists `jobs`, and the plugin's data.
fn open(
    jobs: Vec<JobInfo>,
    data: &str,
    cx: &mut TestAppContext,
) -> (Entity<Workspace>, VisualTestContext) {
    install();
    cx.update(tau_ui_remote::init);
    let window = cx.add_window(|window, cx| {
        let mut catalog = Catalog {
            repos: vec![Repo::new(REPO, "~/tau-agent")],
            open_repos: vec![REPO.into()],
            plugins: vec![PluginInfo {
                name: NAME.into(),
                description: String::new(),
                seams: Vec::new(),
                spend: 0.,
                page: Some(Link::page("main")),
                group: Default::default(),
                note: None,
                entries: Vec::new(),
                settings: false,
            }],
            jobs,
            ..Default::default()
        };
        catalog
            .plugin_data
            .insert(NAME.into(), PluginValue::from_json(data.into()));
        Workspace::new("tau", Vec::new(), catalog, window, cx)
    });
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx);
    cx.simulate_resize(size(gpui::px(1200.), gpui::px(800.)));
    cx.run_until_parked();
    (workspace, cx)
}

fn click(selector: &'static str, cx: &mut VisualTestContext) {
    let bounds = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is drawn"));
    cx.simulate_click(bounds.center(), Modifiers::none());
    cx.run_until_parked();
}

fn route(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Route {
    workspace.read_with(cx, |ws, _| ws.route().clone())
}

/// A running job shows in the bar; clicking it opens the page it names.
#[gpui::test]
fn a_running_job_shows_and_opens_its_page(cx: &mut TestAppContext) {
    let page = Link::page("review").param("repo", REPO);
    let (workspace, mut cx) = open(vec![job("import", Some(page))], "", cx);
    click("status-job-worker-import", &mut cx);
    assert_eq!(
        route(&workspace, &mut cx),
        Route::Plugin {
            plugin: NAME.into(),
            page: "review".into(),
            params: [("repo".to_owned(), REPO.to_owned())].into(),
        }
    );
}

/// A job that names no page opens its plugin's own.
#[gpui::test]
fn a_job_without_a_page_opens_its_plugin_s(cx: &mut TestAppContext) {
    let (workspace, mut cx) = open(vec![job("import", None)], "", cx);
    click("status-job-worker-import", &mut cx);
    assert_eq!(
        route(&workspace, &mut cx),
        Route::Plugin {
            plugin: NAME.into(),
            page: "main".into(),
            params: Default::default(),
        }
    );
}

/// Two jobs show side by side; three collapse to a count, the first of
/// them shown.
#[gpui::test]
fn several_jobs_collapse_to_a_count(cx: &mut TestAppContext) {
    let (_, mut cx) = open(vec![job("a", None), job("b", None)], "", cx);
    assert!(cx.debug_bounds("status-job-worker-a").is_some());
    assert!(cx.debug_bounds("status-job-worker-b").is_some());

    let jobs = vec![job("a", None), job("b", None), job("c", None)];
    let (_, mut cx) = open(jobs, "", &mut cx);
    assert!(cx.debug_bounds("status-job-worker-a").is_some());
    assert!(cx.debug_bounds("status-job-worker-b").is_none());
    assert!(cx.debug_bounds("status-job-worker-c").is_none());
}

/// What a plugin adds to the bar from its data shows; with nothing to
/// say and no jobs, there is no bar to show it in.
#[gpui::test]
fn a_plugin_s_activity_shows_from_its_data(cx: &mut TestAppContext) {
    let (_, mut cx) = open(Vec::new(), "3 notes to review", cx);
    assert!(cx.debug_bounds("worker-activity").is_some());

    let (_, mut cx) = open(Vec::new(), "", &mut cx);
    assert!(cx.debug_bounds("worker-activity").is_none());
}
