//! What the host offers a plugin's host half outside any run (ADR
//! 0044): the person's model, whose use is booked to the plugin and
//! which only a plugin that declares it is given (ADR 0045), and
//! jobs the host keeps, which the catalog lists with their progress,
//! which read the repositories as they are, stop when cancelled, and
//! start again as the host does; and the end of first-run setup.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    path::{Path, PathBuf},
    sync::{Mutex, Once},
    time::{Duration, Instant},
};

use tau_agent::{agent::Agent, event::RunEvent};
use tau_ai::message::{Message, UserContent, UserMessage};
use tau_store::Store;
use tau_testing::{git::git, scripted::ScriptedModel};
use tau_ui::{
    accounts::Credentials,
    host::{Host, HostConfig},
};
use tau_ui_plugin::{
    AskSettings,
    HostCx,
    HostHalf,
    JobInfo,
    Manifest,
    PluginInfo,
    RunCtx,
    UiPlugin,
    points::{SetupClone, SetupRepo},
};
use tau_vcs_host::{Identity, Project};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_util::sync::CancellationToken;

const NAME: &str = "resumer";

/// What the resumer heard as each first-run setup ended.
static FINISHED: Mutex<Vec<Vec<SetupRepo>>> = Mutex::new(Vec::new());

/// A plugin that counts the repositories in a job, from a checkpoint it
/// keeps, started again as the host starts.
struct Resumer;

impl UiPlugin for Resumer {
    type State = ();
    type Data = ();
    type RepoData = ();
    type Settings = ();

    fn name(&self) -> &'static str {
        NAME
    }

    fn declaration(&self) -> tau_ui_plugin::Declaration {
        tau_ui_plugin::Declaration::new()
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
    }
}

struct ResumerHost;

impl HostHalf for ResumerHost {
    type Plugin = Resumer;
    type Host = ();

    async fn agent_plugins(
        &self,
        _: &(),
        _: &RunCtx,
        _: &(),
    ) -> anyhow::Result<Vec<Box<dyn tau_agent::plugin::Plugin>>> {
        Ok(Vec::new())
    }

    async fn catalog(&self, _: &(), _: &HostCx, _: &()) -> PluginInfo {
        PluginInfo::default()
    }

    async fn setup_finished(&self, _: &(), repos: &[SetupRepo], _: &HostCx) {
        FINISHED.lock().unwrap().push(repos.to_vec());
    }

    /// Says whether it is given the person's model, which it does not
    /// declare it uses.
    async fn act(
        &self,
        _: &(),
        _: serde_json::Value,
        cx: &HostCx,
    ) -> anyhow::Result<Option<serde_json::Value>> {
        Ok(Some(serde_json::json!(cx.model(NAME).is_some())))
    }

    /// Goes on counting when its checkpoint says it was: until it is
    /// cancelled, then it says so in the checkpoint.
    async fn resume(&self, _: &(), cx: &HostCx) {
        let checkpoint = cx.plugin_dir(NAME).join("checkpoint");
        let Ok(from) = tokio::fs::read_to_string(&checkpoint).await else {
            return;
        };
        let jobs = cx.jobs().expect("the host keeps jobs");
        jobs.start(NAME, "count", "Counting", move |job| async move {
            loop {
                let names: Vec<String> =
                    job.repos().into_iter().map(|repo| repo.name).collect();
                job.progress(0.5, format!("from {from}: {}", names.join(",")));
                tokio::select! {
                    () = job.cancelled() => break,
                    () = tokio::time::sleep(Duration::from_millis(20)) => {}
                }
            }
            tokio::fs::write(job.plugin_dir().join("checkpoint"), "stopped")
                .await?;
            Ok(())
        })
        .unwrap();
    }
}

/// The app's plugins and the one above: a process has one registry.
fn install() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        tau_ui_remote::plugins::install(
            tau_ui::hosted::halves(tau_plugins::plugins().with(Resumer))
                .host(ResumerHost),
        )
    });
}

/// A directory of the test's own, where the host keeps plugins' files.
fn own_dir() -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNT: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "tau-ui-services-{}-{}",
        std::process::id(),
        COUNT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// A host on `runtime` whose plugins keep their files under `dir`, on
/// `agent`.
fn host_in(
    dir: &Path,
    runtime: tokio::runtime::Runtime,
    store: Store,
    agent: Agent,
) -> (Host, UnboundedReceiver<RunEvent>) {
    install();
    let config = HostConfig {
        credentials: Credentials::new(dir.join("config")),
        model: Some("gpt-6-luna".into()),
        store: dir.join("runs.db"),
        repos: dir.join("repos"),
        settings: dir.join("models.json"),
        repo_list: dir.join("repos.json"),
        home: None,
        services: Default::default(),
    };
    Host::with_agent(runtime, agent, store, config)
}

