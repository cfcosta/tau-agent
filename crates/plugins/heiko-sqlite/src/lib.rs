//! The store in SQLite, as a plugin (ADR 0045): it fills the
//! `heiko.store` slot with `heiko-store-sqlite`.

#[cfg(feature = "host")]
mod half;

#[cfg(feature = "host")]
pub use half::SqliteHost;
use heiko_ui_plugin::{Declaration, Manifest, UiPlugin, store::STORE};

pub const NAME: &str = "heiko-sqlite";

/// heiko-sqlite with its UI: its row on the Plugins screen.
#[derive(Debug, Clone, Copy, Default)]
pub struct SqliteUi;

impl UiPlugin for SqliteUi {
    type State = ();
    type Data = ();
    type RepoData = ();
    type Settings = ();

    fn name(&self) -> &'static str {
        NAME
    }

    fn declaration(&self) -> Declaration {
        Declaration::new().fills(&[STORE.name()])
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
    }
}
