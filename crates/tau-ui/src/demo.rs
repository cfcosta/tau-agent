//! A scripted session for running the interface without an agent: the
//! `retry-after` run from the design mockups, as the run events a real
//! run would stream, plus the plugin updates no event carries yet.

use std::{sync::Arc, time::Duration};

use gpui::{App, Entity};
use serde_json::{Value, json};
use tau_agent::{
    event::{RunEvent, StopReason},
    tool::{RunId, ToolOutput},
};
use tau_ai::message::{Usage, UsageCost};

use crate::{
    Workspace,
    WorkspaceEvent,
    catalog::{
        Catalog,
        Constitution,
        JevStats,
        Link,
        Memory,
        Note,
        PluginInfo,
        PluginScreen,
        Repo,
        Rule,
        Seam,
        StoreInfo,
    },
    pull_request::{Checks, PrCommit, PrState, PullRequest},
    setup::{
        CloneState,
        DeviceCode,
        GitHub,
        ModelAccess,
        RepoChoice,
        RepoClone,
        Setup,
        SetupStep,
        SetupUpdate,
    },
    view::{
        BranchCode,
        ContextWindow,
        Decision,
        FileChange,
        FileKind,
        FileStat,
        LedgerEntry,
        Limits,
        NoteBody,
        Origin,
        PlanField,
        PluginNote,
        PluginStatus,
        Proposal,
        RunStatus,
        RunUpdate,
        RunView,
        Tone,
    },
};

/// One scripted update, and how long to wait before it.
pub type Step = (Duration, RunUpdate);

pub fn run_id() -> RunId {
    RunId(Arc::from("retry-after"))
}

/// The run as it looks before its first event.
pub fn retry_after() -> RunView {
    let mut view = RunView::new(run_id(), "retry-after", "coder", "gpt-5.5")
        .in_repo("tau-agent")
        .started("today 04:31");
    view.limits = Limits {
        max_turns: Some(20),
        max_tokens: Some(1_000_000),
        max_usd: Some(2.0),
        timeout: Some(Duration::from_secs(600)),
        elapsed: Duration::ZERO,
    };
    view.context = ContextWindow {
        used: 0,
        window: Some(272_000),
        trigger: Some(0.6),
        before: None,
    };
    view.plugins = plugins(["waiting", "waiting", "watching edits", "idle"]);
    view
}

/// Finished runs for the run list: a run with a fork to compare, and
/// older runs with only their outcome.
pub fn history() -> Vec<RunView> {
    let mut runs = vec![rotation_jitter(), backoff_fork()];
    let stop = || StopReason::Stop;
    let limit = || StopReason::Limit(tau_agent::event::LimitKind::Usd);
    // (repo, title, started, stop, turns, tokens, cost); no stop is live.
    let stubs = [
        (
            "tau-agent",
            "plugin-docs",
            "yesterday 22:10",
            Some(stop()),
            14,
            388_000,
            0.42,
        ),
        (
            "tau-agent",
            "mutants-triage",
            "Sep 26 19:02",
            Some(stop()),
            20,
            497_000,
            1.07,
        ),
        (
            "tau-agent",
            "lane-audit",
            "Sep 26 11:47",
            Some(limit()),
            17,
            462_000,
            2.0,
        ),
        (
            "tau-agent",
            "grep-all-cores",
            "Sep 25 16:20",
            Some(StopReason::Cancelled),
            5,
            96_000,
            0.112,
        ),
        (
            "docbert",
            "pdf-ingest",
            "today 04:02",
            None,
            5,
            131_000,
            0.094,
        ),
        (
            "docbert",
            "rerank-latency",
            "yesterday 18:40",
            Some(stop()),
            9,
            204_000,
            0.118,
        ),
        (
            "docbert",
            "bm25-tokenizer",
            "Sep 26 09:15",
            Some(stop()),
            6,
            88_000,
            0.064,
        ),
        (
            "homelab.nix",
            "backup-timer",
            "Sep 24 21:30",
            Some(stop()),
            4,
            61_000,
            0.052,
        ),
    ];
    runs.extend(stubs.into_iter().map(
        |(repo, title, started, stop, turns, tokens, cost)| {
            let mut view = RunView::new(
                RunId(Arc::from(title)),
                title,
                "coder",
                "gpt-5.5",
            )
            .in_repo(repo)
            .started(started);
            view.turn = turns;
            view.usage.tokens = tokens;
            view.usage.cost = cost;
            view.push_user(format!("(stored transcript of {title})"));
            let Some(stop) = stop else {
                view.status = RunStatus::Running;
                return view;
            };
            view.status = RunStatus::Finished(stop.clone());
            view.items.push(crate::view::Item::Stop {
                stop,
                turns,
                tokens,
                cost,
                plugin_cost: 0.0,
            });
            view
        },
    ));
    runs
}

fn play(mut view: RunView, script: Script) -> RunView {
    for (_, update) in script.steps {
        view.update(update);
    }
    view
}

fn rotation_jitter() -> RunView {
    let id = RunId(Arc::from("rotation-jitter"));
    let mut s = Script::for_run(id.clone());
    s.at(
        0,
        RunUpdate::User(
            "The pool rotates every connection at the same deadline, so they \
         all reconnect together. Add jitter so rotations spread out, and \
         keep each lane's continuation chain intact."
                .into(),
        ),
    );
    s.event(
        0,
        RunEvent::RunStart {
            run: id.clone(),
            parent: None,
            agent: Arc::from("coder"),
        },
    );
    for turn in 1..=5 {
        s.turn();
        if turn == 1 {
            s.tool(0, "j1", "grep", json!({ "pattern": "rotate|max_age", "path": "crates/tau-ai/src/ws" }), lines(6));
            s.tool(
                0,
                "j2",
                "read",
                json!({ "path": "crates/tau-ai/src/ws/proto/pool.rs" }),
                lines(57),
            );
        }
        s.end_turn(18_000 * turn as u64, 900, 0.018);
    }
    s.say(
        "Each deadline is `opened_at + MAX_AGE`, so connections opened in \
         one burst expire in one burst. I'll draw a jitter once, when the \
         connection opens.",
    );
    s.turn();
    let (args, output) = edit(
        "crates/tau-ai/src/ws/proto/pool.rs",
        "conn.opened_at + MAX_AGE - conn.jitter",
        "+    jitter: Duration,\n\
         \x20   ...\n\
         +    let jitter = MAX_AGE.mul_f64(rng.random_range(0.0..0.1));\n\
         \x20   ...\n\
         -        conn.opened_at + MAX_AGE\n\
         +        conn.opened_at + MAX_AGE - conn.jitter\n",
    );
    s.tool(0, "j3", "edit", args, output);
    s.end_turn(96_000, 1_400, 0.04);
    s.turn();
    s.tool(
        0,
        "j4",
        "bash",
        json!({ "command": "cargo nextest run -p tau-ai ws::" }),
        ToolOutput::text(
            "    PASS [0.012s] tau-ai ws::proto::pool::tests::deadline_is_jittered\n\
             \x20    Summary [4.2s] 52 tests run: 52 passed, 0 skipped",
        ),
    );
    s.end_turn(102_000, 800, 0.05);
    s.turn();
    s.say(
        "Draws a jitter of up to 10% of `MAX_AGE` when the connection \
         opens. One field, one line in `deadline()`. The reviewer approved \
         it with no problems.",
    );
    s.end_turn(104_000, 500, 0.033);
    s.event(
        0,
        RunEvent::RunEnd {
            run: id.clone(),
            parent: None,
            stop: StopReason::Stop,
            cost: 0.231,
        },
    );
    let mut view = play(
        RunView::new(id, "rotation-jitter", "coder", "gpt-5.5")
            .in_repo("tau-agent")
            .started("today 02:10"),
        s,
    );
    view.limits = retry_after().limits;
    view.context = retry_after().context;
    view.children.push(crate::view::ChildRun {
        id: fork_id(),
        title: "fork · backoff".into(),
        kind: crate::view::ChildKind::Fork,
        status: RunStatus::Finished(StopReason::Stop),
    });
    view
}

