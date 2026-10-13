//! The demo's screens by name, for `--open`: each one opens the
//! workspace on a screen, in the state it names.

use gpui::Context;
use tau_ui_plugin::PluginValue;

use super::*;
use crate::{route::Route, setup::ModelAccess, workspace::PickerTarget};

/// Opens a screen of the demo on the workspace, with what it needs from
/// the demo's host.
pub type Screen = fn(&mut Workspace, &DemoHost, &mut Context<Workspace>);

/// Every screen `--open` names.
pub static SCREENS: &[(&str, Screen)] = &[
    ("run", |ws, _, cx| ws.navigate(Route::Run(run_id()), cx)),
    // Main's crew tray: two sub-agents at work, one landed, one failed.
    ("crew", |ws, _, cx| crew(ws, cx)),
    ("repo", |ws, _, cx| ws.open_repo_page("tau-agent", cx)),
    ("history", |ws, _, cx| ws.navigate(Route::History, cx)),
    ("plugins", |ws, _, cx| ws.navigate(Route::Plugins, cx)),
    // tau-memory's settings: the model it writes notes with.
    ("plugins-memory", |ws, _, cx| {
        ws.navigate(Route::Plugins, cx);
        ws.pick_plugin(tau_memory::NAME, None, cx);
    }),
    ("models", |ws, _, cx| ws.navigate(Route::Models, cx)),
    ("plan", |ws, _, cx| ws.navigate(Route::Plan(run_id()), cx)),
    ("compare", |ws, _, cx| {
        let (main, fork) = (run_id(), fork_id());
        ws.navigate(Route::Compare { main, fork }, cx)
    }),
    ("memory", |ws, _, cx| {
        plugin_page(ws, tau_memory_host::NAME, "notes", "repo", cx)
    }),
    ("constitution", |ws, _, cx| {
        plugin_page(ws, tau_constitution_host::NAME, "rules", "repo", cx)
    }),
    ("mcp", |ws, _, cx| {
        plugin_page(ws, tau_mcp_host::NAME, "servers", "repo", cx)
    }),
    // The person's own plugins (ADR 0036): their page, and the page of
    // one that waits for them.
    ("plugins-yours", |ws, host, cx| yours(ws, host, None, cx)),
    ("plugins-yours-entry", |ws, host, cx| {
        yours(ws, host, Some("no-force-push"), cx)
    }),
    // Agents (ADR 0037): the Agents page, one agent's conversation,
    // and the New agent form.
    ("agents", |ws, host, cx| {
        agents(ws, host, AgentsAt::Overview, cx)
    }),
    ("agent", |ws, host, cx| {
        agents(ws, host, AgentsAt::Conversation, cx)
    }),
    ("agent-new", |ws, host, cx| {
        agents(ws, host, AgentsAt::New, cx)
    }),
    // Home with the agents: on a phone, the run list and its Agents.
    ("agents-home", |ws, host, cx| {
        agents(ws, host, AgentsAt::Home, cx)
    }),
    // pr-triage's conversation with its details open: on a phone, the
    // sheet with its panel and what it left the person.
    ("agent-details", |ws, host, cx| {
        agents(ws, host, AgentsAt::Details, cx)
    }),
    ("ledger", |ws, _, cx| {
        let run = run_id().0.to_string();
        let page = Route::Plugin {
            plugin: tau_fast_compaction::NAME.into(),
            page: "ledger".into(),
            params: [("run".to_owned(), run)].into(),
        };
        ws.navigate(page, cx)
    }),
    // Onboarding's steps.
    ("welcome", |ws, _, cx| setup_at(ws, SetupStep::Welcome, cx)),
    ("setup", |ws, _, cx| setup_at(ws, SetupStep::Welcome, cx)),
    ("github", |ws, _, cx| setup_at(ws, SetupStep::GitHub, cx)),
    ("token", |ws, _, cx| setup_at(ws, SetupStep::Token, cx)),
    ("model", |ws, _, cx| setup_at(ws, SetupStep::Model, cx)),
    ("repos", |ws, _, cx| setup_at(ws, SetupStep::Repos, cx)),
    ("ready", |ws, _, cx| setup_at(ws, SetupStep::Ready, cx)),
    // The model step's other states: waiting for the ChatGPT sign-in,
    // the wait ending in one after 2.5 s to watch the handshake land,
    // signed in, plan use declined, and an account that cannot share
    // its plan.
    ("plan-signing-in", |ws, _, cx| {
        let url = Some(DEMO_AUTHORIZE_URL.into());
        model_at(ws, ModelAccess::SigningIn { url }, cx)
    }),
    ("plan-connecting", |ws, _, cx| {
        let url = Some(DEMO_AUTHORIZE_URL.into());
        model_at(ws, ModelAccess::SigningIn { url }, cx);
        cx.spawn(async |workspace, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(2500))
                .await;
            let connected = ModelAccess::Connected {
                label: "gpt-6.1-sol · ChatGPT plan".into(),
            };
            let update = HostUpdate::Setup(SetupUpdate::Model(connected));
            let _ = workspace.update(cx, |ws, cx| ws.apply(update, cx));
        })
        .detach();
    }),
    ("model-signed-in", |ws, _, cx| {
        let label = "gpt-6.1-sol · ChatGPT plan".into();
        model_at(ws, ModelAccess::Connected { label }, cx)
    }),
    ("plan-declined", |ws, _, cx| {
        let account = chatgpt_accounts()[0].label.clone();
        model_at(ws, ModelAccess::PlanDisabled { account }, cx)
    }),
    ("not-eligible", |ws, _, cx| {
        let account = chatgpt_accounts()[0].label.clone();
        let detail = "403 subscription_sharing_user_not_eligible · request \
                      req_7f3a9c01"
            .into();
        model_at(ws, ModelAccess::NotEligible { account, detail }, cx)
    }),
    // A phone pairing, which starts at its welcome so the others can
    // go back; and the computer's Phones screen.
    ("pair", |ws, _, cx| pair_at(ws, PairStep::Welcome, cx)),
    ("pair-scan", |ws, _, cx| pair_at(ws, PairStep::Scan, cx)),
    ("pair-address", |ws, _, cx| {
        pair_at(ws, PairStep::Address, cx)
    }),
    ("pair-paired", |ws, _, cx| pair_at(ws, PairStep::Paired, cx)),
    ("pair-unreachable", |ws, _, cx| {
        pair_at(ws, PairStep::Unreachable, cx)
    }),
    ("phones", |ws, _, cx| {
        ws.set_phones(phones(), cx);
        ws.navigate(Route::Phones, cx);
    }),
    // The main chat ahead of GitHub (ADR 0023): its header and sidebar
    // row; the card of a push that went; GitHub's main moved.
    ("push", |ws, _, cx| ws.navigate(Route::Run(run_id()), cx)),
    ("pushed", |ws, _, cx| {
        ws.navigate(Route::Run(run_id()), cx);
        let result = Ok(pushed());
        ws.apply(
            HostUpdate::Pushed {
                repo: "tau-agent".into(),
                result,
            },
            cx,
        );
    }),
    ("push-rejected", |ws, _, cx| {
        ws.navigate(Route::Run(run_id()), cx);
        let result = Err(crate::push::PushFailure::Moved {
            branch: "main".into(),
            ahead: 3,
        });
        ws.apply(
            HostUpdate::Pushed {
                repo: "tau-agent".into(),
                result,
            },
            cx,
        );
    }),
    // A pull request from the demo run, written and opened.
    ("pr", |ws, _, cx| pull_request_at(ws, pull_request(), cx)),
    ("pr-opened", |ws, _, cx| {
        let pr = PullRequest {
            state: opened(),
            ..pull_request()
        };
        pull_request_at(ws, pr, cx)
    }),
    // The model picker's states.
    ("picker", |ws, _, cx| {
        ws.navigate(Route::NewRun, cx);
        ws.show_picker(PickerTarget::Next, cx);
    }),
    ("run-picker", |ws, _, cx| {
        ws.navigate(Route::Run(run_id()), cx);
        ws.show_picker(PickerTarget::Run(run_id()), cx);
    }),
    ("fork-picker", |ws, _, cx| {
        ws.navigate(Route::Run(run_id()), cx);
        ws.start_fork_at(&run_id(), 2, cx);
        ws.show_picker(PickerTarget::Fork, cx);
    }),
    // Landing (ADR 0014): a fork's landing card, open, with a conflict.
    ("land", |ws, _, cx| {
        let run = fork_id();
        ws.navigate(Route::Run(run.clone()), cx);
        let preview = Ok(landing_preview(true));
        ws.apply(HostUpdate::LandingPreview { run, preview }, cx);
    }),
    // A chat that landed, or was dropped: read-only, with a way to a
    // new chat from main in the composer's place.
    ("landed", |ws, _, cx| {
        let run = fork_id();
        let landing = Ok(landing_preview(false));
        ws.apply(
            HostUpdate::Landed {
                run: run.clone(),
                landing,
            },
            cx,
        );
        ws.navigate(Route::Run(run), cx);
    }),
    ("dropped", |ws, _, cx| {
        let run = fork_id();
        ws.apply(
            HostUpdate::Dropped {
                run: run.clone(),
                result: Ok(()),
            },
            cx,
        );
        ws.navigate(Route::Run(run), cx);
    }),
    // A chat tau closing cut off: it waits to be resumed.
    ("interrupted", |ws, _, cx| {
        let Some(mut view) = ws.run(&fork_id()).cloned() else {
            return;
        };
        view.id = RunId("interrupted".into());
        view.title = "Atomic landing".into();
        view.items
            .retain(|item| !matches!(item, crate::view::Item::Stop { .. }));
        view.status = crate::view::RunStatus::Interrupted;
        let run = view.id.clone();
        ws.apply(HostUpdate::History(vec![view]), cx);
        ws.navigate(Route::Run(run), cx);
    }),
    // A landing tau closed in the middle of, finished at start: its card
    // in main.
    ("landing-finished", |ws, _, cx| {
        let run = fork_id();
        let Some(title) = ws.run(&run).map(|view| view.title.clone()) else {
            return;
        };
        let record = crate::view::LandingRecord {
            from: run.0.to_string(),
            title,
            landing: landing_preview(false),
            recovered: true,
            kept: false,
        };
        ws.apply(HostUpdate::LandingFinished(record), cx);
        ws.navigate(Route::Run(run_id()), cx);
    }),
    // Main's landing queue (ADR 0024): two chats wait for its turn, one
    // clean, one with conflicts the person confirmed; and the waiting
    // chat's own card.
    ("land-queue", |ws, _, cx| {
        land_queue(ws, cx);
        ws.navigate(Route::Run(run_id()), cx);
    }),
    ("land-queued", |ws, _, cx| {
        land_queue(ws, cx);
        ws.navigate(Route::Run(fork_id()), cx);
    }),
    // A resolving turn left conflicts on main: the card that holds
    // landings and new chats.
    ("conflicts-on-main", |ws, _, cx| {
        let conflicts = crate::queue::MainConflicts {
            files: vec![
                "crates/tau-ui/src/host.rs".into(),
                "crates/tau-ui/src/host/landing.rs".into(),
            ],
            from: Some(fork_id().0.to_string()),
            prompt: "Landing `Retry with backoff` left conflicts.".into(),
            dismissed: false,
        };
        // tau's turn ended: the card offers to resolve again.
        ws.apply_event(
            &tau_agent::event::RunEvent::RunEnd {
                run: run_id(),
                parent: None,
                stop: tau_agent::event::StopReason::Stop,
                cost: 0.0,
            },
            cx,
        );
        ws.apply(
            HostUpdate::LandingQueue {
                main: run_id(),
                queue: Vec::new(),
                conflicts: Some(conflicts),
            },
            cx,
        );
        ws.navigate(Route::Run(run_id()), cx);
    }),
    // tau-ask's panel in the composer's place (ADR 0019): a question
    // with previews; a checklist with a note being written; the review.
    // tau-skills: the Skills screen; a skill loaded, its card open; a
    // skill not there.
    ("skills", |ws, _, cx| {
        ws.navigate(
            Route::Plugin {
                plugin: tau_skills::NAME.into(),
                page: "skills".into(),
                params: Default::default(),
            },
            cx,
        )
    }),
    ("skill", |ws, _, cx| skill(ws, true, cx)),
    ("skill-missing", |ws, _, cx| skill(ws, false, cx)),
    ("codemode", |ws, _, cx| codemode(ws, cx)),
    ("reasoning-wait", |ws, _, cx| reasoning(ws, None, cx)),
    ("reasoning", |ws, _, cx| {
        reasoning(
            ws,
            Some(
                "The policy caps every delay at max_delay.\n\
                 Checking how the retry policy computes its delay before \
                 reading the header.",
            ),
            cx,
        )
    }),
    ("ask", |ws, _, cx| ask(ws, "ask", cx)),
    ("ask-note", |ws, _, cx| ask(ws, "ask-note", cx)),
    ("ask-review", |ws, _, cx| ask(ws, "ask-review", cx)),
    // tau-watcher (ADR 0032): a new note with its band, the note with
    // its explanation open, and the note answered "knew this".
    ("watcher", |ws, _, cx| watcher(ws, None, cx)),
    ("watcher-open", |ws, _, cx| {
        watcher(ws, Some(tau_watcher::record::Answer::Learned), cx)
    }),
    ("watcher-known", |ws, _, cx| {
        watcher(ws, Some(tau_watcher::record::Answer::Knew), cx)
    }),
    // tau-direnv (ADR 0025): the question in the composer's place, the
    // environment loading, a load that failed, and the repository
    // menu's toggle.
    ("envrc-consent", |ws, _, cx| {
        envrc(
            ws,
            tau_direnv::Record::Asked {
                repo: "tau-agent".into(),
                envrc: "watch_file flake.nix flake.lock nix/*.nix\n\
                        watch_file rust-toolchain.toml\n\nuse flake\n"
                    .into(),
            },
            cx,
        )
    }),
    ("envrc-loading", |ws, _, cx| {
        let since = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |now| now.as_millis() as u64)
            .saturating_sub(41_000);
        envrc(ws, tau_direnv::Record::Loading { since }, cx)
    }),
    ("envrc-failed", |ws, _, cx| {
        envrc(
            ws,
            tau_direnv::Record::Failed {
                status: "direnv exited 1".into(),
                output: "error: flake 'path:/…/runs/fix-retry' does not \
                         provide attribute 'devShells.x86_64-linux.default'"
                    .into(),
            },
            cx,
        )
    }),
    ("envrc-menu", |ws, _, cx| {
        ws.toggle_repo_menu("tau-agent", cx);
    }),
    // tau-vcs's cards, open: the log with a change picked; status, show
    // and diff with a file open.
    ("log", |ws, _, cx| {
        let run = run_id();
        ws.navigate(Route::Run(run.clone()), cx);
        ws.toggle_card(&run, LOG_CALL, cx);
        if let Some(cards) = ws.plugin_ui::<tau_vcs::ui::Ui>(tau_vcs::ui::NAME)
        {
            cards.update(cx, |cards, _| cards.pick(&run, LOG_CALL, LOG_PICKED));
        }
    }),
    ("status", |ws, _, cx| {
        vcs_card(ws, STATUS_CALL, SHOW_FILE, cx)
    }),
    ("show", |ws, _, cx| vcs_card(ws, SHOW_CALL, SHOW_FILE, cx)),
    ("diff", |ws, _, cx| vcs_card(ws, DIFF_CALL, DIFF_FILE, cx)),
    ("commit", |ws, _, cx| {
        vcs_card(ws, COMMIT_CALL, DIFF_FILE, cx)
    }),
    // The repository tree's states.
    ("repo-open", |ws, _, cx| ws.toggle_repo_open("docbert", cx)),
    // Every chat state a row can show, at once, under tau-agent's main.
    ("sidebar-states", |ws, _, cx| sidebar_states(ws, cx)),
    ("repo-menu", |ws, _, cx| {
        ws.toggle_repo_open("docbert", cx);
        ws.toggle_repo_menu("homelab.nix", cx);
    }),
    // The Constitution page's states.
    ("rule-editor", |ws, _, cx| rules_at(ws, "rule-editor", cx)),
    ("rule-missing", |ws, _, cx| rules_at(ws, "rule-missing", cx)),
    ("rules-review", |ws, _, cx| rules_at(ws, "rules-review", cx)),
    // Rules that cannot be read; and none, with no Jev to try them.
    ("rules-broken", |ws, _, cx| {
        rules_at(ws, "rules-broken", cx);
        let mut catalog = ws.catalog().clone();
        if let Some(repo) = catalog.repo_mut("tau-agent") {
            repo.plugins.insert(
                tau_constitution_host::NAME.into(),
                PluginValue::typed(tau_constitution_host::demo::broken()),
            );
        }
        ws.apply(HostUpdate::catalog(catalog), cx);
    }),
    ("rules-empty", |ws, host, cx| {
        rules_at(ws, "rules-empty", cx);
        let reset = tau_constitution::ui::Act::Reset {
            repo: "tau-agent".into(),
        };
        let reset = serde_json::to_value(reset).expect("an act serializes");
        if let Err(error) = host.act(tau_constitution_host::NAME, reset) {
            eprintln!("tau-ui: the demo's rules stay: {error:#}");
        }
        let mut catalog = host.catalog(ws.catalog().clone());
        catalog.models.access.jev = false;
        ws.apply(HostUpdate::catalog(catalog), cx);
    }),
    // /goal: the command menu, writing a goal, and a goal's states.
    ("slash", |ws, _, cx| {
        ws.navigate(Route::Run(run_id()), cx);
        ws.set_composer("/", cx);
    }),
    ("goal-args", |ws, _, cx| {
        ws.navigate(Route::Run(run_id()), cx);
        ws.set_composer("/goal ", cx);
    }),
    ("goal", |ws, _, cx| goal_at(ws, "working", cx)),
    ("goal-met", |ws, _, cx| goal_at(ws, "met", cx)),
    ("goal-stopped", |ws, _, cx| goal_at(ws, "stopped", cx)),
    ("goal-sheet", |ws, _, cx| {
        goal_at(ws, "working", cx);
        ws.toggle_sheet(cx);
    }),
    // The run's other states.
    ("run-plugins", |ws, _, cx| {
        ws.navigate(Route::Run(run_id()), cx)
    }),
    // The demo streams tau-reasoning's note in right after the task.
    ("note-open", |ws, _, cx| ws.toggle_note(&run_id(), 1, cx)),
    // tau-reasoning keeping a long conversation's effort over Jev's
    // pick: its note, then its choices page.
    ("effort-kept", |ws, _, cx| {
        ws.navigate(Route::Run(run_id()), cx);
        effort_kept(ws, cx);
    }),
    ("effort-kept-page", |ws, _, cx| {
        effort_kept(ws, cx);
        let page = Route::Plugin {
            plugin: tau_reasoning::NAME.into(),
            page: "choices".into(),
            params: [("run".to_owned(), run_id().0.to_string())].into(),
        };
        ws.navigate(page, cx)
    }),
    ("composer-lines", |ws, _, cx| {
        ws.navigate(Route::Run(run_id()), cx);
        ws.set_composer(
            "Two things before you merge:\n- keep the jitter under 10%\n- \
             log the header we ignored, once per run",
            cx,
        );
    }),
    ("sheet-plugins", |ws, _, cx| {
        ws.navigate(Route::Run(run_id()), cx);
        ws.toggle_sheet(cx);
    }),
    ("sheet-run", |ws, _, cx| {
        ws.navigate(Route::Run(run_id()), cx);
        ws.toggle_sheet(cx);
    }),
    ("search", |ws, _, cx| ws.show_search("re", cx)),
    ("history-query", |ws, _, cx| {
        ws.navigate(Route::History, cx);
        ws.run_query(cx);
    }),
    // The ChatGPT plan's states: its limit on a run, and the Models
    // screen with plan use not enabled.
    ("usage-limit", |ws, _, cx| {
        ws.navigate(Route::Run(run_id()), cx);
        ws.apply(HostUpdate::PlanRefusal(usage_limit()), cx);
    }),
    ("plan-disabled", |ws, _, cx| {
        let mut catalog = ws.catalog().clone();
        let access = &mut catalog.models.access;
        access.chatgpt = false;
        access.label = "signed out".into();
        for account in &mut access.accounts {
            account.active = account.state == AccountState::PlanDisabled;
        }
        ws.apply(HostUpdate::catalog(catalog), cx);
        ws.navigate(Route::Models, cx);
    }),
    // Dialogs: a sample one, and a long one on New run.
    ("alert", |ws, _, cx| {
        let alert = HostUpdate::alert(
            "Could not fork the run",
            "Forking needs a project: /home/you/notes could not be copied: it \
             is not a Git repository.",
        );
        ws.apply(alert, cx);
    }),
    ("attach-alert", |ws, _, cx| {
        ws.navigate(Route::NewRun, cx);
        let alert = HostUpdate::alert(
            "Could not attach Before Orthodoxy - Shahab Ahmed.epub",
            "it is 581 KB; files up to 200 KB can be attached",
        );
        ws.apply(alert, cx);
    }),
];

