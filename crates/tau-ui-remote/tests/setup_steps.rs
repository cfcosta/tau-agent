//! A plugin's step in first-run setup (`points::SETUP`): it comes after
//! the repositories, or before them when it says so, the step bar counts
//! it, it ends when the plugin says
//! so or the person skips it, and a step the plugin leaves out is not
//! there. The host hears once that setup ended, with the repositories
//! picked. What the plugin says it reads outside repositories shows
//! under its step, and on its page in the Plugins screen.

use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Mutex, Once},
};

use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext, size};
use tau_ui_plugin::{
    Manifest,
    PluginValue,
    UiPlugin,
    ViewCx,
    points::{self, AtSetup, SetupAt, SetupClone, SetupRepo, SetupView},
    tree::{Kind, Node},
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

const BEFORE: &str = "stepper-before";

/// The step of [`Before`].
const BEFORE_STEP: SetupStep = SetupStep::Plugin {
    plugin: BEFORE,
    step: "sign-in",
};

/// A plugin whose step comes before the repositories, as a sign-in's
/// does, shown while its data says so; its button ends it.
struct Before;

impl UiPlugin for Before {
    type State = ();
    type Data = bool;
    type RepoData = ();
    type Settings = ();

    fn name(&self) -> &'static str {
        BEFORE
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new().contribute(
            points::SETUP,
            |_: &AtSetup, view: &mut ViewCx<'_, Self>| {
                (*view.data).then(|| SetupView {
                    step: "sign-in",
                    at: SetupAt::BeforeRepositories,
                    title: "Sign in".into(),
                    heading: "Sign in first".into(),
                    detail: "Where the repositories come from.".into(),
                    body: Node::Button {
                        label: "Signed in".into(),
                        action: "done".into(),
                        args: serde_json::Value::Null,
                        kind: Kind::Primary,
                        icon: None,
                    },
                })
            },
        )
    }

    fn on_action(
        &self,
        action: &str,
        _: &serde_json::Value,
        view: &mut ViewCx<'_, Self>,
    ) {
        if action == "done" {
            view.handle.setup_done();
        }
    }
}

impl UiPlugin for Stepper {
    type State = ();
    /// Whether it has something to ask in setup.
    type Data = bool;
    type RepoData = ();
    type Settings = ();

    fn name(&self) -> &'static str {
        NAME
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
            .contribute(points::SETUP, step)
            .reads(&["~/.stepper"])
    }

    /// Its one button ends the step.
    fn on_action(
        &self,
        action: &str,
        _: &serde_json::Value,
        view: &mut ViewCx<'_, Self>,
    ) {
        if action == "done" {
            view.handle.setup_done();
        }
    }
}

fn step(at: &AtSetup, view: &mut ViewCx<'_, Stepper>) -> Option<SetupView> {
    if !*view.data {
        return None;
    }
    *SEEN.lock().unwrap() = at.repos.clone();
    let body = Node::Button {
        label: "Bring them".into(),
        action: "done".into(),
        args: serde_json::Value::Null,
        kind: Kind::Primary,
        icon: None,
    };
    Some(SetupView {
        step: "pick",
        at: SetupAt::AfterRepositories,
        title: "Stepper".into(),
        heading: "Bring what you had".into(),
        detail: "A step of a plugin's own.".into(),
        body,
    })
}

/// The app's plugins and the one above: a process has one registry.
fn install() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        plugins::install(tau_plugins::plugins().with(Stepper).with(Before))
    });
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

