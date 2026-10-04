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
/// Where the server writes what its launcher set, when given.
const SEEN: &str = "TAU_MCP_TEST_SEEN";

/// The tests, by name.
const TESTS: [&str; 2] = [
    "closing_ends_the_server_and_its_children",
    "a_launcher_starts_the_server",
];

/// Speaks enough of libtest's command line for `cargo test` and nextest:
/// `--list` names the tests (nextest asks with `--format terse`, and asks
/// for ignored ones apart, of which there are none), and a name runs the
/// tests that match it, exactly with `--exact`; none runs them all.
fn main() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    if std::env::var_os(SERVE).is_some() {
        runtime.block_on(serve());
        return;
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().any(|arg| arg == name);
    if flag("--list") {
        if !flag("--ignored") {
            for name in TESTS {
                println!("{name}: test");
            }
        }
        return;
    }
    if flag("--ignored") {
        return;
    }
    let filter = args.iter().find(|arg| !arg.starts_with('-'));
    let chosen = TESTS.into_iter().filter(|name| match filter {
        None => true,
        Some(filter) if flag("--exact") => name == filter,
        Some(filter) => name.contains(filter.as_str()),
    });
    for name in chosen {
        match name {
            "closing_ends_the_server_and_its_children" => {
                runtime.block_on(closing_ends_the_server_and_its_children())
            }
            _ => runtime.block_on(a_launcher_starts_the_server()),
        }
        println!("{name}: ok");
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
    if let Ok(seen) = std::env::var(SEEN) {
        let var = |name| std::env::var(name).unwrap_or_else(|_| "none".into());
        let text = format!("{} {}", var("LAUNCHED"), var("EXTRA"));
        std::fs::write(seen, text).unwrap();
    }
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
        auth: None,
        launcher: Default::default(),
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

/// Answers `env LAUNCHED=prefix` with `EXTRA=env`, keeping the
/// directory it was asked about.
struct Probe(std::sync::Mutex<Vec<std::path::PathBuf>>);

#[async_trait::async_trait]
impl tau_agent::launch::Launcher for Probe {
    async fn launch(&self, dir: &Path) -> tau_agent::launch::Launch {
        self.0.lock().unwrap().push(dir.to_owned());
        tau_agent::launch::Launch {
            prefix: vec!["/usr/bin/env".into(), "LAUNCHED=prefix".into()],
            env: vec![("EXTRA".into(), "env".into())],
        }
    }
}

/// A stdio server of a repository with a launcher starts through it,
/// asked for the repository's main workspace.
async fn a_launcher_starts_the_server() {
    let dir = tempfile::tempdir().unwrap();
    let seen = dir.path().join("seen");
    let exe = std::env::current_exe().unwrap();
    let config = ServerConfig::new(
        "launched",
        Transport::Stdio(StdioConfig {
            command: exe.display().to_string(),
            args: Vec::new(),
            env: vec![
                (SERVE.into(), "1".into()),
                (PIDS.into(), dir.path().join("pids").display().to_string()),
                (SEEN.into(), seen.display().to_string()),
            ],
            cwd: None,
        }),
    );
    let environment = Environment {
        env: Arc::new(|_| None),
        home: None,
        repo: Some(dir.path().to_owned()),
        auth: None,
        launcher: Default::default(),
    };
    let probe = Arc::new(Probe(Default::default()));
    let main = dir.path().join("main");
    environment.set_launcher(tau_ui_plugin::RepoLauncher {
        launcher: probe.clone(),
        dir: main.clone(),
    });
    let connection = Connection::new(config, Origin::Repo, environment);
    connection.connect();
    connection.settled(&CancellationToken::new()).await;
    assert_eq!(
        connection.status().state,
        State::Connected,
        "{:?}",
        connection.status()
    );
    assert_eq!(read(&seen), "prefix env");
    assert_eq!(*probe.0.lock().unwrap(), [main]);
    connection.shutdown().await;
}
