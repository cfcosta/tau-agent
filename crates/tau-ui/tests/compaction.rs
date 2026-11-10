//! Compaction in a host: tau-compaction is Luau now (ADR 0050), run by
//! tau-luau-plugins, so these tests install the app's plugins, the Luau
//! half among them. An idle chat is compacted in its own conversation,
//! from its connection's cache; a message stops it; and a long chat left
//! idle compacts by itself before its cache lapses; and a large output
//! is pruned into tau's archive for the repository.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{path::Path, sync::Once, time::Duration};

use tau_agent::{agent::Agent, event::RunEvent};
use tau_store::Store;
use tau_testing::{git::git, scripted::ScriptedModel};
use tau_ui::{
    accounts::Credentials,
    host::{Host, HostConfig},
};
use tau_ui_remote::{models::ModelChoice, view::Item};
use tau_vcs_host::{Identity, Project};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::common::LuauSettings as _;

/// The repository a host under test lists.
const REPO: &str = "repo";

/// How long a test waits for what a run does.
const WAIT: Duration = Duration::from_secs(60);

/// The app's plugins, the Luau ones among them: a process has one
/// registry.
fn install() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| tau_ui_remote::plugins::install(tau::plugins()));
}

fn host(llm: ScriptedModel) -> (Host, UnboundedReceiver<RunEvent>) {
    host_on(llm, &tempfile::tempdir().unwrap().keep())
}

/// A project imported from `checkout`, which is made a Git repository
/// with one commit first if it is not one.
fn project_of(checkout: &Path) -> Project {
    if !checkout.join(".git").exists() {
        git(checkout, &["init", "--quiet"]);
        git(
            checkout,
            &["commit", "--quiet", "--allow-empty", "-m", "first"],
        );
    }
    tau_vcs_host::ProjectRepo::import(
        checkout.to_str().unwrap(),
        tempfile::tempdir().unwrap().keep().join("p"),
        Identity::default(),
    )
    .map(Project::from)
    .unwrap()
}

fn host_on(
    llm: ScriptedModel,
    root: &Path,
) -> (Host, UnboundedReceiver<RunEvent>) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let store = runtime.block_on(tau_store_sqlite::memory()).unwrap();
    host_over(runtime, store, llm, root)
}

fn host_over(
    runtime: tokio::runtime::Runtime,
    store: Store,
    llm: ScriptedModel,
    root: &Path,
) -> (Host, UnboundedReceiver<RunEvent>) {
    host_of(runtime, store, Agent::new(llm).name("coder"), root)
}

/// A host whose runs start from `agent`.
fn host_of(
    runtime: tokio::runtime::Runtime,
    store: Store,
    agent: Agent,
    root: &Path,
) -> (Host, UnboundedReceiver<RunEvent>) {
    install();
    // Plugins keep their files beside the repositories, in the parent of
    // `repos`: a directory of the host's own, not the shared temporary one.
    let own = fresh_repo_list();
    let config = HostConfig {
        credentials: Credentials::new(
            fresh_repo_list().with_extension("config"),
        ),
        model: Some("gpt-6-luna".into()),
        // Unused but for its directory, where the person's images are
        // kept: the test's own.
        store: own.join("runs.db"),
        // Tau's directory for repositories, where memory lives too: the
        // test's own.
        repos: own.join("repos"),
        // The test's own: a test that saves settings leaves others be.
        settings: fresh_repo_list().with_extension("models"),
        repo_list: fresh_repo_list(),
        home: None,
        services: Default::default(),
    };
    let (host, events) = Host::with_agent(runtime, agent, store, config);
    // Memory's pass after each run asks the model once more: these
    // scripts answer only the requests they write.
    let host = host.with_luau_settings(
        tau_luau_plugins::names::MEMORY,
        serde_json::json!({ "after_each_run": false }),
    );
    (
        host.with_repository(
            REPO,
            std::sync::Arc::new(tau_vcs_host::JjRepository::new(
                project_of(root),
                Identity::default(),
            )),
        ),
        events,
    )
}

