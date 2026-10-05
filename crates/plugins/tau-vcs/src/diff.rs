//! Tree diffs as the tools show them: a list of changed paths, and
//! Git-style unified diff text (`docs/reference/vcs.md`, "vcs_diff").

#![allow(
    clippy::disallowed_methods,
    reason = "runs only inside a job in spawn_blocking (ADR 0027)"
)]

use futures_util::StreamExt as _;
use jj_lib::{
    backend::MergedTreeValue,
    conflicts::{
        ConflictMarkerStyle,
        ConflictMaterializeOptions,
        materialize_tree_value,
    },
    diff_presentation::unified::{GitDiffPart, git_diff_part},
    matchers::Matcher,
    merge::Merge,
    merged_tree::MergedTree,
    repo::Repo,
    settings::UserSettings,
    tree_merge::MergeOptions,
};
use pollster::block_on;

use crate::{ChangeKind, FileChange, error::VcsError};

/// The most bytes of diff text a tool returns: 50 KiB, as `tau-tools`.
pub const MAX_DIFF_BYTES: usize = 50 * 1024;

/// Compare native merge meaning, not incidental arity or addend order.
/// The native delta `before - after + absent` cancels to absent exactly
/// when its tree-value terms agree. jj-lib owns flattening/cancellation;
/// file presence, executable bits and copy provenance stay in the values.
fn tree_values_match(
    before: &MergedTreeValue,
    after: &MergedTreeValue,
) -> bool {
    before == after
        || Merge::from_vec(vec![before.clone(), after.clone(), Merge::absent()])
            .flatten()
            .simplify()
            .is_absent()
}

/// The paths that differ between `from` and `to` under `matcher`.
pub(crate) fn changed_paths(
    from: &MergedTree,
    to: &MergedTree,
    matcher: &dyn Matcher,
) -> Result<Vec<FileChange>, VcsError> {
    block_on(async {
        let mut stream = from.diff_stream(to, matcher);
        let mut changes = Vec::new();
        while let Some(entry) = stream.next().await {
            let values = entry.values?;
            // Native snapshots can cancel or reorder conflict terms
            // without editing the file or resolving its conflict.
            if tree_values_match(&values.before, &values.after) {
                continue;
            }
            changes.push(FileChange {
                path: entry.path.as_internal_file_string().to_owned(),
                kind: kind(values.before.is_absent(), values.after.is_absent()),
            });
        }
        Ok(changes)
    })
}

/// The diff from `from` to `to` under `matcher`, as unified diff text
/// with three lines of context, and the paths it touches.
pub(crate) fn unified(
    repo: &dyn Repo,
    settings: &UserSettings,
    from: &MergedTree,
    to: &MergedTree,
    matcher: &dyn Matcher,
) -> Result<(String, Vec<FileChange>), VcsError> {
    let options = ConflictMaterializeOptions {
        marker_style: ConflictMarkerStyle::Diff,
        marker_len: None,
        merge: MergeOptions::from_settings(settings)?,
    };
    let store = repo.store();
    block_on(async {
        let mut stream = from.diff_stream(to, matcher);
        let mut text = String::new();
        let mut changes = Vec::new();
        while let Some(entry) = stream.next().await {
            let path = entry.path;
            let values = entry.values?;
            if tree_values_match(&values.before, &values.after) {
                continue;
            }
            let name = path.as_internal_file_string().to_owned();
            let change =
                kind(values.before.is_absent(), values.after.is_absent());
            let before = materialize_tree_value(
                store,
                &path,
                values.before,
                from.labels(),
            )
            .await?;
            let after =
                materialize_tree_value(store, &path, values.after, to.labels())
                    .await?;
            let before = git_diff_part(&path, before, &options).await.map_err(
                |source| VcsError::Read {
                    name: name.clone(),
                    source,
                },
            )?;
            let after = git_diff_part(&path, after, &options).await.map_err(
                |source| VcsError::Read {
                    name: name.clone(),
                    source,
                },
            )?;
            file_diff(&mut text, &name, &before, &after);
            changes.push(FileChange {
                path: name,
                kind: change,
            });
        }
        Ok((text, changes))
    })
}

fn kind(before_absent: bool, after_absent: bool) -> ChangeKind {
    match (before_absent, after_absent) {
        (true, _) => ChangeKind::Added,
        (_, true) => ChangeKind::Removed,
        _ => ChangeKind::Modified,
    }
}

