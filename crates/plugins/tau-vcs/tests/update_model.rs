//! Updating a project while its main chat commits on trunk
//! (`docs/reference/vcs.md`, "Projects" and "A repository's main chat").
//!
//! A Git checkout is changed by drawn `git` commands: commits, amends,
//! resets, branches made, moved, renamed and deleted, a new `HEAD`,
//! tags, packed refs and `git gc`. The project is made from it as a
//! checkout, a linked worktree, a bare repository or a remote reached
//! through a `file://` URL, and is updated from it again and again. The
//! main chat commits on trunk and catches up between updates, and chats
//! sit on the trunk as it was.
//!
//! The oracles are `git`, for the source's branches, tags and `HEAD`,
//! and a model of what the main chat moved: a bookmark it moved stays
//! where it put it until upstream moves it too, and trunk then takes
//! upstream's.
//!

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
    UpdateFrom,
    Vcs,
    clone_bare,
};

const BRANCHES: [&str; 5] = ["main", "master", "trunk", "feature", "feat/x"];
const TAGS: [&str; 2] = ["v1", "v2"];
/// jj's root commit, in a Git-backed repository.
const ROOT: &str = "0000000000000000000000000000000000000000";

/// The refs under `prefix` in the repository at `dir`, by short name,
/// each with the object it names (a tag's own object, unpeeled), as
/// `git for-each-ref` lists them. Read in process: the checks ask after
/// every step, and a `git` process each time was a third of the run.
fn refs(dir: &Path, prefix: &str) -> BTreeMap<String, String> {
    let repo = gix::open(dir).unwrap();
    let references = repo.references().unwrap();
    references
        .prefixed(prefix)
        .unwrap()
        .map(|reference| {
            let reference = reference.unwrap();
            let name = reference.name().as_bstr().to_string();
            let id = match reference.target().try_id() {
                Some(id) => id.to_owned(),
                None => reference.into_fully_peeled_id().unwrap().detach(),
            };
            (
                name.strip_prefix(prefix).unwrap().to_owned(),
                id.to_string(),
            )
        })
        .collect()
}

/// Whether `ancestor` is `of` or an ancestor of it in the repository at
/// `dir`, as `git merge-base --is-ancestor` says; one that is not there
/// is not.
fn is_ancestor(dir: &Path, ancestor: &str, of: &str) -> bool {
    let repo = gix::open(dir).unwrap();
    let (Ok(ancestor), Ok(of)) = (
        gix::ObjectId::from_hex(ancestor.as_bytes()),
        gix::ObjectId::from_hex(of.as_bytes()),
    ) else {
        return false;
    };
    let Ok(walk) = repo.rev_walk([of]).all() else {
        return false;
    };
    walk.filter_map(Result::ok)
        .any(|commit| commit.id == ancestor)
}

/// The branch `HEAD` names in the Git directory `git_dir`, if any.
fn head_branch(git_dir: &Path) -> Option<String> {
    std::fs::read_to_string(git_dir.join("HEAD"))
        .unwrap()
        .trim()
        .strip_prefix("ref: refs/heads/")
        .map(str::to_owned)
}

/// Where the project comes from.
#[derive(Debug, Clone, Copy, PartialEq, hegel::PrettyPrintable)]
enum Kind {
    /// The checkout itself, with a `core.worktree` key in its config.
    Checkout,
    /// A linked worktree of the checkout, on a detached `HEAD`.
    Worktree,
    /// A bare repository the checkout pushes to.
    Bare,
    /// A clone of that bare repository, updated over a `file://` URL.
    Remote,
}

/// Something done to the checkout.
#[derive(Debug, Clone, hegel::PrettyPrintable)]
enum Op {
    /// A commit on whatever `HEAD` is, rewriting `f.txt`.
    Commit,
    /// `HEAD`'s commit replaced by another on the same parent.
    Amend,
    /// `HEAD` moved back by one commit.
    Reset,
    /// A branch at `HEAD`.
    Branch(String),
    Checkout(String),
    Detach,
    Delete(String),
    /// `git branch -m`: `HEAD` follows when it names the branch.
    Rename(String, String),
    /// A branch moved to another's commit.
    Move(String, String),
    Tag(String, bool),
    DeleteTag(String),
    PackRefs,
    /// `git gc` that prunes what no ref reaches.
    Gc,
    /// Every object in one new pack with a multi-pack index, and the
    /// loose ones and old packs gone.
    Repack,
}

