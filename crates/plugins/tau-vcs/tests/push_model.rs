//! Pushing a project's work to its remote (`docs/reference/vcs.md`,
//! "Pushing", ADR 0023), against a bare repository reached through a
//! `file://` URL, as GitHub would be.
//!
//! - The main chat's commits on trunk that the remote lacks are what
//!   `unpushed` counts, and a push takes exactly those, as they are:
//!   the remote gets the very commits, and a project made from the
//!   remote reads the same change ids. A push after the remote moved is
//!   refused and changes nothing, until a fetch and a catch-up.
//! - A chat's commits replay onto the remote's trunk by a three-way
//!   merge of each commit, which a model of single-line files predicts,
//!   conflicts included; a chat that keeps off the files the main chat
//!   has not pushed never conflicts, and its copies push as a branch.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use hegel::{Generator as _, TestCase, generators as gs};
use tau_testing::{block_on, git::git};
use tau_vcs::{
    DEFAULT_WORKSPACE,
    Identity,
    Link,
    ProjectRepo,
    Remote,
    UpdateFrom,
    Vcs,
    VcsError,
    clone_bare,
};

/// A bare repository standing for GitHub, a checkout that pushes to it
/// as someone else would, and a project cloned from it whose main chat
/// has caught up with trunk.
struct Origin {
    home: tempfile::TempDir,
    work: PathBuf,
    bare: PathBuf,
    url: String,
    project: ProjectRepo,
    main: Vcs,
}

/// Files by path, each one line; absent paths are not in the tree.
type Files = BTreeMap<String, String>;

impl Origin {
    /// The remote's `main` starts with one commit holding `files`.
    fn new(files: &Files) -> Self {
        let home = tempfile::tempdir().unwrap();
        let bare = home.path().join("origin.git");
        std::fs::create_dir_all(&bare).unwrap();
        git(&bare, &["init", "--quiet", "--bare"]);
        let work = home.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "--quiet"]);
        write_files(&work, files);
        git(&work, &["add", "-A"]);
        git(
            &work,
            &["commit", "--quiet", "--allow-empty", "-m", "first"],
        );
        git(&work, &["remote", "add", "origin", bare.to_str().unwrap()]);
        git(&work, &["push", "--quiet", "origin", "main"]);
        let url = format!("file://{}", bare.display());
        let clone = home.path().join("clone.git");
        clone_bare(&url, None, &clone).unwrap();
        let project = tau_vcs::ProjectRepo::import(
            clone.to_str().unwrap(),
            home.path().join("p"),
            Identity::default(),
        )
        .unwrap();
        let main = tau_testing::block_on_io(tau_vcs::Vcs::open(
            project.workspace_dir(DEFAULT_WORKSPACE),
            Identity::default(),
        ))
        .unwrap();
        let origin = Self {
            home,
            work,
            bare,
            url,
            project,
            main,
        };
        origin.catch_up();
        origin
    }

    fn remote(&self) -> Remote<'_> {
        Remote {
            url: &self.url,
            token: None,
        }
    }

    /// The remote's `main`.
    fn origin_main(&self) -> String {
        git(&self.bare, &["rev-parse", "refs/heads/main"])
    }

    /// Someone else pushes a commit writing `file` to the remote.
    fn upstream_commit(&self, file: &str) {
        git(&self.work, &["fetch", "--quiet", "origin"]);
        git(&self.work, &["reset", "--quiet", "--hard", "origin/main"]);
        std::fs::write(self.work.join(file), "theirs\n").unwrap();
        git(&self.work, &["add", "-A"]);
        git(&self.work, &["commit", "--quiet", "-m", "upstream"]);
        git(&self.work, &["push", "--quiet", "origin", "HEAD:main"]);
    }

    /// The main chat's workspace onto trunk's head, as the host does
    /// before each of its turns.
    fn catch_up(&self) {
        block_on(self.main.move_onto(
            self.project.trunk().unwrap(),
            self.project.trunk_name().unwrap(),
            true,
        ))
        .unwrap();
    }

    /// The main chat makes `edits` in its workspace and commits them on
    /// trunk. Returns the commit's change id, if it made one.
    fn main_commit(&self, edits: &Files, removed: &[String]) -> Option<String> {
        let dir = self.project.workspace_dir(DEFAULT_WORKSPACE);
        edit(&dir, edits, removed);
        let committed =
            block_on(self.main.commit_all("main's change", "main")).unwrap();
        committed.changed.then_some(committed.change_id)
    }

    /// Where the change `change_id` is now in `project`.
    fn now(project: &ProjectRepo, change_id: &str) -> String {
        let link = Link {
            turn: 0,
            workspace: String::new(),
            commit_id: String::new(),
            change_id: change_id.to_owned(),
            changed: false,
            from: None,
            snapshot: false,
        };
        project.current([link]).unwrap().remove(0).commit_id
    }

    /// The change ids `unpushed` lists, oldest first.
    fn unpushed(&self) -> Vec<String> {
        self.project
            .unpushed()
            .unwrap()
            .into_iter()
            .map(|change| change.change_id)
            .collect()
    }
}