/// Appends one file's diff, as `git diff` writes it.
fn file_diff(
    out: &mut String,
    path: &str,
    before: &GitDiffPart,
    after: &GitDiffPart,
) {
    out.push_str(&format!("diff --git a/{path} b/{path}\n"));
    match (before.mode, after.mode) {
        (None, Some(mode)) => {
            out.push_str(&format!("new file mode {mode}\n"));
        }
        (Some(mode), None) => {
            out.push_str(&format!("deleted file mode {mode}\n"));
        }
        (Some(old), Some(new)) if old != new => {
            out.push_str(&format!("old mode {old}\nnew mode {new}\n"));
        }
        _ => {}
    }
    if before.content.contents == after.content.contents {
        return;
    }
    let old_name = match before.mode {
        Some(_) => format!("a/{path}"),
        None => "/dev/null".to_owned(),
    };
    let new_name = match after.mode {
        Some(_) => format!("b/{path}"),
        None => "/dev/null".to_owned(),
    };
    if before.content.is_binary || after.content.is_binary {
        out.push_str(&format!(
            "Binary files {old_name} and {new_name} differ\n"
        ));
        return;
    }
    let old = String::from_utf8_lossy(&before.content.contents);
    let new = String::from_utf8_lossy(&after.content.contents);
    out.push_str(&format!("--- {old_name}\n+++ {new_name}\n"));
    unified_hunks(out, &git_lines(&old), &git_lines(&new));
}

/// `text`'s lines as Git splits them: each ends at `\n`, and the last
/// may end without one. A lone `\r` is part of its line, not an end.
fn git_lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

/// Appends the hunks from `old` to `new`, with three lines of context.
/// A line without its `\n` is followed by `\ No newline at end of file`,
/// as Git writes it. `similar`'s own unified diff is not used: it takes
/// a lone `\r` for a line end, so it would split lines Git keeps whole
/// and leave out the marker after a last line that ends in `\r`.
fn unified_hunks(out: &mut String, old: &[&str], new: &[&str]) {
    use similar::{ChangeTag, TextDiff, udiff::UnifiedHunkHeader};

    let diff = TextDiff::from_slices(old, new);
    for group in diff.grouped_ops(3) {
        out.push_str(&format!("{}\n", UnifiedHunkHeader::new(&group)));
        for op in &group {
            for change in diff.iter_changes(op) {
                out.push(match change.tag() {
                    ChangeTag::Equal => ' ',
                    ChangeTag::Delete => '-',
                    ChangeTag::Insert => '+',
                });
                let line = change.value();
                out.push_str(line);
                if !line.ends_with('\n') {
                    out.push_str("\n\\ No newline at end of file\n");
                }
            }
        }
    }
}

/// What follows a diff cut to [`MAX_DIFF_BYTES`] in the model's text.
pub(crate) const CUT_NOTICE: &str = "\n[Diff truncated at 50KB. Pass paths \
                                     to see the rest, a few files at a time.]";

/// Cuts `text` to [`MAX_DIFF_BYTES`] at a line boundary. Whether it cut
/// comes second.
pub(crate) fn cut(text: String) -> (String, bool) {
    if text.len() <= MAX_DIFF_BYTES {
        return (text, false);
    }
    let mut end = MAX_DIFF_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let end = text[..end].rfind('\n').map_or(end, |newline| newline + 1);
    (text[..end].to_owned(), true)
}

#[cfg(test)]
mod tests {
    use hegel::generators as gs;

    use super::*;

    type Leaf = Option<(u8, bool, u8)>;

    fn native_terms(terms: &[Leaf]) -> MergedTreeValue {
        use jj_lib::backend::{CopyId, FileId, TreeValue};
        Merge::from_vec(
            terms
                .iter()
                .map(|value| {
                    value.map(|(blob, executable, copy)| TreeValue::File {
                        // Synthetic identities for a pure algebra test; no store reads.
                        id: FileId::new(vec![blob; 20]),
                        executable,
                        copy_id: CopyId::new(vec![copy]),
                    })
                })
                .collect::<Vec<_>>(),
        )
    }

    fn signed_terms(terms: &[Leaf]) -> std::collections::BTreeMap<Leaf, i32> {
        let mut counts = std::collections::BTreeMap::new();
        for (index, value) in terms.iter().enumerate() {
            *counts.entry(*value).or_default() +=
                if index % 2 == 0 { 1 } else { -1 };
        }
        counts.retain(|_, count| *count != 0);
        counts
    }