#[hegel::composite]
fn op(tc: &TestCase) -> Op {
    let branch = || gs::sampled_from(BRANCHES.to_vec()).map(String::from);
    let tag = || gs::sampled_from(TAGS.to_vec()).map(String::from);
    match tc.draw(gs::integers::<u8>().max_value(15)) {
        0..=2 => Op::Commit,
        3 => Op::Amend,
        4 => Op::Reset,
        5 | 6 => Op::Branch(tc.draw(branch())),
        7 => Op::Checkout(tc.draw(branch())),
        8 => Op::Detach,
        9 | 10 => Op::Delete(tc.draw(branch())),
        11 => Op::Rename(tc.draw(branch()), tc.draw(branch())),
        12 => Op::Move(tc.draw(branch()), tc.draw(branch())),
        13 => Op::Tag(tc.draw(tag()), tc.draw(gs::booleans())),
        14 => Op::DeleteTag(tc.draw(tag())),
        _ => tc.draw(gs::sampled_from(vec![Op::PackRefs, Op::Gc, Op::Repack])),
    }
}

/// One of the main chat's own commits.
#[derive(Debug, Clone)]
struct Own {
    change_id: String,
    file: String,
}

/// A chat, standing on trunk as it was when it started.
#[derive(Debug, Clone)]
struct Chat {
    name: String,
    commit_id: String,
    change_id: String,
    wc: String,
    file: String,
}

struct Machine {
    home: tempfile::TempDir,
    kind: Kind,
    /// The checkout the drawn commands change.
    work: PathBuf,
    project: ProjectRepo,
    main: Vcs,
    /// Commits made in the checkout, for distinct contents.
    commits: usize,
    /// The source's branches as the last import or update saw them.
    imported: BTreeMap<String, String>,
    /// The branch trunk follows, as the copy's `HEAD` names it: the
    /// source's at the import, and a bare source's or a remote's after
    /// each update. A checkout's `HEAD` is its checked-out branch, which
    /// an update leaves alone.
    head: Option<String>,
    /// Bookmarks the main chat moved since upstream last moved them:
    /// where it put them.
    local: BTreeMap<String, String>,
    /// The main chat's own commits, oldest first. None is pushed, so
    /// upstream never has them.
    own: Vec<Own>,
    /// The upstream commit the main chat's commits (or its `@`) stand
    /// on; the root commit before its first catch-up.
    base: String,
    /// Trunk, as the project should report it.
    trunk: String,
    chats: Vec<Chat>,
    /// The checkout changed since the last update.
    dirty: bool,
}

impl Machine {
    fn new(tc: &TestCase) -> Self {
        let kind = tc.draw(gs::sampled_from(vec![
            Kind::Checkout,
            Kind::Worktree,
            Kind::Bare,
            Kind::Remote,
        ]));
        tc.event(match kind {
            Kind::Checkout => "from a checkout",
            Kind::Worktree => "from a linked worktree",
            Kind::Bare => "from a bare repository",
            Kind::Remote => "from a remote",
        });
        let initial = tc.draw(gs::sampled_from(BRANCHES.to_vec()));
        Self::from_source(kind, initial)
    }

