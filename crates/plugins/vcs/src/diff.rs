//! Tree diffs as the tools show them: a list of changed paths, and
//! Git-style unified diff text (`docs/reference/vcs.md`, "vcs_diff").

use futures_util::StreamExt as _;
use jj_lib::{
    conflicts::{
        ConflictMarkerStyle,
        ConflictMaterializeOptions,
        materialize_tree_value,
    },
    diff_presentation::unified::{GitDiffPart, git_diff_part},
    matchers::Matcher,
    merged_tree::MergedTree,
    repo::Repo,
    settings::UserSettings,
    tree_merge::MergeOptions,
};
use pollster::block_on;

use crate::{ChangeKind, FileChange, error::VcsError};

/// The most bytes of diff text a tool returns: 50 KiB, as `tau-tools`.
pub const MAX_DIFF_BYTES: usize = 50 * 1024;

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
    let diff = similar::TextDiff::from_lines(old.as_ref(), new.as_ref());
    let unified = diff
        .unified_diff()
        .context_radius(3)
        .missing_newline_hint(true)
        .header(&old_name, &new_name)
        .to_string();
    out.push_str(&unified);
    if !out.ends_with('\n') {
        out.push('\n');
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
