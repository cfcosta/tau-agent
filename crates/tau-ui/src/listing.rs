//! What an `ls` result shows: a directory's entries with their kind,
//! size and age, whether `.gitignore` leaves them out, and, with the
//! vcs plugin, what `@` changes in them.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_tools::ls::Entry;
use tau_vcs::ChangeKind;

/// The tool whose results read as a [`DirListing`].
pub const TOOL: &str = "ls";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirListing {
    /// Directories first, then files, each in the model's order.
    pub entries: Vec<Listed>,
    /// The listing stopped at the entry limit or the byte cap.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listed {
    #[serde(flatten)]
    pub entry: Entry,
    /// How `@` changes it; a directory is `Modified` when anything under
    /// it changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<ChangeKind>,
}

#[derive(Deserialize)]
struct Details {
    entries: Vec<Listed>,
    truncated: bool,
}

impl Listed {
    pub fn is_dir(&self) -> bool {
        self.entry.kind.is_dir()
    }

    /// `.git` and `.jj`: the version control's own directories.
    pub fn is_vcs(&self) -> bool {
        self.is_dir() && [".git", ".jj"].contains(&self.entry.name.as_str())
    }
}

impl DirListing {
    /// Reads an `ls` result's details. `None` when they are not that
    /// shape.
    pub fn parse(details: &Value) -> Option<Self> {
        let details = Details::deserialize(details).ok()?;
        let (mut entries, files): (Vec<_>, Vec<_>) =
            details.entries.into_iter().partition(Listed::is_dir);
        entries.extend(files);
        Some(Self {
            entries,
            truncated: details.truncated,
        })
    }

    pub fn dirs(&self) -> impl Iterator<Item = &Listed> {
        self.entries.iter().filter(|entry| entry.is_dir())
    }

    pub fn files(&self) -> impl Iterator<Item = &Listed> {
        self.entries.iter().filter(|entry| !entry.is_dir())
    }

    /// The bytes the files hold, ignored ones left out.
    pub fn bytes(&self) -> u64 {
        self.files()
            .filter(|file| !file.entry.ignored)
            .filter_map(|file| file.entry.size)
            .sum()
    }

    pub fn changed(&self) -> usize {
        self.entries.iter().filter(|e| e.change.is_some()).count()
    }

    pub fn ignored(&self) -> usize {
        self.entries.iter().filter(|e| e.entry.ignored).count()
    }

    /// `3 folders · 12 files · 118 KB`, leaving out what it has none of.
    pub fn summary(&self) -> String {
        let (dirs, files) = (self.dirs().count(), self.files().count());
        let mut parts = Vec::new();
        if dirs > 0 {
            parts.push(count(dirs, "folder"));
        }
        if files > 0 {
            parts.push(count(files, "file"));
            parts.push(size(self.bytes()));
        }
        if parts.is_empty() {
            return "empty".into();
        }
        parts.join(" · ")
    }
}

pub fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// `83 B`, `4.2 KB`, `267 KB`, `1.4 MB`.
pub fn size(bytes: u64) -> String {
    const KB: f64 = 1024.;
    let b = bytes as f64;
    if bytes < 1024 {
        format!("{bytes} B")
    } else if b < 10. * KB {
        format!("{:.1} KB", b / KB)
    } else if b < KB * KB {
        format!("{:.0} KB", b / KB)
    } else if b < KB * KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else {
        format!("{:.1} GB", b / (KB * KB * KB))
    }
}

/// How long ago `modified` was, from `now`, both in Unix seconds:
/// `now`, `8m`, `3h`, `12d`, `5mo`, `2y`.
pub fn age(modified: i64, now: i64) -> String {
    let secs = (now - modified).max(0);
    match secs {
        0..60 => "now".into(),
        60..3_600 => format!("{}m", secs / 60),
        3_600..86_400 => format!("{}h", secs / 3_600),
        86_400..2_592_000 => format!("{}d", secs / 86_400),
        2_592_000..31_536_000 => format!("{}mo", secs / 2_592_000),
        _ => format!("{}y", secs / 31_536_000),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn details() -> Value {
        json!({
            "dir": "/repo/crates",
            "entries": [
                { "name": ".envrc", "kind": "file", "size": 83, "modified": 100 },
                { "name": "Cargo.toml", "kind": "file", "size": 4329,
                  "modified": 100, "change": "modified" },
                { "name": "crates", "kind": "dir", "items": 10,
                  "modified": 100, "change": "modified" },
                { "name": "notes.md", "kind": "file", "size": 1000,
                  "ignored": true },
                { "name": "target", "kind": "dir", "items": 7,
                  "ignored": true },
            ],
            "truncated": false,
        })
    }

    #[test]
    fn directories_come_first_in_the_models_order() {
        let listing = DirListing::parse(&details()).unwrap();
        let names: Vec<&str> = listing
            .entries
            .iter()
            .map(|e| e.entry.name.as_str())
            .collect();
        assert_eq!(
            names,
            ["crates", "target", ".envrc", "Cargo.toml", "notes.md"]
        );
    }

    #[test]
    fn the_summary_counts_and_weighs_what_is_kept() {
        let listing = DirListing::parse(&details()).unwrap();
        assert_eq!(listing.summary(), "2 folders · 3 files · 4.3 KB");
        assert_eq!(listing.changed(), 2);
        assert_eq!(listing.ignored(), 2);
        assert_eq!(listing.entries[0].change, Some(ChangeKind::Modified));
    }

    #[test]
    fn other_details_are_not_a_listing() {
        assert!(DirListing::parse(&json!({ "summary": "5 lines" })).is_none());
    }

    #[test]
    fn sizes_and_ages_read_short() {
        assert_eq!(size(83), "83 B");
        assert_eq!(size(4329), "4.2 KB");
        assert_eq!(size(272_948), "267 KB");
        assert_eq!(size(1_468_006), "1.4 MB");
        assert_eq!(age(1_000, 1_030), "now");
        assert_eq!(age(1_000, 1_000 + 8 * 60), "8m");
        assert_eq!(age(0, 3 * 86_400), "3d");
        assert_eq!(age(0, 400 * 86_400), "1y");
    }
}
