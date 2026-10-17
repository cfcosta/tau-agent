//! A plugin's step in first-run setup (`points::SETUP`): it comes after
//! the repositories, the step bar counts it, it ends when the plugin says
//! so or the person skips it, and a step the plugin leaves out is not
//! there. The host hears once that setup ended, with the repositories
//! picked.

use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Mutex, Once},
};

use gpui::{
    AnyElement,
    Entity,
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
use tau_ui_plugin::{
    Manifest,
    PluginValue,
    UiPlugin,
    ViewCx,
    points::{self, AtSetup, SetupClone, SetupRepo, SetupView},
};
use tau_ui_remote::{
    Workspace,
    catalog::Catalog,
    plugins,
    route::Route,
    setup::{
        CloneState,
        ModelAccess,
        RepoChoice,
        RepoClone,
        Setup,
        SetupStep,
        SetupUpdate,
    },
    workspace::WorkspaceEvent,
};

const NAME: &str = "stepper";

/// The step of the plugin below.
const STEP: SetupStep = SetupStep::Plugin {
    plugin: NAME,
    step: "pick",
};

/// What the step was last given: the repositories picked so far.
static SEEN: Mutex<Vec<SetupRepo>> = Mutex::new(Vec::new());

/// A plugin with one setup step, shown while its data says so; its
/// button ends it.
struct Stepper;

impl UiPlugin for Stepper {
    type State = ();
    /// Whether it has something to ask in setup.
    type Data = bool;
    type RepoData = ();
    type Settings = ();
    type Ui = ();

    fn name(&self) -> &'static str {
        NAME
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new().contribute(points::SETUP, step)
    }
}

fn step(at: &AtSetup, view: &mut ViewCx<'_, Stepper>) -> Option<SetupView> {
    if !*view.data {
        return None;
    }
    *SEEN.lock().unwrap() = at.repos.clone();
    let handle = view.handle.clone();
    let body: AnyElement = div()
        .id("stepper-done")
        .debug_selector(|| "stepper-done".to_owned())
        .child(button("Bring them", ButtonKind::Primary, view.theme()))
        .on_click(move |_, _, cx| handle.setup_done(cx))
        .into_any_element();
    Some(SetupView {
        step: "pick",
        title: "Stepper".into(),
        heading: "Bring what you had".into(),
        detail: "A step of a plugin's own.".into(),
        body,
    })
}

/// The app's plugins and the one above: a process has one registry.
fn install() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| plugins::install(plugins::plugins().with(Stepper)));
}

type Events = Rc<RefCell<Vec<WorkspaceEvent>>>;

/// A desktop window whose catalog says whether the plugin's step shows.
fn open(
    shows: bool,
    cx: &mut TestAppContext,
) -> (Entity<Workspace>, VisualTestContext, Events) {
    install();
    cx.update(tau_ui_remote::init);
    let window = cx.add_window(|window, cx| {
        let mut catalog = Catalog::default();
        catalog.plugin_data.insert(
            NAME.into(),
            PluginValue::from_json(serde_json::json!(shows)),
        );
        Workspace::new("tau", Vec::new(), catalog, window, cx)
    });
    let workspace = window.root(cx).unwrap();
    let events: Events = Rc::default();
    let seen = events.clone();
    cx.update(|cx| {
        cx.subscribe(&workspace, move |_, event: &WorkspaceEvent, _| {
            seen.borrow_mut().push(event.clone())
        })
        .detach()
    });
    let cx = VisualTestContext::from_window(window.into(), cx);
    cx.simulate_resize(size(gpui::px(1200.), gpui::px(800.)));
    (workspace, cx, events)
}

/// Onboarding signed in to GitHub and a model, with one repository to
/// pick, on the repositories' step.
fn at_repositories(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) {
    workspace.update(cx, |ws, cx| {
        ws.set_setup(
            Setup {
                model: ModelAccess::Connected { label: "m".into() },
                repos: vec![RepoChoice {
                    name: "octocat/hello".into(),
                    description: String::new(),
                    branch: "main".into(),
                    selected: true,
                    pushed_at: String::new(),
                    remote: Some("https://github.com/octocat/hello.git".into()),
                }],
                ..Setup::default()
            },
            cx,
        );
        ws.start_setup(SetupStep::Repos, cx);
    });
}

fn stage_names(
    workspace: &Entity<Workspace>,
    cx: &mut VisualTestContext,
) -> Vec<String> {
    workspace.update(cx, |ws, cx| {
        ws.setup_stages(cx)
            .into_iter()
            .map(|stage| stage.name)
            .collect()
    })
}

