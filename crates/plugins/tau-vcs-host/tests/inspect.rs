//! Inspecting a folder before adding it as a local repository
//! (`docs/reference/vcs.md`, "Inspecting a folder") sees what `git`
//! sees, and changes nothing:
//!
//! - on random repositories built with the git CLI (commits, branches,
//!   a detached or unborn `HEAD`, changed, staged, untracked and ignored
//!   files, linked worktrees, subfolders, LFS attributes, gitlinks and
//!   `.gitmodules`, `origin` URLs of many forms), the root is `git
//!   rev-parse --show-toplevel`, a worktree's main checkout is the
//!   parent of `--git-common-dir`, the branch is `git symbolic-ref -q
//!   HEAD`'s, the uncommitted paths are `git status --porcelain`'s, and
//!   each refusal is the one git's own view of the folder calls for;
//! - inspecting leaves every file under the folders byte for byte as it
//!   was: `HEAD`, the index, refs and config included;
//! - on random jj repositories built with the jj CLI of jj-lib's version,
//!   colocated (`jj git init --colocate`, on a new folder or a Git
//!   checkout) or not (`--no-colocate`), with bookmarks, a remote and a
//!   `trunk()` of the person's or the repository's config, and other
//!   workspaces: a colocated root is found as git sees it, with the
//!   trunk `jj log -r 'trunk()'` names when `origin/HEAD` names none;
//!   a repository jj says is not colocated is refused with jj's hint;
//!   another workspace is refused for `jj workspace root --name
//!   default`, a subfolder for `jj workspace root`; and nothing changes;
//! - an `origin` URL names a GitHub repository exactly when it is one of
//!   the forms Git and GitHub take for one.
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
    path::{Path, PathBuf},
    process::{Command, Output},
};

use hegel::{TestCase, generators as gs};
use tau_vcs_host::{
    Identity,
    JjEnv,
    local::{self, Found, GitHub, Kind, Origin, Refusal, Trunk, TrunkFrom},
};

/// Files the worlds commit and change, some of whose names `git status`
/// would quote.
const FILES: [&str; 9] = [
    "a",
    "b.txt",
    "dir/c",
    "dir/sub/d",
    "with space",
    "quo\"te",
    "ünï.md",
    "dir2/e",
    "back\\slash",
];

/// Ignore patterns, one of which (`dir/sub/`) covers a tracked file.
const PATTERNS: [&str; 5] =
    ["*.log", "build/", "ignored*", "!keep.log", "dir/sub/"];

/// Files some pattern may ignore.
const IGNORED: [&str; 6] = [
    "x.log",
    "build/out.o",
    "ignored1",
    "keep.log",
    "dir/y.log",
    "dir/sub/z",
];

const BRANCHES: [&str; 4] = ["feature", "release/1.x", "fix-a", "v2"];

/// Subfolders a world may point at; `newdir` is made empty.
const SUBDIRS: [&str; 4] = ["dir", "dir/sub", "build", "newdir"];

#[derive(Debug, Clone, hegel::PrettyPrintable)]
enum Edit {
    /// Writes the file, making it when it is not there.
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
    /// `git rm --cached`.
    Unstage {
        file: usize,
    },
    /// `git mv`.
    Move {
        from: usize,
        to: usize,
    },
    /// Flips the executable bit.
    Chmod {
        file: usize,
    },
    /// Writes one of [`IGNORED`].
    Ignored {
        file: usize,
    },
    /// `git add -N`.
    IntentToAdd {
        file: usize,
    },
}

/// Where `HEAD` ends up.
#[derive(Debug, Clone, hegel::PrettyPrintable)]
enum HeadAt {
    Main,
    Branch(usize),
    Detached(usize),
    /// `git checkout --orphan`: unborn, with the index kept.
    Orphan,
}

/// Where an `origin/HEAD` points.
#[derive(Debug, Clone, hegel::PrettyPrintable)]
enum OriginHead {
    Absent,
    /// At `main`, or the branch of this index.
    Names(Option<usize>),
    /// At a remote branch that does not exist.
    Dangling,
}

/// A URL, and the GitHub repository it names, when it names one.
#[derive(Debug, Clone, hegel::PrettyPrintable)]
struct Url {
    url: String,
    github: Option<(String, String)>,
}

#[derive(Debug, Clone, hegel::PrettyPrintable)]
struct OriginPlan {
    url: Url,
    head: OriginHead,
}

/// Where a world asks for Git LFS, or seems to without asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, hegel::PrettyPrintable)]
enum Lfs {
    CommittedRoot,
    CommittedSub,
    StagedSub,
    /// The root `.gitattributes`, untracked.
    WorktreeRoot,
    InfoAttributes,
    LfsconfigCommitted,
    LfsconfigUntracked,
    /// `filter=lfs` in a comment, and filters by other names.
    Decoy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, hegel::PrettyPrintable)]
enum Submodule {
    GitlinkCommitted,
    GitlinkStaged,
    GitmodulesCommitted,
    GitmodulesUntracked,
}

/// The person's own excludes file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, hegel::PrettyPrintable)]
enum Global {
    /// `git/ignore` in the config directory.
    Xdg,
    /// What `core.excludesFile` in `~/.gitconfig` names.
    Key,
}

#[derive(Debug, Clone, hegel::PrettyPrintable)]
struct Ignores {
    /// `.gitignore`'s patterns, and whether the first commit has it.
    gitignore: Option<(Vec<usize>, bool)>,
    exclude: Vec<usize>,
    global: Option<(Vec<usize>, Global)>,
}

#[derive(Debug, Clone, hegel::PrettyPrintable)]
struct World {
    commits: Vec<Vec<(usize, u8)>>,
    branches: Vec<(usize, usize)>,
    head: HeadAt,
    ignores: Ignores,
    origin: Option<OriginPlan>,
    lfs: Vec<Lfs>,
    submodules: Vec<Submodule>,
    /// Linked worktrees, each inside the main checkout or beside it.
    worktrees: Vec<bool>,
    edits: Vec<Edit>,
}

fn index(tc: &TestCase, len: usize) -> usize {
    tc.draw(gs::integers::<usize>().max_value(len - 1))
}

