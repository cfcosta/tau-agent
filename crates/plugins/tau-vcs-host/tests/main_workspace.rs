//! The main chat's workspace (`docs/reference/vcs.md`, "Projects" and
//! "A repository's main chat"; ADR 0043, "Where things live"): it is
//! `tau-main`, in `main/`, and jj's `default` workspace is never tau's.
//!
//! - A project made before, whose main chat worked in `default`, is
//!   renamed as it opens. The old layout is built for real: by the jj
//!   CLI's `jj workspace rename default` when it is installed, else by
//!   the same steps through jj-lib, with work left uncommitted in
//!   `main/` and staged in Git's index. Opening it keeps the
//!   working-copy commit, the files, Git's `HEAD` and index and
//!   `.jj/tau-moved-onto`, and leaves no `default`; runs then go on as
//!   in a twin project that never had one (metamorphic).
//! - A rename cut off between its steps is finished by the next open,
//!   to the same end.
//! - The jj CLI of jj-lib's version lists `tau-main` and works in
//!   `main/`. tau's `forget_workspace` leaves the workspace store as `jj
//!   workspace remove` does, and as `jj workspace forget` does but for
//!   the forgotten name, which `forget` keeps on purpose, for `jj undo`.
//! - A sweep never forgets a workspace the person made with `jj
//!   workspace add`, wherever it is, nor deletes its directory.
//!
//! Where that CLI is not installed, the tests that need it say they did
//! not run. Each world lives in a temporary directory and reads no real
//! config.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

#[path = "common/jj_cli.rs"]
mod jj_cli;
#[path = "common/ops.rs"]
mod ops;

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    process::Command,
};

use hegel::{
    TestCase,
    generators::{self as gs, Generator as _},
};
use jj_lib::{
    default_backend_factories::{
        default_backend_factories,
        default_working_copy_factories,
    },
    object_id::ObjectId as _,
    ref_name::{WorkspaceName, WorkspaceNameBuf},
    simple_workspace_store::SimpleWorkspaceStore,
    workspace::Workspace,
    workspace_store::WorkspaceStore as _,
};
use pollster::block_on;
use tau_testing::{block_on_io, git::git};
use tau_vcs_host::{
    Identity,
    JjEnv,
    MAIN_WORKSPACE,
    ProjectRepo,
    Vcs,
    sweep::{Keep, Owner, Standing},
};

const DEFAULT: &str = "default";
const PATHS: [&str; 3] = ["a.txt", "c.txt", "dir/b.txt"];

