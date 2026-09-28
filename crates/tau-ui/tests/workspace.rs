//! The workspace's onboarding and pull request flows, driven the way a
//! host drives them, in GPUI's test app.

use gpui::{Entity, TestAppContext, VisualTestContext};
use tau_agent::event::StopReason;
use tau_ui::{
    Workspace,
    WorkspaceEvent,
    catalog::Catalog,
    demo,
    models::{Effort, ModelChoice, ModelSettings},
    pull_request::PrState,
    route::Route,
    setup::{GitHub, ModelAccess, Setup, SetupStep, SetupUpdate},
    view::{BranchCode, CodeState, Item, RunStatus},
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
    // The next message is an ordinary one again: it goes on with the
    // run.
    workspace.update(&mut cx, |ws, cx| {
        ws.submit_prompt("hello".into(), cx);
    });
    assert!(matches!(
        events.borrow().last(),
        Some(WorkspaceEvent::Resume { .. })
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
        // The fork keeps the conversation up to turn 2, then its own.
        let users: Vec<&str> = fork
            .items
            .iter()
            .filter_map(|item| match item {
                Item::User(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(users.len(), 2, "{users:?}");
        assert_eq!(users[1], "double the delay instead");
        assert!(matches!(fork.items.first(), Some(Item::User(_))));
        // Its turns count on from the fork point, without repeats.
        let turns: Vec<u32> = fork
            .items
            .iter()
            .filter_map(|item| match item {
                Item::TurnEnd { turn } => Some(*turn),
                _ => None,
            })
            .collect();
        assert_eq!(&turns[..3], [1, 2, 3], "{turns:?}");
        assert!(turns.windows(2).all(|pair| pair[0] < pair[1]), "{turns:?}");
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
    use tau_ui::{
        accounts::{Access, Credentials},
        host::{Host, HostConfig},
    };

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
        credentials: Credentials::new(tempfile::tempdir().unwrap().keep()),
        model: "gpt-5.5".into(),
        root: std::env::temp_dir(),
        store: std::env::temp_dir().join("unused.db"),
        repos: std::env::temp_dir().join("unused-repos"),
        settings: std::env::temp_dir().join("unused-models.json"),
        repo_list: std::env::temp_dir().join("unused-repos.json"),
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
            repo: String::new(),
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

/// The demo's workspace: three repositories, tau-agent open.
fn open_demo(
    cx: &mut TestAppContext,
) -> (
    Entity<Workspace>,
    VisualTestContext,
    std::rc::Rc<std::cell::RefCell<Vec<WorkspaceEvent>>>,
) {
    let (workspace, mut cx, events) = open(cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.add_history(demo::history(), cx);
        ws.set_catalog(demo::catalog(), cx);
    });
    events.borrow_mut().clear();
    (workspace, cx, events)
}

#[gpui::test]
fn the_tree_groups_runs_by_repository(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open_demo(cx);
    workspace.update(&mut cx, |ws, cx| {
        assert_eq!(ws.selected_repo(), Some("tau-agent"));
        let rows = ws.repo_rows("");
        let names: Vec<&str> =
            rows.iter().map(|rows| rows.repo.name.as_str()).collect();
        assert_eq!(names, ["tau-agent", "docbert", "homelab.nix"]);
        // tau-agent: 6 root runs, the fork under rotation-jitter.
        assert!(rows[0].open);
        assert_eq!(
            (rows[0].total, rows[0].runs.len(), rows[0].older),
            (6, 5, 1)
        );
        assert_eq!(rows[0].live, 1);
        assert!(!rows[1].open);
        assert_eq!((rows[1].total, rows[1].live), (3, 1));

        ws.toggle_repo_open("docbert", cx);
        assert!(ws.is_repo_open("docbert"));
        assert_eq!(ws.selected_repo(), Some("docbert"));
        ws.show_older_runs("tau-agent", cx);
        assert_eq!(ws.repo_rows("")[0].older, 0);
    });
    assert_eq!(
        events.borrow().last(),
        Some(&WorkspaceEvent::OpenRepos(vec![
            "tau-agent".into(),
            "docbert".into()
        ]))
    );
}

#[gpui::test]
fn a_filter_finds_repositories_and_runs(cx: &mut TestAppContext) {
    let (workspace, mut cx, _) = open_demo(cx);
    workspace.update(&mut cx, |ws, _| {
        let rows = ws.repo_rows("home");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].repo.name, "homelab.nix");
        // A run's name opens its repository on the runs that match.
        let rows = ws.repo_rows("rerank");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].repo.name, "docbert");
        assert!(rows[0].open);
        let titles: Vec<&str> =
            rows[0].runs.iter().map(|run| run.title.as_str()).collect();
        assert_eq!(titles, ["rerank-latency"]);
        assert!(ws.repo_rows("nothing like it").is_empty());
    });
}

#[gpui::test]
fn a_new_run_starts_in_the_chosen_repository(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open_demo(cx);
    workspace.update_in(&mut cx, |ws, window, cx| {
        ws.new_run_in("homelab.nix", window, cx);
        assert_eq!(ws.route(), &Route::NewRun);
        ws.submit_prompt("rotate the backups".into(), cx);
    });
    assert!(matches!(
        events.borrow().last(),
        Some(WorkspaceEvent::NewRun { repo, .. }) if repo == "homelab.nix"
    ));
    // Opening a run selects its repository, and opens it in the tree.
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(
            Route::Run(tau_agent::tool::RunId("pdf-ingest".into())),
            cx,
        );
        assert_eq!(ws.selected_repo(), Some("docbert"));
        assert!(ws.is_repo_open("docbert"));
    });
}