pub fn fork_id() -> RunId {
    RunId(Arc::from("rotation-jitter/backoff"))
}

fn backoff_fork() -> RunView {
    let id = fork_id();
    let mut s = Script::for_run(id.clone());
    s.turn = 6;
    s.at(
        0,
        RunUpdate::User("Try exponential backoff instead.".into()),
    );
    s.event(
        0,
        RunEvent::RunStart {
            run: id.clone(),
            parent: None,
            agent: Arc::from("coder"),
        },
    );
    s.turn();
    let (args, output) = edit(
        "crates/tau-ai/src/ws/io/driver.rs",
        "backoff",
        "+    let mut backoff = Backoff::exponential(BASE, MAX);\n\
         +    while let Err(error) = self.reconnect().await {\n\
         +        sleep(backoff.next()).await;\n\
         +    }\n\
         -    self.rotate_at(deadline);\n",
    );
    s.tool(0, "f1", "edit", args, output);
    s.end_turn(64_000, 1_100, 0.03);
    s.turn();
    s.tool(
        0,
        "f2",
        "bash",
        json!({ "command": "cargo nextest run -p tau-ai ws::" }),
        ToolOutput::text(
            "    FAIL ws::proto::continuation::tests::survives_rotation\n\
             \x20   FAIL ws::proto::lane::tests::drain_before_rotate\n\
             \x20 lane 3 resumed on connection #7 after it was retired\n\
             \x20    Summary [4.4s] 52 tests run: 50 passed, 2 failed",
        ),
    );
    s.end_turn(70_000, 700, 0.03);
    s.turn();
    s.say(
        "Backs off exponentially after each failed reconnect. It also moves \
         the rotation into the driver, and two continuation tests fail: a \
         lane can resume on a socket that already rotated.",
    );
    s.end_turn(71_000, 400, 0.028);
    s.event(
        0,
        RunEvent::RunEnd {
            run: id.clone(),
            parent: None,
            stop: StopReason::Stop,
            cost: 0.088,
        },
    );
    play(
        RunView::new(id, "fork · backoff", "coder", "gpt-5.5")
            .in_repo("tau-agent")
            .started("today 02:36")
            .with_origin(Origin::Fork {
                from: RunId(Arc::from("rotation-jitter")),
                turn: 6,
            }),
        s,
    )
}

/// A fork the demo starts when asked: the run `from`, continued after
/// `turn` on `prompt`, as a view to push and the script it plays.
pub fn fork_run(
    from: &RunView,
    turn: u32,
    prompt: &str,
    model: &crate::models::ModelChoice,
    id: RunId,
) -> (RunView, Vec<Step>) {
    let mut s = Script::for_run(id.clone());
    s.turn = turn;
    s.event(
        300,
        RunEvent::RunStart {
            run: id.clone(),
            parent: None,
            agent: Arc::from("coder"),
        },
    );
    s.turn();
    s.say(&format!(
        "Starting from turn {turn} of {}, with its code as it was then.",
        from.title
    ));
    let (args, output) = edit(
        "crates/tau-ai/src/retry.rs",
        "attempt",
        "-    self.base * attempt\n+    self.base * 2u32.pow(attempt.min(6))\n",
    );
    s.tool(400, "k1", "edit", args, output);
    s.end_turn(41_000, 600, 0.021);
    s.turn();
    s.say(
        "Done: the delay doubles with each attempt, up to 64 times the base.",
    );
    s.end_turn(42_000, 300, 0.019);
    s.event(
        200,
        RunEvent::RunEnd {
            run: id.clone(),
            parent: None,
            stop: StopReason::Stop,
            cost: 0.04,
        },
    );
    let mut view =
        RunView::new(id, crate::host::title(prompt), "coder", &model.model)
            .in_repo(from.repo.clone())
            .started("just now")
            .with_origin(Origin::Fork {
                from: from.id.clone(),
                turn,
            });
    view.plan = vec![PlanField {
        name: "reasoning".into(),
        value: model.effort.label().into(),
        set_by: None,
    }];
    view.push_user(prompt);
    (view, s.steps)
}

/// A reply the demo plays when a finished run goes on: one more turn,
/// numbered after the run's last.
pub fn resume_script(run: &RunView, prompt: &str) -> Vec<Step> {
    let mut s = Script::for_run(run.id.clone());
    s.turn = run.turn;
    s.event(
        300,
        RunEvent::RunStart {
            run: run.id.clone(),
            parent: None,
            agent: Arc::from(run.agent.as_str()),
        },
    );
    s.turn();
    s.say(&format!(
        "Going on from turn {}. You asked: {prompt}",
        run.turn
    ));
    s.end_turn(12_000, 200, 0.012);
    s.event(
        200,
        RunEvent::RunEnd {
            run: run.id.clone(),
            parent: None,
            stop: StopReason::Stop,
            cost: 0.012,
        },
    );
    s.steps
}

/// A demo screen by name, for `--open`.
pub fn route(name: &str) -> Option<crate::route::Route> {
    use crate::route::Route;
    Some(match name {
        "run" => Route::Run(run_id()),
        "history" => Route::History,
        "memory" => Route::Memory {
            repo: "tau-agent".into(),
            note: None,
        },
        "plugins" => Route::Plugins,
        "constitution" => Route::Constitution {
            repo: "tau-agent".into(),
            rule: None,
        },
        "compare" => Route::Compare {
            main: RunId(Arc::from("rotation-jitter")),
            fork: fork_id(),
        },
        "plan" => Route::Plan(run_id()),
        "ledger" => Route::Ledger(run_id()),
        "welcome" | "setup" => Route::Setup(SetupStep::Welcome),
        "github" => Route::Setup(SetupStep::GitHub),
        "token" => Route::Setup(SetupStep::Token),
        "model" => Route::Setup(SetupStep::Model),
        "repos" => Route::Setup(SetupStep::Repos),
        "ready" => Route::Setup(SetupStep::Ready),
        "pr" | "pr-opened" => Route::PullRequest(run_id()),
        "models" => Route::Models,
        _ => return None,
    })
}

const DEVICE_CODE: &str = "WDJB-MJHT";

fn device_code() -> DeviceCode {
    DeviceCode {
        code: DEVICE_CODE.into(),
        url: "github.com/login/device".into(),
        expires: "15 minutes".into(),
    }
}

fn repos() -> Vec<RepoChoice> {
    [
        ("cfcosta/tau-agent", "Rust library for LLM agents", true),
        (
            "cfcosta/docbert",
            "Local hybrid search over documents",
            true,
        ),
        (
            "cfcosta/homelab.nix",
            "NixOS configuration for the homelab",
            false,
        ),
        ("cfcosta/home.nix", "Home Manager configuration", false),
        ("cfcosta/duskpi", "", false),
        ("cfcosta/cfcosta.github.io", "", false),
    ]
    .into_iter()
    .map(|(name, description, selected)| RepoChoice {
        name: name.into(),
        description: description.into(),
        branch: "main".into(),
        selected,
    })
    .collect()
}

