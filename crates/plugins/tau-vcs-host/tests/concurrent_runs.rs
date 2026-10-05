//! Runs in one project, side by side (`docs/reference/vcs.md`,
//! "Threading", "Stale working copies", "A repository's main chat"):
//! the host runs chats in parallel, each on its own `Vcs` thread and
//! jj workspace of one repository, while the main chat takes turns,
//! catches up with trunk, takes landings and drops, and an update
//! brings upstream's commits. jj sees these as concurrent operations
//! and merges the operation log's heads on the next load.
//!
//! The property runs rounds. Each round draws one action for the main
//! chat, one for the host (an update, or a new chat), and one for each
//! open chat, then runs them all at once from threads that start
//! together. Each actor owns one file: the main chat `main.txt`,
//! upstream `up.txt`, chat `i` `chat<i>.txt`. So every order of a
//! round's actions has the same outcome, and the model needs no
//! interleaving: what any sequential order does is what the round must
//! do. After each round it holds to:
//! - no action fails;
//! - each run's own file on disk is what it last wrote: no work is
//!   lost, whatever ran beside it;
//! - the main chat's workspace holds each landed chat's file, as the
//!   chat committed it, and no other chat's;
//! - trunk's bookmark names one commit, never the root, and holds
//!   upstream's newest commit;
//! - `ProjectRepo::current` resolves every change a run committed to one
//!   commit, and once the main chat has caught up, trunk holds the main
//!   chat's commits and every landed chat's;
//! - an open chat's bookmark names its newest commit.
//!
//! At the end the main chat catches up and commits once more, and
//! trunk's files are the model's: the main chat's, upstream's, and the
//! landed chats'.
//!
//! The races it found are examples below, each one shrunk to its steps,
//! with what went wrong before the fix. The landing and the drop follow
//! the host as it is now: the chat's head is read after the main chat's
//! catch-up, and a drop catches up and keeps what the main chat's `@`
//! stands on.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Barrier},
};

use hegel::{TestCase, generators as gs};
use tau_testing::{block_on, git::git};
use tau_vcs_host::{
    DEFAULT_WORKSPACE,
    Identity,
    Link,
    ProjectRepo,
    UpdateFrom,
    Vcs,
};

/// A file's contents; `None` is no file.
type Val = Option<&'static str>;

const VALUES: [Val; 3] = [None, Some("x\n"), Some("y\n")];
/// Chats made in one case, open or closed.
const MAX_CHATS: usize = 4;
const ROOT: &str = "0000000000000000000000000000000000000000";

fn write(dir: &Path, path: &str, value: Val) {
    let file = dir.join(path);
    match value {
        Some(text) => std::fs::write(&file, text).unwrap(),
        None => match std::fs::remove_file(&file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => panic!("{e}"),
        },
    }
}

fn read(dir: &Path, path: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(path)).ok()
}

fn chat_file(chat: usize) -> String {
    format!("chat{chat}.txt")
}

fn change_link(change_id: &str, commit_id: &str) -> Link {
    Link {
        turn: 0,
        workspace: String::new(),
        commit_id: commit_id.to_owned(),
        change_id: change_id.to_owned(),
        changed: true,
        from: None,
        snapshot: false,
    }
}

/// A project imported from a one-commit checkout, and its main chat's
/// workspace caught up with trunk, as the host has it before the main
/// chat's first turn.
struct Repo {
    home: tempfile::TempDir,
    project: ProjectRepo,
    trunk_name: String,
    /// The main chat's own `Vcs`, as its `RunWorkspace` has it.
    main: Vcs,
}

impl Repo {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let src = home.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        git(&src, &["init", "--quiet"]);
        std::fs::write(src.join("a.txt"), "one\n").unwrap();
        git(&src, &["add", "."]);
        git(&src, &["commit", "--quiet", "-m", "first"]);
        let project = tau_vcs_host::ProjectRepo::import(
            src.to_str().unwrap(),
            home.path().join("p"),
            Identity::default(),
        )
        .unwrap();
        let trunk_name = project.trunk_name().unwrap();
        catch_up(&project).unwrap();
        let main = tau_testing::block_on_io(tau_vcs_host::Vcs::open(
            project.workspace_dir(DEFAULT_WORKSPACE),
            Identity::default(),
        ))
        .unwrap();
        Self {
            home,
            project,
            trunk_name,
            main,
        }
    }

    fn src(&self) -> PathBuf {
        self.home.path().join("src")
    }

    fn main_dir(&self) -> PathBuf {
        self.project.workspace_dir(DEFAULT_WORKSPACE)
    }

    /// Commits `value` as `up.txt` in the source checkout. Returns the
    /// new commit, for `ProjectRepo::update` to bring in.
    fn push_upstream(&self, value: Val) -> String {
        let src = self.src();
        write(&src, "up.txt", value);
        git(&src, &["add", "-A"]);
        git(
            &src,
            &["commit", "--quiet", "--allow-empty", "-m", "upstream"],
        );
        git(&src, &["rev-parse", "HEAD"])
    }
}