/// One of `0..n`, each as likely: integers lean to their bounds, which
/// would make the first and last cases most of a run.
fn case(tc: &TestCase, n: u8) -> u8 {
    tc.draw(gs::sampled_from((0..n).collect::<Vec<_>>()))
}

#[hegel::composite]
fn edit(tc: &TestCase) -> Edit {
    let file = index(tc, FILES.len());
    match case(tc, 8) {
        0 => Edit::Write {
            file,
            content: tc.draw(gs::integers::<u8>().max_value(3)),
        },
        1 => Edit::Stage { file },
        2 => Edit::Remove { file },
        3 => Edit::Unstage { file },
        4 => Edit::Move {
            from: file,
            to: index(tc, FILES.len()),
        },
        5 => Edit::Chmod { file },
        6 => Edit::Ignored {
            file: index(tc, IGNORED.len()),
        },
        _ => Edit::IntentToAdd { file },
    }
}

fn patterns(tc: &TestCase) -> Vec<usize> {
    tc.draw(
        gs::vecs(gs::integers::<usize>().max_value(PATTERNS.len() - 1))
            .min_size(1)
            .max_size(3),
    )
}

#[hegel::composite]
fn ignores(tc: &TestCase) -> Ignores {
    Ignores {
        gitignore: tc
            .draw(gs::booleans())
            .then(|| (patterns(tc), tc.draw(gs::booleans()))),
        exclude: if tc.draw(gs::booleans()) {
            patterns(tc)
        } else {
            Vec::new()
        },
        global: tc.draw(gs::booleans()).then(|| {
            (
                patterns(tc),
                tc.draw(gs::sampled_from(vec![Global::Xdg, Global::Key])),
            )
        }),
    }
}

/// A GitHub owner: letters, digits and inner hyphens.
#[hegel::composite]
fn owner(tc: &TestCase) -> String {
    tc.draw(
        gs::from_regex("[A-Za-z0-9]([A-Za-z0-9-]{0,6}[A-Za-z0-9])?")
            .fullmatch(true),
    )
}

/// A GitHub repository name, never `.`, `..` or ending in `.git`.
#[hegel::composite]
fn repo_name(tc: &TestCase) -> String {
    let name: String = tc.draw(
        gs::from_regex("[A-Za-z0-9_][A-Za-z0-9._-]{0,9}").fullmatch(true),
    );
    if name.ends_with(".git") {
        format!("{name}x")
    } else {
        name
    }
}

/// A URL on GitHub in one of the forms Git takes, or one that is not on
/// GitHub, with what it names.
#[hegel::composite]
fn url(tc: &TestCase) -> Url {
    let owner = tc.draw(owner());
    let name = tc.draw(repo_name());
    let suffix = if tc.draw(gs::booleans()) { ".git" } else { "" };
    let path = format!("{owner}/{name}{suffix}");
    let host = tc.draw(gs::sampled_from(vec![
        "github.com",
        "GitHub.com",
        "www.github.com",
    ]));
    let on_github = |url: String| Url {
        url,
        github: Some((owner.clone(), name.clone())),
    };
    let elsewhere = |url: String| Url { url, github: None };
    match case(tc, 12) {
        0 => {
            let slash = if tc.draw(gs::booleans()) { "/" } else { "" };
            on_github(format!("https://{host}/{path}{slash}"))
        }
        1 => on_github(format!("https://x-access-token:t0k@{host}/{path}")),
        2 => on_github(format!("http://{host}/{path}")),
        3 => on_github(format!("git://github.com/{path}")),
        4 => on_github(format!("git@github.com:{path}")),
        5 => on_github(format!("ssh://git@github.com/{path}")),
        6 => on_github(format!("ssh://git@ssh.github.com:443/{path}")),
        7 => on_github(format!("org-1@{host}:{path}")),
        8 => {
            let other = tc.draw(gs::sampled_from(vec![
                "gitlab.com",
                "github.com.evil.example",
                "notgithub.com",
                "github.co",
                "gist.github.com",
            ]));
            elsewhere(format!("https://{other}/{path}"))
        }
        9 => elsewhere(format!("https://github.com/{path}/extra")),
        10 => elsewhere(format!("https://github.com/{owner}")),
        _ => {
            let local = tc.draw(gs::sampled_from(vec![
                "/srv/github.com/",
                "file:///github.com/",
                "../",
            ]));
            elsewhere(format!("{local}{path}"))
        }
    }
}

#[hegel::composite]
fn world(tc: &TestCase) -> World {
    // A repository with no commits now and then; most have some.
    let fewest = usize::from(case(tc, 5) != 0);
    let commits: Vec<Vec<(usize, u8)>> = tc.draw(
        gs::vecs(
            gs::vecs(gs::tuples!(
                gs::integers::<usize>().max_value(FILES.len() - 1),
                gs::integers::<u8>().max_value(3)
            ))
            .max_size(4),
        )
        .min_size(fewest)
        .max_size(4),
    );
    let branches = tc.draw(
        gs::vecs(gs::tuples!(
            gs::integers::<usize>().max_value(BRANCHES.len() - 1),
            gs::integers::<usize>().max_value(3)
        ))
        .max_size(3),
    );
    let head = match case(tc, 6) {
        0..=2 => HeadAt::Main,
        3 => HeadAt::Branch(index(tc, BRANCHES.len())),
        4 => HeadAt::Detached(tc.draw(gs::integers::<usize>().max_value(3))),
        _ => HeadAt::Orphan,
    };
    let origin = tc.draw(gs::booleans()).then(|| OriginPlan {
        url: tc.draw(url()),
        head: match case(tc, 3) {
            0 => OriginHead::Absent,
            1 => OriginHead::Names(tc.draw(gs::optional(
                gs::integers::<usize>().max_value(BRANCHES.len() - 1),
            ))),
            _ => OriginHead::Dangling,
        },
    });
    // Most worlds use neither, so the other checks are reached.
    let lfs = if case(tc, 6) == 0 {
        tc.draw(
            gs::vecs(gs::sampled_from(vec![
                Lfs::CommittedRoot,
                Lfs::CommittedSub,
                Lfs::StagedSub,
                Lfs::WorktreeRoot,
                Lfs::InfoAttributes,
                Lfs::LfsconfigCommitted,
                Lfs::LfsconfigUntracked,
                Lfs::Decoy,
                Lfs::Decoy,
            ]))
            .min_size(1)
            .max_size(2),
        )
    } else {
        Vec::new()
    };
    let submodules = if case(tc, 6) == 0 {
        tc.draw(
            gs::vecs(gs::sampled_from(vec![
                Submodule::GitlinkCommitted,
                Submodule::GitlinkStaged,
                Submodule::GitmodulesCommitted,
                Submodule::GitmodulesUntracked,
            ]))
            .min_size(1)
            .max_size(2),
        )
    } else {
        Vec::new()
    };
    let worktrees: Vec<bool> = tc.draw(gs::vecs(gs::booleans()).max_size(2));
    let edits = tc.draw(gs::vecs(edit()).max_size(8));
    World {
        commits,
        branches,
        head,
        ignores: tc.draw(ignores()),
        origin,
        lfs,
        submodules,
        worktrees,
        edits,
    }
}