/// Opens the screen `name` on the workspace; false when the demo has
/// none by that name.
pub fn open(
    name: &str,
    workspace: &mut Workspace,
    host: &DemoHost,
    cx: &mut Context<Workspace>,
) -> bool {
    let Some((_, screen)) = SCREENS.iter().find(|(known, _)| *known == name)
    else {
        return false;
    };
    screen(workspace, host, cx);
    true
}

/// Main's crew (ADR 0031): four sub-agents spawned this round, two
/// still at work in a call, one landed with two changes, one failed.
fn crew(workspace: &mut Workspace, cx: &mut Context<Workspace>) {
    use tau_agent::{
        event::{RunEvent, StopReason},
        tool::ToolOutput,
    };

    let main = run_id();
    let agents = [
        ("crew-diff", "Fix the diff parser"),
        ("crew-adr", "Update ADR 0031"),
        ("crew-tests", "Rewrite the host tests"),
        ("crew-wait", "Remove wait from tau-vcs-host"),
    ];
    for (n, (run, task)) in agents.iter().enumerate() {
        let call = format!("crew-spawn-{n}");
        let child = RunId(Arc::from(*run));
        workspace.apply_event(
            &RunEvent::ToolStart {
                run: main.clone(),
                call_id: call.as_str().into(),
                tool: Arc::from("spawn"),
                args: serde_json::json!({ "task": task }),
                parent: None,
            },
            cx,
        );
        workspace.apply_event(
            &RunEvent::RunStart {
                run: child.clone(),
                parent: Some(main.clone()),
                agent: Arc::from("coder"),
                call: Some(call.as_str().into()),
            },
            cx,
        );
        workspace.apply_event(
            &RunEvent::ToolEnd {
                run: main.clone(),
                call_id: call.as_str().into(),
                output: Arc::new(ToolOutput {
                    details: Some(serde_json::json!({ "run": run })),
                    ..ToolOutput::text("started")
                }),
                is_error: false,
                parent: None,
            },
            cx,
        );
    }
    // The two at work, each in a call.
    for (run, turn, tool, args) in [
        (
            "crew-wait",
            4,
            "edit",
            serde_json::json!({ "path": "crates/plugins/tau-vcs-host/src/sub_agents.rs" }),
        ),
        (
            "crew-tests",
            2,
            "exec_command",
            serde_json::json!({ "cmd": "cargo nextest run --release -p tau-ui" }),
        ),
    ] {
        let run = RunId(Arc::from(run));
        for turn in 1..=turn {
            workspace.apply_event(
                &RunEvent::TurnStart {
                    run: run.clone(),
                    turn,
                },
                cx,
            );
        }
        workspace.apply_event(
            &RunEvent::ToolStart {
                run: run.clone(),
                call_id: "crew-call".into(),
                tool: Arc::from(tool),
                args,
                parent: None,
            },
            cx,
        );
    }
    let end = |run: &str, stop: StopReason| RunEvent::RunEnd {
        run: RunId(Arc::from(run)),
        parent: Some(main.clone()),
        stop,
        cost: 0.0,
    };
    workspace.apply_event(
        &end("crew-diff", StopReason::Error("tests kept failing".into())),
        cx,
    );
    workspace.apply_event(&end("crew-adr", StopReason::Stop), cx);
    let change = |id: &str, description: &str| tau_vcs::ChangeInfo {
        change_id: id.repeat(32),
        commit_id: id.repeat(40),
        description: description.into(),
        empty: false,
        conflict: false,
        immutable: false,
        working_copy: false,
        divergent: false,
        bookmarks: Vec::new(),
    };
    workspace.apply(
        HostUpdate::Landed {
            run: RunId(Arc::from("crew-adr")),
            landing: Ok(tau_vcs::Landing {
                changes: vec![
                    change("k", "docs: the main chat orchestrates"),
                    change("l", "docs: amend 0026"),
                ],
                conflicts: Vec::new(),
                head: "a".repeat(40),
            }),
        },
        cx,
    );
    workspace.navigate(Route::Run(main), cx);
}

