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
    workspace::{LandingState, PickerTarget},
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
        ws.sign_in_chatgpt(None, false, cx);
        ws.update_setup(
            SetupUpdate::Model(ModelAccess::Connected { label: "m".into() }),
            cx,
        );
        // Signed in: the step stays, to show the account and pick the
        // default model, until "Continue to repositories".
        assert_eq!(ws.route(), &Route::Setup(SetupStep::Model));
        ws.continue_from_model(cx);
        assert_eq!(ws.route(), &Route::Setup(SetupStep::Repos));
        ws.clone_selected(cx);
        assert_eq!(ws.route(), &Route::Setup(SetupStep::Ready));
    });
    let events = events.borrow();
    assert_eq!(
        events[1],
        WorkspaceEvent::ChatGptSignIn {
            account: None,
            consent: false,
        }
    );
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
        assert_eq!(ws.route(), &Route::Setup(SetupStep::Model));
        ws.continue_from_model(cx);
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
            model: ModelChoice::new("gpt-5.5", Effort::Auto),
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
            model: ModelChoice::new("gpt-5.5", Effort::Auto),
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
        assert_eq!(fork.title, "double the delay instead");
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
    // A written title replaces the placeholder, in the list and under
    // the parent.
    let fork =
        workspace.read_with(&cx, |ws, _| ws.current().unwrap().id.clone());
    workspace.update(&mut cx, |ws, cx| {
        ws.apply(
            tau_ui::update::HostUpdate::Titled {
                run: fork.clone(),
                title: "Double the retry delay".into(),
            },
            cx,
        )
    });
    workspace.read_with(&cx, |ws, _| {
        assert_eq!(ws.run(&fork).unwrap().title, "Double the retry delay");
        let parent = ws.run(&run).unwrap();
        let child = parent.children.iter().find(|child| child.id == fork);
        assert_eq!(child.unwrap().title, "Double the retry delay");
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
        accounts::Credentials,
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
        account: tau_ai::chatgpt::AccountId::parse("test-account").unwrap(),
        credentials: Credentials::new(tempfile::tempdir().unwrap().keep()),
        model: Some("gpt-5.5".into()),
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

/// The picker offers the plan's models from the model table, one per
/// family, and picks one at once.
#[gpui::test]
fn the_plan_picker_offers_the_plan_models(cx: &mut TestAppContext) {
    let (workspace, mut cx, _) = open(cx);
    workspace.update(&mut cx, |ws, cx| {
        let mut catalog = ws.catalog().clone();
        catalog.models = demo::models();
        ws.set_catalog(catalog, cx);
        ws.navigate(Route::NewRun, cx);
        ws.show_picker(PickerTarget::Next, cx);
        let shown: Vec<String> = ws
            .catalog()
            .models
            .shown("")
            .iter()
            .map(|option| option.label().to_owned())
            .collect();
        assert_eq!(
            shown,
            ["GPT-6.1 Sol", "GPT-6 Luna", "GPT-6 Astra", "GPT-5.6 Terra"],
            "the table's order and names"
        );
        ws.pick_model("gpt-6-astra", cx);
        assert!(ws.alert().is_none(), "nothing to confirm");
        assert_eq!(ws.next_model().model, "gpt-6-astra");
    });
    // Drawn, with the plan's line under the composer.
    cx.run_until_parked();
}

#[gpui::test]
fn the_next_run_starts_on_the_picked_model(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open_with_models(cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::NewRun, cx);
        assert_eq!(
            ws.next_model(),
            &ModelChoice::new("gpt-6.1-sol", Effort::Auto)
        );
        ws.show_picker(PickerTarget::Next, cx);
        ws.pick_effort(Effort::Low, cx);
        ws.pick_model("gpt-6-luna", cx);
        assert!(ws.picker().is_none(), "picking a model closes the picker");
        ws.submit_prompt("fix it".into(), cx);
    });
    assert_eq!(
        events.borrow().last(),
        Some(&WorkspaceEvent::NewRun {
            prompt: "fix it".into(),
            model: ModelChoice::new("gpt-6-luna", Effort::Low),
            repo: String::new(),
        })
    );
}

#[gpui::test]
fn settings_changes_are_saved_and_defaults_follow(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open_with_models(cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.show_picker(PickerTarget::Default("coder".into()), cx);
        ws.pick_model("gpt-6-luna", cx);
        // The next run follows coder's new default until one is picked.
        assert_eq!(ws.next_model().model, "gpt-6-luna");
        ws.toggle_model_hidden("gpt-6-astra", cx);
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
    assert_eq!(saved.len(), 2);
    let last = saved.last().unwrap();
    assert_eq!(last.default_for("coder").model, "gpt-6-luna");
    assert!(last.is_hidden("gpt-6-astra"));
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
        // On an open run, the composer's model is the chat's.
        assert_eq!(ws.composer_target(), Some(PickerTarget::Run(run.clone())));
        ws.start_fork_at(&run, 3, cx);
        ws.show_picker(PickerTarget::Fork, cx);
        ws.pick_model("gpt-6-luna", cx);
        ws.submit_prompt("same task, other model".into(), cx);
    });
    match events.borrow().last() {
        Some(WorkspaceEvent::Fork { turn, model, .. }) => {
            assert_eq!(*turn, Some(3));
            // tau-reasoning picked high for the run; the fork is scored
            // again.
            assert_eq!(model, &ModelChoice::new("gpt-6-luna", Effort::Auto));
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
        ws.sign_out(cx);
    });
    assert_eq!(events.borrow().last(), Some(&WorkspaceEvent::SignOut));
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