    /// A project made from a source of `kind`, whose checkout has one
    /// commit on branch `initial`.
    fn from_source(kind: Kind, initial: &str) -> Self {
        let home = tempfile::tempdir().unwrap();
        let work = home.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "--quiet", "-b", initial]);
        std::fs::write(work.join("f.txt"), "0\n").unwrap();
        git(&work, &["add", "-A"]);
        git(&work, &["commit", "--quiet", "-m", "first"]);
        let source = match kind {
            Kind::Checkout => {
                let path = work.to_str().unwrap();
                git(&work, &["config", "core.worktree", path]);
                work.clone()
            }
            Kind::Worktree => {
                git(
                    &work,
                    &["worktree", "add", "--quiet", "--detach", "../wt"],
                );
                home.path().join("wt")
            }
            Kind::Bare | Kind::Remote => {
                let bare = home.path().join("bare.git");
                std::fs::create_dir_all(&bare).unwrap();
                git(&bare, &["init", "--quiet", "--bare"]);
                // The checkout's pushes may delete the branch `HEAD`
                // names; the next push sets `HEAD` again.
                git(&bare, &["config", "receive.denyDeleteCurrent", "ignore"]);
                home.path().join("bare.git")
            }
        };
        publish(kind, &work, &home.path().join("bare.git"));
        let from = match kind {
            Kind::Remote => {
                let clone = home.path().join("clone.git");
                clone_bare(&url(home.path()), None, &clone).unwrap();
                clone
            }
            _ => source,
        };
        let project = tau_vcs::ProjectRepo::import(
            from.to_str().unwrap(),
            home.path().join("p"),
            Identity::default(),
        )
        .unwrap();
        let dir = project.workspace_dir(DEFAULT_WORKSPACE);
        let main = tau_testing::block_on_io(tau_vcs::Vcs::open(
            &dir,
            Identity::default(),
        ))
        .unwrap();
        let mut machine = Self {
            home,
            kind,
            work,
            project,
            main,
            commits: 0,
            imported: BTreeMap::new(),
            head: None,
            local: BTreeMap::new(),
            own: Vec::new(),
            base: ROOT.to_owned(),
            trunk: ROOT.to_owned(),
            chats: Vec::new(),
            dirty: false,
        };
        machine.imported = machine.branches();
        machine.head = head_branch(&machine.source_git());
        machine.trunk = machine.want_trunk().1;
        machine.check_mirror();
        machine
    }

    /// The Git directory the project mirrors.
    fn source_git(&self) -> PathBuf {
        match self.kind {
            Kind::Checkout | Kind::Worktree => self.work.join(".git"),
            Kind::Bare | Kind::Remote => self.home.path().join("bare.git"),
        }
    }

    /// The source's branches, as `git` reports them.
    fn branches(&self) -> BTreeMap<String, String> {
        refs(&self.source_git(), "refs/heads/")
    }

    /// The bookmarks the project should have, but for `tau/` ones: the
    /// source's branches, with the main chat's moves on top.
    fn want_bookmarks(&self) -> BTreeMap<String, Option<String>> {
        self.bookmarks_over(self.branches())
    }

    /// [`Self::want_bookmarks`], over the source's `branches` as read.
    fn bookmarks_over(
        &self,
        branches: BTreeMap<String, String>,
    ) -> BTreeMap<String, Option<String>> {
        let mut want: BTreeMap<String, Option<String>> = branches
            .into_iter()
            .map(|(name, id)| (name, Some(id)))
            .collect();
        for (name, id) in &self.local {
            want.insert(name.clone(), Some(id.clone()));
        }
        want
    }

    /// Trunk's name and commit, by the reference's rule: the branch
    /// `HEAD` names, else `main`, `master` or `trunk`, whichever is a
    /// bookmark with one target; else the root commit, named after
    /// `HEAD`'s branch or `main`.
    fn want_trunk(&self) -> (String, String) {
        self.trunk_over(&self.want_bookmarks())
    }

    /// [`Self::want_trunk`], over the `bookmarks` the project should have.
    fn trunk_over(
        &self,
        bookmarks: &BTreeMap<String, Option<String>>,
    ) -> (String, String) {
        let head = self.head.clone();
        head.iter()
            .cloned()
            .chain(["main", "master", "trunk"].map(String::from))
            .find_map(|name| {
                let id = bookmarks.get(&name)?.clone()?;
                Some((name, id))
            })
            .unwrap_or_else(|| {
                (head.unwrap_or_else(|| "main".to_owned()), ROOT.to_owned())
            })
    }

    /// The project's Git store has the source's branches, tags and
    /// `HEAD`, and the project its bookmarks and trunk.
    fn check_mirror(&self) {
        let store = self.project.root().join("git");
        let source = self.source_git();
        // Read once: every check below is against the same branches.
        let branches = self.branches();
        assert_eq!(
            refs(&store, "refs/heads/"),
            branches,
            "the Git store's branches"
        );
        assert_eq!(
            refs(&store, "refs/tags/"),
            refs(&source, "refs/tags/"),
            "the Git store's tags"
        );
        assert_eq!(
            self.project.default_branch(),
            self.head,
            "the default branch"
        );
        let got: BTreeMap<String, Option<String>> = self
            .project
            .bookmarks("")
            .unwrap()
            .into_iter()
            .filter(|name| !name.starts_with("tau/"))
            .map(|name| {
                let id = self.project.bookmark(&name).unwrap();
                (name, id)
            })
            .collect();
        let want = self.bookmarks_over(branches.clone());
        assert_eq!(got, want, "bookmarks");
        let (name, id) = self.trunk_over(&want);
        assert_eq!(self.project.trunk_name().unwrap(), name, "trunk's name");
        assert_eq!(self.project.trunk().unwrap(), id, "trunk");
        // Every branch's files are readable at its commit.
        for id in branches.values() {
            let want = git(&source, &["show", &format!("{id}:f.txt")]);
            let (got, _) = self.project.file_at(id, "f.txt").unwrap().unwrap();
            assert_eq!(String::from_utf8(got).unwrap().trim(), want);
        }
    }

    /// Where the change `change_id` is now: one visible commit.
    fn now(&self, change_id: &str) -> String {
        let link = Link {
            turn: 0,
            workspace: String::new(),
            commit_id: String::new(),
            change_id: change_id.to_owned(),
            changed: false,
            from: None,
            snapshot: false,
        };
        let now = self.project.current([link]).unwrap().remove(0);
        assert!(!now.commit_id.is_empty(), "change {change_id} is gone");
        now.commit_id
    }

    /// Whether `ancestor` is an ancestor of `of`, as `git` says in the
    /// project's store.
    fn git_ancestor(&self, ancestor: &str, of: &str) -> bool {
        if ancestor == ROOT {
            return true;
        }
        if of == ROOT {
            return false;
        }
        is_ancestor(&self.project.root().join("git"), ancestor, of)
    }

    /// Whether upstream dropped what the main chat stands on: the trunk
    /// it last caught up with is no longer under trunk.
    fn base_dropped(&self) -> bool {
        !self.git_ancestor(&self.base, &self.trunk)
            && !self.own_ancestor_of_trunk()
    }

    /// Trunk is the main chat's newest commit.
    fn own_ancestor_of_trunk(&self) -> bool {
        self.own
            .last()
            .is_some_and(|own| self.now(&own.change_id) == self.trunk)
    }

    fn do_op(&mut self, tc: &TestCase, op: &Op) {
        let work = self.work.clone();
        let branches = refs(&work.join(".git"), "refs/heads/");
        let current = head_branch(&work.join(".git"));
        let tags = refs(&work.join(".git"), "refs/tags/");
        let write = |me: &mut Self| {
            me.commits += 1;
            std::fs::write(work.join("f.txt"), format!("{}\n", me.commits))
                .unwrap();
            git(&work, &["add", "-A"]);
        };
        match op {
            Op::Commit => {
                write(self);
                git(&work, &["commit", "--quiet", "-m", "upstream"]);
            }
            Op::Amend => {
                write(self);
                git(&work, &["commit", "--quiet", "--amend", "-m", "amended"]);
                tc.event("upstream amends");
            }
            Op::Reset
                if tau_testing::git::output(
                    &work,
                    &["rev-parse", "--verify", "-q", "HEAD~1"],
                )
                .status
                .success() =>
            {
                git(&work, &["reset", "--quiet", "--hard", "HEAD~1"]);
                tc.event("upstream resets back");
            }
            Op::Branch(name) if !branches.contains_key(name) => {
                if branches.keys().any(|b| {
                    b.starts_with(&format!("{name}/"))
                        || name.starts_with(&format!("{b}/"))
                }) {
                    return;
                }
                git(&work, &["branch", name]);
            }
            Op::Checkout(name) if branches.contains_key(name) => {
                git(&work, &["checkout", "--quiet", name]);
            }
            Op::Detach => {
                git(&work, &["checkout", "--quiet", "--detach"]);
            }
            Op::Delete(name)
                if branches.contains_key(name)
                    && current.as_ref() != Some(name) =>
            {
                git(&work, &["branch", "--quiet", "-D", name]);
                tc.event("upstream deletes a branch");
            }
            Op::Rename(from, to)
                if branches.contains_key(from)
                    && !branches.contains_key(to)
                    && !branches.keys().any(|b| {
                        b != from
                            && (b.starts_with(&format!("{to}/"))
                                || to.starts_with(&format!("{b}/")))
                    })
                    && !(to.starts_with(&format!("{from}/"))
                        || from.starts_with(&format!("{to}/"))) =>
            {
                git(&work, &["branch", "--quiet", "-m", from, to]);
                tc.event("upstream renames a branch");
            }
            Op::Move(name, to)
                if branches.contains_key(name)
                    && branches.contains_key(to)
                    && current.as_ref() != Some(name) =>
            {
                git(&work, &["branch", "--quiet", "-f", name, to]);
            }
            Op::Tag(name, annotated) if !tags.contains_key(name) => {
                if *annotated {
                    git(&work, &["tag", "-a", "-m", "tag", name]);
                } else {
                    git(&work, &["tag", name]);
                }
            }
            Op::DeleteTag(name) if tags.contains_key(name) => {
                git(&work, &["tag", "-d", name]);
            }
            Op::PackRefs => {
                git(&work, &["pack-refs", "--all"]);
            }
            Op::Gc => {
                git(&work, &["reflog", "expire", "--expire=now", "--all"]);
                git(&work, &["gc", "--quiet", "--prune=now"]);
                tc.event("upstream collects garbage");
            }
            Op::Repack => {
                git(&work, &["repack", "--quiet", "-a", "-d", "--write-midx"]);
                git(&work, &["prune", "--expire=now"]);
                tc.event("upstream repacks with a multi-pack index");
            }
            _ => return,
        }
        self.dirty = true;
    }

    /// `ProjectRepo::update`, against the model.
    fn do_update(&mut self, tc: &TestCase) {
        publish(self.kind, &self.work, &self.home.path().join("bare.git"));
        let from = match self.kind {
            Kind::Remote => None,
            Kind::Checkout => Some(self.work.clone()),
            Kind::Worktree => Some(self.home.path().join("wt")),
            Kind::Bare => Some(self.source_git()),
        };
        let url = url(self.home.path());
        let before = self.trunk.clone();
        let updated = match &from {
            Some(path) => self.project.update(UpdateFrom::Checkout(path)),
            None => self.project.update(UpdateFrom::Remote {
                url: &url,
                token: None,
            }),
        }
        .unwrap();
        if !self.dirty {
            tc.event("an update with nothing new");
        }

        // A bookmark the main chat moved stays where it put it while
        // upstream leaves it; once upstream moves or deletes it too, it
        // takes upstream's side.
        let now = self.branches();
        if matches!(self.kind, Kind::Bare | Kind::Remote) {
            self.head = head_branch(&self.source_git());
        } else if self.head != head_branch(&self.source_git()) {
            tc.event("an update from a checkout on another branch");
        }
        let head = self.head.clone();
        for (name, id) in std::mem::take(&mut self.local) {
            let base = self.imported.get(&name);
            let new = now.get(&name);
            if new == base {
                self.local.insert(name, id);
            } else if new != Some(&id) {
                tc.event("an update under the main chat's commits");
                if new.is_none() {
                    tc.event("upstream deleted a bookmark the main chat moved");
                }
            }
        }
        self.imported = now;
        let (_, trunk) = self.want_trunk();
        self.trunk = trunk;
        assert_eq!(updated.before, before, "Updated::before");
        assert_eq!(updated.after, self.trunk, "Updated::after");
        if updated.changed() {
            tc.event("an update moves trunk");
        }
        if head.is_none() {
            tc.event("an update from a detached HEAD");
        }
        if self.trunk == ROOT {
            tc.event("an update leaves no trunk branch");
        }
        self.dirty = false;
        self.check_mirror();
        // Chats stay as they were.
        for chat in &self.chats {
            assert_eq!(
                self.project
                    .bookmark(&format!("tau/{}", chat.name))
                    .unwrap(),
                Some(chat.commit_id.clone()),
                "an update moved {}",
                chat.name
            );
            assert_eq!(
                self.project.workspace_head(&chat.name).unwrap(),
                Some(chat.wc.clone())
            );
        }
    }

    /// The host's catch-up before a main chat's turn: its own commits
    /// that trunk lacks, and `@`, move onto trunk's head, and nothing
    /// else, even when upstream dropped what they stood on. Trunk can
    /// have some of them already: a bookmark the main chat moved onto
    /// one becomes trunk when `HEAD` names it again.
    fn do_catch_up(&mut self, tc: &TestCase) {
        if self.base_dropped() {
            tc.event("upstream dropped the main chat's base");
        }
        let name = self.project.trunk_name().unwrap();
        let trunk = self.trunk.clone();
        let ahead = self.own_ancestor_of_trunk();
        // Oldest first.
        let behind: Vec<&Own> = self
            .own
            .iter()
            .filter(|own| !self.git_ancestor(&self.now(&own.change_id), &trunk))
            .collect();
        if !behind.is_empty() && behind.len() < self.own.len() {
            tc.event("a catch-up onto some of the main chat's commits");
        }
        let old: BTreeMap<String, String> = self
            .own
            .iter()
            .map(|own| (self.now(&own.change_id), own.change_id.clone()))
            .collect();
        let moved =
            block_on(self.main.move_onto(trunk.clone(), name.clone(), true))
                .unwrap();
        let ids: Vec<&str> =
            moved.changes.iter().map(|c| c.change_id.as_str()).collect();
        let want: Vec<&str> =
            behind.iter().rev().map(|o| o.change_id.as_str()).collect();
        assert_eq!(ids, want, "the catch-up moves the main chat's commits");
        assert!(moved.conflicts.is_empty(), "{moved:?}");
        if want.is_empty() {
            assert_eq!(moved.head, trunk, "the catch-up moved trunk");
        } else {
            tc.event("a catch-up restacks the main chat's commits");
        }
        if let Some(oldest) = behind.first() {
            let first = self.now(&oldest.change_id);
            assert_eq!(
                self.project.parent_of(&first).unwrap(),
                (trunk != ROOT).then(|| trunk.clone()),
                "the main chat's commits go on top of trunk"
            );
        }
        if !ahead {
            self.base = trunk.clone();
        }
        // jj moves bookmarks with the commits they name.
        for id in self.local.values_mut() {
            if let Some(change) = old.get(id) {
                let link = Link {
                    turn: 0,
                    workspace: String::new(),
                    commit_id: id.clone(),
                    change_id: change.clone(),
                    changed: false,
                    from: None,
                    snapshot: false,
                };
                *id = self.project.current([link]).unwrap().remove(0).commit_id;
            }
        }
        let head = moved.head.clone();
        // No bookmark names the root commit: a catch-up onto no trunk
        // with nothing of its own leaves trunk's name unset.
        if head == ROOT {
            self.local.remove(&name);
        } else if self.imported.get(&name) != Some(&head) {
            self.local.insert(name, head.clone());
        }
        self.trunk = head;
        assert_eq!(self.project.trunk().unwrap(), self.trunk);
        // A chat on a commit the catch-up rewrote moves with it.
        for i in 0..self.chats.len() {
            let now = self.now(&self.chats[i].change_id);
            if now != self.chats[i].commit_id {
                tc.event("a catch-up moves a chat");
                let wc = self.project.workspace_head(&self.chats[i].name);
                self.chats[i].wc = wc.unwrap().unwrap();
                self.chats[i].commit_id = now;
            }
        }
    }

    fn check_own(&self) {
        // The main chat's commits stay, each one visible, with its file.
        for own in &self.own {
            let at = self.now(&own.change_id);
            assert!(
                self.project.file_at(&at, &own.file).unwrap().is_some(),
                "{own:?} lost its file"
            );
        }
        for chat in &self.chats {
            let at = self.now(&chat.change_id);
            assert!(self.project.file_at(&at, &chat.file).unwrap().is_some());
        }
    }
}