#[gpui::test]
fn memory_and_rules_belong_to_their_repository(cx: &mut TestAppContext) {
    let (workspace, mut cx, _) = open_demo(cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.open_memory("docbert", cx);
        assert_eq!(
            ws.route(),
            &Route::Memory {
                repo: "docbert".into(),
                note: None
            }
        );
        assert_eq!(ws.selected_repo(), Some("docbert"));
        assert_eq!(ws.repo_named("docbert").memory.notes.len(), 3);
        assert_eq!(ws.repo_named("tau-agent").memory.notes.len(), 8);
        ws.open_constitution("homelab.nix", cx);
        assert!(ws.repo_named("homelab.nix").constitution.rules.is_empty());
        assert!(ws.repo_named("not listed").memory.notes.is_empty());
    });
}

#[gpui::test]
fn repositories_are_added_and_removed(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open_demo(cx);
    workspace.update_in(&mut cx, |ws, window, cx| {
        ws.show_add_repo(window, cx);
        ws.navigate(
            Route::Memory {
                repo: "docbert".into(),
                note: None,
            },
            cx,
        );
    });
    workspace.update(&mut cx, |ws, cx| {
        ws.add_repo(tau_ui::catalog::Repo::new("dotfiles", "~/dotfiles"), cx);
        assert_eq!(ws.selected_repo(), Some("dotfiles"));
        assert!(ws.is_repo_open("dotfiles"));
        ws.remove_repo("docbert", cx);
        assert!(ws.catalog().repo("docbert").is_none());
        // Its screen closes with it.
        assert_eq!(ws.route(), &Route::Home);
        // Its runs stay, for History.
        assert!(ws.runs().iter().any(|run| run.repo == "docbert"));
        assert!(
            ws.repo_rows("")
                .iter()
                .all(|rows| rows.repo.name != "docbert")
        );
    });
    assert!(events.borrow().contains(&WorkspaceEvent::HideRepo {
        repo: "docbert".into()
    }));
}

#[gpui::test]
fn the_demo_adds_a_repository_by_path(cx: &mut TestAppContext) {
    let (workspace, mut cx, _) = open_demo(cx);
    cx.update(|_, cx| demo::respond(&workspace, cx));
    workspace.update_in(&mut cx, |ws, window, cx| {
        ws.show_add_repo(window, cx);
    });
    cx.simulate_input("~/Code/you/dotfiles");
    cx.simulate_keystrokes("enter");
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    workspace.read_with(&cx, |ws, _| {
        let repo = ws.catalog().repo("dotfiles").expect("added");
        assert_eq!(repo.path, "~/Code/you/dotfiles");
        assert_eq!(ws.selected_repo(), Some("dotfiles"));
    });
}

#[gpui::test]
fn a_kept_note_goes_to_its_runs_repository(cx: &mut TestAppContext) {
    let (workspace, mut cx, _) = open_demo(cx);
    workspace.update(&mut cx, |ws, cx| {
        let run = demo::run_id();
        for (_, update) in demo::script() {
            ws.update_run(&run, update, cx);
        }
        let (_, proposal) =
            ws.pending_proposals().next().expect("a suggestion");
        let title = proposal.title.clone();
        let before = ws.repo_named("tau-agent").memory.notes.len();
        ws.keep_note(&run, &title, cx);
        assert_eq!(ws.repo_named("tau-agent").memory.notes.len(), before + 1);
        assert_eq!(ws.repo_named("docbert").memory.notes.len(), 3);
    });
}