fn write_files(dir: &Path, files: &Files) {
    for (path, text) in files {
        std::fs::write(dir.join(path), text).unwrap();
    }
}

/// Writes `edits` in `dir` and deletes `removed`.
fn edit(dir: &Path, edits: &Files, removed: &[String]) {
    write_files(dir, edits);
    for path in removed {
        let _ = std::fs::remove_file(dir.join(path));
    }
}

/// The files `commit` holds among `paths`, from the project's Git store.
fn files_at(project: &ProjectRepo, commit: &str, paths: &[String]) -> Files {
    paths
        .iter()
        .filter_map(|path| {
            let (bytes, _) = project.file_at(commit, path).unwrap()?;
            Some((path.clone(), String::from_utf8(bytes).unwrap()))
        })
        .collect()
}

/// The `change-id` header of a commit in the Git store at `git_dir`.
fn change_header(git_dir: &Path, commit: &str) -> Option<String> {
    git(git_dir, &["cat-file", "commit", commit])
        .lines()
        .take_while(|line| !line.is_empty())
        .find_map(|line| line.strip_prefix("change-id ").map(str::to_owned))
}

/// The main chat's pushes against a remote that others push to too.
struct Machine {
    origin: Origin,
    /// The main chat's commits the remote lacks, oldest first, by
    /// change id.
    unpushed: Vec<String>,
    /// The remote moved since the project last fetched.
    moved: bool,
    /// Files written, for distinct names.
    files: usize,
}

#[hegel::state_machine]
impl Machine {
    /// The main chat's turn: it commits a file of its own on trunk.
    #[rule(weight = 3)]
    fn main_commit(&mut self, tc: TestCase) {
        self.files += 1;
        let file = format!("m{}.txt", self.files);
        let edits = Files::from([(file, "ours\n".to_owned())]);
        let change = self.origin.main_commit(&edits, &[]).unwrap();
        self.unpushed.push(change);
        tc.event("the main chat commits");
    }

    /// Someone else pushes to the remote's `main`.
    #[rule]
    fn upstream(&mut self, tc: TestCase) {
        self.files += 1;
        self.origin.upstream_commit(&format!("u{}.txt", self.files));
        self.moved = true;
        tc.event("upstream moves");
    }

    /// An update from the remote, then the main chat's catch-up: its
    /// commits go on top of upstream's, keeping their change ids, and
    /// none of upstream's counts as its.
    #[rule]
    fn fetch(&mut self, _tc: TestCase) {
        let origin = &self.origin;
        origin
            .project
            .update(UpdateFrom::Remote {
                url: &origin.url,
                token: None,
            })
            .unwrap();
        origin.catch_up();
        self.moved = false;
    }

