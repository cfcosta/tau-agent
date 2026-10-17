//! Projects and run workspaces on real repositories: a Git repository
//! imported into a project, a workspace per run, a commit per turn, and
//! a fork that starts from one turn's code.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

mod common;

use std::path::Path;

use hegel::{Generator as _, TestCase, generators as gs};
use tau_testing::{block_on, git::git};
use tau_vcs_host::{Identity, Link, ProjectRepo, UpdateFrom};

/// A Git repository with one commit on `main`, and that commit's id.
fn source(dir: &Path) -> String {
    git(dir, &["init", "--quiet"]);
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    git(dir, &["add", "README.md"]);
    git(dir, &["commit", "--quiet", "-m", "first"]);
    git(dir, &["rev-parse", "HEAD"])
}

#[test]
fn a_clone_imports_like_a_checkout() {
    let src = tempfile::tempdir().unwrap();
    let head = source(src.path());
    let home = tempfile::tempdir().unwrap();
    let url = format!("file://{}", src.path().display());
    let project = tau_vcs_host::ProjectRepo::clone(
        &url,
        Some("unused"),
        home.path().join("owner/clone"),
        Identity::default(),
    )
    .unwrap();
    assert_eq!(project.trunk().unwrap(), head);

    // A failed clone leaves nothing behind.
    let missing = home.path().join("missing");
    let error = tau_vcs_host::ProjectRepo::clone(
        "file:///no/such/repository",
        None,
        &missing,
        Identity::default(),
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("Cannot clone"), "{error:#}");
    assert!(!missing.exists());
}

/// A clone's main workspace is colocated with its Git store, as
/// `jj git clone --colocate` makes one: `git` in `main/` sees upstream's
/// branches as `origin/*`, the bookmarks as its branches, and `HEAD` on
/// the parent of the main workspace's working copy, with nothing to
/// commit when the working copy is empty.
#[test]
fn a_clone_is_colocated_with_its_main_workspace() {
    let src = tempfile::tempdir().unwrap();
    let head = source(src.path());
    git(src.path(), &["branch", "feature"]);
    let home = tempfile::tempdir().unwrap();
    let url = format!("file://{}", src.path().display());
    let project = tau_vcs_host::ProjectRepo::clone(
        &url,
        None,
        home.path().join("project"),
        Identity::default(),
    )
    .unwrap();
    let main = project.main_dir().to_owned();
    assert!(main.join(".git").is_dir());
    assert!(main.join(".jj").is_dir());
    let branches = |prefix: &str| {
        git(
            &main,
            &["for-each-ref", "--format=%(refname:short)", prefix],
        )
    };
    assert_eq!(branches("refs/heads/"), "feature\nmain");
    assert_eq!(
        branches("refs/remotes/origin/"),
        "origin\norigin/feature\norigin/main"
    );
    assert_eq!(
        git(&main, &["symbolic-ref", "refs/remotes/origin/HEAD"]),
        "refs/remotes/origin/main"
    );
    assert_eq!(git(&main, &["remote", "get-url", "origin"]), url);

    // The main chat catches up with trunk: `HEAD` follows.
    let vcs =
        block_on(tau_vcs_host::Vcs::open(&main, Identity::default())).unwrap();
    block_on(vcs.move_onto(head.clone(), "main", true)).unwrap();
    assert_eq!(git(&main, &["rev-parse", "HEAD"]), head);
    std::fs::write(main.join("b.txt"), "b\n").unwrap();
    let ours = block_on(vcs.commit_all("ours", "main")).unwrap();
    assert_eq!(git(&main, &["rev-parse", "HEAD"]), ours.commit_id);
    assert_eq!(git(&main, &["rev-parse", "main"]), ours.commit_id);
    assert_eq!(git(&main, &["rev-parse", "origin/main"]), head);
    assert_eq!(git(&main, &["status", "--porcelain"]), "");
}

/// Clones a small public repository from GitHub over HTTPS. Needs the
/// network: `cargo nextest run --release -p tau-vcs-host --run-ignored only
/// clones_over_https`.
#[test]
#[ignore = "needs the network"]
fn clones_over_https() {
    let home = tempfile::tempdir().unwrap();
    let project = tau_vcs_host::ProjectRepo::clone(
        "https://github.com/octocat/Hello-World.git",
        None,
        home.path().join("project"),
        Identity::default(),
    )
    .unwrap();
    assert!(!project.trunk().unwrap().is_empty());
    let updated = project
        .update(UpdateFrom::Remote {
            url: "https://github.com/octocat/Hello-World.git",
            token: None,
        })
        .unwrap();
    assert!(!updated.changed(), "Hello-World does not change");
}