#[gpui::test]
fn connecting_from_the_models_screen_comes_back_to_it(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open_demo(cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Models, cx);
        ws.connect_model(cx);
        assert_eq!(ws.route(), &Route::Setup(SetupStep::Model));
        assert!(ws.can_go_back(), "the Models screen is a step back");
        ws.update_setup(
            SetupUpdate::Model(ModelAccess::Connected { label: "m".into() }),
            cx,
        );
        assert_eq!(ws.route(), &Route::Models);
        ws.sign_out(tau_ui::models::AccessKind::ApiKey, cx);
    });
    assert_eq!(
        events.borrow().last(),
        Some(&WorkspaceEvent::SignOut(tau_ui::models::AccessKind::ApiKey))
    );
}

#[gpui::test]
fn github_from_the_app_comes_back_when_done(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open_demo(cx);
    workspace.update(&mut cx, |ws, cx| {
        // Signing in from the Models screen goes back there.
        ws.navigate(Route::Models, cx);
        ws.connect_github(cx);
        assert_eq!(ws.route(), &Route::Setup(SetupStep::GitHub));
        ws.update_setup(
            SetupUpdate::GitHub(GitHub::SignedIn {
                user: "octocat".into(),
            }),
            cx,
        );
        assert_eq!(ws.route(), &Route::Models);
        ws.sign_out_github(cx);
    });
    assert_eq!(events.borrow().first(), Some(&WorkspaceEvent::GitHubSignIn));
    assert_eq!(events.borrow().last(), Some(&WorkspaceEvent::GitHubSignOut));

    // Adding from GitHub, signed in: pick, clone, and back to the run.
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Run(demo::run_id()), cx);
        ws.update_setup(
            SetupUpdate::Repos(vec![tau_ui::setup::RepoChoice {
                name: "octocat/hello".into(),
                description: String::new(),
                branch: "main".into(),
                selected: false,
            }]),
            cx,
        );
        ws.pick_github_repos(cx);
        assert_eq!(ws.route(), &Route::Setup(SetupStep::Repos));
        ws.toggle_repo("octocat/hello", cx);
        ws.clone_selected(cx);
        assert_eq!(ws.route(), &Route::Run(demo::run_id()));
    });
    assert_eq!(
        events.borrow().last(),
        Some(&WorkspaceEvent::CloneRepos {
            repos: vec!["octocat/hello".into()]
        })
    );
}

#[gpui::test]
fn github_sign_in_during_onboarding_moves_on_to_the_model(
    cx: &mut TestAppContext,
) {
    let (workspace, mut cx, _) = open(cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.start_setup(SetupStep::Welcome, cx);
        ws.sign_in_github(cx);
        ws.update_setup(
            SetupUpdate::GitHub(GitHub::SignedIn { user: "o".into() }),
            cx,
        );
        assert_eq!(ws.route(), &Route::Setup(SetupStep::Model));
    });
}

/// Plays the demo run to its end.
fn finish_demo_run(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) {
    workspace.update(cx, |ws, cx| {
        for (_, update) in demo::script() {
            ws.update_run(&demo::run_id(), update, cx);
        }
    });
}

#[gpui::test]
fn a_message_to_a_finished_run_goes_on_with_it(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open_demo(cx);
    finish_demo_run(&workspace, &mut cx);
    let run = demo::run_id();
    workspace.update(&mut cx, |ws, cx| {
        // An older run, opened from the list, goes on too.
        let older = tau_agent::tool::RunId("plugin-docs".into());
        ws.navigate(Route::Run(older.clone()), cx);
        ws.navigate(Route::Run(run.clone()), cx);
        assert!(
            ws.run(&run).unwrap().status
                == RunStatus::Finished(StopReason::Stop)
        );
        let runs_before = ws.runs().len();
        ws.submit_prompt("now add a test for it".into(), cx);
        // No new chat: the same run, back to work, at the top.
        assert_eq!(ws.runs().len(), runs_before);
        assert_eq!(ws.runs()[0].id, run);
        let view = ws.run(&run).unwrap();
        assert!(view.status.is_live());
        assert!(matches!(
            view.items.last(),
            Some(Item::User(text)) if text == "now add a test for it"
        ));
        assert_eq!(ws.route(), &Route::Run(run.clone()));
    });
    assert_eq!(
        events.borrow().last(),
        Some(&WorkspaceEvent::Resume {
            run: run.clone(),
            prompt: "now add a test for it".into(),
            model: ModelChoice::new("gpt-5.5", Effort::High),
        })
    );
    // If the host cannot, the run ends as it had.
    workspace.update(&mut cx, |ws, cx| {
        ws.resume_failed(&run, cx);
        let view = ws.run(&run).unwrap();
        assert_eq!(view.status, RunStatus::Finished(StopReason::Stop));
        assert!(!matches!(view.items.last(), Some(Item::User(_))));
    });
    // A new run is still a new run.
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::NewRun, cx);
        ws.submit_prompt("something else".into(), cx);
    });
    assert!(matches!(
        events.borrow().last(),
        Some(WorkspaceEvent::NewRun { .. })
    ));
}