    #[rule(weight = 2)]
    fn push(&mut self, tc: TestCase) {
        let origin = &self.origin;
        let before = origin.origin_main();
        let pushed = origin.project.push_trunk(origin.remote());
        if self.unpushed.is_empty() {
            let pushed = pushed.unwrap();
            assert!(pushed.changes.is_empty(), "{pushed:?}");
            assert_eq!(origin.origin_main(), before, "nothing to push");
            tc.event("nothing to push");
            return;
        }
        if self.moved {
            let error = pushed.unwrap_err();
            assert!(
                matches!(&error, VcsError::PushRejected { branch } if branch == "main"),
                "{error}"
            );
            assert_eq!(origin.origin_main(), before, "a refused push");
            tc.event("the remote moved");
            return;
        }
        let pushed = pushed.unwrap();
        let ids: Vec<String> = pushed
            .changes
            .iter()
            .map(|change| change.change_id.clone())
            .collect();
        assert_eq!(ids, self.unpushed, "what was pushed");
        assert_eq!(pushed.from.as_deref(), Some(before.as_str()));
        let trunk = origin.project.trunk().unwrap();
        assert_eq!(pushed.to, trunk);
        // The remote has trunk's very commits: its branch is trunk's
        // commit, and every pushed commit is in its history.
        assert_eq!(origin.origin_main(), trunk);
        for change in &pushed.changes {
            let is_ancestor = tau_testing::git::output(
                &origin.bare,
                &["merge-base", "--is-ancestor", &change.commit_id, &trunk],
            );
            assert!(is_ancestor.status.success(), "{change:?}");
        }
        // A project made from the remote reads the same change ids, at
        // the same commits.
        let fresh = tau_vcs::ProjectRepo::import(
            origin.bare.to_str().unwrap(),
            origin.home.path().join(format!("fresh-{}", self.files)),
            Identity::default(),
        )
        .unwrap();
        for change in &pushed.changes {
            assert_eq!(
                Origin::now(&fresh, &change.change_id),
                change.commit_id,
                "change {} after a round trip",
                change.change_id
            );
        }
        self.unpushed.clear();
        tc.event("pushed");
    }

    #[invariant(always_run)]
    fn unpushed_is_the_main_chats_own(&self, tc: TestCase) {
        tc.event_value("unpushed", self.unpushed.len() as f64);
        assert_eq!(self.origin.unpushed(), self.unpushed, "unpushed");
    }
}

/// The main chat commits, others push, the project fetches and pushes,
/// in any order: `unpushed` is always the main chat's commits the remote
/// lacks, a push takes exactly those with their ids, and a push after
/// the remote moved is refused.
#[hegel::test(
    test_cases = 30,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn pushes_take_the_main_chats_commits_as_they_are(tc: TestCase) {
    let machine = Machine {
        origin: Origin::new(&Files::from([("a.txt".into(), "a\n".into())])),
        unpushed: Vec::new(),
        moved: false,
        files: 0,
    };
    hegel::stateful::machine(machine).steps(12).run(tc);
}

/// The paths replay tests use.
const PATHS: [&str; 4] = ["p0", "p1", "p2", "p3"];

/// A commit's edits: for some paths, a new line or a removal.
type Edits = BTreeMap<String, Option<String>>;

/// One line of a file.
fn line() -> impl hegel::PrintableGenerator<String> {
    gs::sampled_from(vec!["a\n", "b\n", "c\n"])
        .map(String::from)
        .print_as_debug()
}

#[hegel::composite]
fn edits(tc: &TestCase, paths: Vec<String>) -> Edits {
    let mut edits = Edits::new();
    for path in paths {
        if tc.draw(gs::booleans()) {
            edits.insert(path, tc.draw(gs::optional(line())));
        }
    }
    edits
}

#[hegel::composite]
fn files(tc: &TestCase, paths: Vec<String>) -> Files {
    let mut files = Files::new();
    for path in paths {
        if let Some(text) = tc.draw(gs::optional(line())) {
            files.insert(path, text);
        }
    }
    files
}