/// Signed in, the model step picks the default model the way the
/// picker does, and says which one runs start with.
#[gpui::test]
fn the_signed_in_model_step_picks_the_default_model(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open_demo(cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.set_setup(demo::setup(SetupStep::Model), cx);
        ws.start_setup(SetupStep::Model, cx);
        ws.update_setup(
            SetupUpdate::Model(ModelAccess::Connected {
                label: "gpt-6.1-sol · ChatGPT plan".into(),
            }),
            cx,
        );
    });
    // The step, with the account's models as pills.
    cx.run_until_parked();
    workspace.update(&mut cx, |ws, cx| {
        ws.pick_setup_model("gpt-6-astra", cx);
        // One the account does not list changes nothing.
        ws.pick_setup_model("no-such-model", cx);
        let settings = &ws.catalog().models.settings;
        assert_eq!(settings.default_for("coder").model, "gpt-6-astra");
        assert_eq!(
            ws.setup().model_label(),
            Some("gpt-6-astra · ChatGPT plan")
        );
    });
    let saved: Vec<_> = events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            WorkspaceEvent::SaveModelSettings(settings) => {
                Some(settings.default_for("coder").model)
            }
            _ => None,
        })
        .collect();
    assert_eq!(saved, ["gpt-6-astra"]);
}

/// An account that cannot share its plan stays on the model step, with
/// the refusal, and can sign in with another account.
#[gpui::test]
fn a_not_eligible_account_offers_another(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open_demo(cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.set_setup(demo::setup(SetupStep::Model), cx);
        ws.start_setup(SetupStep::Model, cx);
        ws.update_setup(
            SetupUpdate::Model(ModelAccess::NotEligible {
                account: "you@example.com".into(),
                detail: "403 subscription_sharing_user_not_eligible".into(),
            }),
            cx,
        );
        assert_eq!(ws.route(), &Route::Setup(SetupStep::Model));
    });
    cx.run_until_parked();
    workspace.update(&mut cx, |ws, cx| ws.sign_in_chatgpt(None, false, cx));
    assert_eq!(
        events.borrow().last(),
        Some(&WorkspaceEvent::ChatGptSignIn {
            account: None,
            consent: false,
        })
    );
}

/// Cancelling the sign-in waiting for the browser goes back to the start
/// of the model step and tells the host to stop listening.
#[gpui::test]
fn a_waiting_chatgpt_sign_in_can_be_cancelled(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open_demo(cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.set_setup(demo::setup(SetupStep::Model), cx);
        ws.start_setup(SetupStep::Model, cx);
        ws.sign_in_chatgpt(None, false, cx);
        ws.update_setup(
            SetupUpdate::Model(ModelAccess::SigningIn {
                url: Some(demo::DEMO_AUTHORIZE_URL.into()),
            }),
            cx,
        );
    });
    cx.run_until_parked();
    workspace.update(&mut cx, |ws, cx| {
        ws.cancel_chatgpt_sign_in(cx);
        assert_eq!(ws.setup().model, ModelAccess::None);
    });
    assert_eq!(events.borrow().last(), Some(&WorkspaceEvent::ChatGptCancel));
}

/// A sign-in landing moves the handshake once: the change animates, and
/// redrawing it starts nothing new.
#[gpui::test]
fn a_landing_sign_in_animates_the_handshake_once(cx: &mut TestAppContext) {
    use tau_ui::motion::{Link, Mood};
    let (workspace, mut cx, _) = open_demo(cx);
    workspace.update(&mut cx, |ws, cx| {
        let mut setup = demo::setup(SetupStep::Model);
        setup.model = ModelAccess::SigningIn { url: None };
        ws.set_setup(setup, cx);
        ws.start_setup(SetupStep::Model, cx);
    });
    cx.run_until_parked();
    let waited = workspace.read_with(&cx, |ws, _| {
        let motion = ws.setup_motion();
        assert_eq!(motion.link.current, Link::Waiting);
        motion.link.epoch
    });
    workspace.update(&mut cx, |ws, cx| {
        ws.update_setup(
            SetupUpdate::Model(ModelAccess::Connected { label: "m".into() }),
            cx,
        )
    });
    cx.run_until_parked();
    let landed = workspace.read_with(&cx, |ws, _| {
        let motion = ws.setup_motion();
        assert_eq!(motion.link.current, Link::Connected);
        assert_eq!(motion.link.previous, Link::Waiting);
        assert_eq!(motion.mood.current, Mood::Done);
        assert_eq!(motion.link.epoch, waited + 1);
        motion.link.epoch
    });
    workspace.update(&mut cx, |_, cx| cx.notify());
    cx.run_until_parked();
    workspace.read_with(&cx, |ws, _| {
        assert_eq!(
            ws.setup_motion().link.epoch,
            landed,
            "a redraw is no change"
        )
    });
}