#[gpui::test]
fn the_demo_goes_on_with_a_finished_run(cx: &mut TestAppContext) {
    let (workspace, mut cx, _) = open_demo(cx);
    finish_demo_run(&workspace, &mut cx);
    cx.update(|_, cx| demo::respond(&workspace, cx));
    let run = demo::run_id();
    let (turn, cost) = workspace.read_with(&cx, |ws, _| {
        let view = ws.run(&run).unwrap();
        (view.turn, view.usage.cost)
    });
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Run(run.clone()), cx);
        ws.submit_prompt("and then?".into(), cx);
    });
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(5));
    cx.run_until_parked();
    workspace.read_with(&cx, |ws, _| {
        let view = ws.run(&run).unwrap();
        assert_eq!(view.status, RunStatus::Finished(StopReason::Stop));
        // Turns keep counting, and the cost adds up.
        assert_eq!(view.turn, turn + 1);
        assert!((view.usage.cost - (cost + 0.012)).abs() < 1e-9);
        assert!(view.last_text().unwrap().contains("and then?"));
    });
}

#[gpui::test]
fn the_repository_menu_fetches_new_commits(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open_demo(cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.toggle_repo_menu("docbert", cx);
        ws.update_repo("docbert", cx);
    });
    assert_eq!(
        events.borrow().last(),
        Some(&WorkspaceEvent::UpdateRepo {
            repo: "docbert".into()
        })
    );
}

#[gpui::test]
fn replies_are_unread_until_their_conversation_is_open(
    cx: &mut TestAppContext,
) {
    let (workspace, mut cx, _) = open_demo(cx);
    let run = demo::run_id();
    let other = tau_agent::tool::RunId("plugin-docs".into());
    workspace.update(&mut cx, |ws, cx| {
        // Looking at another conversation while this one works.
        ws.navigate(Route::Run(other.clone()), cx);
        for (_, update) in demo::script() {
            ws.update_run(&run, update, cx);
        }
        let view = ws.run(&run).unwrap();
        assert!(ws.unread(view) > 0, "its replies are unread");
        assert_eq!(ws.unread(ws.run(&other).unwrap()), 0);
        ws.navigate(Route::Run(run.clone()), cx);
        assert_eq!(ws.unread(ws.run(&run).unwrap()), 0, "opening reads them");
    });
}

#[gpui::test]
fn closing_a_conversation_takes_it_off_the_sidebar(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open_demo(cx);
    let live = tau_agent::tool::RunId("pdf-ingest".into());
    let done = tau_agent::tool::RunId("plugin-docs".into());
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Run(done.clone()), cx);
        ws.close_run(&done, cx);
        assert!(ws.is_closed(&done));
        // The screen moves to another open conversation.
        assert_ne!(ws.route(), &Route::Run(done.clone()));
        let listed = |ws: &Workspace, id: &tau_agent::tool::RunId| {
            ws.repo_rows("")
                .iter()
                .any(|rows| rows.runs.iter().any(|run| &run.id == id))
        };
        assert!(!listed(ws, &done));
        // History still has it.
        assert!(ws.runs().iter().any(|run| run.id == done));
        ws.close_run(&live, cx);
    });
    let events = events.borrow();
    assert!(events.contains(&WorkspaceEvent::CloseRun { run: done.clone() }));
    // Closing one that works stops it.
    assert!(events.contains(&WorkspaceEvent::Cancel { run: live.clone() }));
    drop(events);
    // A message to it opens it again.
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Run(done.clone()), cx);
        ws.submit_prompt("one more thing".into(), cx);
        assert!(!ws.is_closed(&done));
    });
}

#[gpui::test]
fn a_phone_closes_the_conversation_it_shows(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open_demo(cx);
    let done = tau_agent::tool::RunId("plugin-docs".into());
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Run(done.clone()), cx);
        ws.close_run_to_list(&done, cx);
        assert!(ws.is_closed(&done));
        // Back to the list, with nothing to go back to.
        assert_eq!(ws.route(), &Route::Home);
        assert!(!ws.can_go_back());
    });
    assert!(
        events
            .borrow()
            .contains(&WorkspaceEvent::CloseRun { run: done.clone() })
    );
}

