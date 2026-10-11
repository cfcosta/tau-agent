//! Resource subscriptions (ADR 0037, step 4): against the in-process
//! server, on 2026-07-28 (a `subscriptions/listen` per resource) and on
//! 2025-11-25 (`resources/subscribe`). A watch hears each change, two
//! watches of one resource share a subscription and the last one to go
//! unsubscribes, a server that cannot subscribe says so, and a watch
//! ends with its connection, so watching again connects again.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

mod common;

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use common::State as Fixture;
use serde_json::Value;
use tau_mcp::config::{Origin, ServerConfig, Transport};
use tau_mcp_host::connection::{
    Connection,
    Environment,
    ResourceWatch,
    State,
    WatchError,
};
use tokio_util::sync::CancellationToken;

const STATUS: &str = "file:///status";

fn environment() -> Environment {
    Environment {
        env: Arc::new(|_| None),
        home: None,
        repo: None,
        auth: None,
        launcher: Default::default(),
    }
}

async fn connected(fixture: &Arc<Fixture>) -> Arc<Connection> {
    let config = ServerConfig::new("srv", Transport::Stream(fixture.dial()));
    let connection = Connection::new(config, Origin::User, environment());
    connection.connect();
    connection.settled(&CancellationToken::new()).await;
    assert_eq!(connection.status().state, State::Connected);
    connection
}

/// Waits up to 5 s for `check`.
async fn eventually(mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Whether `watch` hears a change within 5 s.
async fn hears(watch: &mut ResourceWatch) -> bool {
    tokio::time::timeout(Duration::from_secs(5), watch.changed())
        .await
        .expect("a change in time")
}

/// What `file:///status` reads now.
async fn status(connection: &Arc<Connection>) -> String {
    let read = connection
        .read_resource(STATUS, &CancellationToken::new())
        .await
        .unwrap();
    let text = read["contents"][0]["text"].as_str().unwrap();
    let value: Value = serde_json::from_str(text).unwrap();
    value["build"].as_str().unwrap().to_owned()
}

/// How many subscriptions to `file:///status` the server holds.
fn held(fixture: &Fixture) -> usize {
    if fixture.legacy {
        fixture.subscriptions()
    } else {
        fixture.listening_to(STATUS)
    }
}

async fn a_watch_hears_each_change(legacy: bool) {
    let fixture = Fixture::subscribable(legacy);
    let connection = connected(&fixture).await;
    let mut watch = connection
        .watch_resource(STATUS, &CancellationToken::new())
        .await
        .unwrap();
    fixture.set_status("red").await;
    assert!(hears(&mut watch).await);
    assert_eq!(status(&connection).await, "red");
    fixture.set_status("green").await;
    assert!(hears(&mut watch).await);
    assert_eq!(status(&connection).await, "green");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_watch_hears_each_change_2026() {
    a_watch_hears_each_change(false).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_watch_hears_each_change_2025() {
    a_watch_hears_each_change(true).await;
}

async fn watches_share_a_subscription(legacy: bool) {
    let fixture = Fixture::subscribable(legacy);
    let connection = connected(&fixture).await;
    let cancel = CancellationToken::new();
    let first = connection.watch_resource(STATUS, &cancel).await.unwrap();
    let mut second = connection.watch_resource(STATUS, &cancel).await.unwrap();
    assert_eq!(connection.subscriptions(), 1);
    eventually(|| held(&fixture) == 1).await;
    // One goes: the other still hears.
    drop(first);
    assert_eq!(connection.subscriptions(), 1);
    fixture.set_status("red").await;
    assert!(hears(&mut second).await);
    // The last one goes: the server is told.
    drop(second);
    assert_eq!(connection.subscriptions(), 0);
    eventually(|| held(&fixture) == 0).await;
    if legacy {
        assert_eq!(*fixture.unsubscribed.lock().unwrap(), [STATUS]);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn watches_share_a_subscription_2026() {
    watches_share_a_subscription(false).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn watches_share_a_subscription_2025() {
    watches_share_a_subscription(true).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_cannot_subscribe_says_so() {
    let cancel = CancellationToken::new();
    // Resources, but no word of their changes.
    let connection = connected(&Fixture::with_features(false)).await;
    let refused = connection.watch_resource(STATUS, &cancel).await;
    assert!(
        matches!(&refused, Err(WatchError::Unsupported(why)) if why.contains("poll")),
        "{refused:?}"
    );
    // No resources at all.
    let connection = connected(&Fixture::new(false)).await;
    let refused = connection.watch_resource(STATUS, &cancel).await;
    assert!(
        matches!(refused, Err(WatchError::Unsupported(_))),
        "{refused:?}"
    );
}

async fn a_watch_ends_with_its_connection(legacy: bool) {
    let fixture = Fixture::subscribable(legacy);
    let connection = connected(&fixture).await;
    let cancel = CancellationToken::new();
    let mut watch = connection.watch_resource(STATUS, &cancel).await.unwrap();
    // A 2025-11-25 server is dialled twice a connect: once to find out.
    let dialled = fixture.dials();
    fixture.crash();
    // It ends, rather than wait for changes that cannot come.
    assert!(!hears(&mut watch).await);
    assert_eq!(connection.subscriptions(), 0);
    // Watching again connects again, and hears.
    let mut watch = connection.watch_resource(STATUS, &cancel).await.unwrap();
    assert!(fixture.dials() > dialled);
    fixture.set_status("red").await;
    assert!(hears(&mut watch).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_watch_ends_with_its_connection_2026() {
    a_watch_ends_with_its_connection(false).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_watch_ends_with_its_connection_2025() {
    a_watch_ends_with_its_connection(true).await;
}