/// A person: their home and config directory, which `git` and the
/// inspection read.
struct Person {
    root: PathBuf,
}

impl Person {
    fn new(root: &Path) -> Self {
        for dir in ["home", "config/git"] {
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

    /// `git` as this person runs it in `dir`: their global config and
    /// excludes, no system config.
    fn git_command(&self, dir: &Path) -> Command {
        let mut command = tau_testing::git::command(dir);
        command
            .env_remove("GIT_CONFIG_GLOBAL")
            .env("HOME", self.home())
            .env("XDG_CONFIG_HOME", self.config());
        command
    }

    fn output(&self, dir: &Path, args: &[&str]) -> Output {
        self.git_command(dir).args(args).output().expect("git runs")
    }

    /// `git args` in `dir`, which must succeed; what it printed, trimmed.
    fn git(&self, dir: &Path, args: &[&str]) -> String {
        let output = self.output(dir, args);
        assert!(
            output.status.success(),
            "git {args:?} in {dir:?}: {output:?}"
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    /// `git args` in `dir`, when it succeeds.
    fn try_git(&self, dir: &Path, args: &[&str]) -> Option<String> {
        let output = self.output(dir, args);
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn lines(indices: &[usize]) -> String {
    indices
        .iter()
        .map(|&i| format!("{}\n", PATTERNS[i]))
        .collect()
}

/// What a `.gitattributes` holds for each place.
fn attributes(place: Lfs) -> &'static str {
    match place {
        Lfs::Decoy => {
            "# *.bin filter=lfs\n*.psd filter=lfsx\n*.txt text -filter\n"
        }
        _ => "*.bin filter=lfs diff=lfs merge=lfs -text\n",
    }
}

/// A gitlink's commit, which need not exist.
const GITLINK: &str = "0123456789abcdef0123456789abcdef01234567";

/// The world's folders, built with the git CLI.
struct Built {
    /// The folders to inspect: the checkout and folders in it, its
    /// worktrees, and folders in no checkout.
    targets: Vec<PathBuf>,
}

fn build(person: &Person, base: &Path, world: &World) -> Built {
    let main = base.join("main");
    std::fs::create_dir_all(&main).unwrap();
    person.git(&main, &["init", "--quiet"]);
    let ignores = &world.ignores;
    if let Some((patterns, _)) = &ignores.gitignore {
        write(&main.join(".gitignore"), &lines(patterns));
    }
    if !ignores.exclude.is_empty() {
        write(&main.join(".git/info/exclude"), &lines(&ignores.exclude));
    }
    match &ignores.global {
        Some((patterns, Global::Xdg)) => {
            write(&person.config().join("git/ignore"), &lines(patterns));
        }
        Some((patterns, Global::Key)) => {
            write(&person.home().join("my-ignore"), &lines(patterns));
            write(
                &person.home().join(".gitconfig"),
                "[core]\n\texcludesFile = ~/my-ignore\n",
            );
        }
        None => {}
    }
    let mut commits = Vec::new();
    for (k, writes) in world.commits.iter().enumerate() {
        let mut paths = Vec::new();
        for &(file, content) in writes {
            write(&main.join(FILES[file]), &format!("{k}-{content}\n"));
            paths.push(FILES[file]);
        }
        if k == 0 && matches!(ignores.gitignore, Some((_, true))) {
            paths.push(".gitignore");
        }
        if !paths.is_empty() {
            let mut args = vec!["add", "-f", "--"];
            args.extend(paths);
            person.git(&main, &args);
        }
        person.git(
            &main,
            &["commit", "--quiet", "--allow-empty", "-m", &format!("c{k}")],
        );
        commits.push(person.git(&main, &["rev-parse", "HEAD"]));
    }
    if !commits.is_empty() {
        for &(branch, at) in &world.branches {
            person.git(
                &main,
                &[
                    "branch",
                    "-f",
                    BRANCHES[branch],
                    &commits[at % commits.len()],
                ],
            );
        }
    }
    // What a commit of its own brings in.
    let mut extra = Vec::new();
    for place in &world.lfs {
        match place {
            Lfs::CommittedRoot => {
                write(&main.join(".gitattributes"), attributes(*place));
                extra.push(".gitattributes");
            }
            Lfs::CommittedSub => {
                write(&main.join("dir/.gitattributes"), attributes(*place));
                extra.push("dir/.gitattributes");
            }
            Lfs::LfsconfigCommitted => {
                write(&main.join(".lfsconfig"), "[lfs]\n\turl = x\n");
                extra.push(".lfsconfig");
            }
            _ => {}
        }
    }
    for place in &world.submodules {
        match place {
            Submodule::GitlinkCommitted => {
                let info = format!("160000,{GITLINK},lib/sub");
                person.git(
                    &main,
                    &["update-index", "--add", "--cacheinfo", &info],
                );
            }
            Submodule::GitmodulesCommitted => {
                write(
                    &main.join(".gitmodules"),
                    "[submodule \"a\"]\n\tpath = mods/a\n\turl = ../a\n",
                );
                extra.push(".gitmodules");
            }
            _ => {}
        }
    }
    let gitlink = world.submodules.contains(&Submodule::GitlinkCommitted);
    if !extra.is_empty() || gitlink {
        if !extra.is_empty() {
            let mut args = vec!["add", "-f", "--"];
            args.extend(extra);
            person.git(&main, &args);
        }
        person.git(&main, &["commit", "--quiet", "-m", "extra"]);
        commits.push(person.git(&main, &["rev-parse", "HEAD"]));
    }
    if let Some(origin) = &world.origin {
        person.git(&main, &["remote", "add", "origin", &origin.url.url]);
        if let Some(first) = commits.first() {
            let named = |branch: &str| format!("refs/remotes/origin/{branch}");
            for branch in std::iter::once("main").chain(BRANCHES) {
                person.git(&main, &["update-ref", &named(branch), first]);
            }
            let head = match origin.head {
                OriginHead::Absent => None,
                OriginHead::Names(None) => Some(named("main")),
                OriginHead::Names(Some(branch)) => {
                    Some(named(BRANCHES[branch]))
                }
                OriginHead::Dangling => Some(named("gone")),
            };
            if let Some(head) = head {
                person.git(
                    &main,
                    &["symbolic-ref", "refs/remotes/origin/HEAD", &head],
                );
            }
        }
    }
    match world.head {
        HeadAt::Main => {}
        HeadAt::Branch(branch) => {
            // A branch the world has; a new one at HEAD when it has none.
            let name = BRANCHES[branch];
            if person
                .try_git(&main, &["rev-parse", "--verify", "-q", name])
                .is_some()
            {
                person.git(&main, &["checkout", "--quiet", "-f", name]);
            } else {
                person.git(&main, &["checkout", "--quiet", "-b", name]);
            }
        }
        HeadAt::Detached(at) => {
            if !commits.is_empty() {
                let at = &commits[at % commits.len()];
                person
                    .git(&main, &["checkout", "--quiet", "-f", "--detach", at]);
            }
        }
        HeadAt::Orphan => {
            person.git(&main, &["checkout", "--quiet", "--orphan", "orphan"]);
        }
    }
    for place in &world.lfs {
        match place {
            Lfs::StagedSub => {
                write(&main.join("dir2/.gitattributes"), attributes(*place));
                person.git(&main, &["add", "-f", "--", "dir2/.gitattributes"]);
            }
            Lfs::WorktreeRoot | Lfs::Decoy => {
                write(&main.join(".gitattributes"), attributes(*place));
            }
            Lfs::InfoAttributes => {
                write(&main.join(".git/info/attributes"), attributes(*place));
            }
            Lfs::LfsconfigUntracked => {
                write(&main.join(".lfsconfig"), "[lfs]\n\turl = x\n");
            }
            _ => {}
        }
    }
    for place in &world.submodules {
        match place {
            Submodule::GitlinkStaged => {
                let info = format!("160000,{GITLINK},vendor/x");
                person.git(
                    &main,
                    &["update-index", "--add", "--cacheinfo", &info],
                );
            }
            Submodule::GitmodulesUntracked => {
                write(
                    &main.join(".gitmodules"),
                    "[submodule \"b\"]\n\tpath = mods/b\n\turl = ../b\n",
                );
            }
            _ => {}
        }
    }
    let mut worktrees = Vec::new();
    if let Some(first) = commits.first() {
        for (k, &inside) in world.worktrees.iter().enumerate() {
            let path = if inside {
                main.join(format!("nested-wt{k}"))
            } else {
                base.join(format!("wt{k}"))
            };
            person.git(
                &main,
                &[
                    "worktree",
                    "add",
                    "--quiet",
                    "--detach",
                    path.to_str().unwrap(),
                    first,
                ],
            );
            worktrees.push(path);
        }
    }
    for edit in &world.edits {
        apply(person, &main, edit);
    }
    // Every kind of folder there is to point at, each made here.
    let mut targets =
        vec![main.clone(), main.join(".git"), main.join(".git/refs")];
    for dir in SUBDIRS {
        std::fs::create_dir_all(main.join(dir)).unwrap();
        targets.push(main.join(dir));
    }
    for worktree in &worktrees {
        std::fs::create_dir_all(worktree.join("in")).unwrap();
        targets.extend([worktree.clone(), worktree.join("in")]);
    }
    let plain = base.join("plain");
    write(&plain.join("notes.txt"), "notes\n");
    let file = base.join("file.txt");
    write(&file, "file\n");
    let bare = base.join("bare.git");
    std::fs::create_dir_all(&bare).unwrap();
    person.git(&bare, &["init", "--quiet", "--bare"]);
    targets.extend([
        plain,
        file,
        base.join("missing"),
        bare.join("refs"),
        bare,
    ]);
    Built { targets }
}

fn apply(person: &Person, main: &Path, edit: &Edit) {
    let path = |file: usize| main.join(FILES[file]);
    let tracked = |file: usize| {
        person
            .try_git(main, &["ls-files", "--error-unmatch", "--", FILES[file]])
            .is_some()
    };
    match *edit {
        Edit::Write { file, content } => {
            write(&path(file), &format!("edit-{content}\n"));
        }
        Edit::Stage { file } => {
            if path(file).exists() || tracked(file) {
                person.git(main, &["add", "-f", "--", FILES[file]]);
            }
        }
        Edit::Remove { file } => {
            let _ = std::fs::remove_file(path(file));
        }
        Edit::Unstage { file } => {
            if tracked(file) {
                person.git(
                    main,
                    &["rm", "--cached", "--quiet", "--", FILES[file]],
                );
            }
        }
        Edit::Move { from, to } => {
            if from != to
                && tracked(from)
                && path(from).exists()
                && !path(to).exists()
            {
                std::fs::create_dir_all(path(to).parent().unwrap()).unwrap();
                person.git(main, &["mv", "-f", "--", FILES[from], FILES[to]]);
            }
        }
        Edit::Chmod { file } => {
            use std::os::unix::fs::PermissionsExt as _;
            if let Ok(metadata) = std::fs::metadata(path(file)) {
                let mode = metadata.permissions().mode() ^ 0o111;
                std::fs::set_permissions(
                    path(file),
                    std::fs::Permissions::from_mode(mode),
                )
                .unwrap();
            }
        }
        Edit::Ignored { file } => {
            write(&main.join(IGNORED[file]), "ignored\n");
        }
        Edit::IntentToAdd { file } => {
            if path(file).exists() && !tracked(file) {
                person.git(main, &["add", "-f", "-N", "--", FILES[file]]);
            }
        }
    }
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap()
}

/// Whether the attributes `text` set `filter=lfs`: a line, not a
/// comment, with that word.
fn sets_lfs(text: &str) -> bool {
    text.lines().any(|line| {
        !line.trim_start().starts_with('#')
            && line.split_whitespace().any(|word| word == "filter=lfs")
    })
}

/// Where git says the repository at `root` asks for LFS: the rule, read
/// through the git CLI.
fn git_lfs(person: &Person, root: &Path) -> bool {
    let at_head = person
        .try_git(root, &["ls-tree", "-r", "--name-only", "HEAD"])
        .unwrap_or_default();
    let in_head = at_head
        .lines()
        .filter(|path| path.rsplit('/').next() == Some(".gitattributes"))
        .any(|path| {
            sets_lfs(&person.git(root, &["show", &format!("HEAD:{path}")]))
        });
    let staged = person.git(root, &["ls-files", "-s"]);
    let in_index = staged.lines().any(|line| {
        let (meta, path) = line.split_once('\t').unwrap();
        path.rsplit('/').next() == Some(".gitattributes")
            && sets_lfs(&person.git(
                root,
                &["cat-file", "blob", meta.split(' ').nth(1).unwrap()],
            ))
    });
    let git_dir =
        PathBuf::from(person.git(root, &["rev-parse", "--absolute-git-dir"]));
    let files = [root.join(".gitattributes"), git_dir.join("info/attributes")];
    let on_disk = files.iter().any(|file| {
        std::fs::read_to_string(file).is_ok_and(|text| sets_lfs(&text))
    });
    let lfsconfig = root.join(".lfsconfig").exists()
        || at_head.lines().any(|path| path == ".lfsconfig");
    in_head || in_index || on_disk || lfsconfig
}

/// The submodules' paths git sees in the repository at `root`, when it
/// has any: gitlinks in `HEAD` and the index, and `.gitmodules`' paths.
fn git_submodules(person: &Person, root: &Path) -> Option<Vec<String>> {
    let mut paths = std::collections::BTreeSet::new();
    let tree = person
        .try_git(root, &["ls-tree", "-r", "--full-tree", "HEAD"])
        .unwrap_or_default();
    for line in tree
        .lines()
        .chain(person.git(root, &["ls-files", "-s"]).lines())
    {
        if line.starts_with("160000 ") {
            paths.insert(line.split_once('\t').unwrap().1.to_owned());
        }
    }
    let modules = if root.join(".gitmodules").exists() {
        Some(
            person
                .try_git(
                    root,
                    &[
                        "config",
                        "-f",
                        ".gitmodules",
                        "--get-regexp",
                        "^submodule\\..*\\.path$",
                    ],
                )
                .unwrap_or_default(),
        )
    } else {
        person
            .try_git(root, &["cat-file", "-e", "HEAD:.gitmodules"])
            .map(|_| {
                person
                    .try_git(
                        root,
                        &[
                            "config",
                            "--blob",
                            "HEAD:.gitmodules",
                            "--get-regexp",
                            "^submodule\\..*\\.path$",
                        ],
                    )
                    .unwrap_or_default()
            })
    };
    if let Some(modules) = &modules {
        for line in modules.lines() {
            paths.insert(line.split_once(' ').unwrap().1.to_owned());
        }
    }
    (modules.is_some() || !paths.is_empty())
        .then(|| paths.into_iter().collect())
}

/// `git status --porcelain`'s paths in `root`, renames off, without
/// refreshing the index.
fn git_status(person: &Person, root: &Path) -> Vec<String> {
    let output = person.output(
        root,
        &[
            "--no-optional-locks",
            "status",
            "--porcelain",
            "-z",
            "--no-renames",
        ],
    );
    assert!(output.status.success(), "{output:?}");
    let mut paths: Vec<String> = output
        .stdout
        .split(|&byte| byte == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| String::from_utf8_lossy(&entry[3..]).into_owned())
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

/// What inspecting `target` should give, from git's own view of it.
fn expected(
    person: &Person,
    world: &World,
    target: &Path,
) -> Result<Found, Refusal> {
    if !target.exists() {
        return Err(Refusal::Missing(target.to_owned()));
    }
    if target.is_file() {
        return Err(Refusal::NotAFolder(target.to_owned()));
    }
    let folder = canonical(target);
    let Some(view) = person.try_git(
        target,
        &[
            "rev-parse",
            "--is-bare-repository",
            "--is-inside-git-dir",
            "--absolute-git-dir",
            "--git-common-dir",
        ],
    ) else {
        return Err(Refusal::NotARepository(folder));
    };
    let view: Vec<&str> = view.lines().collect();
    let git_dir = canonical(Path::new(view[2]));
    if view[0] == "true" {
        return Err(Refusal::Bare(git_dir));
    }
    let common = canonical(&target.join(view[3]));
    if git_dir != common {
        let main = (common.file_name().unwrap() == ".git")
            .then(|| common.parent().unwrap().to_owned());
        return Err(Refusal::Worktree { main });
    }
    if view[1] == "true" {
        return Err(Refusal::Inside {
            root: git_dir.parent().unwrap().to_owned(),
        });
    }
    let root = canonical(Path::new(
        &person.git(target, &["rev-parse", "--show-toplevel"]),
    ));
    if folder != root {
        return Err(Refusal::Inside { root });
    }
    if git_lfs(person, &root) {
        return Err(Refusal::Lfs {
            found_in: String::new(),
        });
    }
    if let Some(paths) = git_submodules(person, &root) {
        return Err(Refusal::Submodules { paths });
    }
    let branch = person
        .try_git(&root, &["symbolic-ref", "-q", "HEAD"])
        .map(|name| name.strip_prefix("refs/heads/").unwrap().to_owned());
    let origin_head = person
        .try_git(
            &root,
            &[
                "rev-parse",
                "-q",
                "--verify",
                "--symbolic-full-name",
                "refs/remotes/origin/HEAD",
            ],
        )
        .and_then(|name| {
            Some(name.strip_prefix("refs/remotes/origin/")?.to_owned())
        });
    let trunk = match origin_head {
        Some(branch) => Some(Trunk {
            branch,
            from: TrunkFrom::OriginHead,
        }),
        None => branch.clone().map(|branch| Trunk {
            branch,
            from: TrunkFrom::CheckedOut,
        }),
    };
    let origin = person
        .try_git(&root, &["config", "--get", "remote.origin.url"])
        .map(|url| Origin {
            github: world
                .origin
                .as_ref()
                .and_then(|origin| origin.url.github.clone())
                .map(|(owner, name)| GitHub { owner, name }),
            url,
        });
    let commits = person
        .try_git(&root, &["rev-list", "--count", "HEAD"])
        .map_or(0, |count| count.parse().unwrap());
    Ok(Found {
        kind: Kind::Git,
        root: root.clone(),
        branch,
        origin,
        commits,
        uncommitted: git_status(person, &root),
        trunk,
    })
}

/// Where LFS was found is the inspection's to word.
fn without_lfs_place(result: Result<Found, Refusal>) -> Result<Found, Refusal> {
    result.map_err(|refusal| match refusal {
        Refusal::Lfs { .. } => Refusal::Lfs {
            found_in: String::new(),
        },
        other => other,
    })
}

/// Every file under `dir`, with what it holds: a link's target, or a
/// file's bytes and mode.
fn files(dir: &Path) -> BTreeMap<PathBuf, (u32, Vec<u8>)> {
    use std::os::unix::fs::PermissionsExt as _;
    let mut files = BTreeMap::new();
    let mut pending = vec![dir.to_owned()];
    while let Some(next) = pending.pop() {
        for entry in std::fs::read_dir(&next).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                files.insert(path.clone(), (0, Vec::new()));
                pending.push(path);
            } else if kind.is_symlink() {
                let target = std::fs::read_link(&path).unwrap();
                files.insert(
                    path,
                    (1, target.into_os_string().into_encoded_bytes()),
                );
            } else {
                let mode = entry.metadata().unwrap().permissions().mode();
                files.insert(
                    path.clone(),
                    (mode, std::fs::read(&path).unwrap()),
                );
            }
        }
    }
    files
}

/// Inspecting any folder of any world finds what git sees there, or
/// refuses it as git's view of it calls for.
///
/// The case kept: `dir/`'s one tracked file is deleted and a new file
/// is beside it. git lists the new file alone, since `dir` holds a
/// tracked path; gix's own collapsing listed `dir/`.
///
/// Also kept: a file only the person's own excludes file ignores, found
/// in either place git looks for it.
#[hegel::test(
    test_cases = 50,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
#[hegel::explicit_test_case(world = World {
    commits: vec![vec![(2, 0)]],
    edits: vec![Edit::Remove { file: 2 }, Edit::Ignored { file: 4 }],
    ..World::default()
})]
#[hegel::explicit_test_case(world = World {
    commits: vec![vec![(0, 0)]],
    ignores: Ignores { gitignore: None, exclude: vec![], global: Some((vec![0], Global::Xdg)) },
    edits: vec![Edit::Ignored { file: 0 }],
    ..World::default()
})]
#[hegel::explicit_test_case(world = World {
    commits: vec![vec![(0, 0)]],
    ignores: Ignores { gitignore: None, exclude: vec![], global: Some((vec![1], Global::Key)) },
    edits: vec![Edit::Ignored { file: 1 }],
    ..World::default()
})]
fn inspection_sees_what_git_sees(tc: TestCase) {
    let world = tc.draw(world());
    check_git(&world);
}