#[gpui::test]
fn rules_are_written_edited_and_removed_in_the_editor(cx: &mut TestAppContext) {
    use tau_ui::rule_editor::{Mark, Preset};
    let (workspace, mut cx, events) = open_demo(cx);
    cx.update(|_, cx| demo::respond(&workspace, cx));
    workspace.update(&mut cx, |ws, cx| {
        ws.open_constitution("docbert", cx);
        ws.open_rule_editor("docbert", None, cx);
        ws.rule_text_for_test("Never delete an index.", cx);
        // Nothing picked: nothing is sent, and the editor says why, in
        // place of a dialog.
        ws.save_rule(cx);
        assert!(ws.alert().is_none());
        assert_eq!(
            ws.draft_problem(cx),
            Some("Pick at least one place, or Jev has nothing to check.")
        );
        assert!(ws.rule_draft().unwrap().tried_to_save);
        // Places are picked, and any tool's field can be added by name.
        ws.toggle_place("bash.command", cx);
        ws.rule_on_for_test("not a field", cx);
        assert!(!ws.add_other_place(cx));
        ws.rule_on_for_test("grep.pattern", cx);
        assert!(ws.add_other_place(cx));
        // Presets, then a step: review never passes block.
        ws.set_preset(Preset::Strict, cx);
        assert_eq!(ws.rule_draft().unwrap().preset(), Some(Preset::Strict));
        ws.nudge(Mark::Block, 0.05, cx);
        assert_eq!(ws.rule_draft().unwrap().preset(), None);
        for _ in 0..20 {
            ws.nudge(Mark::Review, 0.05, cx);
        }
        let draft = ws.rule_draft().unwrap();
        assert_eq!((draft.review, draft.block), (0.65, 0.65));
        ws.nudge(Mark::Review, -0.35, cx);
        ws.save_rule(cx);
        assert!(ws.rule_draft().is_none());
    });
    assert_eq!(
        events.borrow().last(),
        Some(&WorkspaceEvent::AddRule {
            repo: "docbert".into(),
            text: "Never delete an index.".into(),
            on: vec!["bash.command".into(), "grep.pattern".into()],
            review: 0.3,
            block: 0.65,
        })
    );
    cx.run_until_parked();
    workspace.update(&mut cx, |ws, cx| {
        let rules = ws.repo_named("docbert").constitution.rules.clone();
        assert_eq!(rules.len(), 4);
        // Editing opens with the rule as it is, and saves over it.
        let id = rules.last().unwrap().id.clone();
        ws.open_rule_editor("docbert", Some(&id), cx);
        let draft = ws.rule_draft().unwrap();
        assert_eq!(draft.editing.as_deref(), Some(id.as_str()));
        assert_eq!(draft.places, ["bash.command", "grep.pattern"]);
        ws.toggle_place("grep.pattern", cx);
        ws.save_rule(cx);
    });
    cx.run_until_parked();
    workspace.update(&mut cx, |ws, cx| {
        let rules = ws.repo_named("docbert").constitution.rules.clone();
        let edited = rules.last().unwrap();
        assert_eq!(edited.applies_to, ["bash.command"]);
        assert_eq!(rules.len(), 4, "saved over, not added");
        let id = edited.id.clone();
        ws.remove_rule("docbert", &id, cx);
    });
    cx.run_until_parked();
    workspace.read_with(&cx, |ws, _| {
        assert_eq!(ws.repo_named("docbert").constitution.rules.len(), 3);
    });
}

#[gpui::test]
fn a_rule_is_tried_on_the_repositorys_latest_calls(cx: &mut TestAppContext) {
    use tau_ui::rule_editor::Trying;
    let (workspace, mut cx, events) = open_demo(cx);
    cx.update(|_, cx| demo::respond(&workspace, cx));
    workspace.update(&mut cx, |ws, cx| {
        for (_, update) in demo::script() {
            ws.update_run(&demo::run_id(), update, cx);
        }
        ws.open_rule_editor("tau-agent", None, cx);
        ws.rule_text_for_test("Never touch the production database.", cx);
        ws.toggle_place("bash.command", cx);
        ws.try_rule(cx);
        assert_eq!(ws.rule_draft().unwrap().trying, Trying::Asking);
    });
    {
        let events = events.borrow();
        let Some(WorkspaceEvent::TryRule { calls, answers, .. }) =
            events.last()
        else {
            panic!("no TryRule: {events:?}");
        };
        assert!(!calls.is_empty() && calls.len() <= 6);
        assert!(calls.iter().all(|(tool, _)| tool == "bash"), "{calls:?}");
        assert!(answers.is_empty(), "the rule reads no answers");
    }
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(2));
    cx.run_until_parked();
    workspace.read_with(&cx, |ws, _| {
        let Trying::Done { trials, .. } = &ws.rule_draft().unwrap().trying
        else {
            panic!("not tried");
        };
        assert!(!trials.is_empty());
    });
}