/// A repository list of the test's own, so tests do not share one.
fn fresh_repo_list() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNT: AtomicU32 = AtomicU32::new(0);
    // A directory of its own: a host keeps its plugins' files next to
    // its repositories, and tests must not share them.
    let dir = std::env::temp_dir().join(format!(
        "tau-ui-host-{}-{}",
        std::process::id(),
        COUNT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("repos.json")
}

/// Receives until `RunEnd`, with a timeout so a hang fails the test.
fn until_end(events: &mut UnboundedReceiver<RunEvent>) -> Vec<RunEvent> {
    let deadline = std::time::Instant::now() + WAIT;
    let mut seen = Vec::new();
    while std::time::Instant::now() < deadline {
        match events.try_recv() {
            Ok(event) => {
                let end = matches!(event, RunEvent::RunEnd { .. });
                seen.push(event);
                if end {
                    return seen;
                }
            }
            Err(_) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    panic!("no RunEnd within {WAIT:?}: {seen:?}");
}

/// Waits until `run` is no longer running.
fn wait_until_done(host: &Host, run: &tau_agent::tool::RunId) {
    let deadline = std::time::Instant::now() + WAIT;
    while host.is_running(run) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!host.is_running(run));
}

/// An idle chat is compacted in its own conversation: on the connection
/// that served it, with the instructions and tools of its last request,
/// so the summary reads the chat from that connection's prompt cache.
/// The rewrite says it was an idle one, and the chat goes on from the
/// summary and the turn it kept.
#[test]
fn an_idle_chat_is_compacted_from_its_cache() {
    use tau_ai::{client::OpenAi, ws::proto::pool::Limits};
    use tau_testing::fake_openai::{FakeOpenAi, LocalConnector, Reply};

    let fake = FakeOpenAi::new(vec![
        Reply::text("resp_1", "read the first"),
        Reply::text("resp_2", "read the second"),
        Reply::text("resp_3", "## Goal\nRead two long texts."),
        // tau-memory keeps what the summary drops.
        Reply::text("resp_4", "nothing to keep"),
        Reply::text("resp_5", "back"),
    ])
    .report_cache();
    let address = fake.listen();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let store = runtime.block_on(tau_store_sqlite::memory()).unwrap();
    let client = {
        let _guard = runtime.enter();
        OpenAi::with_connector(LocalConnector(address), Limits::default())
    };
    let agent = Agent::new(client.clone()).name("coder");
    let root = tempfile::tempdir().unwrap().keep();
    let (host, mut events) = host_of(runtime, store, agent, &root);

    // Two turns, each longer than what compaction keeps: the first is
    // summarized, the second kept.
    let long = |word: &str| format!("{word} ").repeat(20_000);
    let chat = host
        .block_on(host.start(&long("first"), &ModelChoice::default(), REPO))
        .unwrap()
        .id;
    until_end(&mut events);
    wait_until_done(&host, &chat);
    host.block_on(host.resume(&chat, &long("second"), &ModelChoice::default()))
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &chat);

    let compacted = host
        .block_on(host.compact_idle(&chat))
        .unwrap()
        .expect("compacted");
    assert_eq!(&*compacted.plugin, tau_luau_plugins::NAME);
    assert!(compacted.tokens_after < compacted.tokens_before / 2);
    let received = fake.received();
    let (last, summary) = (&received[1], &received[2]);
    assert_eq!(summary.connection, last.connection);
    assert_eq!(summary.body["prompt_cache_key"], chat.0.as_ref());
    for field in ["model", "instructions", "tools", "reasoning"] {
        assert_eq!(summary.body[field], last.body[field], "{field}");
    }
    // The chat up to the summarized turn comes from cache.
    assert!(
        summary.cached_tokens >= received[0].input_tokens,
        "{} cached of {}",
        summary.cached_tokens,
        received[0].input_tokens
    );
    let rewritten =
        std::iter::from_fn(|| events.try_recv().ok()).find_map(|event| {
            match event {
                RunEvent::ContextRewritten { run, trigger, .. }
                    if run == chat =>
                {
                    Some(trigger)
                }
                _ => None,
            }
        });
    assert_eq!(rewritten, Some(tau_agent::plugin::Trigger::Idle));

    host.block_on(host.resume(&chat, "back", &ModelChoice::default()))
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &chat);
    let next = fake.received().last().unwrap().body.to_string();
    assert!(next.contains("Read two long texts."), "{next}");
    assert!(next.contains("second second"));
    assert!(!next.contains("first first"));
    let history = host.block_on(host.history()).unwrap();
    let view = history.iter().find(|view| view.id == chat).unwrap();
    assert!(
        view.items
            .iter()
            .any(|item| matches!(item, Item::Rewrite { idle: true, .. })),
        "{:?}",
        view.items
    );
}

