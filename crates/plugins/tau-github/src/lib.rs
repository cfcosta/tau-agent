//! GitHub as where tau's repositories are hosted (ADR 0045): its host
//! half, tau-github-host, fills the `tau.forge` slot. Its UI draws
//! nothing yet: signing in is still tau-ui's.

use tau_ui_plugin::{Manifest, UiPlugin};

/// The plugin's name, for both its halves.
pub const NAME: &str = "tau-github";

/// tau-github with its UI.
#[derive(Debug, Clone, Copy, Default)]
pub struct GitHubUi;

impl UiPlugin for GitHubUi {
    type State = ();
    type Data = ();
    type RepoData = ();
    type Settings = ();

    fn name(&self) -> &'static str {
        NAME
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
    }
}