#[gpui::test]
fn the_constitution_counts_its_runs_and_lists_what_waits(
    cx: &mut TestAppContext,
) {
    let (workspace, mut cx, _) = open_demo(cx);
    workspace.update(&mut cx, |ws, cx| {
        for (_, update) in demo::script() {
            ws.update_run(&demo::run_id(), update, cx);
        }
        let stats = ws.rules_stats("tau-agent");
        assert!(stats.checked > 0);
        assert_eq!((stats.blocked, stats.flagged, stats.waiting), (1, 1, 1));
        assert_eq!(stats.per_rule.get("R2"), Some(&(1, 0, 0)));
        let waiting = ws.review_items("tau-agent");
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].rule, "R1");
        assert_eq!(waiting[0].tool.as_deref(), Some("bash"));
        // Looks fine: off the queue, and onto what was handled.
        let (run, key) = (waiting[0].run.clone(), waiting[0].key.clone());
        ws.mark_reviewed(&run, &key, cx);
        assert!(ws.review_items("tau-agent").is_empty());
        let fine = ws
            .handled("tau-agent")
            .into_iter()
            .filter(|done| {
                done.what == tau_ui::rule_editor::HandledKind::LookedFine
            })
            .count();
        assert_eq!(fine, 1);
    });
}

#[gpui::test]
fn the_typesafe_key_is_asked_for_and_forgotten(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open_demo(cx);
    workspace.update_in(&mut cx, |ws, window, cx| {
        ws.ask_for_jev_key(window, cx);
    });
    cx.simulate_input("ts-secret");
    cx.simulate_keystrokes("enter");
    assert_eq!(
        events.borrow().last(),
        Some(&WorkspaceEvent::JevKey {
            key: Some("ts-secret".into())
        })
    );
    workspace.update(&mut cx, |ws, cx| ws.forget_jev_key(cx));
    assert_eq!(
        events.borrow().last(),
        Some(&WorkspaceEvent::JevKey { key: None })
    );
}

#[gpui::test]
fn history_runs_the_query_typed_and_shows_its_rows(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open_demo(cx);
    cx.update(|_, cx| demo::respond(&workspace, cx));
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::History, cx);
        ws.run_query(cx);
    });
    // The box starts with the store's sample query.
    assert_eq!(
        events.borrow().last(),
        Some(&WorkspaceEvent::Query {
            sql: demo::catalog().store.sample_query
        })
    );
    cx.run_until_parked();
    workspace.read_with(&cx, |ws, _| {
        let Some(Ok(table)) = ws.query_result() else {
            panic!("a table")
        };
        assert_eq!(table.columns, ["agent", "sum(cost_usd)"]);
        assert_eq!(table.rows.len(), 2);
    });
    workspace.update(&mut cx, |ws, cx| {
        ws.set_query_result(Err("no such table: nope".into()), cx);
        assert!(matches!(ws.query_result(), Some(Err(_))));
    });
}

#[gpui::test]
fn a_reviewed_call_leaves_the_queue_for_good(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open_demo(cx);
    let run = demo::run_id();
    workspace.update(&mut cx, |ws, cx| {
        ws.mark_reviewed(&run, "c6", cx);
    });
    assert_eq!(
        events.borrow().last(),
        Some(&WorkspaceEvent::Reviewed {
            run: run.clone(),
            call_id: "c6".into()
        })
    );
}

#[gpui::test]
fn search_finds_runs_repositories_and_actions(cx: &mut TestAppContext) {
    let (workspace, mut cx, _) = open_demo(cx);
    workspace.read_with(&cx, |ws, _| {
        use tau_ui::search::Pick;
        let hits = ws.search_hits("rerank");
        assert_eq!(hits[0].label, "rerank-latency");
        assert_eq!(hits[0].detail, "run in docbert");
        // Every word must match.
        assert!(ws.search_hits("rerank homelab").is_empty());
        let hits = ws.search_hits("homelab");
        assert!(
            hits.iter()
                .any(|hit| hit.pick == Pick::Repo("homelab.nix".into()))
        );
        assert!(
            hits.iter()
                .any(|hit| hit.pick == Pick::NewRunIn("homelab.nix".into()))
        );
        let hits = ws.search_hits("history");
        assert_eq!(hits[0].pick, Pick::Screen(Route::History));
        // Nothing typed: repositories and things to do.
        assert!(!ws.search_hits("").is_empty());
    });
    // Ctrl K, a query, Enter: the conversation opens.
    cx.simulate_keystrokes("ctrl-k");
    workspace.read_with(&cx, |ws, _| assert!(ws.is_searching()));
    cx.simulate_input("backup");
    cx.simulate_keystrokes("enter");
    workspace.read_with(&cx, |ws, _| {
        assert!(!ws.is_searching());
        assert_eq!(
            ws.route(),
            &Route::Run(tau_agent::tool::RunId("backup-timer".into()))
        );
        assert_eq!(ws.selected_repo(), Some("homelab.nix"));
    });
}