/// Onboarding signed in to a model, with one repository to pick, on the
/// repositories' step.
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
    workspace.update(cx, |ws, _| {
        ws.setup_stages()
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

/// The step comes after the repositories, the bar counts four stages,
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
        ["Model", "Repositories", "Stepper", "Ready"]
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
    // What it says it reads outside repositories shows under it.
    cx.run_until_parked();
    assert!(cx.debug_bounds("setup-reach").is_some());

    click("button:Bring them", &mut cx);
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
    click("button:Bring them", &mut cx);
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
/// three stages as before, and the repositories lead straight to Ready.
#[gpui::test]
fn an_absent_step_is_not_counted_or_shown(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open(false, cx);
    at_repositories(&workspace, &mut cx);
    assert_eq!(
        stage_names(&workspace, &mut cx),
        ["Model", "Repositories", "Ready"]
    );
    cx.run_until_parked();
    workspace.update(&mut cx, |ws, cx| ws.clone_selected(cx));
    assert_eq!(route(&workspace, &mut cx), Route::Setup(SetupStep::Ready));
    assert_eq!(finished(&events).len(), 1);
}

/// With no repositories to pick, the model leads straight to the
/// plugin's step; ending it leaves setup for a new run.
#[gpui::test]
fn without_repositories_the_model_leads_to_the_plugin_s_step(
    cx: &mut TestAppContext,
) {
    let (workspace, mut cx, events) = open(true, cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.set_setup(Setup::default(), cx);
        ws.start_setup(SetupStep::Welcome, cx);
        // What "Get started" does.
        ws.navigate(Route::Setup(SetupStep::Model), cx);
        ws.update_setup(
            SetupUpdate::Model(ModelAccess::Connected { label: "m".into() }),
            cx,
        );
        ws.continue_from_model(cx);
    });
    assert_eq!(route(&workspace, &mut cx), Route::Setup(STEP));
    click("button:Bring them", &mut cx);
    assert_eq!(route(&workspace, &mut cx), Route::NewRun);
    assert_eq!(finished(&events), [Vec::<SetupRepo>::new()]);
}

/// A step placed before the repositories comes right after the model,
/// and the bar counts it there; ending it opens the repositories once
/// the forge is signed in, which picking repositories from the app also
/// starts with while it is not.
#[gpui::test]
fn a_step_before_the_repositories_follows_the_model(cx: &mut TestAppContext) {
    let (workspace, mut cx, _events) = open(false, cx);
    workspace.update(&mut cx, |ws, cx| {
        let mut catalog = ws.catalog().clone();
        catalog.plugin_data.insert(
            BEFORE.into(),
            PluginValue::from_json(serde_json::json!(true)),
        );
        ws.set_catalog(catalog, cx);
        ws.set_setup(Setup::default(), cx);
        ws.start_setup(SetupStep::Model, cx);
    });
    assert_eq!(
        stage_names(&workspace, &mut cx),
        ["Model", "Sign in", "Repositories", "Ready"]
    );
    workspace.update(&mut cx, |ws, cx| {
        ws.update_setup(
            SetupUpdate::Model(ModelAccess::Connected { label: "m".into() }),
            cx,
        );
        ws.continue_from_model(cx);
    });
    assert_eq!(route(&workspace, &mut cx), Route::Setup(BEFORE_STEP));
    // Signed in: the forge lists repositories.
    workspace.update(&mut cx, |ws, cx| {
        let mut catalog = ws.catalog().clone();
        catalog.pull_requests = true;
        ws.set_catalog(catalog, cx);
    });
    click("button:Signed in", &mut cx);
    assert_eq!(route(&workspace, &mut cx), Route::Setup(SetupStep::Repos));

    // From the app, signed out: the sign-in first, then the repositories.
    workspace.update(&mut cx, |ws, cx| {
        let mut catalog = ws.catalog().clone();
        catalog.pull_requests = false;
        ws.set_catalog(catalog, cx);
        ws.finish_setup(cx);
        ws.pick_repos(cx);
    });
    assert_eq!(route(&workspace, &mut cx), Route::Setup(BEFORE_STEP));
    click("button:Signed in", &mut cx);
    assert_eq!(route(&workspace, &mut cx), Route::Setup(SetupStep::Repos));
}

/// The plugin's page in the Plugins screen says what it reads outside
/// repositories; one that reads nothing there says nothing of it.
#[gpui::test]
fn a_plugin_s_page_says_what_it_reads(cx: &mut TestAppContext) {
    let (workspace, mut cx, _events) = open(true, cx);
    workspace.update(&mut cx, |ws, cx| {
        let mut catalog = ws.catalog().clone();
        catalog.plugins.push(tau_ui_remote::catalog::PluginInfo {
            name: NAME.into(),
            description: "Steps.".into(),
            seams: Vec::new(),
            spend: 0.,
            page: None,
            group: tau_ui_plugin::Group::Context,
            note: None,
            entries: Vec::new(),
            settings: false,
            ui: true,
            rewrites_keep_transcript: false,
        });
        ws.set_catalog(catalog, cx);
        ws.navigate(Route::Plugins, cx);
        ws.pick_plugin(NAME, None, cx);
        ws.set_plugin_tab(tau_ui_remote::ui::screens::plugins::Tab::About, cx);
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("plugin-reach").is_some());
    assert_eq!(
        plugins::registry().get(NAME).unwrap().reads(),
        ["~/.stepper"]
    );
    assert!(
        plugins::registry()
            .get("tau-goal")
            .unwrap()
            .reads()
            .is_empty()
    );
}