    /// An independent signed-term map checks native cancellation, including
    /// file presence, executable bits and copy provenance. Reordering adds
    /// or padding with a cancelling pair is not an edit; a new blob is.
    #[hegel::test(test_cases = 200)]
    fn tree_values_match_the_signed_term_model(tc: hegel::TestCase) {
        let leaf = || {
            gs::optional(hegel::tuples!(
                gs::integers::<u8>().max_value(3),
                gs::booleans(),
                gs::integers::<u8>().max_value(2)
            ))
        };
        let sides: usize = tc.draw(gs::integers().max_value(3));
        let before: Vec<Leaf> = tc.draw(
            gs::vecs(leaf())
                .min_size(2 * sides + 1)
                .max_size(2 * sides + 1),
        );
        let other_sides: usize = tc.draw(gs::integers().max_value(3));
        let after: Vec<Leaf> = tc.draw(
            gs::vecs(leaf())
                .min_size(2 * other_sides + 1)
                .max_size(2 * other_sides + 1),
        );
        let pair: Leaf = tc.draw(leaf());
        assert_eq!(
            tree_values_match(&native_terms(&before), &native_terms(&after)),
            signed_terms(&before) == signed_terms(&after)
        );
        let mut padded = before.clone();
        padded.extend([pair, pair]);
        assert!(tree_values_match(
            &native_terms(&before),
            &native_terms(&padded)
        ));
        let mut reordered = padded;
        let last = reordered.len() - 1;
        reordered.swap(0, last); // both are addends, not a role change
        assert!(tree_values_match(
            &native_terms(&before),
            &native_terms(&reordered)
        ));
        let mut changed = before.clone();
        *changed.last_mut().unwrap() = Some((4, false, 0));
        assert!(!tree_values_match(
            &native_terms(&before),
            &native_terms(&changed)
        ));
        // A nontrivial reordered conflict survives even the smallest draws.
        let witness = [Some((0, false, 0)), Some((1, false, 0)), None];
        let swapped = [None, Some((1, false, 0)), Some((0, false, 0))];
        assert!(tree_values_match(
            &native_terms(&witness),
            &native_terms(&swapped)
        ));
        for partner in [None, Some((0, true, 0)), Some((0, false, 1))] {
            assert!(!tree_values_match(
                &native_terms(&[Some((0, false, 0))]),
                &native_terms(&[partner])
            ));
        }
    }

    /// Text around [`MAX_DIFF_BYTES`] long: runs of one character, some
    /// of them several bytes wide, each maybe ending a line. Long runs
    /// make lines far longer than the limit as well as short ones.
    #[hegel::composite]
    fn text(tc: &hegel::TestCase) -> String {
        let runs: Vec<(char, usize, bool)> = tc.draw(
            gs::vecs(hegel::tuples!(
                gs::sampled_from(vec!['a', 'é', '€', '😀']),
                gs::integers::<usize>().max_value(MAX_DIFF_BYTES / 2),
                gs::booleans(),
            ))
            .max_size(8),
        );
        let mut text = String::new();
        for (c, count, newline) in runs {
            text.extend(std::iter::repeat_n(c, count / c.len_utf8()));
            if newline {
                text.push('\n');
            }
        }
        text
    }

    /// A diff at most [`MAX_DIFF_BYTES`] long comes back whole. A longer
    /// one is cut to its longest prefix that fits and ends a line, or,
    /// when no line ends in reach, to the longest that fits and splits
    /// no character.
    #[hegel::test(test_cases = 300)]
    fn a_cut_keeps_the_most_whole_lines_that_fit(tc: hegel::TestCase) {
        let text = tc.draw(text());
        let (kept, was_cut) = cut(text.clone());
        assert_eq!(was_cut, text.len() > MAX_DIFF_BYTES);
        if !was_cut {
            assert_eq!(kept, text);
            return;
        }
        let fits = (0..=MAX_DIFF_BYTES)
            .rev()
            .filter(|&end| text.is_char_boundary(end));
        let expected = fits
            .clone()
            .find(|&end| text[..end].ends_with('\n'))
            .or_else(|| fits.max())
            .unwrap();
        assert_eq!(kept.len(), expected);
        assert!(text.starts_with(&kept));
    }
}
