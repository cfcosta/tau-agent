//! Projects and run workspaces on real repositories: a Git repository
//! imported into a project, a workspace per run, a commit per turn, and
//! a fork that starts from one turn's code.

use std::{path::Path, process::Command};

use serde_json::json;
use tau_agent::agent::{Agent, Checkpoint};
use tau_store::Store;
use tau_testing::scripted::ScriptedModel;
use tau_tools::{path::Root, plugin::CodingTools};
use tau_vcs::{Identity, Link, Project, RunWorkspace, run_workspace::PLUGIN};

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

#[test]
fn turns_are_commits_and_forks_start_from_one() {
    let src = tempfile::tempdir().unwrap();
    source(src.path());
    let home = tempfile::tempdir().unwrap();
    let project = Project::import(
        src.path().to_str().unwrap(),
        home.path().join("p"),
        Identity::default(),
    )
    .unwrap();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let store = Store::memory().await.unwrap();

        // Two turns that each write a file, and a turn that only talks.
        let first =
            RunWorkspace::new(project.clone(), "first", Identity::default())
                .unwrap();
        let llm = ScriptedModel::new()
            .turn(|t| t.tool_call("write", write("a.txt", "one\n")))
            .turn(|t| t.tool_call("write", write("a.txt", "two\n")))
            .turn(|t| t.text("done"));
        let outcome = agent(llm, &first)
            .run("write a.txt twice", &store)
            .await
            .unwrap();
        assert_eq!(outcome.text, "done");
        let dir = first.dir();
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "two\n"
        );

        let turns =
            links(&store.plugin_entries(&outcome.run.0, PLUGIN).await.unwrap());
        let changed: Vec<(u32, bool)> = turns
            .iter()
            .map(|(_, link)| (link.turn, link.changed))
            .collect();
        assert_eq!(changed, [(1, true), (2, true), (3, false)]);
        // A turn that changed nothing points at the commit before it.
        assert_eq!(turns[2].1.commit_id, turns[1].1.commit_id);
        assert_ne!(turns[0].1.commit_id, turns[1].1.commit_id);

        // A fork at turn 1 starts from turn 1's files, in a workspace of
        // its own, and leaves the first run's alone.
        let (seq, _) = turns[0].clone();
        let fork =
            RunWorkspace::new(project.clone(), "fork", Identity::default())
                .unwrap();
        let llm = ScriptedModel::new()
            .turn(|t| t.tool_call("read", json!({"path": "a.txt"})))
            .turn(|t| t.text("forked"));
        let forked = agent(llm.clone(), &fork)
            .fork(&Checkpoint::at(outcome.run.clone(), seq))
            .start("what does a.txt say?", &store)
            .outcome()
            .await
            .unwrap();
        assert_eq!(forked.text, "forked");
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
    });
}