/// `plugin`'s page `page` for tau-agent, which it takes as `param`.
/// The plugins repository with three plugins, one active, one waiting
/// for the person and one whose tests fail, and a chat there writing
/// one; then their page, or `entry`'s.
fn yours(
    ws: &mut Workspace,
    host: &DemoHost,
    entry: Option<&str>,
    cx: &mut Context<Workspace>,
) {
    use tau_luau_plugins::{
        Declaration,
        Entry,
        Hooks,
        Overview,
        Standing,
        TestResult,
        Uses,
    };
    use tau_ui_kit::theme::Tone;
    use tau_ui_plugin::{CatalogEntry, Group, Link, Note};

    let test = |file: &str, name: &str, error: Option<&str>| TestResult {
        file: file.into(),
        name: name.into(),
        passed: error.is_none(),
        error: error.map(Into::into),
    };
    let plugin = |name: &str,
                  description: &str,
                  hooks: Hooks,
                  uses: Uses,
                  settings: Option<Value>,
                  standing: Standing,
                  tests: Vec<TestResult>| Entry {
        name: name.into(),
        description: description.into(),
        keeps_earlier: !matches!(standing, Standing::Active),
        standing,
        declaration: Some(Declaration {
            name: name.into(),
            description: description.into(),
            uses,
            settings,
            hooks,
            ..Default::default()
        }),
        tests,
    };
    let overview = Overview {
        commit: Some("4f2a9c1".into()),
        plugins: vec![
            plugin(
                "commit-style",
                "Keeps commit messages in the repository's style: a type, \
                 a scope, and a title in the imperative.",
                Hooks {
                    before_tool: true,
                    ..Default::default()
                },
                Uses::default(),
                None,
                Standing::Active,
                vec![
                    test("style", "accepts feat(scope): title", None),
                    test("style", "refuses a title in the past tense", None),
                    test("style", "refuses a missing scope", None),
                    test("style", "says how to fix it", None),
                ],
            ),
            plugin(
                "no-force-push",
                "Refuses a push that would rewrite a bookmark someone else \
                 may have built on.",
                Hooks {
                    before_tool: true,
                    ..Default::default()
                },
                Uses {
                    tools: vec!["exec_command".into()],
                    ..Default::default()
                },
                Some(json!({
                    "schema": { "type": "object", "properties": {
                        "protected": { "type": "string" },
                        "own_bookmarks": { "type": "boolean" },
                    } },
                    "default": { "protected": "main, release/*", "own_bookmarks": true },
                })),
                Standing::Waiting {
                    grown: vec!["the `exec_command` tool".into()],
                },
                vec![
                    test("push", "refuses a forced push to main", None),
                    test("push", "refuses a forced push to release/*", None),
                    test("push", "lets a plain push through", None),
                    test(
                        "push",
                        "lets a run force-push its own bookmark",
                        None,
                    ),
                    test("push", "says why it refused", None),
                ],
            ),
            plugin(
                "test-before-stop",
                "A run may stop only once the repository's tests pass.",
                Hooks {
                    before_stop: true,
                    ..Default::default()
                },
                Uses::default(),
                None,
                Standing::Failing,
                vec![
                    test("stop", "lets a run stop when tests pass", None),
                    test(
                        "stop",
                        "refuses a stop while tests fail",
                        Some("expected a refusal, got allow"),
                    ),
                    test("stop", "says which tests fail", None),
                ],
            ),
        ],
        error: None,
    };

    let mut catalog = ws.catalog().clone();
    let entries = overview
        .plugins
        .iter()
        .map(|entry| CatalogEntry {
            name: entry.name.clone(),
            description: entry.description.clone(),
            group: Group::Yours,
            seams: Vec::new(),
            note: match entry.standing {
                Standing::Active => None,
                Standing::Waiting { .. } => {
                    Some(Note::new("waits for you", Tone::Warn))
                }
                _ => Some(Note::new("tests fail", Tone::Danger)),
            },
        })
        .collect();
    let info = crate::catalog::PluginInfo {
        name: tau_luau_plugins::NAME.into(),
        description: "Plugins written in Luau".into(),
        group: Group::Yours,
        page: Some(Link::page(tau_luau_plugins::ui::PAGE)),
        seams: Vec::new(),
        spend: 0.0,
        note: None,
        entries,
        settings: true,
    };
    let data = PluginValue::typed(overview);
    catalog.plugins.retain(|plugin| plugin.name != info.name);
    catalog.plugins.push(info.clone());
    catalog
        .plugin_data
        .insert(tau_luau_plugins::NAME.into(), data.clone());
    host.pin(info, data);
    let chat = RunId("plugins-commit-style".into());
    if catalog.own_repo().is_none() {
        catalog.repos.push(crate::catalog::Repo {
            name: tau_luau_plugins::REPO.into(),
            main: Some(chat.clone()),
            own: Some(crate::catalog::Own::Plugins),
            ..Default::default()
        });
    }
    ws.set_catalog(catalog, cx);
    let mut run = RunView::new(
        chat.clone(),
        "Teach commit-style scopes",
        "coder",
        "gpt-6.1-sol",
    )
    .in_repo(tau_luau_plugins::REPO);
    run.turn = 3;
    ws.apply(HostUpdate::Run(Box::new(run)), cx);
    let mut params = std::collections::BTreeMap::new();
    if let Some(entry) = entry {
        params.insert(tau_ui_plugin::ENTRY.to_owned(), entry.to_owned());
    }
    ws.navigate(
        Route::Plugin {
            plugin: tau_luau_plugins::NAME.into(),
            page: tau_luau_plugins::ui::PAGE.into(),
            params,
        },
        cx,
    );
}

