//! What an `ls` result shows: a directory's entries with their kind,
//! size and age, whether `.gitignore` leaves them out, and, with the
//! vcs plugin, what `@` changes in them.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::details::Entry;

/// How the run's change touches a path, as the vcs plugin marks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    Added,
    Modified,
    Removed,
}

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

/// `83 B`, `4.2 KB`, `267 KB`, `1.4 MB`: the smallest unit whose
/// number, as printed, stays under 1024, with one decimal but for KB from
/// 10 up. The unit is picked after rounding, so 1023.99 KB is `1.0 MB`.
pub fn size(bytes: u64) -> String {
    const KB: f64 = 1024.;
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let kb = bytes as f64 / KB;
    let mb = kb / KB;
    if (kb * 10.).round() < 100. {
        format!("{kb:.1} KB")
    } else if kb.round() < KB {
        format!("{kb:.0} KB")
    } else if (mb * 10.).round() < KB * 10. {
        format!("{mb:.1} MB")
    } else {
        format!("{:.1} GB", mb / KB)
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

    /// The byte counts where `size`'s rule changes: the first KB, the
    /// rounding to 10 KB (9.95 KB) and to the next unit (1023.5 KB,
    /// 1023.95 MB), and each unit, one byte either side.
    fn size_boundaries() -> Vec<u64> {
        let edges = [1_024u64, 10_189, 10_240, 1_048_064, 1_048_576]
            .into_iter()
            .chain([1_073_689_396, 1 << 30]);
        edges.flat_map(|edge| [edge - 1, edge, edge + 1]).collect()
    }

    /// A size reads as bytes below 1 KiB, else as a number of the
    /// smallest unit (KB, MB, GB) that keeps it, as printed, under 1024:
    /// within half its last digit of the exact amount, at least 1, with
    /// one decimal, but for KB from 10 up, which has none.
    #[hegel::test(test_cases = 500)]
    // These read "10.0 KB" and "1024 KB" when the unit was picked before
    // rounding.
    #[hegel::explicit_test_case(bytes = 10_189_u64)]
    #[hegel::explicit_test_case(bytes = 1_048_575_u64)]
    fn a_size_reads_in_the_smallest_unit_that_keeps_it_short(
        tc: hegel::TestCase,
    ) {
        use hegel::generators as gs;
        let bytes: u64 = tc.draw(hegel::one_of!(
            gs::sampled_from(size_boundaries()),
            gs::integers::<u64>().max_value(1 << 21),
            gs::integers::<u64>().max_value(1 << 34),
        ));
        let text = size(bytes);
        let (number, unit) = text.split_once(' ').expect("a number and a unit");
        if unit == "B" {
            assert!(bytes < 1024, "{text}");
            assert_eq!(number, bytes.to_string());
            return;
        }
        assert!(bytes >= 1024, "{text}");
        let scale = match unit {
            "KB" => 1024f64,
            "MB" => 1024f64 * 1024.,
            "GB" => 1024f64 * 1024. * 1024.,
            other => panic!("{other} is no unit: {text}"),
        };
        let value: f64 = number.parse().unwrap();
        let decimals = number.split_once('.').map_or(0, |(_, d)| d.len());
        let wanted = if unit == "KB" && value >= 10. { 0 } else { 1 };
        assert_eq!(decimals, wanted, "{bytes} as {text}");
        let step = if decimals == 0 { 1. } else { 0.1 };
        let exact = bytes as f64 / scale;
        assert!(
            (value - exact).abs() <= step / 2. + 1e-9,
            "{bytes} as {text}"
        );
        assert!(value >= 1., "{bytes} as {text}: a smaller unit keeps it");
        if unit != "GB" {
            assert!(value < 1024., "{bytes} as {text}: a larger unit keeps it");
        }
    }

    /// Units of time, largest last, and their names.
    const UNITS: [(i64, &str); 5] = [
        (60, "m"),
        (3_600, "h"),
        (86_400, "d"),
        (2_592_000, "mo"),
        (31_536_000, "y"),
    ];

    /// An age is `now` under a minute (or in the future), else the whole
    /// number of the largest unit it reaches.
    #[hegel::test(test_cases = 500)]
    fn an_age_is_the_largest_unit_it_reaches(tc: hegel::TestCase) {
        use hegel::generators as gs;
        let edges: Vec<i64> = UNITS
            .iter()
            .flat_map(|(unit, _)| [unit - 1, *unit, unit + 1])
            .chain([-1, 0, 59])
            .collect();
        let elapsed: i64 = tc.draw(hegel::one_of!(
            gs::sampled_from(edges),
            gs::integers::<i64>()
                .min_value(-100)
                .max_value(10 * 31_536_000),
        ));
        let modified: i64 = tc
            .draw(gs::integers::<i64>().min_value(0).max_value(2_000_000_000));
        let text = age(modified, modified + elapsed);
        match UNITS.iter().rev().find(|(unit, _)| elapsed >= *unit) {
            None => assert_eq!(text, "now", "{elapsed}"),
            Some((unit, name)) => {
                assert_eq!(text, format!("{}{name}", elapsed / unit))
            }
        }
    }
}
