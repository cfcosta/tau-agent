//! Landing a child run on its parent (ADR 0009): the child's changes
//! restack onto the parent's newest commit, keeping their change ids.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

mod common;

use std::path::Path;

use tau_vcs_host::{ProjectRepo, Vcs, VcsError};

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

    fn head(&self) -> tau_vcs_host::Head {
        tau_vcs_host::Head::Run(tau_agent::tool::RunId(self.name.into()))
    }

    /// Ends a turn; returns the commit that holds it.
    fn turn(&self) -> String {
        block(
            self.vcs
                .commit_all(format!("{} turn", self.name), self.head()),
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
    block(parent.vcs.land(&child_head, parent.head(), true));
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