/// A message for a chat being compacted while idle stops the
/// compaction: the chat goes on from its transcript as it was, and
/// nothing of the summary is stored.
#[test]
fn a_message_stops_an_idle_compaction() {
    let llm = ScriptedModel::new()
        .turn(|t| t.text("read the first"))
        .turn(|t| t.text("read the second"))
        .turn(|t| t.text("## Goal").stall(Duration::from_secs(600)))
        .turn(|t| t.text("went on"));
    let (host, mut events) = host(llm.clone());
    let long = |word: &str| format!("{word} ").repeat(20_000);
    let chat = host
        .block_on(host.start(&long("first"), &ModelChoice::default(), REPO))
        .unwrap()
        .id;
    until_end(&mut events);
    wait_until_done(&host, &chat);
    host.block_on(host.resume(&chat, &long("second"), &ModelChoice::default()))
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &chat);

    let compacted = host.block_on(async {
        let compacting = host.compact_idle(&chat);
        let message = async {
            while llm.requests().len() < 3 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            host.resume(&chat, "go on", &ModelChoice::default())
                .await
                .unwrap();
        };
        tokio::join!(compacting, message).0
    });
    assert_eq!(compacted.unwrap(), None);
    until_end(&mut events);
    wait_until_done(&host, &chat);
    let requests = llm.requests();
    assert_eq!(requests.len(), 4);
    let went_on = format!("{:?}", requests[3].transcript);
    assert!(went_on.contains("first first"));
    assert!(!went_on.contains("<summary>"));
    let history = host.block_on(host.history()).unwrap();
    let view = history.iter().find(|view| view.id == chat).unwrap();
    assert!(
        !view
            .items
            .iter()
            .any(|item| matches!(item, Item::Rewrite { .. }))
    );
}

/// A long chat left idle is compacted by itself shortly before its
/// connection's prompt cache lapses, and says so.
#[test]
fn a_long_idle_chat_compacts_before_its_cache_lapses() {
    use tau_ai::{client::OpenAi, ws::proto::pool::Limits};
    use tau_testing::fake_openai::{FakeOpenAi, LocalConnector, Reply};

    let fake = FakeOpenAi::new(vec![
        Reply::text("resp_1", "read the first"),
        Reply::text("resp_2", "read the second"),
        Reply::text("resp_3", "## Goal\nRead two long texts."),
        Reply::text("resp_4", "nothing to keep"),
    ])
    .report_cache();
    let address = fake.listen();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let store = runtime.block_on(tau_store_sqlite::memory()).unwrap();
    let client = {
        let _guard = runtime.enter();
        OpenAi::with_connector(
            LocalConnector(address),
            Limits {
                cache_lifetime: Duration::from_secs(3),
                ..Limits::default()
            },
        )
    };
    let agent = Agent::new(client.clone()).name("coder");
    let root = tempfile::tempdir().unwrap().keep();
    let (host, mut events) = host_of(runtime, store, agent, &root);
    let host = std::sync::Arc::new(
        host.with_provider(std::sync::Arc::new(tau_chatgpt::Over::new(client))),
    );

    // More than half the window, and a last turn longer than what
    // compaction keeps.
    let chat = host
        .block_on(host.start(
            &"first ".repeat(100_000),
            &ModelChoice::default(),
            REPO,
        ))
        .unwrap()
        .id;
    until_end(&mut events);
    wait_until_done(&host, &chat);
    // The lapse counts from the connection's last use, in the turn below.
    // The test sees that turn end late under load, so it counts from
    // before the turn: a lower bound that holds however late it looks.
    let began = std::time::Instant::now();
    host.block_on(host.resume(
        &chat,
        &"second ".repeat(12_000),
        &ModelChoice::default(),
    ))
    .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &chat);
    host.watch_idle(&chat);

    let deadline = std::time::Instant::now() + WAIT;
    let trigger = loop {
        assert!(std::time::Instant::now() < deadline, "no compaction");
        match events.try_recv() {
            Ok(RunEvent::ContextRewritten { run, trigger, .. })
                if run == chat =>
            {
                break trigger;
            }
            Ok(_) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    };
    assert_eq!(trigger, tau_agent::plugin::Trigger::Idle);
    // Nine tenths of the way to the lapse, 2.7 s, in the pool's time,
    // which is as of its last tick, a second apart: well after the turn,
    // not at once, though up to a tick short of 2.7 s.
    assert!(
        began.elapsed() >= Duration::from_secs(1),
        "{:?}",
        began.elapsed()
    );
    let summary = &fake.received()[2];
    assert!(summary.cached_tokens > 0);
}