/// Which of the agents' screens to open.
enum AgentsAt {
    Overview,
    Conversation,
    New,
    Home,
    Details,
}

/// Three agents (ADR 0037): pr-triage at work, with a sub-agent and an
/// event message in its conversation; morning-digest waiting for its
/// schedule; release-notes at its cap. Then the screen `at` names.
fn agents(
    ws: &mut Workspace,
    host: &DemoHost,
    at: AgentsAt,
    cx: &mut Context<Workspace>,
) {
    use tau_agents::{
        Agent,
        Last,
        Overview,
        Status,
        TestResult,
        Trigger,
        TriggerKind,
        Version,
        event::{Batch, Event, Kind},
    };
    use tau_ai::message::{
        AssistantBlock,
        AssistantMessage,
        Message,
        StopReason,
        TextContent,
        Usage,
        UserContent,
        UserMessage,
    };
    use tau_ui_plugin::Group;

    let now = tau_agents::shown::now();
    let trigger = |name: &str,
                   kind: TriggerKind,
                   last: Option<i64>,
                   next: Option<i64>| Trigger {
        name: name.into(),
        kind,
        last,
        next,
        error: None,
        live: None,
    };
    let tests = |passed: usize, failed: usize| {
        (0..passed + failed)
            .map(|n| TestResult {
                file: "triggers".into(),
                name: format!("case {n}"),
                passed: n < passed,
                error: (n >= passed).then(|| "expected 1 event, got 2".into()),
            })
            .collect::<Vec<_>>()
    };
    let main = |name: &str| RunId(format!("agent-{name}").into());
    let agent = |name: &str, purpose: &str, status: Status| Agent {
        name: name.into(),
        purpose: purpose.into(),
        status,
        triggers: Vec::new(),
        versions: Vec::new(),
        spend_today: 0.0,
        cap_today: 1.0,
        queued: 0,
        last: None,
        model: tau_agents::default_model(),
        effort: tau_agents::DEFAULT_EFFORT.into(),
        main: Some(main(name).0.to_string()),
        secrets: Vec::new(),
        sandbox: None,
        notices: Vec::new(),
        scheduled: Vec::new(),
    };
    let overview = Overview {
        agents: vec![
            Agent {
                triggers: vec![
                    trigger(
                        "evening",
                        TriggerKind::Schedule {
                            every: None,
                            at: Some("08:30".into()),
                            days: Vec::new(),
                        },
                        Some(now - 30 * 3600),
                        Some(now + 17 * 3600),
                    ),
                    trigger("runs", TriggerKind::Message, None, None),
                ],
                versions: vec![Version {
                    commit: "9c1d2e0a".into(),
                    summary: "Group the digest by repository".into(),
                    tests: tests(4, 0),
                    live: true,
                    error: None,
                }],
                spend_today: 0.08,
                notices: vec![tau_agents::Notice {
                    id: "n3".into(),
                    at: now - 40 * 60,
                    text: "Which repositories should the digest leave out?"
                        .into(),
                    choices: Some(Vec::new()),
                }],
                last: Some(Last {
                    at: now - 6 * 3600,
                    line: "caught up at 08:41: tau was closed at 08:30".into(),
                }),
                ..agent(
                    "morning-digest",
                    "Each morning, a page of what landed yesterday across my \
                     repositories, what failed, and the chats I left half way.",
                    Status::Waiting,
                )
            },
            Agent {
                triggers: vec![
                    trigger(
                        "pulls",
                        TriggerKind::Poll {
                            source: "mcp__github__list_pull_requests".into(),
                            every: "5m".into(),
                            tool: Some(
                                "mcp__github__list_pull_requests".into(),
                            ),
                            args: None,
                            http: None,
                        },
                        Some(now - 240),
                        Some(now + 60),
                    ),
                    trigger(
                        "triage",
                        TriggerKind::Command {
                            command: "triage".into(),
                            hint: Some("<number>".into()),
                        },
                        Some(now - 86_400),
                        None,
                    ),
                    trigger(
                        "evening",
                        TriggerKind::Schedule {
                            every: None,
                            at: Some("18:00".into()),
                            days: ["Mon", "Tue", "Wed", "Thu", "Fri"]
                                .map(Into::into)
                                .to_vec(),
                        },
                        None,
                        Some(now + 4 * 3600),
                    ),
                ],
                versions: vec![
                    Version {
                        commit: "7f3a2c91".into(),
                        summary: "Poll every 5 min in work hours".into(),
                        tests: tests(11, 0),
                        live: true,
                        error: None,
                    },
                    Version {
                        commit: "51be09d4".into(),
                        summary: "Parse the review stream".into(),
                        tests: tests(9, 2),
                        live: false,
                        error: Some("2 tests fail".into()),
                    },
                    Version {
                        commit: "0a77c3f2".into(),
                        summary: "Its own triage module".into(),
                        tests: tests(9, 0),
                        live: false,
                        error: None,
                    },
                ],
                spend_today: 0.41,
                queued: 2,
                scheduled: vec![
                    tau_agents::Scheduled {
                        name: "recheck-409".into(),
                        note: "See whether #409 gained a test for a reused \
                               refresh token; if not, comment on it."
                            .into(),
                        when: tau_agents::When::Once { at: now + 3 * 3600 },
                        next: now + 3 * 3600,
                        last: None,
                    },
                    tau_agents::Scheduled {
                        name: "stale-sweep".into(),
                        note: "Ping the authors of pull requests idle for a \
                               week."
                            .into(),
                        when: tau_agents::When::Daily {
                            at: "10:00".into(),
                            days: vec!["Fri".into()],
                        },
                        next: now + 3 * 86_400,
                        last: None,
                    },
                ],
                notices: vec![
                    tau_agents::Notice {
                        id: "n1".into(),
                        at: now - 25 * 60,
                        text: "#409 rotates refresh tokens on every use; the \
                               sub-agent found no test for a token reused \
                               after rotation."
                            .into(),
                        choices: None,
                    },
                    tau_agents::Notice {
                        id: "n2".into(),
                        at: now - 8 * 60,
                        text: "Close #398 as a duplicate of #377? Same stack \
                               trace and the same failing test."
                            .into(),
                        choices: Some(vec![
                            "Close it".into(),
                            "Leave it open".into(),
                        ]),
                    },
                ],
                last: Some(Last {
                    at: now - 240,
                    line: "labelled #412; reviewing #409".into(),
                }),
                secrets: vec![
                    tau_agents::SecretState {
                        name: "ci".into(),
                        set: false,
                    },
                    tau_agents::SecretState {
                        name: "github".into(),
                        set: true,
                    },
                ],
                ..agent(
                    "pr-triage",
                    "Labels new pull requests on tau-agent, says what each one \
                     touches, and sends a sub-agent to review any that change \
                     auth or storage.",
                    Status::Working,
                )
            },
            Agent {
                triggers: vec![trigger(
                    "notes",
                    TriggerKind::Command {
                        command: "release-notes".into(),
                        hint: None,
                    },
                    None,
                    None,
                )],
                spend_today: 1.0,
                queued: 3,
                last: Some(Last {
                    at: now - 2 * 3600,
                    line: "reached its $1.00 for today".into(),
                }),
                ..agent(
                    "release-notes",
                    "Keeps a draft of the next release's notes from every \
                     landing on main, and turns it into a changelog entry on \
                     /release-notes.",
                    Status::AtCap,
                )
            },
        ],
    };
    let mut catalog = ws.catalog().clone();
    let info = crate::catalog::PluginInfo {
        name: tau_agents::NAME.into(),
        description: "Agents that keep going on their own".into(),
        group: Group::Tools,
        page: Some(tau_ui_plugin::Link::page(tau_agents::ui::OVERVIEW)),
        seams: Vec::new(),
        spend: 0.0,
        note: None,
        entries: Vec::new(),
        settings: true,
    };
    let data = PluginValue::typed(overview.clone());
    catalog.plugins.retain(|plugin| plugin.name != info.name);
    catalog.plugins.push(info.clone());
    catalog
        .plugin_data
        .insert(tau_agents::NAME.into(), data.clone());
    host.pin(info, data);
    // The person's MCP servers, for the New agent form to offer: no
    // server answers in a demo.
    if let Some(mcp) = catalog
        .plugins
        .iter()
        .find(|plugin| plugin.name == tau_mcp_host::NAME)
        .cloned()
    {
        let servers = PluginValue::typed(tau_mcp::demo::servers());
        catalog
            .plugin_data
            .insert(tau_mcp_host::NAME.into(), servers.clone());
        host.pin(mcp, servers);
    }
    for agent in &overview.agents {
        if catalog.repo(&agent.name).is_none() {
            catalog.repos.push(crate::catalog::Repo {
                name: agent.name.clone(),
                main: Some(main(&agent.name)),
                own: Some(crate::catalog::Own::Agent),
                ..Default::default()
            });
        }
    }
    ws.set_catalog(catalog, cx);

    // pr-triage's conversation: a batch of events, and its answer.
    let event =
        |trigger: &str, kind: Kind, id: &str, ago: i64, data: Value| Event {
            trigger: trigger.into(),
            kind,
            id: id.into(),
            at: now - ago,
            data,
        };
    let batch = Batch {
        events: vec![
            event(
                "pulls",
                Kind::Poll,
                "pulls:412",
                75,
                serde_json::json!({
                    "number": 412,
                    "title": "Fold the plugins tree on narrow windows",
                    "author": "cfcosta",
                }),
            ),
            event(
                "pulls",
                Kind::Poll,
                "pulls:409",
                62,
                serde_json::json!({
                    "number": 409,
                    "title": "Rotate refresh tokens on every use",
                    "author": "a-contributor",
                }),
            ),
            event(
                "triage",
                Kind::Command,
                "cmd:9f2c",
                48,
                serde_json::json!({
                    "args": "398",
                    "from": "tau-agent",
                }),
            ),
        ],
    };
    let text = |text: &str| {
        Message::Assistant(AssistantMessage {
            content: vec![AssistantBlock::Text(TextContent {
                text: text.into(),
                text_signature: None,
            })],
            model: tau_agents::default_model(),
            response_id: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 0,
        })
    };
    let user = |text: String| {
        Message::User(UserMessage {
            content: UserContent::Text(text),
            timestamp: 0,
        })
    };
    let messages = [
        user(
            "Watch new pull requests on tau-agent, label them, and tell me \
              when one touches auth."
                .into(),
        ),
        text(
            "Set up: `agent.luau` polls `list_pull_requests` every 10 minutes \
              and answers `/triage <number>`. My instructions are in AGENTS.md.",
        ),
        user(batch.message(0)),
        text(
            "#412 is small and UI only: labelled `area/ui` and `size/s`. #409 \
              rotates refresh tokens, so a sub-agent is reviewing it now. #398 \
              looks like a duplicate of #377: same stack trace and failing test.",
        ),
    ];
    let conversation = RunView::from_messages(
        main("pr-triage"),
        "pr-triage",
        "coder",
        tau_agents::default_model(),
        &messages,
    )
    .in_repo("pr-triage");
    ws.apply(HostUpdate::Run(Box::new(conversation)), cx);
    let review = RunView::new(
        RunId("agent-pr-triage-review".into()),
        "Review #409's token rotation",
        "coder",
        tau_agents::default_model(),
    )
    .in_repo("pr-triage")
    .with_origin(crate::view::Origin::SubAgent {
        parent: main("pr-triage"),
    });
    ws.apply(HostUpdate::Run(Box::new(review)), cx);
    for name in ["morning-digest", "release-notes"] {
        let mut run = RunView::new(
            main(name),
            name,
            "coder",
            tau_agents::default_model(),
        )
        .in_repo(name);
        run.finish_stored(tau_agent::event::StopReason::Stop, 0.0, 0.0);
        ws.apply(HostUpdate::Run(Box::new(run)), cx);
    }
    ws.toggle_agent_open("pr-triage", cx);
    match at {
        AgentsAt::Overview => {
            ws.navigate(Route::agents(tau_agents::ui::OVERVIEW), cx)
        }
        AgentsAt::Conversation => ws.open_agent("pr-triage", cx),
        AgentsAt::New => ws.navigate(Route::agents(tau_agents::ui::NEW), cx),
        AgentsAt::Home => ws.navigate(Route::Home, cx),
        AgentsAt::Details => {
            ws.open_agent("pr-triage", cx);
            ws.toggle_sheet(cx);
        }
    }
}

