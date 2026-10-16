//! Connections kept across changes (`docs/reference/mcp.md`, "On the
//! host"): as a property, the diff keeps exactly the unchanged enabled
//! servers, connects the added and changed ones and lets go of the
//! removed, changed and disabled ones, and the pool keeps the very
//! connections it says it keeps. Against in-process servers: a changed
//! entry reconnects only its server, and two repositories share one
//! connection to a user server.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

mod common;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use common::State as Fixture;
use hegel::{TestCase, generators as gs};
use serde_json::json;
use tau_mcp::config::{Origin, ServerConfig, Settings, StdioConfig, Transport};
use tau_mcp_host::{
    Host,
    connection::{Connection, Environment, State},
    pool::{Pool, diff},
};
use tokio_util::sync::CancellationToken;

const NAMES: [&str; 4] = ["a", "b", "c", "d"];
const ORIGINS: [Origin; 3] = [Origin::User, Origin::Settings, Origin::Repo];

/// An entry from few enough choices that old and new often agree:
/// where it comes from, which of two commands, on or off.
#[hegel::composite]
fn entry(tc: &TestCase) -> (usize, usize, bool) {
    (
        tc.draw(gs::integers().max_value(ORIGINS.len() - 1)),
        tc.draw(gs::integers().max_value(1_usize)),
        tc.draw(gs::booleans()),
    )
}

#[hegel::composite]
fn servers(tc: &TestCase) -> BTreeMap<&'static str, (usize, usize, bool)> {
    tc.draw(gs::btree_maps(gs::sampled_from(NAMES.to_vec()), entry()))
}

fn config(
    servers: &BTreeMap<&'static str, (usize, usize, bool)>,
) -> Vec<(Origin, ServerConfig)> {
    servers
        .iter()
        .map(|(name, (origin, command, enabled))| {
            let mut server = ServerConfig::new(
                *name,
                Transport::Stdio(StdioConfig {
                    command: format!("cmd{command}"),
                    ..StdioConfig::default()
                }),
            );
            server.enabled = *enabled;
            (ORIGINS[*origin], server)
        })
        .collect()
}

fn environment() -> Environment {
    Environment {
        env: Arc::new(|_| None),
        home: None,
        repo: None,
        auth: None,
        launcher: Default::default(),
        refused: Default::default(),
    }
}

/// For any old and new servers: kept is the unchanged servers enabled in
/// both; connected is the added and changed ones enabled now; closed is
/// the removed, changed and disabled ones that were enabled. The pool
/// keeps the connection of every kept server and makes a new one for
/// every connected server.
#[hegel::test(test_cases = 300)]
fn the_diff_keeps_only_what_did_not_change(tc: TestCase) {
    let old = tc.draw(servers());
    let new = tc.draw(servers());
    let on = |servers: &BTreeMap<&str, (usize, usize, bool)>| {
        servers
            .iter()
            .filter(|(_, entry)| entry.2)
            .map(|(name, _)| name.to_string())
            .collect::<BTreeSet<String>>()
    };
    let (old_on, new_on) = (on(&old), on(&new));
    let names = |filter: &dyn Fn(&str) -> bool| {
        NAMES
            .iter()
            .filter(|name| filter(name))
            .map(|name| name.to_string())
            .collect::<BTreeSet<String>>()
    };
    let unchanged = names(&|n| old.contains_key(n) && old.get(n) == new.get(n));
    let changed = names(&|n| {
        old.contains_key(n) && new.contains_key(n) && old[n] != new[n]
    });
    let added = names(&|n| !old.contains_key(n) && new.contains_key(n));
    let removed = names(&|n| old.contains_key(n) && !new.contains_key(n));
    let disabled = names(&|n| {
        old.get(n).is_some_and(|e| e.2) && new.get(n).is_some_and(|e| !e.2)
    });

    let result = diff(&config(&old), &config(&new));
    let kept: BTreeSet<String> =
        unchanged.intersection(&new_on).cloned().collect();
    let connected: BTreeSet<String> = added
        .union(&changed)
        .filter(|n| new_on.contains(*n))
        .cloned()
        .collect();
    let closed: BTreeSet<String> = removed
        .union(&changed)
        .chain(&disabled)
        .filter(|n| old_on.contains(*n))
        .cloned()
        .collect();
    assert_eq!(result.kept, kept);
    assert_eq!(result.connected, connected);
    assert_eq!(result.closed, closed);

    let pool = Pool::new(environment());
    pool.update(config(&old));
    let before: BTreeMap<String, Arc<Connection>> = pool
        .connections()
        .into_iter()
        .map(|c| (c.name().to_owned(), c))
        .collect();
    assert_eq!(pool.update(config(&new)), result);
    let after = pool.connections();
    assert_eq!(after.len(), new.len());
    for connection in after {
        let name = connection.name();
        let same = before
            .get(name)
            .is_some_and(|b| Arc::ptr_eq(b, &connection));
        if result.kept.contains(name) {
            assert!(same, "{name} was kept but connected again");
        }
        if result.connected.contains(name) {
            assert!(!same, "{name} changed but kept its connection");
        }
    }
}

