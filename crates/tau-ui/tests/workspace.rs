//! The workspace's onboarding and pull request flows, driven the way a
//! host drives them, in GPUI's test app.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use gpui::{Entity, TestAppContext, VisualTestContext};
use tau_agent::event::StopReason;
use tau_ui::demo;
use tau_ui_remote::{
    Workspace,
    WorkspaceEvent,
    catalog::Catalog,
    models::{Effort, ModelChoice},
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
    // The app's plugins, every host half included.
    tau::install();
    cx.update(tau_ui_remote::init);
    let window = cx.add_window(|window, cx| {
        // `retry-after` is tau-agent's main chat, which chats fork.
        let catalog = Catalog {
            repos: vec![tau_ui_remote::catalog::Repo {
                name: "tau-agent".into(),
                main: Some(demo::run_id()),
                ..Default::default()
            }],
            ..Default::default()
        };
        Workspace::new("tau", vec![demo::retry_after()], catalog, window, cx)
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

/// Each way into a fork sends the turn it forks at and the model it
/// runs on: fork mode from the header forks the latest turn, a turn's
/// fork button its own turn, and a fork set up at a turn may pick
/// another model first; otherwise a fork starts on its run's model and
/// effort. Only turns the run has can be forked from, and once the fork
/// is sent, the next message is an ordinary one again.
#[gpui::test]
fn each_way_into_a_fork_sends_its_turn_and_model(cx: &mut TestAppContext) {
    #[derive(Debug, Clone, Copy)]
    enum Way {
        Mode,
        Button(u32),
        OtherModel(u32),
    }
    let own = ModelChoice::new("gpt-5.5", Effort::Auto);
    for way in [Way::Mode, Way::Button(2), Way::OtherModel(3)] {
        let (workspace, mut cx, events) = open_with_models(cx);
        let run = demo::run_id();
        workspace.update(&mut cx, |ws, cx| {
            for (_, update) in demo::script() {
                ws.update_run(&run, update, cx);
            }
            ws.navigate(Route::Run(run.clone()), cx);
        });
        let last = workspace.read_with(&cx, |ws, _| ws.run(&run).unwrap().turn);
        assert!(last > 3, "{last}");
        let (turn, model) = workspace.update_in(&mut cx, |ws, window, cx| {
            let sent = match way {
                Way::Mode => {
                    ws.start_fork(window, cx);
                    (last, own.clone())
                }
                Way::Button(turn) => {
                    let view = ws.run(&run).unwrap();
                    assert!(ws.can_fork_at(view, turn));
                    assert!(!ws.can_fork_at(view, 0));
                    assert!(!ws.can_fork_at(view, view.turn + 1));
                    ws.fork_from(&run, turn, window, cx);
                    (turn, own.clone())
                }
                Way::OtherModel(turn) => {
                    // On an open run, the composer's model is the chat's.
                    assert_eq!(
                        ws.composer_target(),
                        Some(PickerTarget::Run(run.clone()))
                    );
                    assert!(ws.start_fork_at(&run, turn, cx));
                    ws.show_picker(PickerTarget::Fork, cx);
                    ws.pick_model("gpt-6-luna", cx);
                    (turn, ModelChoice::new("gpt-6-luna", Effort::Auto))
                }
            };
            ws.submit_prompt("go another way".into(), cx);
            sent
        });
        assert_eq!(
            events.borrow().last(),
            Some(&WorkspaceEvent::Fork {
                run: run.clone(),
                turn: Some(turn),
                prompt: "go another way".into(),
                model,
                images: Vec::new(),
            }),
            "{way:?}"
        );
        workspace.update(&mut cx, |ws, cx| {
            ws.submit_prompt("hello".into(), cx);
        });
        assert!(
            matches!(events.borrow().last(), Some(WorkspaceEvent::Say { .. })),
            "{way:?}"
        );
    }
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

/// A fork the host answers is a chat of its own, open, under the run
/// it forked; a written title replaces its placeholder, in the list and
/// under the parent.
#[gpui::test]
fn the_demo_answers_a_fork_with_a_run(cx: &mut TestAppContext) {
    let (workspace, mut cx, _) = open(cx);
    cx.update(|_, cx| {
        demo::start(&workspace, cx);
    });
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
    let fork = workspace.read_with(&cx, |ws, _| {
        let fork = ws.current().expect("the fork is open");
        assert_eq!(
            fork.origin,
            tau_ui_remote::view::Origin::Fork {
                from: run.clone(),
                turn: 2
            }
        );
        let parent = ws.run(&run).unwrap();
        assert!(parent.children.iter().any(|child| child.id == fork.id));
        fork.id.clone()
    });
    workspace.update(&mut cx, |ws, cx| {
        ws.apply(
            tau_ui_remote::update::HostUpdate::Titled {
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

    // The host works on its own runtime's threads, as it does in tau.
    cx.executor().allow_parking();
    let (workspace, mut cx, _) = open(cx);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let store = runtime.block_on(tau_store_sqlite::memory()).unwrap();
    let agent =
        tau_agent::agent::Agent::new(ScriptedModel::new()).name("coder");
    // The test's own directory: a repository list another run left in
    // the shared temp directory would give the host a repository.
    let dir = tempfile::tempdir().unwrap().keep();
    let config = HostConfig {
        account: tau_ai::chatgpt::AccountId::parse("test-account").unwrap(),
        credentials: Credentials::new(dir.join("config")),
        model: Some("gpt-5.5".into()),
        store: dir.join("unused.db"),
        repos: dir.join("repos"),
        settings: dir.join("models.json"),
        repo_list: dir.join("repos.json"),
        skills: std::env::temp_dir().join("tau-test-skills-none"),
        identity: tau_vcs_host::Identity::default(),
    };
    // No repository is listed, so the demo's run cannot be forked.
    let (host, events) = Host::with_agent(runtime, agent, store, config);
    // Memory's pass after each run asks the model once more: these
    // scripts answer only the requests they write.
    let host = host.with_plugin_settings(
        tau_memory::NAME,
        serde_json::json!({ "after_each_run": false }),
    );
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
        assert!(message.contains("no listed repository"), "{message}");
    });
}

fn open_with_models(
    cx: &mut TestAppContext,
) -> (
    Entity<Workspace>,
    VisualTestContext,
    std::rc::Rc<std::cell::RefCell<Vec<WorkspaceEvent>>>,
) {
    // The app's plugins, every host half included.
    tau::install();
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
        assert_eq!(
            ws.next_model(),
            &ModelChoice::new("gpt-6.1-sol", Effort::Auto)
        );
        ws.show_picker(PickerTarget::Next, cx);
        ws.pick_effort(Effort::Low, cx);
        ws.pick_model("gpt-6-luna", cx);
        assert!(ws.alert().is_none(), "a plan model asks nothing to confirm");
        assert!(ws.picker().is_none(), "picking a model closes the picker");
        ws.submit_prompt("fix it".into(), cx);
    });
    assert_eq!(
        events.borrow().last(),
        Some(&WorkspaceEvent::NewRun {
            prompt: "fix it".into(),
            model: ModelChoice::new("gpt-6-luna", Effort::Low),
            repo: "tau-agent".into(),
            images: Vec::new(),
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
        // tau-reasoning's settings, from its pane on the Plugins screen,
        // through its handle: everywhere, then tau-agent's own copy.
        ws.navigate(Route::Plugins, cx);
        ws.pick_plugin(tau_reasoning::NAME, None, cx);
        let reasoning = ws.plugin_handle(tau_reasoning::NAME);
        reasoning.save_settings(
            &tau_reasoning::ui::Settings {
                redecide: true,
                threshold: 0.9,
            },
            cx,
        );
        ws.set_plugin_scope(Some("tau-agent".into()), cx);
        reasoning.save_settings_in(
            Some("tau-agent"),
            &tau_reasoning::ui::Settings {
                redecide: false,
                threshold: 0.5,
            },
            cx,
        );
    });
    // Drawn with them.
    cx.run_until_parked();
    workspace.update(&mut cx, |ws, _| {
        let catalog = ws.catalog();
        assert_eq!(
            catalog
                .settings_in(tau_reasoning::NAME, Some("tau-agent"))
                .map(|value| value.json().clone()),
            Some(serde_json::json!({ "redecide": false, "threshold": 0.5 }))
        );
        assert!(catalog.has_own_settings("tau-agent", tau_reasoning::NAME));
    });
    let plugin_saved: Vec<(String, Option<String>, Option<serde_json::Value>)> =
        events
            .borrow()
            .iter()
            .filter_map(|event| match event {
                WorkspaceEvent::PluginSettings {
                    plugin,
                    repo,
                    settings,
                } => Some((plugin.clone(), repo.clone(), settings.clone())),
                _ => None,
            })
            .collect();
    assert_eq!(
        plugin_saved,
        [
            (
                tau_reasoning::NAME.to_owned(),
                None,
                Some(serde_json::json!({ "redecide": true, "threshold": 0.9 }))
            ),
            (
                tau_reasoning::NAME.to_owned(),
                Some("tau-agent".to_owned()),
                Some(
                    serde_json::json!({ "redecide": false, "threshold": 0.5 })
                )
            ),
        ]
    );
    // Each change is sent as itself, not as the settings this window has.
    let events = events.borrow();
    assert!(events.iter().any(|event| matches!(event,
        WorkspaceEvent::SetDefaultModel { agent, choice }
            if agent == "coder" && choice.model == "gpt-6-luna")));
    assert!(events.contains(&WorkspaceEvent::HideModel {
        id: "gpt-6-astra".into(),
        hidden: true,
    }));
}

/// tau-reasoning draws itself: its note in the demo's transcript, its
/// line and plan field, its step on the Plan screen, and its page,
/// reached from its catalog entry.
#[gpui::test]
fn reasoning_draws_its_own_ui(cx: &mut TestAppContext) {
    let (workspace, mut cx, _) = open_demo(cx);
    let run = demo::run_id();
    workspace.update(&mut cx, |ws, cx| {
        for (_, update) in demo::script() {
            ws.update_run(&run, update, cx);
        }
        ws.navigate(Route::Run(run.clone()), cx);
    });
    cx.run_until_parked();
    workspace.update(&mut cx, |ws, cx| {
        let view = ws.run(&run).unwrap().clone();
        let plan = ws.run_plan(&view, cx);
        assert!(plan.iter().any(|field| field.name == "reasoning"
            && field.value == "high"
            && field.set_by.as_deref() == Some(tau_reasoning::NAME)));
        let statuses = ws.run_statuses(&view, cx);
        assert!(
            statuses
                .iter()
                .any(|status| status.name == tau_reasoning::NAME
                    && status.state == "chose high")
        );
        // Its catalog entry leads to its page, about the run in view.
        let route = ws.plugin_route_named(tau_reasoning::NAME, &run).unwrap();
        let Route::Plugin {
            plugin,
            page,
            params,
        } = &route
        else {
            panic!("{route:?}");
        };
        assert_eq!(
            (plugin.as_str(), page.as_str()),
            (tau_reasoning::NAME, "choices")
        );
        assert_eq!(params.get("run").map(String::as_str), Some(&*run.0));
        ws.navigate(route, cx);
    });
    cx.run_until_parked();
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Plan(run.clone()), cx);
    });
    cx.run_until_parked();
}

/// The demo's workspace: three repositories, tau-agent open.
/// The demo's runs and repositories, its host answering.
fn open_demo(
    cx: &mut TestAppContext,
) -> (
    Entity<Workspace>,
    VisualTestContext,
    std::rc::Rc<std::cell::RefCell<Vec<WorkspaceEvent>>>,
) {
    // The app's plugins, every host half included.
    tau::install();
    let (workspace, cx, events, _) = open_demo_host(cx);
    (workspace, cx, events)
}

/// What the workspace emitted, in order.
type Events = std::rc::Rc<std::cell::RefCell<Vec<WorkspaceEvent>>>;

fn open_demo_host(
    cx: &mut TestAppContext,
) -> (
    Entity<Workspace>,
    VisualTestContext,
    Events,
    std::sync::Arc<demo::DemoHost>,
) {
    // The app's plugins, every host half included.
    tau::install();
    let (workspace, mut cx, events) = open(cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.add_history(demo::history(), cx);
        ws.set_catalog(demo::catalog(), cx);
    });
    let host = cx.update(|_, cx| demo::start(&workspace, cx));
    events.borrow_mut().clear();
    (workspace, cx, events, host)
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
fn repositories_are_added_and_removed(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open_demo(cx);
    workspace.update_in(&mut cx, |ws, _, cx| {
        ws.open_memory("docbert", cx);
    });
    workspace.update(&mut cx, |ws, cx| {
        ws.add_repo(
            tau_ui_remote::catalog::Repo::new("dotfiles", "~/dotfiles"),
            cx,
        );
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
fn connecting_from_the_models_screen_comes_back_to_it(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open_demo(cx);
    let work = demo::chatgpt_accounts()[1].id.clone();
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Models, cx);
        // The account picker asks the host to switch.
        ws.switch_chatgpt(&work, cx);
        ws.connect_model(cx);
        assert_eq!(ws.route(), &Route::Setup(SetupStep::Model));
        assert!(ws.can_go_back(), "the Models screen is a step back");
        // A sign-in without plan usage stays on the step, to say so.
        ws.update_setup(
            SetupUpdate::Model(ModelAccess::PlanDisabled {
                account: "you@work.example".into(),
            }),
            cx,
        );
        assert_eq!(ws.route(), &Route::Setup(SetupStep::Model));
        ws.update_setup(
            SetupUpdate::Model(ModelAccess::Connected { label: "m".into() }),
            cx,
        );
        assert_eq!(ws.route(), &Route::Models);
        ws.sign_out(cx);
    });
    assert!(
        events
            .borrow()
            .contains(&WorkspaceEvent::SwitchChatGpt { account: work })
    );
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
            SetupUpdate::Repos(vec![tau_ui_remote::setup::RepoChoice {
                name: "octocat/hello".into(),
                description: String::new(),
                branch: "main".into(),
                selected: false,
                pushed_at: String::new(),
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
            WorkspaceEvent::SetDefaultModel { choice, .. } => {
                Some(choice.model.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(saved, ["gpt-6-astra"]);
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
    use tau_ui_remote::motion::{Link, Mood};
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

/// On the model step, whatever the sign-in reports (signed in, waiting,
/// refused, without plan usage, not eligible, failed) is shown there:
/// the step stays, to show the account or the refusal, until the person
/// moves on. (Every onboarding screen draws in `every_demo_screen_opens`.)
#[gpui::test]
fn the_model_step_stays_whatever_the_sign_in_reports(cx: &mut TestAppContext) {
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
            account: "you@example.com".into(),
            detail: "403 subscription_sharing_user_not_eligible".into(),
        },
        ModelAccess::Failed("no".into()),
    ];
    for model in &models {
        workspace.update(&mut cx, |ws, cx| {
            ws.set_setup(demo::setup(SetupStep::Model), cx);
            ws.start_setup(SetupStep::Model, cx);
            ws.update_setup(SetupUpdate::Model(model.clone()), cx);
        });
        cx.run_until_parked();
        workspace.read_with(&cx, |ws, _| {
            assert_eq!(
                ws.route(),
                &Route::Setup(SetupStep::Model),
                "{model:?}"
            );
        });
    }
}

/// Answers the last message sent as a host does for a finished run: it
/// goes on with it.
fn host_resumes(
    workspace: &Entity<Workspace>,
    cx: &mut VisualTestContext,
    events: &Events,
) {
    let Some(WorkspaceEvent::Say {
        run,
        text,
        model,
        images: _,
    }) = events.borrow().last().cloned()
    else {
        panic!("no message was sent");
    };
    workspace.update(cx, |ws, cx| {
        let resumed = tau_ui_remote::update::HostUpdate::Resumed {
            run,
            prompt: text,
            model,
            pictures: Vec::new(),
        };
        ws.apply(resumed, cx)
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
        ws.submit_prompt("now add a test for it".into(), cx);
    });
    assert_eq!(
        events.borrow().last(),
        Some(&WorkspaceEvent::Say {
            run: run.clone(),
            text: "now add a test for it".into(),
            model: ModelChoice::new("gpt-5.5", Effort::Auto),
            images: Vec::new(),
        })
    );
    // The host took it: no new chat, the same run back to work, at the
    // top.
    workspace.update(&mut cx, |ws, _| {
        let view = ws.run(&run).unwrap();
        assert_eq!(ws.runs()[0].id, run);
        assert!(view.status.is_live());
        assert!(view.items.iter().any(
            |item| matches!(item, Item::User(text) if text == "now add a test for it")
        ));
        assert_eq!(ws.route(), &Route::Run(run.clone()));
    });
    // If the host cannot, the run ends as it had.
    workspace.update(&mut cx, |ws, cx| {
        ws.apply(
            tau_ui_remote::update::HostUpdate::Resumed {
                run: run.clone(),
                prompt: "and the docs".into(),
                model: ModelChoice::new("gpt-5.5", Effort::Auto),
                pictures: Vec::new(),
            },
            cx,
        );
        ws.apply(
            tau_ui_remote::update::HostUpdate::ResumeFailed(run.clone()),
            cx,
        );
        let view = ws.run(&run).unwrap();
        assert!(!view.items.iter().any(
            |item| matches!(item, Item::User(text) if text == "and the docs")
        ));
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
    });
    workspace.update(&mut cx, |ws, cx| {
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
    });
    workspace.read_with(&cx, |ws, _| assert!(!ws.is_closed(&done)));
}

#[gpui::test]
fn a_phone_closes_the_conversation_it_shows(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open_demo(cx);
    let done = tau_agent::tool::RunId("plugin-docs".into());
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Run(done.clone()), cx);
        ws.close_run_to_list(&done, cx);
    });
    workspace.update(&mut cx, |ws, _| {
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
fn history_runs_the_query_typed_and_shows_the_answer(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open_demo(cx);
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
    // The host's answer shows.
    cx.run_until_parked();
    workspace.read_with(&cx, |ws, _| {
        assert!(matches!(ws.query_result(), Some(Ok(_))), "a table");
    });
}

#[gpui::test]
fn search_finds_runs_repositories_and_actions(cx: &mut TestAppContext) {
    let (workspace, mut cx, _) = open_demo(cx);
    workspace.update(&mut cx, |ws, cx| {
        use tau_ui_remote::search::Pick;
        let hits = ws.search_hits("rerank", cx);
        assert_eq!(hits[0].label, "rerank-latency");
        assert_eq!(hits[0].detail, "chat in docbert");
        // Every word must match.
        assert!(ws.search_hits("rerank homelab", cx).is_empty());
        let hits = ws.search_hits("homelab", cx);
        assert!(
            hits.iter()
                .any(|hit| hit.pick == Pick::Repo("homelab.nix".into()))
        );
        assert!(
            hits.iter()
                .any(|hit| hit.pick == Pick::NewRunIn("homelab.nix".into()))
        );
        let hits = ws.search_hits("history", cx);
        assert_eq!(hits[0].pick, Pick::Screen(Route::History));
        // Nothing typed: repositories and things to do.
        assert!(!ws.search_hits("", cx).is_empty());
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
        assert!(ws.alert().unwrap().1.contains("neither text nor"));
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

/// A PNG file in `dir`, `n` pixels wide.
fn png_file(dir: &std::path::Path, n: u32) -> std::path::PathBuf {
    let path = dir.join(format!("shot-{n}.png"));
    image::RgbImage::new(n, 2).save(&path).unwrap();
    path
}

/// The pictures the workspace asked the host for.
fn fetched(
    events: &std::rc::Rc<std::cell::RefCell<Vec<WorkspaceEvent>>>,
) -> usize {
    events
        .borrow()
        .iter()
        .filter(|event| matches!(event, WorkspaceEvent::Picture { .. }))
        .count()
}

/// An image attached goes with the message as an image, not as text;
/// the window that sent it draws it without asking the host, and one it
/// never had it asks for once, however often it draws.
#[gpui::test]
fn attached_images_go_as_images_and_are_fetched_once(cx: &mut TestAppContext) {
    use tau_ui_remote::{update::HostUpdate, view::Picture};
    let dir = tempfile::tempdir().unwrap();
    let shot = png_file(dir.path(), 4);
    let (workspace, mut cx, events) = open(cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::NewRun, cx);
        ws.attach_path(&shot, cx);
        assert_eq!(ws.attachments().len(), 1);
        ws.submit_prompt("what is wrong here?".into(), cx);
        assert!(ws.attachments().is_empty());
    });
    let Some(WorkspaceEvent::NewRun { prompt, images, .. }) =
        events.borrow().last().cloned()
    else {
        panic!("a new run")
    };
    assert_eq!(
        prompt, "what is wrong here?",
        "no attached tag for an image"
    );
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].mime_type, "image/png");
    let sent = Picture::of(&images[0]);

    // The host's view of the new run shows it: drawn from what the
    // window kept.
    let run = tau_agent::tool::RunId("with-shot".into());
    let mut view = tau_ui_remote::view::RunView::new(
        run.clone(),
        "with-shot",
        "coder",
        "gpt-5.5",
    )
    .in_repo("tau-agent");
    view.push_user("what is wrong here?");
    view.push_pictures(vec![sent.clone()]);
    workspace.update(&mut cx, |ws, cx| {
        ws.apply(HostUpdate::Run(Box::new(view)), cx);
        ws.navigate(Route::Run(run.clone()), cx);
    });
    cx.run_until_parked();
    assert_eq!(fetched(&events), 0, "the sender had it");

    // Another's picture, steered in: asked for once.
    let theirs = Picture {
        key: "ab".repeat(32),
        mime: "image/png".into(),
    };
    workspace.update(&mut cx, |ws, cx| {
        ws.apply(
            HostUpdate::Pictures {
                run: run.clone(),
                pictures: vec![theirs.clone()],
            },
            cx,
        )
    });
    cx.run_until_parked();
    workspace.update(&mut cx, |_, cx| cx.notify());
    cx.run_until_parked();
    assert_eq!(fetched(&events), 1);
    let Some(WorkspaceEvent::Picture {
        run: asked,
        picture,
    }) = events
        .borrow()
        .iter()
        .rev()
        .find(|event| matches!(event, WorkspaceEvent::Picture { .. }))
        .cloned()
    else {
        unreachable!()
    };
    assert_eq!((asked, picture), (run.clone(), theirs.clone()));
    // Its bytes came: no more asking.
    workspace.update(&mut cx, |ws, cx| {
        ws.apply(
            HostUpdate::Picture {
                picture: theirs,
                data: Some(images[0].data.clone()),
                error: None,
            },
            cx,
        )
    });
    cx.run_until_parked();
    assert_eq!(fetched(&events), 1);
}

fn composer_slash(
    workspace: &Entity<Workspace>,
    cx: &mut VisualTestContext,
    text: &str,
) -> tau_ui_remote::slash::Slash {
    workspace.update(cx, |ws, cx| {
        ws.set_composer(text, cx);
        ws.composer_slash(cx)
    })
}

fn names(slash: tau_ui_remote::slash::Slash) -> Vec<String> {
    match slash {
        tau_ui_remote::slash::Slash::Menu(commands) => {
            commands.into_iter().map(|command| command.name).collect()
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
    // The demo's skills and those tau ships come with the plugins'
    // commands, by name, but not the folder that is not a skill, nor
    // the principles.
    let mut skills: Vec<String> =
        ["code-review", "release-notes", "tau-plugins"]
            .into_iter()
            .chain(tau_skills::shipped::SKILLS.iter().map(|(name, _)| *name))
            .filter(|name| !name.starts_with(tau_skills::PRINCIPLE))
            .map(|name| format!("/{name}"))
            .collect();
    skills.sort();
    let listed = skills.len();
    let mut expected: Vec<String> =
        ["/goal", "/plugin", "/agent"].map(String::from).to_vec();
    expected.extend(skills);
    expected.extend(["/model", "/attach"].map(String::from));
    assert_eq!(names(composer_slash(&workspace, &mut cx, "/")), expected);
    // Only the main chat forks (ADR 0016).
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Run(demo::run_id()), cx)
    });
    assert_eq!(names(composer_slash(&workspace, &mut cx, "/fo")), ["/fork"]);
    workspace
        .update(&mut cx, |ws, cx| ws.navigate(Route::Run(done.clone()), cx));
    assert_eq!(
        names(composer_slash(&workspace, &mut cx, "/")).len(),
        7 + listed
    );
    assert!(names(composer_slash(&workspace, &mut cx, "/fo")).is_empty());
    // Not a command: a message.
    assert_eq!(
        composer_slash(&workspace, &mut cx, "/usr/bin is slow"),
        tau_ui_remote::slash::Slash::None
    );
    assert_eq!(
        composer_slash(&workspace, &mut cx, "/nope"),
        tau_ui_remote::slash::Slash::None
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
        assert_eq!(ws.composer_slash(cx), tau_ui_remote::slash::Slash::None);
        ws.set_composer("/g", cx);
        assert_ne!(ws.composer_slash(cx), tau_ui_remote::slash::Slash::None);
        // Enter on a command being typed runs it.
        ws.submit_prompt("/cl".into(), cx);
    });
    workspace.update(&mut cx, |ws, cx| {
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
            WorkspaceEvent::Say { .. } | WorkspaceEvent::NewRun { .. }
        )),
        "no command was sent as a message: {events:?}"
    );
}

/// tau-goal's state in `run`, as its fold leaves it.
fn goal_of(
    ws: &Workspace,
    run: &tau_agent::tool::RunId,
) -> tau_goal::ui::State {
    ws.run(run)
        .unwrap()
        .plugin_states
        .get(tau_goal::NAME)
        .map(|state| serde_json::from_value(state.json().clone()).unwrap())
        .unwrap_or_default()
}

/// The records tau-goal's UI stored, by run.
fn goal_records(events: &[WorkspaceEvent]) -> Vec<(String, tau_goal::Record)> {
    events
        .iter()
        .filter_map(|event| match event {
            WorkspaceEvent::PluginRecord { run, plugin, body }
                if plugin == tau_goal::NAME =>
            {
                Some((run.0.to_string(), tau_goal::Record::parse(body)?))
            }
            _ => None,
        })
        .collect()
}

/// Whether tau-goal checks `run` now, as the host says when it goes on.
fn goal_checks(
    run: &tau_agent::tool::RunId,
    checks: bool,
) -> tau_ui_remote::update::HostUpdate {
    tau_ui_remote::update::HostUpdate::PluginFold {
        run: run.clone(),
        plugin: tau_goal::NAME.into(),
        body: serde_json::json!({ "kind": "starting", "checks": checks }),
    }
}

/// `/plugin` starts a chat in tau's plugins repository that loads the
/// skill for writing one, wherever it is written; alone, it waits to be
/// written.
#[gpui::test]
fn slash_plugin_starts_a_chat_in_the_plugins_repository(
    cx: &mut TestAppContext,
) {
    let (workspace, mut cx, events) = open_demo(cx);
    let done = tau_agent::tool::RunId("plugin-docs".into());
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Run(done.clone()), cx);
        ws.submit_prompt("/plugin".into(), cx);
    });
    workspace.update(&mut cx, |ws, cx| {
        assert_eq!(ws.composer_text(cx), "/plugin ");
        ws.set_composer("", cx);
        ws.submit_prompt("/plugin block deploys on Fridays".into(), cx);
    });
    cx.run_until_parked();
    assert!(
        matches!(
            events.borrow().last(),
            Some(WorkspaceEvent::NewRun { repo, prompt, .. })
                if repo == "tau-plugins"
                    && prompt == "/tau-plugins block deploys on Fridays"
        ),
        "{:?}",
        events.borrow().last()
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
    });
    workspace.update(&mut cx, |ws, cx| {
        assert_eq!(ws.composer_text(cx), "/goal ");
        ws.set_composer("", cx);
        ws.submit_prompt(
            "/goal Every lane has an owner in lanes.toml".into(),
            cx,
        );
    });
    workspace.update(&mut cx, |ws, cx| {
        // The host says the run went on with tau-goal.
        ws.apply(goal_checks(&done, true), cx);
        // Going on now: a new goal is stored for its next stop, and the
        // model is told. Typed limits win over the popover's.
        ws.set_composer("", cx);
        ws.submit_prompt("/goal --continuations 3 it ships".into(), cx);
    });
    cx.run_until_parked();
    {
        let events = events.borrow();
        let prompts: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                WorkspaceEvent::Say { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        // The first goal went on with the run; the new one is told to
        // the model, which reads it as its next turn starts.
        assert_eq!(
            prompts,
            [
                "/goal --continuations 10 --budget 2.00 Every lane has an \
                 owner in lanes.toml",
                &tau_goal::set_input("it ships"),
            ]
        );
        assert_eq!(
            goal_records(&events),
            [(
                "plugin-docs".to_owned(),
                tau_goal::Record::Set {
                    goal: "it ships".into(),
                    continuations: 3,
                    budget: 2.0,
                }
            )]
        );
    }
    events.borrow_mut().clear();

    // Going on without tau-goal (started before the key): the goal is
    // kept for the next message, and the model is not told it is
    // checked now.
    workspace.update(&mut cx, |ws, cx| {
        ws.apply(goal_checks(&done, false), cx);
        ws.set_composer("", cx);
        ws.submit_prompt("/goal it ships later".into(), cx);
    });
    workspace.update(&mut cx, |ws, cx| {
        assert!(ws.alert().is_some());
        let goal = goal_of(ws, &done).goal.unwrap();
        assert_eq!(goal.condition, "it ships later");
        ws.dismiss_alert(cx);
    });
    {
        let events = events.borrow();
        assert!(goal_records(&events).iter().any(|(_, record)| matches!(
            record,
            tau_goal::Record::Set { goal, .. } if goal == "it ships later"
        )));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, WorkspaceEvent::Say { .. })),
            "the model is not told"
        );
    }
    events.borrow_mut().clear();

    // Without a TypeSafe key, the goal is not sent: nothing would check it.
    workspace.update(&mut cx, |ws, cx| {
        let mut catalog = demo::catalog();
        catalog.models.access.jev = false;
        ws.set_catalog(catalog, cx);
        ws.navigate(Route::NewRun, cx);
        ws.submit_prompt("/goal it ships".into(), cx);
    });
    workspace.update(&mut cx, |ws, cx| {
        assert!(ws.alert().is_some());
        assert_eq!(ws.composer_text(cx), "/goal it ships");
    });
    assert!(events.borrow().is_empty(), "{:?}", events.borrow());
}

