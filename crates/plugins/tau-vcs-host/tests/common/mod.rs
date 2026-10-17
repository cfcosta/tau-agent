//! Fixtures the vcs tests share: a project imported from a fresh git
//! repository, a coding agent on a run's workspace, the merge model
//! (`merge`), and the operation log as another program reads it
//! (`ops`).

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]
// Each test crate uses only some of these.
#![allow(dead_code)]

pub mod merge;
pub mod ops;

use std::{path::Path, sync::Arc};

use tau_agent::{agent::Agent, tool::RunId};
use tau_testing::{git::git, scripted::ScriptedModel};
use tau_tools_host::{path::Root, plugin::CodingTools};
use tau_vcs_host::{
    Identity,
    Link,
    ProjectRepo,
    RunWorkspace,
    SubAgents,
    VcsPlugin,
    sub_agents::Ending,
};
use tokio::sync::mpsc;

/// A project imported from a git repository whose one commit,
/// `first`, holds `files`.
pub fn project_with(home: &Path, files: &[(&str, &str)]) -> ProjectRepo {
    let src = home.join("src");
    std::fs::create_dir_all(&src).unwrap();
    git(&src, &["init", "--quiet"]);
    for (path, text) in files {
        let path = src.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    git(&src, &["add", "."]);
    git(&src, &["commit", "--quiet", "-m", "first"]);
    tau_vcs_host::ProjectRepo::import(
        src.to_str().unwrap(),
        home.join("p"),
        Identity::default(),
    )
    .unwrap()
}

/// A project whose trunk holds `a.txt`, `one`.
pub fn project(home: &Path) -> ProjectRepo {
    project_with(home, &[("a.txt", "one\n")])
}

/// The project's Git refs under `prefix`, as `git for-each-ref` lists
/// them, by name, at their commits.
pub fn git_refs(
    project: &ProjectRepo,
    prefix: &str,
) -> std::collections::BTreeMap<String, String> {
    git(
        project.main_dir(),
        &["for-each-ref", "--format=%(refname) %(objectname)", prefix],
    )
    .lines()
    .map(|line| {
        let (name, id) = line.split_once(' ').unwrap();
        (name.to_owned(), id.to_owned())
    })
    .collect()
}

/// No run's head is a branch to Git, nor a bookmark to jj (ADR 0043):
/// none of the refs `git branch -a` lists, local and remote-tracking
/// branches, is under `tau/`, and the project has no `tau/` bookmark.
/// (`git branch` itself fails while `main/`'s `HEAD` is jj's unborn
/// root, before the main chat's first catch-up.)
pub fn assert_no_run_branches(project: &ProjectRepo) {
    let branches = git(
        project.main_dir(),
        &[
            "for-each-ref",
            "--format=%(refname)",
            "refs/heads",
            "refs/remotes",
        ],
    );
    for name in branches.lines() {
        assert!(
            !name.contains("tau/"),
            "git branch -a lists a run's head: {name}"
        );
    }
    assert_eq!(
        project.bookmarks("tau/").unwrap(),
        Vec::<String>::new(),
        "a run's head is a bookmark"
    );
}

/// The coding agent on `workspace`, with the model's vcs tools when
/// `vcs`.
pub fn coder(llm: ScriptedModel, workspace: &RunWorkspace, vcs: bool) -> Agent {
    let agent = Agent::new(llm)
        .name("coder")
        .plugin(CodingTools::new(Root::new(workspace.dir())));
    let agent = if vcs {
        agent.plugin(VcsPlugin::new(workspace.vcs().clone()))
    } else {
        agent
    };
    agent.plugin(workspace.clone())
}

/// Sub-agents whose endings come out of the receiver as they end, once
/// their work is checked: what the host hears, to land them.
pub fn heard_sub_agents()
-> (SubAgents, mpsc::UnboundedReceiver<(RunId, Ending)>) {
    let (sender, ends) = mpsc::unbounded_channel();
    let agents = SubAgents::new(
        None,
        Some(Arc::new(move |run: &RunId, ending: &Ending| {
            let _ = sender.send((run.clone(), ending.clone()));
        })),
    );
    (agents, ends)
}

/// The turn links among a run's records, with their sequence numbers.
pub fn links(entries: &[(i64, String)]) -> Vec<(i64, Link)> {
    entries
        .iter()
        .filter_map(|(seq, body)| Link::parse(body).map(|link| (*seq, link)))
        .collect()
}