/// Onboarding as it stands when `step` opens.
pub fn setup(step: SetupStep) -> Setup {
    let mut setup = Setup {
        repos: repos(),
        ..Setup::default()
    };
    let stage = step.stage();
    setup.github = if step == SetupStep::Welcome {
        GitHub::SignedOut
    } else if stage == 0 {
        GitHub::Waiting(device_code())
    } else {
        GitHub::SignedIn {
            user: "cfcosta".into(),
        }
    };
    if stage >= 2 {
        setup.model = ModelAccess::Connected {
            label: "gpt-5.5 · Codex".into(),
        };
    }
    if stage >= 3 {
        setup.clones = vec![
            RepoClone {
                name: "cfcosta/tau-agent".into(),
                state: CloneState::Ready,
            },
            RepoClone {
                name: "cfcosta/docbert".into(),
                state: CloneState::Cloning {
                    share: 0.64,
                    detail: "41 MB of 64 MB".into(),
                },
            },
        ];
    }
    setup
}

/// The pull request the finished demo run would open.
pub fn pull_request() -> PullRequest {
    PullRequest {
        repo: "cfcosta/tau-agent".into(),
        head: "tau/retry-after".into(),
        base: "main".into(),
        mergeable: true,
        summary:
            "From run retry-after, which finished after 14 turns with its \
                  tests passing. tau pushes a branch and opens the pull \
                  request as you."
                .into(),
        tests: Some("14 tests passed".into()),
        title: "Honor retry-after on 429 and 503".into(),
        body: "The retry loop ignored `retry-after`. It now overrides the \
               backoff on\n429 and 503, as seconds or an HTTP date, capped at \
               `max_delay`. A\nheader that does not parse falls back to the \
               normal backoff.\n\nTests: cargo nextest run -p tau-ai retry:: \
               (14 passed, 4 new).\n\nMade with tau from run retry-after."
            .into(),
        commits: vec![
            PrCommit {
                title: "feat(tau-ai): honor retry-after on 429 and 503".into(),
                added: 11,
                removed: 2,
            },
            PrCommit {
                title: "test(tau-ai): cover the retry-after header".into(),
                added: 64,
                removed: 0,
            },
        ],
        draft: true,
        keep_pushing: true,
        state: PrState::Draft,
    }
}

/// The opened pull request's state.
pub fn opened() -> PrState {
    PrState::Opened {
        number: 142,
        url: "https://github.com/cfcosta/tau-agent/pull/142".into(),
        checks: Checks::Running,
    }
}

/// Answers what the workspace asks for the way a host would, after a
/// pause: sign-ins succeed, clones progress, pull requests open.
pub fn respond(workspace: &Entity<Workspace>, cx: &mut App) {
    cx.subscribe(workspace, |workspace, event: &WorkspaceEvent, cx| {
        // Each answer comes after its pause, in milliseconds.
        let later = |steps: Vec<(u64, Answer)>, cx: &mut App| {
            let workspace = workspace.downgrade();
            cx.spawn(async move |cx| {
                for (wait, answer) in steps {
                    cx.background_executor()
                        .timer(Duration::from_millis(wait))
                        .await;
                    let done = workspace.update(cx, |ws, cx| match answer {
                        Answer::Setup(update) => ws.update_setup(update, cx),
                        Answer::Repo(repo) => ws.add_repo(repo, cx),
                        Answer::Pr(run, state) => {
                            ws.set_pull_request_state(&run, state, cx)
                        }
                        Answer::Code(main, fork, code) => ws.set_branch_code(
                            &main,
                            &fork,
                            crate::view::CodeState::Ready(code),
                            cx,
                        ),
                    });
                    if done.is_err() {
                        return;
                    }
                }
            })
            .detach();
        };
        let setup = Answer::Setup;
        let signed_in = || {
            setup(SetupUpdate::GitHub(GitHub::SignedIn {
                user: "cfcosta".into(),
            }))
        };
        match event {
            WorkspaceEvent::GitHubSignIn => later(
                vec![
                    (
                        400,
                        setup(SetupUpdate::GitHub(GitHub::Waiting(
                            device_code(),
                        ))),
                    ),
                    (0, setup(SetupUpdate::Repos(repos()))),
                    (5000, signed_in()),
                ],
                cx,
            ),
            WorkspaceEvent::GitHubCheck
            | WorkspaceEvent::GitHubToken { .. } => {
                later(vec![(600, signed_in())], cx)
            }
            WorkspaceEvent::CodexSignIn { device } => {
                let pending = ModelAccess::SigningIn {
                    url: None,
                    device: device.then(|| DeviceCode {
                        code: "K7PX-2QRM".into(),
                        url: "auth.openai.com/codex/device".into(),
                        expires: "15 minutes".into(),
                    }),
                };
                let connected = ModelAccess::Connected {
                    label: "gpt-5.5 · Codex".into(),
                };
                later(
                    vec![
                        (300, setup(SetupUpdate::Model(pending))),
                        (2500, setup(SetupUpdate::Model(connected))),
                    ],
                    cx,
                )
            }
            WorkspaceEvent::ApiKey { .. } => {
                let connected = ModelAccess::Connected {
                    label: "gpt-5.5 · API key".into(),
                };
                later(vec![(600, setup(SetupUpdate::Model(connected)))], cx)
            }
            WorkspaceEvent::CloneRepos { repos } => {
                let steps = (1..=10)
                    .flat_map(|tenth| {
                        repos.iter().map(move |name| {
                            let state = if tenth == 10 {
                                CloneState::Ready
                            } else {
                                CloneState::Cloning {
                                    share: tenth as f32 / 10.,
                                    detail: format!("{tenth}0%"),
                                }
                            };
                            let clone = RepoClone {
                                name: name.clone(),
                                state,
                            };
                            (250, setup(SetupUpdate::Clone(clone)))
                        })
                    })
                    .collect();
                later(steps, cx)
            }
            WorkspaceEvent::PreparePullRequest { run } => {
                let run = run.clone();
                workspace.update(cx, |ws, cx| {
                    ws.set_pull_request(&run, pull_request(), cx)
                });
            }
            WorkspaceEvent::CreatePullRequest { run, .. } => {
                later(vec![(1200, Answer::Pr(run.clone(), opened()))], cx)
            }
            WorkspaceEvent::Fork {
                run,
                turn,
                prompt,
                model,
            } => {
                use std::sync::atomic::{AtomicU32, Ordering};
                static FORKS: AtomicU32 = AtomicU32::new(0);
                let n = FORKS.fetch_add(1, Ordering::Relaxed) + 1;
                let id = RunId(Arc::from(format!("{}/fork-{n}", run.0)));
                let (run, turn, prompt, model) =
                    (run.clone(), *turn, prompt.clone(), model.clone());
                workspace.update(cx, |ws, cx| {
                    let Some(from) = ws.run(&run).cloned() else {
                        return;
                    };
                    let turn = turn.unwrap_or(from.turn);
                    let (view, steps) =
                        fork_run(&from, turn, &prompt, &model, id.clone());
                    ws.push_run(view, cx);
                    ws.replay(id, steps, cx);
                });
            }
            // Rules change in the catalog, as the host's file would.
            WorkspaceEvent::AddRule { repo, text, on } => {
                let (repo, text, on) = (repo.clone(), text.clone(), on.clone());
                workspace.update(cx, |ws, cx| {
                    let mut catalog = ws.catalog().clone();
                    if let Some(listed) = catalog.repo_mut(&repo) {
                        let rules = &mut listed.constitution.rules;
                        let id = (1..)
                            .map(|n| format!("R{n}"))
                            .find(|id| rules.iter().all(|rule| &rule.id != id))
                            .unwrap_or_default();
                        rules.push(Rule {
                            id,
                            text,
                            applies_to: on,
                            review: 0.5,
                            block: 0.8,
                        });
                    }
                    ws.set_catalog(catalog, cx);
                });
            }
            WorkspaceEvent::RemoveRule { repo, id } => {
                let (repo, id) = (repo.clone(), id.clone());
                workspace.update(cx, |ws, cx| {
                    let mut catalog = ws.catalog().clone();
                    if let Some(listed) = catalog.repo_mut(&repo) {
                        listed.constitution.rules.retain(|rule| rule.id != id);
                    }
                    ws.set_catalog(catalog, cx);
                });
            }
            WorkspaceEvent::JevKey { key } => {
                let saved = key.is_some();
                workspace.update(cx, |ws, cx| {
                    let mut catalog = ws.catalog().clone();
                    catalog.models.access.jev = saved;
                    ws.set_catalog(catalog, cx);
                });
            }
            // The demo's store answers every query with its runs' costs.
            WorkspaceEvent::Query { .. } => {
                workspace.update(cx, |ws, cx| {
                    let table = tau_store::Table {
                        columns: vec!["agent".into(), "sum(cost_usd)".into()],
                        rows: vec![
                            vec!["coder".into(), "4.312".into()],
                            vec!["reviewer".into(), "0.206".into()],
                        ],
                        truncated: false,
                    };
                    ws.set_query_result(Ok(table), cx);
                });
            }
            // Updating finds nothing new.
            WorkspaceEvent::UpdateRepo { repo } => {
                let text = format!("{repo} is up to date");
                workspace.update(cx, |ws, cx| {
                    let mut catalog = ws.catalog().clone();
                    catalog.update = Some(text);
                    ws.set_catalog(catalog, cx);
                });
            }
            // A finished run goes on with one more turn.
            WorkspaceEvent::Resume { run, prompt, .. } => {
                let (run, prompt) = (run.clone(), prompt.clone());
                workspace.update(cx, |ws, cx| {
                    let Some(view) = ws.run(&run).cloned() else {
                        return;
                    };
                    let steps = resume_script(&view, &prompt);
                    ws.replay(run, steps, cx);
                });
            }
            // Signing out leaves what else is saved in use.
            WorkspaceEvent::SignOut(kind) => {
                let kind = *kind;
                workspace.update(cx, |ws, cx| {
                    let mut catalog = ws.catalog().clone();
                    let access = &mut catalog.models.access;
                    access.saved.retain(|saved| *saved != kind);
                    match kind {
                        crate::models::AccessKind::ChatGpt => {
                            access.chatgpt = false
                        }
                        crate::models::AccessKind::ApiKey => {
                            access.api_key = false
                        }
                    }
                    ws.set_catalog(catalog, cx);
                });
            }
            // The checkout at `path` becomes a repository with no notes
            // or rules yet.
            WorkspaceEvent::AddRepo { path } => {
                let name = path
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or(path)
                    .to_owned();
                later(
                    vec![(400, Answer::Repo(Repo::new(name, path.clone())))],
                    cx,
                )
            }
            WorkspaceEvent::CompareCode { main, fork } => later(
                vec![(
                    400,
                    Answer::Code(main.clone(), fork.clone(), branch_code()),
                )],
                cx,
            ),
            other => eprintln!("tau-ui: {other:?}"),
        }
    })
    .detach();
}

