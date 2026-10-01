//! Opens the interface.
//!
//! With a ChatGPT sign-in that allows plan use, `tau-ui` runs a real
//! coding agent in the repositories cloned from GitHub: type a task to
//! start a run.
//! Without one it opens onboarding to sign in. With `--demo` it
//! replays the scripted session instead.
//!
//! - `--model <id>`: the model; gpt-6.1-sol by default.
//! - `--prompt <text>`: start a run with this task right away.
//! - `--demo`: the scripted session; `--finished` opens it done.
//! - `--open <screen>`: run, history, memory, plugins, constitution,
//!   compare, plan or ledger; onboarding's welcome, github, token,
//!   model, repos or ready; pr and pr-opened; alert, a sample dialog; attach-alert, a long one on New run;
//!   usage-limit, the ChatGPT plan's limit; plan-disabled, the Models
//!   screen with plan use not enabled; plan-signing-in, onboarding
//!   waiting for the ChatGPT sign-in; plan-connecting, that wait
//!   ending in a sign-in after 2.5 s, to watch the handshake land;
//!   model-signed-in, plan-declined and not-eligible, its other states;
//!   a phone pairing: pair, pair-scan, pair-address, pair-paired or
//!   pair-unreachable; phones, the computer's Phones screen; land,
//!   merge or resolving, a run's landing card (ADR 0014);
//!   models, picker, run-picker, fork-picker, log, status, show or diff
//!   (demo screens).
//!   With a sign-in, only `page:<plugin>/<page>[?key=value&...]`, a
//!   plugin's page, such as `page:tau-mcp/servers?repo=` for the MCP
//!   servers outside a repository.
//! - `--phone`: the phone layout in a 390×844 frame.
//! - `--frame <w>x<h>`: lay out at exactly that size in the top-left
//!   corner, to compare with the designs.
//! - `--steps <n>`: stop the demo script after its first `n` updates.
//! - `--reduce-motion`: no looping motion, and transitions as plain
//!   fades. `TAU_REDUCE_MOTION=1` asks the same (`0` asks for all the
//!   motion), then `"reduce_motion": true` in `interface.json`, then
//!   the desktop's `enable-animations`.

use gpui::{
    App,
    AppContext,
    Bounds,
    TitlebarOptions,
    WindowBounds,
    WindowOptions,
    px,
    size,
};
use tau_ui::{
    Workspace,
    accounts::Credentials,
    assets::Assets,
    catalog::Catalog,
    demo,
    host::{self, Host, HostConfig},
    models::AccountState,
    motion::{self, MotionPreference},
    pairing::PairStep,
    route::Route,
    setup::SetupStep,
};

struct Args {
    demo: bool,
    finished: bool,
    phone: bool,
    open: Option<String>,
    prompt: Option<String>,
    frame: Option<(f32, f32)>,
    steps: Option<usize>,
    /// `None` takes gpt-6.1-sol.
    model: Option<String>,
    reduce_motion: bool,
}

fn args() -> Args {
    let args: Vec<String> = std::env::args().collect();
    let flag = |name: &str| args.iter().any(|arg| arg == name);
    let value = |name: &str| {
        args.iter()
            .position(|arg| arg == name)
            .and_then(|at| args.get(at + 1))
            .cloned()
    };
    Args {
        demo: flag("--demo"),
        finished: flag("--finished"),
        phone: flag("--phone"),
        open: value("--open"),
        prompt: value("--prompt"),
        frame: value("--frame").and_then(|frame| {
            let (w, h) = frame.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        }),
        steps: value("--steps").and_then(|steps| steps.parse().ok()),
        model: value("--model"),
        reduce_motion: flag("--reduce-motion"),
    }
}

