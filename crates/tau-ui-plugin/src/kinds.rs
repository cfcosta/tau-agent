//! Repository kinds as the host keeps them (ADR 0045): tau's own
//! repositories a plugin asks for, such as an agent's or the plugins
//! repository, where they live, what they start with, and how their runs
//! go. A plugin declares them ([`crate::UiPlugin::repo_kinds`]), and
//! draws their place in the sidebar ([`crate::points::REPO_KIND`]).

/// One kind of tau's own repository.
#[derive(Debug, Clone)]
pub struct KindHost {
    /// Its name, as the sidebar's kind names it (`RepoKind::kind`).
    pub kind: &'static str,
    /// Its directory, beside the clones' in tau's data directory.
    pub root: &'static str,
    pub list: KindList,
    /// What a repository of the kind starts with, made the first time.
    pub first_files: fn() -> Vec<(String, String)>,
    /// Whether its runs are kept in: only its own plugin's launcher, and
    /// with none, no commands at all.
    pub sandboxed: bool,
    /// The plugins left out of its runs.
    pub without: &'static [&'static str],
}

/// How many repositories of a kind there are.
#[derive(Debug, Clone, Copy)]
pub enum KindList {
    /// One, its directory, listed as `name`.
    One { name: &'static str },
    /// One for each folder in its directory whose name `valid` takes,
    /// and made one at a time ([`crate::Conversations::make_repo`]).
    Each { valid: fn(&str) -> bool },
}
