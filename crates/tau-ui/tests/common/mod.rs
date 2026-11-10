//! What tests share of a host's repositories: their projects, opened as
//! the VCS keeps them.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

pub mod forge;

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

/// A host's settings for the Luau plugin `plugin` (one of
/// tau-luau-plugins'), given as a test starts it: memory's pass after
/// each run turned off, for scripts that answer only what they write.
pub trait LuauSettings {
    fn with_luau_settings(self, plugin: &str, value: serde_json::Value)
    -> Self;
}

impl LuauSettings for Host {
    fn with_luau_settings(
        self,
        plugin: &str,
        value: serde_json::Value,
    ) -> Self {
        self.with_plugin_settings(
            tau_luau_plugins::NAME,
            serde_json::json!({ "plugins": { plugin: value } }),
        )
    }
}