enum Answer {
    Setup(SetupUpdate),
    Repo(Repo),
    Pr(RunId, PrState),
    Code(RunId, RunId, BranchCode),
}

/// The code of `rotation-jitter` and its `backoff` fork.
pub fn branch_code() -> BranchCode {
    let stat = |path: &str, kind, added, removed| FileStat {
        path: path.into(),
        kind,
        added,
        removed,
    };
    let lines = |text: &str| crate::view::parse_diff(text);
    BranchCode {
        main: vec![
            stat(
                "crates/tau-ai/src/transport/pool.rs",
                FileKind::Modified,
                14,
                3,
            ),
            stat("crates/tau-ai/tests/rotation.rs", FileKind::Added, 38, 0),
        ],
        fork: vec![
            stat(
                "crates/tau-ai/src/transport/pool.rs",
                FileKind::Modified,
                9,
                3,
            ),
            stat("crates/tau-ai/src/retry.rs", FileKind::Modified, 6, 1),
        ],
        between: vec![
            FileChange {
                stat: stat(
                    "crates/tau-ai/src/transport/pool.rs",
                    FileKind::Modified,
                    3,
                    6,
                ),
                lines: lines(
                    " fn next_rotation(&self, now: Instant) -> Instant {\n\
                     -    let jitter = self.rng.gen_range(0..=self.max_jitter);\n\
                     -    now + self.period + jitter\n\
                     +    // Back off instead of jittering: the pool waits longer\n\
                     +    // after each failed rotation.\n\
                     +    now + self.period * 2u32.pow(self.failures.min(4))\n\
                      }\n",
                ),
            },
            FileChange {
                stat: stat(
                    "crates/tau-ai/src/retry.rs",
                    FileKind::Modified,
                    6,
                    1,
                ),
                lines: lines(
                    " pub fn delay(&self, attempt: u32) -> Duration {\n\
                     -    self.base * attempt\n\
                     +    self.base * 2u32.pow(attempt.min(6))\n\
                      }\n",
                ),
            },
            FileChange {
                stat: stat(
                    "crates/tau-ai/tests/rotation.rs",
                    FileKind::Removed,
                    0,
                    38,
                ),
                lines: lines(
                    "-#[test]\n-fn rotation_is_jittered() {\n-    // ...\n-}\n",
                ),
            },
        ],
    }
}

