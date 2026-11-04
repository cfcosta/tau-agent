//! What tests share of a host's repositories: their projects, opened as
//! the VCS keeps them.

use std::time::{Duration, Instant};

use tau_ui::host::Host;
use tau_vcs_host::{Identity, Project, ProjectRepo};

/// The project of the repository `host` lists as `repo`, once every
/// import ends, opened again where its workspaces are: for checks that
/// read it. A clone's, not a local one's (ADR 0043).
pub fn project(host: &Host, repo: &str) -> Project {
    let started = Instant::now();
    while host.is_importing() {
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "{repo}'s import never ends"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let root = host
        .repo(repo)
        .expect("the repository is listed")
        .workspaces;
    ProjectRepo::open(root, Identity::default())
        .map(Project::from)
        .expect("its project opens")
}
