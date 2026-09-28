//! The workspace's onboarding and pull request flows, driven the way a
//! host drives them, in GPUI's test app.

use gpui::{Entity, TestAppContext, VisualTestContext};
use tau_ui::{
    Workspace,
    WorkspaceEvent,
    catalog::Catalog,
    demo,
    models::{Effort, ModelChoice, ModelSettings},
    pull_request::PrState,
    route::Route,
    setup::{GitHub, ModelAccess, Setup, SetupStep, SetupUpdate},
    view::{BranchCode, CodeState},
    workspace::PickerTarget,
};

fn open(
    cx: &mut TestAppContext,
) -> (
    Entity<Workspace>,
    VisualTestContext,
    std::rc::Rc<std::cell::RefCell<Vec<WorkspaceEvent>>>,
) {
    cx.update(tau_ui::init);
    let window = cx.add_window(|window, cx| {
        Workspace::new(
            "tau",
            vec![demo::retry_after()],
            Catalog::default(),
            window,
            cx,
        )
    });
    let workspace = window.root(cx).unwrap();
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = events.clone();
    cx.update(|cx| {
        cx.subscribe(&workspace, move |_, event: &WorkspaceEvent, _| {
            seen.borrow_mut().push(event.clone())
        })
        .detach()
    });
    let visual = VisualTestContext::from_window(window.into(), cx);
    (workspace, visual, events)
}

#[gpui::test]
fn setup_moves_on_as_sign_ins_land(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open(cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.set_setup(demo::setup(SetupStep::Welcome), cx);
        ws.start_setup(SetupStep::Welcome, cx);
        ws.sign_in_github(cx);
    });
    assert_eq!(events.borrow().as_slice(), [WorkspaceEvent::GitHubSignIn]);
    workspace.update(&mut cx, |ws, cx| {
        assert_eq!(ws.route(), &Route::Setup(SetupStep::GitHub));
        // Asking again while a code is showing does not start over.
        ws.update_setup(SetupUpdate::GitHub(GitHub::SignedOut), cx);
        ws.update_setup(
            SetupUpdate::GitHub(GitHub::SignedIn { user: "ada".into() }),
            cx,
        );
        assert_eq!(ws.route(), &Route::Setup(SetupStep::Model));
        ws.sign_in_codex(true, cx);
        ws.update_setup(
            SetupUpdate::Model(ModelAccess::Connected { label: "m".into() }),
            cx,
        );
        assert_eq!(ws.route(), &Route::Setup(SetupStep::Repos));
        ws.clone_selected(cx);
        assert_eq!(ws.route(), &Route::Setup(SetupStep::Ready));
    });
    let events = events.borrow();
    assert_eq!(events[1], WorkspaceEvent::CodexSignIn { device: true });
    assert_eq!(
        events[2],
        WorkspaceEvent::CloneRepos {
            repos: vec!["cfcosta/tau-agent".into(), "cfcosta/docbert".into()],
        }
    );
}

#[gpui::test]
fn setup_without_repositories_ends_at_a_new_run(cx: &mut TestAppContext) {
    let (workspace, mut cx, _) = open(cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.set_setup(Setup::default(), cx);
        ws.start_setup(SetupStep::Model, cx);
        ws.update_setup(
            SetupUpdate::Model(ModelAccess::Connected { label: "m".into() }),
            cx,
        );
        assert_eq!(ws.route(), &Route::Home);
        assert!(!ws.can_go_back());
    });
}