#[gpui::test]
fn reduced_motion_stops_onboardings_loops(cx: &mut TestAppContext) {
    let (workspace, mut cx, _) = open_demo(cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.set_setup(demo::setup(SetupStep::Model), cx);
        ws.start_setup(SetupStep::Model, cx);
        ws.set_reduce_motion(true, cx);
    });
    cx.run_until_parked();
    workspace.read_with(&cx, |ws, _| {
        assert!(ws.setup_motion().reduce && !ws.setup_motion().loops())
    });
}

/// Every onboarding screen draws, on a desktop and on a phone.
#[gpui::test]
fn every_onboarding_screen_draws(cx: &mut TestAppContext) {
    let (workspace, mut cx, _) = open_demo(cx);
    let models = [
        ModelAccess::None,
        ModelAccess::SigningIn { url: None },
        ModelAccess::SigningIn {
            url: Some(demo::DEMO_AUTHORIZE_URL.into()),
        },
        ModelAccess::Connected { label: "m".into() },
        ModelAccess::PlanDisabled {
            account: "you@example.com".into(),
        },
        ModelAccess::NotEligible {
            account: String::new(),
            detail: "403".into(),
        },
        ModelAccess::Failed("no".into()),
    ];
    for phone in [false, true] {
        workspace.update(&mut cx, |ws, cx| {
            ws.set_frame(phone.then_some((390., 844.)), cx)
        });
        for step in [
            SetupStep::Welcome,
            SetupStep::GitHub,
            SetupStep::Token,
            SetupStep::Repos,
            SetupStep::Ready,
        ] {
            workspace.update(&mut cx, |ws, cx| {
                ws.set_setup(demo::setup(step), cx);
                ws.start_setup(step, cx);
            });
            cx.run_until_parked();
        }
        for model in &models {
            workspace.update(&mut cx, |ws, cx| {
                let mut setup = demo::setup(SetupStep::Model);
                setup.model = model.clone();
                ws.set_setup(setup, cx);
                ws.start_setup(SetupStep::Model, cx);
            });
            cx.run_until_parked();
        }
    }
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
            model: ModelChoice::new("gpt-5.5", Effort::Auto),
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
        assert!(ws.run(&stopped).unwrap().goal.is_some());
        let done = tau_agent::tool::RunId("plugin-docs".into());
        assert!(ws.run(&done).unwrap().goal.is_none());

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

#[gpui::test]
fn a_plugin_note_opens_and_closes(cx: &mut TestAppContext) {
    let (workspace, mut cx, _) = open_demo(cx);
    workspace.update(&mut cx, |ws, cx| {
        let run = demo::run_id();
        assert!(!ws.note_open(&run, 3), "notes start closed");
        ws.toggle_note(&run, 3, cx);
        assert!(ws.note_open(&run, 3));
        assert!(!ws.note_open(&run, 4), "only that note opens");
        ws.toggle_note(&run, 3, cx);
        assert!(!ws.note_open(&run, 3));
    });
}

#[gpui::test]
fn a_log_card_opens_and_picks_one_change(cx: &mut TestAppContext) {
    let (workspace, mut cx, _) = open_demo(cx);
    workspace.update(&mut cx, |ws, cx| {
        let run = demo::run_id();
        assert!(!ws.card_open(&run, "log"), "logs start closed");
        ws.toggle_card(&run, "log", cx);
        assert!(ws.card_open(&run, "log"));
        assert!(!ws.card_open(&run, "other"), "only that card opens");

        assert_eq!(ws.picked_change(&run, "log"), None);
        ws.pick_change(&run, "log", "qpvuntsm", cx);
        assert_eq!(ws.picked_change(&run, "log"), Some("qpvuntsm"));
        ws.pick_change(&run, "log", "rlvkpnrz", cx);
        assert_eq!(ws.picked_change(&run, "log"), Some("rlvkpnrz"));
        assert_eq!(ws.picked_change(&run, "other"), None);
        ws.pick_change(&run, "log", "rlvkpnrz", cx);
        assert_eq!(
            ws.picked_change(&run, "log"),
            None,
            "a second click puts it back"
        );

        ws.toggle_card(&run, "log", cx);
        assert!(!ws.card_open(&run, "log"));
    });
}

#[gpui::test]
fn a_diff_card_opens_one_file_at_a_time(cx: &mut TestAppContext) {
    let (workspace, mut cx, _) = open_demo(cx);
    workspace.update(&mut cx, |ws, cx| {
        let run = demo::run_id();
        assert!(!ws.file_open(&run, "diff", "a.rs"), "files start closed");
        ws.toggle_file(&run, "diff", "a.rs", cx);
        assert!(ws.file_open(&run, "diff", "a.rs"));
        assert!(!ws.file_open(&run, "diff", "b.rs"));
        assert!(!ws.file_open(&run, "show", "a.rs"), "per card");
        ws.toggle_file(&run, "diff", "a.rs", cx);
        assert!(!ws.file_open(&run, "diff", "a.rs"));
    });
}

/// A fork lands from the compare screen: a preview first, then the
/// landing, which leaves a card in the parent's chat, closes the fork,
/// and opens the parent.
#[gpui::test]
fn a_fork_lands_on_its_parent(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open_demo(cx);
    let fork = demo::fork_id();
    let parent = tau_agent::tool::RunId("rotation-jitter".into());
    let change = serde_json::from_value::<tau_vcs::ChangeInfo>(serde_json::json!({
        "change_id": "qlmxnpvoqlmxnpvoqlmxnpvoqlmxnpvo",
        "commit_id": "0123456789abcdef0123456789abcdef01234567",
        "description": "feat(tau-ai): cap the backoff at the policy's max\n",
        "empty": false, "conflict": false, "immutable": false,
        "working_copy": false, "divergent": false, "bookmarks": [],
    }))
    .unwrap();
    let landing = tau_vcs::Landing {
        changes: vec![change],
        conflicts: Vec::new(),
        head: "0123456789abcdef0123456789abcdef01234567".into(),
    };
    let waiting = |ws: &Workspace| {
        ws.run(&parent).unwrap().items.iter().any(
            |item| matches!(item, Item::ForkReady { fork: f } if *f == fork),
        )
    };
    workspace.update(&mut cx, |ws, cx| {
        // The fork finishes: it waits in its parent's chat.
        ws.apply_event(
            &tau_agent::event::RunEvent::RunEnd {
                run: fork.clone(),
                parent: None,
                stop: StopReason::Stop,
                cost: 0.0,
            },
            cx,
        );
        assert!(waiting(ws), "the fork waits in its parent's chat");
        assert_eq!(ws.landing(&fork), None);
        ws.preview_landing(&fork, cx);
        assert_eq!(ws.landing(&fork), Some(&LandingState::Previewing));
        ws.set_landing_preview(&fork, Ok(landing.clone()), cx);
        ws.land(&fork, cx);
        assert_eq!(ws.landing(&fork), Some(&LandingState::Landing));
        ws.landed(&fork, Ok(landing), cx);
        assert_eq!(ws.landing(&fork), None);
        assert!(ws.is_closed(&fork), "the fork's chat closes");
        assert_eq!(ws.route(), &Route::Run(parent.clone()));
        let card = ws
            .run(&parent)
            .and_then(|view| {
                view.items.iter().rev().find_map(|item| match item {
                    Item::Landed(card) => Some(card.clone()),
                    _ => None,
                })
            })
            .expect("a landed card in the parent");
        assert_eq!(card.from, fork);
        assert_eq!(
            card.changes[0].subject,
            "cap the backoff at the policy's max"
        );
        assert!(!waiting(ws), "landed, it no longer waits");
    });
    let events = events.borrow();
    assert!(events.iter().any(|event| matches!(event,
        WorkspaceEvent::PreviewLanding { run } if *run == fork)));
    assert!(events.iter().any(|event| matches!(event,
        WorkspaceEvent::Land { run } if *run == fork)));
}

/// Dropping asks first, then closes the fork and opens its parent.
#[gpui::test]
fn a_fork_is_dropped_after_asking(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open_demo(cx);
    let fork = demo::fork_id();
    let parent = tau_agent::tool::RunId("rotation-jitter".into());
    workspace.update(&mut cx, |ws, cx| {
        ws.ask_drop(&fork, cx);
        assert_eq!(ws.landing(&fork), Some(&LandingState::ConfirmDrop));
        assert!(events.borrow().is_empty(), "asking sends nothing");
        ws.drop_child(&fork, cx);
        assert_eq!(ws.landing(&fork), Some(&LandingState::Dropping));
        ws.dropped(&fork, Ok(()), cx);
        assert_eq!(ws.landing(&fork), None);
        assert!(ws.is_closed(&fork));
        assert_eq!(ws.route(), &Route::Run(parent.clone()));
    });
    assert!(events.borrow().iter().any(|event| matches!(event,
        WorkspaceEvent::DropChild { run } if *run == fork)));
}

/// A sub-agent gets a chat of its own when it starts, on the task its
/// parent handed it, and closes when the parent's call returns.
#[gpui::test]
fn a_sub_agent_is_a_chat_until_its_call_returns(cx: &mut TestAppContext) {
    use std::sync::Arc;

    use tau_agent::{
        event::RunEvent,
        tool::{RunId, ToolOutput},
    };

    let (workspace, mut cx, events) = open_demo(cx);
    let parent = demo::run_id();
    let child = RunId("sub-1".into());
    workspace.update(&mut cx, |ws, cx| {
        ws.apply_event(
            &RunEvent::ToolStart {
                run: parent.clone(),
                call_id: "d1".into(),
                tool: Arc::from("delegate"),
                args: serde_json::json!({ "task": "write the tests" }),
            },
            cx,
        );
        ws.apply_event(
            &RunEvent::RunStart {
                run: child.clone(),
                parent: Some(parent.clone()),
                agent: Arc::from("coder"),
            },
            cx,
        );
        let view = ws.run(&child).expect("a chat for the sub-agent");
        assert_eq!(view.origin, tau_ui::view::Origin::SubAgent { parent: parent.clone() });
        assert!(matches!(view.items.first(), Some(Item::User(task)) if task == "write the tests"));
        ws.navigate(Route::Run(child.clone()), cx);

        ws.apply_event(
            &RunEvent::RunEnd {
                run: child.clone(),
                parent: Some(parent.clone()),
                stop: StopReason::Stop,
                cost: 0.0,
            },
            cx,
        );
        assert!(!ws.is_closed(&child), "open until the call returns");
        ws.apply_event(
            &RunEvent::ToolEnd {
                run: parent.clone(),
                call_id: "d1".into(),
                output: Arc::new(ToolOutput {
                    details: Some(serde_json::json!({
                        "run": "sub-1",
                        "landing": { "changes": [], "conflicts": [], "head": "00" },
                    })),
                    ..ToolOutput::text("done")
                }),
                is_error: false,
            },
            cx,
        );
        assert!(ws.is_closed(&child));
        assert_eq!(ws.route(), &Route::Run(parent.clone()), "back to the parent");
        let card = ws.run(&parent).unwrap().tool("d1").unwrap();
        assert!(matches!(&card.body, tau_ui::view::ToolBody::Delegated(landed) if landed.from == child));
    });
    assert!(events.borrow().iter().any(|event| matches!(event,
        WorkspaceEvent::CloseRun { run } if *run == child)));
}

/// A landing the host refuses shows why, in place of the preview.
#[gpui::test]
fn a_refused_landing_says_why(cx: &mut TestAppContext) {
    let (workspace, mut cx, _) = open_demo(cx);
    let fork = demo::fork_id();
    workspace.update(&mut cx, |ws, cx| {
        ws.land(&fork, cx);
        ws.landed(&fork, Err("rotation-jitter is still running".into()), cx);
        assert_eq!(
            ws.landing(&fork),
            Some(&LandingState::Preview(Err(
                "rotation-jitter is still running".into()
            )))
        );
        assert!(!ws.is_closed(&fork));
    });
}

#[gpui::test]
fn the_picker_offers_the_models_own_efforts(cx: &mut TestAppContext) {
    let (workspace, mut cx, _) = open_with_models(cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::NewRun, cx);
        ws.show_picker(PickerTarget::Next, cx);
        ws.pick_model("gpt-6-luna", cx);
        ws.show_picker(PickerTarget::Next, cx);
        ws.pick_effort(Effort::None, cx);
        assert_eq!(
            ws.next_model(),
            &ModelChoice::new("gpt-6-luna", Effort::None)
        );
        // gpt-6.1-sol starts at low, so none gives way to auto.
        ws.pick_model("gpt-6.1-sol", cx);
        assert_eq!(
            ws.next_model(),
            &ModelChoice::new("gpt-6.1-sol", Effort::Auto)
        );
    });
    assert_eq!(
        Effort::offered("gpt-6-sol"),
        [
            Effort::Auto,
            Effort::None,
            Effort::Low,
            Effort::Medium,
            Effort::High,
            Effort::Xhigh,
            Effort::Max
        ]
    );
    assert_eq!(Effort::offered("gpt-4.1"), [Effort::Auto]);
}