/// Waits up to 5 s for `check`.
async fn eventually(mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn stream(name: &str, fixture: &Arc<Fixture>) -> ServerConfig {
    ServerConfig::new(name, Transport::Stream(fixture.dial()))
}

async fn settle(connections: &[Arc<Connection>]) {
    for connection in connections {
        connection.settled(&CancellationToken::new()).await;
        assert_eq!(connection.status().state, State::Connected);
    }
}

/// Changing one server's entry dials that server again, and only it; a
/// run that holds the old connection keeps it.
#[tokio::test(flavor = "multi_thread")]
async fn a_changed_entry_reconnects_only_its_server() {
    let (a, b) = (Fixture::new(false), Fixture::new(false));
    let (a_config, b_config) = (stream("a", &a), stream("b", &b));
    let pool = Pool::new(environment());
    pool.update(vec![
        (Origin::Settings, a_config.clone()),
        (Origin::Settings, b_config.clone()),
    ]);
    let first = pool.connections();
    first.iter().for_each(Connection::start);
    settle(&first).await;

    let mut changed = b_config.clone();
    changed.description = Some("Now described.".into());
    pool.update(vec![
        (Origin::Settings, a_config),
        (Origin::Settings, changed),
    ]);
    let second = pool.connections();
    second.iter().for_each(Connection::start);
    settle(&second).await;
    // Which connections the pool keeps is the diff property's; here the
    // servers themselves show it: `a` was dialed once, `b` twice.
    assert_eq!((a.dials(), b.dials()), (1, 2));
    // The old connection is still the run's that holds it.
    assert_eq!(first[1].status().state, State::Connected);
    pool.shutdown().await;
    first[1].shutdown().await;
}

/// The host runs a user server once for every repository: two
/// repositories' plugins, and the user's alone, hold the same
/// connection, which dialed once, and the page shows it connected and
/// shared.
#[test]
fn repositories_share_a_user_server() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (one, two) =
        (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let fixture = Fixture::new(false);
    let host = Host::new(runtime.handle().clone(), None);
    host.set_servers(vec![stream("srv", &fixture)]);
    let settings = Settings::default();
    let first = host.plugin(Some(one.path()), &settings);
    let second = host.plugin(Some(two.path()), &settings);
    let user = host.plugin(None, &settings);
    assert!(Arc::ptr_eq(
        &first.connections()[0],
        &second.connections()[0]
    ));
    assert!(Arc::ptr_eq(&first.connections()[0], &user.connections()[0]));
    runtime.block_on(settle(first.connections()));
    runtime.block_on(eventually(|| second.tools().len() == 5));
    assert_eq!(fixture.dials(), 1);
    let shown = host.servers(Some(two.path()), &settings);
    assert!(shown.servers[0].shared);
    assert_eq!(shown.servers[0].state.as_deref(), Some("connected"));

    runtime.block_on(async move {
        drop(host);
        drop((first, second, user));
    });
}

/// A repository whose approved file names a user server's name gets its
/// own server there; the other repositories keep sharing the user's.
#[test]
fn a_repository_server_wins_over_a_shared_one() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let dirs = [(); 3].map(|()| tempfile::tempdir().unwrap());
    let [user, one, two] = &dirs;
    std::fs::write(
        user.path().join("mcp.json"),
        json!({ "mcpServers": { "srv": { "command": "/nonexistent/user" } } })
            .to_string(),
    )
    .unwrap();
    write_repo_file(
        two.path(),
        &json!({ "mcpServers": { "srv": { "command": "/nonexistent/repo" } } }),
    );
    let host = Host::new(runtime.handle().clone(), Some(user.path().into()));
    let pending = host.servers(Some(two.path()), &Settings::default()).pending;
    let settings = Settings {
        approved: BTreeSet::from([pending[0].hash.clone()]),
        ..Settings::default()
    };
    let first = host.plugin(Some(one.path()), &settings);
    let own = host.plugin(Some(two.path()), &settings);
    assert_eq!(first.connections()[0].origin(), Origin::User);
    assert_eq!(own.connections()[0].origin(), Origin::Repo);
    assert!(!Arc::ptr_eq(&own.connections()[0], &first.connections()[0]));
    assert!(host.servers(Some(one.path()), &settings).servers[0].shared);
    assert!(!host.servers(Some(two.path()), &settings).servers[0].shared);
    runtime.block_on(async move {
        drop(host);
        drop((first, own));
    });
}