/// A project imported from a new repository with one commit.
fn project() -> Project {
    let checkout = tempfile::tempdir().unwrap().keep();
    git(&checkout, &["init", "--quiet"]);
    git(
        &checkout,
        &["commit", "--quiet", "--allow-empty", "-m", "first"],
    );
    tau_vcs_host::ProjectRepo::import(
        checkout.to_str().unwrap(),
        tempfile::tempdir().unwrap().keep().join("p"),
        Identity::default(),
    )
    .map(Project::from)
    .unwrap()
}

/// The catalog's jobs once `done` holds of them, failing after a while.
fn jobs_until(host: &Host, done: impl Fn(&[JobInfo]) -> bool) -> Vec<JobInfo> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let jobs = host.block_on(host.catalog()).jobs;
        if done(&jobs) {
            return jobs;
        }
        assert!(Instant::now() < deadline, "the jobs stayed {jobs:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A job a plugin's checkpoint says was cut off starts again with the
/// host, and the catalog lists it with its progress; it reads a
/// repository listed after it started; cancelled, it stops, writing its
/// checkpoint, and leaves the catalog.
#[test]
fn a_job_resumes_reports_reads_live_repositories_and_stops() {
    let dir = own_dir();
    let plugin_dir = dir.join("plugins").join(NAME);
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(plugin_dir.join("checkpoint"), "3").unwrap();
    let runtime = runtime();
    let store = runtime.block_on(tau_store_sqlite::memory()).unwrap();
    let (host, _events) = host_in(
        &dir,
        runtime,
        store,
        Agent::new(ScriptedModel::new()).name("coder"),
    );

    let jobs = jobs_until(&host, |jobs| {
        jobs.iter().any(|job| job.line.starts_with("from 3"))
    });
    assert_eq!(
        jobs,
        [JobInfo {
            plugin: NAME.into(),
            name: "count".into(),
            label: "Counting".into(),
            fraction: 0.5,
            line: "from 3: ".into(),
            page: None,
        }]
    );

    let host = host.with_repository(
        "later",
        std::sync::Arc::new(tau_vcs_host::JjRepository::new(
            project(),
            Identity::default(),
        )),
    );
    jobs_until(&host, |jobs| {
        jobs.iter().any(|job| job.line == "from 3: later")
    });

    let cx = host.host_cx();
    assert!(cx.jobs().unwrap().cancel(NAME, "count"));
    jobs_until(&host, <[JobInfo]>::is_empty);
    assert_eq!(
        std::fs::read_to_string(plugin_dir.join("checkpoint")).unwrap(),
        "stopped"
    );
    assert!(!cx.jobs().unwrap().cancel(NAME, "count"), "none is going");
}

/// A host half asks the person's model outside any run: the request
/// reaches the plan's client, and what it used is booked to the plugin,
/// in no run. Signed out, there is no model to ask.
#[test]
fn a_host_half_asks_the_model_and_the_plugin_pays() {
    use tau_ai::{client::OpenAi, ws::proto::pool::Limits};
    use tau_testing::fake_openai::{FakeOpenAi, LocalConnector, Reply};

    let fake = FakeOpenAi::new(vec![Reply::text("resp_1", "noted")]);
    let address = fake.listen();
    let runtime = runtime();
    let store = runtime.block_on(tau_store_sqlite::memory()).unwrap();
    let client = {
        let _guard = runtime.enter();
        OpenAi::with_connector(LocalConnector(address), Limits::default())
    };
    let agent = Agent::new(client.clone()).name("coder");
    let (host, _events) = host_in(&own_dir(), runtime, store.clone(), agent);
    let host =
        host.with_provider(std::sync::Arc::new(tau_chatgpt::Over::new(client)));

    let model = host.host_cx().model(NAME).expect("signed in");
    let input = [Message::User(UserMessage {
        content: UserContent::Text("What did they learn?".into()),
        timestamp: 0,
    })];
    let answer = host
        .block_on(model.ask(
            &AskSettings::new("gpt-6-luna", None).instructions("Take notes."),
            &input,
            &[],
            &CancellationToken::new(),
        ))
        .unwrap();
    assert_eq!(answer.text(), "noted");
    let received = fake.received();
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].body["instructions"], "Take notes.");

    let booked = host
        .block_on(store.query(
            "SELECT plugin, input_tokens, output_tokens FROM plugin_usage",
            10,
        ))
        .unwrap();
    assert_eq!(booked.rows, [[NAME, "1", "1"]]);
    let spend = host.block_on(store.plugin_spend("1970")).unwrap();
    assert!(spend.iter().any(|(plugin, _)| plugin == NAME), "{spend:?}");
    let runs = host.block_on(store.recent_runs(10)).unwrap();
    assert!(runs.is_empty(), "no run: {runs:?}");

    // The resumer does not declare the model: the host has it, and does
    // not give it.
    let given = host
        .block_on(host.plugin_act(NAME, serde_json::Value::Null))
        .unwrap();
    assert_eq!(given, Some(serde_json::json!(false)));
}