fn main() {
    let args = args();
    let credentials = Credentials::default_dir();
    let account = if args.demo {
        None
    } else {
        credentials.plan_account()
    };
    let model = args.model.clone();
    let saved = credentials.clone();
    let config = move |account| HostConfig {
        model: model.clone(),
        account,
        credentials: saved.clone(),
        store: HostConfig::default_store(),
        repos: HostConfig::default_repos(),
        settings: HostConfig::default_settings(),
        repo_list: HostConfig::default_repo_list(),
    };
    let host = account.and_then(|account| match Host::new(config(account)) {
        Ok(host) => Some(host),
        Err(error) => {
            eprintln!("tau-ui: cannot start agents: {error}");
            None
        }
    });
    let onboarding = host.is_none() && !args.demo;

    gpui_platform::application().with_assets(Assets).run(
        move |cx: &mut App| {
            tau_ui::init(cx);
            let bounds = Bounds::centered(None, size(px(1440.), px(900.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Tau".into()),
                    ..Default::default()
                }),
                window_min_size: Some(size(px(360.), px(560.))),
                app_id: Some("tau-ui".into()),
                ..Default::default()
            };
            let name = "tau";
            let live = match &host {
                Some((host, _)) => Some(host.catalog()),
                None if onboarding => Some(Catalog::default()),
                None => None,
            };
            let opened = cx.open_window(options, |window, cx| {
                cx.new(|cx| match live {
                    Some(catalog) => {
                        let mut workspace = Workspace::new(
                            name,
                            Vec::new(),
                            catalog,
                            window,
                            cx,
                        );
                        workspace.set_phone_preview(args.phone, cx);
                        workspace.set_frame(args.frame, cx);
                        workspace
                    }
                    None => demo_workspace(&args, window, cx),
                })
            });
            let Ok(workspace) = opened.and_then(|window| window.entity(cx))
            else {
                eprintln!("tau-ui: could not open a window");
                cx.quit();
                return;
            };
            follow_motion_preference(&workspace, args.reduce_motion, cx);
            match host {
                Some((host, events)) => {
                    workspace
                        .update(cx, |ws, cx| ws.navigate(Route::NewRun, cx));
                    host.attach(&workspace, events, cx);
                    if let Some(prompt) = args.prompt.clone() {
                        workspace
                            .update(cx, |ws, cx| ws.submit_prompt(prompt, cx));
                    }
                    if let Some(route) =
                        args.open.as_deref().and_then(plugin_page)
                    {
                        workspace.update(cx, |ws, cx| ws.navigate(route, cx));
                    }
                }
                // No model yet: set one up, then start the host.
                None if onboarding => {
                    // GitHub first, unless a saved sign-in makes it done.
                    let step = if tau_ui::github::Token::load(&credentials)
                        .is_some()
                    {
                        SetupStep::Model
                    } else {
                        SetupStep::Welcome
                    };
                    workspace.update(cx, |ws, cx| ws.start_setup(step, cx));
                    let entity = workspace.clone();
                    host::onboard(
                        &workspace,
                        args.model.clone(),
                        credentials.clone(),
                        cx,
                        move |account, cx| match Host::new(config(account)) {
                            Ok((host, events)) => {
                                let catalog = host.catalog();
                                entity.update(cx, |ws, cx| {
                                    ws.set_catalog(catalog, cx)
                                });
                                host.attach(&entity, events, cx);
                            }
                            Err(error) => {
                                eprintln!(
                                    "tau-ui: cannot start agents: {error}"
                                )
                            }
                        },
                    );
                }
                // The demo: answer the way a host would.
                None => {
                    demo::respond(&workspace, cx);
                    // After the responder, so what the screen asks for
                    // on opening is answered.
                    workspace
                        .update(cx, |ws, cx| open_demo_screen(ws, &args, cx));
                }
            }
            cx.activate(true);
        },
    );
}

/// The plugin page `--open page:<plugin>/<page>[?key=value&...]` names,
/// such as `page:tau-mcp/servers?repo=`.
fn plugin_page(open: &str) -> Option<Route> {
    let rest = open.strip_prefix("page:")?;
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (plugin, page) = path.split_once('/')?;
    let params = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (key.to_owned(), value.to_owned())
        })
        .collect();
    Some(Route::Plugin {
        plugin: plugin.to_owned(),
        page: page.to_owned(),
        params,
    })
}

/// Reduces motion when asked: at once from the flag, the environment or
/// the saved setting, and otherwise when the desktop answers whether it
/// animates.
fn follow_motion_preference(
    workspace: &gpui::Entity<Workspace>,
    flag: bool,
    cx: &mut App,
) {
    let mut preference = MotionPreference::from_startup(
        flag,
        &HostConfig::default_interface_settings(),
    );
    workspace
        .update(cx, |ws, cx| ws.set_reduce_motion(preference.reduce(), cx));
    if preference.stated().is_some() {
        return;
    }
    let workspace = workspace.downgrade();
    cx.spawn(async move |cx| {
        let asking = cx.background_spawn(motion::desktop_animations());
        preference.desktop_animations = asking.await;
        let reduce = preference.reduce();
        let _ = workspace.update(cx, |ws, cx| ws.set_reduce_motion(reduce, cx));
    })
    .detach();
}