fn write_repo_file(repo: &Path, file: &serde_json::Value) {
    std::fs::create_dir_all(repo.join(".tau")).unwrap();
    std::fs::write(repo.join(".tau/mcp.json"), file.to_string()).unwrap();
}

/// What an agent keeps of its repository at `repo`, for the tests: its
/// sandbox, or why it has none.
struct AgentRepo {
    repo: std::path::PathBuf,
    sandbox: Result<tau_ui_plugin::RepoLauncher, String>,
}

impl tau_ui_plugin::UnattendedSource for AgentRepo {
    fn unattended(
        &self,
        checkout: &Path,
    ) -> Option<tau_ui_plugin::UnattendedRepo> {
        (checkout == self.repo).then(|| tau_ui_plugin::UnattendedRepo {
            sandbox: self.sandbox.clone(),
        })
    }
}

/// A launcher that says each time it was asked, and starts nothing
/// that runs.
struct Asked(Arc<std::sync::atomic::AtomicUsize>);

#[async_trait::async_trait]
impl tau_agent::launch::Launcher for Asked {
    async fn launch(&self, _dir: &Path) -> tau_agent::launch::Launch {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        tau_agent::launch::Launch {
            prefix: vec!["/nonexistent/sandbox".into()],
            env: Vec::new(),
        }
    }
}

/// An agent's repository (ADR 0037): its servers connect without the
/// person's approval, and a stdio one starts only in its sandbox. With
/// none, it does not start, and says why.
#[tokio::test(flavor = "multi_thread")]
async fn an_agent_s_servers_need_no_approval_and_start_only_in_its_sandbox() {
    let (agent, person) =
        (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let file =
        json!({ "mcpServers": { "own": { "command": "/nonexistent/own" } } });
    write_repo_file(agent.path(), &file);
    write_repo_file(person.path(), &file);
    let unattended = tau_ui_plugin::Unattended::default();
    unattended.offer(Arc::new(AgentRepo {
        repo: agent.path().to_owned(),
        sandbox: Err("bwrap is not installed".into()),
    }));
    let host = Host::new(tokio::runtime::Handle::current(), None)
        .with_unattended(unattended);
    let settings = Settings::default();
    // The person's repository still asks; the agent's does not.
    assert_eq!(
        host.servers(Some(person.path()), &settings).pending.len(),
        1
    );
    assert!(
        host.servers(Some(agent.path()), &settings)
            .pending
            .is_empty()
    );
    // Building the plugin started its server: calling `connect` again
    // would start a second attempt whenever the first had already failed.
    let plugin = host.plugin(Some(agent.path()), &settings);
    let own = plugin.connections()[0].clone();
    assert_eq!(own.origin(), Origin::Repo);
    own.settled(&CancellationToken::new()).await;
    let status = own.status();
    assert_eq!(status.state, State::Failed);
    assert_eq!(status.error.as_deref(), Some("bwrap is not installed"));

    // With a sandbox, its stdio servers start through it, run or not.
    let sandboxed = tempfile::tempdir().unwrap();
    write_repo_file(sandboxed.path(), &file);
    let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let unattended = tau_ui_plugin::Unattended::default();
    unattended.offer(Arc::new(AgentRepo {
        repo: sandboxed.path().to_owned(),
        sandbox: Ok(tau_ui_plugin::RepoLauncher {
            launcher: Arc::new(Asked(asked.clone())),
            dir: sandboxed.path().to_owned(),
        }),
    }));
    let host = Host::new(tokio::runtime::Handle::current(), None)
        .with_unattended(unattended);
    let plugin = host.plugin(Some(sandboxed.path()), &settings);
    let own = plugin.connections()[0].clone();
    own.settled(&CancellationToken::new()).await;
    assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1);
    drop((host, plugin, own));
}