/// Carries out a goal button's `act` on `run`, as a click does.
fn goal_act(
    workspace: &Entity<Workspace>,
    cx: &mut VisualTestContext,
    run: &tau_agent::tool::RunId,
    act: tau_goal::ui::Act,
) {
    workspace.update(cx, |ws, cx| {
        let goal = goal_of(ws, run).goal.unwrap();
        let info = ws.run(run).unwrap().info();
        let jev = ws.catalog().models.access.jev;
        let handle = ws.plugin_handle(tau_goal::NAME);
        tau_goal::ui::act(act, &info, &goal, jev, &handle, cx);
    });
    cx.run_until_parked();
}

#[gpui::test]
fn goal_buttons_store_changes_and_keep_going(cx: &mut TestAppContext) {
    use tau_goal::{Record, Status, ui::Act};
    let (workspace, mut cx, events) = open_demo(cx);
    let stopped = tau_agent::tool::RunId("lane-audit".into());
    let met = tau_agent::tool::RunId("mutants-triage".into());
    workspace.update(&mut cx, |ws, cx| {
        assert!(goal_of(ws, &stopped).goal.is_some());
        let done = tau_agent::tool::RunId("plugin-docs".into());
        assert!(goal_of(ws, &done).goal.is_none());
        ws.navigate(Route::Run(stopped.clone()), cx);
    });
    // Keep going: more continuations, and the conversation goes on.
    goal_act(&workspace, &mut cx, &stopped, Act::KeepGoing);
    workspace.update(&mut cx, |ws, _| {
        let goal = goal_of(ws, &stopped).goal.unwrap();
        assert_eq!(goal.status, Status::Active);
        assert_eq!(goal.max_continuations, 12);
    });
    // Edit puts the goal back in the composer.
    goal_act(&workspace, &mut cx, &stopped, Act::Edit);
    workspace.update(&mut cx, |ws, cx| {
        assert_eq!(
            ws.composer_text(cx),
            "/goal Every lane has an owner in lanes.toml"
        );
    });
    goal_act(&workspace, &mut cx, &stopped, Act::Pause);
    goal_act(&workspace, &mut cx, &met, Act::Clear);
    workspace.update(&mut cx, |ws, _| {
        assert_eq!(goal_of(ws, &stopped).goal.unwrap().status, Status::Paused);
        assert!(goal_of(ws, &met).goal.is_none());
    });
    let events = events.borrow();
    assert_eq!(
        goal_records(&events),
        [
            ("lane-audit".to_owned(), Record::Extended { by: 10 }),
            ("lane-audit".to_owned(), Record::Paused),
            ("mutants-triage".to_owned(), Record::Cleared),
        ]
    );
    assert!(events.iter().any(|event| matches!(event,
        WorkspaceEvent::Say { run, text, .. }
            if run == &stopped && text == tau_goal::ui::KEEP_GOING)));
}

