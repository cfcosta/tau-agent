//! `exec_command` through a launcher (`tau_agent::launch`): the
//! environment plugins give a run's commands. The launcher's words
//! come before the shell and its variables reach the command; without
//! one nothing changes; a launcher that holds the command back is
//! waited for, and a cancel ends the wait.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]
#![cfg(unix)]

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use serde_json::json;
use tau_agent::{
    launch::{Launch, Launcher},
    tool::{AgentTool, RunId, ToolCtx, ToolOutput, ToolUpdates},
};
use tau_ai::message::InputBlock;
use tau_tools_host::{
    exec::{ExecCommand, Processes},
    path::Root,
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

/// A launcher that answers `launch`, once `open` lets it, keeps the
/// directories it was asked about, and says it was asked through
/// `waiting`.
struct Fake {
    launch: Launch,
    open: Option<Arc<Notify>>,
    asked: Mutex<Vec<PathBuf>>,
    waiting: Arc<Notify>,
    answered: AtomicUsize,
}

impl Fake {
    fn new(launch: Launch) -> Self {
        Self {
            launch,
            open: None,
            asked: Mutex::default(),
            waiting: Arc::new(Notify::new()),
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
        self.waiting.notify_one();
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

const ECHO: &str = "echo \"${PROBE:-none} ${EXTRA:-none} $(pwd)\"";

#[tokio::test(flavor = "multi_thread")]
async fn the_launchers_words_and_variables_reach_the_command() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path().canonicalize().unwrap();
    let fake = Arc::new(Fake::new(probing()));
    let exec = ExecCommand::new(Root::new(dir.clone()), Processes::new())
        .with_launcher(fake.clone());
    let output = exec
        .call(json!({ "cmd": ECHO }), ctx(CancellationToken::new()))
        .await
        .unwrap();
    assert_eq!(
        text_of(&output).trim_end(),
        format!("prefixed set {}", dir.display())
    );
    assert_eq!(*fake.asked.lock().unwrap(), std::slice::from_ref(&dir));
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_launcher_or_with_a_direct_one_nothing_changes() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path().canonicalize().unwrap();
    let plain = ExecCommand::new(Root::new(dir.clone()), Processes::new());
    let direct = ExecCommand::new(Root::new(dir.clone()), Processes::new())
        .with_launcher(Arc::new(Fake::new(Launch::default())));
    for exec in [plain, direct] {
        let output = exec
            .call(json!({ "cmd": ECHO }), ctx(CancellationToken::new()))
            .await
            .unwrap();
        assert_eq!(
            text_of(&output).trim_end(),
            format!("none none {}", dir.display())
        );
    }
}

/// The command starts only once the launcher answers, and with its
/// answer: it prints the launcher's variable.
#[tokio::test(flavor = "multi_thread")]
async fn a_held_command_starts_once_the_launcher_answers() {
    let dir = tempfile::tempdir().unwrap();
    let open = Arc::new(Notify::new());
    let fake = Arc::new(Fake::new(probing()).held(open.clone()));
    let exec = ExecCommand::new(Root::new(dir.path()), Processes::new())
        .with_launcher(fake.clone());
    let marker = dir.path().join("ran");
    let call = tokio::spawn(async move {
        exec.call(
            json!({ "cmd": "touch ran; echo $PROBE" }),
            ctx(CancellationToken::new()),
        )
        .await
    });
    // The call is waiting on the launcher.
    fake.waiting.notified().await;
    assert!(!marker.exists(), "the command waits for the launcher");
    assert_eq!(fake.answered.load(Ordering::SeqCst), 0);
    open.notify_one();
    let output = call.await.unwrap().unwrap();
    assert_eq!(text_of(&output).trim_end(), "prefixed");
    assert!(marker.exists());
}

/// A cancel while the launcher holds the command ends it, unstarted.
#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_ends_the_wait() {
    let dir = tempfile::tempdir().unwrap();
    let processes = Processes::new();
    let fake = Arc::new(Fake::new(probing()).held(Arc::new(Notify::new())));
    let exec = ExecCommand::new(Root::new(dir.path()), processes.clone())
        .with_launcher(fake.clone());
    let cancel = CancellationToken::new();
    let call = {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            exec.call(json!({ "cmd": "touch ran" }), ctx(cancel)).await
        })
    };
    // Cancelled while the launcher holds it.
    fake.waiting.notified().await;
    cancel.cancel();
    let error = call.await.unwrap().unwrap_err();
    assert_eq!(error.to_string(), "Operation aborted");
    assert!(!dir.path().join("ran").exists());
    assert!(processes.is_empty());
}