#[gpui::test]
fn pull_requests_are_drafted_then_created(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open(cx);
    let run = demo::run_id();
    workspace.update(&mut cx, |ws, cx| ws.open_pull_request(&run, cx));
    assert_eq!(
        events.borrow().as_slice(),
        [WorkspaceEvent::PreparePullRequest { run: run.clone() }]
    );
    workspace.update(&mut cx, |ws, cx| {
        ws.set_pull_request(&run, demo::pull_request(), cx);
        ws.create_pull_request(&run, cx);
        assert_eq!(ws.pull_request(&run).unwrap().state, PrState::Creating);
    });
    match &events.borrow()[1] {
        WorkspaceEvent::CreatePullRequest {
            title,
            draft,
            reviewers,
            ..
        } => {
            // The title field took the draft's title.
            assert_eq!(title, "Honor retry-after on 429 and 503");
            assert!(*draft);
            assert!(reviewers.is_empty());
        }
        other => panic!("expected a pull request, got {other:?}"),
    }
    workspace.update(&mut cx, |ws, cx| {
        ws.set_pull_request_state(&run, demo::opened(), cx);
        assert!(ws.pull_request(&run).unwrap().is_open());
    });
}

#[gpui::test]
fn fork_mode_sends_the_chosen_turn(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open(cx);
    let run = demo::run_id();
    workspace.update(&mut cx, |ws, cx| {
        for (_, update) in demo::script() {
            ws.update_run(&run, update, cx);
        }
        ws.navigate(Route::Run(run.clone()), cx);
    });
    let last = workspace.read_with(&cx, |ws, _| ws.run(&run).unwrap().turn);
    assert!(last > 2);
    workspace.update_in(&mut cx, |ws, window, cx| {
        ws.start_fork(window, cx);
        ws.submit_prompt("try a longer backoff".into(), cx);
    });
    assert_eq!(
        events.borrow().last(),
        Some(&WorkspaceEvent::Fork {
            run: run.clone(),
            turn: Some(last),
            prompt: "try a longer backoff".into(),
            // A fork starts on its run's model and effort.
            model: ModelChoice::new("gpt-5.5", Effort::High),
        })
    );
    // The next message is an ordinary one again.
    workspace.update(&mut cx, |ws, cx| {
        ws.submit_prompt("hello".into(), cx);
    });
    assert!(matches!(
        events.borrow().last(),
        Some(WorkspaceEvent::NewRun { .. })
    ));
}

#[gpui::test]
fn comparing_asks_for_the_code_once(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open(cx);
    let (main, fork) = (demo::run_id(), demo::fork_id());
    let compare = Route::Compare {
        main: main.clone(),
        fork: fork.clone(),
    };
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(compare.clone(), cx);
        assert_eq!(ws.branch_code(&main, &fork), Some(&CodeState::Loading));
        ws.back(cx);
        ws.navigate(compare.clone(), cx);
        ws.set_branch_code(
            &main,
            &fork,
            CodeState::Ready(BranchCode::default()),
            cx,
        );
    });
    let asked = events
        .borrow()
        .iter()
        .filter(|event| matches!(event, WorkspaceEvent::CompareCode { .. }))
        .count();
    assert_eq!(asked, 1);
    workspace.read_with(&cx, |ws, _| {
        assert!(matches!(
            ws.branch_code(&main, &fork),
            Some(CodeState::Ready(_))
        ));
    });
}

#[gpui::test]
fn forking_from_a_turn_sends_that_turn(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open(cx);
    let run = demo::run_id();
    workspace.update(&mut cx, |ws, cx| {
        for (_, update) in demo::script() {
            ws.update_run(&run, update, cx);
        }
    });
    workspace.update_in(&mut cx, |ws, window, cx| {
        ws.fork_from(&run, 2, window, cx);
        ws.submit_prompt("go another way".into(), cx);
    });
    assert_eq!(
        events.borrow().last(),
        Some(&WorkspaceEvent::Fork {
            run: run.clone(),
            turn: Some(2),
            prompt: "go another way".into(),
            model: ModelChoice::new("gpt-5.5", Effort::High),
        })
    );
    workspace.read_with(&cx, |ws, _| {
        let view = ws.run(&run).unwrap();
        assert!(tau_ui::Workspace::can_fork_at(view, 2));
        assert!(!tau_ui::Workspace::can_fork_at(view, 0));
        assert!(!tau_ui::Workspace::can_fork_at(view, view.turn + 1));
    });
}