fn plugin_page(
    workspace: &mut Workspace,
    plugin: &str,
    page: &str,
    param: &str,
    cx: &mut Context<Workspace>,
) {
    let route = Route::Plugin {
        plugin: plugin.into(),
        page: page.into(),
        params: [(param.to_owned(), "tau-agent".to_owned())].into(),
    };
    workspace.navigate(route, cx);
}

fn setup_at(
    workspace: &mut Workspace,
    step: SetupStep,
    cx: &mut Context<Workspace>,
) {
    workspace.set_setup(setup(step), cx);
    workspace.navigate(Route::Setup(step), cx);
}

/// Onboarding's model step with the model reached as `model`.
fn model_at(
    workspace: &mut Workspace,
    model: ModelAccess,
    cx: &mut Context<Workspace>,
) {
    workspace.set_setup(
        Setup {
            model,
            ..setup(SetupStep::Model)
        },
        cx,
    );
    workspace.navigate(Route::Setup(SetupStep::Model), cx);
}

fn pair_at(
    workspace: &mut Workspace,
    step: PairStep,
    cx: &mut Context<Workspace>,
) {
    workspace.start_pairing(PairStep::Welcome, cx);
    workspace.set_pairing(pairing(step), cx);
    if step != PairStep::Welcome {
        workspace.navigate(Route::Pair(step), cx);
    }
}