/// The bare repository's URL.
fn url(home: &Path) -> String {
    format!("file://{}", home.join("bare.git").display())
}

/// Copies the checkout's refs and `HEAD` to the bare repository, for
/// the kinds that have one.
fn publish(kind: Kind, work: &Path, bare: &Path) {
    if !matches!(kind, Kind::Bare | Kind::Remote) {
        return;
    }
    let to = bare.to_str().unwrap();
    git(work, &["push", "--quiet", "--mirror", "--force", to]);
    match head_branch(&work.join(".git")) {
        Some(name) => {
            git(
                bare,
                &["symbolic-ref", "HEAD", &format!("refs/heads/{name}")],
            );
        }
        None => {
            // A detached `HEAD` needs its commit there, under a ref that
            // is neither a branch nor a tag.
            git(
                work,
                &["push", "--quiet", "--force", to, "HEAD:refs/keep/head"],
            );
            let id = git(work, &["rev-parse", "HEAD"]);
            git(bare, &["update-ref", "--no-deref", "HEAD", &id]);
        }
    }
}

#[hegel::state_machine]
impl Machine {
    /// Upstream moves on: a few commands in the checkout.
    #[rule(weight = 3)]
    fn upstream(&mut self, tc: TestCase) {
        let ops = tc.draw(gs::vecs(op()).min_size(1).max_size(4));
        for op in &ops {
            self.do_op(&tc, op);
        }
    }