#[gpui::test]
fn the_demo_answers_a_fork_with_a_run(cx: &mut TestAppContext) {
    let (workspace, mut cx, _) = open(cx);
    cx.update(|_, cx| demo::respond(&workspace, cx));
    let run = demo::run_id();
    workspace.update(&mut cx, |ws, cx| {
        for (_, update) in demo::script() {
            ws.update_run(&run, update, cx);
        }
    });
    workspace.update_in(&mut cx, |ws, window, cx| {
        ws.fork_from(&run, 2, window, cx);
        ws.submit_prompt("double the delay instead".into(), cx);
    });
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(30));
    cx.run_until_parked();
    workspace.read_with(&cx, |ws, _| {
        let fork = ws.current().expect("the fork is open");
        assert_eq!(
            fork.origin,
            tau_ui::view::Origin::Fork {
                from: run.clone(),
                turn: 2
            }
        );
        assert_eq!(fork.title, "double-the-delay-instead");
        // It runs on its run's model, which the fork kept.
        assert_eq!(fork.model, "gpt-5.5");
        assert!(!fork.status.is_live(), "the fork played to its end");
        assert!(fork.items.iter().any(|item| matches!(
            item,
            tau_ui::view::Item::TurnEnd { turn: 3 }
        )));
        let parent = ws.run(&run).unwrap();
        assert!(parent.children.iter().any(|child| child.id == fork.id));
    });
}

#[gpui::test]
fn escape_closes_an_alert_before_going_back(cx: &mut TestAppContext) {
    let (workspace, mut cx, _) = open(cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::History, cx);
        ws.show_alert("Could not fork the run", "no project", cx);
        ws.escape(cx);
        assert!(ws.alert().is_none());
        assert_eq!(ws.route(), &Route::History, "the dialog took the escape");
        ws.escape(cx);
        assert_ne!(ws.route(), &Route::History);
    });
}

#[gpui::test]
fn a_failed_fork_opens_a_dialog(cx: &mut TestAppContext) {
    use tau_testing::scripted::ScriptedModel;
    use tau_ui::host::{Access, Host, HostConfig};

    let (workspace, mut cx, _) = open(cx);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let store = runtime.block_on(tau_store::Store::memory()).unwrap();
    let agent =
        tau_agent::agent::Agent::new(ScriptedModel::new()).name("coder");
    let config = HostConfig {
        access: Access::ApiKey("sk-test".into()),
        model: "gpt-5.5".into(),
        root: std::env::temp_dir(),
        store: std::env::temp_dir().join("unused.db"),
        repos: std::env::temp_dir().join("unused-repos"),
        settings: std::env::temp_dir().join("unused-models.json"),
    };
    // No project: runs work in the checkout, and forking cannot work.
    let (host, events) = Host::with_agent(runtime, agent, store, config);
    cx.update(|_, cx| host.attach(&workspace, events, cx));
    let run = demo::run_id();
    workspace.update(&mut cx, |ws, cx| {
        for (_, update) in demo::script() {
            ws.update_run(&run, update, cx);
        }
    });
    workspace.update_in(&mut cx, |ws, window, cx| {
        ws.fork_from(&run, 2, window, cx);
        ws.submit_prompt("try again".into(), cx);
    });
    cx.run_until_parked();
    workspace.read_with(&cx, |ws, _| {
        let (title, message) = ws.alert().expect("a dialog");
        assert_eq!(title, "Could not fork the run");
        assert!(message.starts_with("Forking needs a project"), "{message}");
    });
}

fn open_with_models(
    cx: &mut TestAppContext,
) -> (
    Entity<Workspace>,
    VisualTestContext,
    std::rc::Rc<std::cell::RefCell<Vec<WorkspaceEvent>>>,
) {
    let (workspace, mut cx, events) = open(cx);
    workspace.update(&mut cx, |ws, cx| {
        let mut catalog = ws.catalog().clone();
        catalog.models = demo::models();
        ws.set_catalog(catalog, cx);
    });
    (workspace, cx, events)
}

