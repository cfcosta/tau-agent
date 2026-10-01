//! Fixtures the vcs tests share: a project imported from a fresh git
//! repository, a coding agent on a run's workspace, and the merge model
//! (`merge`).

// Each test crate uses only some of these.
#![allow(dead_code)]

pub mod merge;

use std::path::Path;

use tau_agent::agent::Agent;
use tau_testing::{git::git, scripted::ScriptedModel};
use tau_tools::{path::Root, plugin::CodingTools};
use tau_vcs::{Identity, Link, Project, RunWorkspace, VcsPlugin};

/// A project imported from a git repository whose one commit,
/// `first`, holds `files`.
pub fn project_with(home: &Path, files: &[(&str, &str)]) -> Project {
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
    Project::import(src.to_str().unwrap(), home.join("p"), Identity::default())
        .unwrap()
}

/// A project whose trunk holds `a.txt`, `one`.
pub fn project(home: &Path) -> Project {
    project_with(home, &[("a.txt", "one\n")])
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

/// The turn links among a run's records, with their sequence numbers.
pub fn links(entries: &[(i64, String)]) -> Vec<(i64, Link)> {
    entries
        .iter()
        .filter_map(|(seq, body)| Link::parse(body).map(|link| (*seq, link)))
        .collect()
}