    #[rule(weight = 3)]
    fn update(&mut self, tc: TestCase) {
        self.do_update(&tc);
    }

    /// The main chat's turn: it catches up, then commits a file of its
    /// own on trunk.
    #[rule(weight = 3)]
    fn main_commit(&mut self, tc: TestCase) {
        self.do_catch_up(&tc);
        let name = self.project.trunk_name().unwrap();
        let file = format!("m{}.txt", self.own.len());
        let dir = self.project.workspace_dir(DEFAULT_WORKSPACE);
        std::fs::write(dir.join(&file), "ours\n").unwrap();
        let before = self.trunk.clone();
        let committed =
            block_on(self.main.commit_all("ours", name.clone())).unwrap();
        assert!(committed.changed);
        assert_eq!(
            self.project.parent_of(&committed.commit_id).unwrap(),
            (before != ROOT).then_some(before)
        );
        self.own.push(Own {
            change_id: committed.change_id,
            file,
        });
        self.local.insert(name.clone(), committed.commit_id.clone());
        self.trunk = committed.commit_id;
        tc.event("the main chat commits");
    }

    #[rule]
    fn catch_up(&mut self, tc: TestCase) {
        self.do_catch_up(&tc);
    }

    /// A chat starts on trunk and commits.
    #[rule]
    fn chat(&mut self, tc: TestCase) {
        if self.chats.len() >= 2 {
            return;
        }
        let name = format!("chat{}", self.chats.len());
        let vcs = self.project.add_workspace(&name, &self.trunk).unwrap();
        let file = format!("{name}.txt");
        std::fs::write(self.project.workspace_dir(&name).join(&file), "c\n")
            .unwrap();
        let committed =
            block_on(vcs.commit_all("chat", format!("tau/{name}"))).unwrap();
        let wc = self.project.workspace_head(&name).unwrap().unwrap();
        self.chats.push(Chat {
            name,
            commit_id: committed.commit_id,
            change_id: committed.change_id,
            wc,
            file,
        });
        tc.event("a chat on trunk as it was");
    }

