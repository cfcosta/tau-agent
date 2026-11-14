//! Where heiko keeps what the person signed in with: heiko's config
//! directory, where each plugin that signs in keeps its own (heiko-chatgpt
//! its `chatgpt/`, ADR 0045). Only what the person set up in heiko counts;
//! the environment is never read.

use std::path::{Path, PathBuf};

use heiko_ai::chatgpt::Store;

/// Where heiko keeps what the user signed in with, each readable only by
/// them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    /// Usually `$XDG_CONFIG_HOME/heiko`.
    pub dir: PathBuf,
}

impl Credentials {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// heiko's config directory: `$XDG_CONFIG_HOME/heiko`.
    pub fn default_dir() -> Self {
        let dir = Store::default_dir()
            .ok()
            .and_then(|dir| dir.parent().map(Path::to_owned))
            .unwrap_or_else(|| PathBuf::from("."));
        Self::new(dir)
    }
}
