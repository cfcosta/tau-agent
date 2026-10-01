//! Connections against an in-process rmcp server over a duplex stream
//! (`docs/reference/mcp.md`, "Tests"): listing, calling, structured
//! content, `isError`, progress, cancel, `list_changed`, a server that
//! drops and reconnects, a call that is not retried, and HTTP connect
//! retries. Both protocols: 2026-07-28 (list changes through
//! `subscriptions/listen`) and 2025-11-25 (plain notifications).

mod common;

use std::{
    future::Future,
    sync::{
        Arc,
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use common::State as Fixture;
use serde_json::{Value, json};
use tau_mcp::{
    config::{HttpConfig, Origin, ServerConfig, Transport},
    connection::{
        CallFailure,
        Connection,
        Environment,
        HTTP_RETRY_DELAYS,
        Progress,
        State,
    },
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_util::sync::CancellationToken;

fn environment() -> Environment {
    Environment {
        env: Arc::new(|_| None),
        home: None,
        repo: None,
        auth: None,
    }
}

fn connect(fixture: &Arc<Fixture>, timeout: f64) -> Arc<Connection> {
    let mut config =
        ServerConfig::new("test-server", Transport::Stream(fixture.dial()));
    config.timeout = timeout;
    let connection = Connection::new(config, Origin::User, environment());
    connection.connect();
    connection
}

async fn settle(connection: &Connection) {
    connection.settled(&CancellationToken::new()).await;
}

/// Waits up to 5 s for `check`.
async fn eventually(mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn no_progress() -> impl Fn(Progress) + Send + Sync {
    |_| {}
}

async fn call(
    connection: &Arc<Connection>,
    tool: &str,
    args: Value,
) -> Result<Value, CallFailure> {
    connection
        .call(tool, args, &no_progress(), &CancellationToken::new())
        .await
}

/// Runs `test` against a modern and a legacy server.
async fn both<F, Fut>(test: F)
where
    F: Fn(Arc<Fixture>) -> Fut,
    Fut: Future<Output = ()>,
{
    test(Fixture::new(false)).await;
    test(Fixture::new(true)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn lists_every_page_and_calls_a_tool() {
    both(|fixture| async move {
        let connection = connect(&fixture, 60.0);
        settle(&connection).await;
        assert_eq!(
            connection.status().state,
            State::Connected,
            "{:?}",
            connection.status()
        );
        let names: Vec<String> =
            connection.tools().into_iter().map(|t| t.name).collect();
        assert_eq!(names, ["echo", "fail", "slow", "hang", "crash"]);
        let echo = &connection.tools()[0];
        assert_eq!(echo.annotations.read_only, Some(true));
        assert!(echo.output_schema.is_some());
        assert_eq!(
            connection.instructions().as_deref(),
            Some("Use echo to echo.\nMore details.")
        );

        let result = call(&connection, "echo", json!({"text": "hi"}))
            .await
            .unwrap();
        assert_eq!(result["content"], json!([{"type": "text", "text": "hi"}]));
        assert_eq!(result["structuredContent"], json!({"echo": "hi"}));

        let result = call(&connection, "fail", json!({})).await.unwrap();
        assert_eq!(result["isError"], json!(true));
        connection.shutdown().await;
        assert_eq!(connection.status().state, State::Closed);
    })
    .await;
}

/// Progress reaches the caller and starts the timeout again; without
/// progress the call times out.
#[tokio::test(flavor = "multi_thread")]
async fn progress_restarts_the_timeout() {
    both(|fixture| async move {
        let connection = connect(&fixture, 0.3);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = {
            let seen = seen.clone();
            move |progress: Progress| seen.lock().unwrap().push(progress)
        };
        let result = connection
            .call(
                "slow",
                json!({"steps": 6, "ms": 100}),
                &record,
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(result["content"][0]["text"], json!("done"));
        // Every step reached the caller in order; the last may race the
        // result, and tau ignores updates after a call ends anyway.
        let seen = seen.lock().unwrap().clone();
        assert!(seen.len() >= 5, "{seen:?}");
        for (index, progress) in seen.iter().enumerate() {
            assert_eq!(progress.progress, index as f64 + 1.0);
            assert_eq!(progress.total, Some(6.0));
            assert_eq!(progress.message, Some(format!("step {}", index + 1)));
        }

        let error = call(&connection, "hang", json!({})).await.unwrap_err();
        assert!(matches!(error, CallFailure::TimedOut { .. }), "{error}");
        connection.shutdown().await;
    })
    .await;
}

/// The run's token cancels the request, and the server hears of it.
#[tokio::test(flavor = "multi_thread")]
async fn cancel_tells_the_server() {
    both(|fixture| async move {
        let connection = connect(&fixture, 60.0);
        let cancel = CancellationToken::new();
        let canceller = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            canceller.cancel();
        });
        let error = connection
            .call("hang", json!({}), &no_progress(), &cancel)
            .await
            .unwrap_err();
        assert!(matches!(error, CallFailure::Cancelled { .. }), "{error}");
        eventually(|| fixture.cancelled.lock().unwrap().len() == 1).await;
        // The connection is still good.
        call(&connection, "echo", json!({"text": "after"}))
            .await
            .unwrap();
        connection.shutdown().await;
    })
    .await;
}

/// `list_changed` lists the tools again; a withdrawn tool fails.
#[tokio::test(flavor = "multi_thread")]
async fn list_changed_lists_the_tools_again() {
    both(|fixture| async move {
        let connection = connect(&fixture, 60.0);
        settle(&connection).await;
        let before = connection.generation();
        fixture
            .add_tool(common::tool("added", "Added later."))
            .await;
        eventually(|| connection.tools().iter().any(|t| t.name == "added"))
            .await;
        assert!(connection.generation() > before);
        call(&connection, "added", json!({})).await.unwrap();

        fixture.remove_tool("added").await;
        eventually(|| !connection.tools().iter().any(|t| t.name == "added"))
            .await;
        let error = call(&connection, "added", json!({})).await.unwrap_err();
        assert!(matches!(error, CallFailure::Withdrawn { .. }), "{error}");
        connection.shutdown().await;
    })
    .await;
}

/// A call the connection drops under fails and is not sent again; the
/// next call connects again.
#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_server_reconnects_and_calls_are_not_retried() {
    both(|fixture| async move {
        let connection = connect(&fixture, 60.0);
        settle(&connection).await;
        // A legacy server takes two dials: `server/discover`, then
        // `initialize`.
        let dialed = fixture.dials();
        let error = call(&connection, "crash", json!({})).await.unwrap_err();
        assert!(matches!(error, CallFailure::Disconnected { .. }), "{error}");
        assert!(error.to_string().contains("not retried"));
        eventually(|| connection.status().state == State::Disconnected).await;
        assert_eq!(fixture.calls("crash"), 1);
        assert_eq!(fixture.dials(), dialed);
        // The last tools are kept, and the next call connects again.
        assert_eq!(connection.tools().len(), 5);
        let result = call(&connection, "echo", json!({"text": "back"}))
            .await
            .unwrap();
        assert_eq!(result["structuredContent"], json!({"echo": "back"}));
        assert!(fixture.dials() > dialed);
        assert_eq!(fixture.calls("crash"), 1);
        assert_eq!(connection.status().state, State::Connected);
        connection.shutdown().await;
    })
    .await;
}

/// A server that does not answer `initialize` in time is not waited on
/// forever by a call, and a cancelled wait fails as cancelled.
#[tokio::test(flavor = "multi_thread")]
async fn a_call_waiting_for_a_connect_can_be_cancelled() {
    let fixture = Fixture::new(false);
    fixture.gate(false);
    let connection = connect(&fixture, 60.0);
    let cancel = CancellationToken::new();
    let canceller = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        canceller.cancel();
    });
    let error = connection
        .call("echo", json!({"text": "x"}), &no_progress(), &cancel)
        .await
        .unwrap_err();
    assert!(matches!(error, CallFailure::Cancelled { .. }), "{error}");
    assert_eq!(connection.status().state, State::Connecting);
    fixture.gate(true);
    settle(&connection).await;
    assert_eq!(connection.status().state, State::Connected);
    connection.shutdown().await;
}

/// An HTTP server that answers `status` to everything.
async fn http_server(status: u16) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let counter = requests.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let counter = counter.clone();
            tokio::spawn(async move {
                let mut buffer = vec![0; 65536];
                let mut seen = Vec::new();
                // Read the head and the body the length says.
                loop {
                    let Ok(n) = socket.read(&mut buffer).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    seen.extend_from_slice(&buffer[..n]);
                    let text = String::from_utf8_lossy(&seen);
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length = text[..end]
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| {
                                        value.trim().parse::<usize>().ok()
                                    })?
                            })
                            .unwrap_or(0);
                        if seen.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                counter.fetch_add(1, Ordering::SeqCst);
                let response = format!(
                    "HTTP/1.1 {status} Nope\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    (format!("http://{address}/mcp"), requests)
}

fn http_connection(url: String) -> Arc<Connection> {
    let config = ServerConfig::new(
        "remote",
        Transport::Http(HttpConfig {
            url,
            headers: vec![("Authorization".into(), "Bearer ${TOKEN}".into())],
            oauth: None,
        }),
    );
    let environment = Environment {
        env: Arc::new(|name| (name == "TOKEN").then(|| "secret".to_owned())),
        ..environment()
    };
    Connection::new(config, Origin::User, environment)
}

/// A 503 is transient: the connect is tried three times, after 250 ms
/// and 1 s. A 404 is not: one try.
#[tokio::test(flavor = "multi_thread")]
async fn http_connects_retry_transient_errors() {
    let (url, requests) = http_server(503).await;
    let connection = http_connection(url);
    let started = Instant::now();
    connection.connect();
    settle(&connection).await;
    let elapsed = started.elapsed();
    let status = connection.status();
    assert_eq!(status.state, State::Failed, "{status:?}");
    let delays: Duration = HTTP_RETRY_DELAYS.iter().sum();
    assert!(elapsed >= delays, "{elapsed:?}");
    // One POST per try: the 503 ends `server/discover` at once.
    assert_eq!(requests.load(Ordering::SeqCst), 3);

    let (url, requests) = http_server(404).await;
    let connection = http_connection(url);
    let started = Instant::now();
    connection.connect();
    settle(&connection).await;
    assert_eq!(connection.status().state, State::Failed);
    assert!(
        started.elapsed() < HTTP_RETRY_DELAYS[0],
        "{:?}",
        started.elapsed()
    );
    // `server/discover`, then `initialize` after the 404 marks a legacy
    // server; no second try.
    assert_eq!(requests.load(Ordering::SeqCst), 2);
}

/// A variable that is not set fails the server, naming it.
#[tokio::test(flavor = "multi_thread")]
async fn a_missing_variable_fails_the_server() {
    let config = ServerConfig::new(
        "remote",
        Transport::Http(HttpConfig {
            url: "http://127.0.0.1:9/mcp".into(),
            headers: vec![("Authorization".into(), "Bearer ${MISSING}".into())],
            oauth: None,
        }),
    );
    let connection = Connection::new(config, Origin::User, environment());
    connection.connect();
    settle(&connection).await;
    let status = connection.status();
    assert_eq!(status.state, State::Failed);
    assert_eq!(
        status.error.as_deref(),
        Some("the environment variable `MISSING` is not set")
    );
}