/// A repository that asks for LFS or has submodules, in any of the
/// places tau looks, is refused as git's view of it calls for: each
/// world here has one or both.
#[hegel::test(
    test_cases = 50,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn lfs_and_submodules_are_refused_where_git_has_them(tc: TestCase) {
    let mut world = tc.draw(world());
    let lfs = || {
        gs::vecs(gs::sampled_from(vec![
            Lfs::CommittedRoot,
            Lfs::CommittedSub,
            Lfs::StagedSub,
            Lfs::WorktreeRoot,
            Lfs::InfoAttributes,
            Lfs::LfsconfigCommitted,
            Lfs::LfsconfigUntracked,
            Lfs::Decoy,
        ]))
        .min_size(1)
        .max_size(2)
    };
    let submodules = || {
        gs::vecs(gs::sampled_from(vec![
            Submodule::GitlinkCommitted,
            Submodule::GitlinkStaged,
            Submodule::GitmodulesCommitted,
            Submodule::GitmodulesUntracked,
        ]))
        .min_size(1)
        .max_size(2)
    };
    match case(&tc, 3) {
        0 => world.lfs = tc.draw(lfs()),
        1 => world.submodules = tc.draw(submodules()),
        _ => {
            world.lfs = tc.draw(lfs());
            world.submodules = tc.draw(submodules());
        }
    }
    check_git(&world);
}

