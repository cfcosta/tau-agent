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
    ("history", |ws, _, cx| ws.navigate(Route::History, cx)),
    ("plugins", |ws, _, cx| ws.navigate(Route::Plugins, cx)),
    ("models", |ws, _, cx| ws.navigate(Route::Models, cx)),
    ("plan", |ws, _, cx| ws.navigate(Route::Plan(run_id()), cx)),
    ("compare", |ws, _, cx| {
        let (main, fork) = (run_id(), fork_id());
        ws.navigate(Route::Compare { main, fork }, cx)
    }),
    ("memory", |ws, _, cx| {
        plugin_page(ws, tau_memory::plugin::NAME, "notes", "repo", cx)
    }),
    ("constitution", |ws, _, cx| {
        plugin_page(ws, tau_constitution::NAME, "rules", "repo", cx)
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
        };
        ws.apply(HostUpdate::LandingFinished(record), cx);
        ws.navigate(Route::Run(run_id()), cx);
    }),
    // tau-ask's panel in the composer's place (ADR 0019): a question
    // with previews; a checklist with a note being written; the review.
    ("ask", |ws, _, cx| ask(ws, "ask", cx)),
    ("ask-note", |ws, _, cx| ask(ws, "ask-note", cx)),
    ("ask-review", |ws, _, cx| ask(ws, "ask-review", cx)),
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
    // The repository tree's states.
    ("repo-open", |ws, _, cx| ws.toggle_repo_open("docbert", cx)),
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
                tau_constitution::NAME.into(),
                PluginValue::typed(tau_constitution::demo::broken()),
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
        if let Err(error) = host.act(tau_constitution::NAME, reset) {
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

/// `plugin`'s page `page` for tau-agent, which it takes as `param`.
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
    plugin_page(workspace, tau_constitution::NAME, "rules", "repo", cx);
    let Some(ui) = workspace
        .plugin_ui::<tau_constitution::ui::page::Ui>(tau_constitution::NAME)
    else {
        return;
    };
    let calls = workspace
        .run(&run_id())
        .map(|run| run.cards())
        .unwrap_or_default()
        .into_iter()
        .filter(|card| card.tool == "bash")
        .map(|card| (card.tool, card.args))
        .collect();
    tau_constitution::demo::open(name, &ui, "tau-agent", calls, cx);
}

/// The demo run working on [`GOAL`], with the records tau-goal
/// published by `state`.
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
