//! Landing a child run on its parent (ADR 0009): the child's changes
//! restack onto the parent's newest commit, keeping their change ids.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

mod common;

use std::path::Path;

use tau_vcs_host::{DEFAULT_WORKSPACE, Identity, ProjectRepo, Vcs, VcsError};

/// A project with one commit on `main`: `a.txt` holding `a`.
fn project(home: &Path) -> (ProjectRepo, String) {
    let project = common::project_with(home, &[("a.txt", "a\n")]);
    let trunk = project.trunk().unwrap();
    (project, trunk)
}

struct Run<'a> {
    project: &'a ProjectRepo,
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
                .commit_all(format!("{} turn", self.name), self.bookmark()),
        )
        .commit_id
    }
}

fn block<T>(
    future: impl std::future::Future<Output = Result<T, VcsError>>,
) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
        .unwrap()
}

fn run<'a>(
    project: &'a ProjectRepo,
    name: &'static str,
    base: &str,
) -> Run<'a> {
    let vcs = project.add_workspace(name, base).unwrap();
    Run { project, name, vcs }
}

/// What a confirmed landing left in conflict stays on the parent's
/// stack (`Vcs::conflicts`) until a commit resolves it, and a resolution
/// in `@` counts before it is committed.
#[test]
fn conflicts_stay_on_the_stack_until_resolved() {
    let home = tempfile::tempdir().unwrap();
    let (project, trunk) = project(home.path());
    let parent = run(&project, "parent", &trunk);
    let child = run(&project, "child", &trunk);
    assert!(block(parent.vcs.conflicts()).is_empty());
    parent.write("a.txt", "parent\n");
    parent.turn();
    child.write("a.txt", "child\n");
    let child_head = child.turn();
    block(parent.vcs.land(&child_head, parent.bookmark(), true));
    assert_eq!(block(parent.vcs.conflicts()), ["a.txt"]);
    // Another file changed leaves the conflict where it is.
    parent.write("b.txt", "b\n");
    parent.turn();
    assert_eq!(block(parent.vcs.conflicts()), ["a.txt"]);
    parent.write("a.txt", "both\n");
    assert!(block(parent.vcs.conflicts()).is_empty(), "resolved in @");
    parent.turn();
    assert!(block(parent.vcs.conflicts()).is_empty());
}

/// The main chat's commits move trunk (ADR 0015). It catches up with
/// trunk first, as the host has it: its workspace starts on jj's root.
fn commit_on_trunk(project: &ProjectRepo, path: &str, text: &str) -> String {
    let dir = project.workspace_dir(DEFAULT_WORKSPACE);
    let main = tau_testing::block_on_io(tau_vcs_host::Vcs::open(
        &dir,
        Identity::default(),
    ))
    .unwrap();
    let trunk = project.trunk_name().unwrap();
    block(main.move_onto(project.trunk().unwrap(), trunk.clone(), true));
    std::fs::write(dir.join(path), text).unwrap();
    block(main.commit_all("on trunk", trunk)).commit_id
}

/// A run moves onto trunk's newest commit, as the main chat catches up
/// (ADR 0014): previewed first, then its changes go on top and its
/// bookmark follows.
#[test]
fn a_run_moves_onto_trunk() {
    let home = tempfile::tempdir().unwrap();
    let (project, trunk) = project(home.path());
    let session = run(&project, "session", &trunk);
    session.write("s.txt", "s\n");
    session.turn();
    // Trunk moves on while the session works.
    let trunk = commit_on_trunk(&project, "t.txt", "t\n");
    assert_eq!(project.trunk().unwrap(), trunk);

    let preview =
        block(session.vcs.move_onto(&trunk, session.bookmark(), false));
    assert_eq!(preview.changes.len(), 1);
    assert!(preview.conflicts.is_empty());
    assert_eq!(session.read("t.txt"), None, "a preview changes nothing");

    let moved = block(session.vcs.move_onto(&trunk, session.bookmark(), true));
    assert_eq!(session.read("t.txt").as_deref(), Some("t\n"));
    assert_eq!(session.read("s.txt").as_deref(), Some("s\n"));
    assert_eq!(project.parent_of(&moved.head).unwrap(), Some(trunk));
    assert_eq!(
        project.bookmark(&session.bookmark()).unwrap(),
        Some(moved.head.clone())
    );
}

/// A run that changed what trunk changed since shows the conflict in its
/// preview, as data.
#[test]
fn moving_onto_trunk_shows_conflicts() {
    let home = tempfile::tempdir().unwrap();
    let (project, trunk) = project(home.path());
    let two = run(&project, "two", &trunk);
    two.write("a.txt", "two\n");
    two.turn();
    let trunk = commit_on_trunk(&project, "a.txt", "one\n");
    let preview = block(two.vcs.move_onto(&trunk, two.bookmark(), false));
    assert_eq!(preview.conflicts, ["a.txt"]);
}