fn apply(files: &Files, edits: &Edits) -> Files {
    let mut after = files.clone();
    for (path, text) in edits {
        match text {
            Some(text) => after.insert(path.clone(), text.clone()),
            None => after.remove(path),
        };
    }
    after
}

/// Writes `edits` into `dir`.
fn write_edits(dir: &Path, edits: &Edits) {
    let written: Files = edits
        .iter()
        .filter_map(|(path, text)| Some((path.clone(), text.clone()?)))
        .collect();
    let removed: Vec<String> = edits
        .iter()
        .filter(|(_, text)| text.is_none())
        .map(|(path, _)| path.clone())
        .collect();
    edit(dir, &written, &removed);
}

/// A chat on trunk makes one commit per `edits`, where they change
/// something. Returns the files after each commit it made.
fn chat_commits(origin: &Origin, start: &Files, edits: &[Edits]) -> Vec<Files> {
    let trunk = origin.project.trunk().unwrap();
    let chat = origin.project.add_workspace("chat", &trunk).unwrap();
    let mut files = start.clone();
    let mut made = Vec::new();
    for (n, edits) in edits.iter().enumerate() {
        write_edits(chat.root(), edits);
        let committed =
            block_on(chat.commit_all(&format!("chat {n}"), "tau/chat"))
                .unwrap();
        let after = apply(&files, edits);
        assert_eq!(committed.changed, after != files);
        if committed.changed {
            made.push(after.clone());
        }
        files = after;
    }
    made
}

/// The model of one replayed path: the remote's line `ours`, the
/// commit's parent's `base`, the commit's `theirs`. `Err` is a conflict.
fn merge(
    ours: Option<&String>,
    base: Option<&String>,
    theirs: Option<&String>,
) -> Result<Option<String>, ()> {
    if base == theirs {
        Ok(ours.cloned())
    } else if ours == base || ours == theirs {
        Ok(theirs.cloned())
    } else {
        Err(())
    }
}

/// The main chat's unpushed commits, then a chat's commits on them,
/// replayed onto the remote's `main`: each copy is the three-way merge
/// of its commit's change onto the copy before, path by path, as the
/// model computes it for one-line files; the first copy that would
/// conflict refuses the replay with its paths. Copies keep their
/// commit's description, stack on each other from the remote's head,
/// and get change ids of their own.
#[hegel::test(
    test_cases = 40,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn a_replay_is_the_three_way_merge_of_each_commit(tc: TestCase) {
    let paths: Vec<String> = PATHS.map(String::from).to_vec();
    let base = tc.draw(files(paths.clone()));
    let origin = Origin::new(&base);
    let mains = tc.draw(gs::vecs(edits(paths.clone())).max_size(2));
    let mut main_files = base.clone();
    for edits in &mains {
        write_edits(&origin.project.workspace_dir(DEFAULT_WORKSPACE), edits);
        block_on(origin.main.commit_all("main's change", "main")).unwrap();
        main_files = apply(&main_files, edits);
    }
    let chats = tc.draw(gs::vecs(edits(paths.clone())).min_size(1).max_size(3));
    let made = chat_commits(&origin, &main_files, &chats);
    let head = origin.project.bookmark("tau/chat").unwrap();
    let stack = match &head {
        Some(head) => origin.project.stack(head).unwrap(),
        None => Vec::new(),
    };
    assert_eq!(stack.len(), made.len());
    let onto = origin.project.upstream().unwrap().unwrap();
    assert_eq!(onto, origin.origin_main());
    let ids: Vec<String> = stack
        .iter()
        .map(|change| change.commit_id.clone())
        .collect();
    let replayed = origin.project.replay(&ids, &onto);

    // The model, commit by commit.
    let mut want = Vec::new();
    let mut ours = base.clone();
    let mut parent = main_files.clone();
    let mut conflict = None;
    for theirs in &made {
        let mut next = Files::new();
        let mut conflicted = Vec::new();
        for path in &paths {
            match merge(ours.get(path), parent.get(path), theirs.get(path)) {
                Ok(Some(text)) => {
                    next.insert(path.clone(), text);
                }
                Ok(None) => {}
                Err(()) => conflicted.push(path.clone()),
            }
        }
        if !conflicted.is_empty() {
            conflict = Some(conflicted);
            break;
        }
        want.push(next.clone());
        ours = next;
        parent = theirs.clone();
    }

    match (replayed, conflict) {
        (Err(VcsError::WouldConflict(got)), Some(paths)) => {
            tc.event("would conflict");
            assert_eq!(got, paths);
        }
        (Ok(copies), None) => {
            tc.event_value("copies", copies.len() as f64);
            assert_eq!(copies.len(), want.len());
            let store = origin.project.root().join("git");
            let mut previous = onto.clone();
            for ((copy, files), change) in copies.iter().zip(&want).zip(&stack)
            {
                assert_eq!(&files_at(&origin.project, copy, &paths), files);
                assert_eq!(
                    origin.project.parent_of(copy).unwrap().as_deref(),
                    Some(previous.as_str())
                );
                let message = git(&store, &["log", "-1", "--format=%B", copy]);
                assert_eq!(message, change.description.trim_end());
                let own = change_header(&store, copy);
                assert!(own.is_some(), "a copy has a change id");
                assert_ne!(own, change_header(&store, &change.commit_id));
                previous = copy.clone();
            }
        }
        (got, want) => panic!("replayed {got:?}, the model says {want:?}"),
    }
}