/// A chat changes model from its next message: no fork, the same run,
/// and a live run keeps its model until it stops.
#[gpui::test]
fn a_chat_goes_on_on_another_model(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open_with_models(cx);
    let run = demo::run_id();
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Run(run.clone()), cx);
        assert_eq!(ws.composer_target(), Some(PickerTarget::Run(run.clone())));
        // Live: the pick waits for the next message.
        ws.show_picker(PickerTarget::Run(run.clone()), cx);
        ws.pick_model("gpt-6-luna", cx);
        assert_eq!(ws.run(&run).unwrap().model, "gpt-5.5");
        assert_eq!(
            ws.choice_for(&PickerTarget::Run(run.clone())).model,
            "gpt-6-luna"
        );
    });
    finish_demo_run(&workspace, &mut cx);
    workspace.update(&mut cx, |ws, cx| {
        let runs = ws.runs().len();
        ws.submit_prompt("go on".into(), cx);
        assert_eq!(ws.runs().len(), runs, "no fork, no new run");
        let view = ws.run(&run).unwrap();
        assert_eq!(view.model, "gpt-6-luna");
        let reasoning =
            view.plan.iter().find(|f| f.name == "reasoning").unwrap();
        // Picked while the run was still on auto, so tau-reasoning
        // chooses again for the new model.
        assert_eq!(reasoning.value, "auto");
    });
    assert_eq!(
        events.borrow().last(),
        Some(&WorkspaceEvent::Resume {
            run: run.clone(),
            prompt: "go on".into(),
            model: ModelChoice::new("gpt-6-luna", Effort::Auto),
        })
    );
    // The pick was spent: the next message stays on gpt-6-luna from the
    // run itself.
    workspace.update(&mut cx, |ws, _| {
        assert_eq!(
            ws.choice_for(&PickerTarget::Run(run.clone())),
            ModelChoice::new("gpt-6-luna", Effort::Auto)
        );
    });
}

