//! Connections kept across changes (`docs/reference/mcp.md`, "On the
//! host"): as a property, the diff keeps exactly the unchanged enabled
//! servers, connects the added and changed ones and lets go of the
//! removed, changed and disabled ones, and the pool keeps the very
//! connections it says it keeps. Against in-process servers: a changed
//! entry reconnects only its server, and two repositories share one
//! connection to a user server.

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
use tau_mcp::{
    config::{Origin, ServerConfig, Settings, StdioConfig, Transport},
    connection::{Connection, Environment, State},
    pool::{Pool, diff},
    ui::Host,
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

/// Changing one server's entry connects that one again, and only it; a
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
    let changes = pool.update(vec![
        (Origin::Settings, a_config),
        (Origin::Settings, changed),
    ]);
    assert_eq!(changes.kept, BTreeSet::from(["a".to_owned()]));
    assert_eq!(changes.connected, BTreeSet::from(["b".to_owned()]));
    assert_eq!(changes.closed, BTreeSet::from(["b".to_owned()]));
    let second = pool.connections();
    second.iter().for_each(Connection::start);
    settle(&second).await;
    assert!(Arc::ptr_eq(&first[0], &second[0]));
    assert!(!Arc::ptr_eq(&first[1], &second[1]));
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
