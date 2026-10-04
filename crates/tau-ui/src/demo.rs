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
use tau_ui_plugin::Push;

pub use self::{
    host::DemoHost,
    screens::{SCREENS, Screen, open},
};
use crate::{
    Workspace,
    WorkspaceEvent,
    catalog::{Catalog, JevStats, Repo, StoreInfo},
    models::{AccountState, ChatGptAccount},
    pairing::{
        Computer,
        PairRequest,
        PairStep,
        Pairing,
        PairingUpdate,
        Progress,
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
    update::HostUpdate,
    view::{
        BranchCode,
        ContextWindow,
        FileChange,
        FileKind,
        FileStat,
        Limits,
        Origin,
        PlanField,
        PluginStatus,
        RunStatus,
        RunUpdate,
        RunView,
        Tone,
    },
};

mod host;
mod screens;

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
        before: None,
    };
    // fast-compaction runs with it, and steps in at 60%.
    view.fold(
        tau_fast_compaction::NAME,
        &json!({ "kind": "starting", "on": true }),
    );
    view.plugins = plugins(["waiting", "waiting", "watching edits", "idle"]);
    view
}

/// The condition the demo's goals share.
pub const GOAL: &str = "All tests in tau-ai pass and cargo clippy is clean";

/// Finished runs for the run list: a run with a fork to compare, and
/// older runs with only their outcome.
pub fn history() -> Vec<RunView> {
    let mut runs = vec![rotation_jitter(), backoff_fork()];
    let stop = || StopReason::Stop;
    let limit = || StopReason::Limit(tau_agent::event::LimitKind::Usd);
    // (repo, title, started, stop, turns, tokens, cost); no stop is live.
    // Each repository's main chat, then its chats.
    let stubs = [
        (
            "docbert",
            "docbert-main",
            "Sep 20 10:00",
            Some(stop()),
            3,
            41_000,
            0.031,
        ),
        (
            "homelab.nix",
            "homelab-main",
            "Sep 20 10:05",
            Some(stop()),
            2,
            22_000,
            0.018,
        ),
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
            let main = match repo {
                "docbert" => "docbert-main",
                "homelab.nix" => "homelab-main",
                _ => "retry-after",
            };
            let (title, origin) = if title == main {
                ("main", Origin::Root)
            } else {
                (
                    title,
                    Origin::Fork {
                        from: RunId(Arc::from(main)),
                        turn: 1,
                    },
                )
            };
            let id = if title == "main" { main } else { title };
            let mut view =
                RunView::new(RunId(Arc::from(id)), title, "coder", "gpt-5.5")
                    .in_repo(repo)
                    .started(started)
                    .with_origin(origin);
            view.turn = turns;
            view.usage.tokens = tokens;
            view.usage.cost = cost;
            view.push_user(format!("(stored transcript of {title})"));
            // Goals, as history has them.
            match title {
                "mutants-triage" => view.restate(
                    tau_goal::NAME,
                    &tau_goal::demo::records(
                        "Every mutant in retry.rs is caught",
                        "met",
                    ),
                ),
                "lane-audit" => view.restate(
                    tau_goal::NAME,
                    &tau_goal::demo::records(
                        "Every lane has an owner in lanes.toml",
                        "stopped",
                    ),
                ),
                _ => {}
            }
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
            call: None,
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
        term_output(
            &format!(
                "{}{MAGENTA}     Summary{RESET} [   4.200s] 52 tests run: 52 {GREEN}passed{RESET}, 0 skipped\n",
                pass(0.012, "tau-ai ws::proto::pool::tests::deadline_is_jittered")
            ),
            0,
            1,
            None,
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
            .started("today 02:10")
            .with_origin(Origin::Fork {
                from: run_id(),
                turn: 1,
            }),
        s,
    );
    view.limits = retry_after().limits;
    view.context = retry_after().context;
    view
}

/// What landing the `backoff` fork would do:
/// two changes, and with `conflict`, one of them in conflict.
pub fn landing_preview(conflict: bool) -> tau_vcs::Landing {
    let change =
        |change_id: &str, commit_id: &str, description: &str, conflict| {
            tau_vcs::ChangeInfo {
                change_id: change_id.into(),
                commit_id: commit_id.into(),
                description: description.into(),
                empty: false,
                conflict,
                immutable: false,
                working_copy: false,
                divergent: false,
                bookmarks: Vec::new(),
            }
        };
    tau_vcs::Landing {
        changes: vec![
            change(
                "tnrlwzvoqkmxpulsyvrnzotqwmkpxsly",
                "5c1e9a0b77d24f3e8a61f0c2b9d4e7a3f10c8b26",
                "feat(tau-ai): cap backoff at 30 s, honor retry-after when present\n",
                conflict,
            ),
            change(
                "vmpxuqskrlonwyzptvqmsxkkoprunwlz",
                "9f02b6d4c1e87a53b0d9f6e2a4c7b1d8e3f50a19",
                "feat(tau-ai): jittered exponential backoff\n",
                false,
            ),
        ],
        conflicts: if conflict {
            vec!["crates/tau-ai/src/retry.rs".into()]
        } else {
            Vec::new()
        },
        head: "5c1e9a0b77d24f3e8a61f0c2b9d4e7a3f10c8b26".into(),
    }
}

pub fn fork_id() -> RunId {
    RunId(Arc::from("backoff"))
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
            call: None,
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
    s.start_tool(
        "f2",
        "bash",
        json!({ "command": "cargo nextest run -p tau-ai ws::" }),
    );
    s.fail_tool(
        0,
        "f2",
        term_output(
            &format!(
                "{RED}        FAIL{RESET} [   0.031s] tau-ai ws::proto::continuation::tests::survives_rotation\n\
                 {RED}        FAIL{RESET} [   0.027s] tau-ai ws::proto::lane::tests::drain_before_rotate\n\
                 \x20 lane 3 resumed on connection #7 after it was retired\n\
                 {MAGENTA}     Summary{RESET} [   4.400s] 52 tests run: 50 {GREEN}passed{RESET}, 2 {RED}failed{RESET}\n"
            ),
            100,
            1,
            None,
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
                from: run_id(),
                turn: 2,
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
            call: None,
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
    let mut view = RunView::new(
        id,
        crate::titles::placeholder(prompt),
        "coder",
        &model.model,
    )
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
            call: None,
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

/// The computer a demo phone pairs with.
pub fn computer() -> Computer {
    let mut fingerprint = [0; 32];
    for (n, byte) in fingerprint.iter_mut().enumerate() {
        *byte = (n as u8).wrapping_mul(67).wrapping_add(0x4f);
    }
    Computer {
        name: "cfcosta-desk".into(),
        address: tau_remote::Address {
            host: "100.84.12.7".into(),
            port: tau_remote::Address::DEFAULT_PORT,
        },
        fingerprint: tau_remote::Fingerprint(fingerprint),
        via: Some("over Tailscale".into()),
    }
}

/// The Phones screen with phones allowed, a code showing and a phone
/// paired.
pub fn phones() -> crate::phones::Phones {
    use crate::phones::{LocalAddress, Phones, ShownCode};
    let computer = computer();
    Phones {
        allowed: true,
        addresses: vec![
            LocalAddress {
                ip: "100.84.12.7".into(),
                label: "Tailscale".into(),
            },
            LocalAddress {
                ip: "192.168.1.20".into(),
                label: "wlan0".into(),
            },
        ],
        listen: Some("100.84.12.7".into()),
        port: tau_remote::Address::DEFAULT_PORT,
        listening: Some(computer.address.clone()),
        fingerprint: Some(computer.fingerprint),
        code: Some(ShownCode {
            code: tau_remote::PairingCode {
                address: computer.address,
                host: computer.name,
                fingerprint: computer.fingerprint,
                secret: tau_remote::PairingSecret::typed("K7QM-2XPA")
                    .expect("a valid secret"),
            },
            expires: std::time::Instant::now()
                + std::time::Duration::from_secs(272),
        }),
        paired: vec![tau_remote::Device {
            id: "d1".into(),
            name: "Pixel 9".into(),
            paired_at: "2026-09-28T19:12:40Z".into(),
            last_seen: Some("2026-09-30T08:41:02Z".into()),
        }],
        error: None,
    }
}

/// Pairing as it stands when `step` opens: halfway for the camera, a
/// certificate to compare for a typed address.
pub fn pairing(step: PairStep) -> Pairing {
    let computer = computer();
    let (address, fingerprint) =
        (computer.address.clone(), computer.fingerprint);
    match step {
        PairStep::Welcome => Pairing::default(),
        PairStep::Scan => Pairing {
            progress: Progress::Pairing {
                address,
                fingerprint,
            },
            ..Pairing::default()
        },
        PairStep::Address => Pairing {
            progress: Progress::Compare {
                address,
                fingerprint,
            },
            ..Pairing::default()
        },
        PairStep::Paired => Pairing {
            computer: Some(computer),
            ..Pairing::default()
        },
        PairStep::Unreachable => Pairing {
            computer: Some(computer),
            tries: 3,
            ..Pairing::default()
        },
    }
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
            label: "gpt-6.1-sol · ChatGPT plan".into(),
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
/// What the demo's main chat pushed to GitHub: three changes.
pub fn pushed() -> crate::push::Pushed {
    use crate::push::{Pushed, PushedChange};
    let change = |id: &str, title: &str| PushedChange {
        change_id: id.into(),
        title: title.into(),
    };
    Pushed {
        branch: "main".into(),
        from: Some("0fe6b403f1c2a9e8d7b6a5c4e3f2a1b0c9d8e7f6".into()),
        to: "8cc5b9ce2d4f6a8b0c1e3f5a7b9d1e3f5a7c9e1b".into(),
        changes: vec![
            change(
                "kmqzrtyvwlpnosux",
                "feat(host): load AGENTS.md into the system prompt",
            ),
            change(
                "vtponmlkzyxwsrqp",
                "fix(host): refuse to reopen a landed chat",
            ),
            change(
                "ywrnqpzokmlstuvx",
                "feat(vcs): sweep orphan workspaces at start",
            ),
        ],
    }
}

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

/// Starts the demo's host on `workspace`: the plugins' host halves fill
/// its catalog, and it answers what the workspace asks.
pub fn start(workspace: &Entity<Workspace>, cx: &mut App) -> Arc<DemoHost> {
    let host = Arc::new(
        DemoHost::new(workspace.read(cx).catalog())
            .expect("the demo's plugins have a place to keep their files"),
    );
    workspace.update(cx, |ws, cx| {
        let catalog = host.catalog(ws.catalog().clone());
        ws.apply(HostUpdate::catalog(catalog), cx);
    });
    respond(workspace, host.clone(), cx);
    host
}

/// Answers what the workspace asks for the way a host would, after a
/// pause: sign-ins succeed, clones progress, pull requests open, and
/// plugins act through their host halves in `host`. Every answer is a
/// [`HostUpdate`], so a paired phone sees it too.
fn respond(workspace: &Entity<Workspace>, host: Arc<DemoHost>, cx: &mut App) {
    if let Some(mut pushed) = host.pushed() {
        let (workspace, host) = (workspace.downgrade(), host.clone());
        cx.spawn(async move |cx| {
            while let Some(push) = pushed.recv().await {
                let update = match push {
                    Push::Record { run, plugin, body } => {
                        HostUpdate::PluginRecord { run, plugin, body }
                    }
                    Push::Catalog => match workspace.read_with(cx, |ws, _| {
                        host.catalog(ws.catalog().clone())
                    }) {
                        Ok(catalog) => HostUpdate::catalog(catalog),
                        Err(_) => return,
                    },
                    Push::Alert { title, message } => {
                        HostUpdate::alert(title, message)
                    }
                };
                if workspace.update(cx, |ws, cx| ws.apply(update, cx)).is_err()
                {
                    return;
                }
            }
        })
        .detach();
    }
    cx.subscribe(workspace, move |workspace, event: &WorkspaceEvent, cx| {
        // Each answer comes after its pause, in milliseconds.
        let later = |steps: Vec<(u64, Answer)>, cx: &mut App| {
            let workspace = workspace.downgrade();
            cx.spawn(async move |cx| {
                for (wait, answer) in steps {
                    cx.background_executor()
                        .timer(Duration::from_millis(wait))
                        .await;
                    let done = workspace.update(cx, |ws, cx| match answer {
                        Answer::Host(update) => ws.apply(update, cx),
                        Answer::Pair(update) => ws.update_pairing(update, cx),
                    });
                    if done.is_err() {
                        return;
                    }
                }
            })
            .detach();
        };
        // The catalog as `change` leaves it.
        let recatalog = |change: &dyn Fn(&mut Catalog), cx: &mut App| {
            workspace.update(cx, |ws, cx| {
                let mut catalog = ws.catalog().clone();
                change(&mut catalog);
                ws.apply(HostUpdate::catalog(catalog), cx);
            });
        };
        let setup = |update| Answer::Host(HostUpdate::Setup(update));
        let signed_in = || {
            setup(SetupUpdate::GitHub(GitHub::SignedIn {
                user: "cfcosta".into(),
            }))
        };
        let computer = computer();
        // Pairing is the phone's own: the computer it reaches answers.
        let progress =
            |progress| Answer::Pair(PairingUpdate::Progress(progress));
        let connecting = || {
            progress(Progress::Connecting {
                address: computer.address.clone(),
            })
        };
        let certificate = Progress::Pairing {
            address: computer.address.clone(),
            fingerprint: computer.fingerprint,
        };
        match event {
            // The camera reads the code at once; the computer answers,
            // its certificate matches, and it pairs.
            WorkspaceEvent::Pair(PairRequest::Scan) => later(
                vec![
                    (900, connecting()),
                    (700, progress(certificate)),
                    (900, Answer::Pair(PairingUpdate::Paired(computer))),
                ],
                cx,
            ),
            WorkspaceEvent::Pair(PairRequest::Typed { .. }) => {
                let compare = Progress::Compare {
                    address: computer.address.clone(),
                    fingerprint: computer.fingerprint,
                };
                later(vec![(700, progress(compare))], cx)
            }
            WorkspaceEvent::Pair(PairRequest::Trust(_)) => later(
                vec![(800, Answer::Pair(PairingUpdate::Paired(computer)))],
                cx,
            ),
            // The computer answers the second time.
            WorkspaceEvent::Pair(PairRequest::Retry) => later(
                vec![(1200, Answer::Pair(PairingUpdate::Connected(computer)))],
                cx,
            ),
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
            // The browser never opens in the demo: the page waits, and a
            // pasted redirect finishes it.
            WorkspaceEvent::ChatGptSignIn { .. } => {
                let pending = ModelAccess::SigningIn {
                    url: Some(DEMO_AUTHORIZE_URL.into()),
                };
                later(vec![(300, setup(SetupUpdate::Model(pending)))], cx)
            }
            WorkspaceEvent::ChatGptCallback { .. } => {
                let connected = ModelAccess::Connected {
                    label: "gpt-6.1-sol · ChatGPT plan".into(),
                };
                later(vec![(600, setup(SetupUpdate::Model(connected)))], cx)
            }
            WorkspaceEvent::SwitchChatGpt { account } => recatalog(
                &|catalog| {
                    let access = &mut catalog.models.access;
                    for known in &mut access.accounts {
                        known.active = known.id == *account;
                    }
                    access.chatgpt =
                        access.active_account().is_some_and(|active| {
                            active.state == AccountState::Plan
                        });
                    access.label = if access.chatgpt {
                        "ChatGPT plan".into()
                    } else {
                        "signed out".into()
                    };
                },
                cx,
            ),
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
                let update = HostUpdate::PullRequest {
                    run: run.clone(),
                    pr: Box::new(pull_request()),
                };
                workspace.update(cx, |ws, cx| ws.apply(update, cx));
            }
            WorkspaceEvent::Push { repo, .. } => {
                let pushed = HostUpdate::Pushed {
                    repo: repo.clone(),
                    result: Ok(pushed()),
                };
                later(vec![(1200, Answer::Host(pushed))], cx)
            }
            WorkspaceEvent::CreatePullRequest { run, .. } => {
                let opened = HostUpdate::PullRequestState {
                    run: run.clone(),
                    state: opened(),
                };
                later(vec![(1200, Answer::Host(opened))], cx)
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
                workspace.update(cx, |ws, cx| {
                    let Some(from) = ws.run(run).cloned() else {
                        return;
                    };
                    let turn = turn.unwrap_or(from.turn);
                    let (view, steps) =
                        fork_run(&from, turn, prompt, model, id.clone());
                    ws.apply(HostUpdate::Run(Box::new(view)), cx);
                    ws.replay(id, steps, cx);
                });
            }
            // Off the UI thread, as on a host: an action may ask Jev.
            WorkspaceEvent::PluginAct { plugin, action } => {
                let (job, plugin, action) =
                    (host.clone(), plugin.clone(), action.clone());
                let workspace = workspace.downgrade();
                let acting = cx.background_executor().spawn({
                    let plugin = plugin.clone();
                    async move { job.act(&plugin, action) }
                });
                let host = host.clone();
                cx.spawn(async move |cx| {
                    let done = acting.await;
                    let _ = workspace.update(cx, |ws, cx| {
                        match done {
                            Ok(Some(reply)) => ws.apply(
                                HostUpdate::PluginReply { plugin, reply },
                                cx,
                            ),
                            Ok(None) => {}
                            Err(error) => ws.apply(
                                HostUpdate::alert(
                                    format!("{plugin} could not do that"),
                                    format!("{error:#}"),
                                ),
                                cx,
                            ),
                        }
                        let catalog = host.catalog(ws.catalog().clone());
                        ws.apply(HostUpdate::catalog(catalog), cx);
                    });
                })
                .detach();
            }
            WorkspaceEvent::PluginSettings { plugin, settings } => {
                host.save_settings(plugin, settings.clone());
                workspace.update(cx, |ws, cx| {
                    let catalog = host.catalog(ws.catalog().clone());
                    ws.apply(HostUpdate::catalog(catalog), cx);
                });
            }
            WorkspaceEvent::JevKey { key } => {
                let saved = key.is_some();
                recatalog(&|catalog| catalog.models.access.jev = saved, cx);
            }
            // The demo's store answers every query with its runs' costs.
            WorkspaceEvent::Query { .. } => {
                let table = tau_store::Table {
                    columns: vec!["agent".into(), "sum(cost_usd)".into()],
                    rows: vec![
                        vec!["coder".into(), "4.312".into()],
                        vec!["reviewer".into(), "0.206".into()],
                    ],
                    truncated: false,
                };
                workspace.update(cx, |ws, cx| {
                    ws.apply(HostUpdate::QueryResult(Ok(table)), cx)
                });
            }
            // Updating finds nothing new.
            WorkspaceEvent::UpdateRepo { repo } => {
                let text = format!("{repo} is up to date");
                recatalog(&|catalog| catalog.update = Some(text.clone()), cx);
            }
            // A finished run goes on with one more turn.
            // The demo keeps what plugins store, and every branch.
            WorkspaceEvent::PluginRecord { run, plugin, body } => {
                let record = HostUpdate::PluginRecord {
                    run: run.clone(),
                    plugin: plugin.clone(),
                    body: body.clone(),
                };
                workspace.update(cx, |ws, cx| ws.apply(record, cx))
            }
            WorkspaceEvent::KeepBranch { run } => {
                let kept = HostUpdate::BranchKept(run.clone());
                workspace.update(cx, |ws, cx| ws.apply(kept, cx))
            }
            WorkspaceEvent::CloseRun { run } => workspace
                .update(cx, |ws, cx| {
                    ws.apply(HostUpdate::Closed(run.clone()), cx)
                }),
            // A message steers a run that is going; a finished one goes
            // on with one more turn.
            WorkspaceEvent::Say { run, text, model } => {
                workspace.update(cx, |ws, cx| {
                    let Some(view) = ws.run(run).cloned() else {
                        return;
                    };
                    if view.status.is_live() {
                        let (run, text) = (run.clone(), text.clone());
                        ws.apply(HostUpdate::Steered { run, text }, cx);
                        return;
                    }
                    let resumed = HostUpdate::Resumed {
                        run: run.clone(),
                        prompt: text.clone(),
                        model: model.clone(),
                    };
                    ws.apply(resumed, cx);
                    let steps = resume_script(&view, text);
                    ws.replay(run.clone(), steps, cx);
                });
            }
            // Signing out leaves the active account signed out.
            WorkspaceEvent::SignOut => recatalog(
                &|catalog| {
                    let access = &mut catalog.models.access;
                    access.chatgpt = false;
                    access.label = "signed out".into();
                    for account in &mut access.accounts {
                        if account.active {
                            account.state = AccountState::SignedOut;
                        }
                    }
                },
                cx,
            ),
            WorkspaceEvent::CompareCode { main, fork } => {
                let code = HostUpdate::BranchCode {
                    main: main.clone(),
                    fork: fork.clone(),
                    code: crate::view::CodeState::Ready(branch_code()),
                };
                later(vec![(400, Answer::Host(code))], cx)
            }
            other => eprintln!("tau-ui: {other:?}"),
        }
    })
    .detach();
}

enum Answer {
    Host(HostUpdate),
    Pair(PairingUpdate),
}

/// The code of `retry-after`, tau-agent's main chat, and its `backoff`
/// chat.
pub fn branch_code() -> BranchCode {
    let stat = |path: &str, kind, added, removed| FileStat {
        path: path.into(),
        kind,
        added,
        removed,
    };
    let lines = |text: &str| tau_ui_kit::diff::parse(text);
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

/// The repositories and store the demo workspace shows. Its plugins'
/// part comes from their host halves: [`DemoHost::catalog`].
pub fn catalog() -> Catalog {
    Catalog {
        plugin_data: Default::default(),
        plugin_settings: Default::default(),
        plugins: Vec::new(),
        agent: "coder".into(),
        agent_source: Some("src/agents.rs:14".into()),
        jev: Some(JevStats {
            model: "Jev 1.13".into(),
            key_env: "typesafe-key, set on Models".into(),
            price: "$0.042 / M input".into(),
            requests: 412,
            input_tokens: 1_940_000,
            spent: 0.081,
            latency_p50_ms: 180,
            failed: 3,
        }),
        repos: vec![
            // Its main chat has changes GitHub does not have yet.
            Repo {
                main: Some(run_id()),
                unpushed: 3,
                trunk: Some("main".into()),
                ..Repo::new("tau-agent", "~/Code/cfcosta/tau-agent")
            },
            Repo {
                main: Some(RunId(Arc::from("docbert-main"))),
                ..Repo::new("docbert", "~/Code/cfcosta/docbert")
            },
            Repo {
                main: Some(RunId(Arc::from("homelab-main"))),
                ..Repo::new("homelab.nix", "~/Code/cfcosta/homelab.nix")
            },
        ],
        open_repos: vec!["tau-agent".into()],
        closed_runs: Vec::new(),
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

/// Where the demo's ChatGPT sign-in would open: OpenAI's authorization
/// page, without the parameters of a real attempt.
pub const DEMO_AUTHORIZE_URL: &str =
    "https://auth.openai.com/api/accounts/authorize";

/// What the plan route answers when the plan's usage limit is reached,
/// as the transport keeps it.
pub fn usage_limit() -> tau_ai::refusal::Refusal {
    tau_ai::refusal::Refusal {
        recovery: tau_ai::retry::Recovery::UsageLimit,
        status: Some(429),
        code: Some("subscription_sharing_usage_limit_exceeded".into()),
        request_id: Some("req_demo".into()),
        body: String::new(),
        message: "OpenAI refused the request: HTTP 429 \
                  subscription_sharing_usage_limit_exceeded (request req_demo)"
            .into(),
    }
}

/// The demo's ChatGPT sign-ins: a personal account whose plan runs use,
/// and a work one.
pub fn chatgpt_accounts() -> Vec<ChatGptAccount> {
    vec![
        ChatGptAccount {
            id: "oaiapp_demo1-1a2b3c".into(),
            label: "you@example.com".into(),
            state: AccountState::Plan,
            active: true,
        },
        ChatGptAccount {
            id: "oaiapp_demo2-4d5e6f".into(),
            label: "you@work.example".into(),
            state: AccountState::PlanDisabled,
            active: false,
        },
    ]
}

/// The models the demo offers: the plan's, from the model table, with
/// defaults for each of the demo's agents.
pub fn models() -> crate::models::Models {
    use crate::models::{
        AccessInfo,
        Effort,
        ModelChoice,
        ModelSettings,
        Models,
        plan_models,
    };
    let mut settings = ModelSettings::default();
    settings.set_default(
        "reviewer",
        ModelChoice::new("gpt-6-luna", Effort::Medium),
    );
    settings
        .set_default("tau-memory", ModelChoice::new("gpt-6-luna", Effort::Low));
    Models {
        options: plan_models(),
        settings,
        access: AccessInfo {
            label: "ChatGPT plan".into(),
            chatgpt: true,
            jev: true,
            accounts: chatgpt_accounts(),
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
        tau_fast_compaction::NAME,
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

    /// What a plugin charged to the run, as the run emits it.
    fn charged(&mut self, plugin: &str, cost: f64) {
        let mut usage = tau_ai::message::Usage::default();
        usage.cost.total = cost;
        self.event(
            0,
            RunEvent::PluginCharged {
                run: self.run.clone(),
                plugin: plugin.into(),
                usage,
            },
        );
    }

    /// A report from tau-constitution.
    fn verdict(&mut self, body: Value) {
        self.report("tau-constitution", body);
    }

    /// A plugin's report, as the run emits it.
    fn report(&mut self, plugin: &str, body: Value) {
        self.event(
            60,
            RunEvent::PluginReport {
                run: self.run.clone(),
                plugin: Arc::from(plugin),
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
                parent: None,
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
                parent: None,
            },
        );
    }

    /// Ends a call that failed, keeping its output (a `bash` command
    /// that exited non-zero).
    fn fail_tool(&mut self, millis: u64, id: &str, output: ToolOutput) {
        self.event(
            millis,
            RunEvent::ToolEnd {
                run: self.run.clone(),
                call_id: id.into(),
                output: Arc::new(output),
                is_error: true,
                parent: None,
            },
        );
    }

    /// A progress update of a command under a terminal: `ansi` as chunk
    /// `seq`, with the text so far.
    fn term_chunk(
        &mut self,
        millis: u64,
        id: &str,
        seq: u64,
        ansi: &str,
        so_far: &str,
    ) {
        self.event(
            millis,
            RunEvent::ToolUpdate {
                run: self.run.clone(),
                call_id: id.into(),
                partial: Arc::new(ToolOutput {
                    details: Some(json!({ "term": {
                        "seq": seq,
                        "bytes": base64_of(&crlf(ansi)),
                    }})),
                    ..ToolOutput::text(plain(so_far))
                }),
                parent: None,
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
}

/// A terminal's line ends: the terminal turns each `\n` into `\r\n`.
fn crlf(text: &str) -> Vec<u8> {
    text.replace('\n', "\r\n").into_bytes()
}

fn base64_of(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// `ansi` less its SGR sequences: the text a model gets of it.
fn plain(ansi: &str) -> String {
    let mut out = String::new();
    let mut chars = ansi.chars();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            for ch in chars.by_ref() {
                if ch.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// What `bash` returns for a command run under a 120×40 terminal that
/// wrote `ansi` and exited with `code`: the plain text for the model
/// (or `seen`, when output pruning replaced it), and the raw output in
/// its details.
fn term_output(
    ansi: &str,
    code: i32,
    chunks: u64,
    seen: Option<String>,
) -> ToolOutput {
    let bytes = crlf(ansi);
    let text = plain(ansi);
    let text = if code == 0 {
        text
    } else {
        format!("{}\n\nCommand exited with code {code}", text.trim_end())
    };
    ToolOutput {
        details: Some(json!({ "term": {
            "cols": 120, "rows": 40, "status": "exited", "exitCode": code,
            "chunks": chunks, "outputBytes": bytes.len(), "replay": "stream",
            "bytes": base64_of(&bytes),
        }})),
        ..ToolOutput::text(seen.unwrap_or(text))
    }
}

/// Colors cargo and nextest write in.
const GREEN: &str = "\x1b[1;32m";
const RED: &str = "\x1b[1;31m";
const YELLOW: &str = "\x1b[1;33m";
const BLUE: &str = "\x1b[1;34m";
const MAGENTA: &str = "\x1b[1;35m";
const CYAN: &str = "\x1b[1;36m";
const GRAY: &str = "\x1b[90m";
const RESET: &str = "\x1b[0m";

fn pass(seconds: f32, test: &str) -> String {
    format!("{GREEN}        PASS{RESET} [{seconds:>8.3}s] {test}\n")
}

fn compiling(krate: &str, path: &str) -> String {
    format!(
        "{GREEN}   Compiling{RESET} {krate} v0.1.0 (/home/you/Code/tau-agent/{path})\n"
    )
}

/// A failing `cargo nextest run -p tau-ai`: a warning while it builds,
/// then a failed test with its panic.
fn failing_tests() -> String {
    let mut out = String::new();
    out.push_str(&compiling("tau-ai", "crates/tau-ai"));
    out.push_str(&format!(
        "{YELLOW}warning{RESET}\x1b[1m: unused variable: `hint`{RESET}\n\
         {BLUE}   -->{RESET} crates/tau-ai/src/retry.rs:88:13\n\
         {BLUE}    |{RESET}\n\
         {BLUE} 88 |{RESET}         let hint = server_hint(&headers);\n\
         {BLUE}    |{RESET}             {YELLOW}^^^^ help: if this is intentional, prefix it with an underscore: `_hint`{RESET}\n\
         {GREEN}    Finished{RESET} `test` profile [unoptimized + debuginfo] target(s) in 38.90s\n\
         {GREEN}    Starting{RESET} 212 tests across 9 binaries\n"
    ));
    for (seconds, test) in [
        (0.004, "tau-ai retry::tests::honors_seconds"),
        (0.011, "tau-ai retry::tests::delay_is_capped"),
        (0.009, "tau-ai retry::tests::server_hint_parses_seconds"),
    ] {
        out.push_str(&pass(seconds, test));
    }
    out.push_str(&format!(
        "{RED}        FAIL{RESET} [   0.019s] tau-ai retry::tests::honors_http_date\n\
         {RED}--- STDERR:{RESET} tau-ai retry::tests::honors_http_date {RED}---{RESET}\n\
         thread 'honors_http_date' panicked at crates/tau-ai/src/retry.rs:141:9:\n\
         assertion `left == right` failed\n\
         \x20 left: {RED}None\x1b[0m\n\
         \x20right: {GREEN}Some(120s){RESET}\n\
         {GRAY}────────────{RESET}\n\
         {MAGENTA}     Summary{RESET} [  41.207s] 212 tests run: 211 {GREEN}passed{RESET}, 1 {RED}failed{RESET}, 0 skipped\n\
         {RED}        FAIL{RESET} [   0.019s] tau-ai retry::tests::honors_http_date\n\
         {RED}error{RESET}: test run failed\n"
    ));
    out
}

/// The whole-workspace test run output pruning cut down: 4,810 lines.
fn workspace_tests() -> String {
    let mut out = format!(
        "{GREEN}    Starting{RESET} 4,403 tests across 61 binaries (12 tests skipped)\n"
    );
    let crates = [
        "tau-agent",
        "tau-ai",
        "tau-store",
        "tau-ui",
        "tau-memory",
        "tau-vcs",
    ];
    for i in 0..4_807 {
        let krate = crates[i % crates.len()];
        out.push_str(&pass(
            0.001 * (i % 97) as f32,
            &format!("{krate} tests::case_{i:04}"),
        ));
    }
    out.push_str(&format!(
        "{GRAY}────────────{RESET}\n\
         {MAGENTA}     Summary{RESET} [  88.514s] 4,391 tests run: 4,391 {GREEN}passed{RESET}, 12 {YELLOW}skipped{RESET}\n"
    ));
    out
}

/// An output that says how to sum itself up, as tools may.
/// The session's `ls` of `crates/tau-ai/src`, before it greps: the
/// folders and files there, `retry.rs` already modified in `@`.
pub const LS_CALL: &str = "c1a";

fn listing() -> ToolOutput {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64);
    let dir = |name: &str, items: u64, hours: i64| {
        json!({
            "name": name, "kind": "dir", "items": items,
            "modified": now - hours * 3_600,
        })
    };
    let file = |name: &str, size: u64, hours: i64| {
        json!({
            "name": name, "kind": "file", "size": size,
            "modified": now - hours * 3_600,
        })
    };
    let mut retry = file("retry.rs", 15_620, 0);
    retry["change"] = json!("modified");
    retry["modified"] = json!(now - 240);
    let entries = vec![
        dir("chatgpt", 6, 30),
        file("client.rs", 6_690, 52),
        file("cost.rs", 5_216, 200),
        file("event.rs", 11_458, 52),
        file("http.rs", 19_503, 9),
        file("lib.rs", 292, 400),
        file("llm.rs", 5_047, 200),
        file("message.rs", 7_300, 52),
        file("model.rs", 13_683, 30),
        file("partial_json.rs", 32_868, 900),
        file("refusal.rs", 5_590, 200),
        dir("responses", 3, 52),
        file("responses.rs", 118, 900),
        retry,
        dir("ws", 4, 9),
        file("ws.rs", 102, 900),
    ];
    let text = entries
        .iter()
        .map(|entry| {
            let name = entry["name"].as_str().unwrap_or_default();
            if entry["kind"] == "dir" {
                format!("{name}/")
            } else {
                name.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    ToolOutput {
        details: Some(json!({
            "dir": "/home/me/tau-agent/crates/tau-ai/src",
            "entries": entries,
            "truncated": false,
        })),
        ..ToolOutput::text(text)
    }
}

fn summarized(output: ToolOutput, summary: &str) -> ToolOutput {
    ToolOutput {
        details: Some(json!({ "summary": summary })),
        ..output
    }
}

/// The file the retry demo reads, for its card to show and color.
const RETRY_RS: &str = r#"//! How long to wait before trying a request again.

use std::time::Duration;

/// Backoff between attempts, doubling up to `max_delay`.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub base: Duration,
    pub max_delay: Duration,
    pub attempts: u32,
}

impl RetryPolicy {
    /// The wait before attempt `n`, counting from 1.
    pub fn delay(&self, n: u32) -> Duration {
        let factor = 2u32.saturating_pow(n.saturating_sub(1));
        self.base.saturating_mul(factor).min(self.max_delay)
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            base: Duration::from_millis(250),
            max_delay: Duration::from_secs(30),
            attempts: 5,
        }
    }
}"#;

fn lines(count: usize) -> ToolOutput {
    ToolOutput::text(
        (1..=count)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// The session's `vcs_log` call.
pub const LOG_CALL: &str = "c12";
/// The change `--open log` picks in it: the commit with a body.
pub const LOG_PICKED: &str = "onvkmqwosvvznnzyzztmonuszpkqvokt";

/// The newest trunk commit in the session's log, where `main` points.
const TRUNK_HEAD: &str = "smrmsnykuxsolrnpslpqttqtypsvkslk";

/// What `vcs_log` returns at the end of the session: the run's turns
/// and commits over trunk.
fn change_log() -> ToolOutput {
    let changes = [
        (
            "uowlmnvlqlmxxmrmxlnrlwlrlotxontp",
            "36b3216fdaeeb975729fae923d5a4fd12aabfe22",
            "",
            "@",
        ),
        (
            "szmltytwvkyvpnzlqtorwwzmpywsoxsx",
            "bc74254770f58904dba41ecccc3fc1626e53a130",
            "test(tau-ai): retry-after as seconds, as a date, capped, malformed\n",
            "",
        ),
        (
            "onvkmqwosvvznnzyzztmonuszpkqvokt",
            "28b5b7a767c76fb008f86bebb2737f6a6f0fb23c",
            "feat(tau-ai): honor retry-after on 429 and 503\n\nThe server's \
          hint wins over our backoff, capped at the policy's max_delay. A \
          header that does not parse falls back to the backoff.\n",
            "",
        ),
        (
            "qzpxumwywmppokoyozvookknoxqqksqt",
            "7a8d41bed440e50454f31af3176813e02ea68ef7",
            "refactor(tau-ai): the policy takes the parsed hint\n",
            "",
        ),
        (
            "sqyoxnwyumrxmqtnovosoyrnwzprpxwu",
            "d6ba2b0aee0ca923732881584d8c4fa2815d2802",
            "feat(tau-ai): parse retry-after as seconds or a date\n",
            "",
        ),
        (
            "smrmsnykuxsolrnpslpqttqtypsvkslk",
            "06f7e3dfc967a64cb14028d512c9791e558e08ba",
            "fix(tau-ui): a message's bubble holds all its lines\n",
            "immutable",
        ),
        (
            "urltqvpkuwmzsqrkmsmowlwkttrmowuz",
            "4941d4072014b3ce107f80e222f828767efc2f91",
            "feat(tau-agent): plugins see the transcript a rewrite replaced\n",
            "immutable",
        ),
        (
            "qmoustokzlzsnqzttyyynqtmzktymysw",
            "662248b483b7ffc050fec94dbca3a0aac36098b2",
            "docs: decide tau-memory's design\n",
            "immutable",
        ),
    ];
    let text = changes
        .iter()
        .map(|(change, commit, description, flag)| {
            let flags = match *flag {
                "@" => " @ (empty)",
                "immutable" if *change == TRUNK_HEAD => " (immutable) [main]",
                "immutable" => " (immutable)",
                _ => "",
            };
            let line =
                description.lines().next().unwrap_or("(no description set)");
            format!("{} {}{flags} {line}", &change[..12], &commit[..12])
        })
        .chain([
            "\n[Showing the newest 8 changes. Use limit=16 for more]".into()
        ])
        .collect::<Vec<_>>()
        .join("\n");
    let changes: Vec<Value> = changes
        .iter()
        .map(|(change, commit, description, flag)| {
            json!({
                "change_id": change,
                "commit_id": commit,
                "description": description,
                "empty": *flag == "@",
                "conflict": false,
                "immutable": *flag == "immutable",
                "working_copy": *flag == "@",
                "divergent": false,
                "bookmarks": if *change == TRUNK_HEAD { vec!["main"] } else { vec![] },
            })
        })
        .collect();
    ToolOutput {
        details: Some(json!({ "changes": changes, "more": true })),
        ..ToolOutput::text(text)
    }
}

/// The session's `vcs_show` call, on the commit that honors the header.
pub const SHOW_CALL: &str = "c13";
/// The session's `vcs_diff` call, on the commit that adds the tests.
pub const DIFF_CALL: &str = "c14";
/// The files `--open status`, `--open show` and `--open diff` open in
/// their cards.
pub const SHOW_FILE: &str = "crates/tau-ai/src/retry.rs";
pub const DIFF_FILE: &str = "crates/tau-ai/tests/retry_after.rs";

fn vcs_change(change: &str, commit: &str, description: &str) -> Value {
    json!({
        "change_id": change, "commit_id": commit, "description": description,
        "empty": false, "conflict": false, "immutable": false,
        "working_copy": false, "divergent": false, "bookmarks": [],
    })
}

/// A tool result whose text is `diff` after `head`, and whose details
/// hold the diff and its files.
fn vcs_output(head: &str, diff: &str, mut details: Value) -> ToolOutput {
    let files: Vec<Value> = diff_files(diff);
    details["files"] = json!(files);
    details["diff"] = json!(diff);
    details["truncated"] = json!(false);
    ToolOutput {
        details: Some(details),
        ..ToolOutput::text(format!("{head}{diff}").trim_end().to_owned())
    }
}

/// The files a diff touches, as the tools list them.
fn diff_files(diff: &str) -> Vec<Value> {
    tau_vcs::ui::change_diff::parse_files(diff)
        .into_iter()
        .map(|file| json!({ "path": file.path, "kind": file.kind }))
        .collect()
}

/// The change that honors `retry-after`: what `vcs_status` finds in
/// `@` right after the edit, and what `vcs_show` shows once it is
/// described.
const HONOR_RETRY_AFTER: &str = "\
diff --git a/crates/tau-ai/src/backoff.rs b/crates/tau-ai/src/backoff.rs
deleted file mode 100644
--- a/crates/tau-ai/src/backoff.rs
+++ /dev/null
@@ -1,7 +0,0 @@
-//! Exponential backoff, before RetryPolicy took it over.
-
-use std::time::Duration;
-
-pub fn backoff(base: Duration, attempt: u32) -> Duration {
-    base * 2u32.saturating_pow(attempt)
-}
diff --git a/crates/tau-ai/src/client.rs b/crates/tau-ai/src/client.rs
--- a/crates/tau-ai/src/client.rs
+++ b/crates/tau-ai/src/client.rs
@@ -112,6 +112,7 @@
             let response = self.http.execute(request.try_clone()?).await?;
             if matches!(response.status().as_u16(), 429 | 503) && attempt < max {
-                tokio::time::sleep(self.retry.delay(attempt)).await;
+                let hint = RetryPolicy::hint(response.headers(), SystemTime::now());
+                tokio::time::sleep(self.retry.delay(attempt, hint)).await;
                 attempt += 1;
                 continue;
             }
diff --git a/crates/tau-ai/src/lib.rs b/crates/tau-ai/src/lib.rs
--- a/crates/tau-ai/src/lib.rs
+++ b/crates/tau-ai/src/lib.rs
@@ -3,6 +3,5 @@
 //! Model clients for tau.
 
-mod backoff;
 pub mod client;
 pub mod message;
 pub mod retry;
diff --git a/crates/tau-ai/src/retry.rs b/crates/tau-ai/src/retry.rs
--- a/crates/tau-ai/src/retry.rs
+++ b/crates/tau-ai/src/retry.rs
@@ -38,9 +38,24 @@ impl RetryPolicy {
     /// How long to wait before attempt `attempt`.
-    pub fn delay(&self, attempt: u32) -> Duration {
-        let backoff = self.base * 2u32.saturating_pow(attempt);
-        backoff.min(self.max_delay)
+    /// A server's `retry-after` wins over the backoff, capped at
+    /// `max_delay`.
+    pub fn delay(&self, attempt: u32, hint: Option<Duration>) -> Duration {
+        let wait = match hint {
+            Some(hint) => hint,
+            None => self.base * 2u32.saturating_pow(attempt),
+        };
+        wait.min(self.max_delay)
     }
 
+    /// Reads `retry-after` as seconds or an HTTP date.
+    pub fn hint(headers: &HeaderMap, now: SystemTime) -> Option<Duration> {
+        let value = headers.get(RETRY_AFTER)?.to_str().ok()?;
+        if let Ok(secs) = value.trim().parse::<u64>() {
+            return Some(Duration::from_secs(secs));
+        }
+        let date = httpdate::parse_http_date(value).ok()?;
+        Some(date.duration_since(now).unwrap_or_default())
+    }
+
     pub fn attempts(&self) -> u32 {
         self.attempts
     }
";

/// The session's `vcs_status` call, right after the edit: `@` holds the
/// change that later becomes the one that honors `retry-after`, not yet
/// described. jj keeps its change id through the describe.
pub const STATUS_CALL: &str = "c15";

fn change_status() -> ToolOutput {
    let parent = vcs_change(
        "qzpxumwywmppokoyozvookknoxqqksqt",
        "7a8d41bed440e50454f31af3176813e02ea68ef7",
        "refactor(tau-ai): the policy takes the parsed hint\n",
    );
    let mut working_copy =
        vcs_change(LOG_PICKED, "5e0c93f1b27a4d86c2f0e9b1a37d58c46f21e0b9", "");
    working_copy["working_copy"] = json!(true);
    let files = diff_files(HONOR_RETRY_AFTER);
    let text = format!(
        "Working copy (@): onvkmqwosvvz 5e0c93f1b27a @ (no description set)\n\
         Parent (@-):      qzpxumwywmpp 7a8d41bed440 refactor(tau-ai): the policy takes the parsed hint\n\
         Working copy changes:\n{}",
        files
            .iter()
            .map(|file| {
                let letter = match file["kind"].as_str() {
                    Some("added") => "A",
                    Some("removed") => "D",
                    _ => "M",
                };
                format!(
                    "{letter} {}",
                    file["path"].as_str().unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    );
    ToolOutput {
        details: Some(json!({
            "working_copy": working_copy,
            "parents": [parent],
            "changes": files,
            "conflicts": [],
            "too_large": [],
            "diff": HONOR_RETRY_AFTER,
            "truncated": false,
        })),
        ..ToolOutput::text(text)
    }
}

fn change_show() -> ToolOutput {
    let description = "feat(tau-ai): honor retry-after on 429 and 503\n\n\
                       The server's hint wins over our backoff, capped at the \
                       policy's max_delay. A header that does not parse falls \
                       back to the backoff.\n";
    let head = format!(
        "Change ID: {LOG_PICKED}\nCommit ID: 28b5b7a767c76fb008f86bebb2737f6a6f0fb23c\n\
         Author: tau <tau@localhost>\n\
         Parent: qzpxumwywmpp 7a8d41bed440 refactor(tau-ai): the policy takes the parsed hint\n\n{}\n",
        description
            .lines()
            .map(|line| format!("    {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    vcs_output(
        &head,
        HONOR_RETRY_AFTER,
        json!({
            "change": vcs_change(
                LOG_PICKED,
                "28b5b7a767c76fb008f86bebb2737f6a6f0fb23c",
                description,
            ),
            "parents": [vcs_change(
                "qzpxumwywmppokoyozvookknoxqqksqt",
                "7a8d41bed440e50454f31af3176813e02ea68ef7",
                "refactor(tau-ai): the policy takes the parsed hint\n",
            )],
            "author": { "name": "tau", "email": "tau@localhost" },
        }),
    )
}

fn change_diff() -> ToolOutput {
    let diff = "\
diff --git a/crates/tau-ai/tests/retry_after.rs b/crates/tau-ai/tests/retry_after.rs
new file mode 100644
--- /dev/null
+++ b/crates/tau-ai/tests/retry_after.rs
@@ -0,0 +1,21 @@
+//! A 429 or 503 with `retry-after` waits as long as the server says,
+//! up to the policy's `max_delay`.
+
+use std::time::Duration;
+
+use tau_testing::fake_openai::{FakeServer, Reply};
+
+#[tokio::test(start_paused = true)]
+async fn honors_seconds() {
+    let server = FakeServer::start([
+        Reply::status(429).header(\"retry-after\", \"3\"),
+        Reply::ok(),
+    ]);
+    let started = tokio::time::Instant::now();
+    server.client().send(server.request()).await.unwrap();
+    assert_eq!(started.elapsed(), Duration::from_secs(3));
+}
+
+#[tokio::test(start_paused = true)]
+async fn caps_at_max_delay() {
+    // ...
diff --git a/crates/tau-testing/src/fake_openai.rs b/crates/tau-testing/src/fake_openai.rs
--- a/crates/tau-testing/src/fake_openai.rs
+++ b/crates/tau-testing/src/fake_openai.rs
@@ -58,4 +58,10 @@ impl Reply {
     pub fn status(code: u16) -> Self {
         Self { code, ..Self::ok() }
     }
+
+    /// Sends `name: value` with the reply.
+    pub fn header(mut self, name: &str, value: &str) -> Self {
+        self.headers.push((name.to_owned(), value.to_owned()));
+        self
+    }
 }
";
    vcs_output(
        "",
        diff,
        json!({
            "change": vcs_change(
                "szmltytwvkyvpnzlqtorwwzmpywsoxsx",
                "bc74254770f58904dba41ecccc3fc1626e53a130",
                "test(tau-ai): retry-after as seconds, as a date, capped, malformed\n",
            ),
        }),
    )
}

/// An `edit` call and its result. `lines` are the hunk's lines; the
/// hunk header that counts them goes on top, as the tool writes it.
fn edit(path: &str, new_text: &str, lines: &str) -> (Value, ToolOutput) {
    let old = lines.lines().filter(|l| !l.starts_with('+')).count();
    let new = lines.lines().filter(|l| !l.starts_with('-')).count();
    let diff = format!("@@ -131,{old} +131,{new} @@\n{lines}");
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
            call: None,
        },
    );

    // tau-reasoning and tau-memory run in `start`, before the session.
    s.report(
        "tau-reasoning",
        json!({
            "kind": "choice", "verdict": "chose", "effort": "high", "confidence": 0.84,
            "threshold": 0.7, "cost": 0.00002,
            "levels": [
                { "effort": "none", "suits": "no thought", "p": 0.01 },
                { "effort": "low", "suits": "small edits", "p": 0.02 },
                { "effort": "medium", "suits": "routine code", "p": 0.09 },
                { "effort": "high", "suits": "refactors", "p": 0.84 },
                { "effort": "xhigh", "suits": "audits, proofs", "p": 0.04 },
            ],
        }),
    );
    // tau-memory found notes for the task as the run started.
    s.report(
        tau_memory::plugin::NAME,
        json!({
            "kind": "recalled",
            "notes": [
                { "id": "n-0388", "title": "Retry policy honors server hints" },
                { "id": "n-0390", "title": "429 vs 503 in the Responses API" },
                { "id": "n-0205", "title": "Tests use the fake OpenAI server" },
            ],
        }),
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
        120,
        LS_CALL,
        "ls",
        json!({ "path": "crates/tau-ai/src" }),
        listing(),
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
        ToolOutput::text(RETRY_RS),
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
    s.charged("tau-constitution", 0.00007);
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
    // The first test run fails: the date parser is not wired in yet.
    s.start_tool(
        "c5t",
        "bash",
        json!({ "command": "cargo nextest run -p tau-ai" }),
    );
    s.fail_tool(900, "c5t", term_output(&failing_tests(), 100, 4, None));
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
            call: None,
        },
    );

    s.turn();
    // Output pruning trims the test run's log before the model sees it.
    s.start_tool(
        "c7",
        "bash",
        json!({ "command": "cargo nextest run --workspace" }),
    );
    let archive = "/home/you/.local/share/tau/repos/tau-agent-3f2a91c0/\
                   archive/tau-output-41822-1790716482-0.txt";
    s.report(
        tau_fast_compaction::NAME,
        json!({
            "kind": "output", "call_id": "c7", "lines": 4810,
            "chunks": 200, "kept": 212, "dropped_lines": 4598,
            "segments": 1, "requests": 2, "tokens_before": 14_200,
            "tokens_after": 1_100, "pruned": true, "archive": archive,
        }),
    );
    s.end_tool(
        800,
        "c7",
        term_output(
            &workspace_tests(),
            0,
            310,
            Some(format!(
                "{}\n    Starting 4,403 tests across 61 binaries (12 tests skipped)\n\
                 [1960 lines omitted]\n\
                 \x20       PASS [   1.902s] tau-ui host::a_large_output_is_archived\n\
                 [2634 lines omitted]\n\
                 \x20    Summary [  88.514s] 4,391 tests run: 4,391 passed, 12 skipped\n\n\
                 [full output: {archive} (read or grep it if needed)]",
                tau_fast_compaction::output::HEADER
            )),
        ),
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

    // fast-compaction prunes at the turn boundary: its ledger, then the
    // rewrite, as a run reports them.
    s.charged(tau_fast_compaction::NAME, 0.0011);
    let decision =
        |call_id: &str, tool: &str, action: &str, odds: (f64, f64)| {
            json!({
                "call_id": call_id, "tool": tool, "action": action,
                "keep_call": odds.0, "keep_result": odds.1,
            })
        };
    s.report(
        tau_fast_compaction::NAME,
        json!({
            "kind": "ledger",
            "decisions": [
                decision("c2", "grep", "keep", (0.64, 0.22)),
                decision("c3", "read", "drop_result", (0.71, 0.18)),
                decision("c5", "edit", "keep", (0.94, 0.88)),
                decision("c6", "bash", "keep", (0.18, 0.04)),
                decision("c7", "bash", "keep", (0.62, 0.03)),
                decision("c8", "read", "drop_result", (0.52, 0.09)),
                decision("c9", "read", "drop_result", (0.58, 0.11)),
            ],
            "stats": {
                "calls": 9, "pinned": 2, "kept": 4, "results_dropped": 3,
                "calls_dropped": 0, "requests": 1, "state_tokens": 18_400,
                "state_stage": "whole", "chars_before": 688_000,
                "chars_after": 324_000, "reduction_ratio": 0.53,
                "cost": 0.0011,
            },
        }),
    );
    s.event(
        700,
        RunEvent::ContextRewritten {
            run: run.clone(),
            plugin: Arc::from(tau_fast_compaction::NAME),
            tokens_before: 172_000,
            tokens_after: 81_000,
        },
    );
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
    s.tool(150, STATUS_CALL, "vcs_status", json!({}), change_status());
    s.start_tool(
        "c11",
        "bash",
        json!({ "command": "cargo nextest run -p tau-ai retry::" }),
    );
    let mut log = compiling("tau-ai", "crates/tau-ai");
    log.push_str(&format!(
        "{GREEN}    Finished{RESET} `test` profile [unoptimized + debuginfo] target(s) in 2.71s\n\
         {GREEN}    Starting{RESET} 14 tests across 1 binary (198 tests skipped)\n"
    ));
    // A progress bar, redrawn in place and erased once the build ends.
    let building = format!(
        "{CYAN}    Building{RESET} [=========================>   ] 11/12: tau-ai(test)\r\x1b[K"
    );
    let mut raw = format!("{building}{log}");
    s.term_chunk(400, "c11", 0, &raw.clone(), &log.clone());
    for (seq, test) in [
        "retry::tests::honors_seconds",
        "retry::tests::honors_http_date",
        "retry::tests::caps_at_max_delay",
        "retry::tests::ignores_malformed_header",
    ]
    .into_iter()
    .enumerate()
    {
        let line = pass(0.01, &format!("tau-ai {test}"));
        log.push_str(&line);
        raw.push_str(&line);
        let so_far = log.clone();
        s.term_chunk(500, "c11", seq as u64 + 1, &line, &so_far);
    }
    let summary = format!(
        "{GRAY}────────────{RESET}\n\
         {MAGENTA}     Summary{RESET} [   3.104s] 14 tests run: 14 {GREEN}passed{RESET}, 198 {YELLOW}skipped{RESET}\n"
    );
    log.push_str(&summary);
    raw.push_str(&summary);
    let so_far = log.clone();
    s.term_chunk(300, "c11", 5, &summary, &so_far);
    s.end_tool(300, "c11", term_output(&raw, 0, 6, Some(plain(&log))));
    s.tool(200, LOG_CALL, "vcs_log", json!({}), change_log());
    s.tool(
        200,
        SHOW_CALL,
        "vcs_show",
        json!({ "change": "onvkmqwo" }),
        change_show(),
    );
    s.tool(
        200,
        DIFF_CALL,
        "vcs_diff",
        json!({ "change": "szmltytw" }),
        change_diff(),
    );
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
         new:\n\n\
         | Test | Header | Waits |\n\
         |---|---|--:|\n\
         | `honors_seconds` | `retry-after: 3` | 3 s |\n\
         | `honors_http_date` | an HTTP date | until then |\n\
         | `caps_at_max_delay` | `retry-after: 600` | 30 s |\n\
         | `ignores_malformed_header` | `retry-after: soon` | backoff |\n\n\
         The header's rules are in [RFC 9110, section 10.2.3]\
         (https://www.rfc-editor.org/rfc/rfc9110#section-10.2.3). Still open:\n\n\
         1. *Jitter* on top of the hint, so clients don't retry in step.\n\
         2. Logging a malformed header ~~every time~~ once per run.",
    );
    s.end_turn(82_600, 300, 0.012);
    s.checked(None, &[("R6", 0.04)]);
    s.charged("tau-constitution", 0.012);
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
    s.report(
        tau_memory::plugin::NAME,
        json!({
            "kind": "saved",
            "calls": [
                { "tool": "memory_write", "details": {} },
                { "tool": "memory_write", "details": {} },
            ],
        }),
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
    fn every_demo_screen_has_one_name() {
        let mut names: Vec<&str> =
            SCREENS.iter().map(|(name, _)| *name).collect();
        let listed = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), listed, "a name opens one screen");
        for name in ["run", "memory", "constitution", "ledger", "pr-opened"] {
            assert!(names.contains(&name), "{name}");
        }
    }

    #[test]
    fn the_vcs_calls_draw_as_their_cards() {
        let mut run = retry_after();
        for (_, update) in script() {
            run.update(update);
        }
        use tau_vcs::ui::{
            change_diff::ChangeDiff,
            change_log::ChangeLog,
            change_status::ChangeStatus,
        };
        let details = |call: &str| {
            let card = run.tool(call).expect(call);
            card.data.result.as_ref().unwrap().details.clone().unwrap()
        };
        assert!(ChangeLog::parse(&details(LOG_CALL)).is_some());
        let show = ChangeDiff::parse(&details(SHOW_CALL)).expect("a show");
        assert_eq!(show.files.len(), 4);
        assert!(show.files.iter().any(|file| file.path == SHOW_FILE));
        assert_eq!(show.parents.len(), 1);
        let diff = ChangeDiff::parse(&details(DIFF_CALL)).expect("a diff");
        assert_eq!(diff.files[0].path, DIFF_FILE);
        assert_eq!(diff.files[0].kind, tau_vcs::ChangeKind::Added);
        assert_eq!(diff.files[0].added, 21);
        let status =
            ChangeStatus::parse(&details(STATUS_CALL)).expect("a status");
        assert_eq!(status.working_copy.info.change_id, LOG_PICKED);
        assert_eq!(status.files.len(), show.files.len());
        assert!(status.files.iter().all(|file| !file.hunks.is_empty()));
    }

    #[test]
    fn the_ls_call_draws_as_a_listing() {
        let mut run = retry_after();
        for (_, update) in script() {
            run.update(update);
        }
        let card = run.tool(LS_CALL).expect(LS_CALL);
        let details = card.data.result.as_ref().unwrap().details.clone();
        let listing =
            tau_tools::ui::listing::DirListing::parse(&details.unwrap())
                .expect("a listing");
        assert_eq!(listing.dirs().count(), 3);
        assert_eq!(listing.changed(), 1);
        assert!(matches!(card.state, ToolState::Done { .. }));
        assert_eq!(listing.summary(), "3 folders · 13 files · 121 KB");
    }

    #[test]
    fn the_fork_points_at_its_run() {
        let runs = history();
        let fork = runs.iter().find(|run| run.id == fork_id()).expect("fork");
        let crate::view::Origin::Fork { from, turn } = &fork.origin else {
            panic!("not a fork: {:?}", fork.origin);
        };
        assert_eq!(*from, run_id(), "a chat forks the main chat");
        assert_eq!(*turn, 2);
        assert!(fork.last_diff().is_some());
    }

    #[test]
    fn catalog_links_point_at_notes_of_their_repository() {
        let host = DemoHost::new(&catalog()).unwrap();
        let catalog = host.catalog(catalog());
        let notes = |repo: &Repo| -> tau_memory::ui::Notebook {
            serde_json::from_value(
                repo.plugins[tau_memory::plugin::NAME].json().clone(),
            )
            .unwrap()
        };
        for repo in &catalog.repos {
            let memory = &notes(repo);
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
        let tau = notes(catalog.repo("tau-agent").unwrap());
        assert!(tau.backlinks("n-0417").count() >= 2);
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
        assert!(
            matches!(view.items.last(), Some(Item::Anchor { plugin, .. }) if plugin == tau_memory::plugin::NAME)
        );
        // The ledger lists every call before the rewrite, which it
        // names, and marks the cards it pruned.
        let rewrite = view
            .items
            .iter()
            .position(|item| matches!(item, Item::Rewrite { .. }))
            .unwrap();
        let calls = view.items[..rewrite]
            .iter()
            .filter(|item| matches!(item, Item::Tool(_)))
            .count();
        let pruning: tau_fast_compaction::ui::State = serde_json::from_value(
            view.plugin_states[tau_fast_compaction::NAME].json().clone(),
        )
        .unwrap();
        assert_eq!(pruning.ledger.len(), calls);
        let Item::Rewrite { key: Some(key), .. } = &view.items[rewrite] else {
            panic!("the rewrite is named");
        };
        assert!(
            pruning.passes[key].detail.contains("$0.0011"),
            "the pass's cost"
        );
        assert_eq!(
            view.tool("c8").and_then(|card| card.dropped),
            Some(crate::view::Dropped::Result)
        );
        // The test run output pruning cut stays, with its terminal.
        assert_eq!(view.tool("c7").and_then(|card| card.dropped), None);
        assert!(view.tool("c7").is_some_and(|card| card.cut.is_some()));
        assert!(view.tool("c7").is_some_and(|card| {
            tau_tools::ui::term_card::TermCards::default()
                .output(
                    &(view.id.clone(), card.call_id.clone()),
                    &card.data,
                    true,
                )
                .is_some()
        }));
        // What `start` decided comes before the first turn: what memory
        // recalled, and tau-reasoning's choice, which each draws itself.
        let anchored: Vec<&str> = view.items[1..3]
            .iter()
            .filter_map(|item| match item {
                Item::Anchor { plugin, .. } => Some(plugin.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(anchored, [tau_reasoning::NAME, tau_memory::plugin::NAME]);
        let reasoning: tau_reasoning::ui::State = serde_json::from_value(
            view.plugin_states[tau_reasoning::NAME].json().clone(),
        )
        .unwrap();
        assert_eq!(reasoning.plan.as_deref(), Some("high"));
        let memory: tau_memory::ui::State = serde_json::from_value(
            view.plugin_states[tau_memory::plugin::NAME].json().clone(),
        )
        .unwrap();
        assert!(
            memory
                .marks
                .values()
                .any(|mark| *mark == tau_memory::ui::Mark::Saved(2))
        );
    }
}