/// Shift+enter takes a new line in the composer; up moves between its
/// lines; enter sends it all.
#[gpui::test]
fn the_composer_takes_new_lines(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open_demo(cx);
    workspace.update_in(&mut cx, |ws, window, cx| {
        ws.start_new_run(window, cx);
    });
    cx.simulate_input("one");
    cx.simulate_keystrokes("shift-enter");
    cx.simulate_input("two");
    cx.run_until_parked();
    workspace.read_with(&cx, |ws, cx| {
        assert_eq!(ws.composer_text(cx), "one\ntwo");
    });
    // From the end of "two", up lands at the end of "one".
    cx.simulate_keystrokes("up");
    cx.simulate_input("!");
    cx.simulate_keystrokes("enter");
    assert!(matches!(
        events.borrow().last(),
        Some(WorkspaceEvent::NewRun { prompt, .. }) if prompt == "one!\ntwo"
    ));
}

/// With the model picker open, the wheel over the chat behind it does
/// not scroll the chat.
#[gpui::test]
fn the_picker_keeps_the_chat_from_scrolling(cx: &mut TestAppContext) {
    let (workspace, mut cx, _) = open_with_models(cx);
    let run = demo::run_id();
    finish_demo_run(&workspace, &mut cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Run(run.clone()), cx);
    });
    cx.run_until_parked();
    let wheel_up = |cx: &mut VisualTestContext| {
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: gpui::point(gpui::px(420.), gpui::px(200.)),
            delta: gpui::ScrollDelta::Pixels(gpui::point(
                gpui::px(0.),
                gpui::px(120.),
            )),
            ..Default::default()
        });
    };
    workspace.update(&mut cx, |ws, cx| {
        ws.show_picker(PickerTarget::Run(run.clone()), cx);
    });
    cx.run_until_parked();
    wheel_up(&mut cx);
    workspace.read_with(&cx, |ws, _| {
        assert!(ws.follows(), "the chat did not scroll");
    });
    // Closed, the same wheel scrolls it.
    workspace.update(&mut cx, |ws, cx| ws.close_picker(cx));
    cx.run_until_parked();
    wheel_up(&mut cx);
    workspace.read_with(&cx, |ws, _| {
        assert!(!ws.follows(), "the chat scrolled");
    });
}