/// The main chat's catch-up as `Host::catch_up` runs it: a `Vcs` of its
/// own on the default workspace, moving it onto trunk.
fn catch_up(project: &ProjectRepo) -> Result<(), String> {
    let vcs = tau_testing::block_on_io(tau_vcs_host::Vcs::open(
        project.workspace_dir(DEFAULT_WORKSPACE),
        Identity::default(),
    ))
    .map_err(|e| format!("open: {e}"))?;
    let trunk = project.trunk().map_err(|e| format!("trunk: {e}"))?;
    let name = project.trunk_name().map_err(|e| format!("name: {e}"))?;
    block_on(vcs.move_onto(trunk, name, true))
        .map_err(|e| format!("catch-up: {e}"))?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MainAct {
    Idle,
    /// A turn: the host's catch-up, then `main.txt` written, then the
    /// work committed (`commit_all`) or left in `@` (`end_turn`).
    Turn {
        value: Val,
        commit: bool,
    },
    /// The host's catch-up alone.
    CatchUp,
    /// `Host::land` of a chat: catch-up, landing, then the chat closes.
    Land(usize),
    /// `Host::drop_child` of a chat.
    Drop(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostAct {
    Idle,
    /// Upstream commits `up.txt` before the round, and
    /// `ProjectRepo::update` brings it in during the round.
    Update(Val),
    /// `Host::start`: a new chat forks the main chat at its latest link.
    Fork,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChatAct {
    Idle,
    /// Its file written, with no tool after it.
    Write(Val),
    /// A turn left uncommitted: its file written, then `end_turn`.
    Snapshot(Val),
    /// A turn committed: its file written, then `commit_all`.
    Commit(Val),
}

hegel::pretty_print_as_debug!(MainAct);
hegel::pretty_print_as_debug!(HostAct);
hegel::pretty_print_as_debug!(ChatAct);

/// The main chat's newest link, which a new chat forks at.
#[derive(Debug, Clone)]
enum Latest {
    /// No turn yet: a new chat starts on trunk.
    Trunk,
    /// A committed change: found by its change id.
    Change {
        change_id: String,
        commit_id: String,
    },
    /// A turn's snapshot: that very commit.
    Snapshot(String),
}

struct Chat {
    name: String,
    vcs: Vcs,
    /// Its file on disk.
    wc: Val,
    /// Its file in its newest commit.
    committed: Val,
    /// A commit has set `tau/<name>`.
    bookmarked: bool,
    /// The changes it committed, as `(change id, commit id)`.
    changes: Vec<(String, String)>,
    state: State,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Open,
    /// Landed on the main chat with its file as it committed it.
    Landed(Val),
    Dropped,
}

struct Model {
    repo: Repo,
    /// `main.txt` in the main chat's workspace.
    main_wc: Val,
    /// The main chat's own commits, as `(change id, commit id)`.
    main_changes: Vec<(String, String)>,
    latest: Latest,
    /// The main chat's newest change link, for when `latest` is a
    /// snapshot a new chat cannot fork at.
    last_change: Latest,
    /// `main.txt` in the main chat's newest commit.
    main_committed: Val,
    /// `up.txt` upstream, and upstream's newest commit.
    up: Val,
    up_head: String,
    /// Trunk moved without the main chat since its last catch-up.
    behind: bool,
    chats: Vec<Chat>,
}

/// What an action returned, for the model.
enum Outcome {
    Done,
    Committed(tau_vcs_host::Committed),
    Snapshot(tau_vcs_host::TurnSnapshot),
    Landed(tau_vcs_host::Landing),
    Forked(Vcs),
}

type Job = Box<dyn FnOnce() -> Result<Outcome, String> + Send>;

/// Runs `jobs` at once, each on its own thread, and waits for all.
fn run_together(
    jobs: Vec<(String, Job)>,
) -> Vec<(String, Result<Outcome, String>)> {
    let barrier = Arc::new(Barrier::new(jobs.len()));
    let handles: Vec<_> = jobs
        .into_iter()
        .map(|(who, job)| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                (who, job())
            })
        })
        .collect();
    handles.into_iter().map(|h| h.join().unwrap()).collect()
}

impl Model {
    fn new() -> Self {
        let repo = Repo::new();
        let up_head = repo.project.trunk().unwrap();
        Self {
            repo,
            main_wc: None,
            main_changes: Vec::new(),
            latest: Latest::Trunk,
            last_change: Latest::Trunk,
            main_committed: None,
            up: None,
            up_head,
            behind: false,
            chats: Vec::new(),
        }
    }

    fn open_chats(&self) -> Vec<usize> {
        (0..self.chats.len())
            .filter(|&c| self.chats[c].state == State::Open)
            .collect()
    }

    /// A chat that can land: open, its work committed, with a bookmark.
    fn landable(&self) -> Vec<usize> {
        self.open_chats()
            .into_iter()
            .filter(|&c| {
                let chat = &self.chats[c];
                chat.bookmarked && chat.wc == chat.committed
            })
            .collect()
    }

    /// The main chat's job for one round, as the host runs it.
    fn main_job(&self, main: MainAct) -> Option<Job> {
        let project = self.repo.project.clone();
        let trunk_name = self.repo.trunk_name.clone();
        let job: Job = match main {
            MainAct::Idle => return None,
            MainAct::Turn { value, commit } => {
                let vcs = self.repo.main.clone();
                let dir = self.repo.main_dir();
                Box::new(move || {
                    catch_up(&project)?;
                    write(&dir, "main.txt", value);
                    if commit {
                        block_on(vcs.commit_all("main turn", trunk_name))
                            .map(Outcome::Committed)
                            .map_err(|e| format!("commit: {e}"))
                    } else {
                        block_on(vcs.end_turn(trunk_name, None))
                            .map(Outcome::Snapshot)
                            .map_err(|e| format!("end_turn: {e}"))
                    }
                })
            }
            MainAct::CatchUp => {
                Box::new(move || catch_up(&project).map(|()| Outcome::Done))
            }
            MainAct::Land(c) => {
                let name = self.chats[c].name.clone();
                Box::new(move || {
                    let bookmark = format!("tau/{name}");
                    catch_up(&project)?;
                    // After the catch-up, which may restack the chat:
                    // `Host::land` reads it before, and lands a stale
                    // head (`landing_a_chat_after_an_update_makes_it_divergent`).
                    let head = project
                        .bookmark(&bookmark)
                        .map_err(|e| e.to_string())?
                        .ok_or("the chat has no bookmark")?;
                    let trunk = project.trunk().map_err(|e| e.to_string())?;
                    let parent = project
                        .add_workspace(DEFAULT_WORKSPACE, &trunk)
                        .map_err(|e| format!("open: {e}"))?;
                    let landing = block_on(parent.land(head, trunk_name, true))
                        .map_err(|e| format!("land: {e}"))?;
                    project
                        .forget_workspace(&name)
                        .map_err(|e| format!("forget: {e}"))?;
                    project
                        .remove_bookmark(&bookmark)
                        .map_err(|e| format!("remove bookmark: {e}"))?;
                    Ok(Outcome::Landed(landing))
                })
            }
            MainAct::Drop(c) => {
                let name = self.chats[c].name.clone();
                Box::new(move || {
                    let bookmark = format!("tau/{name}");
                    // As `Host::drop_child`: the main chat catches up,
                    // and keeps what its working copy stands on.
                    catch_up(&project)?;
                    let head = project
                        .bookmark(&bookmark)
                        .map_err(|e| e.to_string())?;
                    if let Some(head) = head {
                        let wc = project
                            .workspace_head(DEFAULT_WORKSPACE)
                            .map_err(|e| e.to_string())?
                            .ok_or("the main chat has no working copy")?;
                        let keep = project
                            .parent_of(&wc)
                            .map_err(|e| e.to_string())?
                            .ok_or("the main chat's @ has no parent")?;
                        project
                            .abandon_between(&keep, &head)
                            .map_err(|e| format!("abandon: {e}"))?;
                    }
                    project
                        .forget_workspace(&name)
                        .map_err(|e| format!("forget: {e}"))?;
                    project
                        .remove_bookmark(&bookmark)
                        .map_err(|e| format!("remove bookmark: {e}"))?;
                    Ok(Outcome::Done)
                })
            }
        };
        Some(job)
    }

    /// The host's job for one round.
    fn host_job(&self, host: HostAct) -> Option<Job> {
        let project = self.repo.project.clone();
        let job: Job = match host {
            HostAct::Idle => return None,
            HostAct::Update(_) => {
                let src = self.repo.src();
                Box::new(move || {
                    project
                        .update(UpdateFrom::Checkout(&src))
                        .map(|_| Outcome::Done)
                        .map_err(|e| format!("update: {e}"))
                })
            }
            HostAct::Fork => {
                let name = format!("c{}", self.chats.len());
                let latest = self.latest.clone();
                Box::new(move || {
                    let vcs = match latest {
                        Latest::Trunk => {
                            let trunk =
                                project.trunk().map_err(|e| e.to_string())?;
                            project.add_workspace(&name, &trunk)
                        }
                        Latest::Change {
                            change_id,
                            commit_id,
                        } => {
                            let now = project
                                .current([change_link(&change_id, &commit_id)])
                                .map_err(|e| format!("current: {e}"))?
                                .remove(0);
                            project.add_workspace(&name, &now.commit_id)
                        }
                        Latest::Snapshot(commit_id) => project
                            .add_workspace_from_snapshot(&name, &commit_id),
                    };
                    vcs.map(Outcome::Forked).map_err(|e| format!("fork: {e}"))
                })
            }
        };
        Some(job)
    }

    /// A chat's job for one round.
    fn chat_job(&self, c: usize, act: ChatAct) -> Option<Job> {
        let chat = &self.chats[c];
        let vcs = chat.vcs.clone();
        let dir = self.repo.project.workspace_dir(&chat.name);
        let bookmark = format!("tau/{}", chat.name);
        let file = chat_file(c);
        let job: Job = match act {
            ChatAct::Idle => return None,
            ChatAct::Write(value) => Box::new(move || {
                write(&dir, &file, value);
                Ok(Outcome::Done)
            }),
            ChatAct::Snapshot(value) => Box::new(move || {
                write(&dir, &file, value);
                block_on(vcs.end_turn(bookmark, None))
                    .map(Outcome::Snapshot)
                    .map_err(|e| format!("end_turn: {e}"))
            }),
            ChatAct::Commit(value) => Box::new(move || {
                write(&dir, &file, value);
                block_on(vcs.commit_all("chat turn", bookmark))
                    .map(Outcome::Committed)
                    .map_err(|e| format!("commit: {e}"))
            }),
        };
        Some(job)
    }

    /// Runs one round, all its actions at once, and moves the model on.
    fn round(
        &mut self,
        tc: &TestCase,
        main: MainAct,
        host: HostAct,
        chats: &[(usize, ChatAct)],
    ) {
        tc.note(&format!("round: {main:?}, {host:?}, {chats:?}"));
        if let HostAct::Update(value) = host {
            self.up_head = self.repo.push_upstream(value);
        }
        let mut jobs: Vec<(String, Job)> = Vec::new();
        if let Some(job) = self.main_job(main) {
            jobs.push(("main".to_owned(), job));
        }
        if let Some(job) = self.host_job(host) {
            jobs.push(("host".to_owned(), job));
        }
        for &(c, act) in chats {
            if let Some(job) = self.chat_job(c, act) {
                jobs.push((self.chats[c].name.clone(), job));
            }
        }
        tc.event_value("actions at once", jobs.len() as f64);
        let mut results: BTreeMap<String, Outcome> = run_together(jobs)
            .into_iter()
            .map(|(who, result)| match result {
                Ok(outcome) => (who, outcome),
                Err(error) => panic!("{who} failed: {error}"),
            })
            .collect();

        match main {
            MainAct::Idle => {}
            MainAct::Turn { value, .. } => {
                self.behind = false;
                self.main_wc = value;
                match results.remove("main").unwrap() {
                    Outcome::Committed(c) => {
                        self.main_committed = value;
                        self.latest = Latest::Change {
                            change_id: c.change_id.clone(),
                            commit_id: c.commit_id.clone(),
                        };
                        self.last_change = self.latest.clone();
                        if c.changed {
                            self.main_changes.push((c.change_id, c.commit_id));
                        }
                    }
                    // A chat forked at a snapshot that holds uncommitted
                    // work would start with the main chat's `main.txt`,
                    // and one file would have two owners. It forks at
                    // the newest change instead; `runs_model` covers
                    // forks at snapshots.
                    Outcome::Snapshot(s) if value == self.main_committed => {
                        self.latest = Latest::Snapshot(s.commit_id);
                    }
                    Outcome::Snapshot(_) => {
                        self.latest = self.last_change.clone();
                    }
                    _ => unreachable!(),
                }
            }
            MainAct::CatchUp => self.behind = false,
            MainAct::Land(c) => {
                self.behind = false;
                let chat = &mut self.chats[c];
                chat.state = State::Landed(chat.committed);
                // `Host::land` links the landed changes at the main chat's
                // latest turn, so new chats fork at the newest of them.
                let Some(Outcome::Landed(landing)) = results.remove("main")
                else {
                    unreachable!()
                };
                if let Some(head) = landing.changes.first() {
                    self.latest = Latest::Change {
                        change_id: head.change_id.clone(),
                        commit_id: head.commit_id.clone(),
                    };
                    self.last_change = self.latest.clone();
                }
            }
            MainAct::Drop(c) => {
                self.behind = false;
                self.chats[c].state = State::Dropped;
            }
        }
        match host {
            HostAct::Idle => {}
            HostAct::Update(value) => {
                self.up = value;
                // Behind unless the main chat's action went after the
                // update; which went first is not known.
                self.behind = true;
            }
            HostAct::Fork => {
                let Some(Outcome::Forked(vcs)) = results.remove("host") else {
                    unreachable!()
                };
                let c = self.chats.len();
                self.chats.push(Chat {
                    name: format!("c{c}"),
                    vcs,
                    wc: None,
                    committed: None,
                    bookmarked: false,
                    changes: Vec::new(),
                    state: State::Open,
                });
            }
        }
        for &(c, act) in chats {
            let name = self.chats[c].name.clone();
            let chat = &mut self.chats[c];
            match act {
                ChatAct::Idle => {}
                ChatAct::Write(value) | ChatAct::Snapshot(value) => {
                    chat.wc = value
                }
                ChatAct::Commit(value) => {
                    chat.wc = value;
                    chat.committed = value;
                    chat.bookmarked = true;
                    let Some(Outcome::Committed(c)) = results.remove(&name)
                    else {
                        unreachable!()
                    };
                    if c.changed {
                        chat.changes.push((c.change_id, c.commit_id));
                    }
                }
            }
        }
        self.check();
    }

    /// What every round leaves: see the module docs.
    fn check(&self) {
        let project = &self.repo.project;

        // Each run's own file is what it last wrote.
        let main_dir = self.repo.main_dir();
        assert_eq!(
            read(&main_dir, "main.txt").as_deref(),
            self.main_wc,
            "the main chat's main.txt"
        );
        for (c, chat) in self.chats.iter().enumerate() {
            let file = chat_file(c);
            let in_main = match chat.state {
                State::Landed(value) => value,
                _ => None,
            };
            assert_eq!(
                read(&main_dir, &file).as_deref(),
                in_main,
                "{file} in the main chat"
            );
            if chat.state == State::Open {
                let dir = project.workspace_dir(&chat.name);
                assert_eq!(
                    read(&dir, &file).as_deref(),
                    chat.wc,
                    "{file} in its chat"
                );
            }
        }

        // Trunk names one commit, not the root, with upstream's in it.
        let trunk = project
            .bookmark(&self.repo.trunk_name)
            .unwrap()
            .expect("trunk's bookmark names one commit");
        assert_eq!(project.trunk().unwrap(), trunk);
        assert_ne!(trunk, ROOT, "trunk is the root commit");
        assert!(
            project.is_ancestor(&self.up_head, &trunk).unwrap(),
            "trunk lacks upstream's newest commit"
        );

        // Every change a run committed is one commit; once the main chat
        // has caught up, trunk holds its commits and the landed chats'.
        let mut on_trunk = self.main_changes.clone();
        let mut open = Vec::new();
        for chat in &self.chats {
            match chat.state {
                State::Landed(_) => on_trunk.extend(chat.changes.clone()),
                State::Open => open.push(chat),
                State::Dropped => {}
            }
        }
        let now = project
            .current(on_trunk.iter().map(|(ch, co)| change_link(ch, co)))
            .expect("every committed change is one commit");
        if !self.behind {
            for link in &now {
                assert!(
                    project.is_ancestor(&link.commit_id, &trunk).unwrap(),
                    "trunk lacks {}",
                    link.change_id
                );
            }
        }
        for chat in open {
            let now = project
                .current(
                    chat.changes.iter().map(|(ch, co)| change_link(ch, co)),
                )
                .expect("every committed change is one commit");
            let bookmark =
                project.bookmark(&format!("tau/{}", chat.name)).unwrap();
            if let Some(last) = now.last() {
                assert_eq!(
                    bookmark.as_ref(),
                    Some(&last.commit_id),
                    "{}'s bookmark",
                    chat.name
                );
            }
        }
    }

    /// The main chat catches up and commits all it has: trunk's files
    /// are then the main chat's, upstream's, and the landed chats'.
    fn finish(&self) {
        let project = &self.repo.project;
        catch_up(project).unwrap();
        block_on(
            self.repo
                .main
                .commit_all("main last", self.repo.trunk_name.clone()),
        )
        .unwrap();
        let trunk = project.trunk().unwrap();
        let at = |path: &str| {
            project
                .file_at(&trunk, path)
                .unwrap()
                .map(|(bytes, _)| String::from_utf8(bytes).unwrap())
        };
        assert_eq!(at("a.txt").as_deref(), Some("one\n"));
        assert_eq!(at("main.txt").as_deref(), self.main_wc, "main.txt");
        assert_eq!(at("up.txt").as_deref(), self.up, "up.txt");
        for (c, chat) in self.chats.iter().enumerate() {
            let want = match chat.state {
                State::Landed(value) => value,
                _ => None,
            };
            assert_eq!(at(&chat_file(c)).as_deref(), want, "{}", chat_file(c));
        }
    }
}

/// Draws a round: one action for the main chat, one for the host, one
/// for each open chat the main chat's action leaves alone.
fn draw_round(
    tc: &TestCase,
    m: &Model,
) -> (MainAct, HostAct, Vec<(usize, ChatAct)>) {
    let open = m.open_chats();
    let landable = m.landable();
    let value = || tc.draw(gs::sampled_from(VALUES.to_vec()));

    let mut mains = vec![
        MainAct::Idle,
        MainAct::CatchUp,
        MainAct::Turn {
            value: value(),
            commit: tc.draw(gs::booleans()),
        },
    ];
    if !landable.is_empty() {
        // Twice: a chat must commit before it can land, and drops close
        // chats as well.
        let chat = tc.draw(gs::sampled_from(landable));
        mains.extend([MainAct::Land(chat), MainAct::Land(chat)]);
    }
    if !open.is_empty() {
        mains.push(MainAct::Drop(tc.draw(gs::sampled_from(open.clone()))));
    }
    let main = tc.draw(gs::sampled_from(mains));

    let mut hosts = vec![HostAct::Idle];
    if m.chats.len() < MAX_CHATS {
        hosts.push(HostAct::Fork);
    }
    hosts.push(HostAct::Update(value()));
    let host = tc.draw(gs::sampled_from(hosts));

    let busy = match main {
        MainAct::Land(c) | MainAct::Drop(c) => Some(c),
        _ => None,
    };
    let mut chats = Vec::new();
    for c in open {
        if Some(c) == busy {
            continue;
        }
        let acts = vec![
            ChatAct::Idle,
            ChatAct::Write(value()),
            ChatAct::Snapshot(value()),
            ChatAct::Commit(value()),
            ChatAct::Commit(value()),
        ];
        chats.push((c, tc.draw(gs::sampled_from(acts))));
    }
    (main, host, chats)
}

/// Records which races a round runs, for `HEGEL_STATISTICS=1`.
fn events(
    tc: &TestCase,
    m: &Model,
    main: MainAct,
    host: HostAct,
    chats: &[(usize, ChatAct)],
) {
    let tools = chats
        .iter()
        .filter(|(_, act)| {
            matches!(act, ChatAct::Commit(_) | ChatAct::Snapshot(_))
        })
        .count();
    let commits = chats
        .iter()
        .filter(|(_, act)| matches!(act, ChatAct::Commit(_)))
        .count();
    if commits >= 2 {
        tc.event("two chats commit at once");
    }
    if tools > 0 {
        match main {
            MainAct::Turn { .. } | MainAct::Land(_) if m.behind => tc.event(
                "a main chat turn or landing that restacks races a chat's turn",
            ),
            MainAct::Turn { commit: true, .. } => {
                tc.event("a main chat commit races a chat's turn")
            }
            MainAct::Turn { commit: false, .. } => {
                tc.event("a main chat snapshot races a chat's turn")
            }
            MainAct::CatchUp if m.behind => {
                tc.event("a catch-up that restacks races a chat's turn")
            }
            MainAct::CatchUp => tc.event("a catch-up races a chat's turn"),
            MainAct::Land(_) => tc.event("a landing races a chat's turn"),
            MainAct::Drop(_) => tc.event("a drop races a chat's turn"),
            MainAct::Idle => {}
        }
        match host {
            HostAct::Update(_) => tc.event("an update races a chat's turn"),
            HostAct::Fork => tc.event("a new chat races a chat's turn"),
            HostAct::Idle => {}
        }
    }
    match (main, host) {
        (MainAct::Drop(_), HostAct::Update(_)) => {
            tc.event("an update races a drop")
        }
        (MainAct::Turn { .. }, HostAct::Update(_)) => {
            tc.event("an update races a main chat turn")
        }
        (MainAct::CatchUp, HostAct::Update(_)) => {
            tc.event("an update races a catch-up")
        }
        (MainAct::Land(_), HostAct::Update(_)) => {
            tc.event("an update races a landing")
        }
        (MainAct::Land(_), HostAct::Fork) => {
            tc.event("a new chat races a landing")
        }
        (MainAct::Drop(_), HostAct::Fork) => {
            tc.event("a new chat races a drop")
        }
        (MainAct::Turn { .. } | MainAct::CatchUp, HostAct::Fork) => {
            tc.event(if m.behind {
                "a new chat races a catch-up that restacks"
            } else {
                "a new chat races a main chat turn"
            })
        }
        _ => {}
    }
    if m.behind && main != MainAct::Idle {
        if m.open_chats().is_empty() {
            tc.event("a catch-up after an update");
        } else {
            tc.event("a catch-up after an update restacks chats");
        }
    }
    if let MainAct::Land(c) = main
        && !m.chats[c].changes.is_empty()
    {
        tc.event("a landing with changes");
    }
}

/// Runs side by side keep every run's work, trunk and the links: see
/// the module docs. Each case starts a main chat turn and up to three
/// chats, then runs up to 8 rounds.
#[hegel::test(
    test_cases = 25,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn runs_side_by_side_behave_like_one_after_another(tc: TestCase) {
    side_by_side(tc);
}

#[hegel::test(
    profile = "nightly_slow",
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
#[ignore = "nightly"]
fn runs_side_by_side_behave_like_one_after_another_nightly(tc: TestCase) {
    side_by_side(tc);
}

fn side_by_side(tc: TestCase) {
    let mut m = Model::new();
    // A main chat turn and a few chats first, one after another, so the
    // races start with the first round.
    let value = tc.draw(gs::sampled_from(VALUES.to_vec()));
    let first = MainAct::Turn {
        value,
        commit: tc.draw(gs::booleans()),
    };
    m.round(&tc, first, HostAct::Idle, &[]);
    for _ in 0..tc.draw(gs::integers::<usize>().min_value(1).max_value(3)) {
        m.round(&tc, MainAct::Idle, HostAct::Fork, &[]);
    }
    let rounds = tc.draw(gs::integers::<usize>().min_value(1).max_value(8));
    for _ in 0..rounds {
        let (main, host, chats) = draw_round(&tc, &m);
        events(&tc, &m, main, host, &chats);
        m.round(&tc, main, host, &chats);
    }
    m.finish();
}

// The races the property found, one example each.

/// Runs `a` and `b` at once and fails on either's error.
fn race(a: (&str, Job), b: (&str, Job)) {
    let jobs = vec![(a.0.to_owned(), a.1), (b.0.to_owned(), b.1)];
    for (who, result) in run_together(jobs) {
        if let Err(error) = result {
            panic!("{who} failed: {error}");
        }
    }
}

/// An update racing the main chat's commit leaves trunk's bookmark
/// naming one commit.
///
/// Without the repository's lock, both operations started from the same
/// one: the update set trunk to upstream's commit, the commit set it to
/// the main chat's, and jj's merge of the two kept both targets.
/// `ProjectRepo::trunk` then fell back to the root commit, and the next
/// catch-up dropped upstream's commits from trunk. Under the lock one
/// goes after the other.
#[test]
fn an_update_racing_a_main_chat_commit_leaves_trunk_one_commit() {
    for _ in 0..5 {
        let repo = Repo::new();
        repo.push_upstream(Some("x\n"));
        write(&repo.main_dir(), "main.txt", Some("x\n"));
        let (project, src) = (repo.project.clone(), repo.src());
        let (main, name) = (repo.main.clone(), repo.trunk_name.clone());
        race(
            (
                "update",
                Box::new(move || {
                    project
                        .update(UpdateFrom::Checkout(&src))
                        .map(|_| Outcome::Done)
                        .map_err(|e| e.to_string())
                }),
            ),
            (
                "commit",
                Box::new(move || {
                    block_on(main.commit_all("main turn", name))
                        .map(Outcome::Committed)
                        .map_err(|e| e.to_string())
                }),
            ),
        );
        let trunk = repo.project.bookmark(&repo.trunk_name).unwrap();
        assert!(
            trunk.is_some(),
            "trunk's bookmark is conflicted, and trunk() is {}",
            repo.project.trunk().unwrap()
        );
    }
}

/// The main chat's catch-up racing a chat's turn keeps the chat's
/// work: its new file stays in its `@` and on disk.
///
/// Without the repository's lock, the catch-up (restacking the chat's
/// `@` with the main chat's commits) and the chat's `end_turn`
/// (snapshotting its new file into `@`) both rewrote `@` from the same
/// operation. After jj merged them, `@`'s change was divergent and the
/// workspace pointed at the catch-up's side, so the chat's next tool
/// checked out its files without the new one. With `commit_all`, the
/// chat's commit was divergent and could not land.
#[test]
fn a_catch_up_racing_a_chat_turn_keeps_the_chats_work() {
    for _ in 0..5 {
        let repo = Repo::new();
        write(&repo.main_dir(), "main.txt", Some("x\n"));
        let main = block_on(
            repo.main.commit_all("main turn", repo.trunk_name.clone()),
        )
        .unwrap();
        let chat = repo.project.add_workspace("c0", &main.commit_id).unwrap();
        repo.push_upstream(Some("x\n"));
        repo.project
            .update(UpdateFrom::Checkout(&repo.src()))
            .unwrap();
        let dir = repo.project.workspace_dir("c0");
        write(&dir, "chat0.txt", Some("x\n"));
        let project = repo.project.clone();
        let vcs = chat.clone();
        race(
            (
                "catch-up",
                Box::new(move || catch_up(&project).map(|()| Outcome::Done)),
            ),
            (
                "chat",
                Box::new(move || {
                    block_on(vcs.end_turn("tau/c0", None))
                        .map(Outcome::Snapshot)
                        .map_err(|e| e.to_string())
                }),
            ),
        );
        let wc = block_on(chat.working_copy()).unwrap();
        assert_eq!(wc.paths, ["chat0.txt"], "the chat's @ lost its work");
        assert_eq!(read(&dir, "chat0.txt").as_deref(), Some("x\n"));
    }
}

/// An update while the main chat's turn runs keeps upstream's commit on
/// trunk: the turn's commit, its snapshot and a landing on the main chat
/// catch it up first, and a catch-up that read trunk before the update
/// goes onto upstream's newest commit.
///
/// The host catches the main chat up before its turn (`Host::resume`),
/// but `update_repo` can run while the turn does. `commit_all` and
/// `end_turn` pointed trunk at the main chat's head whatever trunk named
/// then, so trunk moved aside and lost upstream's commit. Now they move
/// the run onto trunk first (`land::follow_bookmark`), and `move_onto`
/// goes onto the bookmark when it moved on from the commit it was given.
#[test]
fn an_update_during_a_main_chat_turn_keeps_upstream_on_trunk() {
    // The turn commits, or leaves its work in `@`.
    for commit in [true, false] {
        let repo = Repo::new();
        write(&repo.main_dir(), "main.txt", Some("x\n"));
        let up = repo.push_upstream(Some("x\n"));
        repo.project
            .update(UpdateFrom::Checkout(&repo.src()))
            .unwrap();
        let name = repo.trunk_name.clone();
        if commit {
            block_on(repo.main.commit_all("main turn", name)).unwrap();
        } else {
            block_on(repo.main.end_turn(name, None)).unwrap();
        }
        let trunk = repo.project.trunk().unwrap();
        assert!(
            repo.project.is_ancestor(&up, &trunk).unwrap(),
            "trunk lacks upstream's commit"
        );
        assert_eq!(read(&repo.main_dir(), "up.txt").as_deref(), Some("x\n"));
        assert_eq!(read(&repo.main_dir(), "main.txt").as_deref(), Some("x\n"));
    }
    // The host's catch-up read trunk before the update, and the main
    // chat had moved trunk with a commit of its own: upstream's commit is
    // beside it, not after it.
    let repo = Repo::new();
    write(&repo.main_dir(), "main.txt", Some("x\n"));
    block_on(repo.main.commit_all("main turn", repo.trunk_name.clone()))
        .unwrap();
    let before = repo.project.trunk().unwrap();
    let up = repo.push_upstream(Some("x\n"));
    repo.project
        .update(UpdateFrom::Checkout(&repo.src()))
        .unwrap();
    block_on(repo.main.move_onto(before, repo.trunk_name.clone(), true))
        .unwrap();
    let trunk = repo.project.trunk().unwrap();
    assert!(
        repo.project.is_ancestor(&up, &trunk).unwrap(),
        "the catch-up moved trunk back"
    );
}

/// Dropping a chat after an update, as `Host::drop_child` does it,
/// abandons only the chat's own commit: the main chat catches up first
/// and keeps what its working copy stands on.
///
/// The host used to keep only what trunk's bookmark had. After an update
/// that is upstream's commit, which lacks the main chat's commits under
/// the chat, so they were abandoned with the chat's, and the main chat's
/// committed work left its workspace on its next tool.
#[test]
fn dropping_a_chat_after_an_update_keeps_the_main_chats_commits() {
    let repo = Repo::new();
    write(&repo.main_dir(), "main.txt", Some("x\n"));
    let main =
        block_on(repo.main.commit_all("main turn", repo.trunk_name.clone()))
            .unwrap();
    let chat = repo.project.add_workspace("c0", &main.commit_id).unwrap();
    write(&repo.project.workspace_dir("c0"), "chat0.txt", Some("x\n"));
    block_on(chat.commit_all("chat turn", "tau/c0")).unwrap();
    repo.push_upstream(Some("x\n"));
    repo.project
        .update(UpdateFrom::Checkout(&repo.src()))
        .unwrap();
    // `Host::drop_child`.
    catch_up(&repo.project).unwrap();
    let head = repo.project.bookmark("tau/c0").unwrap().unwrap();
    let wc = repo
        .project
        .workspace_head(DEFAULT_WORKSPACE)
        .unwrap()
        .unwrap();
    let keep = repo.project.parent_of(&wc).unwrap().unwrap();
    let dropped = repo.project.abandon_between(&keep, &head).unwrap();
    assert_eq!(dropped, 1, "only the chat's own commit goes");
    block_on(repo.main.working_copy()).unwrap();
    assert_eq!(read(&repo.main_dir(), "main.txt").as_deref(), Some("x\n"));
}

/// A landing given a chat head that a catch-up rewrote since it was
/// read is refused, naming the head, and changes nothing.
///
/// `Host::landing` read the chat's head from its bookmark, then caught
/// the main chat up, which restacked the chat and moved its bookmark.
/// `Vcs::land` then took what the old head had that the main chat's new
/// head lacked: the old copies of the main chat's commit and the chat's,
/// rebased again beside the new ones, and failed with
/// `DivergentAfterLanding`, every time. The host now reads the head after
/// the catch-up, and a stale head gets a clear error.
#[test]
fn a_landing_refuses_a_head_that_was_rewritten() {
    let repo = Repo::new();
    write(&repo.main_dir(), "main.txt", Some("x\n"));
    let main =
        block_on(repo.main.commit_all("main turn", repo.trunk_name.clone()))
            .unwrap();
    let chat = repo.project.add_workspace("c0", &main.commit_id).unwrap();
    write(&repo.project.workspace_dir("c0"), "chat0.txt", Some("x\n"));
    block_on(chat.commit_all("chat turn", "tau/c0")).unwrap();
    repo.push_upstream(Some("x\n"));
    repo.project
        .update(UpdateFrom::Checkout(&repo.src()))
        .unwrap();
    let stale = repo.project.bookmark("tau/c0").unwrap().unwrap();
    catch_up(&repo.project).unwrap();
    let trunk = repo.project.trunk().unwrap();
    let parent = repo
        .project
        .add_workspace(DEFAULT_WORKSPACE, &trunk)
        .unwrap();
    let err =
        block_on(parent.land(stale.clone(), repo.trunk_name.clone(), true))
            .unwrap_err();
    assert!(
        matches!(&err, tau_vcs_host::VcsError::HiddenHead(head) if *head == stale),
        "{err}"
    );
    assert_eq!(repo.project.trunk().unwrap(), trunk, "nothing moved");
    // The head read after the catch-up lands.
    let head = repo.project.bookmark("tau/c0").unwrap().unwrap();
    let landing =
        block_on(parent.land(head, repo.trunk_name.clone(), true)).unwrap();
    assert_eq!(landing.changes.len(), 1);
}

/// The repository's lock never deadlocks: threads that each take turns
/// in their own chat, update, add and forget workspaces, drop commits and
/// remove bookmarks, all on one repository, all finish, and the main
/// chat's catch-up and commit after them see one trunk.
#[test]
fn the_repository_lock_never_deadlocks() {
    let repo = Repo::new();
    let chats: Vec<Vcs> = (0..3)
        .map(|c| {
            let trunk = repo.project.trunk().unwrap();
            repo.project
                .add_workspace(&format!("c{c}"), &trunk)
                .unwrap()
        })
        .collect();
    let (done, finished) = std::sync::mpsc::channel();
    let mut threads = Vec::new();
    for (c, vcs) in chats.into_iter().enumerate() {
        let (project, done) = (repo.project.clone(), done.clone());
        threads.push(std::thread::spawn(move || {
            let dir = project.workspace_dir(&format!("c{c}"));
            for turn in 0..10 {
                let value = ["x\n", "y\n"][turn % 2];
                write(&dir, &chat_file(c), Some(value));
                let bookmark = format!("tau/c{c}");
                if turn % 3 == 0 {
                    block_on(vcs.end_turn(bookmark, None)).unwrap();
                } else {
                    block_on(vcs.commit_all("chat turn", bookmark)).unwrap();
                }
            }
            done.send(()).unwrap();
        }));
    }
    {
        let (project, main, done) =
            (repo.project.clone(), repo.main.clone(), done.clone());
        let name = repo.trunk_name.clone();
        let dir = repo.main_dir();
        threads.push(std::thread::spawn(move || {
            for turn in 0..10 {
                catch_up(&project).unwrap();
                write(&dir, "main.txt", Some(["x\n", "y\n"][turn % 2]));
                block_on(main.commit_all("main turn", name.clone())).unwrap();
            }
            done.send(()).unwrap();
        }));
    }
    {
        let (project, src, done) =
            (repo.project.clone(), repo.src(), done.clone());
        threads.push(std::thread::spawn(move || {
            for turn in 0..10 {
                write(&src, "up.txt", Some(["x\n", "y\n"][turn % 2]));
                git(&src, &["add", "-A"]);
                git(&src, &["commit", "--quiet", "-m", "upstream"]);
                project.update(UpdateFrom::Checkout(&src)).unwrap();
                let name = format!("scratch{turn}");
                let trunk = project.trunk().unwrap();
                let vcs = project.add_workspace(&name, &trunk).unwrap();
                write(&project.workspace_dir(&name), "s.txt", Some("x\n"));
                let head =
                    block_on(vcs.commit_all("scratch", "tau/scratch")).unwrap();
                drop(vcs);
                project.forget_workspace(&name).unwrap();
                project.abandon_between(&trunk, &head.commit_id).unwrap();
                project.remove_bookmark("tau/scratch").unwrap();
            }
            done.send(()).unwrap();
        }));
    }
    drop(done);
    for _ in 0..threads.len() {
        finished
            .recv_timeout(std::time::Duration::from_secs(120))
            .expect("a thread is stuck on the repository's lock");
    }
    for thread in threads {
        thread.join().unwrap();
    }
    catch_up(&repo.project).unwrap();
    block_on(repo.main.commit_all("main last", repo.trunk_name.clone()))
        .unwrap();
    let trunk = repo.project.bookmark(&repo.trunk_name).unwrap();
    assert!(trunk.is_some(), "trunk's bookmark names one commit");
}

/// A chat started on a commit the main chat's catch-up has rewritten
/// since the host read it starts where the commit's change is now, and
/// the change stays one commit.
///
/// `Host::start` reads the main chat's latest link, resolves it with
/// `ProjectRepo::current`, then makes the chat's workspace on that commit.
/// A catch-up in between restacked it. Checking out the old commit
/// brought it back, visible beside its rewrite: the main chat's change
/// was divergent, and `ProjectRepo::current` failed on its links.
#[test]
fn a_chat_started_on_a_rewritten_commit_starts_where_it_is_now() {
    let repo = Repo::new();
    write(&repo.main_dir(), "main.txt", Some("x\n"));
    let main =
        block_on(repo.main.commit_all("main turn", repo.trunk_name.clone()))
            .unwrap();
    repo.push_upstream(Some("x\n"));
    repo.project
        .update(UpdateFrom::Checkout(&repo.src()))
        .unwrap();
    catch_up(&repo.project).unwrap();
    repo.project.add_workspace("c0", &main.commit_id).unwrap();
    let now = repo
        .project
        .current([change_link(&main.change_id, &main.commit_id)])
        .expect("the main chat's change is one commit")
        .remove(0);
    assert_ne!(now.commit_id, main.commit_id, "the catch-up rewrote it");
    let wc = repo.project.workspace_head("c0").unwrap().unwrap();
    assert_eq!(repo.project.parent_of(&wc).unwrap(), Some(now.commit_id));
}

/// A chat whose `@` another program rewrote at the same time as the
/// chat's own snapshot, so that jj merged the two into a divergent `@`,
/// fails with `VcsError::Stale`, and keeps its files on disk.
///
/// tau-vcs's own writes take the repository's lock and never fork the
/// operation log. Another process, as a `jj` command run by hand, can:
/// here a transaction started before the chat's `end_turn` rewrites the
/// chat's `@` after it. Either side of the divergent change may hold
/// the chat's work, so the tools stop rather than check out one of them.
#[test]
fn a_divergent_working_copy_is_stale() {
    use jj_lib::{
        config::{ConfigLayer, ConfigSource, StackedConfig},
        default_backend_factories::{
            default_backend_factories,
            default_working_copy_factories,
        },
        ref_name::WorkspaceNameBuf,
        repo::Repo as _,
        settings::UserSettings,
        workspace::Workspace,
    };
    let repo = Repo::new();
    let trunk = repo.project.trunk().unwrap();
    let chat = repo.project.add_workspace("c0", &trunk).unwrap();
    let dir = repo.project.workspace_dir("c0");
    write(&dir, "chat0.txt", Some("x\n"));

    let mut config = StackedConfig::with_defaults();
    let mut user = ConfigLayer::empty(ConfigSource::User);
    user.set_value("user.name", "other").unwrap();
    user.set_value("user.email", "other@localhost").unwrap();
    config.add_layer(user);
    let settings = UserSettings::from_config(config).unwrap();
    let workspace = Workspace::load(
        &settings,
        &repo.main_dir(),
        &default_backend_factories(),
        &default_working_copy_factories(),
    )
    .unwrap();
    // The other program's operation starts here.
    let before =
        pollster::block_on(workspace.repo_loader().load_at_head()).unwrap();

    block_on(chat.end_turn("tau/c0", None)).unwrap();

    let id = before
        .view()
        .get_wc_commit_id(&WorkspaceNameBuf::from("c0"))
        .unwrap()
        .clone();
    let wc = before.store().get_commit(&id).unwrap();
    let mut tx = before.start_transaction();
    pollster::block_on(
        tx.repo_mut()
            .rewrite_commit(&wc)
            .set_description("described elsewhere")
            .write(),
    )
    .unwrap();
    pollster::block_on(tx.repo_mut().rebase_descendants()).unwrap();
    pollster::block_on(tx.commit("other program")).unwrap();

    let err = block_on(chat.working_copy()).unwrap_err();
    assert!(matches!(err, tau_vcs_host::VcsError::Stale), "{err}");
    assert_eq!(read(&dir, "chat0.txt").as_deref(), Some("x\n"));
}