/// Every folder of `world`, inspected, against git's view of it.
fn check_git(world: &World) {
    let base = tempfile::tempdir().unwrap();
    let person = Person::new(&base.path().join("person"));
    let built = build(&person, base.path(), world);
    for target in &built.targets {
        let found = local::inspect_blocking(target, &person.identity());
        let expected = expected(&person, world, target);
        assert_eq!(without_lfs_place(found), expected, "{}", target.display());
    }
}

/// Inspecting changes nothing on disk, in the checkout, its `.git` and
/// its worktrees: `HEAD`, the index, refs and config are byte-equal
/// before and after, though `git status` would refresh the index.
#[hegel::test(
    test_cases = 50,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn inspection_changes_nothing(tc: TestCase) {
    let world = tc.draw(world());
    let base = tempfile::tempdir().unwrap();
    let person = Person::new(&base.path().join("person"));
    let built = build(&person, base.path(), &world);
    let before = files(base.path());
    for target in &built.targets {
        let found = local::inspect_blocking(target, &person.identity());
        tc.note(&format!("{}: {found:?}", target.display()));
    }
    assert!(before == files(base.path()), "inspecting wrote");
}

/// An `origin` URL names a GitHub repository exactly when it is one of
/// the forms Git takes for one: HTTPS, HTTP, `git://`, SSH and
/// `user@host:owner/name`, on `github.com`, with or without `.git`.
#[hegel::test(test_cases = 500)]
fn origin_names_a_github_repository_in_any_form(tc: TestCase) {
    let url = tc.draw(url());
    let expected = url
        .github
        .clone()
        .map(|(owner, name)| GitHub { owner, name });
    assert_eq!(local::github(&url.url), expected, "{}", url.url);
}