fn route(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Route {
    workspace.read_with(cx, |ws, _| ws.route().clone())
}

fn click(selector: &'static str, cx: &mut VisualTestContext) {
    cx.run_until_parked();
    let bounds = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is drawn"));
    cx.simulate_click(bounds.center(), Modifiers::none());
    cx.run_until_parked();
}

fn finished(events: &Events) -> Vec<Vec<SetupRepo>> {
    events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            WorkspaceEvent::SetupFinished { repos } => Some(repos.clone()),
            _ => None,
        })
        .collect()
}

/// The step comes after the repositories, the bar counts five stages,
/// the step sees the picked repository, and its own button ends it: on
/// to Ready, and the host hears setup ended, once, even after going back
/// and through the step again.
#[gpui::test]
fn a_plugin_s_step_follows_the_repositories_and_ends_when_it_says(
    cx: &mut TestAppContext,
) {
    let (workspace, mut cx, events) = open(true, cx);
    at_repositories(&workspace, &mut cx);
    assert_eq!(
        stage_names(&workspace, &mut cx),
        ["GitHub", "Model", "Repositories", "Stepper", "Ready"]
    );
    workspace.update(&mut cx, |ws, cx| ws.clone_selected(cx));
    assert_eq!(route(&workspace, &mut cx), Route::Setup(STEP));
    let picked = SEEN.lock().unwrap().clone();
    assert_eq!(
        picked,
        [SetupRepo {
            name: "octocat/hello".into(),
            id: None,
            remote: Some("https://github.com/octocat/hello.git".into()),
            folder: None,
            clone: SetupClone::Cloning(0.),
        }]
    );
    assert!(finished(&events).is_empty(), "not yet");

    click("stepper-done", &mut cx);
    assert_eq!(route(&workspace, &mut cx), Route::Setup(SetupStep::Ready));
    assert_eq!(finished(&events), [picked]);

    // Back to the step, and on again: the host heard it already.
    workspace.update(&mut cx, |ws, cx| ws.back(cx));
    assert_eq!(route(&workspace, &mut cx), Route::Setup(STEP));
    workspace.update(&mut cx, |ws, cx| {
        ws.update_setup(
            SetupUpdate::Clone(RepoClone {
                name: "octocat/hello".into(),
                state: CloneState::Ready,
                id: Some(tau_ui_plugin::RepoId("id-hello".into())),
            }),
            cx,
        )
    });
    click("stepper-done", &mut cx);
    assert_eq!(route(&workspace, &mut cx), Route::Setup(SetupStep::Ready));
    assert_eq!(finished(&events).len(), 1, "said once");
    assert_eq!(
        SEEN.lock().unwrap()[0].id,
        Some(tau_ui_plugin::RepoId("id-hello".into())),
        "the clone's id, once listed"
    );
}

/// The person can skip the step: tau draws the way past it.
#[gpui::test]
fn skipping_a_plugin_s_step_goes_on_to_ready(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open(true, cx);
    at_repositories(&workspace, &mut cx);
    workspace.update(&mut cx, |ws, cx| ws.clone_selected(cx));
    assert_eq!(route(&workspace, &mut cx), Route::Setup(STEP));
    click("setup-skip", &mut cx);
    assert_eq!(route(&workspace, &mut cx), Route::Setup(SetupStep::Ready));
    assert_eq!(finished(&events).len(), 1);
}

/// A step the plugin leaves out is not in the bar, which counts tau's
/// four stages as before, and the repositories lead straight to Ready.
#[gpui::test]
fn an_absent_step_is_not_counted_or_shown(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open(false, cx);
    at_repositories(&workspace, &mut cx);
    assert_eq!(
        stage_names(&workspace, &mut cx),
        ["GitHub", "Model", "Repositories", "Ready"]
    );
    cx.run_until_parked();
    workspace.update(&mut cx, |ws, cx| ws.clone_selected(cx));
    assert_eq!(route(&workspace, &mut cx), Route::Setup(SetupStep::Ready));
    assert_eq!(finished(&events).len(), 1);
}

/// Skipping GitHub, with no repositories to pick, still reaches the
/// plugin's step after the model; ending it leaves setup for a new run.
#[gpui::test]
fn skipping_github_still_reaches_the_plugin_s_step(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open(true, cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.set_setup(Setup::default(), cx);
        ws.start_setup(SetupStep::Welcome, cx);
        // What "Skip GitHub for now" does.
        ws.navigate(Route::Setup(SetupStep::Model), cx);
        ws.update_setup(
            SetupUpdate::Model(ModelAccess::Connected { label: "m".into() }),
            cx,
        );
        ws.continue_from_model(cx);
    });
    assert_eq!(route(&workspace, &mut cx), Route::Setup(STEP));
    click("stepper-done", &mut cx);
    assert_eq!(route(&workspace, &mut cx), Route::NewRun);
    assert_eq!(finished(&events), [Vec::<SetupRepo>::new()]);
}
