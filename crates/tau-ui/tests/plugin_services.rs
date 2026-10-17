//! What the host offers a plugin's host half outside any run (ADR
//! 0044): the person's model, whose use is booked to the plugin.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    path::{Path, PathBuf},
    sync::Once,
};

use tau_agent::{agent::Agent, event::RunEvent};
use tau_ai::message::{Message, UserContent, UserMessage};
use tau_store::Store;
use tau_ui::{
    accounts::Credentials,
    host::{Host, HostConfig},
};
use tau_ui_plugin::{
    AskSettings,
    HostCx,
    HostHalf,
    Manifest,
    PluginInfo,
    RunCtx,
    UiPlugin,
};
use tau_vcs_host::Identity;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_util::sync::CancellationToken;

const NAME: &str = "resumer";

/// A plugin with a host half and nothing else.
struct Resumer;

impl UiPlugin for Resumer {
    type State = ();
    type Data = ();
    type RepoData = ();
    type Settings = ();
    type Ui = ();

    fn name(&self) -> &'static str {
        NAME
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
}

/// The app's plugins and the one above: a process has one registry.
fn install() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        tau_ui_remote::plugins::install(
            tau_ui::hosted::halves(
                tau_ui_remote::plugins::plugins().with(Resumer),
            )
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
        account: tau_ai::chatgpt::AccountId::parse("test-account").unwrap(),
        credentials: Credentials::new(dir.join("config")),
        model: Some("gpt-6-luna".into()),
        store: dir.join("runs.db"),
        repos: dir.join("repos"),
        settings: dir.join("models.json"),
        repo_list: dir.join("repos.json"),
        skills: std::env::temp_dir().join("tau-test-skills-none"),
        identity: Identity::default(),
    };
    Host::with_agent(runtime, agent, store, config)
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
    let host = host.with_client(client);

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

    host.set_account(None).unwrap();
    assert!(host.host_cx().model(NAME).is_none());
}