#[gpui::test]
fn the_next_run_starts_on_the_picked_model(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open_with_models(cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::NewRun, cx);
        assert_eq!(ws.next_model(), &ModelChoice::new("gpt-5.5", Effort::Auto));
        ws.show_picker(PickerTarget::Next, cx);
        ws.pick_effort(Effort::Low, cx);
        ws.pick_model("gpt-6-sol", cx);
        assert!(ws.picker().is_none(), "picking a model closes the picker");
        ws.submit_prompt("fix it".into(), cx);
    });
    assert_eq!(
        events.borrow().last(),
        Some(&WorkspaceEvent::NewRun {
            prompt: "fix it".into(),
            model: ModelChoice::new("gpt-6-sol", Effort::Low),
        })
    );
}

#[gpui::test]
fn a_pricey_model_asks_first_and_a_locked_one_is_not_picked(
    cx: &mut TestAppContext,
) {
    let (workspace, mut cx, _) = open_with_models(cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.show_picker(PickerTarget::Next, cx);
        // Locked: the ChatGPT sign-in cannot run it.
        ws.pick_model("gpt-5.5-pro", cx);
        assert_eq!(ws.next_model().model, "gpt-5.5");
        assert!(ws.alert().is_none());
        // $50 per M out is above the $20 to ask about.
        ws.pick_model("gpt-6-astra", cx);
        let (title, _) = ws.alert().expect("a price check");
        assert_eq!(title, "Use gpt-6-astra?");
        assert_eq!(ws.next_model().model, "gpt-5.5", "not until confirmed");
        ws.dismiss_alert(cx);
        assert_eq!(ws.next_model().model, "gpt-5.5", "cancel keeps the model");
        ws.pick_model("gpt-6-astra", cx);
        ws.confirm_dialog(cx);
        assert_eq!(ws.next_model().model, "gpt-6-astra");
        assert!(ws.picker().is_none());
    });
}

#[gpui::test]
fn settings_changes_are_saved_and_defaults_follow(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open_with_models(cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.show_picker(PickerTarget::Default("coder".into()), cx);
        ws.pick_model("gpt-6-sol", cx);
        // The next run follows coder's new default until one is picked.
        assert_eq!(ws.next_model().model, "gpt-6-sol");
        ws.toggle_model_hidden("gpt-6-luna", cx);
        ws.step_ask_above(1, cx);
    });
    let saved: Vec<ModelSettings> = events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            WorkspaceEvent::SaveModelSettings(settings) => {
                Some(settings.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(saved.len(), 3);
    let last = saved.last().unwrap();
    assert_eq!(last.default_for("coder").model, "gpt-6-sol");
    assert!(last.is_hidden("gpt-6-luna"));
    assert_eq!(last.ask_above, Some(50.));
}

#[gpui::test]
fn a_fork_can_run_on_another_model(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open_with_models(cx);
    let run = demo::run_id();
    workspace.update(&mut cx, |ws, cx| {
        for (_, update) in demo::script() {
            ws.update_run(&run, update, cx);
        }
        ws.navigate(Route::Run(run.clone()), cx);
        // On an open run, the title bar's model is the run's, fixed.
        assert!(ws.shows_run_model());
        ws.start_fork_at(&run, 3, cx);
        ws.show_picker(PickerTarget::Fork, cx);
        ws.pick_model("gpt-6-sol", cx);
        ws.submit_prompt("same task, other model".into(), cx);
    });
    match events.borrow().last() {
        Some(WorkspaceEvent::Fork { turn, model, .. }) => {
            assert_eq!(*turn, Some(3));
            assert_eq!(model, &ModelChoice::new("gpt-6-sol", Effort::High));
        }
        other => panic!("expected a fork, got {other:?}"),
    }
}
