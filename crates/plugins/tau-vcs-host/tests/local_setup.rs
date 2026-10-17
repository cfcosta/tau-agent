//! Setting jj up in the person's folder, in place (ADR 0043,
//! `docs/reference/vcs.md`, "Setting up a local repository"):
//!
//! - after a setup, the person's Git is as it was: `HEAD` (symbolic, and
//!   the commit it names), the index's bytes, their branches and tags,
//!   `.git/config` and their files, but for the branches their
//!   `auto-track-bookmarks` makes follow `origin`'s, as jj's would; a jj
//!   repository of theirs keeps its working copy and bookmarks
//!   (metamorphic);
//! - a setup cut off after any step, or failing at its last, leaves the
//!   folder as it was: refs, `HEAD`, index, config and files, no `.jj`
//!   when tau made it and the person's `.jj` as it was (but for jj's
//!   content-addressed stores, which keep the new operations, index
//!   files and commit metadata, unreachable), the person's jj config,
//!   and nothing in tau's directories; one cut off by a crash is
//!   cleared by the next (fault injection);
//! - a setup leaves the repository as `jj git init --colocate` leaves a
//!   copy of the folder, but for the index and tau's workspace: the
//!   commits, bookmarks and tags, `default`'s working copy, the
//!   `trunk()` alias, `.jj/.gitignore` and Git's refs; and the person's
//!   `jj status` in the folder shows what it shows in the copy
//!   (differential, with the jj CLI of jj-lib's version);
//! - tau's trunk commits from its own workspace, interleaved with the
//!   person's `git commit`, `git checkout` and `git commit --amend`,
//!   never move the person's `HEAD` but by jj's detaching on the same
//!   commit when `HEAD` was on trunk and trunk moved, and never their
//!   index; an amended commit tau imported is hidden exactly when the
//!   person's `git.abandon-unreachable-commits` says so (model).
//!
//! Each world lives in a temporary directory and reads no real config.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

#[path = "common/jj_cli.rs"]
mod jj_cli;

