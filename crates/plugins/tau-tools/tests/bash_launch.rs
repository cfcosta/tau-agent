//! `bash` through a launcher (`tau_agent::launch`): the environment
//! plugins give a run's commands. The launcher's words come before the
//! shell and its variables reach the command, with pipes and under a
//! terminal; without one nothing changes; a launcher that holds the
//! command back is waited for, within the command's timeout, and a
//! cancel ends the wait.

#![cfg(unix)]

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use serde_json::{Value, json};
use tau_agent::{
    error::ToolError,
    launch::{Launch, Launcher},
    tool::{AgentTool, RunId, ToolCtx, ToolOutput, ToolUpdates},
};
use tau_ai::message::InputBlock;
use tau_tools::{bash::Bash, path::Root};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

/// A launcher that answers `launch`, once `open` lets it, and keeps the
/// directories it was asked about.
struct Fake {
    launch: Launch,
    open: Option<Arc<Notify>>,
    asked: Mutex<Vec<PathBuf>>,
    answered: AtomicUsize,
}

impl Fake {
    fn new(launch: Launch) -> Self {
        Self {
            launch,
            open: None,
            asked: Mutex::default(),
            answered: AtomicUsize::new(0),
        }
    }

    fn held(mut self, open: Arc<Notify>) -> Self {
        self.open = Some(open);
        self
    }
}

#[async_trait]
impl Launcher for Fake {
    async fn launch(&self, dir: &Path) -> Launch {
        self.asked.lock().unwrap().push(dir.to_owned());
        if let Some(open) = &self.open {
            open.notified().await;
        }
        self.answered.fetch_add(1, Ordering::SeqCst);
        self.launch.clone()
    }
}

/// `env PROBE=prefixed` before the shell, and `EXTRA=set` over its
/// environment.
fn probing() -> Launch {
    Launch {
        prefix: vec!["/usr/bin/env".into(), "PROBE=prefixed".into()],
        env: vec![("EXTRA".into(), "set".into())],
    }
}

/// Both ways `bash` runs commands.
fn modes(root: &Root) -> Vec<Bash> {
    let pipes = Bash::new(root.clone());
    #[cfg(feature = "terminal")]
    {
        vec![
            pipes.with_terminal(false),
            Bash::new(root.clone()).with_terminal(true),
        ]
    }
    #[cfg(not(feature = "terminal"))]
    vec![pipes]
}

fn ctx(cancel: CancellationToken) -> ToolCtx {
    let (updates, _) = ToolUpdates::channel("call_1");
    ToolCtx::new(cancel, updates, RunId("run_1".into()))
}

fn text_of(output: &ToolOutput) -> String {
    match &output.content[0] {
        InputBlock::Text(text) => text.text.replace("\r\n", "\n"),
        InputBlock::Image(_) => panic!("expected a text block"),
    }
}

fn failure(error: &ToolError) -> (String, Value) {
    match error {
        ToolError::Output(output) => (
            text_of(output),
            output.structured.clone().expect("structured"),
        ),
        other => panic!("expected an output error: {other}"),
    }
}

const ECHO: &str = "echo \"${PROBE:-none} ${EXTRA:-none} $(pwd)\"";

#[tokio::test(flavor = "multi_thread")]
async fn the_launchers_words_and_variables_reach_the_command() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path().canonicalize().unwrap();
    let root = Root::new(dir.clone());
    for bash in modes(&root) {
        let fake = Arc::new(Fake::new(probing()));
        let bash = bash.with_launcher(fake.clone());
        let output = bash
            .call(json!({ "command": ECHO }), ctx(CancellationToken::new()))
            .await
            .unwrap();
        assert_eq!(
            text_of(&output).trim_end(),
            format!("prefixed set {}", dir.display())
        );
        assert_eq!(*fake.asked.lock().unwrap(), std::slice::from_ref(&dir));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_launcher_or_with_a_direct_one_nothing_changes() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path().canonicalize().unwrap();
    let root = Root::new(dir.clone());
    for (bash, direct) in modes(&root).into_iter().zip(modes(&root)) {
        let direct =
            direct.with_launcher(Arc::new(Fake::new(Launch::default())));
        for bash in [bash, direct] {
            let output = bash
                .call(json!({ "command": ECHO }), ctx(CancellationToken::new()))
                .await
                .unwrap();
            assert_eq!(
                text_of(&output).trim_end(),
                format!("none none {}", dir.display())
            );
        }
    }
}

/// The command starts only once the launcher answers.
#[tokio::test(flavor = "multi_thread")]
async fn a_held_command_starts_once_the_launcher_answers() {
    let dir = tempfile::tempdir().unwrap();
    let root = Root::new(dir.path().to_owned());
    for bash in modes(&root) {
        let open = Arc::new(Notify::new());
        let fake = Arc::new(Fake::new(probing()).held(open.clone()));
        let bash = bash.with_launcher(fake.clone());
        let marker = dir.path().join("ran");
        let _ = std::fs::remove_file(&marker);
        let call = tokio::spawn(async move {
            bash.call(
                json!({ "command": "touch ran; echo $PROBE", "timeout": 30 }),
                ctx(CancellationToken::new()),
            )
            .await
        });
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!marker.exists(), "the command waits for the launcher");
        assert_eq!(fake.answered.load(Ordering::SeqCst), 0);
        open.notify_one();
        let output = call.await.unwrap().unwrap();
        assert_eq!(text_of(&output).trim_end(), "prefixed");
        assert!(marker.exists());
    }
}

/// The command's timeout counts the wait: past it, the command never
/// starts and times out, saying why.
#[tokio::test(flavor = "multi_thread")]
async fn the_wait_counts_against_the_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let root = Root::new(dir.path().to_owned());
    for bash in modes(&root) {
        let fake = Arc::new(Fake::new(probing()).held(Arc::new(Notify::new())));
        let bash = bash.with_launcher(fake);
        let error = bash
            .call(
                json!({ "command": "touch ran", "timeout": 0.3 }),
                ctx(CancellationToken::new()),
            )
            .await
            .unwrap_err();
        let (text, structured) = failure(&error);
        assert_eq!(
            text,
            "Command timed out waiting for the repository's environment to load"
        );
        assert_eq!(structured["status"], "timed_out");
        assert_eq!(structured["exit_code"], Value::Null);
        assert!(!dir.path().join("ran").exists());
    }
}

/// A cancel while the launcher holds the command ends it, unstarted.
#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_ends_the_wait() {
    let dir = tempfile::tempdir().unwrap();
    let root = Root::new(dir.path().to_owned());
    for bash in modes(&root) {
        let fake = Arc::new(Fake::new(probing()).held(Arc::new(Notify::new())));
        let bash = bash.with_launcher(fake);
        let cancel = CancellationToken::new();
        let call = {
            let cancel = cancel.clone();
            tokio::spawn(async move {
                bash.call(json!({ "command": "touch ran" }), ctx(cancel))
                    .await
            })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel.cancel();
        let error = call.await.unwrap().unwrap_err();
        let (text, structured) = failure(&error);
        assert_eq!(text, "Command aborted");
        assert_eq!(structured["status"], "cancelled");
        assert!(!dir.path().join("ran").exists());
    }
}