/// Adds a commit on `main` in `dir`, and returns its id.
fn commit(dir: &Path, file: &str) -> String {
    std::fs::write(dir.join(file), "more\n").unwrap();
    git(dir, &["add", file]);
    git(dir, &["commit", "--quiet", "-m", file]);
    git(dir, &["rev-parse", "HEAD"])
}

/// A shallow clone imports: its `shallow` file comes along, so the
/// commits whose parents it lacks are where history starts. Updates
/// bring its new commits.
#[test]
fn a_shallow_checkout_imports_and_updates() {
    let src = tempfile::tempdir().unwrap();
    source(src.path());
    commit(src.path(), "b.txt");
    let home = tempfile::tempdir().unwrap();
    let url = format!("file://{}", src.path().display());
    git(
        home.path(),
        &["clone", "--quiet", "--depth", "1", &url, "shallow"],
    );
    let shallow = home.path().join("shallow");
    let head = git(&shallow, &["rev-parse", "HEAD"]);
    let project = tau_vcs_host::ProjectRepo::import(
        shallow.to_str().unwrap(),
        home.path().join("project"),
        Identity::default(),
    )
    .unwrap();
    assert_eq!(project.trunk().unwrap(), head);
    assert_eq!(
        project.file_at(&head, "b.txt").unwrap().unwrap().0,
        b"more\n"
    );

    let next = commit(&shallow, "c.txt");
    let updated = project.update(UpdateFrom::Checkout(&shallow)).unwrap();
    assert_eq!(updated.after, next);
    assert_eq!(project.parent_of(&next).unwrap(), Some(head));
}

#[test]
fn files_and_parents_are_read_at_a_commit() {
    let src = tempfile::tempdir().unwrap();
    let first = source(src.path());
    std::fs::write(src.path().join("run.sh"), "echo hi\n").unwrap();
    git(src.path(), &["add", "run.sh"]);
    git(src.path(), &["update-index", "--chmod=+x", "run.sh"]);
    git(src.path(), &["commit", "--quiet", "-m", "script"]);
    let second = git(src.path(), &["rev-parse", "HEAD"]);
    let home = tempfile::tempdir().unwrap();
    let project = tau_vcs_host::ProjectRepo::import(
        src.path().to_str().unwrap(),
        home.path().join("project"),
        Identity::default(),
    )
    .unwrap();
    assert_eq!(project.default_branch().as_deref(), Some("main"));
    assert_eq!(project.parent_of(&second).unwrap(), Some(first.clone()));
    assert_eq!(
        project.file_at(&second, "run.sh").unwrap(),
        Some((b"echo hi\n".to_vec(), true))
    );
    assert_eq!(
        project.file_at(&first, "README.md").unwrap(),
        Some((b"hello\n".to_vec(), false))
    );
    assert_eq!(project.file_at(&first, "run.sh").unwrap(), None);
}

