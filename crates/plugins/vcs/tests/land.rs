//! Landing a child run on its parent (ADR 0009): the child's changes
//! restack onto the parent's newest commit, keeping their change ids.

use std::{path::Path, process::Command};

use tau_vcs::{Identity, Project, Vcs};

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
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// A project with one commit on `main`: `a.txt` holding `a`.
fn project(home: &Path) -> (Project, String) {
    let src = home.join("src");
    std::fs::create_dir_all(&src).unwrap();
    git(&src, &["init", "--quiet"]);
    std::fs::write(src.join("a.txt"), "a\n").unwrap();
    git(&src, &["add", "a.txt"]);
    git(&src, &["commit", "--quiet", "-m", "first"]);
    let project = Project::import(
        src.to_str().unwrap(),
        home.join("p"),
        Identity::default(),
    )
    .unwrap();
    let trunk = project.trunk().unwrap();
    (project, trunk)
}

struct Run<'a> {
    project: &'a Project,
    name: &'static str,
    vcs: Vcs,
}

impl Run<'_> {
    fn write(&self, path: &str, text: &str) {
        std::fs::write(self.project.workspace_dir(self.name).join(path), text)
            .unwrap();
    }

    fn read(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(
            self.project.workspace_dir(self.name).join(path),
        )
        .ok()
    }

    fn bookmark(&self) -> String {
        format!("tau/{}", self.name)
    }

    /// Ends a turn; returns the commit that holds it.
    fn turn(&self) -> String {
        block(
            self.vcs
                .checkpoint(format!("{} turn", self.name), self.bookmark()),
        )
        .commit_id
    }
}

fn block<T>(future: impl std::future::Future<Output = anyhow::Result<T>>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
        .unwrap()
}

fn run<'a>(project: &'a Project, name: &'static str, base: &str) -> Run<'a> {
    let vcs = project.add_workspace(name, base).unwrap();
    Run { project, name, vcs }
}

/// A fork of turn 1 lands on the parent's turn 2: previewed first, which
/// changes nothing, then for real, with the child's change on top.
#[test]
fn a_fork_restacks_onto_its_parent() {
    let home = tempfile::tempdir().unwrap();
    let (project, trunk) = project(home.path());
    let parent = run(&project, "parent", &trunk);
    parent.write("b.txt", "b\n");
    let turn1 = parent.turn();
    let child = run(&project, "child", &turn1);
    parent.write("c.txt", "c\n");
    let turn2 = parent.turn();
    child.write("d.txt", "d\n");
    let child_head = child.turn();

    let preview = block(parent.vcs.land(&child_head, parent.bookmark(), false));
    assert_eq!(preview.changes.len(), 1);
    assert!(preview.conflicts.is_empty());
    assert_eq!(parent.read("d.txt"), None, "a preview changes nothing");
    assert_eq!(
        project.bookmark(&parent.bookmark()).unwrap(),
        Some(turn2.clone())
    );

    let landed = block(parent.vcs.land(&child_head, parent.bookmark(), true));
    let change = &landed.changes[0];
    assert_eq!(landed.head, change.commit_id);
    assert_ne!(change.commit_id, child_head, "the child's commit moved");
    assert_eq!(parent.read("d.txt").as_deref(), Some("d\n"));
    assert_eq!(parent.read("c.txt").as_deref(), Some("c\n"));
    assert_eq!(
        project.bookmark(&parent.bookmark()).unwrap(),
        Some(landed.head.clone())
    );
    // The child's change sits on the parent's turn 2, and the parent's
    // next turn goes on above it.
    assert_eq!(project.parent_of(&landed.head).unwrap(), Some(turn2));
    parent.write("e.txt", "e\n");
    let turn3 = parent.turn();
    assert_eq!(project.parent_of(&turn3).unwrap(), Some(landed.head));
}

/// A child the parent waited on already sits on the parent's head, so
/// landing it rewrites nothing.
#[test]
fn a_child_the_parent_waited_on_is_not_rewritten() {
    let home = tempfile::tempdir().unwrap();
    let (project, trunk) = project(home.path());
    let parent = run(&project, "parent", &trunk);
    parent.write("b.txt", "b\n");
    let turn1 = parent.turn();
    let child = run(&project, "child", &turn1);
    child.write("c.txt", "c\n");
    let first = child.turn();
    child.write("d.txt", "d\n");
    let child_head = child.turn();

    let landed = block(parent.vcs.land(&child_head, parent.bookmark(), true));
    assert_eq!(landed.head, child_head);
    let ids: Vec<&str> = landed
        .changes
        .iter()
        .map(|change| change.commit_id.as_str())
        .collect();
    assert_eq!(ids, [child_head.as_str(), first.as_str()], "newest first");
    assert_eq!(parent.read("d.txt").as_deref(), Some("d\n"));
}

/// A child that edits what the parent edited since lands with the
/// conflict in it; the preview says so first.
#[test]
fn a_conflict_shows_in_the_preview_and_lands_as_data() {
    let home = tempfile::tempdir().unwrap();
    let (project, trunk) = project(home.path());
    let parent = run(&project, "parent", &trunk);
    let child = run(&project, "child", &trunk);
    parent.write("a.txt", "parent\n");
    parent.turn();
    child.write("a.txt", "child\n");
    let child_head = child.turn();

    let preview = block(parent.vcs.land(&child_head, parent.bookmark(), false));
    assert_eq!(preview.conflicts, ["a.txt"]);
    assert!(preview.changes[0].conflict);

    let landed = block(parent.vcs.land(&child_head, parent.bookmark(), true));
    assert_eq!(landed.conflicts, ["a.txt"]);
    let text = parent.read("a.txt").unwrap();
    assert!(text.contains("<<<<<<<"), "{text}");
}

/// The parent's working copy must hold nothing: landing is between turns.
#[test]
fn landing_waits_for_the_parents_turn_to_end() {
    let home = tempfile::tempdir().unwrap();
    let (project, trunk) = project(home.path());
    let parent = run(&project, "parent", &trunk);
    let child = run(&project, "child", &trunk);
    child.write("c.txt", "c\n");
    let child_head = child.turn();
    parent.write("b.txt", "b\n");
    let err = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(parent.vcs.land(&child_head, parent.bookmark(), true))
        .unwrap_err()
        .to_string();
    assert!(err.contains("after the parent's turn ends"), "{err}");
}