fn pull_request_at(
    workspace: &mut Workspace,
    pr: PullRequest,
    cx: &mut Context<Workspace>,
) {
    let run = run_id();
    let pr = Box::new(pr);
    workspace.apply(
        HostUpdate::PullRequest {
            run: run.clone(),
            pr,
        },
        cx,
    );
    workspace.open_pull_request(&run, cx);
}

/// The demo run's tau-vcs card `call`, open, with `file` open in it.
fn vcs_card(
    workspace: &mut Workspace,
    call: &str,
    file: &str,
    cx: &mut Context<Workspace>,
) {
    let run = run_id();
    workspace.navigate(Route::Run(run.clone()), cx);
    workspace.toggle_card(&run, call, cx);
    if let Some(cards) =
        workspace.plugin_ui::<tau_vcs::ui::Ui>(tau_vcs::ui::NAME)
    {
        cards.update(cx, |cards, _| cards.toggle_file(&run, call, file));
    }
}

/// tau-agent's rules page in the state `name` names, which
/// tau-constitution opens; a rule is tried on the demo run's shell
/// commands.
fn rules_at(
    workspace: &mut Workspace,
    name: &str,
    cx: &mut Context<Workspace>,
) {
    plugin_page(workspace, tau_constitution_host::NAME, "rules", "repo", cx);
    let Some(ui) = workspace.plugin_ui::<tau_constitution::ui::page::Ui>(
        tau_constitution_host::NAME,
    ) else {
        return;
    };
    let calls = workspace
        .run(&run_id())
        .map(|run| run.cards())
        .unwrap_or_default()
        .into_iter()
        .filter(|card| card.tool == "exec_command")
        .map(|card| (card.tool, card.args))
        .collect();
    tau_constitution_host::demo::open(name, &ui, "tau-agent", calls, cx);
}

/// The demo run working on [`GOAL`], with the records tau-goal
/// published by `state`.
/// A message of the demo run that kept medium reasoning over Jev's pick
/// of high, its conversation's prefix being 48k tokens.
fn effort_kept(workspace: &mut Workspace, cx: &mut Context<Workspace>) {
    let body = serde_json::json!({
        "kind": "choice", "verdict": "chose", "effort": "high",
        "confidence": 0.81, "threshold": 0.7, "cost": 0.00002,
        "runs_at": "medium", "kept_for_cache": 48_213,
        "levels": [
            { "effort": "none", "suits": "no thought", "p": 0.01 },
            { "effort": "low", "suits": "small edits", "p": 0.03 },
            { "effort": "medium", "suits": "routine code", "p": 0.11 },
            { "effort": "high", "suits": "refactors", "p": 0.81 },
            { "effort": "xhigh", "suits": "audits, proofs", "p": 0.04 },
        ],
    });
    let event = RunEvent::PluginReport {
        run: run_id(),
        plugin: tau_reasoning::NAME.into(),
        body,
    };
    workspace.apply(HostUpdate::Event(event), cx);
}

fn goal_at(
    workspace: &mut Workspace,
    state: &str,
    cx: &mut Context<Workspace>,
) {
    workspace.navigate(Route::Run(run_id()), cx);
    for body in tau_goal::demo::records(GOAL, state) {
        let event = RunEvent::PluginReport {
            run: run_id(),
            plugin: tau_goal::NAME.into(),
            body,
        };
        workspace.apply(HostUpdate::Event(event), cx);
    }
}