/// Closing search or the model picker with escape gives the keys back:
/// ctrl+k opens search again.
#[gpui::test]
fn search_opens_again_after_escape(cx: &mut TestAppContext) {
    let (workspace, mut cx, _) = open_with_models(cx);
    let searching = |cx: &mut VisualTestContext| {
        workspace.read_with(cx, |ws, _| ws.is_searching())
    };
    cx.simulate_keystrokes("ctrl-k");
    assert!(searching(&mut cx));
    cx.simulate_keystrokes("escape");
    assert!(!searching(&mut cx));
    cx.simulate_keystrokes("ctrl-k");
    assert!(searching(&mut cx), "ctrl+k works after escape");
    cx.simulate_keystrokes("escape");

    workspace.update_in(&mut cx, |ws, window, cx| {
        ws.navigate(Route::NewRun, cx);
        ws.open_picker(PickerTarget::Next, window, cx);
    });
    cx.simulate_keystrokes("escape");
    workspace.read_with(&cx, |ws, _| assert!(ws.picker().is_none()));
    cx.simulate_keystrokes("ctrl-k");
    assert!(searching(&mut cx), "ctrl+k works after the picker");
}

/// The demo's catalog on the ChatGPT plan, with the note on it not yet
/// read.
fn on_plan(ws: &mut Workspace, cx: &mut gpui::Context<Workspace>) {
    let mut catalog = ws.catalog().clone();
    catalog.models.settings.plan_notice_seen = false;
    ws.set_catalog(catalog, cx);
}

/// On the plan, the note shows until "Got it", which saves that it was
/// read; the composer says runs use the plan, and "Manage usage" opens
/// ChatGPT's usage settings.
#[gpui::test]
fn the_plan_notice_shows_once_and_the_composer_says_so(
    cx: &mut TestAppContext,
) {
    let (workspace, mut cx, events) = open_demo(cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::NewRun, cx);
        on_plan(ws, cx);
        assert!(ws.shows_plan_notice());
        assert!(ws.uses_plan());
    });
    // Drawn with the note and the line under the composer.
    cx.run_until_parked();
    workspace.update(&mut cx, |ws, cx| {
        ws.dismiss_plan_notice(cx);
        assert!(!ws.shows_plan_notice());
        assert!(ws.uses_plan(), "the line stays");
        ws.manage_usage(cx);
    });
    let saved = events.borrow().iter().rev().find_map(|event| match event {
        WorkspaceEvent::SaveModelSettings(settings) => Some(settings.clone()),
        _ => None,
    });
    assert!(saved.is_some_and(|settings| settings.plan_notice_seen));
    assert_eq!(
        cx.opened_url().as_deref(),
        Some(tau_ui::models::USAGE_SETTINGS_URL)
    );
    // Off the plan, neither shows.
    workspace.update(&mut cx, |ws, cx| {
        let mut catalog = ws.catalog().clone();
        catalog.models.access.chatgpt = false;
        catalog.models.settings.plan_notice_seen = false;
        ws.set_catalog(catalog, cx);
        assert!(!ws.shows_plan_notice() && !ws.uses_plan());
    });
    cx.run_until_parked();
}

/// A run stopped on the plan's usage limit says so, with "Manage usage"
/// first; a temporary refusal, already retried, says nothing more.
#[gpui::test]
fn a_usage_limit_offers_to_manage_usage(cx: &mut TestAppContext) {
    use tau_ui::plan_usage::{PlanAction, PlanAlert};
    let (workspace, mut cx, events) = open_demo(cx);
    workspace.update(&mut cx, |ws, cx| {
        let mut busy = demo::usage_limit();
        busy.recovery = tau_ai::retry::Recovery::RetryLater;
        ws.show_plan_refusal(&busy, cx);
        assert_eq!(ws.plan_alert(), None);
        ws.show_plan_refusal(&demo::usage_limit(), cx);
        let alert = ws.plan_alert().unwrap();
        assert_eq!(alert, &PlanAlert::UsageLimit);
        assert_eq!(alert.title(), "Usage limit reached");
    });
    // Drawn over the app.
    cx.run_until_parked();
    workspace.update(&mut cx, |ws, cx| {
        ws.plan_action(PlanAction::ManageUsage, cx);
        assert_eq!(ws.plan_alert(), None);
    });
    assert_eq!(
        cx.opened_url().as_deref(),
        Some(tau_ui::models::USAGE_SETTINGS_URL)
    );
    // Never a silent switch: nothing asked the host to use another way
    // to pay.
    assert!(!events.borrow().iter().any(|event| matches!(
        event,
        WorkspaceEvent::SwitchChatGpt { .. } | WorkspaceEvent::SignOut
    )));
}