/// The plugins, notes, rules and store the demo workspace shows.
pub fn catalog() -> Catalog {
    let plugin = |name: &str,
                  description: &str,
                  seams: &[Seam],
                  spend: f64,
                  screen: Option<PluginScreen>| PluginInfo {
        name: name.into(),
        description: description.into(),
        seams: seams.to_vec(),
        spend,
        screen,
    };
    let link = |to: &str, why: &str| Link {
        to: to.into(),
        why: why.into(),
    };
    let note = |id: &str,
                title: &str,
                body: &[&str],
                links: Vec<Link>,
                paths: &[&str],
                used: u32| Note {
        id: id.into(),
        title: title.into(),
        body: body.iter().map(|p| (*p).to_owned()).collect(),
        links,
        paths: paths.iter().map(|p| (*p).to_owned()).collect(),
        written_by: "coder · lane-audit".into(),
        edited: "Sep 26".into(),
        used_by_runs: used,
    };
    let rule =
        |id: &str, text: &str, on: &[&str], review: f32, block: f32| Rule {
            id: id.into(),
            text: text.into(),
            applies_to: on.iter().map(|f| (*f).to_owned()).collect(),
            review,
            block,
        };
    Catalog {
        agent: "coder".into(),
        agent_source: Some("src/agents.rs:14".into()),
        plugins: vec![
            plugin("tau-reasoning", "Scores the job and picks the reasoning effort", &[Seam::Start], 0.004, Some(PluginScreen::Plan)),
            plugin("tau-memory", "Zettelkasten notes on docbert", &[Seam::Start, Seam::Tools, Seam::Finish], 0.212, Some(PluginScreen::Memory)),
            plugin("tau-constitution", "6 rules on edit, write, bash and the final answer", &[Seam::BeforeTool, Seam::BeforeStop], 0.031, Some(PluginScreen::Constitution)),
            plugin("tau-fast-compaction", "Prunes stale tool history with Jev", &[Seam::Start, Seam::Rewrite], 0.046, Some(PluginScreen::Ledger)),
            plugin("tau-compaction", "Summarizes when pruning is not enough", &[Seam::Start, Seam::Rewrite], 0.061, Some(PluginScreen::Ledger)),
            plugin("tau-tools", "read bash edit write grep find ls", &[Seam::Tools], 0.0, None),
        ],
        jev: Some(JevStats {
            model: "Jev 1.13".into(),
            key_env: "TYPESAFE_API_KEY".into(),
            price: "$0.042 / M input".into(),
            requests: 412,
            input_tokens: 1_940_000,
            spent: 0.081,
            latency_p50_ms: 180,
            retried: 3,
        }),
        repos: vec![
            Repo {
                name: "tau-agent".into(),
                path: "~/Code/cfcosta/tau-agent".into(),
                memory: tau_agent_memory(&note, &link),
                constitution: tau_agent_rules(&rule),
            },
            Repo {
                name: "docbert".into(),
                path: "~/Code/cfcosta/docbert".into(),
                memory: Memory {
                    path: "~/.tau/memory/docbert".into(),
                    collection: "docbert-memory".into(),
                    notes: vec![
                        note("d-0102", "Scanned pages have no text layer", &[
                            "PDFs made from scans come in with empty pages. Run OCR on a page only when it has no text layer, so born-digital PDFs stay fast.",
                        ], vec![], &["src/ingest/pdf.rs"], 3),
                        note("d-0118", "Rerank only the top 50", &[
                            "ColBERT reranking costs grow with the candidate list. BM25 recalls 200, and only the top 50 go to the reranker.",
                        ], vec![link("d-0131", "BM25 recalls the candidates")], &["src/search/rerank.rs"], 4),
                        note("d-0131", "The BM25 tokenizer keeps identifiers whole", &[
                            "`snake_case` and `CamelCase` stay one token, and are also split into their parts, so both searches hit.",
                        ], vec![], &["src/search/bm25.rs"], 2),
                    ],
                },
                constitution: Constitution {
                    path: "constitution.toml".into(),
                    max_continuations: 3,
                    error: None,
                    rules: vec![
                        rule("D1", "Never rebuild the whole index to fix one document.", &["bash.command"], 0.30, 0.70),
                        rule("D2", "Search results keep their scores; never sort them away.", &["edit.newText"], 0.40, 0.85),
                        rule("D3", "The final answer names the tests that ran.", &["final answer"], 0.40, 0.75),
                    ],
                },
            },
            Repo {
                name: "homelab.nix".into(),
                path: "~/Code/cfcosta/homelab.nix".into(),
                memory: Memory {
                    path: "~/.tau/memory/homelab.nix".into(),
                    collection: "homelab-memory".into(),
                    notes: vec![note("h-0007", "Backups run from a systemd timer", &[
                        "`restic` runs from `backup.timer` at 03:00, never from cron, so a missed run catches up on boot.",
                    ], vec![], &["hosts/nas/backup.nix"], 1)],
                },
                constitution: Constitution::default(),
            },
        ],
        open_repos: vec!["tau-agent".into()],
        closed_runs: Vec::new(),
        reviewed: Vec::new(),
        store: StoreInfo {
            path: "runs.db".into(),
            size: "18.4 MB".into(),
            sample_query: "select agent, sum(cost_usd) from runs where created_at > date('now', '-7 days') group by agent".into(),
        },
        pull_requests: true,
        project: Default::default(),
        update: None,
        models: models(),
    }
}

type NoteFn<'a> =
    &'a dyn Fn(&str, &str, &[&str], Vec<Link>, &[&str], u32) -> Note;
type LinkFn<'a> = &'a dyn Fn(&str, &str) -> Link;
type RuleFn<'a> = &'a dyn Fn(&str, &str, &[&str], f32, f32) -> Rule;

/// tau-agent's notes, the ones the mockups show.
fn tau_agent_memory(note: NoteFn<'_>, link: LinkFn<'_>) -> Memory {
    Memory {
        path: "~/.tau/memory".into(),
        collection: "tau-memory".into(),
        notes: vec![
            note(
                "n-0417",
                "Rotation must drain lanes first",
                &[
                    "A connection that is past its deadline can still carry lanes with a response in flight. Retiring it right away breaks their continuation: the next request would name a `previous_response_id` that the new socket has never seen.",
                    "So the pool marks the connection as draining, sends no new lanes to it, and closes it when its last lane finishes.",
                    "Jitter on the deadline only moves when draining starts. It does not replace it.",
                ],
                vec![
                    link("n-0212", "how a lane knows what it continues"),
                    link("n-0433", "jitter moves the deadline"),
                ],
                &[
                    "crates/tau-ai/src/ws/proto/pool.rs",
                    "crates/tau-ai/src/ws/proto/lane.rs",
                ],
                7,
            ),
            note(
                "n-0433",
                "Spread reconnects with jitter",
                &[
                    "Connections opened together expire together unless the deadline moves. A jitter drawn at open time spreads the rotations.",
                ],
                vec![link("n-0417", "draining still applies")],
                &["crates/tau-ai/src/ws/proto/pool.rs"],
                2,
            ),
            note(
                "n-0212",
                "Continuation ids are call_id|item_id",
                &[
                    "The Responses API needs both halves to resume a tool call. tau-ai joins them with a `|` in the tool call id.",
                ],
                vec![],
                &["crates/tau-ai/src/responses/input.rs"],
                9,
            ),
            note(
                "n-0301",
                "16 in flight per connection",
                &[
                    "The pool enforces the limit per socket, not per client. Draining sockets still count.",
                ],
                vec![link("n-0417", "draining sockets still count")],
                &["crates/tau-ai/src/ws/proto/pool.rs"],
                3,
            ),
            note(
                "n-0388",
                "Retry policy honors server hints",
                &[
                    "When the server sends `retry-after`, it wins over our backoff, capped at `max_delay`. The header can be seconds or an HTTP date.",
                ],
                vec![link("n-0212", "retries resume the same continuation")],
                &["crates/tau-ai/src/retry.rs"],
                4,
            ),
            note(
                "n-0390",
                "429 vs 503 in the Responses API",
                &[
                    "429 means we sent too much; 503 means they are overloaded. Both are retried, and both may carry `retry-after`.",
                ],
                vec![link("n-0388", "both carry the hint")],
                &["crates/tau-ai/src/retry.rs"],
                2,
            ),
            note(
                "n-0205",
                "Tests use the fake OpenAI server",
                &[
                    "`tau-testing` runs a fake Responses server over WebSocket. Tests never reach the network.",
                ],
                vec![],
                &["crates/tau-testing/src/fake_openai.rs"],
                11,
            ),
            note(
                "n-0350",
                "Rewrites force one full resend",
                &[
                    "Any edit to the transcript breaks the delta chain once. The next request goes in full, and turns are deltas again after it.",
                ],
                vec![link("n-0417", "a resend can land on a draining socket")],
                &["crates/tau-agent/src/plugin.rs"],
                5,
            ),
        ],
    }
}

/// tau-agent's rules.
fn tau_agent_rules(rule: RuleFn<'_>) -> Constitution {
    Constitution {
        path: "constitution.toml".into(),
        max_continuations: 3,
        error: None,
        rules: vec![
            rule(
                "R1",
                "Never delete outside target/ or rewrite published history.",
                &["bash.command"],
                0.30,
                0.60,
            ),
            rule(
                "R2",
                "Library code returns errors. No unwrap or expect outside tests.",
                &["edit.newText", "write.content"],
                0.30,
                0.80,
            ),
            rule(
                "R3",
                "Every sqlx query uses the checked macros.",
                &["edit.newText", "write.content"],
                0.40,
                0.85,
            ),
            rule(
                "R4",
                "Comments explain why, not what the code does.",
                &["edit.newText"],
                0.50,
                0.90,
            ),
            rule(
                "R5",
                "No network calls from tests except the fake server.",
                &["write.content"],
                0.35,
                0.80,
            ),
            rule(
                "R6",
                "The final answer names the tests that ran and their result.",
                &["final answer"],
                0.40,
                0.75,
            ),
        ],
    }
}