/// Without a TypeSafe key a goal is not checked: the banner says so,
/// Keep going says why it does not go on, and a change the host could
/// not save is put back as stored.
#[gpui::test]
fn an_unchecked_goal_says_so(cx: &mut TestAppContext) {
    use tau_goal::{Record, Status, ui::Act};
    let (workspace, mut cx, events) = open_demo(cx);
    let stopped = tau_agent::tool::RunId("lane-audit".into());
    let set = || {
        vec![
            serde_json::to_value(Record::Set {
                goal: "Every lane has an owner in lanes.toml".into(),
                continuations: 10,
                budget: 2.0,
            })
            .unwrap(),
        ]
    };
    workspace.update(&mut cx, |ws, cx| {
        let mut catalog = ws.catalog().clone();
        catalog.models.access.jev = false;
        ws.set_catalog(catalog, cx);
        ws.navigate(Route::Run(stopped.clone()), cx);
        // A goal still to be met, then paused.
        ws.apply(
            tau_ui_remote::update::HostUpdate::PluginRestate {
                run: stopped.clone(),
                plugin: tau_goal::NAME.into(),
                records: set(),
            },
            cx,
        );
    });
    goal_act(&workspace, &mut cx, &stopped, Act::Pause);
    workspace.update(&mut cx, |ws, _| {
        let goal = goal_of(ws, &stopped).goal.unwrap();
        assert_eq!(goal.status, Status::Paused);
        let live = ws.run(&stopped).unwrap().info().live;
        assert!(tau_goal::ui::unchecked(&goal, false, live, false).is_some());
        assert!(tau_goal::ui::unchecked(&goal, true, live, false).is_none());
    });
    goal_act(&workspace, &mut cx, &stopped, Act::KeepGoing);
    workspace.update(&mut cx, |ws, _| {
        assert!(ws.alert().is_some(), "says it needs a key");
    });
    assert!(
        !events
            .borrow()
            .iter()
            .any(|event| matches!(event, WorkspaceEvent::Say { .. })),
        "nothing goes on unchecked"
    );
    // The pause could not be saved: the host puts back what is stored.
    workspace.update(&mut cx, |ws, cx| {
        ws.apply(
            tau_ui_remote::update::HostUpdate::PluginRestate {
                run: stopped.clone(),
                plugin: tau_goal::NAME.into(),
                records: set(),
            },
            cx,
        );
        assert_eq!(goal_of(ws, &stopped).goal.unwrap().status, Status::Active);
    });
}