/// A sign-in OpenAI no longer takes asks to sign the active account in
/// again; one without plan usage asks to enable it, from the model
/// setup.
#[gpui::test]
fn plan_alerts_lead_to_signing_in(cx: &mut TestAppContext) {
    use tau_ui::plan_usage::{PlanAction, PlanAlert};
    let (workspace, mut cx, events) = open_demo(cx);
    let active = demo::chatgpt_accounts()[0].id.clone();
    workspace.update(&mut cx, |ws, cx| {
        let mut refusal = demo::usage_limit();
        refusal.recovery = tau_ai::retry::Recovery::SignInAgain;
        ws.show_plan_refusal(&refusal, cx);
        assert!(matches!(
            ws.plan_alert(),
            Some(PlanAlert::SignInAgain { .. })
        ));
        ws.plan_action(PlanAction::SignInAgain, cx);
        assert_eq!(ws.route(), &Route::Setup(SetupStep::Model));
        assert_eq!(ws.setup().model, ModelAccess::SigningIn { url: None });
    });
    assert_eq!(
        events.borrow().last(),
        Some(&WorkspaceEvent::ChatGptSignIn {
            account: Some(active.clone()),
            consent: false,
        })
    );
    workspace.update(&mut cx, |ws, cx| {
        let mut refusal = demo::usage_limit();
        refusal.recovery = tau_ai::retry::Recovery::EnablePlanUsage;
        ws.show_plan_refusal(&refusal, cx);
        assert_eq!(ws.plan_alert(), Some(&PlanAlert::EnablePlanUsage));
        ws.plan_action(PlanAction::EnablePlanUsage, cx);
    });
    assert_eq!(
        events.borrow().last(),
        Some(&WorkspaceEvent::ChatGptSignIn {
            account: Some(active),
            consent: true,
        })
    );
}

/// The account picker: switching asks the host, a sign-in without plan
/// usage stays on the model step, and a pasted redirect finishes the
/// sign-in in progress.
#[gpui::test]
fn chatgpt_accounts_switch_and_sign_in(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open_demo(cx);
    let work = demo::chatgpt_accounts()[1].id.clone();
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Models, cx);
    });
    // The Models screen, with the accounts.
    cx.run_until_parked();
    workspace.update(&mut cx, |ws, cx| {
        ws.switch_chatgpt(&work, cx);
        ws.connect_model(cx);
        ws.sign_in_chatgpt(None, false, cx);
        ws.update_setup(
            SetupUpdate::Model(ModelAccess::SigningIn {
                url: Some(demo::DEMO_AUTHORIZE_URL.into()),
            }),
            cx,
        );
    });
    // The step waiting for the browser, with the paste field.
    cx.run_until_parked();
    workspace.update(&mut cx, |ws, cx| {
        ws.update_setup(
            SetupUpdate::Model(ModelAccess::PlanDisabled {
                account: "you@work.example".into(),
            }),
            cx,
        );
        assert_eq!(ws.route(), &Route::Setup(SetupStep::Model));
    });
    cx.run_until_parked();
    let events = events.borrow();
    assert!(events.contains(&WorkspaceEvent::SwitchChatGpt {
        account: work.clone()
    }));
}

fn computer() -> tau_ui::pairing::Computer {
    demo::computer()
}

#[gpui::test]
fn scanning_pairs_then_opens_the_runs(cx: &mut TestAppContext) {
    use tau_ui::pairing::{PairRequest, PairStep, PairingUpdate, Progress};

    let (workspace, mut cx, events) = open(cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.start_pairing(PairStep::Welcome, cx);
        ws.scan_pairing_code(cx);
        assert_eq!(ws.route(), &Route::Pair(PairStep::Scan));
        assert!(ws.pairing().busy());
        ws.update_pairing(
            PairingUpdate::Progress(Progress::Connecting {
                address: computer().address,
            }),
            cx,
        );
        assert_eq!(ws.route(), &Route::Pair(PairStep::Scan));
        ws.update_pairing(PairingUpdate::Paired(computer()), cx);
        assert_eq!(ws.route(), &Route::Pair(PairStep::Paired));
        // Paired is not a step to go back from.
        assert!(!ws.can_go_back());
    });
    cx.run_until_parked();
    workspace.update(&mut cx, |ws, cx| ws.open_tau(cx));
    workspace.update(&mut cx, |ws, _| assert_eq!(ws.route(), &Route::Home));
    // With no name typed, none is sent.
    assert_eq!(
        events.borrow().as_slice(),
        [WorkspaceEvent::Pair(PairRequest::Scan)]
    );
}