/// The models the demo offers: tau-ai's, as a ChatGPT Pro sign-in sees
/// them, with defaults for each of the demo's agents.
pub fn models() -> crate::models::Models {
    use crate::models::{
        AccessInfo,
        Effort,
        ModelChoice,
        ModelSettings,
        Models,
        coding_models,
    };
    let mut settings = ModelSettings::default();
    settings.set_default(
        "reviewer",
        ModelChoice::new("gpt-6-luna", Effort::Medium),
    );
    settings.set_default(
        "tau-memory",
        ModelChoice::new("gpt-5.6-luna", Effort::Low),
    );
    Models {
        options: coding_models(|id| tau_ai::codex::MODELS.contains(&id)),
        settings,
        access: AccessInfo {
            label: "ChatGPT Pro".into(),
            chatgpt: true,
            api_key: false,
            saved: vec![crate::models::AccessKind::ChatGpt],
            jev: true,
        },
        agents: vec![
            ("coder".into(), "Runs you start from the composer.".into()),
            (
                "reviewer".into(),
                "The sub-agent coder asks to review its changes.".into(),
            ),
            (
                "tau-memory".into(),
                "Distills notes at the end of a run.".into(),
            ),
        ],
    }
}

fn plugins(states: [&str; 4]) -> Vec<PluginStatus> {
    let names = [
        "tau-reasoning",
        "tau-memory",
        "tau-constitution",
        "tau-fast-compaction",
    ];
    names
        .into_iter()
        .zip(states)
        .map(|(name, state)| PluginStatus {
            name: name.into(),
            state: state.into(),
            tone: Tone::Quiet,
        })
        .collect()
}

/// Builds the script step by step.
struct Script {
    steps: Vec<Step>,
    run: RunId,
    turn: u32,
    elapsed: Duration,
}

impl Script {
    fn new() -> Self {
        Self::for_run(run_id())
    }

    fn for_run(run: RunId) -> Self {
        Self {
            steps: Vec::new(),
            run,
            turn: 0,
            elapsed: Duration::ZERO,
        }
    }

    fn at(&mut self, millis: u64, update: impl Into<RunUpdate>) {
        let wait = Duration::from_millis(millis);
        self.elapsed += wait;
        self.steps.push((wait, update.into()));
    }

    fn event(&mut self, millis: u64, event: RunEvent) {
        self.at(millis, event);
    }

    fn turn(&mut self) {
        self.turn += 1;
        let turn = self.turn;
        self.event(
            250,
            RunEvent::TurnStart {
                run: self.run.clone(),
                turn,
            },
        );
    }

    fn end_turn(&mut self, input: u64, output: u64, cost: f64) {
        let turn = self.turn;
        self.event(
            150,
            RunEvent::TurnEnd {
                run: self.run.clone(),
                turn,
                usage: Usage {
                    input,
                    output,
                    total_tokens: input + output,
                    cost: UsageCost {
                        total: cost,
                        ..UsageCost::default()
                    },
                    ..Usage::default()
                },
            },
        );
        // Scripted time runs faster than the clock the limits show.
        let elapsed = self.elapsed * 6;
        self.at(
            0,
            RunUpdate::Limits(Limits {
                elapsed,
                ..retry_after().limits
            }),
        );
    }

    /// Streams text a few words at a time, as the model would.
    fn say(&mut self, text: &str) {
        let words: Vec<&str> = text.split_inclusive(' ').collect();
        for (index, chunk) in words.chunks(3).enumerate() {
            self.event(
                if index == 0 { 300 } else { 45 },
                RunEvent::TextDelta {
                    run: self.run.clone(),
                    parent: None,
                    delta: chunk.concat(),
                },
            );
        }
    }

    fn think(&mut self, text: &str) {
        self.event(
            400,
            RunEvent::ThinkingDelta {
                run: self.run.clone(),
                delta: text.into(),
            },
        );
    }

    /// tau-constitution's check of a call (or, with `None`, the final
    /// answer), with each rule's score, as the plugin reports it.
    fn checked(&mut self, call: Option<(&str, &str)>, scores: &[(&str, f64)]) {
        let mut body = json!({
            "kind": "checked",
            "scores": scores
                .iter()
                .map(|(rule, score)| json!({ "rule": rule, "score": score }))
                .collect::<Vec<_>>(),
            "cost": 0.00002,
        });
        if let Some((id, tool)) = call {
            body["call_id"] = id.into();
            body["tool"] = tool.into();
        }
        self.verdict(body);
    }

    /// A report from tau-constitution.
    fn verdict(&mut self, body: Value) {
        self.event(
            60,
            RunEvent::PluginReport {
                run: self.run.clone(),
                plugin: Arc::from("tau-constitution"),
                body,
            },
        );
    }

    fn start_tool(&mut self, id: &str, tool: &str, args: Value) {
        self.event(
            350,
            RunEvent::ToolStart {
                run: self.run.clone(),
                call_id: id.into(),
                tool: Arc::from(tool),
                args,
            },
        );
    }

    fn end_tool(&mut self, millis: u64, id: &str, output: ToolOutput) {
        self.event(
            millis,
            RunEvent::ToolEnd {
                run: self.run.clone(),
                call_id: id.into(),
                output: Arc::new(output),
                is_error: false,
            },
        );
    }

    fn tool(
        &mut self,
        millis: u64,
        id: &str,
        tool: &str,
        args: Value,
        output: ToolOutput,
    ) {
        self.start_tool(id, tool, args);
        self.end_tool(millis, id, output);
    }

    fn note(&mut self, millis: u64, note: PluginNote) {
        self.at(millis, RunUpdate::Note(note));
    }
}

/// An output that says how to sum itself up, as tools may.
fn summarized(output: ToolOutput, summary: &str) -> ToolOutput {
    ToolOutput {
        details: Some(json!({ "summary": summary })),
        ..output
    }
}