#[gpui::test]
fn attached_files_go_with_the_next_message(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let notes = dir.path().join("notes.md");
    std::fs::write(&notes, "the retry budget is 3\n").unwrap();
    let binary = dir.path().join("blob.bin");
    std::fs::write(&binary, [0xff, 0xfe, 0x00]).unwrap();
    let (workspace, mut cx, events) = open(cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::NewRun, cx);
        ws.attach_path(&notes, cx);
        assert_eq!(ws.attachments().len(), 1);
        // A file that is not text is refused, saying why.
        ws.attach_path(&binary, cx);
        assert_eq!(ws.attachments().len(), 1);
        assert!(ws.alert().unwrap().1.contains("not text"));
        ws.submit_prompt("use the notes".into(), cx);
        assert!(ws.attachments().is_empty(), "they went with the message");
    });
    let Some(WorkspaceEvent::NewRun { prompt, .. }) =
        events.borrow().last().cloned()
    else {
        panic!("a new run")
    };
    assert!(prompt.starts_with("use the notes"));
    assert!(
        prompt.contains(
            "<attached file=\"notes.md\">\nthe retry budget is 3\n</attached>"
        ),
        "{prompt}"
    );
}

fn composer_slash(
    workspace: &Entity<Workspace>,
    cx: &mut VisualTestContext,
    text: &str,
) -> tau_ui::slash::Slash {
    workspace.update(cx, |ws, cx| {
        ws.set_composer(text, cx);
        ws.composer_slash(cx)
    })
}

fn names(slash: tau_ui::slash::Slash) -> Vec<&'static str> {
    match slash {
        tau_ui::slash::Slash::Menu(commands) => {
            commands.iter().map(|command| command.name).collect()
        }
        _ => Vec::new(),
    }
}

#[gpui::test]
fn a_slash_lists_the_commands_that_work_here(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open_demo(cx);
    let done = tau_agent::tool::RunId("plugin-docs".into());
    workspace.update(&mut cx, |ws, cx| ws.navigate(Route::NewRun, cx));
    // A new run has no conversation to fork, close or open a PR from.
    assert_eq!(
        names(composer_slash(&workspace, &mut cx, "/")),
        ["/goal", "/model", "/attach"]
    );
    workspace
        .update(&mut cx, |ws, cx| ws.navigate(Route::Run(done.clone()), cx));
    assert_eq!(names(composer_slash(&workspace, &mut cx, "/")).len(), 6);
    assert_eq!(names(composer_slash(&workspace, &mut cx, "/fo")), ["/fork"]);
    // Not a command: a message.
    assert_eq!(
        composer_slash(&workspace, &mut cx, "/usr/bin is slow"),
        tau_ui::slash::Slash::None
    );
    assert_eq!(
        composer_slash(&workspace, &mut cx, "/nope"),
        tau_ui::slash::Slash::None
    );
    workspace.update(&mut cx, |ws, cx| {
        // ↑↓ wrap around; Tab completes the one selected.
        ws.set_composer("/", cx);
        assert!(ws.slash_move(-1, cx));
        assert!(ws.slash_complete(cx));
        assert_eq!(ws.composer_text(cx), "/close");
        // Esc closes the menu until the text changes.
        ws.set_composer("/", cx);
        assert!(ws.slash_dismiss(cx));
        assert_eq!(ws.composer_slash(cx), tau_ui::slash::Slash::None);
        ws.set_composer("/g", cx);
        assert_ne!(ws.composer_slash(cx), tau_ui::slash::Slash::None);
        // Enter on a command being typed runs it.
        ws.submit_prompt("/cl".into(), cx);
        assert!(ws.is_closed(&done));
        // Enter on /go puts /goal in the composer, to write the goal.
        ws.navigate(Route::Run(done.clone()), cx);
        ws.submit_prompt("/go".into(), cx);
        assert_eq!(ws.composer_text(cx), "/goal ");
    });
    let events = events.borrow();
    assert!(events.contains(&WorkspaceEvent::CloseRun { run: done }));
    assert!(
        !events.iter().any(|event| matches!(
            event,
            WorkspaceEvent::Resume { .. } | WorkspaceEvent::NewRun { .. }
        )),
        "no command was sent as a message: {events:?}"
    );
}