/// A chat that keeps off the files the main chat has not pushed never
/// conflicts on the remote's `main`, whatever both do: its copies have
/// the remote's files with the chat's on top, and none of the main
/// chat's unpushed work. They push as a branch of their own, which the
/// project does not take in; pushing over it needs its last commit.
#[hegel::test(
    test_cases = 30,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn a_chat_apart_from_mains_unpushed_files_never_conflicts(tc: TestCase) {
    let ours: Vec<String> = ["m0", "m1"].map(String::from).to_vec();
    let theirs: Vec<String> = ["c0", "c1"].map(String::from).to_vec();
    let all: Vec<String> = ours.iter().chain(&theirs).cloned().collect();
    let base = tc.draw(files(all.clone()));
    let origin = Origin::new(&base);
    let mut main_files = base.clone();
    for edits in tc.draw(gs::vecs(edits(ours.clone())).max_size(2)) {
        write_edits(&origin.project.workspace_dir(DEFAULT_WORKSPACE), &edits);
        block_on(origin.main.commit_all("main's change", "main")).unwrap();
        main_files = apply(&main_files, &edits);
    }
    let chats =
        tc.draw(gs::vecs(edits(theirs.clone())).min_size(1).max_size(3));
    let made = chat_commits(&origin, &main_files, &chats);
    if made.is_empty() {
        return;
    }
    let head = origin.project.bookmark("tau/chat").unwrap().unwrap();
    let ids: Vec<String> = origin
        .project
        .stack(&head)
        .unwrap()
        .into_iter()
        .map(|change| change.commit_id)
        .collect();
    let onto = origin.project.upstream().unwrap().unwrap();
    let copies = origin.project.replay(&ids, &onto).unwrap();
    let last = copies.last().unwrap();
    let chat_files = made.last().unwrap();
    let mut want = base.clone();
    want.retain(|path, _| ours.contains(path));
    want.extend(
        chat_files
            .iter()
            .filter(|(path, _)| theirs.contains(path))
            .map(|(path, text)| (path.clone(), text.clone())),
    );
    assert_eq!(files_at(&origin.project, last, &all), want);

    // The copies push as a branch; the project's bookmarks and what it
    // has not pushed stay as they were.
    let bookmarks = origin.project.bookmarks("").unwrap();
    let unpushed = origin.unpushed();
    let branch = "tau/pr/chat";
    origin
        .project
        .push_branch(origin.remote(), branch, None, last)
        .unwrap();
    let pushed = git(
        &origin.bare,
        &["rev-parse", &format!("refs/heads/{branch}")],
    );
    assert_eq!(&pushed, last);
    assert_eq!(origin.project.bookmarks("").unwrap(), bookmarks);
    assert_eq!(origin.unpushed(), unpushed);
    // Pushing as if the branch were new is refused; from its commit, it
    // goes, even back to where the copies started.
    let error = origin
        .project
        .push_branch(origin.remote(), branch, None, &onto)
        .unwrap_err();
    assert!(matches!(error, VcsError::PushRejected { .. }), "{error}");
    origin
        .project
        .push_branch(origin.remote(), branch, Some(last), &onto)
        .unwrap();
    assert_eq!(
        git(
            &origin.bare,
            &["rev-parse", &format!("refs/heads/{branch}")]
        ),
        onto
    );
}