fn lines(count: usize) -> ToolOutput {
    ToolOutput::text(
        (1..=count)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn edit(path: &str, new_text: &str, diff: &str) -> (Value, ToolOutput) {
    (
        json!({ "path": path, "edits": [{ "oldText": "", "newText": new_text }] }),
        ToolOutput {
            details: Some(json!({ "diff": diff, "firstChangedLine": 131 })),
            ..ToolOutput::text(format!(
                "Successfully replaced 1 block(s) in {path}."
            ))
        },
    )
}

/// The whole session, from the user's message to the notes memory
/// suggests keeping.
pub fn script() -> Vec<Step> {
    let mut s = Script::new();
    let run = s.run.clone();

    s.at(200, RunUpdate::User(
        "The retry loop ignores `retry-after` on 429s. Honor it, cap it at \
         the policy's max delay, and add tests."
            .into(),
    ));
    s.event(
        300,
        RunEvent::RunStart {
            run: run.clone(),
            parent: None,
            agent: Arc::from("coder"),
        },
    );

    // tau-reasoning and tau-memory run in `start`, before the session.
    s.note(
        500,
        PluginNote {
            plugin: "tau-reasoning".into(),
            text: "picked **high** reasoning for this run".into(),
            detail: Some("Jev · 180 ms · $0.00002".into()),
            tone: Tone::Info,
            body: NoteBody::Distribution {
                levels: [
                    ("minimal", 0.01),
                    ("low", 0.02),
                    ("medium", 0.09),
                    ("high", 0.84),
                    ("xhigh", 0.04),
                ]
                .into_iter()
                .map(|(name, p)| (name.to_owned(), p))
                .collect(),
                chosen: 3,
                note:
                    "Confidence 0.84 is above 0.70, so the run uses high. It \
                   stays fixed for the whole run."
                        .into(),
                confidence: Some((0.84, 0.70)),
                hints: [
                    "lookups",
                    "small edits",
                    "routine code",
                    "refactors",
                    "audits, proofs",
                ]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            },
        },
    );
    s.note(
        400,
        PluginNote {
            plugin: "tau-memory".into(),
            text: "added 3 notes to the context".into(),
            detail: Some("hybrid search · 21 ms".into()),
            tone: Tone::Info,
            body: NoteBody::Chips(vec![
                "Retry policy honors server hints".into(),
                "429 vs 503 in the Responses API".into(),
                "Tests use the fake OpenAI server".into(),
            ]),
        },
    );
    s.at(
        0,
        RunUpdate::Plan(vec![
            PlanField {
                name: "reasoning".into(),
                value: "high".into(),
                set_by: Some("tau-reasoning".into()),
            },
            PlanField {
                name: "context".into(),
                value: "3 notes".into(),
                set_by: Some("tau-memory".into()),
            },
            PlanField {
                name: "instructions".into(),
                value: "agent default".into(),
                set_by: None,
            },
            PlanField {
                name: "model".into(),
                value: "gpt-5.5".into(),
                set_by: None,
            },
        ]),
    );
    s.at(
        0,
        RunUpdate::Plugins(plugins([
            "chose high",
            "3 notes",
            "watching edits",
            "idle · 2% of window",
        ])),
    );

    s.turn();
    s.think("The user wants retry-after honored. Memory has a note on this.");
    s.start_tool(
        "c1",
        "memory_read",
        json!({ "id": "n-0388", "title": "Retry policy honors server hints" }),
    );
    s.at(
        0,
        RunUpdate::ToolPlugin {
            call_id: "c1".into(),
            plugin: "tau-memory".into(),
        },
    );
    s.end_tool(
        300,
        "c1",
        summarized(
            ToolOutput::text("Retry policy honors server hints\n\n412 words"),
            "412 words",
        ),
    );
    s.say(
        "The note says we already decided `retry-after` should win over \
         our own backoff, capped at `max_delay`, and that the header can be \
         seconds or an HTTP date. I'll find where the loop reads it.",
    );
    s.tool(
        250,
        "c2",
        "grep",
        json!({ "pattern": "retry_after|RetryPolicy", "path": "crates/tau-ai/src" }),
        summarized(lines(5), "5 matches · 9 ms"),
    );
    s.end_turn(9_800, 420, 0.012);

    s.turn();
    s.tool(
        400,
        "c3",
        "read",
        json!({ "path": "crates/tau-ai/src/retry.rs" }),
        lines(188),
    );
    s.say(
        "`RetryPolicy::delay` only knows the attempt number. I'll pass the \
         parsed header in and take the smaller of it and `max_delay`.",
    );
    // tau-constitution blocks the first edit in `before_tool`.
    s.start_tool(
        "c4",
        "edit",
        json!({
            "path": "crates/tau-ai/src/retry.rs",
            "edits": [{
                "oldText": "",
                "newText": "let secs: u64 = header.to_str().unwrap().parse().unwrap();"
            }]
        }),
    );
    let reason = "Blocked by tau-constitution: this call breaks rule R2 \
                  (\"Library code returns errors. No unwrap or expect outside \
                  tests.\"), violation probability 0.95. Change the call so it \
                  follows the rule, then try again.";
    s.checked(Some(("c4", "edit")), &[("R2", 0.95), ("R4", 0.08)]);
    s.verdict(json!({
        "kind": "blocked", "rule": "R2", "score": 0.95,
        "text": "Library code returns errors. No unwrap or expect outside tests.",
        "call_id": "c4", "tool": "edit", "reason": reason,
    }));
    s.end_tool(500, "c4", ToolOutput::text(reason));
    s.at(0, RunUpdate::PluginCost(0.00007));
    s.end_turn(21_400, 610, 0.027);

    s.turn();
    s.say(
        "Fair. A malformed header from a proxy shouldn't panic the client. \
         If it doesn't parse, I'll fall back to the policy's own backoff.",
    );
    let (args, output) = edit(
        "crates/tau-ai/src/retry.rs",
        "fn server_hint(...)",
        " fn server_hint(headers: &HeaderMap) -> Option<Duration> {\n\
         +    let value = headers.get(RETRY_AFTER)?.to_str().ok()?;\n\
         +    value.parse::<u64>().ok().map(Duration::from_secs)\n\
         +        .or_else(|| parse_http_date(value))\n\
         -    todo!()\n\
         \x20}\n",
    );
    s.start_tool("c5", "edit", args);
    s.checked(Some(("c5", "edit")), &[("R2", 0.02), ("R4", 0.06)]);
    s.end_tool(450, "c5", output);
    s.start_tool(
        "c6",
        "bash",
        json!({ "command": "rm -rf target/debug/incremental" }),
    );
    s.checked(Some(("c6", "bash")), &[("R1", 0.41)]);
    s.verdict(json!({
        "kind": "flagged", "rule": "R1", "score": 0.41,
        "text": "Never delete outside target/ or rewrite published history.",
        "call_id": "c6", "tool": "bash",
    }));
    s.end_tool(600, "c6", ToolOutput::text(""));
    s.at(
        0,
        RunUpdate::Plugins(plugins([
            "chose high",
            "3 notes · 1 read",
            "1 blocked · 1 flagged",
            "idle · 38% of window",
        ])),
    );
    s.end_turn(58_000, 1_900, 0.071);

    // A reviewer sub-agent looks at the parser while the run goes on.
    let reviewer = RunId(Arc::from("retry-after/reviewer"));
    s.event(
        300,
        RunEvent::RunStart {
            run: reviewer.clone(),
            parent: Some(run.clone()),
            agent: Arc::from("reviewer"),
        },
    );

    s.turn();
    s.tool(
        800,
        "c7",
        "bash",
        json!({ "command": "cargo nextest run -p tau-ai" }),
        lines(40),
    );
    s.tool(
        300,
        "c8",
        "read",
        json!({ "path": "crates/tau-ai/src/client.rs" }),
        lines(610),
    );
    s.tool(
        300,
        "c9",
        "read",
        json!({ "path": "crates/tau-testing/src/fake_openai.rs" }),
        lines(402),
    );
    s.event(
        400,
        RunEvent::RunEnd {
            run: reviewer,
            parent: Some(run.clone()),
            stop: StopReason::Stop,
            cost: 0.019,
        },
    );
    s.end_turn(96_000, 2_400, 0.063);

    // tau-fast-compaction prunes at the turn boundary.
    s.event(
        700,
        RunEvent::ContextRewritten {
            run: run.clone(),
            plugin: Arc::from("tau-fast-compaction"),
            tokens_before: 172_000,
            tokens_after: 81_000,
        },
    );
    s.at(
        0,
        RunUpdate::RewriteDetail("38 calls judged · $0.0011".into()),
    );
    let entry = |call_id: &str,
                 turn: u32,
                 tool: &str,
                 input: &str,
                 tokens: u64,
                 odds: Option<(f32, f32)>,
                 decision: Decision| LedgerEntry {
        call_id: call_id.into(),
        turn,
        tool: tool.into(),
        input: input.into(),
        tokens,
        matters: odds.map(|(m, _)| m),
        verbatim: odds.map(|(_, v)| v),
        decision,
    };
    s.at(
        0,
        RunUpdate::Ledger(vec![
            entry(
                "c1",
                1,
                "memory_read",
                "n-0388",
                600,
                None,
                Decision::Pinned,
            ),
            entry(
                "c2",
                1,
                "grep",
                "\"retry_after|RetryPolicy\" crates/tau-ai/src",
                300,
                Some((0.64, 0.22)),
                Decision::Keep,
            ),
            entry(
                "c3",
                2,
                "read",
                "crates/tau-ai/src/retry.rs",
                21_700,
                Some((0.71, 0.18)),
                Decision::DropResult,
            ),
            entry(
                "c4",
                2,
                "edit",
                "crates/tau-ai/src/retry.rs",
                200,
                None,
                Decision::Pinned,
            ),
            entry(
                "c5",
                3,
                "edit",
                "crates/tau-ai/src/retry.rs",
                200,
                Some((0.94, 0.88)),
                Decision::Keep,
            ),
            entry(
                "c6",
                3,
                "bash",
                "rm -rf target/debug/incremental",
                100,
                Some((0.18, 0.04)),
                Decision::Keep,
            ),
            entry(
                "c7",
                4,
                "bash",
                "cargo nextest run -p tau-ai",
                18_200,
                Some((0.12, 0.03)),
                Decision::DropCall,
            ),
            entry(
                "c8",
                4,
                "read",
                "crates/tau-ai/src/client.rs",
                21_700,
                Some((0.52, 0.09)),
                Decision::DropResult,
            ),
            entry(
                "c9",
                4,
                "read",
                "crates/tau-testing/src/fake_openai.rs",
                14_300,
                Some((0.58, 0.11)),
                Decision::DropResult,
            ),
        ]),
    );
    s.at(0, RunUpdate::PluginCost(0.0011));
    s.at(
        0,
        RunUpdate::Plugins(plugins([
            "chose high",
            "3 notes · 1 read",
            "1 blocked · 1 flagged",
            "1 rewrite · −91k",
        ])),
    );

    s.turn();
    s.say(
        "Both parsers are in. Now the loop itself: when `server_hint` \
         returns a value, it replaces the backoff for that attempt, capped \
         at `max_delay`.",
    );
    let (args, output) = edit(
        "crates/tau-ai/src/retry.rs",
        "let wait = ...",
        "-let wait = self.policy.delay(attempt);\n\
         +let wait = server_hint(&headers)\n\
         +    .map(|hint| hint.min(self.policy.max_delay))\n\
         +    .unwrap_or_else(|| self.policy.delay(attempt));\n",
    );
    s.tool(450, "c10", "edit", args, output);
    s.start_tool(
        "c11",
        "bash",
        json!({ "command": "cargo nextest run -p tau-ai retry::" }),
    );
    let mut log = String::new();
    for test in [
        "retry::tests::honors_seconds",
        "retry::tests::honors_http_date",
        "retry::tests::caps_at_max_delay",
        "retry::tests::ignores_malformed_header",
    ] {
        log.push_str(&format!("    PASS [0.01s] tau-ai {test}\n"));
        s.event(
            500,
            RunEvent::ToolUpdate {
                run: run.clone(),
                call_id: "c11".into(),
                partial: Arc::new(ToolOutput::text(log.clone())),
            },
        );
    }
    log.push_str("     Summary [3.1s] 14 tests run: 14 passed, 0 skipped");
    s.end_tool(600, "c11", ToolOutput::text(log));
    s.end_turn(81_000, 1_300, 0.052);

    s.turn();
    s.say(
        "Done. `retry-after` now overrides the backoff on 429 and 503, in \
         seconds or as an HTTP date, and is capped at `max_delay`. A header \
         that doesn't parse falls back to the normal backoff. I added four \
         tests against the fake server.",
    );
    s.end_turn(82_000, 700, 0.024);
    // tau-constitution holds the stop in `before_stop`.
    s.checked(None, &[("R6", 0.91)]);
    let held = "Your answer breaks rule R6 (\"The final answer names the \
                tests that ran and their result.\"). Revise it so it follows \
                the rule, then answer again.";
    s.verdict(json!({
        "kind": "held", "rule": "R6", "score": 0.91,
        "text": "The final answer names the tests that ran and their result.",
        "reason": held, "hold": 1, "max_holds": 3,
    }));
    s.event(
        500,
        RunEvent::Continued {
            run: run.clone(),
            plugin: Arc::from("tau-constitution"),
            message: held.into(),
        },
    );

    s.turn();
    s.say(
        "Tests: `cargo nextest run -p tau-ai retry::`, 14 passed, 4 of them \
         new: `honors_seconds`, `honors_http_date`, `caps_at_max_delay`, \
         `ignores_malformed_header`.",
    );
    s.end_turn(82_600, 300, 0.012);
    s.checked(None, &[("R6", 0.04)]);
    s.at(0, RunUpdate::PluginCost(0.012));
    s.event(
        400,
        RunEvent::RunEnd {
            run: run.clone(),
            parent: None,
            stop: StopReason::Stop,
            cost: 0.274,
        },
    );

    // tau-memory distills in `finish`, after the run is stored.
    s.note(
        900,
        PluginNote {
            plugin: "tau-memory".into(),
            text: "suggests 2 notes from this run".into(),
            detail: Some("finish · distilled with gpt-5.5 · $0.012".into()),
            tone: Tone::Info,
            body: NoteBody::Proposals(vec![
                Proposal {
                    title: "retry-after can be an HTTP date".into(),
                    detail: "Links to Retry policy honors server hints. From \
                         turn 7."
                        .into(),
                },
                Proposal {
                    title: "Proxies send malformed retry-after".into(),
                    detail: "Links to 429 vs 503 in the Responses API. From \
                         turn 5."
                        .into(),
                },
            ]),
        },
    );
    s.at(
        0,
        RunUpdate::Plugins(plugins([
            "chose high",
            "3 in · 2 proposed",
            "1 block · 1 flag · 1 hold",
            "1 rewrite · −91k",
        ])),
    );
    s.steps
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{Item, ToolState};

    #[test]
    fn every_demo_screen_has_a_route() {
        for name in [
            "run",
            "history",
            "memory",
            "plugins",
            "constitution",
            "compare",
            "plan",
            "ledger",
        ] {
            assert!(route(name).is_some(), "{name}");
        }
        assert!(route("nowhere").is_none());
    }

    #[test]
    fn the_fork_points_at_its_run() {
        let runs = history();
        let fork = runs.iter().find(|run| run.id == fork_id()).expect("fork");
        let crate::view::Origin::Fork { from, turn } = &fork.origin else {
            panic!("not a fork: {:?}", fork.origin);
        };
        assert!(runs.iter().any(|run| &run.id == from));
        assert_eq!(*turn, 6);
        assert!(fork.last_diff().is_some());
    }

    #[test]
    fn catalog_links_point_at_notes_of_their_repository() {
        let catalog = catalog();
        for repo in &catalog.repos {
            let memory = &repo.memory;
            for note in &memory.notes {
                for link in &note.links {
                    assert!(
                        memory.note(&link.to).is_some(),
                        "{}: {} -> {}",
                        repo.name,
                        note.id,
                        link.to
                    );
                }
            }
        }
        let tau = catalog.repo("tau-agent").unwrap();
        assert!(tau.memory.backlinks("n-0417").count() >= 2);
    }

    #[test]
    fn every_demo_run_belongs_to_a_listed_repository() {
        let catalog = catalog();
        let mut runs = vec![retry_after()];
        runs.extend(history());
        for run in &runs {
            assert!(catalog.repo(&run.repo).is_some(), "{}", run.title);
        }
    }

    #[test]
    fn the_script_plays_to_a_finished_run() {
        let mut view = retry_after();
        for (_, update) in script() {
            view.update(update);
        }
        assert_eq!(view.status, RunStatus::Finished(StopReason::Stop));
        assert!(matches!(
            view.tool("c4").map(|card| &card.state),
            Some(ToolState::Blocked { .. })
        ));
        assert_eq!(view.children.len(), 1);
        assert!(
            view.items
                .iter()
                .any(|item| matches!(item, Item::Rewrite { .. }))
        );
        assert!(matches!(view.items.last(), Some(Item::Plugin(_))));
        // The ledger marks the cards it pruned.
        assert_eq!(view.ledger.len(), 9);
        assert_eq!(
            view.tool("c7").and_then(|card| card.pruned),
            Some(crate::view::Pruned::CallDropped)
        );
        // What `start` decided comes before the first turn.
        assert_eq!(view.start_notes().count(), 2);
        assert_eq!(view.proposals().count(), 2);
    }
}