#[gpui::test]
fn slash_goal_sets_the_conversations_goal(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open_demo(cx);
    let done = tau_agent::tool::RunId("plugin-docs".into());
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Run(done.clone()), cx);
        // /goal alone is not a goal yet: it stays, to be written.
        ws.set_composer("/goal ", cx);
        ws.submit_prompt("/goal".into(), cx);
        assert_eq!(ws.composer_text(cx), "/goal ");
        ws.set_composer("", cx);
        ws.submit_prompt(
            "/goal Every lane has an owner in lanes.toml".into(),
            cx,
        );
    });
    workspace.update(&mut cx, |ws, cx| {
        let view = ws.run(&done).unwrap();
        assert!(matches!(view.items.last(),
            Some(Item::Goal(c)) if c == "Every lane has an owner in lanes.toml"));
        // Going on now: a new goal is stored for its next stop, and the
        // model is told. Typed limits win over the popover's.
        ws.set_composer("", cx);
        ws.submit_prompt("/goal --continuations 3 it ships".into(), cx);
    });
    {
        let events = events.borrow();
        let prompts: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                WorkspaceEvent::Resume { prompt, .. } => Some(prompt.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            prompts,
            ["/goal --continuations 10 --budget 2.00 Every lane has an \
              owner in lanes.toml"]
        );
        assert!(events.contains(&WorkspaceEvent::Goal {
            run: done.clone(),
            record: tau_goal::Record::Set {
                goal: "it ships".into(),
                continuations: 3,
                budget: 2.0,
            },
        }));
        assert!(events.contains(&WorkspaceEvent::Steer {
            run: done.clone(),
            text: tau_goal::set_input("it ships"),
        }));
    }
    events.borrow_mut().clear();

    // Without a TypeSafe key, the goal is not sent: nothing would check it.
    workspace.update(&mut cx, |ws, cx| {
        let mut catalog = demo::catalog();
        catalog.models.access.jev = false;
        ws.set_catalog(catalog, cx);
        ws.navigate(Route::NewRun, cx);
        ws.submit_prompt("/goal it ships".into(), cx);
        assert!(ws.alert().is_some());
        assert_eq!(ws.composer_text(cx), "/goal it ships");
    });
    assert!(events.borrow().is_empty());
}

#[gpui::test]
fn goal_buttons_store_changes_and_keep_going(cx: &mut TestAppContext) {
    use tau_goal::{Record, Status};
    let (workspace, mut cx, events) = open_demo(cx);
    let stopped = tau_agent::tool::RunId("lane-audit".into());
    let met = tau_agent::tool::RunId("mutants-triage".into());
    workspace.update(&mut cx, |ws, cx| {
        use tau_ui::ui::inspector::Tab;
        assert!(Tab::of(ws.run(&stopped)).contains(&Tab::Goal));
        let done = tau_agent::tool::RunId("plugin-docs".into());
        assert!(!Tab::of(ws.run(&done)).contains(&Tab::Goal));

        // Keep going: more continuations, and the conversation goes on.
        ws.navigate(Route::Run(stopped.clone()), cx);
        ws.keep_going(&stopped, cx);
        let goal = ws.run(&stopped).unwrap().goal.clone().unwrap();
        assert_eq!(goal.status, Status::Active);
        assert_eq!(goal.max_continuations, 12);
        // Edit puts the goal back in the composer.
        ws.edit_goal(&stopped, cx);
        assert_eq!(
            ws.composer_text(cx),
            "/goal Every lane has an owner in lanes.toml"
        );
        ws.pause_goal(&stopped, cx);
        assert_eq!(
            ws.run(&stopped).unwrap().goal.as_ref().unwrap().status,
            Status::Paused
        );
        ws.clear_goal(&met, cx);
        assert!(ws.run(&met).unwrap().goal.is_none());
    });
    let events = events.borrow();
    let records: Vec<(&str, &Record)> = events
        .iter()
        .filter_map(|event| match event {
            WorkspaceEvent::Goal { run, record } => Some((&*run.0, record)),
            _ => None,
        })
        .collect();
    assert_eq!(
        records,
        [
            ("lane-audit", &Record::Extended { by: 10 }),
            ("lane-audit", &Record::Paused),
            ("mutants-triage", &Record::Cleared),
        ]
    );
    assert!(events.iter().any(|event| matches!(event,
        WorkspaceEvent::Resume { run, prompt, .. }
            if run == &stopped && prompt == tau_ui::goal::KEEP_GOING)));
}