/// The live demo run waiting on tau-ask's questions, its panel in the
/// state `open` names.
fn ask(workspace: &mut Workspace, open: &str, cx: &mut Context<Workspace>) {
    use tau_ask::{Ask, Record, ui::Key};
    let run = run_id();
    let ask: Ask = serde_json::from_value(serde_json::json!({ "questions": [
        {
            "question": "How should the ask tool wait for your answer?",
            "header": "Waiting",
            "options": [
                {
                    "label": "Hold the call open (Recommended)",
                    "description": "The tool blocks on a channel; your answer comes back as its result and the turn goes on.",
                    "preview": "let (tx, rx) = oneshot::channel();\nself.pending.insert(ctx.call_id, tx);\nselect! {\n    a = rx => Ok(a?),\n    _ = ctx.cancel => Err(Cancelled),\n}"
                },
                {
                    "label": "End the turn",
                    "description": "Like vcs_land: the run stops, and your answer arrives as the next message.",
                    "preview": "ctx.publish(&asked)?;\nOk(ToolOutput::text(\"Asked; waiting.\"))"
                },
                {
                    "label": "Hold, then time out",
                    "description": "Block for a while; if nobody answers, stop the run and keep the panel open.",
                    "preview": "select! {\n    a = rx => Ok(a?),\n    _ = sleep(timeout) => stop(),\n}"
                }
            ]
        },
        {
            "question": "Where else should a pending question show up?",
            "header": "Surfaces",
            "multi_select": true,
            "options": [
                { "label": "Run list badge", "description": "An amber dot on the run in the sidebar until you answer." },
                { "label": "Desktop notification", "description": "Only when the window is not focused." },
                { "label": "Parent run", "description": "A child run's question surfaces on the run that spawned it." }
            ]
        },
        {
            "question": "How long should a question hold the run?",
            "header": "Timeout",
            "options": [
                { "label": "5 minutes", "description": "Short; the run stops soon and resumes when you answer." },
                { "label": "Never", "description": "Hold until you answer or cancel the run." }
            ]
        }
    ]}))
    .expect("the demo's questions read");
    workspace.navigate(Route::Run(run.clone()), cx);
    workspace.apply(
        HostUpdate::PluginFold {
            run: run.clone(),
            plugin: tau_ask::NAME.into(),
            body: Record::Asked {
                call: "call_ask".into(),
                ask: ask.clone(),
            }
            .to_value(),
        },
        cx,
    );
    let Some(ui) = workspace.plugin_ui::<tau_ask::ui::Ui>(tau_ask::NAME) else {
        return;
    };
    ui.update(cx, |ui, _| {
        let key = (run.0.to_string(), "call_ask".to_owned());
        let draft = ui.draft_for(&key, &ask);
        if open == "ask" {
            return;
        }
        draft.key(&ask, Key::Digit(1));
        draft.key(&ask, Key::Digit(1));
        draft.key(&ask, Key::Digit(3));
        draft.notes[1] = "Only when the run has waited a while.".into();
        if open == "ask-note" {
            draft.note_open = true;
            return;
        }
        draft.key(&ask, Key::Right);
        draft.key(&ask, Key::Digit(2));
    });
}

/// The demo run with a note from the watcher under its last step, and
/// the answer the person gave it, if any.
fn watcher(
    workspace: &mut Workspace,
    answer: Option<tau_watcher::record::Answer>,
    cx: &mut Context<Workspace>,
) {
    use tau_watcher::record::{Explain, Record, Tag};
    let run = run_id();
    workspace.navigate(Route::Run(run.clone()), cx);
    // The note anchors at the end of the transcript so far, so it waits
    // for the script to play out.
    let played: std::time::Duration =
        super::script().iter().map(|(wait, _)| *wait).sum();
    let note = Record::Noted {
        step: 12,
        tag: Tag::HeadsUp,
        line: "The agent now counts queued forks in the \u{201c}N need \
                   you\u{201d} pill, so the pill can show a number when \
                   nothing needs a decision."
            .into(),
        explain: Some(Explain {
            title: "Queued chats now raise the pill".into(),
            bullets: vec![
                "A fork waiting in the queue counts as needing you, \
                     even though it asks for no decision."
                    .into(),
                "The pill can read \u{201c}2 need you\u{201d} while every \
                     chat is only waiting to start."
                    .into(),
                "Running a queued chat, or removing it, lowers the count \
                     again."
                    .into(),
                "The change is in the sidebar's attention states, not in \
                     the chats themselves."
                    .into(),
            ],
        }),
    };
    cx.spawn(async move |this, cx| {
        cx.background_executor()
            .timer(played + std::time::Duration::from_millis(300))
            .await;
        let records = std::iter::once(note).chain(answer.map(|answer| {
            Record::Answered {
                key: "n0".into(),
                answer,
            }
        }));
        for record in records {
            let update = HostUpdate::PluginFold {
                run: run.clone(),
                plugin: tau_watcher::record::NAME.into(),
                body: serde_json::to_value(record).expect("plain JSON"),
            };
            if this.update(cx, |ws, cx| ws.apply(update, cx)).is_err() {
                return;
            }
        }
    })
    .detach();
}

/// tau-agent's main chat with a `skill` call: release-notes loaded, its
/// card open, or a skill not there.
fn skill(workspace: &mut Workspace, found: bool, cx: &mut Context<Workspace>) {
    let run = run_id();
    let call = "call_skill".to_owned();
    let name = if found {
        "release-notes"
    } else {
        "deploy-prod"
    };
    workspace.navigate(Route::Run(run.clone()), cx);
    workspace.apply(
        HostUpdate::Event(RunEvent::ToolStart {
            run: run.clone(),
            call_id: call.clone(),
            tool: tau_skills::TOOL.into(),
            args: serde_json::json!({ "name": name }),
            parent: None,
        }),
        cx,
    );
    let output = if found {
        let mut output = tau_agent::tool::ToolOutput::text(
            "Skill release-notes. Its folder, where the files it names are: \
             ~/.agents/skills/release-notes\n\nRun scripts/commits.sh with \
             the two tags, group what it lists, and lead with what users \
             will notice.",
        );
        output.details = Some(
            serde_json::to_value(tau_skills::Loaded {
                name: name.into(),
                description: "Writes release notes from the commits between \
                              two tags: groups by kind, leads with what users \
                              notice."
                    .into(),
                file: "~/.agents/skills/release-notes/SKILL.md".into(),
            })
            .expect("plain JSON"),
        );
        output
    } else {
        tau_agent::tool::ToolOutput::text(
            "There is no skill \"deploy-prod\". The skills are: code-review, \
             release-notes.",
        )
    };
    workspace.apply(
        HostUpdate::Event(RunEvent::ToolEnd {
            run: run.clone(),
            call_id: call.clone(),
            output: std::sync::Arc::new(output),
            is_error: !found,
            parent: None,
        }),
        cx,
    );
    if found {
        workspace.toggle_card(&run, &call, cx);
    }
}

