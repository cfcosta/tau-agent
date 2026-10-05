//! Resolving the paths models give (`docs/reference/tools.md`, "read"),
//! ported from pi's `path-utils.ts` and `normalizePath`.
//!
//! - Unicode space variants become plain spaces.
//! - A leading `@` is stripped (models copy `@file` mentions).
//! - `~` and a leading `~/` expand to the home directory; any other
//!   leading `~` is literal, so `~draft.md` is a file of that name.
//! - A `file://` URL becomes its path.
//! - A relative path resolves against the tool's root.
//!
//! For reading, a path that does not exist is retried as macOS writes
//! screenshot names: a narrow no-break space before `AM`/`PM` (either
//! case), NFD normalization, curly apostrophes, and NFD with curly
//! apostrophes.

use std::path::{Component, Path, PathBuf};

use unicode_normalization::UnicodeNormalization;

/// Where a tool's paths resolve: its root directory, and the home
/// directory `~` stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    dir: PathBuf,
    home: Option<PathBuf>,
}

const NARROW_NO_BREAK_SPACE: char = '\u{202F}';

fn is_unicode_space(c: char) -> bool {
    matches!(
        c,
        '\u{00A0}' | '\u{2000}'
            ..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}'
    )
}

impl Root {
    /// A root at `dir`, with `~` meaning `$HOME`.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            home: std::env::var_os("HOME").map(PathBuf::from),
        }
    }

    /// Replaces the home directory `~` expands to.
    pub fn with_home(mut self, home: impl Into<PathBuf>) -> Self {
        self.home = Some(home.into());
        self
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The absolute, lexically normalized path `input` names.
    pub fn resolve(&self, input: &str) -> PathBuf {
        let mut path: String = input
            .chars()
            .map(|c| if is_unicode_space(c) { ' ' } else { c })
            .collect();
        if let Some(rest) = path.strip_prefix('@') {
            path = rest.to_owned();
        }
        let expanded = match (&self.home, path.as_str()) {
            (Some(home), "~") => home.clone(),
            (Some(home), p) if p.starts_with("~/") => home.join(&p[2..]),
            (_, p) => match p.strip_prefix("file://") {
                Some(local) => PathBuf::from(local),
                None => PathBuf::from(p),
            },
        };
        normalize(&self.dir.join(expanded))
    }

    /// [`Self::resolve`], then, if nothing exists there, the macOS
    /// screenshot variants in pi's order; the first that exists wins,
    /// and the resolved path is kept when none does.
    pub fn resolve_read(&self, input: &str) -> PathBuf {
        let resolved = self.resolve(input);
        if resolved.exists() {
            return resolved;
        }
        let text = resolved.to_string_lossy().into_owned();
        let nfd: String = text.nfd().collect();
        let candidates = [
            am_pm_variant(&text),
            nfd.clone(),
            curly_variant(&text),
            curly_variant(&nfd),
        ];
        candidates
            .into_iter()
            .filter(|candidate| *candidate != text)
            .map(PathBuf::from)
            .find(|candidate| candidate.exists())
            .unwrap_or(resolved)
    }
}

/// `" AM."`/`" pm."` and the like, with a narrow no-break space instead
/// of the space, as macOS names screenshots.
fn am_pm_variant(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut rest = path;
    while let Some(index) = rest.find(' ') {
        out.push_str(&rest[..index]);
        let after = &rest[index + 1..];
        let marker = after.get(..3).filter(|m| {
            let m = m.to_ascii_lowercase();
            m == "am." || m == "pm."
        });
        out.push(if marker.is_some() {
            NARROW_NO_BREAK_SPACE
        } else {
            ' '
        });
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Straight apostrophes as the right single quotation mark macOS uses.
fn curly_variant(path: &str) -> String {
    path.replace('\'', "\u{2019}")
}

/// Removes `.` and resolves `..` without touching the filesystem.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}