    #[invariant(always_run)]
    fn the_project_follows(&self, tc: TestCase) {
        tc.event_value("main chat commits", self.own.len() as f64);
        assert_eq!(self.project.trunk().unwrap(), self.trunk, "trunk");
        self.check_own();
    }
}

/// Updates, history rewrites, main chat commits and catch-ups against
/// the model, from each kind of source.
#[hegel::test(
    test_cases = 40,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn updates_follow_the_source(tc: TestCase) {
    let machine = Machine::new(&tc);
    hegel::stateful::machine(machine).steps(30).run(tc);
}

#[hegel::test(
    profile = "nightly_slow",
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
#[ignore = "nightly"]
fn updates_follow_the_source_nightly(tc: TestCase) {
    let machine = Machine::new(&tc);
    hegel::stateful::machine(machine).steps(40).run(tc);
}

/// A checkout with one commit on `main`, writing `f.txt`, and a
/// project made from it whose main chat has caught up with trunk.
fn caught_up() -> (tempfile::TempDir, PathBuf, ProjectRepo, Vcs) {
    let home = tempfile::tempdir().unwrap();
    let work = home.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "--quiet"]);
    std::fs::write(work.join("f.txt"), "0\n").unwrap();
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "--quiet", "-m", "first"]);
    let project = tau_vcs::ProjectRepo::import(
        work.to_str().unwrap(),
        home.path().join("p"),
        Identity::default(),
    )
    .unwrap();
    let dir = project.workspace_dir(DEFAULT_WORKSPACE);
    let main =
        tau_testing::block_on_io(tau_vcs::Vcs::open(&dir, Identity::default()))
            .unwrap();
    let trunk = project.trunk().unwrap();
    block_on(main.move_onto(trunk, project.trunk_name().unwrap(), true))
        .unwrap();
    (home, work, project, main)
}