/// tau-agent's main chat with a `codemode` script that read a project's
/// files: its card open, its first output item too.
fn codemode(workspace: &mut Workspace, cx: &mut Context<Workspace>) {
    let run = run_id();
    let call = "call_codemode".to_owned();
    let reads = [
        (
            "README.md",
            "# Ascend\n\nThe public product home page is at `/`; \
                       `/app` opens the dashboard.\n\n## Development\n\n\
                       Run `bun run dev`.\n",
        ),
        (
            "package.json",
            "{\n  \"name\": \"ascend\",\n  \"private\": true\n}\n",
        ),
    ];
    let code = "text(tools.read({path='README.md'})); \
                text(tools.read({path='package.json'})); \
                text(tools.ls({path='docs'})); text(tools.vcs_status({}))";
    workspace.navigate(Route::Run(run.clone()), cx);
    workspace.apply(
        HostUpdate::Event(RunEvent::ToolStart {
            run: run.clone(),
            call_id: call.clone(),
            tool: tau_codemode::description::NAME.into(),
            args: serde_json::json!({ "code": code }),
            parent: None,
        }),
        cx,
    );
    let mut output: Vec<serde_json::Value> = reads
        .iter()
        .map(|(path, text)| {
            serde_json::json!({
                "kind": "json",
                "value": {
                    "complete": true, "kind": "text", "path": path,
                    "returned_lines": text.lines().count(), "text": text,
                },
            })
        })
        .collect();
    output.push(serde_json::json!({
        "kind": "json",
        "value": { "entries": [
            { "kind": "file", "name": "authentication.md" },
            { "kind": "file", "name": "deployment.md" },
            { "kind": "file", "name": "metrics.md" },
        ], "path": "docs" },
    }));
    output.push(serde_json::json!({
        "kind": "json",
        "value": { "change": "qystttnm", "clean": true, "parent": "main" },
    }));
    let row = |id: &str, name: &str, args: &str| {
        serde_json::json!({
            "id": id, "name": name, "args": args, "status": "ok", "ms": 10,
            "error": null, "cost": null, "usage_uncertain": false,
        })
    };
    let mut result = tau_agent::tool::ToolOutput::text(
        "Script completed\nWall time 0.0 seconds\nOutput:\n",
    );
    result.details = Some(serde_json::json!({
        "calls": [
            row("call_codemode/1", "read", r#"{"path":"README.md"}"#),
            row("call_codemode/2", "read", r#"{"path":"package.json"}"#),
            row("call_codemode/3", "ls", r#"{"path":"docs"}"#),
            row("call_codemode/4", "vcs_status", "{}"),
        ],
        "complete": true,
        "output": output,
        "store": null,
        "usage": tau_ai::message::Usage::default(),
        "wall_ms": 35,
    }));
    workspace.apply(
        HostUpdate::Event(RunEvent::ToolEnd {
            run: run.clone(),
            call_id: call.clone(),
            output: std::sync::Arc::new(result),
            is_error: false,
            parent: None,
        }),
        cx,
    );
    workspace.toggle_card(&run, &call, cx);
    if let Some(ui) = workspace
        .plugin_ui::<tau_codemode::ui::InspectorUi>(tau_codemode::PLUGIN)
    {
        ui.update(cx, |ui, _| {
            ui.toggle(tau_codemode::outline::key(&call, 0));
        });
    }
}

/// tau-agent's main chat with its model asked and no answer yet: what it
/// reasoned so far, when it streams any.
fn reasoning(
    workspace: &mut Workspace,
    thought: Option<&str>,
    cx: &mut Context<Workspace>,
) {
    let run = run_id();
    workspace.navigate(Route::Run(run.clone()), cx);
    workspace.apply(
        HostUpdate::Event(RunEvent::TurnStart {
            run: run.clone(),
            turn: 4,
        }),
        cx,
    );
    if let Some(thought) = thought {
        workspace.apply(
            HostUpdate::Event(RunEvent::ThinkingDelta {
                run,
                delta: thought.into(),
            }),
            cx,
        );
    }
}

/// tau-agent's main chat with tau-direnv's `record` folded in.
fn envrc(
    workspace: &mut Workspace,
    record: tau_direnv::Record,
    cx: &mut Context<Workspace>,
) {
    let run = run_id();
    workspace.navigate(Route::Run(run.clone()), cx);
    workspace.apply(
        HostUpdate::PluginFold {
            run,
            plugin: tau_direnv::NAME.into(),
            body: serde_json::to_value(record).expect("a record serializes"),
        },
        cx,
    );
}

/// Main's queue as the canvas draws it: the demo's fork, clean, then a
/// chat with a conflict the person confirmed.
fn land_queue(ws: &mut Workspace, cx: &mut Context<Workspace>) {
    let fork = fork_id();
    let title = ws
        .run(&fork)
        .map_or_else(|| "Queue landings".to_owned(), |view| view.title.clone());
    let host_rs = "crates/tau-ui/src/host.rs".to_owned();
    let queue = vec![
        crate::queue::Waiting {
            run: fork.0.to_string(),
            title,
            changes: 4,
            conflicts: Vec::new(),
            confirmed: Vec::new(),
            sub_agent: None,
        },
        crate::queue::Waiting {
            run: "load-agents".into(),
            title: "Load AGENTS.md".into(),
            changes: 2,
            conflicts: vec![host_rs.clone()],
            confirmed: vec![host_rs],
            sub_agent: None,
        },
    ];
    ws.apply(
        HostUpdate::LandingQueue {
            main: run_id(),
            queue,
            conflicts: None,
        },
        cx,
    );
}

/// tau-agent's main chat with a chat in each state its row can show:
/// working, asking, ready to land, queued to land, would conflict,
/// interrupted, and one that landed and closed; and docbert's main chat
/// with conflicts a turn left on it.
fn sidebar_states(workspace: &mut Workspace, cx: &mut Context<Workspace>) {
    use tau_ui_remote::attention::Forecast;
    let main = run_id();
    let chat = |name: &str, title: &str, stop: Option<StopReason>| {
        let mut view =
            RunView::new(RunId(name.into()), title, "coder", "gpt-5.5")
                .in_repo("tau-agent")
                .started("today 09:12")
                .with_origin(Origin::Fork {
                    from: main.clone(),
                    turn: 0,
                });
        view.turn = 7;
        if let Some(stop) = stop {
            view.finish_stored(stop, 0.12, 0.0);
        }
        view
    };
    let done = || Some(StopReason::Stop);
    // Oldest first: each goes on top of the list.
    let chats = [
        chat("raise-turn-limit", "Raise turn limit", done()),
        {
            // tau closed while it ran.
            let mut view = chat("atomic-landing", "Atomic landing", None);
            view.interrupted_stored(0.12, 0.0);
            view
        },
        chat("sidebar-states", "Sidebar run states", done()),
        chat("queue-landings", "Queue landings", done()),
        chat("load-agents", "Load AGENTS.md", done()),
        chat("sweep-workspaces", "Sweep orphan workspaces", None),
        chat("chat-prs", "Base chat PRs on origin", None),
    ];
    for view in chats {
        workspace.apply(HostUpdate::Run(Box::new(view)), cx);
    }
    let forecast =
        |run: &str, changes, conflicts: &[&str]| HostUpdate::Forecast {
            run: RunId(run.into()),
            forecast: Some(Forecast {
                changes,
                conflicts: conflicts
                    .iter()
                    .map(|path| (*path).to_owned())
                    .collect(),
            }),
        };
    workspace.apply(forecast("load-agents", 2, &[]), cx);
    workspace.apply(
        forecast(
            "sidebar-states",
            3,
            &["crates/tau-ui-remote/src/ui/chrome.rs", "docs/plan.md"],
        ),
        cx,
    );
    let ask: tau_ask::Ask = serde_json::from_value(serde_json::json!({
        "questions": [{
            "question": "Delete the 3 workspaces with no run, or keep them?",
            "header": "Workspaces",
            "options": [
                { "label": "Delete them", "description": "No run uses them." },
                { "label": "Keep them", "description": "Look at them first." }
            ]
        }]
    }))
    .expect("the demo's question reads");
    workspace.apply(
        HostUpdate::PluginFold {
            run: RunId("sweep-workspaces".into()),
            plugin: tau_ask::NAME.into(),
            body: tau_ask::Record::Asked {
                call: "call_sweep".into(),
                ask,
            }
            .to_value(),
        },
        cx,
    );
    // One landed: its chat closed, and main has its card.
    workspace.apply(
        HostUpdate::Landed {
            run: RunId("raise-turn-limit".into()),
            landing: Ok(landing_preview(false)),
        },
        cx,
    );
    // One waits for main's turn to land.
    workspace.apply(
        HostUpdate::LandingQueue {
            main: main.clone(),
            queue: vec![crate::queue::Waiting {
                run: "queue-landings".into(),
                title: "Queue landings".into(),
                changes: 3,
                conflicts: Vec::new(),
                confirmed: Vec::new(),
                sub_agent: None,
            }],
            conflicts: None,
        },
        cx,
    );
    // docbert's main: a turn left conflicts on it.
    workspace.apply(
        HostUpdate::LandingQueue {
            main: RunId("docbert-main".into()),
            queue: Vec::new(),
            conflicts: Some(crate::queue::MainConflicts {
                files: vec!["src/rerank.rs".into(), "src/index.rs".into()],
                from: Some("rerank-latency".into()),
                prompt: "Landing `rerank-latency` left conflicts.".into(),
                dismissed: false,
            }),
        },
        cx,
    );
    for repo in ["tau-agent", "docbert"] {
        if !workspace.is_repo_open(repo) {
            workspace.toggle_repo_open(repo, cx);
        }
    }
    // Every chat listed, down to the landed one, the oldest.
    workspace.show_older_runs("tau-agent", cx);
    // On main; a phone shows its list of runs.
    workspace.navigate(Route::Run(main), cx);
    workspace.navigate(Route::Home, cx);
}
