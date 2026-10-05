//! What a host finishes as it starts, for what the last one left when
//! it closed: a host over the same store, repository list and project
//! stands for tau started again.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0027)"
)]

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
        let project = tau_vcs::ProjectRepo::import(
            src.path().to_str().unwrap(),
            dir.path().join("p"),
            Identity::default(),
        )
        .map(Project::from)
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
        // tau lists only repositories from GitHub, as this one stands
        // for: so the list keeps its main chat across starts.
        let list = root.join("repos.json");
        if let Ok(text) = std::fs::read_to_string(&list) {
            let mut saved: serde_json::Value =
                serde_json::from_str(&text).unwrap();
            for repo in saved["repos"].as_array_mut().into_iter().flatten() {
                repo["github"] = serde_json::json!("owner/repo");
            }
            std::fs::write(&list, saved.to_string()).unwrap();
        }
        let config = HostConfig {
            account: tau_ai::chatgpt::AccountId::parse("test-account").unwrap(),
            credentials: Credentials::new(root.join("config")),
            model: Some("gpt-6-luna".into()),
            store: self.db(),
            repos: root.join("repos"),
            settings: root.join("models.json"),
            repo_list: root.join("repos.json"),
            skills: std::env::temp_dir().join("tau-test-skills-none"),
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
    let trunk = disk.project.blocking().trunk().unwrap();
    disk.project
        .blocking()
        .add_workspace("stray-1", &trunk)
        .unwrap();
    let cut_head = disk
        .project
        .blocking()
        .bookmark(&format!("tau/{}", cut.0))
        .unwrap();
    assert!(cut_head.is_some());

    let llm = ScriptedModel::new().turn(|t| t.text("still here"));
    let (host, mut events) = disk.start(llm.clone());
    host.recover().unwrap();
    let project = &disk.project;
    let workspaces = project.blocking().workspaces().unwrap();
    let kept_name = kept_dir.file_name().unwrap().to_str().unwrap();
    assert_eq!(workspaces, [kept_name]);
    assert!(kept_dir.join("kept.txt").exists());
    assert!(!cut_dir.exists());
    assert!(!project.workspace_dir("stray-1").exists());
    assert!(project.workspace_dir(tau_vcs::DEFAULT_WORKSPACE).exists());
    assert_eq!(
        project.blocking().bookmarks("tau/").unwrap(),
        [format!("tau/{}", kept.0)]
    );
    // The cut chat's commit is no longer on any stack: trunk's has not
    // got it, and sweeping again finds nothing.
    let trunk = project.blocking().trunk().unwrap();
    assert!(
        project
            .blocking()
            .file_at(&trunk, "cut.txt")
            .unwrap()
            .is_none()
    );
    host.recover().unwrap();
    assert_eq!(project.blocking().workspaces().unwrap(), [kept_name]);

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

/// A landed change as `ChangeInfo` says it, less its ids: description,
/// empty, conflict, bookmarks.
type ChangeSeen = (String, bool, bool, Vec<String>);

/// What a landing of a chat that wrote `a.txt` left, in terms that do
/// not hang on ids, which differ from one project to the next.
#[derive(Debug, PartialEq)]
struct Landed {
    /// `a.txt` on trunk, and in the main chat's checkout.
    on_trunk: Option<Vec<u8>>,
    in_checkout: bool,
    workspaces: usize,
    run_bookmarks: usize,
    /// The main chat's landing cards: the chat's title, and each change
    /// as `ChangeInfo` says it, less its ids.
    cards: Vec<(String, Vec<ChangeSeen>)>,
    /// The main chat's links that a landing brought.
    landed_links: usize,
    /// How many changes the chat's ending says landed.
    ending: Option<usize>,
}

fn landed_state(disk: &Disk, host: &Host, chat: &RunId) -> Landed {
    use tau_ui_remote::view::{Ending, Item};
    let project = &disk.project;
    let trunk = project.blocking().trunk().unwrap();
    let main = host.main_of(REPO).unwrap();
    let history = host.history().unwrap();
    let main_view = history.iter().find(|view| view.id == main).unwrap();
    let records = tokio::runtime::Runtime::new().unwrap().block_on(async {
        let store = Store::open(disk.db()).await.unwrap();
        store
            .plugin_entries(&main.0, tau_vcs::run_workspace::PLUGIN)
            .await
            .unwrap()
    });
    Landed {
        on_trunk: project
            .blocking()
            .file_at(&trunk, "a.txt")
            .unwrap()
            .map(|(bytes, _)| bytes),
        in_checkout: project
            .workspace_dir(tau_vcs::DEFAULT_WORKSPACE)
            .join("a.txt")
            .exists(),
        workspaces: project.blocking().workspaces().unwrap().len(),
        run_bookmarks: project.blocking().bookmarks("tau/").unwrap().len(),
        cards: main_view
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Landed(card) => Some((
                    card.title.clone(),
                    card.changes
                        .iter()
                        .map(|change| {
                            let info = &change.info;
                            (
                                info.description.clone(),
                                info.empty,
                                info.conflict,
                                // The chat's bookmark is named by its id.
                                info.bookmarks
                                    .iter()
                                    .map(|name| {
                                        name.replace(&*chat.0, "<chat>")
                                    })
                                    .collect(),
                            )
                        })
                        .collect(),
                )),
                _ => None,
            })
            .collect(),
        landed_links: records
            .iter()
            .filter_map(|(_, body)| tau_vcs::Link::parse(body))
            .filter(|link| link.from.as_deref() == Some(&*chat.0))
            .count(),
        ending: match host.ending_of(chat).unwrap() {
            Some(Ending::Landed { changes, .. }) => Some(changes),
            _ => None,
        },
    }
}