/// What the person opens and picks in a transcript, against a model:
/// a note or a folding card opens and closes with each click, alone of
/// its run's; a log card's change is picked by a click and put back by
/// a second click on it, and a click on another moves the pick.
#[gpui::test]
fn notes_cards_and_picks_follow_the_clicks(cx: &mut TestAppContext) {
    use std::collections::{HashMap, HashSet};

    use hegel::generators as gs;

    #[derive(Debug, Clone)]
    enum Click {
        Note(usize, usize),
        Card(usize, usize),
        Pick(usize, usize, usize),
    }
    hegel::pretty_print_as_debug!(Click);

    const KEYS: [&str; 2] = ["log", "other"];
    const CHANGES: [&str; 2] = ["qpvuntsm", "rlvkpnrz"];
    let runs = [demo::run_id(), demo::fork_id()];
    let (workspace, mut cx, _) = open_demo(cx);
    let cards = workspace.read_with(&cx, |ws, _| {
        ws.plugin_ui::<tau_vcs::ui::Ui>(tau_vcs::ui::NAME)
            .expect("tau-vcs draws its cards")
    });
    // Everything the clicks can reach, as the workspace has it now.
    let observe = |cx: &mut VisualTestContext| {
        let (notes, open) = workspace.read_with(cx, |ws, _| {
            let mut notes = HashSet::new();
            let mut open = HashSet::new();
            for (r, run) in runs.iter().enumerate() {
                for index in 0..3 {
                    if ws.note_open(run, index) {
                        notes.insert((r, index));
                    }
                }
                for (k, key) in KEYS.iter().enumerate() {
                    if ws.card_open(run, key) {
                        open.insert((r, k));
                    }
                }
            }
            (notes, open)
        });
        let picks = cards.read_with(cx, |cards, _| {
            let mut picks = HashMap::new();
            for (r, run) in runs.iter().enumerate() {
                for (k, key) in KEYS.iter().enumerate() {
                    if let Some(change) = cards.picked(run, key) {
                        picks.insert((r, k), change.to_owned());
                    }
                }
            }
            picks
        });
        (notes, open, picks)
    };
    hegel::Hegel::new(|tc: hegel::TestCase| {
        let click = hegel::compose!(|tc| {
            let run = tc.draw(gs::integers::<usize>().max_value(1));
            let key = tc.draw(gs::integers::<usize>().max_value(1));
            match tc.draw(gs::integers::<u8>().max_value(2)) {
                0 => Click::Note(
                    run,
                    tc.draw(gs::integers::<usize>().max_value(2)),
                ),
                1 => Click::Card(run, key),
                _ => Click::Pick(
                    run,
                    key,
                    tc.draw(gs::integers::<usize>().max_value(1)),
                ),
            }
        });
        let clicks: Vec<Click> = tc.draw(gs::vecs(click).max_size(12));
        let (mut notes, mut open, mut picks) = observe(&mut cx);
        for click in &clicks {
            match *click {
                Click::Note(r, index) => {
                    workspace.update(&mut cx, |ws, cx| {
                        ws.toggle_note(&runs[r], index, cx)
                    });
                    if !notes.remove(&(r, index)) {
                        notes.insert((r, index));
                    }
                }
                Click::Card(r, k) => {
                    workspace.update(&mut cx, |ws, cx| {
                        ws.toggle_card(&runs[r], KEYS[k], cx)
                    });
                    if !open.remove(&(r, k)) {
                        open.insert((r, k));
                    }
                }
                Click::Pick(r, k, c) => {
                    cards.update(&mut cx, |cards, _| {
                        cards.pick(&runs[r], KEYS[k], CHANGES[c])
                    });
                    if picks.get(&(r, k)).map(String::as_str)
                        == Some(CHANGES[c])
                    {
                        picks.remove(&(r, k));
                    } else {
                        picks.insert((r, k), CHANGES[c].to_owned());
                    }
                }
            }
            assert_eq!(
                observe(&mut cx),
                (notes.clone(), open.clone(), picks.clone()),
                "{clicks:?}"
            );
        }
    })
    .settings(hegel::Settings::new().test_cases(100))
    .run();
}