/// Upstream drops a commit the main chat stands on, and the main chat
/// has no commit of its own. Its catch-up leaves trunk where upstream
/// has it: what upstream dropped stays dropped.
#[test]
fn a_catch_up_leaves_what_upstream_dropped() {
    let (_home, work, project, main) = caught_up();
    std::fs::write(work.join("f.txt"), "1\n").unwrap();
    git(&work, &["commit", "--quiet", "-am", "second"]);
    project.update(UpdateFrom::Checkout(&work)).unwrap();
    let name = project.trunk_name().unwrap();
    block_on(main.move_onto(project.trunk().unwrap(), name.clone(), true))
        .unwrap();

    git(&work, &["reset", "--quiet", "--hard", "HEAD~1"]);
    let first = git(&work, &["rev-parse", "HEAD"]);
    let updated = project.update(UpdateFrom::Checkout(&work)).unwrap();
    assert_eq!(updated.after, first);
    let moved = block_on(main.move_onto(first.clone(), name, true)).unwrap();
    assert_eq!(moved.changes, [], "the catch-up moved upstream's commits");
    assert_eq!(project.trunk().unwrap(), first);
}

/// Upstream amends the commit the main chat's own commit stands on.
/// The catch-up moves the main chat's commit onto the amended one, and
/// not the commit the amend replaced, so nothing conflicts.
#[test]
fn a_catch_up_onto_an_amend_moves_only_the_main_chats_commits() {
    let (_home, work, project, main) = caught_up();
    let dir = project.workspace_dir(DEFAULT_WORKSPACE);
    std::fs::write(dir.join("ours.txt"), "ours\n").unwrap();
    let name = project.trunk_name().unwrap();
    let ours = block_on(main.commit_all("ours", name.clone())).unwrap();

    std::fs::write(work.join("f.txt"), "amended\n").unwrap();
    git(
        &work,
        &["commit", "--quiet", "-a", "--amend", "-m", "amended"],
    );
    let amended = git(&work, &["rev-parse", "HEAD"]);
    project.update(UpdateFrom::Checkout(&work)).unwrap();
    assert_eq!(project.trunk().unwrap(), amended);
    let moved = block_on(main.move_onto(amended.clone(), name, true)).unwrap();
    let ids: Vec<&str> =
        moved.changes.iter().map(|c| c.change_id.as_str()).collect();
    assert_eq!(ids, [ours.change_id.as_str()]);
    assert!(moved.conflicts.is_empty(), "{moved:?}");
    assert_eq!(project.parent_of(&moved.head).unwrap(), Some(amended));
    assert_eq!(project.trunk().unwrap(), moved.head);
}