fn demo_workspace(
    args: &Args,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<Workspace>,
) -> Workspace {
    let mut runs = vec![demo::retry_after()];
    runs.extend(demo::history());
    let mut workspace =
        Workspace::new("tau-agent", runs, demo::catalog(), window, cx);
    workspace.set_phone_preview(args.phone, cx);
    workspace.set_frame(args.frame, cx);
    if let Some(steps) = args.steps {
        for (_, update) in demo::script().into_iter().take(steps) {
            workspace.update_run(&demo::run_id(), update, cx);
        }
    } else if args.finished {
        for (_, update) in demo::script() {
            workspace.update_run(&demo::run_id(), update, cx);
        }
    } else {
        workspace.replay(demo::run_id(), demo::script(), cx);
    }
    workspace
}

/// The screen `--open` names, with the demo data it needs.
fn open_demo_screen(
    workspace: &mut Workspace,
    args: &Args,
    cx: &mut gpui::Context<Workspace>,
) {
    // The model picker's states, for looking at them.
    match args.open.as_deref() {
        Some("picker") => {
            workspace.navigate(Route::NewRun, cx);
            workspace.show_picker(tau_ui::workspace::PickerTarget::Next, cx);
            return;
        }
        Some("run-picker") => {
            workspace.navigate(Route::Run(demo::run_id()), cx);
            workspace.show_picker(
                tau_ui::workspace::PickerTarget::Run(demo::run_id()),
                cx,
            );
            return;
        }
        // Landing (ADR 0014): a fork's landing card, open, with a
        // conflict.
        Some("land") => {
            use tau_ui::update::HostUpdate;
            let run = demo::fork_id();
            workspace.navigate(Route::Run(run.clone()), cx);
            workspace.apply(
                HostUpdate::LandingPreview {
                    run,
                    preview: Ok(demo::landing_preview(true)),
                },
                cx,
            );
            return;
        }
        // The vcs_log card, open, with a change picked.
        Some("log") => {
            let run = demo::run_id();
            workspace.navigate(Route::Run(run.clone()), cx);
            workspace.toggle_card(&run, demo::LOG_CALL, cx);
            if let Some(cards) =
                workspace.plugin_ui::<tau_vcs::ui::Ui>(tau_vcs::ui::NAME)
            {
                cards.update(cx, |cards, _| {
                    cards.pick(&run, demo::LOG_CALL, demo::LOG_PICKED)
                });
            }
            return;
        }
        // The vcs_status, vcs_show and vcs_diff cards, open, with a file
        // open.
        Some(open @ ("status" | "show" | "diff")) => {
            let run = demo::run_id();
            let (call, file) = match open {
                "status" => (demo::STATUS_CALL, demo::SHOW_FILE),
                "show" => (demo::SHOW_CALL, demo::SHOW_FILE),
                _ => (demo::DIFF_CALL, demo::DIFF_FILE),
            };
            workspace.navigate(Route::Run(run.clone()), cx);
            workspace.toggle_card(&run, call, cx);
            if let Some(cards) =
                workspace.plugin_ui::<tau_vcs::ui::Ui>(tau_vcs::ui::NAME)
            {
                cards
                    .update(cx, |cards, _| cards.toggle_file(&run, call, file));
            }
            return;
        }
        Some("fork-picker") => {
            workspace.navigate(Route::Run(demo::run_id()), cx);
            workspace.start_fork_at(&demo::run_id(), 2, cx);
            workspace.show_picker(tau_ui::workspace::PickerTarget::Fork, cx);
            return;
        }
        _ => {}
    }
    // The repository tree's states.
    match args.open.as_deref() {
        Some("repo-menu") => {
            workspace.toggle_repo_open("docbert", cx);
            workspace.toggle_repo_menu("homelab.nix", cx);
            return;
        }
        // The Constitution page's states.
        Some(
            open @ ("rule-editor" | "rule-missing" | "rules-review"
            | "rules-broken" | "rules-empty"),
        ) => {
            use tau_constitution::ui::{Rules, page};
            let repo = "tau-agent";
            workspace.navigate(
                Route::Plugin {
                    plugin: tau_constitution::NAME.into(),
                    page: "rules".into(),
                    params: [("repo".to_owned(), repo.to_owned())].into(),
                },
                cx,
            );
            let Some(rules_ui) =
                workspace.plugin_ui::<page::Ui>(tau_constitution::NAME)
            else {
                return;
            };
            let rule = "Never run migrations against the production database.";
            match open {
                "rule-editor" => {
                    // Tried on the demo run's shell commands.
                    let calls: Vec<_> = workspace
                        .run(&demo::run_id())
                        .map(|run| run.cards())
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|card| card.tool == "bash")
                        .map(|card| (card.tool, card.args))
                        .collect();
                    rules_ui.update(cx, |ui, cx| {
                        ui.open_editor(repo, None, cx);
                        ui.set_rule_text(rule, cx);
                        ui.toggle_place("bash.command", cx);
                        ui.try_rule(calls, Vec::new(), cx);
                    });
                }
                "rule-missing" => rules_ui.update(cx, |ui, cx| {
                    ui.open_editor(repo, None, cx);
                    ui.set_rule_text(rule, cx);
                    ui.save(cx);
                }),
                "rules-review" => rules_ui
                    .update(cx, |ui, cx| ui.set_tab(page::Tab::Review, cx)),
                _ => {
                    let mut catalog = workspace.catalog().clone();
                    let mut rules = Rules::default();
                    if open == "rules-broken" {
                        rules.error = Some(
                            "The constitution stored for tau-agent is not \
                             valid: Rule R4: review (0.95) is above block (0.9)"
                                .into(),
                        );
                    } else {
                        catalog.models.access.jev = false;
                    }
                    if let Some(listed) = catalog.repo_mut(repo) {
                        listed.plugins.insert(
                            tau_constitution::NAME.into(),
                            serde_json::to_value(rules).unwrap_or_default(),
                        );
                    }
                    workspace.set_catalog(catalog, cx);
                }
            }
            return;
        }
        // /goal: the command menu, writing a goal, and a goal's states.
        Some("slash") => {
            workspace.navigate(Route::Run(demo::run_id()), cx);
            workspace.set_composer("/", cx);
            return;
        }
        Some("goal-args") => {
            workspace.navigate(Route::Run(demo::run_id()), cx);
            workspace.set_composer("/goal ", cx);
            return;
        }
        Some(open @ ("goal" | "goal-met" | "goal-stopped" | "goal-sheet")) => {
            let state = match open {
                "goal-met" => "met",
                "goal-stopped" => "stopped",
                _ => "working",
            };
            workspace.navigate(Route::Run(demo::run_id()), cx);
            for body in demo::goal_records(demo::GOAL, state) {
                workspace.apply_event(
                    &tau_agent::event::RunEvent::PluginReport {
                        run: demo::run_id(),
                        plugin: tau_goal::NAME.into(),
                        body,
                    },
                    cx,
                );
            }
            if open == "goal-sheet" {
                workspace.toggle_sheet(cx);
            }
            return;
        }
        Some("run-plugins") => {
            workspace.navigate(Route::Run(demo::run_id()), cx);
            return;
        }
        Some("note-open") => {
            // The demo streams tau-reasoning's note in right after the
            // task.
            workspace.toggle_note(&demo::run_id(), 1, cx);
            return;
        }
        Some("composer-lines") => {
            workspace.navigate(Route::Run(demo::run_id()), cx);
            workspace.set_composer(
                "Two things before you merge:\n- keep the jitter under 10%\n\
                 - log the header we ignored, once per run",
                cx,
            );
            return;
        }
        Some("search") => {
            workspace.show_search("re", cx);
            return;
        }
        Some("history-query") => {
            workspace.navigate(Route::History, cx);
            workspace.run_query(cx);
            return;
        }
        Some("sheet-plugins") => {
            workspace.navigate(Route::Run(demo::run_id()), cx);
            workspace.toggle_sheet(cx);
            return;
        }
        Some("sheet-run") => {
            workspace.navigate(Route::Run(demo::run_id()), cx);
            workspace.toggle_sheet(cx);
            return;
        }
        Some("repo-open") => {
            workspace.toggle_repo_open("docbert", cx);
            return;
        }
        _ => {}
    }
    if args.open.as_deref() == Some("attach-alert") {
        workspace.navigate(Route::NewRun, cx);
        workspace.show_alert(
            "Could not attach Before Orthodoxy - Shahab Ahmed.epub",
            "it is 581 KB; files up to 200 KB can be attached",
            cx,
        );
        return;
    }
    // The ChatGPT plan's states.
    match args.open.as_deref() {
        Some("usage-limit") => {
            workspace.navigate(Route::Run(demo::run_id()), cx);
            workspace.show_plan_refusal(&demo::usage_limit(), cx);
            return;
        }
        Some("plan-disabled") => {
            let mut catalog = workspace.catalog().clone();
            let access = &mut catalog.models.access;
            access.chatgpt = false;
            access.label = "signed out".into();
            for account in &mut access.accounts {
                account.active = account.state == AccountState::PlanDisabled;
            }
            workspace.set_catalog(catalog, cx);
            workspace.navigate(Route::Models, cx);
            return;
        }
        // The wait ending in a sign-in, to watch the handshake land.
        Some("plan-connecting") => {
            let mut setup = demo::setup(SetupStep::Model);
            setup.model = tau_ui::setup::ModelAccess::SigningIn {
                url: Some(demo::DEMO_AUTHORIZE_URL.into()),
            };
            workspace.set_setup(setup, cx);
            workspace.navigate(Route::Setup(SetupStep::Model), cx);
            cx.spawn(async |workspace, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(2500))
                    .await;
                let _ = workspace.update(cx, |ws, cx| {
                    ws.update_setup(
                        tau_ui::setup::SetupUpdate::Model(
                            tau_ui::setup::ModelAccess::Connected {
                                label: "gpt-6.1-sol · ChatGPT plan".into(),
                            },
                        ),
                        cx,
                    )
                });
            })
            .detach();
            return;
        }
        Some("plan-signing-in") => {
            let mut setup = demo::setup(SetupStep::Model);
            setup.model = tau_ui::setup::ModelAccess::SigningIn {
                url: Some(demo::DEMO_AUTHORIZE_URL.into()),
            };
            workspace.set_setup(setup, cx);
            workspace.navigate(Route::Setup(SetupStep::Model), cx);
            return;
        }
        // The model step's other states: signed in, plan use declined,
        // and an account that cannot share its plan.
        Some(
            state @ ("model-signed-in" | "plan-declined" | "not-eligible"),
        ) => {
            use tau_ui::setup::ModelAccess;
            let account = demo::chatgpt_accounts()[0].label.clone();
            let mut setup = demo::setup(SetupStep::Model);
            setup.model = match state {
                "model-signed-in" => ModelAccess::Connected {
                    label: "gpt-6.1-sol · ChatGPT plan".into(),
                },
                "plan-declined" => ModelAccess::PlanDisabled { account },
                _ => ModelAccess::NotEligible {
                    account,
                    detail: "403 subscription_sharing_user_not_eligible · \
                             request req_7f3a9c01"
                        .into(),
                },
            };
            workspace.set_setup(setup, cx);
            workspace.navigate(Route::Setup(SetupStep::Model), cx);
            return;
        }
        _ => {}
    }
    if args.open.as_deref() == Some("alert") {
        workspace.show_alert(
            "Could not fork the run",
            "Forking needs a project: /home/you/notes could not be copied: \
             it is not a Git repository.",
            cx,
        );
        return;
    }
    match args.open.as_deref().and_then(demo::route) {
        Some(Route::Setup(step)) => {
            workspace.set_setup(demo::setup(step), cx);
            workspace.navigate(Route::Setup(step), cx);
        }
        Some(Route::Phones) => {
            workspace.set_phones(demo::phones(), cx);
            workspace.navigate(Route::Phones, cx);
        }
        // Pairing starts at its welcome, so the others can go back.
        Some(Route::Pair(step)) => {
            workspace.start_pairing(PairStep::Welcome, cx);
            workspace.set_pairing(demo::pairing(step), cx);
            if step != PairStep::Welcome {
                workspace.navigate(Route::Pair(step), cx);
            }
        }
        Some(Route::PullRequest(run)) => {
            let mut pr = demo::pull_request();
            if args.open.as_deref() == Some("pr-opened") {
                pr.state = demo::opened();
            }
            workspace.set_pull_request(&run, pr, cx);
            workspace.open_pull_request(&run, cx);
        }
        Some(route) => workspace.navigate(route, cx),
        None => {}
    }
}