/// A source repository, and the config every project and the jj CLI
/// read here.
struct World {
    _dir: tempfile::TempDir,
    root: PathBuf,
    identity: Identity,
}

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        for sub in ["home", "config", "src/dir"] {
            std::fs::create_dir_all(root.join(sub)).unwrap();
        }
        let src = root.join("src");
        git(&src, &["init", "--quiet"]);
        std::fs::write(src.join("a.txt"), "one\n").unwrap();
        std::fs::write(src.join("dir/b.txt"), "two\n").unwrap();
        git(&src, &["add", "."]);
        git(&src, &["commit", "--quiet", "-m", "first"]);
        let identity = Identity {
            name: "Tau".into(),
            email: "tau@localhost".into(),
            env: JjEnv {
                config_dir: Some(root.join("config")),
                home_dir: Some(root.join("home")),
                system_dir: None,
                hostname: Some("desk".into()),
                username: Some("person".into()),
                vars: HashMap::new(),
            },
        };
        Self {
            _dir: dir,
            root,
            identity,
        }
    }

    /// A project imported from the source, at `name`, with its main chat
    /// caught up with trunk as the host does before its first turn,
    /// which writes `.jj/tau-moved-onto`.
    fn import(&self, name: &str) -> ProjectRepo {
        let project = ProjectRepo::import(
            self.root.join("src").to_str().unwrap(),
            self.root.join(name),
            self.identity.clone(),
        )
        .unwrap();
        let main = self.vcs(project.main_dir());
        let trunk = project.trunk().unwrap();
        block_on_io(main.move_onto(trunk, project.trunk_name().unwrap(), true))
            .unwrap();
        project
    }

    fn open(&self, name: &str) -> ProjectRepo {
        ProjectRepo::open(self.root.join(name), self.identity.clone()).unwrap()
    }

    fn vcs(&self, dir: &Path) -> Vcs {
        block_on_io(Vcs::open(dir, self.identity.clone())).unwrap()
    }

    /// The jj CLI in `dir`, with this world's home and config and no
    /// other variable of this process; it must succeed.
    fn jj(&self, jj: &Path, dir: &Path, args: &[&str]) -> String {
        let output = Command::new(jj)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("JJ_USER", "Person")
            .env("JJ_EMAIL", "person@localhost")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "jj {args:?}: {output:?}");
        String::from_utf8(output.stdout).unwrap()
    }

    /// The main workspace at `main`, loaded with this world's settings.
    fn workspace(&self, main: &Path) -> Workspace {
        let settings = tau_vcs_host::jj_settings(
            &self.identity,
            main,
            &main.join(".jj").join("repo"),
        )
        .unwrap();
        Workspace::load(
            &settings,
            main,
            &default_backend_factories(),
            &default_working_copy_factories(),
        )
        .unwrap()
    }

    /// The view's working-copy commits by workspace, as jj-lib reads
    /// them in `main`.
    fn view(&self, main: &Path) -> BTreeMap<String, String> {
        let workspace = self.workspace(main);
        let repo = block_on(workspace.repo_loader().load_at_head()).unwrap();
        repo.view()
            .wc_commit_ids()
            .iter()
            .map(|(name, id)| (name.as_str().to_owned(), id.hex()))
            .collect()
    }

    /// The working copy's own name for its workspace, in `main`.
    fn named(&self, main: &Path) -> String {
        self.workspace(main).workspace_name().as_str().to_owned()
    }

    /// Renames `main`'s workspace `from` to `to` as jj-cli 0.46.0's `jj
    /// workspace rename` does, its steps in its order under the working
    /// copy's lock (the workspace store, then the view's operation,
    /// then the working copy), stopping after `steps` of the three: what
    /// a rename cut off there leaves.
    fn rename_by_hand(&self, main: &Path, from: &str, to: &str, steps: u8) {
        let (from, to) = (WorkspaceName::new(from), WorkspaceNameBuf::from(to));
        let mut workspace = self.workspace(main);
        let repo = block_on(workspace.repo_loader().load_at_head()).unwrap();
        let store = repo.loader().workspace_store().clone();
        let mut locked =
            block_on(workspace.start_working_copy_mutation()).unwrap();
        let mut op = locked.locked_wc().old_operation_id().clone();
        if steps >= 1 {
            store.rename(from, &to).unwrap();
        }
        if steps >= 2 {
            let mut tx = repo.start_transaction();
            tx.repo_mut().rename_workspace(from, to.clone()).unwrap();
            let renamed = block_on(tx.commit("rename by hand")).unwrap();
            if steps >= 3 {
                op = renamed.op_id().clone();
                locked.locked_wc().rename_workspace(to);
            }
        }
        block_on(locked.finish(op)).unwrap();
    }
}

/// Where the workspace store of the repository in `main` has each of
/// `names`, from `root`.
fn store(
    main: &Path,
    root: &Path,
    names: &BTreeSet<String>,
) -> BTreeMap<String, Option<PathBuf>> {
    let store = SimpleWorkspaceStore::load(
        &main.join(".jj").join("repo").join("workspace_store"),
    )
    .unwrap();
    names
        .iter()
        .map(|name| {
            let path = store
                .get_workspace_path(WorkspaceName::new(name))
                .unwrap()
                .map(|path| path.strip_prefix(root).unwrap().to_owned());
            (name.clone(), path)
        })
        .collect()
}

/// Every file under `dir`, but jj's and Git's own, with its content.
fn files(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut found = BTreeMap::new();
    let mut next = vec![dir.to_owned()];
    while let Some(at) = next.pop() {
        for entry in std::fs::read_dir(&at).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let name = entry.file_name();
            if name == ".jj" || name == ".git" {
                continue;
            }
            if entry.file_type().unwrap().is_dir() {
                next.push(path);
            } else {
                let relative = path.strip_prefix(dir).unwrap();
                found.insert(
                    relative.to_string_lossy().into_owned(),
                    std::fs::read(&path).unwrap(),
                );
            }
        }
    }
    found
}