/// What a `trunk()` alias names.
#[derive(Debug, Clone, Copy, hegel::PrettyPrintable)]
enum Alias {
    /// A local bookmark, by its index.
    Local(usize),
    /// A bookmark on `origin`, by its index.
    Remote(usize),
    /// A bookmark the repository lacks: jj falls back to the root.
    Missing,
}

/// How a jj repository is laid out: each world is built in each.
#[derive(Debug, Clone, Copy)]
enum Layout {
    /// `jj git init --colocate` in a new folder.
    Colocated,
    /// `jj git init --colocate` in a Git checkout with a commit.
    OnGit,
    /// `jj git init --no-colocate`.
    Internal,
}

#[derive(Debug, Clone, hegel::PrettyPrintable)]
struct JjWorld {
    /// Each commit's files, and the bookmark of this index made at it.
    commits: Vec<(Vec<usize>, Option<usize>)>,
    /// Another bookmark at the last commit.
    also: Option<usize>,
    /// The bookmarks pushed to an `origin`, when there is one.
    pushed: Option<Vec<usize>>,
    /// A `trunk()`, in the repository's config or the person's.
    alias: Option<(Alias, bool)>,
    /// Whether `origin/HEAD` names the first pushed bookmark.
    origin_head: bool,
    /// Files written after the last commit.
    edits: Vec<usize>,
}

