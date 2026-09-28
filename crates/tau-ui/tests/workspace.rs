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
fn rules_are_added_and_removed_from_the_constitution_screen(
    cx: &mut TestAppContext,
) {
    let (workspace, mut cx, events) = open_demo(cx);
    cx.update(|_, cx| demo::respond(&workspace, cx));
    workspace.update(&mut cx, |ws, cx| {
        ws.open_constitution("docbert", cx);
        // Without words or a place, nothing is sent.
        ws.add_rule("docbert", cx);
        assert!(ws.alert().is_some());
    });
    workspace.update(&mut cx, |ws, cx| {
        ws.escape(cx);
        ws.rule_text_for_test("Never delete an index.", cx);
        ws.rule_on_for_test("bash.command, final answer", cx);
        ws.add_rule("docbert", cx);
    });
    assert_eq!(
        events.borrow().last(),
        Some(&WorkspaceEvent::AddRule {
            repo: "docbert".into(),
            text: "Never delete an index.".into(),
            on: vec!["bash.command".into(), "final answer".into()],
        })
    );
    cx.run_until_parked();
    workspace.update(&mut cx, |ws, cx| {
        let rules = &ws.repo_named("docbert").constitution.rules;
        assert_eq!(rules.len(), 4);
        let id = rules.last().unwrap().id.clone();
        ws.remove_rule("docbert", &id, cx);
    });
    cx.run_until_parked();
    workspace.read_with(&cx, |ws, _| {
        assert_eq!(ws.repo_named("docbert").constitution.rules.len(), 3);
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