/// A push that finds a conflict in trunk's commits refuses, naming the
/// paths, and pushes nothing.
#[test]
fn a_conflicted_trunk_is_not_pushed() {
    let origin = Origin::new(&Files::from([("f".into(), "0\n".into())]));
    origin.main_commit(&Files::from([("f".into(), "ours\n".into())]), &[]);
    origin.upstream_commit("f");
    origin
        .project
        .update(UpdateFrom::Remote {
            url: &origin.url,
            token: None,
        })
        .unwrap();
    origin.catch_up();
    let before = origin.origin_main();
    let error = origin.project.push_trunk(origin.remote()).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Trunk's changes hold conflicts in f. Resolve them in the main chat \
         before pushing."
    );
    assert_eq!(origin.origin_main(), before);
}

/// A remote that refuses the push for its own reasons, as a protected
/// branch does, is told apart from one that moved.
#[test]
fn a_refused_push_says_why() {
    let origin = Origin::new(&Files::new());
    origin.main_commit(&Files::from([("f".into(), "ours\n".into())]), &[]);
    // The remote's own hooks, whatever the person's settings say.
    let hooks = origin.bare.join("hooks");
    git(
        &origin.bare,
        &["config", "core.hooksPath", hooks.to_str().unwrap()],
    );
    let hook = hooks.join("pre-receive");
    std::fs::write(&hook, "#!/bin/sh\necho protected >&2\nexit 1\n").unwrap();
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))
        .unwrap();
    let error = origin.project.push_trunk(origin.remote()).unwrap_err();
    assert!(
        matches!(&error, VcsError::PushRefused { branch, .. } if branch == "main"),
        "{error}"
    );
    assert_eq!(origin.unpushed().len(), 1, "nothing changed");
}

/// The rejection's words, which the host shows.
#[test]
fn a_rejected_push_says_to_fetch() {
    let origin = Origin::new(&Files::new());
    origin.main_commit(&Files::from([("f".into(), "ours\n".into())]), &[]);
    origin.upstream_commit("g");
    let error = origin.project.push_trunk(origin.remote()).unwrap_err();
    assert_eq!(
        error.to_string(),
        "GitHub's main moved since the last fetch: nothing was pushed. \
         Fetch, then push again."
    );
    // Fetching and catching up puts the main chat's commit on top, and
    // the push goes.
    let before = origin.unpushed();
    origin
        .project
        .update(UpdateFrom::Remote {
            url: &origin.url,
            token: None,
        })
        .unwrap();
    origin.catch_up();
    assert_eq!(origin.unpushed(), before, "the same change, restacked");
    let pushed = origin.project.push_trunk(origin.remote()).unwrap();
    assert_eq!(pushed.changes.len(), 1);
    assert_eq!(origin.origin_main(), origin.project.trunk().unwrap());
    assert!(origin.unpushed().is_empty());
}