/// jj's bookmarks here, some of which `trunk()` knows by name.
const BOOKMARKS: [&str; 4] = ["main", "trunk", "feature", "release/1.x"];

#[hegel::composite]
fn jj_world(tc: &TestCase) -> JjWorld {
    let bookmark = || gs::integers::<usize>().max_value(BOOKMARKS.len() - 1);
    JjWorld {
        commits: tc.draw(
            gs::vecs(gs::tuples!(
                gs::vecs(gs::integers::<usize>().max_value(FILES.len() - 1))
                    .max_size(3),
                gs::optional(bookmark())
            ))
            .max_size(4),
        ),
        also: tc.draw(gs::optional(bookmark())),
        pushed: tc
            .draw(gs::booleans())
            .then(|| tc.draw(gs::vecs(bookmark()).max_size(3))),
        alias: tc.draw(gs::booleans()).then(|| {
            let alias = match case(tc, 3) {
                0 => Alias::Local(index(tc, BOOKMARKS.len())),
                1 => Alias::Remote(index(tc, BOOKMARKS.len())),
                _ => Alias::Missing,
            };
            (alias, tc.draw(gs::booleans()))
        }),
        origin_head: tc.draw(gs::booleans()),
        edits: tc.draw(
            gs::vecs(gs::integers::<usize>().max_value(FILES.len() - 1))
                .max_size(3),
        ),
    }
}

impl Person {
    /// The jj CLI as this person runs it in `dir`: no other variable of
    /// this process but `PATH`, for `git`.
    fn jj(&self, jj: &Path, dir: &Path, args: &[&str]) -> Output {
        Command::new(jj)
            .args(args)
            .current_dir(dir)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.home())
            .env("XDG_CONFIG_HOME", self.config())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("JJ_USER", "t")
            .env("JJ_EMAIL", "t@t")
            .output()
            .expect("jj runs")
    }

    /// `jj args` in `dir`, which must succeed; what it printed, trimmed.
    fn run_jj(&self, jj: &Path, dir: &Path, args: &[&str]) -> String {
        let output = self.jj(jj, dir, args);
        assert!(
            output.status.success(),
            "jj {args:?} in {dir:?}: {output:?}"
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }
}