/// A fork lands from the compare screen: a preview first, then the
/// landing, which leaves a card in the parent's chat, closes the fork,
/// and opens the parent.
#[gpui::test]
fn a_fork_lands_on_its_parent(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open_demo(cx);
    let fork = demo::fork_id();
    let parent = demo::run_id();
    let change = serde_json::from_value::<tau_vcs_host::ChangeInfo>(serde_json::json!({
        "change_id": "qlmxnpvoqlmxnpvoqlmxnpvoqlmxnpvo",
        "commit_id": "0123456789abcdef0123456789abcdef01234567",
        "description": "feat(tau-ai): cap the backoff at the policy's max\n",
        "empty": false, "conflict": false, "immutable": false,
        "working_copy": false, "divergent": false, "bookmarks": [],
    }))
    .unwrap();
    let landing = tau_vcs_host::Landing {
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
    let parent = demo::run_id();
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
/// parent's `spawn` handed it. It stays open after it finishes, until
/// main's queue lands it; one that fails is dropped and closes at
/// once. A sibling from the same batch keeps its own task.
/// Both stay in the sidebar under their parent, newest first, dim.
#[gpui::test]
fn a_sub_agent_is_a_chat_until_it_lands(cx: &mut TestAppContext) {
    use std::sync::Arc;

    use tau_agent::{event::RunEvent, tool::RunId};

    let (workspace, mut cx, events) = open_demo(cx);
    let parent = demo::run_id();
    let child = RunId("sub-1".into());
    let sibling = RunId("sub-2".into());
    workspace.update(&mut cx, |ws, cx| {
        for (call, task) in [("d1", "write the tests"), ("d2", "write the docs")] {
            ws.apply_event(
                &RunEvent::ToolStart {
                    run: parent.clone(),
                    call_id: call.into(),
                    tool: Arc::from("spawn"),
                    args: serde_json::json!({ "task": task }),
                    parent: None,
                },
                cx,
            );
        }
        // The sibling starts first: each is paired by its call.
        for (run, call) in [(&sibling, "d2"), (&child, "d1")] {
            ws.apply_event(
                &RunEvent::RunStart {
                    run: run.clone(),
                    parent: Some(parent.clone()),
                    agent: Arc::from("coder"),
                    call: Some(call.into()),
                },
                cx,
            );
        }
        let view = ws.run(&sibling).expect("a chat for the sibling");
        assert!(matches!(view.items.first(), Some(Item::User(task)) if task == "write the docs"));
        let view = ws.run(&child).expect("a chat for the sub-agent");
        assert_eq!(view.origin, tau_ui_remote::view::Origin::SubAgent { parent: parent.clone() });
        assert!(matches!(view.items.first(), Some(Item::User(task)) if task == "write the tests"));
        // `spawn` answers at once: the chats stay.
        for call in ["d1", "d2"] {
            ws.apply_event(&calls::end(&parent, call, false, Some(serde_json::json!({ "run": call })), None), cx);
        }
        assert!(!ws.is_closed(&child), "open while it works");
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
        assert!(!ws.is_closed(&child), "open until its work lands");
        // The sibling fails: it is dropped, and closes.
        ws.apply_event(
            &RunEvent::RunEnd {
                run: sibling.clone(),
                parent: Some(parent.clone()),
                stop: StopReason::Error("no".into()),
                cost: 0.0,
            },
            cx,
        );
        assert!(ws.is_closed(&sibling), "a failed sub-agent is dropped");
        assert_eq!(ws.route(), &Route::Run(child.clone()), "nothing moved");
        // Main's queue lands it.
        ws.apply(
            tau_ui_remote::update::HostUpdate::Landed {
                run: child.clone(),
                landing: Ok(tau_vcs::Landing {
                    changes: Vec::new(),
                    conflicts: Vec::new(),
                    head: "00".into(),
                }),
            },
            cx,
        );
        assert!(ws.is_closed(&child));
        assert!(ws.run(&parent).unwrap().items.iter().any(|item| matches!(item, Item::Landed(card) if card.from == child)));
        let card = ws.run(&parent).unwrap().tool("d1").unwrap();
        assert_eq!(tau_vcs::ui::spawned(&card.data), Some(RunId("d1".into())));
        // The host takes the closings. The sidebar never lists
        // sub-agents: they are main's crew, in its chat's tray.
        for run in [&child, &sibling] {
            ws.apply(tau_ui_remote::update::HostUpdate::Closed(run.clone()), cx);
        }
        let view = ws.run(&parent).unwrap();
        assert!(!ws.listed_children(view).any(|view| matches!(view.origin, tau_ui_remote::view::Origin::SubAgent { .. })));
        for run in [&child, &sibling] {
            assert!(ws.has_ended(ws.run(run).unwrap()), "{run} ended");
        }
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
    });
    host_resumes(&workspace, &mut cx, &events);
    workspace.update(&mut cx, |ws, _| {
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
        Some(&WorkspaceEvent::Say {
            run: run.clone(),
            text: "go on".into(),
            model: ModelChoice::new("gpt-6-luna", Effort::Auto),
            images: Vec::new(),
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

/// A run stopped on the plan's usage limit says so, with "Manage usage"
/// first; a temporary refusal, already retried, says nothing more.
#[gpui::test]
fn a_usage_limit_offers_to_manage_usage(cx: &mut TestAppContext) {
    use tau_ui_remote::plan_usage::{PlanAction, PlanAlert};
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
        Some(tau_ui_remote::models::USAGE_SETTINGS_URL)
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
    use tau_ui_remote::plan_usage::{PlanAction, PlanAlert};
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

fn computer() -> tau_ui_remote::pairing::Computer {
    demo::computer()
}

#[gpui::test]
fn scanning_pairs_then_opens_the_runs(cx: &mut TestAppContext) {
    use tau_ui_remote::pairing::{
        PairRequest,
        PairStep,
        PairingUpdate,
        Progress,
    };

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
    use tau_ui_remote::pairing::{
        PairRequest,
        PairStep,
        PairingUpdate,
        Progress,
    };

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
    use tau_ui_remote::pairing::{PairRequest, PairStep, Progress};

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
    use tau_ui_remote::pairing::{PairRequest, PairStep, PairingUpdate};

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
fn a_mirrored_workspace_echoes_what_the_host_applies(cx: &mut TestAppContext) {
    use tau_ui_remote::update::HostUpdate;

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

/// A repository's main chat heads its tree and stays open; the chats
/// forked from it go under it, newest first, the older ones behind
/// "Show older runs".
#[gpui::test]
fn the_main_chat_heads_its_repository(cx: &mut TestAppContext) {
    use tau_agent::tool::RunId;
    use tau_ui_remote::view::{Origin, RunView};

    let (workspace, mut cx, events) = open(cx);
    let main = RunId("main".into());
    workspace.update(&mut cx, |ws, cx| {
        let mut catalog = Catalog::default();
        let mut repo = tau_ui_remote::catalog::Repo::new("hello", "");
        repo.main = Some(main.clone());
        catalog.repos.push(repo);
        ws.set_catalog(catalog, cx);
        // History comes newest first: the chats, then main. The even
        // chats are done; the odd ones and main still go.
        let mut runs: Vec<RunView> = (0..7)
            .rev()
            .map(|n| {
                let mut chat = RunView::new(
                    RunId(format!("chat-{n}").into()),
                    format!("chat {n}"),
                    "coder",
                    "gpt-5.5",
                )
                .in_repo("hello")
                .with_origin(Origin::Fork {
                    from: main.clone(),
                    turn: 0,
                });
                if n % 2 == 0 {
                    chat.finish_stored(StopReason::Stop, 0.0, 0.0);
                }
                chat
            })
            .collect();
        runs.push(
            RunView::new(main.clone(), "main", "coder", "gpt-5.5")
                .in_repo("hello"),
        );
        ws.add_history(runs, cx);
    });
    workspace.update(&mut cx, |ws, cx| {
        assert!(ws.is_main(&main));
        let rows = ws.repo_rows("");
        let rows = &rows[0];
        assert_eq!(rows.runs.len(), 1);
        assert_eq!(rows.runs[0].id, main);
        assert_eq!(
            (rows.main_children, rows.older, rows.total),
            (Some(5), 2, 8)
        );
        assert_eq!(rows.live, 4, "main and the three chats still going");
        let chats: Vec<&str> = ws
            .listed_children(rows.runs[0])
            .map(|run| run.title.as_str())
            .collect();
        assert_eq!(chats[0], "chat 6", "newest first");
        ws.show_older_runs("hello", cx);
        let rows = ws.repo_rows("");
        assert_eq!((rows[0].main_children, rows[0].older), (None, 0));
        // A filter finds chats under main, listed alone.
        let rows = ws.repo_rows("chat 3");
        assert!(rows[0].flat);
        let titles: Vec<&str> =
            rows[0].runs.iter().map(|run| run.title.as_str()).collect();
        assert_eq!(titles, ["chat 3"]);

        // Main cannot be closed; its chats can.
        ws.close_run(&main, cx);
        assert!(!ws.is_closed(&main));
        ws.close_run(&RunId("chat-6".into()), cx);
        // The host takes it.
        ws.apply(
            tau_ui_remote::update::HostUpdate::Closed(RunId("chat-6".into())),
            cx,
        );
        assert_eq!(ws.listed_children(ws.run(&main).unwrap()).count(), 6);
    });
    let closed: Vec<RunId> = events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            WorkspaceEvent::CloseRun { run } => Some(run.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(closed, [RunId("chat-6".into())]);

    // A repository's row opens and closes with each click, which
    // selects it, and the host is told which are open.
    let was = workspace.read_with(&cx, |ws, _| ws.is_repo_open("hello"));
    workspace.update(&mut cx, |ws, cx| {
        ws.toggle_repo_open("hello", cx);
        assert_eq!(ws.is_repo_open("hello"), !was);
        assert_eq!(ws.selected_repo(), Some("hello"));
        ws.toggle_repo_open("hello", cx);
        assert_eq!(ws.is_repo_open("hello"), was);
    });
    let told: Vec<bool> = events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            WorkspaceEvent::OpenRepos(open) => {
                Some(open.iter().any(|repo| repo == "hello"))
            }
            _ => None,
        })
        .collect();
    assert_eq!(told[told.len() - 2..], [!was, was]);
}

/// A repository added after tau started (cloned during onboarding, or
/// from the sidebar) comes with its main chat, and the sidebar lists
/// that chat first under it, before any run has touched it.
#[gpui::test]
fn a_new_repository_lists_its_main_chat(cx: &mut TestAppContext) {
    use tau_agent::tool::RunId;
    use tau_ui_remote::{catalog::Repo, update::HostUpdate, view::RunView};

    let (workspace, mut cx, _) = open(cx);
    let main = RunId("main-of-hello".into());
    let mut repo = Repo::new("hello", "/repos/hello");
    repo.main = Some(main.clone());
    let view =
        RunView::new(main.clone(), "main", "coder", "gpt-5.5").in_repo("hello");
    workspace.update(&mut cx, |ws, cx| {
        ws.apply(
            HostUpdate::Repo {
                repo,
                main: Some(Box::new(view)),
            },
            cx,
        );
        let rows = ws.repo_rows("");
        let hello = rows
            .iter()
            .find(|rows| rows.repo.name == "hello")
            .expect("the repository is listed");
        assert!(hello.open, "a new repository opens");
        assert_eq!(
            hello.runs.first().map(|run| run.id.clone()),
            Some(main.clone()),
            "its main chat comes first"
        );
    });
}

/// The effort picked for a chat's next message stays picked after the
/// message is sent, even on a chat read back from the store (or a main
/// chat that has never run), whose plan is empty.
#[gpui::test]
fn a_picked_effort_stays_after_sending(cx: &mut TestAppContext) {
    use tau_agent::tool::RunId;
    use tau_ui_remote::{
        models::Effort,
        view::{RunStatus, RunView},
    };

    let (workspace, mut cx, events) = open(cx);
    let id = RunId("stored".into());
    let mut stored = RunView::new(id.clone(), "stored", "coder", "gpt-5.5");
    stored.status = RunStatus::Finished(StopReason::Stop);
    assert!(stored.plan.is_empty(), "read back without a plan");
    workspace.update_in(&mut cx, |ws, window, cx| {
        ws.add_history(vec![stored], cx);
        ws.navigate(Route::Run(id.clone()), cx);
        ws.open_picker(PickerTarget::Run(id.clone()), window, cx);
        ws.pick_effort(Effort::High, cx);
        ws.submit_prompt("go on".into(), cx);
    });
    assert!(matches!(
        events.borrow().last(),
        Some(WorkspaceEvent::Say { model, .. }) if model.effort == Effort::High
    ));
    host_resumes(&workspace, &mut cx, &events);
    workspace.read_with(&cx, |ws, _| {
        let choice = ws.choice_for(&PickerTarget::Run(id.clone()));
        assert_eq!(choice.effort, Effort::High, "still high after sending");
        assert_eq!(choice.model, "gpt-5.5");
    });
}

/// Runs nest one level: a fork (a chat under the main chat) offers no
/// fork of its own, neither from the header nor with `/fork`.
#[gpui::test]
fn a_chat_under_main_offers_no_fork(cx: &mut TestAppContext) {
    use tau_agent::tool::RunId;
    use tau_ui_remote::view::{Origin, RunStatus, RunView};

    let (workspace, mut cx, _) = open(cx);
    let chat = RunId("chat".into());
    let mut view = RunView::new(chat.clone(), "chat", "coder", "gpt-5.5")
        .with_origin(Origin::Fork {
            from: demo::run_id(),
            turn: 1,
        });
    view.status = RunStatus::Finished(StopReason::Stop);
    workspace.update(&mut cx, |ws, cx| {
        ws.add_history(vec![view], cx);
        ws.navigate(Route::Run(chat.clone()), cx);
        let run = ws.run(&chat).unwrap();
        assert!(!ws.can_fork(run));
        assert!(!ws.can_fork_at(run, 1));
        assert!(!ws.start_fork_at(&chat, 1, cx), "no fork mode");
        assert!(matches!(ws.slash("/fo"), tau_ui_remote::slash::Slash::None));
    });
}

/// Events of a run's tool calls, as a host feeds them in.
mod calls {
    use std::sync::Arc;

    use serde_json::Value;
    use tau_agent::{
        event::{RunEvent, StopReason},
        tool::{RunId, ToolOutput},
    };

    pub fn start(
        run: &RunId,
        id: &str,
        tool: &str,
        args: Value,
        parent: Option<&str>,
    ) -> RunEvent {
        RunEvent::ToolStart {
            run: run.clone(),
            call_id: id.into(),
            tool: Arc::from(tool),
            args,
            parent: parent.map(str::to_owned),
        }
    }

    pub fn end(
        run: &RunId,
        id: &str,
        error: bool,
        details: Option<Value>,
        parent: Option<&str>,
    ) -> RunEvent {
        RunEvent::ToolEnd {
            run: run.clone(),
            call_id: id.into(),
            output: Arc::new(ToolOutput {
                details,
                ..ToolOutput::text(if error { "no" } else { "ok" })
            }),
            is_error: error,
            parent: parent.map(str::to_owned),
        }
    }

    pub fn run_end(run: &RunId, parent: Option<&RunId>) -> RunEvent {
        RunEvent::RunEnd {
            run: run.clone(),
            parent: parent.cloned(),
            stop: StopReason::Stop,
            cost: 0.0,
        }
    }
}

/// One `vcs_land` a run makes: from the model, or from inside a call
/// the model made, `depth` levels down.
#[derive(Debug, Clone)]
struct Land {
    depth: usize,
    ok: bool,
    /// The nested calls' events reached the workspace. Without them, all
    /// it has is the outermost call's result: its `details.calls`, which
    /// list that call's own calls (codemode's), as a stored run has.
    seen: bool,
    /// The outermost call failed anyway, as a script can after its
    /// `vcs_land` went through.
    outer_fails: bool,
}
hegel::pretty_print_as_debug!(Land);

impl Land {
    /// Whether the workspace can know it proposed.
    fn proposes(&self) -> bool {
        self.ok && (self.depth <= 1 || self.seen)
    }

    /// Its events, its calls numbered `n`.
    fn events(
        &self,
        run: &tau_agent::tool::RunId,
        n: usize,
    ) -> Vec<tau_agent::event::RunEvent> {
        use serde_json::json;
        let top = format!("c{n}");
        if self.depth == 0 {
            return vec![
                calls::start(run, &top, "vcs_land", json!({}), None),
                calls::end(run, &top, !self.ok, None, None),
            ];
        }
        // codemode → other → … → vcs_land.
        let ids: Vec<String> = (0..=self.depth)
            .map(|d| {
                std::iter::once(top.clone())
                    .chain((0..d).map(|_| "1".to_owned()))
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .collect();
        let tool = |d: usize| match d {
            0 => "codemode",
            d if d == self.depth => "vcs_land",
            _ => "other",
        };
        let mut events = vec![calls::start(
            run,
            &top,
            "codemode",
            json!({ "code": "" }),
            None,
        )];
        if self.seen {
            for d in 1..=self.depth {
                events.push(calls::start(
                    run,
                    &ids[d],
                    tool(d),
                    json!({}),
                    Some(&ids[d - 1]),
                ));
            }
            for d in (1..=self.depth).rev() {
                let error = d == self.depth && !self.ok;
                events.push(calls::end(
                    run,
                    &ids[d],
                    error,
                    None,
                    Some(&ids[d - 1]),
                ));
            }
        }
        let own_ok = if self.depth == 1 { self.ok } else { true };
        let details = json!({
            "calls": [{
                "id": ids[1], "name": tool(1), "args": "{}",
                "status": if own_ok { "ok" } else { "error" },
                "ms": 1, "error": null, "cost": null,
            }],
            "complete": true,
        });
        events.push(calls::end(
            run,
            &top,
            self.outer_fails,
            Some(details),
            None,
        ));
        events
    }
}

/// Whether a run's `vcs_land` comes from the model or from a call it
/// made, at any depth, and whether the workspace saw the nested calls'
/// events or only the outermost call's result (as a stored run keeps
/// it), the run's landing card opens once when it stops: once however
/// many times it proposed, and never when no landing went through.
#[gpui::test]
fn a_landing_is_proposed_once_from_any_depth(cx: &mut TestAppContext) {
    use hegel::generators as gs;
    use tau_agent::tool::RunId;
    use tau_ui_remote::view::RunView;

    let (workspace, mut cx, events) = open_demo(cx);
    let mut case = 0;
    hegel::Hegel::new(|tc: hegel::TestCase| {
        case += 1;
        let lands: Vec<Land> = tc.draw(
            gs::vecs(hegel::compose!(|tc| {
                Land {
                    depth: tc.draw(gs::integers::<usize>().max_value(3)),
                    ok: tc.draw(gs::booleans()),
                    seen: tc.draw(gs::booleans()),
                    outer_fails: tc.draw(gs::booleans()),
                }
            }))
            .max_size(3),
        );
        let run = RunId(format!("proposes-{case}").into());
        events.borrow_mut().clear();
        workspace.update(&mut cx, |ws, cx| {
            ws.push_run(RunView::new(run.clone(), "run", "coder", "m"), cx);
            for (n, land) in lands.iter().enumerate() {
                for event in land.events(&run, n) {
                    ws.apply_event(&event, cx);
                }
            }
            ws.apply_event(&calls::run_end(&run, None), cx);
        });
        let previews = events
            .borrow()
            .iter()
            .filter(|event| {
                matches!(event,
                WorkspaceEvent::PreviewLanding { run: r } if *r == run)
            })
            .count();
        let expected = usize::from(lands.iter().any(Land::proposes));
        assert_eq!(previews, expected, "{lands:?}");
    })
    .settings(hegel::Settings::new().test_cases(100))
    .run();
}

/// A sub-agent a codemode script spawns is a chat of its own on the
/// task the script handed it, as one the model spawns, open until it
/// lands.
#[gpui::test]
fn a_nested_spawn_opens_its_chat(cx: &mut TestAppContext) {
    use std::sync::Arc;

    use serde_json::json;
    use tau_agent::{event::RunEvent, tool::RunId};

    let (workspace, mut cx, events) = open_demo(cx);
    let parent = demo::run_id();
    let child = RunId("sub-nested".into());
    workspace.update(&mut cx, |ws, cx| {
        ws.apply_event(&calls::start(&parent, "s1", "codemode", json!({ "code": "" }), None), cx);
        ws.apply_event(
            &calls::start(&parent, "s1/1", "spawn", json!({ "task": "write the tests" }), Some("s1")),
            cx,
        );
        ws.apply_event(
            &RunEvent::RunStart {
                run: child.clone(),
                parent: Some(parent.clone()),
                agent: Arc::from("coder"),
                call: Some("s1/1".into()),
            },
            cx,
        );
        let view = ws.run(&child).expect("a chat for the sub-agent");
        assert!(matches!(view.items.first(), Some(Item::User(task)) if task == "write the tests"));
        ws.apply_event(&calls::end(&parent, "s1/1", false, None, Some("s1")), cx);
        ws.apply_event(&calls::run_end(&child, Some(&parent)), cx);
        assert!(!ws.is_closed(&child), "open until it lands");
    });
    assert!(!events.borrow().iter().any(|event| matches!(event,
        WorkspaceEvent::CloseRun { run } if *run == child)));
}

/// Every screen the demo opens by name opens on the demo, with its host
/// answering, and draws, on a computer and on a phone: onboarding's and
/// pairing's steps among them.
#[gpui::test]
fn every_demo_screen_opens(cx: &mut TestAppContext) {
    let (workspace, mut cx, _, host) = open_demo_host(cx);
    for phone in [false, true] {
        workspace.update(&mut cx, |ws, cx| {
            ws.set_frame(phone.then_some((390., 844.)), cx)
        });
        for (name, _) in demo::SCREENS {
            workspace.update(&mut cx, |ws, cx| {
                assert!(
                    demo::open(name, ws, &host, cx),
                    "{name}, phone: {phone}"
                );
            });
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
    }
}

/// A chat that landed or was dropped is read-only: a message sent from
/// its screen goes nowhere, neither resuming nor steering it.
#[gpui::test]
fn an_ended_chat_takes_no_messages(cx: &mut TestAppContext) {
    use tau_ui_remote::view::{Ending, Origin, RunView};
    let (workspace, mut cx, events) = open(cx);
    for (id, ending) in [
        (
            "landed-chat",
            Ending::Landed {
                on: demo::run_id(),
                changes: 2,
            },
        ),
        ("dropped-chat", Ending::Dropped),
    ] {
        let run = tau_agent::tool::RunId(id.into());
        let mut view = RunView::new(run.clone(), id, "coder", "gpt-5.5")
            .in_repo("tau-agent")
            .with_origin(Origin::Fork {
                from: demo::run_id(),
                turn: 1,
            });
        view.finish_stored(StopReason::Stop, 0.0, 0.0);
        view.ending = Some(ending);
        workspace.update(&mut cx, |ws, cx| {
            ws.apply(
                tau_ui_remote::update::HostUpdate::History(vec![view]),
                cx,
            );
            ws.navigate(Route::Run(run.clone()), cx);
            ws.submit_prompt("one more thing".into(), cx);
        });
        cx.run_until_parked();
    }
    let sent: Vec<_> = events
        .borrow()
        .iter()
        .filter(|event| matches!(event, WorkspaceEvent::Say { .. }))
        .cloned()
        .collect();
    assert!(sent.is_empty(), "{sent:?}");
}

/// The main chat pushes from its header: one push at a time; GitHub's
/// moved branch leaves the card that offers Fetch and push; a push that
/// went leaves the card of what it took, nothing ahead, and a new run of
/// the main chat puts the card away. Anything else is a dialog. Each
/// state draws, on a computer and on a phone.
#[gpui::test]
fn the_main_chat_pushes_and_offers_fetch_and_push(cx: &mut TestAppContext) {
    use tau_ui_remote::push::{PushFailure, PushState};
    let (workspace, mut cx, events) = open_demo(cx);
    let main = demo::run_id();
    let repo = "tau-agent";
    let draw = |workspace: &Entity<Workspace>, cx: &mut VisualTestContext| {
        for phone in [false, true] {
            workspace.update(cx, |ws, cx| {
                ws.set_frame(phone.then_some((390., 844.)), cx)
            });
            cx.run_until_parked();
        }
    };
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Run(main.clone()), cx);
        assert_eq!(ws.main_repo(&main), Some(repo));
        assert_eq!(ws.main_repo(&demo::fork_id()), None);
        assert_eq!(ws.unpushed(repo), Some((3, "main")));
        ws.push(repo, false, cx);
        ws.push(repo, false, cx);
        assert_eq!(
            ws.push_state(repo),
            Some(&PushState::Pushing { fetching: false })
        );
    });
    draw(&workspace, &mut cx);
    workspace.update(&mut cx, |ws, cx| {
        let moved = PushFailure::Moved {
            branch: "main".into(),
            ahead: 3,
        };
        ws.pushed(repo, Err(moved), cx);
        assert_eq!(
            ws.push_state(repo),
            Some(&PushState::Moved {
                branch: "main".into(),
                ahead: 3
            })
        );
    });
    draw(&workspace, &mut cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.push(repo, true, cx);
        ws.pushed(repo, Ok(demo::pushed()), cx);
        assert_eq!(
            ws.push_state(repo),
            Some(&PushState::Pushed(demo::pushed()))
        );
        assert_eq!(ws.unpushed(repo), None, "nothing ahead");
    });
    draw(&workspace, &mut cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.apply_event(
            &tau_agent::event::RunEvent::RunStart {
                run: main.clone(),
                parent: None,
                agent: "coder".into(),
                call: None,
            },
            cx,
        );
        assert_eq!(ws.push_state(repo), None, "a new run puts it away");
        ws.push(repo, false, cx);
        ws.pushed(repo, Err(PushFailure::Failed("no git".into())), cx);
        assert_eq!(ws.push_state(repo), None);
        let (title, message) = ws.alert().expect("a dialog");
        assert_eq!(title, "Could not push tau-agent");
        assert_eq!(message, "no git");
    });
    let pushes: Vec<(String, bool)> = events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            WorkspaceEvent::Push { repo, fetch } => {
                Some((repo.clone(), *fetch))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        pushes,
        [
            (repo.to_owned(), false),
            (repo.to_owned(), true),
            (repo.to_owned(), false)
        ]
    );
}

/// A skill is `/name` in the composer: what is typed after it goes with
/// it, as the message, to a new chat or to the one open.
#[gpui::test]
fn a_skill_command_sends_its_name_and_the_task(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open_demo(cx);
    workspace.update(&mut cx, |ws, cx| ws.navigate(Route::NewRun, cx));
    assert_eq!(
        names(composer_slash(&workspace, &mut cx, "/rel")),
        ["/release-notes"]
    );
    workspace.update(&mut cx, |ws, cx| {
        ws.set_composer("", cx);
        ws.submit_prompt("/release-notes from v0.3 to v0.4".into(), cx);
    });
    cx.run_until_parked();
    let Some(WorkspaceEvent::NewRun { prompt, .. }) =
        events.borrow().last().cloned()
    else {
        panic!("a new run: {:?}", events.borrow())
    };
    assert_eq!(prompt, "/release-notes from v0.3 to v0.4");
    events.borrow_mut().clear();

    let done = tau_agent::tool::RunId("plugin-docs".into());
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Run(done.clone()), cx);
        ws.set_composer("", cx);
        // Picked alone, it waits for the task in the composer.
        ws.submit_prompt("/code-review".into(), cx);
        assert_eq!(ws.composer_text(cx), "/code-review ");
        ws.set_composer("", cx);
        ws.submit_prompt("/code-review the retry loop".into(), cx);
    });
    cx.run_until_parked();
    assert!(
        events.borrow().iter().any(|event| matches!(event,
            WorkspaceEvent::Say { run, text, .. }
                if *run == done && text == "/code-review the retry loop")),
        "{:?}",
        events.borrow()
    );
}

/// From the turn asking the model to its first answer, the run reasons:
/// the transcript counts the seconds, and keeps them on the reasoning
/// once the model answers. A model that streamed no reasoning but took
/// a second or more gets a reasoning row holding only the time.
#[gpui::test]
fn the_transcript_times_the_model_reasoning(cx: &mut TestAppContext) {
    use tau_agent::event::RunEvent;

    let (workspace, mut cx, _) = open_demo(cx);
    let run = demo::run_id();
    let turn = |turn| RunEvent::TurnStart {
        run: run.clone(),
        turn,
    };
    let text = RunEvent::TextDelta {
        run: run.clone(),
        parent: None,
        delta: "Done.".into(),
    };
    workspace.update(&mut cx, |ws, cx| {
        ws.apply_streamed(turn(7), cx);
        assert!(ws.reasoning_for(&run).is_some(), "it waits on its model");
        ws.apply_streamed(
            RunEvent::ThinkingDelta {
                run: run.clone(),
                delta: "Check the header first.".into(),
            },
            cx,
        );
        ws.apply_streamed(text.clone(), cx);
        assert!(ws.reasoning_for(&run).is_none(), "it answered");
        let items = &ws.run(&run).unwrap().items;
        assert!(matches!(
            &items[items.len() - 2],
            Item::Thinking { text, secs: Some(0) } if text == "Check the header first."
        ));
        // Answered at once, with no reasoning: nothing to say.
        let before = ws.run(&run).unwrap().items.len();
        ws.apply_streamed(turn(8), cx);
        ws.apply_streamed(text.clone(), cx);
        assert_eq!(ws.run(&run).unwrap().items.len(), before);
        ws.apply_streamed(turn(9), cx);
    });
    std::thread::sleep(std::time::Duration::from_millis(1100));
    workspace.update(&mut cx, |ws, cx| {
        ws.apply_streamed(text.clone(), cx);
        let items = &ws.run(&run).unwrap().items;
        assert!(matches!(
            &items[items.len() - 2],
            Item::Thinking { text, secs: Some(1) } if text.is_empty()
        ));
    });
}

/// The computer times its model's reasoning, and every phone shows the
/// seconds it timed, however late the phone hears of the turn: one that
/// hears of an answer a second after the turn started shows no reasoning
/// the computer does not, and one that hears at once of a turn that took
/// a second shows the reasoning the computer does.
#[gpui::test]
fn a_phone_shows_the_reasoning_the_computer_timed(cx: &mut TestAppContext) {
    use tau_agent::event::RunEvent;
    use tau_ui_remote::{update::HostUpdate, view::RunUpdate};

    let (computer, mut computer_cx, _) = open(cx);
    let (phone, mut phone_cx, _) = open(cx);
    let echoed = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = echoed.clone();
    computer_cx.update(|_, cx| {
        cx.subscribe(&computer, move |_, update: &HostUpdate, _| {
            seen.borrow_mut().push(update.clone())
        })
        .detach()
    });
    computer.update(&mut computer_cx, |ws, _| ws.set_mirrored(true));
    let run = demo::run_id();
    let turn = |turn| {
        RunUpdate::Event(RunEvent::TurnStart {
            run: run.clone(),
            turn,
        })
    };
    let answer = || {
        RunUpdate::Event(RunEvent::TextDelta {
            run: run.clone(),
            parent: None,
            delta: "Done.".into(),
        })
    };
    let a_second = std::time::Duration::from_millis(1100);

    // Answered at once; the phone drops after the turn starts, and hears
    // of the answer a second later.
    computer.update(&mut computer_cx, |ws, cx| {
        ws.update_run(&run, turn(7), cx);
        ws.update_run(&run, answer(), cx);
    });
    computer_cx.run_until_parked();
    let updates: Vec<HostUpdate> = echoed.borrow_mut().drain(..).collect();
    let (started, rest) = updates.split_first().unwrap();
    phone.update(&mut phone_cx, |ws, cx| ws.apply(started.clone(), cx));
    std::thread::sleep(a_second);
    phone.update(&mut phone_cx, |ws, cx| {
        for update in rest {
            ws.apply(update.clone(), cx);
        }
    });
    assert_eq!(
        phone.update(&mut phone_cx, |ws, _| ws.synced()),
        computer.update(&mut computer_cx, |ws, _| ws.synced()),
    );

    // Took a second; the phone hears of the whole turn at once.
    computer
        .update(&mut computer_cx, |ws, cx| ws.update_run(&run, turn(8), cx));
    std::thread::sleep(a_second);
    computer.update(&mut computer_cx, |ws, cx| {
        ws.update_run(&run, answer(), cx);
        let items = &ws.run(&run).unwrap().items;
        assert!(matches!(
            &items[items.len() - 2],
            Item::Thinking { text, secs: Some(1) } if text.is_empty()
        ));
    });
    computer_cx.run_until_parked();
    phone.update(&mut phone_cx, |ws, cx| {
        for update in echoed.borrow_mut().drain(..) {
            ws.apply(update, cx);
        }
    });
    assert_eq!(
        phone.update(&mut phone_cx, |ws, _| ws.synced()),
        computer.update(&mut computer_cx, |ws, _| ws.synced()),
    );
}

/// What a plugin's host half asks of the conversations (ADR 0037): an
/// agent made, its main chat shown, and a turn of it started as tau's.
#[gpui::test]
fn a_host_half_makes_an_agent_and_starts_its_turns(cx: &mut TestAppContext) {
    use tau_testing::scripted::ScriptedModel;
    use tau_ui::{
        accounts::Credentials,
        host::{Host, HostConfig},
    };
    use tau_ui_plugin::{By, Turn};

    cx.executor().allow_parking();
    let (workspace, mut cx, _) = open(cx);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let store = runtime.block_on(tau_store_sqlite::memory()).unwrap();
    let model = ScriptedModel::new()
        .turn(|turn| turn.text("Noted."))
        .turn(|turn| turn.text("Hello."));
    let agent = tau_agent::agent::Agent::new(model).name("coder");
    let dir = tempfile::tempdir().unwrap().keep();
    let config = HostConfig {
        account: tau_ai::chatgpt::AccountId::parse("test-account").unwrap(),
        credentials: Credentials::new(dir.join("config")),
        model: Some("gpt-5.5".into()),
        store: dir.join("unused.db"),
        repos: dir.join("repos"),
        settings: dir.join("models.json"),
        repo_list: dir.join("repos.json"),
        skills: std::env::temp_dir().join("tau-test-skills-none"),
        identity: tau_vcs_host::Identity::default(),
    };
    let (host, events) = Host::with_agent(runtime, agent, store, config);
    let host = host.with_plugin_settings(
        tau_memory::NAME,
        serde_json::json!({ "after_each_run": false }),
    );
    let host = cx.update(|_, cx| host.attach(&workspace, events, cx));
    let conversations = host.conversations();

    // What a host half waits for, on a thread of its own, while the
    // interface answers.
    let wait = |ask: Box<dyn FnOnce() -> anyhow::Result<()> + Send>| {
        let asked = std::thread::spawn(ask);
        while !asked.is_finished() {
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        asked.join().unwrap()
    };
    fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(future)
    }

    let listing = conversations.clone();
    wait(Box::new(move || block_on(listing.list_agent("greet")))).unwrap();
    assert!(dir.join("agents").join("greet").is_dir());
    let main = host.block_on(host.main_of("greet")).unwrap();
    assert!(!conversations.running("greet"));
    let again = conversations.clone();
    let error = wait(Box::new(move || block_on(again.list_agent("greet"))))
        .unwrap_err();
    assert!(error.to_string().contains("already"), "{error}");

    let text = "[events · 1 · 09:00:00]\n- morning · schedule · morning:1 · 09:00:00\n  {\"due\":1}";
    let starting = conversations.clone();
    wait(Box::new(move || {
        block_on(starting.turn(Turn {
            repo: "greet".into(),
            text: text.into(),
            by: By::Tau,
            model: "gpt-5.5".into(),
            effort: "auto".into(),
        }))
    }))
    .unwrap();
    workspace.read_with(&cx, |ws, _| {
        let view = ws.run(&main).expect("the agent's main chat is shown");
        assert!(
            view.items
                .iter()
                .any(|item| *item == Item::Tau(text.to_owned())),
            "{:?}",
            view.items
        );
    });

    // Once that turn is over, the person's words start the next, as
    // theirs.
    while conversations.running("greet") {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let person = conversations.clone();
    wait(Box::new(move || {
        block_on(person.turn(Turn {
            repo: "greet".into(),
            text: "/tau-agents say hello".into(),
            by: By::Person,
            model: "gpt-5.5".into(),
            effort: "auto".into(),
        }))
    }))
    .unwrap();
    workspace.read_with(&cx, |ws, _| {
        let view = ws.run(&main).unwrap();
        assert!(
            view.items
                .iter()
                .any(|item| *item == Item::User("/tau-agents say hello".into())),
            "{:?}",
            view.items
        );
    });
}

/// A chat a plugin's host half wakes later (ADR 0039): it starts a turn
/// on tau's message when idle, is busy while a turn goes, and a chat
/// that is not there is gone.
#[gpui::test]
fn a_host_half_wakes_a_chat_that_is_idle(cx: &mut TestAppContext) {
    use tau_testing::scripted::ScriptedModel;
    use tau_ui::{
        accounts::Credentials,
        host::{Host, HostConfig},
    };
    use tau_ui_plugin::Waking;

    cx.executor().allow_parking();
    let (workspace, mut cx, _) = open(cx);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let store = runtime.block_on(tau_store_sqlite::memory()).unwrap();
    let model = ScriptedModel::new()
        .turn(|turn| {
            turn.text("Checked.")
                .delay(std::time::Duration::from_millis(300))
        })
        .turn(|turn| turn.text("Again."));
    let agent = tau_agent::agent::Agent::new(model).name("coder");
    let dir = tempfile::tempdir().unwrap().keep();
    let config = HostConfig {
        account: tau_ai::chatgpt::AccountId::parse("test-account").unwrap(),
        credentials: Credentials::new(dir.join("config")),
        model: Some("gpt-5.5".into()),
        store: dir.join("unused.db"),
        repos: dir.join("repos"),
        settings: dir.join("models.json"),
        repo_list: dir.join("repos.json"),
        skills: std::env::temp_dir().join("tau-test-skills-none"),
        identity: tau_vcs_host::Identity::default(),
    };
    let (host, events) = Host::with_agent(runtime, agent, store, config);
    let host = host.with_plugin_settings(
        tau_memory::NAME,
        serde_json::json!({ "after_each_run": false }),
    );
    let host = cx.update(|_, cx| host.attach(&workspace, events, cx));
    let conversations = host.conversations();
    let wait = |ask: Box<dyn FnOnce() -> anyhow::Result<Waking> + Send>| {
        let asked = std::thread::spawn(ask);
        while !asked.is_finished() {
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        asked.join().unwrap()
    };
    fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(future)
    }

    // A chat to wake: the main chat a repository gets.
    let listing = conversations.clone();
    let listed =
        std::thread::spawn(move || block_on(listing.list_agent("greet")));
    while !listed.is_finished() {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    listed.join().unwrap().unwrap();
    let main = host.block_on(host.main_of("greet")).unwrap();

    let text = "[scheduled · recheck · 2026-10-13 09:30]\nDid #412 merge?";
    let waking = conversations.clone();
    let id = main.0.to_string();
    let woke = wait(Box::new(move || block_on(waking.wake(&id, text.into()))));
    assert_eq!(woke.unwrap(), Waking::Started);
    workspace.read_with(&cx, |ws, _| {
        let view = ws.run(&main).expect("the chat is shown");
        assert!(
            view.items
                .iter()
                .any(|item| *item == Item::Tau(text.to_owned())),
            "{:?}",
            view.items
        );
    });
    // Its turn is going: another wake waits.
    let busy = conversations.clone();
    let id = main.0.to_string();
    assert_eq!(
        wait(Box::new(move || block_on(busy.wake(&id, "later".into()))))
            .unwrap(),
        Waking::Busy
    );
    let gone = conversations.clone();
    assert_eq!(
        wait(Box::new(move || block_on(
            gone.wake("no-such-run", "x".into())
        )))
        .unwrap(),
        Waking::Gone
    );
}