/// A chat landed on main, with tau closing after `cut` (never, when
/// `None`), then started again: what the landing left once the start
/// finished it.
fn land_cut_off(cut: Option<tau_ui::host::LandingStep>) -> Landed {
    let disk = Disk::new();
    let (host, mut events) = disk.start(writes(ScriptedModel::new(), "a.txt"));
    let chat = chat(&host, &mut events, "write a");
    host.cut_landing_after(cut);
    let landed = host.land(&chat);
    assert_eq!(landed.is_ok(), cut.is_none(), "{landed:?}");
    drop(host);

    let (host, _events) = disk.start(ScriptedModel::new());
    let finished = host.recover().unwrap();
    use tau_ui::host::LandingStep;
    match cut {
        // Once its record is stored, a landing is done but for tidying,
        // which the start's sweep does: there is nothing to tell.
        None | Some(LandingStep::Record | LandingStep::Workspace) => {
            assert!(finished.is_empty(), "{cut:?}")
        }
        Some(LandingStep::Intent | LandingStep::Restack) => {
            assert_eq!(finished.len(), 1, "{cut:?}");
            assert!(finished[0].recovered);
            assert_eq!(finished[0].title, "write a");
            // Its card in main says tau finished it.
            let main = host.main_of(REPO).unwrap();
            let history = host.history().unwrap();
            let view = history.iter().find(|view| view.id == main).unwrap();
            assert!(view.items.iter().any(|item| matches!(
                item,
                tau_ui_remote::view::Item::Landed(card) if card.recovered
            )));
        }
    }
    // A landed chat takes no more messages, whatever cut it off.
    assert!(host.resume(&chat, "more", &ModelChoice::default()).is_err());
    // Starting again finds nothing left to finish.
    assert!(host.recover().unwrap().is_empty());
    landed_state(&disk, &host, &chat)
}

/// A landing cut off after any of its steps ends, once tau starts again,
/// as one that ran whole: main has the chat's change on trunk and in its
/// checkout, the same card and links, and the chat is landed, without a
/// workspace or a bookmark.
#[test]
fn a_landing_cut_off_at_any_step_finishes_at_start() {
    let whole = land_cut_off(None);
    assert_eq!(whole.on_trunk.as_deref(), Some(&b"x\n"[..]));
    assert!(whole.in_checkout);
    assert_eq!(whole.workspaces, 0);
    assert_eq!(whole.run_bookmarks, 0);
    assert_eq!(whole.landed_links, 1);
    assert_eq!(whole.ending, Some(1));
    assert_eq!(whole.cards.len(), 1);
    for step in tau_ui::host::LandingStep::ALL {
        assert_eq!(land_cut_off(Some(step)), whole, "cut after {step:?}");
    }
}