#[test]
fn a_bad_source_fails_to_import() {
    let home = tempfile::tempdir().unwrap();
    let missing = home.path().join("nothing-here");
    let err = tau_vcs_host::ProjectRepo::import(
        missing.to_str().unwrap(),
        home.path().join("p"),
        Identity::default(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("is not a Git repository"), "{err}");
}

/// A file's lines before and after (`None`: no file), and whether each
/// ends in a newline.
type Sides = (Option<Vec<String>>, Option<Vec<String>>, bool, bool);

/// A file's contents as lines drawn from ones that look like diff
/// syntax (`-- comment` shows as `--- comment` when removed, `++x` as
/// `+++x` when added), each with its newline except maybe the last.
#[hegel::composite]
fn lines_of(tc: &TestCase) -> Option<Vec<String>> {
    tc.draw(gs::optional(
        gs::vecs(
            gs::sampled_from(vec![
                "",
                "-",
                "--",
                "-- a",
                "--- a/x",
                "+",
                "++",
                "++ b",
                "+++ b/x",
                "@@ -1 +1 @@",
                "diff --git a/x b/x",
                " ",
                "a",
                "b",
            ])
            .map(String::from),
        )
        .max_size(8),
    ))
}

/// The line counts of a minimal edit from `before` to `after`: the lines
/// not in their longest common subsequence.
fn minimal_counts(before: &[&str], after: &[&str]) -> (usize, usize) {
    let mut lcs = vec![vec![0usize; after.len() + 1]; before.len() + 1];
    for i in (0..before.len()).rev() {
        for j in (0..after.len()).rev() {
            lcs[i][j] = if before[i] == after[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let common = lcs[0][0];
    (after.len() - common, before.len() - common)
}

/// A file's lines as `similar` splits them: each keeps its newline.
fn split_lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

fn contents(lines: &[String], newline_at_end: bool) -> String {
    let mut text = lines.join("\n");
    if newline_at_end && !lines.is_empty() {
        text.push('\n');
    }
    text
}

/// What a diff lists for each changed file is how many lines a minimal
/// edit adds and removes, whatever the lines look like: a removed
/// `-- comment` or an added `++x` counts like any other line.
// Each case commits twice through jj, which waits on the disk: slow
// under load, so the too-slow check would fail it for the machine.
#[hegel::test(
    test_cases = 40,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
#[hegel::explicit_test_case(
    files = vec![(Some(vec![String::from("-- a")]), None::<Vec<String>>, true, true)]
)]
fn diff_line_counts_match_a_minimal_edit(tc: TestCase) {
    let files: Vec<Sides> = tc.draw(
        gs::vecs(gs::tuples!(
            lines_of(),
            lines_of(),
            gs::booleans(),
            gs::booleans()
        ))
        .min_size(1)
        .max_size(2),
    );

    DIFF_FIXTURE.with(|fixture| check_diff_counts(fixture, &files));
}

/// A project with one workspace, made once per test thread: importing
/// one per case is too slow. Each case writes every file it names on
/// both sides, so what earlier cases left does not change its diff.
struct DiffFixture {
    _src: tempfile::TempDir,
    _home: tempfile::TempDir,
    project: ProjectRepo,
    vcs: tau_vcs_host::Vcs,
    runtime: tokio::runtime::Runtime,
}

thread_local! {
    static DIFF_FIXTURE: DiffFixture = {
        let src = tempfile::tempdir().unwrap();
        let head = source(src.path());
        let home = tempfile::tempdir().unwrap();
        let project = tau_vcs_host::ProjectRepo::import(
            src.path().to_str().unwrap(),
            home.path().join("p"),
            Identity::default(),
        )
        .unwrap();
        let vcs = project.add_workspace("w", &head).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        DiffFixture { _src: src, _home: home, project, vcs, runtime }
    };
}

fn check_diff_counts(fixture: &DiffFixture, files: &[Sides]) {
    let DiffFixture {
        project,
        vcs,
        runtime,
        ..
    } = fixture;
    let dir = project.workspace_dir("w");
    let commit = |side: usize| {
        for (n, file) in files.iter().enumerate() {
            let (lines, end) = if side == 0 {
                (&file.0, file.2)
            } else {
                (&file.1, file.3)
            };
            let path = dir.join(format!("f{n}.txt"));
            match lines {
                Some(lines) => {
                    std::fs::write(&path, contents(lines, end)).unwrap()
                }
                None => {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
        runtime
            .block_on(vcs.commit_all(format!("side {side}"), run_head("sides")))
            .unwrap()
            .commit_id
    };
    let before = commit(0);
    let after = commit(1);

    let mut expected = Vec::new();
    for (n, (old, new, old_end, new_end)) in files.iter().enumerate() {
        let old = old.as_ref().map(|lines| contents(lines, *old_end));
        let new = new.as_ref().map(|lines| contents(lines, *new_end));
        if old == new {
            continue;
        }
        let (added, removed) = minimal_counts(
            &split_lines(old.as_deref().unwrap_or_default()),
            &split_lines(new.as_deref().unwrap_or_default()),
        );
        expected.push((format!("f{n}.txt"), added, removed));
    }
    let counted: Vec<(String, usize, usize)> = project
        .diff(&before, &after)
        .unwrap()
        .into_iter()
        .map(|file| (file.path, file.added, file.removed))
        .collect();
    assert_eq!(counted, expected);
}

/// Loads the project's jj repository at `root`, as the host would, to
/// rewrite commits behind the tools' backs.
fn repo_at(root: &Path) -> std::sync::Arc<jj_lib::repo::ReadonlyRepo> {
    use jj_lib::{
        config::{ConfigLayer, ConfigSource, StackedConfig},
        default_backend_factories::{
            default_backend_factories,
            default_working_copy_factories,
        },
        settings::UserSettings,
        workspace::Workspace,
    };
    let mut config = StackedConfig::with_defaults();
    let mut user = ConfigLayer::empty(ConfigSource::User);
    user.set_value("user.name", "host").unwrap();
    user.set_value("user.email", "host@localhost").unwrap();
    config.add_layer(user);
    let settings =
        UserSettings::from_config_and_home_dir(config, None).unwrap();
    let workspace = Workspace::load(
        &settings,
        &root.join("main"),
        &default_backend_factories(),
        &default_working_copy_factories(),
    )
    .unwrap();
    pollster::block_on(workspace.repo_loader().load_at_head()).unwrap()
}

/// A link follows its change when the commit is rewritten, and refuses
/// a divergent one.
#[test]
fn links_follow_their_change() {
    use jj_lib::{
        backend::CommitId,
        object_id::ObjectId as _,
        repo::Repo as _,
    };

    let src = tempfile::tempdir().unwrap();
    let head = source(src.path());
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("p");
    let project = tau_vcs_host::ProjectRepo::import(
        src.path().to_str().unwrap(),
        &root,
        Identity::default(),
    )
    .unwrap();
    let vcs = project.add_workspace("w", &head).unwrap();
    std::fs::write(project.workspace_dir("w").join("a.txt"), "a\n").unwrap();
    let turn = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(vcs.commit_all("turn 1", run_head("r1")))
        .unwrap();
    let link = Link {
        turn: 1,
        workspace: "w".into(),
        commit_id: turn.commit_id.clone(),
        change_id: turn.change_id.clone(),
        changed: true,
        from: None,
        snapshot: false,
    };
    assert_eq!(project.current([link.clone()]).unwrap()[0], link);

    // Rewrite the turn's commit: the change stays, the commit moves.
    let repo = repo_at(&root);
    let old = repo
        .store()
        .get_commit(&CommitId::try_from_hex(&turn.commit_id).unwrap())
        .unwrap();
    let mut tx = repo.start_transaction();
    let new = pollster::block_on(
        tx.repo_mut()
            .rewrite_commit(&old)
            .set_description("turn 1, restacked")
            .write(),
    )
    .unwrap();
    pollster::block_on(tx.repo_mut().rebase_descendants()).unwrap();
    let repo = pollster::block_on(tx.commit("rewrite turn 1")).unwrap();
    let moved = project.current([link.clone()]).unwrap().remove(0);
    assert_eq!(moved.commit_id, new.id().hex());
    assert_eq!(moved.change_id, link.change_id);

    // A second visible commit with the same change id: divergent.
    let mut tx = repo.start_transaction();
    pollster::block_on(
        tx.repo_mut()
            .new_commit(vec![repo.store().root_commit_id().clone()], new.tree())
            .set_change_id(new.change_id().clone())
            .write(),
    )
    .unwrap();
    pollster::block_on(tx.commit("diverge")).unwrap();
    let err = project.current([link]).unwrap_err().to_string();
    assert!(err.contains("divergent"), "{err}");
}

/// The main chat's workspace is `tau-main`, in `main/`: it is not listed
/// among the runs' workspaces, and it is never forgotten. A new project
/// has no `default` workspace.
#[test]
fn the_main_chats_workspace_is_tau_main() {
    let src = tempfile::tempdir().unwrap();
    let head = source(src.path());
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("project");
    let project = tau_vcs_host::ProjectRepo::open_or_import(
        src.path().to_str().unwrap(),
        &root,
        Identity::default(),
    )
    .unwrap();
    let dir = project.main_dir().to_owned();
    assert_eq!(dir, root.join("main"));
    assert_eq!(project.main_workspace(), tau_vcs_host::MAIN_WORKSPACE);
    assert_eq!(
        project.workspace_head("default").unwrap(),
        None,
        "default is never tau's"
    );
    let vcs = project
        .add_workspace(tau_vcs_host::MAIN_WORKSPACE, &head)
        .unwrap();
    assert_eq!(vcs.root(), dir);
    assert!(project.workspaces().unwrap().is_empty());

    project
        .forget_workspace(tau_vcs_host::MAIN_WORKSPACE)
        .unwrap();
    assert!(dir.join(".jj").is_dir(), "it stays");
    // The project still opens.
    assert_eq!(project.trunk().unwrap(), head);
}

/// The head of run `run` (`Head::Run`).
fn run_head(run: &str) -> tau_vcs_host::Head {
    tau_vcs_host::Head::Run(tau_agent::tool::RunId(run.into()))
}