/// `content` with each conflict marker line cut to its marker: the
/// labels after it name commits and changes, whose ids differ between
/// twins.
fn unlabelled(content: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for line in content.split_inclusive(|byte| *byte == b'\n') {
        let marker = line.len() >= 7
            && b"<>+-%\\".contains(&line[0])
            && line[..7].iter().all(|byte| *byte == line[0])
            && line
                .get(7)
                .is_none_or(|byte| *byte == b' ' || *byte == b'\n');
        if marker {
            out.extend_from_slice(&line[..7]);
            out.push(b'\n');
        } else {
            out.extend_from_slice(line);
        }
    }
    out
}

fn write(dir: &Path, file: &str, text: &str) {
    let path = dir.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// What the main workspace in `main/` holds that its rename must keep:
/// its working-copy commit, its files, Git's `HEAD` and index, and the
/// commit it last moved onto.
#[derive(Debug, PartialEq, Eq)]
struct Kept {
    commit: String,
    files: BTreeMap<String, Vec<u8>>,
    head: String,
    index: Vec<u8>,
    moved_onto: Option<Vec<u8>>,
}

impl Kept {
    fn read(main: &Path, commit: String) -> Self {
        Self {
            commit,
            files: files(main),
            head: git(main, &["rev-parse", "HEAD"]),
            index: std::fs::read(main.join(".git").join("index")).unwrap(),
            moved_onto: std::fs::read(main.join(".jj").join("tau-moved-onto"))
                .ok(),
        }
    }
}

/// Asserts the project at `name`, opened once, is renamed: `tau-main`
/// holds what `default` held, in the view, the store and the working
/// copy, and `default` is gone; the runs' workspaces are as they were;
/// what `kept` read is unchanged. `renamed` says whether the view still
/// had to be: then that took one operation, tau's, else none. Opening
/// again writes nothing.
fn assert_renamed(
    world: &World,
    name: &str,
    before: &BTreeMap<String, String>,
    kept: &Kept,
    renamed: bool,
) -> ProjectRepo {
    let main = world.root.join(name).join("main");
    let mut log = ops::OpLog::open(&main.join(".jj").join("repo"));
    log.new_ops();
    let project = world.open(name);
    assert_eq!(project.main_workspace(), MAIN_WORKSPACE);
    assert_eq!(project.main_dir(), main);
    let written = log.new_ops();
    assert_eq!(written.len(), usize::from(renamed), "{written:?}");
    for op in &written {
        ops::assert_taus(op, "the rename");
        assert_eq!(
            op.metadata().description,
            "tau: name the main chat's workspace"
        );
    }
    let mut expected = before.clone();
    let commit = expected
        .remove(DEFAULT)
        .or_else(|| expected.get(MAIN_WORKSPACE).cloned())
        .unwrap();
    expected.insert(MAIN_WORKSPACE.to_owned(), commit.clone());
    assert_eq!(world.view(&main), expected);
    assert_eq!(world.named(&main), MAIN_WORKSPACE);
    let names = BTreeSet::from([DEFAULT.to_owned(), MAIN_WORKSPACE.to_owned()]);
    assert_eq!(
        store(&main, &world.root, &names),
        BTreeMap::from([
            (DEFAULT.to_owned(), None),
            (
                MAIN_WORKSPACE.to_owned(),
                Some(Path::new(name).join("main"))
            ),
        ])
    );
    assert_eq!(&Kept::read(&main, commit), kept);
    // Done once, opening again does nothing.
    let again = world.open(name);
    assert_eq!(again.main_workspace(), MAIN_WORKSPACE);
    assert!(log.new_ops().is_empty(), "a second open wrote");
    project
}

/// Work left in the main chat's checkout: files written, and maybe one
/// staged in Git's index.
#[derive(Debug, Clone, hegel::PrettyPrintable)]
struct Uncommitted {
    writes: Vec<(String, String)>,
    staged: bool,
}

#[hegel::composite]
fn uncommitted(tc: &TestCase) -> Uncommitted {
    let writes = tc.draw(
        gs::vecs(gs::tuples2(
            gs::sampled_from(PATHS.to_vec()).map(str::to_owned),
            gs::from_regex("[a-z]{1,5}\n").fullmatch(true),
        ))
        .max_size(3),
    );
    Uncommitted {
        writes,
        staged: tc.draw(gs::booleans()),
    }
}

impl Uncommitted {
    fn apply(&self, main: &Path) {
        for (file, text) in &self.writes {
            write(main, file, text);
        }
        if self.staged
            && let Some((file, _)) = self.writes.first()
        {
            git(main, &["add", file]);
        }
    }
}

/// What a run does once the project opened.
#[derive(Debug, Clone, hegel::PrettyPrintable)]
enum Step {
    MainWrite {
        file: String,
        text: String,
    },
    MainCommit,
    AddRun,
    RunWrite {
        run: usize,
        file: String,
        text: String,
    },
    RunCommit {
        run: usize,
    },
    /// The run's newest commit lands on the main chat.
    Land {
        run: usize,
    },
    Forget {
        run: usize,
    },
}

#[hegel::composite]
fn step(tc: &TestCase) -> Step {
    let file = || tc.draw(gs::sampled_from(PATHS.to_vec())).to_owned();
    let text = || tc.draw(gs::from_regex("[a-z]{0,4}\n?").fullmatch(true));
    let run = || tc.draw(gs::integers::<usize>().max_value(3));
    match tc.draw(gs::integers::<u8>().max_value(6)) {
        0 => Step::MainWrite {
            file: file(),
            text: text(),
        },
        1 => Step::MainCommit,
        2 => Step::AddRun,
        3 => Step::RunWrite {
            run: run(),
            file: file(),
            text: text(),
        },
        4 => Step::RunCommit { run: run() },
        5 => Step::Land { run: run() },
        _ => Step::Forget { run: run() },
    }
}

/// A project and its runs, as one twin drives them.
struct Twin {
    project: ProjectRepo,
    main: Vcs,
    /// Each run's name and `Vcs`, until it is forgotten.
    runs: Vec<(String, Option<Vcs>)>,
}

impl Twin {
    fn new(world: &World, project: ProjectRepo, runs: usize) -> Self {
        let main = world.vcs(project.main_dir());
        let mut twin = Self {
            project,
            main,
            runs: Vec::new(),
        };
        for (at, file) in PATHS.iter().enumerate().take(runs) {
            twin.step(&Step::AddRun).unwrap();
            twin.step(&Step::RunWrite {
                run: at,
                file: (*file).to_owned(),
                text: format!("run {at}\n"),
            })
            .unwrap();
            twin.step(&Step::RunCommit { run: at }).unwrap();
        }
        twin
    }

    fn run(&self, at: usize) -> Option<(&str, &Vcs)> {
        let (name, vcs) = self.runs.get(at)?;
        Some((name, vcs.as_ref()?))
    }

    /// Takes `step`; says what it came to, for the twins to agree on.
    fn step(&mut self, step: &Step) -> Result<(), String> {
        let trunk_name = self.project.trunk_name().unwrap();
        let result = match step {
            Step::MainWrite { file, text } => {
                write(self.project.main_dir(), file, text);
                Ok(())
            }
            Step::MainCommit => {
                block_on_io(self.main.commit_all("feat: main", trunk_name))
                    .map(drop)
            }
            Step::AddRun => {
                let name = format!("r{}", self.runs.len());
                let trunk = self.project.trunk().unwrap();
                let vcs = self.project.add_workspace(&name, &trunk).unwrap();
                self.runs.push((name, Some(vcs)));
                Ok(())
            }
            Step::RunWrite { run, file, text } => {
                if let Some((_, vcs)) = self.run(*run) {
                    write(vcs.root(), file, text);
                }
                Ok(())
            }
            Step::RunCommit { run } => match self.run(*run) {
                Some((name, vcs)) => block_on_io(vcs.commit_all(
                    format!("feat: {name}"),
                    format!("tau/{name}"),
                ))
                .map(drop),
                None => Ok(()),
            },
            Step::Land { run } => match self.run(*run) {
                Some((name, _)) => {
                    match self.project.bookmark(&format!("tau/{name}")).unwrap()
                    {
                        Some(head) => {
                            block_on_io(self.main.land(head, trunk_name, true))
                                .map(drop)
                        }
                        None => Ok(()),
                    }
                }
                None => Ok(()),
            },
            Step::Forget { run } => {
                if let Some((name, vcs)) = self.runs.get_mut(*run)
                    && vcs.take().is_some()
                {
                    self.project.forget_workspace(name).unwrap();
                }
                Ok(())
            }
        };
        result.map_err(|error| error.to_string())
    }

    /// What the twins must agree on: trunk's files, the runs'
    /// workspaces, and the files in each one and in `main/`. Git's
    /// `HEAD` in `main/` follows the main chat's `@`.
    fn seen(&self) -> Seen {
        let project = &self.project;
        let trunk = project.trunk().unwrap();
        let wc = project
            .workspace_head(project.main_workspace())
            .unwrap()
            .unwrap();
        let parent = project.parent_of(&wc).unwrap().unwrap();
        assert_eq!(
            git(project.main_dir(), &["rev-parse", "HEAD"]),
            parent,
            "Git's HEAD follows the main chat"
        );
        let dirs = std::iter::once(project.main_dir().to_owned())
            .chain(
                self.runs
                    .iter()
                    .filter(|(_, vcs)| vcs.is_some())
                    .map(|(name, _)| project.workspace_dir(name)),
            )
            .map(|dir| {
                files(&dir)
                    .into_iter()
                    .map(|(path, content)| (path, unlabelled(&content)))
                    .collect()
            })
            .collect();
        (
            project
                .files_under(&trunk, "")
                .unwrap()
                .into_iter()
                .map(|(path, content)| (path, unlabelled(&content)))
                .collect(),
            project.workspaces().unwrap(),
            dirs,
        )
    }
}

type Files = BTreeMap<String, Vec<u8>>;

/// Trunk's files, the runs' workspaces, and the files in `main/` and in
/// each run's.
type Seen = (Vec<(String, Vec<u8>)>, Vec<String>, Vec<Files>);

/// An old project, whose main chat worked in jj's `default` workspace,
/// opens renamed, keeping what `main/` held, and goes on as a twin that
/// never had a `default` does.
#[hegel::test(
    test_cases = 20,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn an_old_project_opens_renamed_and_goes_on_as_a_new_one(tc: TestCase) {
    let jj = jj_cli::matching_jj();
    let by_cli = jj.is_some() && tc.draw(gs::booleans());
    let runs: usize = tc.draw(gs::integers().max_value(2));
    let work = tc.draw(uncommitted());
    let steps: Vec<Step> = tc.draw(gs::vecs(step()).max_size(8));
    let world = World::new();
    let old = world.import("old");
    let new = world.import("new");
    let main = old.main_dir().to_owned();
    drop(Twin::new(&world, old, runs));
    let mut new = Twin::new(&world, new, runs);
    // The old layout, as a project made before had it.
    match &jj {
        Some(jj) if by_cli => {
            world.jj(jj, &main, &["workspace", "rename", DEFAULT]);
        }
        _ => world.rename_by_hand(&main, MAIN_WORKSPACE, DEFAULT, 3),
    }
    let before = world.view(&main);
    assert!(
        before.contains_key(DEFAULT) && !before.contains_key(MAIN_WORKSPACE)
    );
    assert_eq!(world.named(&main), DEFAULT);
    work.apply(&main);
    work.apply(new.project.main_dir());
    let kept = Kept::read(&main, before[DEFAULT].clone());

    let old = assert_renamed(&world, "old", &before, &kept, true);
    let mut old = Twin {
        main: world.vcs(old.main_dir()),
        runs: new
            .runs
            .iter()
            .map(|(name, _)| {
                let vcs = world.vcs(&old.workspace_dir(name));
                (name.clone(), Some(vcs))
            })
            .collect(),
        project: old,
    };
    assert_eq!(old.seen(), new.seen());
    for step in &steps {
        let (was, is) = (old.step(step), new.step(step));
        assert_eq!(was.is_ok(), is.is_ok(), "{step:?}: {was:?} {is:?}");
        assert_eq!(old.seen(), new.seen(), "after {step:?}");
    }
    if let Some(jj) = &jj {
        // The CLI sees it as tau does, and works in it.
        let listed = world.jj(
            jj,
            &main,
            &[
                "workspace",
                "list",
                "-T",
                r#"name ++ "\t" ++ self.target().commit_id() ++ "\n""#,
            ],
        );
        let listed: BTreeMap<String, String> = listed
            .lines()
            .map(|line| {
                let (name, id) = line.split_once('\t').unwrap();
                (name.to_owned(), id.to_owned())
            })
            .collect();
        assert_eq!(listed, world.view(&main));
        assert!(
            listed.contains_key(MAIN_WORKSPACE)
                && !listed.contains_key(DEFAULT)
        );
        let status = world.jj(jj, &main, &["status"]);
        assert!(status.contains("Working copy"), "{status}");
        assert_eq!(world.named(&main), MAIN_WORKSPACE);
    }
    tc.event_value("steps", steps.len() as f64);
}

/// A rename cut off after any of its steps is finished by the next
/// open, to the end a whole one reaches.
#[hegel::test(
    test_cases = 20,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn a_rename_cut_off_is_finished_by_the_next_open(tc: TestCase) {
    let runs: usize = tc.draw(gs::integers().max_value(1));
    let work = tc.draw(uncommitted());
    let cut: u8 = tc.draw(gs::integers().max_value(3));
    let world = World::new();
    let project = world.import("p");
    let main = project.main_dir().to_owned();
    drop(Twin::new(&world, project, runs));
    world.rename_by_hand(&main, MAIN_WORKSPACE, DEFAULT, 3);
    let before = world.view(&main);
    work.apply(&main);
    let kept = Kept::read(&main, before[DEFAULT].clone());
    // Cut off: the store renamed, then the view too, then all.
    world.rename_by_hand(&main, DEFAULT, MAIN_WORKSPACE, cut);
    let renamed = cut < 2;
    let project = assert_renamed(&world, "p", &before, &kept, renamed);
    // Runs go on.
    let mut twin = Twin {
        main: world.vcs(project.main_dir()),
        runs: Vec::new(),
        project,
    };
    for step in [
        Step::MainCommit,
        Step::AddRun,
        Step::RunWrite {
            run: 0,
            file: "c.txt".into(),
            text: "x\n".into(),
        },
        Step::RunCommit { run: 0 },
        Step::Land { run: 0 },
    ] {
        let _ = twin.step(&step);
        twin.seen();
    }
    tc.event_value("cut", f64::from(cut));
}

/// A project with runs `r<n>` that tau made and workspaces `p<n>` that
/// the person added with the jj CLI.
struct Shared {
    world: World,
    project: ProjectRepo,
    runs: BTreeSet<String>,
    persons: BTreeMap<String, PathBuf>,
    /// How many workspaces were added: the next one's number.
    made: usize,
}

/// Where the person puts a workspace of theirs.
#[derive(Debug, Clone, Copy, hegel::PrettyPrintable)]
enum Place {
    /// Beside `main/`, in the project's directory.
    Beside,
    /// In tau's runs directory, under a name of the directory's own.
    InRuns,
    /// Outside the project.
    Outside,
}

impl Shared {
    fn new() -> Self {
        let world = World::new();
        let project = world.import("p");
        Self {
            world,
            project,
            runs: BTreeSet::new(),
            persons: BTreeMap::new(),
            made: 0,
        }
    }

    fn add_run(&mut self) -> String {
        let name = format!("r{}", self.made);
        self.made += 1;
        let trunk = self.project.trunk().unwrap();
        self.project.add_workspace(&name, &trunk).unwrap();
        self.runs.insert(name.clone());
        name
    }

    fn add_persons_workspace(
        &mut self,
        jj: &Path,
        place: Place,
        colocate: bool,
    ) {
        let n = self.made;
        self.made += 1;
        let name = format!("p{n}");
        let dir = match place {
            Place::Beside => self.world.root.join("p").join(&name),
            Place::InRuns => {
                self.world.root.join("p/runs").join(format!("x{n}"))
            }
            Place::Outside => self.world.root.join("elsewhere").join(&name),
        };
        std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
        let flag = if colocate {
            "--colocate"
        } else {
            "--no-colocate"
        };
        self.world.jj(
            jj,
            self.project.main_dir(),
            &[
                "workspace",
                "add",
                "--name",
                &name,
                flag,
                dir.to_str().unwrap(),
            ],
        );
        self.persons.insert(name, dir);
    }

    /// Every workspace's name.
    fn names(&self) -> BTreeSet<String> {
        self.runs
            .iter()
            .chain(self.persons.keys())
            .cloned()
            .chain([MAIN_WORKSPACE.to_owned()])
            .collect()
    }

    /// The project holds what the model says: its view names the main
    /// chat's workspace, the runs' and the person's; `workspaces` lists
    /// the runs'; every directory of theirs is there.
    fn check(&self, what: &str) {
        let main = self.project.main_dir();
        let view: BTreeSet<String> =
            self.world.view(main).into_keys().collect();
        assert_eq!(view, self.names(), "{what}: the view");
        assert_eq!(
            self.project.workspaces().unwrap(),
            self.runs.iter().cloned().collect::<Vec<_>>(),
            "{what}: the runs' workspaces"
        );
        for (name, dir) in &self.persons {
            assert!(dir.join(".jj").is_dir(), "{what}: {name}'s directory");
        }
        for name in &self.runs {
            assert!(
                self.project.workspace_dir(name).join(".jj").is_dir(),
                "{what}: {name}'s directory"
            );
        }
        assert!(main.join(".jj").is_dir(), "{what}: main/");
    }
}

/// tau forgetting a run's workspace leaves the workspace store as `jj
/// workspace remove` leaves it, and as `jj workspace forget` does but
/// for the name it forgot, which `forget` keeps.
#[hegel::test(
    test_cases = 10,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn tau_forgets_a_run_as_jj_workspace_remove_does(tc: TestCase) {
    let Some(jj) = jj_cli::matching_jj() else {
        eprintln!("no jj CLI of jj-lib's version: did not compare");
        return;
    };
    // Runs and the person's workspaces, interleaved: `true` is a run.
    let adds: Vec<(bool, Place, bool)> = tc.draw(
        gs::vecs(gs::tuples3(
            gs::booleans(),
            gs::sampled_from(vec![
                Place::Beside,
                Place::InRuns,
                Place::Outside,
            ]),
            gs::booleans(),
        ))
        .min_size(1)
        .max_size(4),
    );
    let pick: usize = tc.draw(gs::integers().max_value(3));
    let worlds: Vec<Shared> = (0..3)
        .map(|_| {
            let mut shared = Shared::new();
            let mut first = true;
            for (run, place, colocate) in &adds {
                if *run || first {
                    shared.add_run();
                    first = false;
                } else {
                    shared.add_persons_workspace(&jj, *place, *colocate);
                }
            }
            shared
        })
        .collect();
    let runs: Vec<&String> = worlds[0].runs.iter().collect();
    let gone = runs[pick % runs.len()].clone();
    let names = worlds[0].names();
    let [by_tau, by_remove, by_forget] = &worlds[..] else {
        unreachable!()
    };
    by_tau.project.forget_workspace(&gone).unwrap();
    let main = |shared: &Shared| shared.project.main_dir().to_owned();
    by_tau
        .world
        .jj(&jj, &main(by_remove), &["workspace", "remove", &gone]);
    by_tau
        .world
        .jj(&jj, &main(by_forget), &["workspace", "forget", &gone]);
    let stores: Vec<_> = worlds
        .iter()
        .map(|shared| store(&main(shared), &shared.world.root, &names))
        .collect();
    assert_eq!(
        stores[0], stores[1],
        "tau's forget and `jj workspace remove`"
    );
    assert_eq!(stores[0][&gone], None);
    let mut forgotten = stores[2].clone();
    assert!(forgotten[&gone].is_some(), "`jj workspace forget` keeps it");
    forgotten.insert(gone.clone(), None);
    assert_eq!(
        stores[0], forgotten,
        "tau's forget and `jj workspace forget`"
    );
    // The CLI lists the same workspaces in each.
    let listed: Vec<String> = worlds
        .iter()
        .map(|shared| {
            shared.world.jj(
                &jj,
                &main(shared),
                &["workspace", "list", "-T", r#"name ++ "\n""#],
            )
        })
        .collect();
    assert_eq!(listed[0], listed[1]);
    assert_eq!(listed[0], listed[2]);
    assert!(!listed[0].lines().any(|name| name == gone));
    for shared in &worlds[..2] {
        assert!(!shared.project.workspace_dir(&gone).exists());
    }
}

/// One step of the person's and tau's in a shared project.
#[derive(Debug, Clone, hegel::PrettyPrintable)]
enum Turn {
    Run,
    Person {
        place: Place,
        colocate: bool,
    },
    /// The sweep, with each run's standing: kept, landed, discarded or
    /// no owner at all.
    Sweep(Vec<u8>),
    /// `forget_workspace` of one of the names, the person's included.
    Forget(usize),
}

#[hegel::composite]
fn turn(tc: &TestCase) -> Turn {
    match tc.draw(gs::integers::<u8>().max_value(3)) {
        0 => Turn::Run,
        1 => Turn::Person {
            place: tc.draw(gs::sampled_from(vec![
                Place::Beside,
                Place::InRuns,
                Place::Outside,
            ])),
            colocate: tc.draw(gs::booleans()),
        },
        2 => Turn::Sweep(
            tc.draw(gs::vecs(gs::integers::<u8>().max_value(3)).max_size(6)),
        ),
        _ => Turn::Forget(tc.draw(gs::integers::<usize>().max_value(8))),
    }
}

/// A sweep, and `forget_workspace`, never take a workspace the person
/// made, wherever they put it, nor the main chat's; the runs' go as the
/// sweep's rule says.
#[hegel::test(
    test_cases = 15,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn a_sweep_never_forgets_the_persons_workspaces(tc: TestCase) {
    let Some(jj) = jj_cli::matching_jj() else {
        eprintln!("no jj CLI of jj-lib's version: did not compare");
        return;
    };
    let steps: Vec<Turn> = tc.draw(gs::vecs(turn()).min_size(1).max_size(8));
    let mut shared = Shared::new();
    for step in &steps {
        match step {
            Turn::Run => {
                shared.add_run();
            }
            Turn::Person { place, colocate } => {
                shared.add_persons_workspace(&jj, *place, *colocate);
            }
            Turn::Sweep(standings) => {
                let main =
                    Keep::Workspace(shared.project.main_workspace().into());
                let owners: Vec<Owner> = shared
                    .runs
                    .iter()
                    .zip(standings.iter().chain(std::iter::repeat(&3)))
                    .filter_map(|(run, standing)| {
                        let standing = match standing {
                            0 => Standing::Kept,
                            1 => Standing::Landed,
                            2 => Standing::Discarded,
                            _ => return None,
                        };
                        Some(Owner {
                            run: run.clone(),
                            standing,
                            workspaces: vec![run.clone()],
                            keep: main.clone(),
                        })
                    })
                    .collect();
                let sweep = shared.project.plan_sweep(&owners).unwrap();
                shared.project.sweep(&sweep).unwrap();
                shared.runs.retain(|run| {
                    owners.iter().any(|owner| {
                        owner.run == *run && owner.standing == Standing::Kept
                    })
                });
            }
            Turn::Forget(at) => {
                let names: Vec<String> = shared.names().into_iter().collect();
                let name = &names[at % names.len()];
                shared.project.forget_workspace(name).unwrap();
                shared.runs.remove(name);
            }
        }
        shared.check(&format!("{step:?}"));
    }
    // The CLI still works in each of the person's workspaces.
    for dir in shared.persons.values() {
        shared.world.jj(&jj, dir, &["status"]);
    }
    tc.event_value("persons", shared.persons.len() as f64);
}
