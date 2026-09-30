//! Projects and run workspaces on real repositories: a Git repository
//! imported into a project, a workspace per run, a commit per turn, and
//! a fork that starts from one turn's code.

use std::{
    path::Path,
    process::Command,
    sync::{Arc, Mutex},
};

use hegel::{Generator as _, TestCase, generators as gs};
use serde_json::json;
use tau_agent::agent::{Agent, Checkpoint};
use tau_store::Store;
use tau_testing::scripted::ScriptedModel;
use tau_tools::{path::Root, plugin::CodingTools};
use tau_vcs::{
    Identity,
    Link,
    Project,
    RunWorkspace,
    UpdateFrom,
    clone_bare,
    run_workspace::{PLUGIN, bookmark},
};

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .current_dir(dir)
        // Keep the user's settings (signing, hooks) out of the fixture.
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git runs");
    assert!(output.status.success(), "git {args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// A Git repository with one commit on `main`, and that commit's id.
fn source(dir: &Path) -> String {
    git(dir, &["init", "--quiet"]);
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    git(dir, &["add", "README.md"]);
    git(dir, &["commit", "--quiet", "-m", "first"]);
    git(dir, &["rev-parse", "HEAD"])
}

#[test]
fn a_project_gives_each_run_a_workspace_on_trunk() {
    let src = tempfile::tempdir().unwrap();
    let head = source(src.path());
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("project");
    let project = Project::open_or_import(
        src.path().to_str().unwrap(),
        &root,
        Identity::default(),
    )
    .unwrap();
    assert_eq!(project.trunk().unwrap(), head);

    let vcs = project.add_workspace("one", &head).unwrap();
    let dir = project.workspace_dir("one");
    assert_eq!(vcs.root(), dir);
    assert_eq!(
        std::fs::read_to_string(dir.join("README.md")).unwrap(),
        "hello\n"
    );

    // Opening again finds the same project.
    let again =
        Project::open_or_import("unused", &root, Identity::default()).unwrap();
    assert_eq!(again.workspaces().unwrap(), ["one"]);

    project.forget_workspace("one").unwrap();
    assert!(!dir.exists());
    assert!(project.workspaces().unwrap().is_empty());
}

#[test]
fn a_clone_imports_like_a_checkout() {
    let src = tempfile::tempdir().unwrap();
    let head = source(src.path());
    let home = tempfile::tempdir().unwrap();
    let bare = home.path().join("owner/clone.git");
    let url = format!("file://{}", src.path().display());
    clone_bare(&url, Some("unused"), &bare).unwrap();
    let project = Project::import(
        bare.to_str().unwrap(),
        home.path().join("project"),
        Identity::default(),
    )
    .unwrap();
    assert_eq!(project.trunk().unwrap(), head);

    // A failed clone leaves nothing behind.
    let missing = home.path().join("missing.git");
    let error =
        clone_bare("file:///no/such/repository", None, &missing).unwrap_err();
    assert!(format!("{error:#}").contains("Cannot clone"), "{error:#}");
    assert!(!missing.exists());
}

/// Clones a small public repository from GitHub over HTTPS. Needs the
/// network: `cargo test -p tau-vcs -- --ignored`.
#[test]
#[ignore = "needs the network"]
fn clones_over_https() {
    let home = tempfile::tempdir().unwrap();
    let bare = home.path().join("hello.git");
    clone_bare("https://github.com/octocat/Hello-World.git", None, &bare)
        .unwrap();
    let project = Project::import(
        bare.to_str().unwrap(),
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

#[test]
fn a_project_updates_from_its_checkout() {
    let src = tempfile::tempdir().unwrap();
    let first = source(src.path());
    let home = tempfile::tempdir().unwrap();
    let project = Project::import(
        src.path().to_str().unwrap(),
        home.path().join("project"),
        Identity::default(),
    )
    .unwrap();
    // A run works on the old trunk meanwhile.
    project.add_workspace("run", &first).unwrap();

    let second = commit(src.path(), "b.txt");
    // Packed objects come along too.
    git(src.path(), &["gc", "--quiet"]);
    let updated = project.update(UpdateFrom::Checkout(src.path())).unwrap();
    assert!(updated.changed());
    assert_eq!((updated.before, updated.after), (first, second.clone()));
    assert_eq!(project.trunk().unwrap(), second);
    let run = project.add_workspace("after", &second).unwrap();
    assert!(run.root().join("b.txt").exists());
    assert!(project.workspace_dir("run").join("README.md").exists());

    // Nothing new: nothing changes.
    let again = project.update(UpdateFrom::Checkout(src.path())).unwrap();
    assert!(!again.changed());
}

#[test]
fn a_clone_updates_from_its_remote() {
    let src = tempfile::tempdir().unwrap();
    source(src.path());
    let home = tempfile::tempdir().unwrap();
    let bare = home.path().join("owner/repo");
    let url = format!("file://{}", src.path().display());
    clone_bare(&url, None, &bare).unwrap();
    let project = Project::import(
        bare.to_str().unwrap(),
        home.path().join("project"),
        Identity::default(),
    )
    .unwrap();
    let second = commit(src.path(), "b.txt");
    let updated = project
        .update(UpdateFrom::Remote {
            url: &url,
            token: Some("unused"),
        })
        .unwrap();
    assert!(updated.changed());
    assert_eq!(project.trunk().unwrap(), second);
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
    let project = Project::import(
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
    let err = Project::import(
        missing.to_str().unwrap(),
        home.path().join("p"),
        Identity::default(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("is not a Git repository"), "{err}");
}

fn write(path: &str, content: &str) -> serde_json::Value {
    json!({ "path": path, "content": content })
}

fn links(entries: &[(i64, String)]) -> Vec<(i64, Link)> {
    entries
        .iter()
        .filter_map(|(seq, body)| Link::parse(body).map(|link| (*seq, link)))
        .collect()
}

fn agent(llm: ScriptedModel, workspace: &RunWorkspace) -> Agent {
    Agent::new(llm)
        .name("coder")
        .plugin(CodingTools::new(Root::new(workspace.dir())))
        .plugin(workspace.clone())
}

/// Turns are snapshots, not commits (ADR 0014): the run's work stays in
/// `@` until the model commits, or until the end, when what is left is
/// committed with a message the model writes. A fork starts from a
/// turn's snapshot.
#[test]
fn turns_are_snapshots_and_forks_start_from_one() {
    let src = tempfile::tempdir().unwrap();
    source(src.path());
    let home = tempfile::tempdir().unwrap();
    let project = Project::import(
        src.path().to_str().unwrap(),
        home.path().join("p"),
        Identity::default(),
    )
    .unwrap();
    let trunk = project.trunk().unwrap();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let store = Store::memory().await.unwrap();

        // Two turns that each write a file, and a turn that only talks;
        // an observer hears what each turn changed.
        let heard: Arc<Mutex<Vec<Vec<String>>>> = Arc::default();
        let first =
            RunWorkspace::new(project.clone(), "first", Identity::default())
                .unwrap()
                .on_turn({
                    let heard = heard.clone();
                    move |turn| heard.lock().unwrap().push(turn.paths.clone())
                });
        let llm = ScriptedModel::new()
            .turn(|t| t.tool_call("write", write("a.txt", "one\n")))
            .turn(|t| t.tool_call("write", write("a.txt", "two\n")))
            .turn(|t| t.text("done"))
            // Asked to commit, it answers again without committing; the
            // model then writes the message tau commits with.
            .turn(|t| t.text("still done"))
            .turn(|t| t.text("feat: write a.txt"));
        let outcome = agent(llm.clone(), &first)
            .run("write a.txt twice", &store)
            .await
            .unwrap();
        llm.assert_exhausted();
        assert_eq!(outcome.text, "still done");
        let asked = format!("{:?}", llm.requests()[3].transcript);
        assert!(
            asked.contains("Commit your work with `vcs_commit`"),
            "{asked}"
        );
        let dir = first.dir();
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "two\n"
        );

        let turns =
            links(&store.plugin_entries(&outcome.run.0, PLUGIN).await.unwrap());
        let changed: Vec<(u32, bool, bool)> = turns
            .iter()
            .map(|(_, link)| (link.turn, link.changed, link.snapshot))
            .collect();
        assert_eq!(
            changed,
            [
                (1, true, true),
                (2, true, true),
                (3, false, true),
                (4, false, true)
            ]
        );
        assert_eq!(
            *heard.lock().unwrap(),
            [
                vec!["a.txt".to_owned()],
                vec!["a.txt".to_owned()],
                vec![],
                vec![]
            ]
        );
        // A turn that changed nothing snapshots the same commit.
        assert_eq!(turns[2].1.commit_id, turns[1].1.commit_id);
        assert_ne!(turns[0].1.commit_id, turns[1].1.commit_id);
        // Nothing was committed during the run; at the end, one change.
        assert_eq!(
            project.parent_of(&turns[1].1.commit_id).unwrap(),
            Some(trunk.clone())
        );
        let head = project.bookmark(&bookmark(&outcome.run)).unwrap().unwrap();
        let stack = project.stack(&head).unwrap();
        assert_eq!(stack.len(), 1);
        assert_eq!(stack[0].description.trim(), "feat: write a.txt");

        // A fork at turn 1 starts from turn 1's files, uncommitted, in a
        // workspace of its own, and leaves the first run's alone.
        let (seq, _) = turns[0].clone();
        let fork =
            RunWorkspace::new(project.clone(), "fork", Identity::default())
                .unwrap();
        let llm = ScriptedModel::new()
            .turn(|t| t.tool_call("read", json!({"path": "a.txt"})))
            .turn(|t| t.text("forked"))
            .turn(|t| t.text("still forked"))
            .turn(|t| t.text("feat: keep turn 1"));
        let forked = agent(llm.clone(), &fork)
            .fork(&Checkpoint::at(outcome.run.clone(), seq))
            .start("what does a.txt say?", &store)
            .outcome()
            .await
            .unwrap();
        llm.assert_exhausted();
        assert_eq!(forked.text, "still forked");
        assert_eq!(
            std::fs::read_to_string(fork.dir().join("a.txt")).unwrap(),
            "one\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "two\n"
        );
        let mut names = project.workspaces().unwrap();
        names.sort();
        assert_eq!(names, ["first", "fork"]);
        // The fork's one commit holds turn 1's files, on trunk.
        let fork_head =
            project.bookmark(&bookmark(&forked.run)).unwrap().unwrap();
        let fork_stack = project.stack(&fork_head).unwrap();
        assert_eq!(fork_stack.len(), 1);
        assert_eq!(fork_stack[0].description.trim(), "feat: keep turn 1");
        assert_eq!(
            project.bookmark(&bookmark(&outcome.run)).unwrap(),
            Some(head)
        );
    });
}

#[test]
fn diffs_between_commits_count_lines_per_file() {
    let src = tempfile::tempdir().unwrap();
    let head = source(src.path());
    let home = tempfile::tempdir().unwrap();
    let project = Project::import(
        src.path().to_str().unwrap(),
        home.path().join("p"),
        Identity::default(),
    )
    .unwrap();
    let vcs = project.add_workspace("w", &head).unwrap();
    let dir = project.workspace_dir("w");
    std::fs::write(dir.join("README.md"), "hello\nworld\n").unwrap();
    std::fs::write(dir.join("new.txt"), "a\nb\nc\n").unwrap();
    let turn = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(vcs.commit_all("turn 1", "tau/r1"))
        .unwrap();
    assert!(turn.changed);
    assert_eq!(turn.paths, ["README.md", "new.txt"]);

    let files = project.diff(&head, &turn.commit_id).unwrap();
    let summary: Vec<(&str, usize, usize)> = files
        .iter()
        .map(|file| (file.path.as_str(), file.added, file.removed))
        .collect();
    assert_eq!(summary, [("README.md", 1, 0), ("new.txt", 3, 0)]);
    assert!(files[1].text.starts_with("diff --git a/new.txt b/new.txt"));
    assert!(files[0].text.contains("+world"));
    assert!(project.diff(&head, &head).unwrap().is_empty());
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
    project: Project,
    vcs: tau_vcs::Vcs,
    runtime: tokio::runtime::Runtime,
}

thread_local! {
    static DIFF_FIXTURE: DiffFixture = {
        let src = tempfile::tempdir().unwrap();
        let head = source(src.path());
        let home = tempfile::tempdir().unwrap();
        let project = Project::import(
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
            .block_on(vcs.commit_all(format!("side {side}"), "tau/sides"))
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
    let settings = UserSettings::from_config(config).unwrap();
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
    let project = Project::import(
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
        .block_on(vcs.commit_all("turn 1", "tau/r1"))
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

/// The default workspace is the repository's own checkout: a main chat
/// works in it, it is not listed among the runs' workspaces, and it is
/// never forgotten.
#[test]
fn the_default_workspace_is_the_repositorys_own() {
    let src = tempfile::tempdir().unwrap();
    let head = source(src.path());
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("project");
    let project = Project::open_or_import(
        src.path().to_str().unwrap(),
        &root,
        Identity::default(),
    )
    .unwrap();
    let dir = project.workspace_dir(tau_vcs::DEFAULT_WORKSPACE);
    assert_eq!(dir, root.join("main"));
    let vcs = project
        .add_workspace(tau_vcs::DEFAULT_WORKSPACE, &head)
        .unwrap();
    assert_eq!(vcs.root(), dir);
    assert!(project.workspaces().unwrap().is_empty());

    project
        .forget_workspace(tau_vcs::DEFAULT_WORKSPACE)
        .unwrap();
    assert!(dir.join(".jj").is_dir(), "it stays");
    // The project still opens.
    assert_eq!(project.trunk().unwrap(), head);
}
