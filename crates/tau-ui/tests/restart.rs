//! What a host finishes as it starts, for what the last one left when
//! it closed: a host over the same store, repository list and project
//! stands for tau started again.

use std::{path::PathBuf, time::Duration};

use tau_agent::{agent::Agent, event::RunEvent, tool::RunId};
use tau_store::{Entry, Store, TurnUsage};
use tau_testing::{git::git, scripted::ScriptedModel};
use tau_ui::{
    accounts::Credentials,
    host::{Host, HostConfig},
};
use tau_ui_remote::models::ModelChoice;
use tau_vcs::{Identity, Project};
use tokio::sync::mpsc::UnboundedReceiver;

const REPO: &str = "repo";
const WAIT: Duration = Duration::from_secs(60);

/// What outlives a host: its store, repository list and project.
struct Disk {
    dir: tempfile::TempDir,
    _src: tempfile::TempDir,
    project: Project,
}

impl Disk {
    fn new() -> Self {
        let src = tempfile::tempdir().unwrap();
        git(src.path(), &["init", "--quiet"]);
        std::fs::write(src.path().join("README.md"), "hello\n").unwrap();
        git(src.path(), &["add", "README.md"]);
        git(src.path(), &["commit", "--quiet", "-m", "first"]);
        let dir = tempfile::tempdir().unwrap();
        let project = Project::import(
            src.path().to_str().unwrap(),
            dir.path().join("p"),
            Identity::default(),
        )
        .unwrap();
        Self {
            dir,
            _src: src,
            project,
        }
    }

    fn db(&self) -> PathBuf {
        self.dir.path().join("runs.db")
    }

    /// A host started on what is on disk, as tau starts.
    fn start(&self, llm: ScriptedModel) -> (Host, UnboundedReceiver<RunEvent>) {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let store = runtime.block_on(Store::open(self.db())).unwrap();
        let root = self.dir.path();
        let config = HostConfig {
            account: tau_ai::chatgpt::AccountId::parse("test-account").unwrap(),
            credentials: Credentials::new(root.join("config")),
            model: Some("gpt-6-luna".into()),
            store: self.db(),
            repos: root.join("repos"),
            settings: root.join("models.json"),
            repo_list: root.join("repos.json"),
        };
        let agent = Agent::new(llm).name("coder");
        let (host, events) = Host::with_agent(runtime, agent, store, config);
        (host.with_repo(REPO, self.project.clone()), events)
    }

    /// Stores `entry` on `run`, as a host would have before it closed.
    fn store(&self, run: &RunId, entry: Entry) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let store = Store::open(self.db()).await.unwrap();
            store
                .append_turn(&run.0, &[entry], TurnUsage::default())
                .await
                .unwrap();
        });
    }
}

fn until_end(events: &mut UnboundedReceiver<RunEvent>) {
    let deadline = std::time::Instant::now() + WAIT;
    while std::time::Instant::now() < deadline {
        match events.try_recv() {
            Ok(RunEvent::RunEnd { .. }) => return,
            Ok(_) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    panic!("the run did not end");
}

fn wait_until_done(host: &Host, run: &RunId) {
    let deadline = std::time::Instant::now() + WAIT;
    while host.is_running(run) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!host.is_running(run));
}

/// A chat on `prompt` that writes and commits `file`.
fn chat(
    host: &Host,
    events: &mut UnboundedReceiver<RunEvent>,
    prompt: &str,
) -> RunId {
    let view = host.start(prompt, &ModelChoice::default(), REPO).unwrap();
    until_end(events);
    wait_until_done(host, &view.id);
    view.id
}

fn writes(llm: ScriptedModel, file: &str) -> ScriptedModel {
    let write = serde_json::json!({ "path": file, "content": "x\n" });
    let commit = serde_json::json!({ "message": format!("feat: {file}") });
    llm.turn(|t| t.tool_call("write", write))
        .turn(|t| t.tool_call("vcs_commit", commit))
        .turn(|t| t.text("done"))
}

/// Started again, tau sweeps what no open chat owns: a chat whose drop
/// was cut off after its record loses its workspace, bookmark and
/// commits, and a workspace no run owns goes. An open chat, finished but
/// not landed, keeps its workspace and bookmark, and goes on in them;
/// the main chat's checkout stays.
#[test]
fn a_restart_sweeps_what_no_open_chat_owns() {
    let disk = Disk::new();
    let llm = writes(writes(ScriptedModel::new(), "kept.txt"), "cut.txt");
    let (host, mut events) = disk.start(llm);
    let kept = chat(&host, &mut events, "keep this");
    let cut = chat(&host, &mut events, "drop this");
    let kept_dir = host.workspace(&kept).unwrap();
    let cut_dir = host.workspace(&cut).unwrap();
    drop(host);
    // tau closed as it dropped `cut`: its record is stored, nothing else.
    disk.store(
        &cut,
        Entry::Plugin {
            plugin: tau_ui_remote::view::DROPPED_RECORD.to_owned(),
            body: "{}".into(),
        },
    );
    let trunk = disk.project.trunk().unwrap();
    disk.project.add_workspace("stray-1", &trunk).unwrap();
    let cut_head = disk.project.bookmark(&format!("tau/{}", cut.0)).unwrap();
    assert!(cut_head.is_some());

    let llm = ScriptedModel::new().turn(|t| t.text("still here"));
    let (host, mut events) = disk.start(llm.clone());
    host.recover().unwrap();
    let project = &disk.project;
    let workspaces = project.workspaces().unwrap();
    let kept_name = kept_dir.file_name().unwrap().to_str().unwrap();
    assert_eq!(workspaces, [kept_name]);
    assert!(kept_dir.join("kept.txt").exists());
    assert!(!cut_dir.exists());
    assert!(!project.workspace_dir("stray-1").exists());
    assert!(project.workspace_dir(tau_vcs::DEFAULT_WORKSPACE).exists());
    assert_eq!(
        project.bookmarks("tau/").unwrap(),
        [format!("tau/{}", kept.0)]
    );
    // The cut chat's commit is no longer on any stack: trunk's has not
    // got it, and sweeping again finds nothing.
    let trunk = project.trunk().unwrap();
    assert!(project.file_at(&trunk, "cut.txt").unwrap().is_none());
    host.recover().unwrap();
    assert_eq!(project.workspaces().unwrap(), [kept_name]);

    // The open chat goes on where it was.
    host.resume(&kept, "anything else?", &ModelChoice::default())
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &kept);
    assert_eq!(host.workspace(&kept).unwrap(), kept_dir);
    llm.assert_exhausted();
}