#[gpui::test]
fn a_typed_address_compares_the_certificate_first(cx: &mut TestAppContext) {
    use tau_ui::pairing::{PairRequest, PairStep, PairingUpdate, Progress};

    let (workspace, mut cx, events) = open(cx);
    let fingerprint = computer().fingerprint;
    workspace.update(&mut cx, |ws, cx| {
        ws.start_pairing(PairStep::Welcome, cx);
        ws.type_address(cx);
        // A code that is not one is caught before anything connects.
        ws.pair_fields_for_test("100.84.12.7", "K7QM", cx);
        ws.connect_typed(cx);
        assert!(matches!(ws.pairing().progress, Progress::Failed(_)));
        ws.pair_fields_for_test("100.84.12.7", "k7qm 2xpa", cx);
        ws.connect_typed(cx);
        assert!(ws.pairing().busy());
        ws.update_pairing(
            PairingUpdate::Progress(Progress::Compare {
                address: computer().address,
                fingerprint,
            }),
            cx,
        );
        ws.trust_certificate(cx);
        assert!(ws.pairing().busy());
    });
    cx.run_until_parked();
    let events = events.borrow();
    let [
        WorkspaceEvent::Pair(PairRequest::Typed { address, secret }),
        WorkspaceEvent::Pair(PairRequest::Trust(trusted)),
    ] = events.as_slice()
    else {
        panic!("{events:?}");
    };
    assert_eq!(address.to_string(), "100.84.12.7:7443");
    assert_eq!(secret.to_string(), "K7QM-2XPA");
    assert_eq!(*trusted, fingerprint);
}

#[gpui::test]
fn leaving_a_pairing_halfway_cancels_it(cx: &mut TestAppContext) {
    use tau_ui::pairing::{PairRequest, PairStep, Progress};

    let (workspace, mut cx, events) = open(cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.start_pairing(PairStep::Welcome, cx);
        ws.scan_pairing_code(cx);
        ws.back(cx);
        assert_eq!(ws.route(), &Route::Pair(PairStep::Welcome));
        assert_eq!(ws.pairing().progress, Progress::Idle);
    });
    cx.run_until_parked();
    assert_eq!(
        events.borrow().as_slice(),
        [
            WorkspaceEvent::Pair(PairRequest::Scan),
            WorkspaceEvent::Pair(PairRequest::Cancel)
        ]
    );
}

#[gpui::test]
fn an_unreachable_computer_leads_back_once_it_answers(cx: &mut TestAppContext) {
    use tau_ui::pairing::{PairRequest, PairStep, PairingUpdate};

    let (workspace, mut cx, events) = open(cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.update_pairing(
            PairingUpdate::Unreachable {
                computer: computer(),
                tries: 3,
            },
            cx,
        );
        assert_eq!(ws.route(), &Route::Pair(PairStep::Unreachable));
        ws.retry_connection(cx);
        assert!(ws.pairing().busy());
        // Tapping again while it tries asks nothing more.
        ws.retry_connection(cx);
        ws.update_pairing(PairingUpdate::Connected(computer()), cx);
        assert_eq!(ws.route(), &Route::Home);
        assert_eq!(ws.pairing().tries, 0);
    });
    cx.run_until_parked();
    assert_eq!(
        events.borrow().as_slice(),
        [WorkspaceEvent::Pair(PairRequest::Retry)]
    );
}

#[gpui::test]
fn every_pairing_screen_draws(cx: &mut TestAppContext) {
    use tau_ui::pairing::PairStep;

    let (workspace, mut cx, _) = open(cx);
    for step in [
        PairStep::Welcome,
        PairStep::Scan,
        PairStep::Address,
        PairStep::Paired,
        PairStep::Unreachable,
    ] {
        workspace.update(&mut cx, |ws, cx| {
            ws.set_pairing(demo::pairing(step), cx);
            ws.start_pairing(step, cx);
        });
        cx.run_until_parked();
    }
}

#[gpui::test]
fn a_mirrored_workspace_echoes_what_the_host_applies(cx: &mut TestAppContext) {
    use tau_ui::update::HostUpdate;

    let (workspace, mut cx, _) = open(cx);
    let echoed = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = echoed.clone();
    cx.update(|_, cx| {
        cx.subscribe(&workspace, move |_, update: &HostUpdate, _| {
            seen.borrow_mut().push(update.clone())
        })
        .detach()
    });
    let alert = HostUpdate::alert("Could not start the run", "no model");
    workspace.update(&mut cx, |ws, cx| {
        // Not mirrored: nothing is echoed.
        ws.apply(alert.clone(), cx);
        ws.set_mirrored(true);
        ws.apply(alert.clone(), cx);
    });
    cx.run_until_parked();
    assert_eq!(echoed.borrow().as_slice(), [alert]);
}

#[gpui::test]
fn a_snapshot_replaces_the_runs(cx: &mut TestAppContext) {
    let (desktop, mut desktop_cx, _) = open(cx);
    let (phone, mut phone_cx, _) = open(cx);
    let snapshot = desktop.update(&mut desktop_cx, |ws, cx| {
        ws.add_history(demo::history(), cx);
        ws.snapshot()
    });
    phone.update(&mut phone_cx, |ws, cx| ws.apply(snapshot, cx));
    let desktop_runs =
        desktop.update(&mut desktop_cx, |ws, _| ws.runs().to_vec());
    let phone_runs = phone.update(&mut phone_cx, |ws, _| ws.runs().to_vec());
    assert_eq!(phone_runs, desktop_runs);
}