use std::{
    collections::BTreeMap,
    ops::ControlFlow,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use hegel::{TestCase, generators as gs};
use tau_testing::block_on_io;
use tau_vcs_host::{
    Identity,
    JjEnv,
    Layout,
    ProjectRepo,
    Vcs,
    VcsError,
    local::{self, Found, Kind, State, Step},
};

const FILES: [&str; 6] =
    ["a", "b.txt", "dir/c", "dir/sub/d", "with space", "ünï.md"];
/// Branches besides `main`, and on remotes.
const BRANCHES: [&str; 4] = ["main", "feature", "release/1.x", "fix"];
const TAGS: [&str; 2] = ["v1", "v2"];
const REMOTES: [&str; 2] = ["origin", "upstream"];

struct Person {
    root: PathBuf,
}

impl Person {
    fn new(root: &Path) -> Self {
        for dir in ["home", "config/jj"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        Self {
            root: root.to_owned(),
        }
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn config(&self) -> PathBuf {
        self.root.join("config")
    }

    /// Writes the person's jj config.
    fn configure(&self, abandon: bool, auto_track: bool) {
        let mut text = format!(
            "user.name = \"P\"\nuser.email = \"p@p\"\n\n[git]\n\
             abandon-unreachable-commits = {abandon}\n"
        );
        if auto_track {
            text.push_str(
                "\n[remotes.origin]\nauto-track-bookmarks = \"glob:*\"\n",
            );
        }
        std::fs::write(self.config().join("jj").join("config.toml"), text)
            .unwrap();
    }

    fn identity(&self) -> Identity {
        Identity {
            env: JjEnv {
                config_dir: Some(self.config()),
                home_dir: Some(self.home()),
                vars: [(
                    "XDG_CONFIG_HOME".to_owned(),
                    self.config().display().to_string(),
                )]
                .into(),
                ..JjEnv::default()
            },
            ..Identity::default()
        }
    }

    fn git_output(&self, dir: &Path, args: &[&str]) -> Output {
        tau_testing::git::command(dir)
            .env("HOME", self.home())
            .env("XDG_CONFIG_HOME", self.config())
            .args(args)
            .output()
            .expect("git runs")
    }

    /// `git args` in `dir`, which must succeed; what it printed, trimmed.
    fn git(&self, dir: &Path, args: &[&str]) -> String {
        let output = self.git_output(dir, args);
        assert!(
            output.status.success(),
            "git {args:?} in {dir:?}: {output:?}"
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    /// `git args` in `dir`, when it succeeds.
    fn try_git(&self, dir: &Path, args: &[&str]) -> Option<String> {
        let output = self.git_output(dir, args);
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    /// The jj CLI as this person runs it in `dir`.
    fn jj(&self, jj: &Path, dir: &Path, args: &[&str]) -> Output {
        Command::new(jj)
            .args(args)
            .current_dir(dir)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.home())
            .env("XDG_CONFIG_HOME", self.config())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("NO_COLOR", "1")
            .output()
            .expect("jj runs")
    }

    /// `jj args` in `dir`, which must succeed; what it printed.
    fn run_jj(&self, jj: &Path, dir: &Path, args: &[&str]) -> String {
        let output = self.jj(jj, dir, args);
        assert!(
            output.status.success(),
            "jj {args:?} in {dir:?}: {output:?}"
        );
        String::from_utf8(output.stdout).unwrap()
    }
}

/// Where `HEAD` ends up.
#[derive(Debug, Clone, hegel::PrettyPrintable)]
enum HeadAt {
    Main,
    Branch(usize),
    Detached(usize),
}

#[derive(Debug, Clone, hegel::PrettyPrintable)]
enum Edit {
    Write {
        file: usize,
        content: u8,
    },
    Stage {
        file: usize,
    },
    Remove {
        file: usize,
    },
    /// A file `*.log`, which `.git/info/exclude` ignores.
    Ignored,
}

/// A remote's branches, as `(branch, commit)`, and the branch its
/// `HEAD` names.
#[derive(Debug, Clone, hegel::PrettyPrintable)]
struct Remote {
    branches: Vec<(usize, usize)>,
    head: Option<usize>,
}

/// A jj repository of the person's, made with the jj CLI on the Git
/// repository: commits of these files, and a bookmark at the last.
#[derive(Debug, Clone, hegel::PrettyPrintable)]
struct JjPlan {
    commits: Vec<usize>,
    bookmark: Option<usize>,
}

#[derive(Debug, Clone, hegel::PrettyPrintable)]
struct World {
    commits: Vec<Vec<(usize, u8)>>,
    branches: Vec<(usize, usize)>,
    tags: Vec<(usize, usize, bool)>,
    head: HeadAt,
    remotes: Vec<Option<Remote>>,
    edits: Vec<Edit>,
    abandon: bool,
    auto_track: bool,
    jj: Option<JjPlan>,
}

fn index(tc: &TestCase, len: usize) -> usize {
    tc.draw(gs::integers::<usize>().max_value(len - 1))
}

fn case(tc: &TestCase, n: u8) -> u8 {
    tc.draw(gs::sampled_from((0..n).collect::<Vec<_>>()))
}

#[hegel::composite]
fn edit(tc: &TestCase) -> Edit {
    let file = index(tc, FILES.len());
    match case(tc, 4) {
        0 => Edit::Write {
            file,
            content: tc.draw(gs::integers::<u8>().max_value(3)),
        },
        1 => Edit::Stage { file },
        2 => Edit::Remove { file },
        _ => Edit::Ignored,
    }
}

fn pairs(tc: &TestCase, names: usize, max: usize) -> Vec<(usize, usize)> {
    tc.draw(
        gs::vecs(gs::tuples!(
            gs::integers::<usize>().max_value(names - 1),
            gs::integers::<usize>().max_value(3)
        ))
        .max_size(max),
    )
}

/// A world; with `jj`, now and then a jj repository of the person's.
fn world(tc: &TestCase, jj: bool, auto_track: bool) -> World {
    let fewest = usize::from(case(tc, 6) != 0);
    World {
        commits: tc.draw(
            gs::vecs(
                gs::vecs(gs::tuples!(
                    gs::integers::<usize>().max_value(FILES.len() - 1),
                    gs::integers::<u8>().max_value(3)
                ))
                .min_size(1)
                .max_size(3),
            )
            .min_size(fewest)
            .max_size(4),
        ),
        branches: pairs(tc, BRANCHES.len(), 2),
        tags: tc.draw(
            gs::vecs(gs::tuples!(
                gs::integers::<usize>().max_value(TAGS.len() - 1),
                gs::integers::<usize>().max_value(3),
                gs::booleans()
            ))
            .max_size(2),
        ),
        head: match case(tc, 4) {
            0 | 1 => HeadAt::Main,
            2 => HeadAt::Branch(index(tc, BRANCHES.len())),
            _ => {
                HeadAt::Detached(tc.draw(gs::integers::<usize>().max_value(3)))
            }
        },
        remotes: REMOTES
            .iter()
            .map(|_| {
                tc.draw(gs::booleans()).then(|| Remote {
                    branches: pairs(tc, BRANCHES.len(), 3),
                    head: tc.draw(gs::optional(
                        gs::integers::<usize>().max_value(BRANCHES.len() - 1),
                    )),
                })
            })
            .collect(),
        edits: tc.draw(gs::vecs(edit()).max_size(5)),
        abandon: tc.draw(gs::booleans()),
        auto_track: auto_track && tc.draw(gs::booleans()),
        jj: (jj && case(tc, 3) == 0).then(|| JjPlan {
            commits: tc.draw(
                gs::vecs(gs::integers::<usize>().max_value(FILES.len() - 1))
                    .max_size(2),
            ),
            bookmark: tc.draw(gs::optional(
                gs::integers::<usize>().max_value(BRANCHES.len() - 1),
            )),
        }),
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// Builds `world`'s folder under `base` with the git CLI, and the jj CLI
/// for its jj repository. Returns the folder.
fn build(
    person: &Person,
    base: &Path,
    world: &World,
    jj: Option<&Path>,
) -> PathBuf {
    person.configure(world.abandon, world.auto_track);
    let folder = base.join("repo");
    std::fs::create_dir_all(&folder).unwrap();
    person.git(&folder, &["init", "--quiet"]);
    write(&folder.join(".git/info/exclude"), "*.log\n");
    for (k, files) in world.commits.iter().enumerate() {
        for (file, content) in files {
            write(&folder.join(FILES[*file]), &format!("{k}-{content}\n"));
        }
        person.git(&folder, &["add", "-A"]);
        person.git(
            &folder,
            &["commit", "--quiet", "--allow-empty", "-m", &format!("c{k}")],
        );
    }
    let ids: Vec<String> = person
        .try_git(&folder, &["rev-list", "--reverse", "HEAD"])
        .map(|text| text.lines().map(str::to_owned).collect())
        .unwrap_or_default();
    let at = |k: usize| ids.get(k % ids.len().max(1)).cloned();
    for &(branch, k) in &world.branches {
        if let Some(id) = at(k)
            && BRANCHES[branch] != "main"
        {
            person.git(&folder, &["branch", "--force", BRANCHES[branch], &id]);
        }
    }
    for &(tag, k, annotated) in &world.tags {
        if let Some(id) = at(k) {
            let mut args = vec!["tag", "--force"];
            if annotated {
                args.extend(["-a", "-m", "tag"]);
            }
            args.extend([TAGS[tag], id.as_str()]);
            person.git(&folder, &args);
        }
    }
    for (remote, plan) in REMOTES.iter().zip(&world.remotes) {
        let Some(plan) = plan else {
            continue;
        };
        let url = format!("https://example.com/{remote}/repo.git");
        person.git(&folder, &["remote", "add", remote, &url]);
        for &(branch, k) in &plan.branches {
            if let Some(id) = at(k) {
                let name =
                    format!("refs/remotes/{remote}/{}", BRANCHES[branch]);
                person.git(&folder, &["update-ref", &name, &id]);
            }
        }
        if let Some(head) = plan.head {
            let name = format!("refs/remotes/{remote}/{}", BRANCHES[head]);
            if person
                .try_git(&folder, &["rev-parse", "--verify", &name])
                .is_some()
            {
                person.git(
                    &folder,
                    &[
                        "symbolic-ref",
                        &format!("refs/remotes/{remote}/HEAD"),
                        &name,
                    ],
                );
            }
        }
    }
    match &world.head {
        HeadAt::Main => {}
        HeadAt::Branch(branch) => {
            let _ = person
                .try_git(&folder, &["checkout", "--quiet", BRANCHES[*branch]]);
        }
        HeadAt::Detached(k) => {
            if let Some(id) = at(*k) {
                person.git(&folder, &["checkout", "--quiet", "--detach", &id]);
            }
        }
    }
    if let (Some(plan), Some(jj)) = (&world.jj, jj) {
        person.run_jj(jj, &folder, &["git", "init", "--colocate"]);
        for (k, file) in plan.commits.iter().enumerate() {
            write(&folder.join(FILES[*file]), &format!("jj-{k}\n"));
            person.run_jj(jj, &folder, &["commit", "-m", &format!("j{k}")]);
        }
        if let Some(bookmark) = plan.bookmark {
            person.run_jj(
                jj,
                &folder,
                &[
                    "bookmark",
                    "set",
                    "--allow-backwards",
                    BRANCHES[bookmark],
                    "-r",
                    "@-",
                ],
            );
        }
    }
    for edit in &world.edits {
        match edit {
            Edit::Write { file, content } => {
                write(&folder.join(FILES[*file]), &format!("edit-{content}\n"));
            }
            Edit::Stage { file } => {
                let _ = person.try_git(&folder, &["add", "--", FILES[*file]]);
            }
            Edit::Remove { file } => {
                let _ = std::fs::remove_file(folder.join(FILES[*file]));
            }
            Edit::Ignored => write(&folder.join("x.log"), "ignored\n"),
        }
    }
    folder
}

/// Where tau keeps the main chat's workspace and the runs, under `base`.
fn layout(base: &Path, folder: &Path) -> Layout {
    let data = base.join("data");
    let runs = data.join("runs");
    let name = local::runs_dir_name(&runs, folder);
    Layout::local(folder, data.join("main"), runs.join(name))
}

fn found(person: &Person, folder: &Path) -> Found {
    local::inspect_blocking(folder, &person.identity()).unwrap()
}

fn go_on(_: Step) -> ControlFlow<()> {
    ControlFlow::Continue(())
}

/// The person's Git, as they see it.
#[derive(Debug, PartialEq, Eq)]
struct GitView {
    head: Vec<u8>,
    head_commit: Option<String>,
    index: Option<Vec<u8>>,
    refs: BTreeMap<String, String>,
    config: Vec<u8>,
    files: BTreeMap<PathBuf, (u32, Vec<u8>)>,
}

/// Reads the index first: nothing here runs a git command that
/// refreshes it.
fn git_view(person: &Person, folder: &Path) -> GitView {
    let git = folder.join(".git");
    GitView {
        index: std::fs::read(git.join("index")).ok(),
        head: std::fs::read(git.join("HEAD")).unwrap(),
        head_commit: person
            .try_git(folder, &["rev-parse", "-q", "--verify", "HEAD"]),
        refs: refs(person, folder, &["refs/heads", "refs/tags"]),
        config: std::fs::read(git.join("config")).unwrap(),
        files: tree(folder, &[".git", ".jj"]),
    }
}

/// The refs under `prefixes`, with their targets.
fn refs(
    person: &Person,
    folder: &Path,
    prefixes: &[&str],
) -> BTreeMap<String, String> {
    let mut args = vec![
        "for-each-ref",
        "--format=%(refname) %(objectname) %(symref)",
    ];
    args.extend(prefixes);
    person
        .git(folder, &args)
        .lines()
        .map(|line| {
            let (name, rest) = line.split_once(' ').unwrap();
            (name.to_owned(), rest.trim_end().to_owned())
        })
        .collect()
}

/// Every file and folder under `dir` but those in `skip` (paths from
/// `dir`), with its mode and bytes.
fn tree(dir: &Path, skip: &[&str]) -> BTreeMap<PathBuf, (u32, Vec<u8>)> {
    use std::os::unix::fs::PermissionsExt as _;
    let mut files = BTreeMap::new();
    let mut pending = vec![dir.to_owned()];
    while let Some(next) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in entries {
            let entry = entry.unwrap();
            let path = entry.path();
            let relative = path.strip_prefix(dir).unwrap().to_owned();
            if skip.iter().any(|skip| relative.starts_with(skip)) {
                continue;
            }
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                files.insert(relative, (0, Vec::new()));
                pending.push(path);
            } else if kind.is_symlink() {
                let target = std::fs::read_link(&path).unwrap();
                files.insert(
                    relative,
                    (1, target.into_os_string().into_encoded_bytes()),
                );
            } else {
                let mode = entry.metadata().unwrap().permissions().mode();
                files.insert(relative, (mode, std::fs::read(&path).unwrap()));
            }
        }
    }
    files
}

/// What the person's jj shows of their repository, read without a
/// snapshot or an import: `default@`'s commit and the bookmarks.
fn persons_jj(jj: &Path, person: &Person, folder: &Path) -> (String, String) {
    let at = |args: &[&str]| {
        let mut all = vec!["--ignore-working-copy"];
        all.extend(args);
        person.run_jj(jj, folder, &all)
    };
    (
        at(&["log", "--no-graph", "-r", "default@", "-T", "commit_id"]),
        at(&["bookmark", "list", "--all-remotes"]),
    )
}

/// After a setup, the person's Git is as it was, and a jj repository of
/// theirs keeps its working copy and bookmarks; tau's workspace is a
/// worktree of their `.git`, and tau's state names it.
#[hegel::test(
    test_cases = 25,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn setup_leaves_the_persons_git_as_it_was(tc: TestCase) {
    let jj = jj_cli::matching_jj();
    let world = world(&tc, jj.is_some(), true);
    let base = tempfile::tempdir().unwrap();
    let person = Person::new(&base.path().join("person"));
    let folder = build(&person, base.path(), &world, jj.as_deref());
    let found = found(&person, &folder);
    let before = git_view(&person, &folder);
    let jj_before = jj
        .as_deref()
        .filter(|_| found.kind == Kind::Jj)
        .map(|jj| persons_jj(jj, &person, &folder));
    let layout = layout(base.path(), &folder);
    let project = ProjectRepo::setup_local(
        &found,
        layout.clone(),
        person.identity(),
        &mut go_on,
    )
    .unwrap();
    let after = git_view(&person, &folder);
    if world.auto_track {
        // A branch `origin` has is tracked, as the person's
        // `auto-track-bookmarks` says: jj makes or moves its bookmark,
        // which goes out as a branch, as `jj git init` does
        // (`setup_matches_jj_git_init_colocate`). Every other ref stays.
        let tracked = |name: &str| {
            name.strip_prefix("refs/heads/").is_some_and(|branch| {
                person
                    .try_git(
                        &folder,
                        &[
                            "rev-parse",
                            "--verify",
                            &format!("refs/remotes/origin/{branch}"),
                        ],
                    )
                    .is_some()
            })
        };
        for (name, target) in &before.refs {
            if !tracked(name) {
                assert_eq!(after.refs.get(name), Some(target), "{name}");
            }
        }
        for name in after.refs.keys() {
            assert!(before.refs.contains_key(name) || tracked(name), "{name}");
        }
        let after = GitView {
            refs: before.refs.clone(),
            ..after
        };
        assert_eq!(after, before);
    } else {
        assert_eq!(after, before);
    }
    if let (Some(jj), Some(jj_before)) = (jj.as_deref(), jj_before) {
        assert_eq!(persons_jj(jj, &person, &folder), jj_before);
    }
    let state = State::read(&folder).unwrap().unwrap();
    assert_eq!(state.main_workspace, project.main_workspace());
    assert_eq!(state.main_workspace, "tau-main");
    assert_eq!(
        Some(state.runs_dir.as_str()),
        layout.runs_dir.file_name().and_then(|name| name.to_str())
    );
    assert!(!state.repo_id.is_empty());
    assert!(!local::unfinished(&folder));
    let common =
        person.git(&layout.main_dir, &["rev-parse", "--git-common-dir"]);
    assert_eq!(
        std::fs::canonicalize(&common).unwrap(),
        std::fs::canonicalize(folder.join(".git")).unwrap()
    );
    if found.kind == Kind::Git {
        assert_eq!(
            std::fs::read_to_string(folder.join(".jj/.gitignore")).unwrap(),
            "/*\n"
        );
    }
    let opened = ProjectRepo::open_local(layout, person.identity()).unwrap();
    assert_eq!(opened.main_workspace(), "tau-main");
    assert_eq!(git_view(&person, &folder).index, before.index);
}

/// Where a setup is made to fail.
#[derive(Debug, Clone, Copy, hegel::PrettyPrintable)]
enum Cut {
    /// Stopped before this step: after the one before.
    Before(usize),
    /// The main chat's directory cannot be made: its parent is a file.
    Blocked,
}

/// Everything a failed setup must leave as it was: the folder (Git's
/// objects aside), the person's config, and tau's directories.
fn everything(base: &Path) -> BTreeMap<PathBuf, (u32, Vec<u8>)> {
    tree(base, &["repo/.git/objects"])
}

/// A setup that fails, cut off after any step or blocked at its last,
/// leaves everything as it was: no `.jj` when tau made it, the person's
/// as it was when they had one, only new operations and index files
/// added, which no head reaches. A setup again then succeeds.
#[hegel::test(
    test_cases = 30,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn a_failed_setup_leaves_the_folder_as_it_was(tc: TestCase) {
    let jj = jj_cli::matching_jj();
    let world = world(&tc, jj.is_some(), true);
    let cut = if case(&tc, 5) == 0 {
        Cut::Blocked
    } else {
        Cut::Before(index(&tc, Step::ALL.len()))
    };
    let base = tempfile::tempdir().unwrap();
    let person = Person::new(&base.path().join("person"));
    let folder = build(&person, base.path(), &world, jj.as_deref());
    let found = found(&person, &folder);
    let mut layout = layout(base.path(), &folder);
    if matches!(cut, Cut::Blocked) {
        write(&base.path().join("data/blocked"), "");
        layout.main_dir = base.path().join("data/blocked/main");
    }
    let before = everything(base.path());
    let mut announced = Vec::new();
    let made = ProjectRepo::setup_local(
        &found,
        layout.clone(),
        person.identity(),
        &mut |step| {
            announced.push(step);
            match cut {
                Cut::Before(k) if Step::ALL[k] == step => {
                    ControlFlow::Break(())
                }
                _ => ControlFlow::Continue(()),
            }
        },
    );
    tc.note(&format!("announced {announced:?}"));
    match cut {
        Cut::Before(k) => {
            if !announced.contains(&Step::ALL[k]) {
                // A jj repository of the person's is not snapshotted.
                assert_eq!(Step::ALL[k], Step::Snapshot);
                assert_eq!(found.kind, Kind::Jj);
                made.unwrap();
                return;
            }
            assert!(
                matches!(made, Err(VcsError::SetupStopped(step)) if step == Step::ALL[k]),
                "{made:?}"
            );
        }
        Cut::Blocked => {
            assert!(made.is_err(), "{made:?}");
            assert!(
                !matches!(made, Err(VcsError::SetupNotUndone { .. })),
                "{made:?}"
            );
        }
    }
    let after = everything(base.path());
    // In a jj repository of the person's, the operations the setup wrote
    // stay, unreachable, with the index and commit metadata jj keeps for
    // them: content-addressed stores, which jj merges as it loads them.
    let stores = |path: &Path| {
        found.kind == Kind::Jj
            && ["op_store", "index", "store/extra"].iter().any(|store| {
                path.starts_with(Path::new("repo/.jj/repo").join(store))
            })
    };
    for (path, entry) in &before {
        if !stores(path) {
            assert_eq!(after.get(path), Some(entry), "{}", path.display());
        }
    }
    for path in after.keys().filter(|path| !before.contains_key(*path)) {
        assert!(stores(path), "{} was added", path.display());
    }
    // Nothing is left that would stop a setup again.
    if matches!(cut, Cut::Before(_)) {
        let found = self::found(&person, &folder);
        ProjectRepo::setup_local(&found, layout, person.identity(), &mut go_on)
            .unwrap();
    }
}

/// A setup cut off by a crash in a Git repository leaves a mark in the
/// `.jj` it made: inspecting the folder sees the Git repository it was,
/// and the next setup takes the `.jj` away and succeeds, leaving the
/// person's Git as it was.
#[hegel::test(
    test_cases = 15,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn a_setup_cut_off_by_a_crash_is_cleared_by_the_next(tc: TestCase) {
    let world = world(&tc, false, false);
    let crash = Step::ALL[index(&tc, Step::ALL.len())];
    let base = tempfile::tempdir().unwrap();
    let person = Person::new(&base.path().join("person"));
    let folder = build(&person, base.path(), &world, None);
    let first = found(&person, &folder);
    let before = git_view(&person, &folder);
    let layout = layout(base.path(), &folder);
    let crashed =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            ProjectRepo::setup_local(
                &first,
                layout.clone(),
                person.identity(),
                &mut |step| {
                    assert!(step != crash, "tau closed");
                    ControlFlow::Continue(())
                },
            )
        }));
    assert!(crashed.is_err());
    if crash == Step::ImportRefs {
        assert!(!folder.join(".jj").exists());
    } else {
        assert!(local::unfinished(&folder));
    }
    let again = found(&person, &folder);
    assert_eq!(again, first);
    ProjectRepo::setup_local(&again, layout, person.identity(), &mut go_on)
        .unwrap();
    assert!(!local::unfinished(&folder));
    assert_eq!(git_view(&person, &folder), before);
}

/// Copies the folder `from` to `to`, files, folders and modes.
fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// What jj shows of a repository, the index aside: read without a
/// snapshot or an import.
fn jj_view(jj: &Path, person: &Person, folder: &Path) -> Vec<String> {
    let at = |args: &[&str]| {
        let mut all = vec!["--ignore-working-copy"];
        all.extend(args);
        let output = person.jj(jj, folder, &all);
        format!(
            "{args:?}: {} {}{}",
            output.status.success(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        )
    };
    vec![
        at(&[
            "log",
            "--no-graph",
            "-r",
            "all() ~ working_copies()",
            "-T",
            "commit_id ++ \" \" ++ parents.map(|c| c.commit_id()).join(\",\") \
             ++ \" \" ++ bookmarks.join(\",\") ++ \" \" ++ tags.join(\",\") \
             ++ \"\\n\"",
        ]),
        at(&[
            "log",
            "--no-graph",
            "-r",
            "default@",
            "-T",
            "parents.map(|c| c.commit_id()).join(\",\") ++ \"|\" ++ description",
        ]),
        at(&["diff", "--git", "-r", "default@"]),
        at(&["bookmark", "list", "--all-remotes"]),
        at(&["tag", "list"]),
        at(&["config", "get", "revset-aliases.\"trunk()\""]),
    ]
}

/// `jj status` as the person runs it, with the line naming `@` itself
/// left out: its change id is random.
fn status(jj: &Path, person: &Person, folder: &Path) -> String {
    person
        .run_jj(jj, folder, &["status"])
        .lines()
        .filter(|line| !line.starts_with("Working copy  (@) :"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// tau's setup leaves the repository as `jj git init --colocate` leaves
/// a copy of the folder, the index and tau's workspace aside, and the
/// person's `jj status` works in the folder and shows their own working
/// copy, as in the copy.
#[hegel::test(
    test_cases = 20,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn setup_matches_jj_git_init_colocate(tc: TestCase) {
    let world = world(&tc, false, true);
    matches_jj_git_init_colocate(&world);
}

/// [`setup_matches_jj_git_init_colocate`]'s case where the person's
/// `auto-track-bookmarks` makes their branch follow `origin`'s, a
/// commit ahead: jj moves the branch, and so does tau.
#[test]
fn a_tracked_branch_follows_origin_as_jj_moves_it() {
    let world = World {
        commits: vec![vec![(0, 0)], vec![(0, 1)]],
        branches: vec![(1, 0)],
        tags: Vec::new(),
        head: HeadAt::Main,
        remotes: vec![
            Some(Remote {
                branches: vec![(1, 1)],
                head: None,
            }),
            None,
        ],
        edits: Vec::new(),
        abandon: true,
        auto_track: true,
        jj: None,
    };
    matches_jj_git_init_colocate(&world);
}

fn matches_jj_git_init_colocate(world: &World) {
    let Some(jj) = jj_cli::matching_jj() else {
        eprintln!(
            "the jj CLI is not installed at jj-lib's version: not compared"
        );
        return;
    };
    let base = tempfile::tempdir().unwrap();
    let person = Person::new(&base.path().join("person"));
    let folder = build(&person, base.path(), world, None);
    let copied = base.path().join("copy");
    copy(&folder, &copied);
    let found = found(&person, &folder);
    ProjectRepo::setup_local(
        &found,
        layout(base.path(), &folder),
        person.identity(),
        &mut go_on,
    )
    .unwrap();
    person.run_jj(&jj, &copied, &["git", "init", "--colocate"]);
    assert_eq!(
        jj_view(&jj, &person, &folder),
        jj_view(&jj, &person, &copied)
    );
    let gitignore = |dir: &Path| std::fs::read(dir.join(".jj/.gitignore")).ok();
    assert_eq!(gitignore(&folder), gitignore(&copied));
    let all = |dir: &Path| {
        let mut refs = refs(&person, dir, &[]);
        refs.retain(|name, _| !name.starts_with("refs/jj/"));
        (refs, std::fs::read(dir.join(".git/HEAD")).unwrap())
    };
    assert_eq!(all(&folder), all(&copied));
    let workspaces = person.run_jj(
        &jj,
        &folder,
        &[
            "--ignore-working-copy",
            "workspace",
            "list",
            "-T",
            "name ++ \"\\n\"",
        ],
    );
    assert_eq!(workspaces, "default\ntau-main\n");
    assert_eq!(status(&jj, &person, &folder), status(&jj, &person, &copied));
}

/// The person's steps and tau's, against the person's own folder.
struct Machine {
    _base: tempfile::TempDir,
    person: Person,
    folder: PathBuf,
    jj: Option<PathBuf>,
    abandon: bool,
    vcs: Vcs,
    main_dir: PathBuf,
    /// Each tau commit's number, for its file's content.
    commits: u32,
}

impl Machine {
    fn new(abandon: bool) -> Self {
        let base = tempfile::tempdir().unwrap();
        let person = Person::new(&base.path().join("person"));
        person.configure(abandon, false);
        let folder = base.path().join("repo");
        std::fs::create_dir_all(&folder).unwrap();
        person.git(&folder, &["init", "--quiet"]);
        write(&folder.join("a"), "first\n");
        person.git(&folder, &["add", "-A"]);
        person.git(&folder, &["commit", "--quiet", "-m", "first"]);
        let found = found(&person, &folder);
        let layout = layout(base.path(), &folder);
        let project = ProjectRepo::setup_local(
            &found,
            layout.clone(),
            person.identity(),
            &mut go_on,
        )
        .unwrap();
        let vcs = block_on_io(Vcs::open(project.main_dir(), person.identity()))
            .unwrap();
        Self {
            jj: jj_cli::matching_jj(),
            person,
            folder,
            abandon,
            vcs,
            main_dir: layout.main_dir,
            _base: base,
            commits: 0,
        }
    }

    fn head(&self) -> Vec<u8> {
        std::fs::read(self.folder.join(".git/HEAD")).unwrap()
    }

    fn index(&self) -> Option<Vec<u8>> {
        std::fs::read(self.folder.join(".git/index")).ok()
    }

    fn main(&self) -> Option<String> {
        self.person.try_git(
            &self.folder,
            &["rev-parse", "-q", "--verify", "refs/heads/main"],
        )
    }

    /// What tau's step must leave of `HEAD`, from `HEAD` before it and
    /// trunk before and after: as it was, or, when it was on trunk and
    /// trunk moved, detached on the commit it was at.
    fn expected_head(
        head: &[u8],
        before: Option<&str>,
        after: Option<&str>,
    ) -> Vec<u8> {
        if head == b"ref: refs/heads/main\n"
            && before != after
            && let Some(before) = before
        {
            return format!("{before}\n").into_bytes();
        }
        head.to_owned()
    }

    /// tau's step: `step` runs with the person's `HEAD` and index read
    /// before and checked after.
    fn taus(&mut self, step: impl FnOnce(&mut Self)) {
        let head = self.head();
        let index = self.index();
        let trunk = self.main();
        step(self);
        let now = self.main();
        assert_eq!(self.index(), index, "tau moved the index");
        assert_eq!(
            self.head(),
            Self::expected_head(&head, trunk.as_deref(), now.as_deref()),
            "tau moved HEAD"
        );
    }

    /// Whether jj sees `commit`, read without an import. A revset that
    /// names a hidden commit by its id would show it.
    fn visible(&self, jj: &Path, commit: &str) -> bool {
        self.person
            .run_jj(
                jj,
                &self.folder,
                &[
                    "--ignore-working-copy",
                    "log",
                    "--no-graph",
                    "-r",
                    "all()",
                    "-T",
                    "commit_id ++ \"\\n\"",
                ],
            )
            .lines()
            .any(|line| line == commit)
    }
}

#[hegel::state_machine]
impl Machine {
    /// tau commits on trunk from its workspace, caught up with trunk as
    /// Git has it first.
    #[rule(weight = 3)]
    fn tau_commits(&mut self, _tc: TestCase) {
        self.commits += 1;
        let k = self.commits;
        self.taus(|machine| {
            // An amend the person's settings do not abandon keeps tau's
            // commit beside theirs, one change twice: tau refuses to move
            // onto it, as it refuses any divergent change.
            let divergent = |error: &VcsError| {
                !machine.abandon
                    && matches!(
                        error,
                        VcsError::DivergentAfterLanding(_)
                            | VcsError::DivergentChange(_)
                    )
            };
            let trunk = machine.main().expect("trunk");
            match block_on_io(machine.vcs.move_onto(trunk, "main", true)) {
                Err(error) if divergent(&error) => return,
                moved => moved.unwrap(),
            };
            write(&machine.main_dir.join("t"), &format!("tau {k}\n"));
            let committed = match block_on_io(
                machine.vcs.commit_all(format!("tau {k}"), "main"),
            ) {
                Err(error) if divergent(&error) => return,
                committed => committed.unwrap(),
            };
            assert_eq!(
                machine.main().as_deref(),
                Some(committed.commit_id.as_str())
            );
        });
    }

    /// tau reads its workspace, which imports Git's refs.
    #[rule]
    fn tau_reads(&mut self, _tc: TestCase) {
        self.taus(|machine| {
            block_on_io(machine.vcs.working_copy()).unwrap();
        });
    }

    /// The person commits with git, on whatever `HEAD` is.
    #[rule(weight = 2)]
    fn person_commits(&mut self, tc: TestCase) {
        let content = tc.draw(gs::integers::<u8>().max_value(3));
        let file = FILES[index(&tc, FILES.len())];
        write(&self.folder.join(file), &format!("person {content}\n"));
        self.person.git(&self.folder, &["add", "-A"]);
        self.person.git(
            &self.folder,
            &["commit", "--quiet", "--allow-empty", "-m", "person"],
        );
    }

    /// The person checks out trunk, or detaches on it.
    #[rule]
    fn person_checks_out(&mut self, tc: TestCase) {
        let detach = tc.draw(gs::booleans());
        let mut args = vec!["checkout", "--quiet"];
        if detach {
            args.push("--detach");
        }
        args.push("main");
        let _ = self.person.try_git(&self.folder, &args);
    }

    /// The person amends trunk's commit, checked out, after tau
    /// imported it; tau's next import hides the old commit exactly when
    /// their `git.abandon-unreachable-commits` says so.
    #[rule]
    fn person_amends(&mut self, _tc: TestCase) {
        if self
            .person
            .try_git(&self.folder, &["checkout", "--quiet", "main"])
            .is_none()
        {
            return;
        }
        // tau sees trunk's commit before the person amends it.
        self.taus(|machine| {
            block_on_io(machine.vcs.working_copy()).unwrap();
        });
        let old = self.main().expect("trunk");
        self.commits += 1;
        self.person.git(
            &self.folder,
            &[
                "commit",
                "--quiet",
                "--amend",
                "--allow-empty",
                "-m",
                &format!("amended {}", self.commits),
            ],
        );
        self.taus(|machine| {
            block_on_io(machine.vcs.working_copy()).unwrap();
        });
        if let Some(jj) = self.jj.clone() {
            assert_eq!(
                self.visible(&jj, &old),
                !self.abandon,
                "{old} after the amend"
            );
        }
    }
}

/// tau's trunk commits and imports never move the person's `HEAD` but
/// by detaching it on trunk's commit, nor their index, whatever the
/// person does with git meanwhile: see the module docs.
#[hegel::test(
    test_cases = 20,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn the_persons_head_and_index_move_only_with_them(tc: TestCase) {
    if jj_cli::matching_jj().is_none() {
        eprintln!("no jj of jj-lib's version: amends are not checked");
    }
    let abandon = tc.draw(gs::booleans());
    hegel::stateful::machine(Machine::new(abandon))
        .steps(12)
        .run(tc);
}