impl Disk {
    /// The run `run` as the store has it, read from a store of its own.
    fn record(&self, run: &RunId) -> tau_store::RunRecord {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let store = Store::open(self.db()).await.unwrap();
            store.run(&run.0).await.unwrap().unwrap()
        })
    }

    /// Leaves `run` `running` in the store, as a tau that closed during
    /// its turn did.
    fn leave_running(&self, run: &RunId) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let store = Store::open(self.db()).await.unwrap();
            store.reopen_run(&run.0, "gpt-6-luna").await.unwrap();
        });
    }
}

/// A run tau closed during stays `running` in the store until tau starts
/// again, which marks it interrupted: history shows it so. Resumed, it
/// goes on in the workspace it has, told it was cut off.
#[test]
fn a_run_cut_off_by_a_restart_is_interrupted_and_resumes() {
    use tau_ui_remote::view::RunStatus;
    let disk = Disk::new();
    let (host, mut events) = disk.start(writes(ScriptedModel::new(), "a.txt"));
    let run = chat(&host, &mut events, "write a");
    let dir = host.workspace(&run).unwrap();
    drop(host);
    disk.leave_running(&run);
    std::fs::write(dir.join("half.txt"), "the cut-off turn's\n").unwrap();

    let commit = serde_json::json!({ "message": "feat: the cut-off turn" });
    let llm = ScriptedModel::new()
        .turn(|t| t.text("picking up"))
        .turn(|t| t.tool_call("vcs_commit", commit))
        .turn(|t| t.text("done"));
    let (host, mut events) = disk.start(llm.clone());
    assert_eq!(disk.record(&run).status, tau_store::Status::Interrupted);
    let view = host
        .history()
        .unwrap()
        .into_iter()
        .find(|view| view.id == run)
        .unwrap();
    assert_eq!(view.status, RunStatus::Interrupted);
    assert!(!view.status.is_live());
    host.recover().unwrap();
    assert!(
        dir.join("half.txt").exists(),
        "an open chat keeps its files"
    );

    // Only a run tau cut off resumes so.
    let main = host.main_of(REPO).unwrap();
    assert!(host.resume_cut_off(&main).is_err());
    host.resume_cut_off(&run).unwrap();
    until_end(&mut events);
    wait_until_done(&host, &run);
    assert_eq!(host.workspace(&run).unwrap(), dir);
    // The first request after the restart ends on tau's message.
    let asked = llm.requests();
    let told = asked[0].transcript.last().unwrap();
    let told = serde_json::to_string(told).unwrap();
    assert!(told.contains("tau closed while you were working"), "{told}");
    assert_eq!(disk.record(&run).status, tau_store::Status::Done);
    assert!(host.resume_cut_off(&run).is_err(), "not cut off any more");
}