/// The folders to inspect, laid out as `layout` says: the root, a
/// folder in it, and a second workspace.
fn build_jj(
    jj: &Path,
    person: &Person,
    base: &Path,
    world: &JjWorld,
    layout: Layout,
) -> Vec<PathBuf> {
    let root = base.join("repo");
    std::fs::create_dir_all(&root).unwrap();
    if matches!(layout, Layout::OnGit) {
        person.git(&root, &["init", "--quiet"]);
        write(&root.join("a"), "git\n");
        person.git(&root, &["add", "a"]);
        person.git(&root, &["commit", "--quiet", "-m", "git"]);
    }
    let mode = if matches!(layout, Layout::Internal) {
        "--no-colocate"
    } else {
        "--colocate"
    };
    person.run_jj(jj, &root, &["git", "init", mode]);
    for (k, (files, bookmark)) in world.commits.iter().enumerate() {
        for &file in files {
            write(&root.join(FILES[file]), &format!("jj-{k}\n"));
        }
        person.run_jj(jj, &root, &["commit", "-m", &format!("c{k}")]);
        if let Some(bookmark) = bookmark {
            person.run_jj(
                jj,
                &root,
                &[
                    "bookmark",
                    "set",
                    "--allow-backwards",
                    BOOKMARKS[*bookmark],
                    "-r",
                    "@-",
                ],
            );
        }
    }
    if let Some(also) = world.also
        && !world.commits.is_empty()
    {
        person.run_jj(
            jj,
            &root,
            &[
                "bookmark",
                "set",
                "--allow-backwards",
                BOOKMARKS[also],
                "-r",
                "@-",
            ],
        );
    }
    if let Some(pushed) = &world.pushed {
        let origin = base.join("origin.git");
        std::fs::create_dir_all(&origin).unwrap();
        person.git(&origin, &["init", "--quiet", "--bare"]);
        person.run_jj(
            jj,
            &root,
            &["git", "remote", "add", "origin", origin.to_str().unwrap()],
        );
        let listed = person.run_jj(
            jj,
            &root,
            &["bookmark", "list", "-T", "name ++ \"\\n\""],
        );
        let mut first = None;
        for &bookmark in pushed {
            let name = BOOKMARKS[bookmark];
            if listed.lines().any(|line| line == name) {
                person.run_jj(jj, &root, &["git", "push", "--bookmark", name]);
                first.get_or_insert(name);
            }
        }
        if !matches!(layout, Layout::Internal)
            && world.origin_head
            && let Some(first) = first
        {
            person.git(
                &root,
                &[
                    "symbolic-ref",
                    "refs/remotes/origin/HEAD",
                    &format!("refs/remotes/origin/{first}"),
                ],
            );
        }
    }
    let mut targets = vec![root.clone()];
    let sub = root.join("dir");
    std::fs::create_dir_all(&sub).unwrap();
    targets.push(sub);
    {
        let second = base.join("second");
        person.run_jj(
            jj,
            &root,
            &["workspace", "add", second.to_str().unwrap()],
        );
        targets.push(second);
    }
    // Brings jj's view up to date before the edits, which stay out of it.
    person.run_jj(jj, &root, &["status"]);
    for &file in &world.edits {
        write(&root.join(FILES[file]), "edit\n");
    }
    // Last: a `trunk()` naming nothing breaks the commands that check
    // out a commit.
    if let Some((alias, in_repo)) = world.alias {
        let value = match alias {
            // TOML literal strings, holding quoted revset symbols.
            Alias::Local(bookmark) => format!("'\"{}\"'", BOOKMARKS[bookmark]),
            Alias::Remote(bookmark) => {
                format!("'\"{}\"@origin'", BOOKMARKS[bookmark])
            }
            Alias::Missing => "nosuch".to_owned(),
        };
        let level = if in_repo { "--repo" } else { "--user" };
        person.run_jj(
            jj,
            &root,
            &[
                "--ignore-working-copy",
                "config",
                "set",
                level,
                "revset-aliases.\"trunk()\"",
                &value,
            ],
        );
    }
    targets
}

/// What inspecting `target` in a jj world should give, from jj's view
/// of it and git's.
fn expected_jj(
    jj: &Path,
    person: &Person,
    target: &Path,
) -> Result<Found, Refusal> {
    let folder = canonical(target);
    let at = |args: &[&str]| {
        let mut all = vec!["--ignore-working-copy"];
        all.extend(args);
        person.run_jj(jj, target, &all)
    };
    let root = canonical(Path::new(&at(&["workspace", "root"])));
    let main =
        canonical(Path::new(&at(&["workspace", "root", "--name", "default"])));
    if root != main {
        return Err(Refusal::Worktree { main: Some(main) });
    }
    if folder != root {
        return Err(Refusal::Inside { root });
    }
    let status = at(&["git", "colocation", "status"]);
    if status.contains("not colocated") {
        return Err(Refusal::JjNotColocated {
            root,
            hint: local::COLOCATE_COMMAND,
        });
    }
    assert!(status.contains("is currently colocated"), "{status}");
    let mut found = expected(person, &World::default(), target)?;
    found.kind = Kind::Jj;
    found.origin = found.origin.map(|origin| Origin {
        github: None,
        ..origin
    });
    if found
        .trunk
        .as_ref()
        .is_none_or(|trunk| trunk.from != TrunkFrom::OriginHead)
    {
        let names = at(&[
            "log",
            "-r",
            "trunk() ~ root()",
            "--no-graph",
            "-T",
            "local_bookmarks.map(|b| b.name() ++ \"\\n\").join(\"\") ++ \
             remote_bookmarks.filter(|b| b.remote() != \"git\")\
             .map(|b| b.name() ++ \"\\n\").join(\"\")",
        ]);
        if let Some(branch) = names.lines().min() {
            found.trunk = Some(Trunk {
                branch: branch.to_owned(),
                from: TrunkFrom::JjTrunk,
            });
        }
    }
    Ok(found)
}

impl Default for World {
    /// A world with nothing in it: what [`expected`] reads of a world
    /// that has no `origin` of its drawing.
    fn default() -> Self {
        Self {
            commits: Vec::new(),
            branches: Vec::new(),
            head: HeadAt::Main,
            ignores: Ignores {
                gitignore: None,
                exclude: Vec::new(),
                global: None,
            },
            origin: None,
            lfs: Vec::new(),
            submodules: Vec::new(),
            worktrees: Vec::new(),
            edits: Vec::new(),
        }
    }
}

/// Inspecting a folder of a jj repository finds what jj and git see
/// there, or refuses it as jj's view of it calls for, and changes
/// nothing on disk. Each world is built in each layout.
///
/// The case kept: two bookmarks at the commit `trunk()` names, the
/// first by name winning.
#[hegel::test(
    test_cases = 20,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
#[hegel::explicit_test_case(world = JjWorld {
    commits: vec![(vec![0], Some(0))],
    also: Some(2),
    pushed: Some(vec![0]),
    alias: None,
    origin_head: false,
    edits: vec![],
})]
fn inspection_sees_what_jj_sees(tc: TestCase) {
    let Some(jj) = jj_cli::matching_jj() else {
        eprintln!(
            "the jj CLI is not installed at jj-lib's version: not compared"
        );
        return;
    };
    let world = tc.draw(jj_world());
    for layout in [Layout::Colocated, Layout::OnGit, Layout::Internal] {
        let base = tempfile::tempdir().unwrap();
        let person = Person::new(&base.path().join("person"));
        let targets = build_jj(&jj, &person, base.path(), &world, layout);
        let before = files(base.path());
        let found: Vec<_> = targets
            .iter()
            .map(|target| local::inspect_blocking(target, &person.identity()))
            .collect();
        assert!(
            before == files(base.path()),
            "inspecting wrote ({layout:?})"
        );
        for (target, found) in targets.iter().zip(found) {
            let expected = expected_jj(&jj, &person, target);
            assert_eq!(found, expected, "{} ({layout:?})", target.display());
        }
    }
}
