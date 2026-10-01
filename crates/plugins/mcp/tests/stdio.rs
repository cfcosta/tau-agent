//! A server over stdio, in a child process (`docs/reference/mcp.md`,
//! "Tests"): this binary runs itself as the server. Closing the
//! connection ends the server and whatever it started, by its process
//! group.

mod common;

use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use rmcp::ServiceExt;
use serde_json::json;
use tau_mcp::{
    config::{Origin, ServerConfig, StdioConfig, Transport},
    connection::{Connection, Environment, State},
};
use tokio_util::sync::CancellationToken;

const SERVE: &str = "TAU_MCP_TEST_SERVE";
const PIDS: &str = "TAU_MCP_TEST_PIDS";

fn main() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    if std::env::var_os(SERVE).is_some() {
        runtime.block_on(serve());
    } else {
        runtime.block_on(closing_ends_the_server_and_its_children());
        println!("stdio: ok");
    }
}

/// The server side: starts a long `sleep`, writes its own pid and the
/// sleep's, and serves until stdin closes.
#[expect(
    clippy::zombie_processes,
    reason = "the sleep must outlive this process, for the group kill to end it"
)]
async fn serve() {
    let sleeper = std::process::Command::new("sleep")
        .arg("60")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let pids = format!("{} {}", std::process::id(), sleeper.id());
    std::fs::write(std::env::var(PIDS).unwrap(), pids).unwrap();
    let server = common::Server(common::State::new(false));
    let service = server.serve(rmcp::transport::stdio()).await.unwrap();
    let _ = service.waiting().await;
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

async fn closing_ends_the_server_and_its_children() {
    let dir = tempfile::tempdir().unwrap();
    let pids_file = dir.path().join("pids");
    let exe = std::env::current_exe().unwrap();
    let config = ServerConfig::new(
        "child",
        Transport::Stdio(StdioConfig {
            command: exe.display().to_string(),
            args: Vec::new(),
            env: vec![
                (SERVE.into(), "1".into()),
                (PIDS.into(), pids_file.display().to_string()),
            ],
            cwd: Some(".".into()),
        }),
    );
    let environment = Environment {
        env: Arc::new(|_| None),
        home: None,
        repo: Some(dir.path().to_owned()),
    };
    let connection = Connection::new(config, Origin::User, environment);
    connection.connect();
    connection.settled(&CancellationToken::new()).await;
    assert_eq!(
        connection.status().state,
        State::Connected,
        "{:?}",
        connection.status()
    );
    assert_eq!(connection.tools().len(), 5);
    let result = connection
        .call(
            "echo",
            json!({"text": "over stdio"}),
            &|_| {},
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(result["structuredContent"], json!({"echo": "over stdio"}));

    let pids: Vec<i32> = read(&pids_file)
        .split_whitespace()
        .map(|pid| pid.parse().unwrap())
        .collect();
    assert!(pids.iter().all(|pid| alive(*pid)));
    connection.shutdown().await;
    let deadline = Instant::now() + Duration::from_secs(5);
    while pids.iter().any(|pid| alive(*pid)) {
        assert!(Instant::now() < deadline, "still running: {pids:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}