/// The main chat commits on `main`, and upstream renames `main` to
/// `trunk`. The update makes `trunk` the trunk, and `main`, which the
/// main chat moved and upstream deleted, goes too: an update takes
/// upstream's side for every bookmark it leaves with two targets. The
/// main chat's commit stays on its `@`, and its catch-up moves it onto
/// the new trunk.
#[test]
fn a_bookmark_the_main_chat_moved_takes_upstreams_side() {
    let (_home, work, project, main) = caught_up();
    let dir = project.workspace_dir(DEFAULT_WORKSPACE);
    std::fs::write(dir.join("ours.txt"), "ours\n").unwrap();
    let ours = block_on(main.commit_all("ours", "main")).unwrap();

    git(&work, &["branch", "--quiet", "-m", "main", "trunk"]);
    project.update(UpdateFrom::Checkout(&work)).unwrap();
    assert_eq!(project.trunk_name().unwrap(), "trunk");
    assert_eq!(project.bookmarks("").unwrap(), ["trunk"]);
    let moved = block_on(main.move_onto(
        project.trunk().unwrap(),
        project.trunk_name().unwrap(),
        true,
    ))
    .unwrap();
    assert_eq!(moved.changes[0].change_id, ours.change_id);
}

/// Upstream deletes its only branch, so trunk is the root commit, and
/// the main chat, with no commit of its own, catches up. It moves onto
/// the root commit, but points no bookmark there: a bookmark on the
/// root commit names no branch's work, and the next update would find
/// trunk's bookmark on a commit upstream never had.
#[test]
fn a_catch_up_onto_no_trunk_points_no_bookmark_at_the_root() {
    let (_home, work, project, main) = caught_up();
    git(&work, &["checkout", "--quiet", "--detach"]);
    git(&work, &["branch", "--quiet", "-D", "main"]);
    let updated = project.update(UpdateFrom::Checkout(&work)).unwrap();
    assert_eq!(updated.after, ROOT);
    let name = project.trunk_name().unwrap();
    let moved = block_on(main.move_onto(ROOT, name, true)).unwrap();
    assert_eq!(moved.head, ROOT);
    assert_eq!(project.bookmarks("").unwrap(), Vec::<String>::new());
    assert_eq!(project.trunk().unwrap(), ROOT);
}

/// The main chat commits twice on `main`, then the source's `HEAD`
/// moves to a new `master`, and the main chat commits once more there;
/// then `HEAD` goes back to `main`, which upstream never moved. Trunk is
/// `main` again, on the main chat's second commit, so its catch-up moves
/// the third alone: the first two are under trunk already. The model
/// used to expect every commit of the main chat's to move unless the
/// newest was trunk.
#[test]
fn a_catch_up_leaves_the_main_chats_commits_under_trunk() {
    hegel::Hegel::new(|tc| {
        let mut m = Machine::from_source(Kind::Bare, "main");
        m.main_commit(tc.clone());
        m.main_commit(tc.clone());
        m.do_op(&tc, &Op::Branch("master".into()));
        m.do_op(&tc, &Op::Checkout("master".into()));
        m.do_update(&tc);
        m.main_commit(tc.clone());
        m.do_op(&tc, &Op::Checkout("main".into()));
        m.do_update(&tc);
        m.the_project_follows(tc.clone());
        m.catch_up(tc.clone());
        m.the_project_follows(tc);
    })
    .settings(hegel::Settings::new().test_cases(1))
    .run();
}

/// The model reads refs and ancestry in process: as `git` reads them,
/// loose or packed, lightweight or annotated, before and after `gc`.
#[test]
fn refs_and_ancestry_read_as_git_does() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path();
    git(dir, &["init", "--quiet", "-b", "main"]);
    std::fs::write(dir.join("f.txt"), "one\n").unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "--quiet", "-m", "one"]);
    let first = git(dir, &["rev-parse", "HEAD"]);
    git(dir, &["tag", "v1"]);
    std::fs::write(dir.join("f.txt"), "two\n").unwrap();
    git(dir, &["commit", "--quiet", "-am", "two"]);
    let second = git(dir, &["rev-parse", "HEAD"]);
    git(dir, &["tag", "-a", "-m", "tag", "v2"]);
    git(dir, &["branch", "feat/x", &first]);
    let git_dir = dir.join(".git");
    let by_git = |prefix: &str| -> BTreeMap<String, String> {
        let format = "--format=%(refname) %(objectname)";
        git(dir, &["for-each-ref", format, prefix])
            .lines()
            .map(|line| {
                let (name, id) = line.split_once(' ').unwrap();
                (name.strip_prefix(prefix).unwrap().to_owned(), id.to_owned())
            })
            .collect()
    };
    for packed in [false, true] {
        if packed {
            git(dir, &["gc", "--quiet", "--prune=now"]);
        }
        for prefix in ["refs/heads/", "refs/tags/"] {
            assert_eq!(refs(&git_dir, prefix), by_git(prefix), "{prefix}");
        }
        assert!(is_ancestor(&git_dir, &first, &second));
        assert!(is_ancestor(&git_dir, &second, &second));
        assert!(!is_ancestor(&git_dir, &second, &first));
        assert!(!is_ancestor(&git_dir, &"1".repeat(40), &second));
    }
}