/// A large `exec_command` output reaches the model pruned, with the
/// whole of it archived in tau's directory for the repository, readable
/// only by the person; the call's card says what was kept, live and from
/// history.
#[test]
fn a_large_output_is_pruned_into_tau_s_archive() {
    use std::os::unix::fs::PermissionsExt as _;
    install();
    let dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    // Over output pruning's 10,000 estimated tokens.
    let command = "for i in $(seq 1 1200); do \
                   echo \"::::::::::::::::::::::::: line $i\"; done";
    let llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call("exec_command", serde_json::json!({ "cmd": command }))
        })
        .turn(|t| t.text("built"));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let store = runtime.block_on(tau_store_sqlite::memory()).unwrap();
    let config = HostConfig {
        credentials: Credentials::new(data.path().join("config")),
        model: Some("gpt-6-luna".into()),
        store: data.path().join("runs.db"),
        repos: data.path().join("repos"),
        settings: data.path().join("models.json"),
        repo_list: data.path().join("repos.json"),
        home: None,
        services: Default::default(),
    };
    let (host, mut events) =
        Host::with_agent(runtime, Agent::new(llm).name("coder"), store, config);
    // Memory's pass after each run asks the model once more: these
    // scripts answer only the requests they write.
    let host = host.with_luau_settings(
        tau_luau_plugins::names::MEMORY,
        serde_json::json!({ "after_each_run": false }),
    );
    let host = host.with_repository(
        REPO,
        std::sync::Arc::new(tau_vcs_host::JjRepository::new(
            project_of(dir.path()),
            Identity::default(),
        )),
    );
    // Jev finds every chunk it is asked about disposable: the first and
    // last stay anyway.
    let host = host.with_service(tau_jev::Upstream(std::sync::Arc::new(
        tau_jev::fake::FakeJev::nouls(|_| 0.0),
    )));
    let mut view = host
        .block_on(host.start("build it", &ModelChoice::default(), REPO))
        .unwrap();
    for event in until_end(&mut events) {
        view.apply(&event);
    }
    wait_until_done(&host, &view.id);
    let cut_of = |view: &tau_ui_remote::view::RunView| {
        view.items.iter().find_map(|item| match item {
            Item::Tool(card) if card.tool == "exec_command" => card.cut.clone(),
            _ => None,
        })
    };
    let cut = cut_of(&view).expect("the card says what was cut");
    assert_eq!(cut.lines, 1200);
    assert!(cut.kept < cut.lines / 10, "{cut:?}");
    let archive = Path::new(&cut.archive);
    assert!(
        archive.starts_with(data.path().join("repos")),
        "{} is not in tau's directory",
        archive.display()
    );
    assert!(archive.parent().unwrap().ends_with("archive"));
    let mode = |path: &Path| {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    };
    assert_eq!(mode(archive), 0o600);
    assert_eq!(mode(archive.parent().unwrap()), 0o700);
    let whole = std::fs::read_to_string(archive).unwrap();
    assert!(whole.contains(":: line 600\n"));
    // With a key, pruning runs, and its status says so.
    let luau: tau_luau_plugins::ui::State = serde_json::from_value(
        view.plugin_states[tau_luau_plugins::NAME].json().clone(),
    )
    .unwrap();
    let pruning = &luau.plugins[tau_luau_plugins::names::FAST_COMPACTION];
    assert_eq!(pruning.state["on"], true);
    assert_eq!(pruning.state["outputs"].as_array().unwrap().len(), 1);

    let history = host.block_on(host.history()).unwrap();
    assert_eq!(cut_of(&history[0]), Some(cut));
}