/// What fills `tau.title` writes a run's title with the person's model,
/// holding the reply to its schema and cleaning it up, and books the
/// request to tau-titles.
#[test]
fn titles_are_written_with_the_persons_model_and_booked_to_tau_titles() {
    use tau_ai::{client::OpenAi, ws::proto::pool::Limits};
    use tau_testing::fake_openai::{FakeOpenAi, LocalConnector, Reply};

    let fake = FakeOpenAi::new(vec![Reply::text(
        "resp_1",
        r#"{"title":"Fix the retry loop."}"#,
    )]);
    let address = fake.listen();
    let runtime = runtime();
    let store = runtime.block_on(tau_store_sqlite::memory()).unwrap();
    let client = {
        let _guard = runtime.enter();
        OpenAi::with_connector(LocalConnector(address), Limits::default())
    };
    let agent = Agent::new(client.clone()).name("coder");
    let (host, _events) = host_in(&own_dir(), runtime, store.clone(), agent);
    let host =
        host.with_provider(std::sync::Arc::new(tau_chatgpt::Over::new(client)));

    let titler = host
        .slots()
        .lock()
        .unwrap()
        .get(&tau_ui_plugin::title::TITLE)
        .expect("tau-titles fills tau.title");
    let title = host
        .block_on(titler.write("the retry loop gives up too soon", "gpt-6-sol"))
        .unwrap();
    assert_eq!(title.as_deref(), Some("Fix the retry loop"));
    let received = fake.received();
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].body["text"]["format"]["name"], "run_title");

    let booked = host
        .block_on(store.query("SELECT plugin FROM plugin_usage", 10))
        .unwrap();
    assert_eq!(booked.rows, [[tau_titles::NAME]]);
}

/// The host's store is opened by what fills `tau.store`, tau-sqlite, at
/// the path the host asks, before any plugin's host state is made.
#[test]
fn the_store_is_opened_by_what_fills_its_slot() {
    install();
    let dir = own_dir();
    let path = dir.join("runs.db");
    let runtime = runtime();
    let ui = tau_ui_plugin::UiState::default();
    assert_eq!(
        tau_ui::hosted::early_fills(&ui)
            .filler(tau_ui_plugin::store::STORE.name()),
        Some(tau_sqlite::NAME.to_owned())
    );
    let store = runtime
        .block_on(tau_ui::hosted::open_store(&path, &ui))
        .unwrap();
    assert!(path.exists(), "the store is kept where the host asked");
    assert!(runtime.block_on(store.recent_runs(1)).unwrap().is_empty());
}

/// `tau.compaction` is the chain's unless the person picks the summary
/// alone: fast pruning, the tree and the summary rewrite, in that turn,
/// or only the summary.
#[test]
fn the_chain_compacts_unless_the_summary_is_picked() {
    use tau_ui_plugin::compaction::COMPACTION;
    let runtime = runtime();
    let store = runtime.block_on(tau_store_sqlite::memory()).unwrap();
    let llm = ScriptedModel::new();
    let (host, _events) = host_in(&own_dir(), runtime, store, Agent::new(llm));
    let rewriters = || {
        host.slots()
            .lock()
            .unwrap()
            .get(&COMPACTION)
            .expect("something fills tau.compaction")
            .rewriters()
    };
    assert_eq!(rewriters(), tau_compaction_chain::CHAIN);
    host.slots()
        .lock()
        .unwrap()
        .pick(COMPACTION.name(), Some(tau_compaction::NAME));
    assert_eq!(rewriters(), [tau_compaction::NAME]);
}

/// The end of first-run setup reaches every plugin's host half, with the
/// repositories the person picked.
#[test]
fn a_host_half_hears_setup_end_with_the_picked_repositories() {
    let runtime = runtime();
    let store = runtime.block_on(tau_store_sqlite::memory()).unwrap();
    let (host, _events) = host_in(
        &own_dir(),
        runtime,
        store,
        Agent::new(ScriptedModel::new()).name("coder"),
    );
    let picked = vec![SetupRepo {
        name: "octocat/hello".into(),
        id: None,
        remote: Some("https://github.com/octocat/hello.git".into()),
        folder: None,
        clone: SetupClone::Cloning(0.5),
    }];
    host.block_on(host.setup_finished(&picked));
    assert_eq!(*FINISHED.lock().unwrap(), [picked]);
}
