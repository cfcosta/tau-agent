//! What each tool does, on the workspace's thread
//! (`docs/reference/vcs.md`, "Tools"). Each returns the text the model
//! sees and the `details` value callers get.

use std::collections::HashSet;

use futures_util::StreamExt as _;
use jj_lib::{
    backend::{CommitId, CopyId, TreeValue},
    commit::Commit,
    conflicts,
    files::{self, MergeResult},
    matchers::{EverythingMatcher, FilesMatcher},
    merge::Merge,
    merged_tree_builder::MergedTreeBuilder,
    object_id::ObjectId as _,
    op_store::{OperationId, RefTarget},
    operation::Operation,
    ref_name::{RefName, WorkspaceName},
    repo::{ReadonlyRepo, Repo},
    repo_path::RepoPathBuf,
    revset::ResolvedRevsetExpression,
    rewrite::restore_tree,
    transaction::Transaction,
};
use pollster::block_on;
use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    ChangeInfo,
    FileChange,
    diff,
    error::VcsError,
    session::{
        self,
        BOOKMARK_ATTRIBUTE,
        CAUGHT_UP_ATTRIBUTE,
        LANDED_ATTRIBUTE,
        Snapshot,
        TOOL_ATTRIBUTE,
        UNDO_ATTRIBUTE,
        is_immutable,
        resolve,
    },
    vcs::Worker,
};

/// The default and largest `limit` of `vcs_log`.
pub const DEFAULT_LOG_LIMIT: u32 = 10;
pub const MAX_LOG_LIMIT: u32 = 100;

/// A tool's result: text for the model, details for callers.
pub(crate) struct Report {
    pub text: String,
    pub details: Value,
}

impl ChangeInfo {
    pub(crate) fn of(
        repo: &dyn Repo,
        commit: &Commit,
        wc: &CommitId,
    ) -> Result<Self, VcsError> {
        Ok(Self {
            change_id: commit.change_id().reverse_hex(),
            commit_id: commit.id().hex(),
            description: commit.description().to_owned(),
            empty: block_on(commit.is_empty(repo))?,
            conflict: commit.has_conflict(),
            immutable: is_immutable(repo, commit.id())?,
            working_copy: commit.id() == wc,
            divergent: block_on(repo.resolve_change_id(commit.change_id()))?
                .is_some_and(|targets| {
                    targets.visible_with_offsets().nth(1).is_some()
                }),
            bookmarks: {
                let mut names: Vec<String> = repo
                    .view()
                    .local_bookmarks_for_commit(commit.id())
                    .map(|(name, _)| name.as_str().to_owned())
                    .collect();
                names.sort();
                names
            },
        })
    }

    /// `<change> <commit> [flags] [bookmarks] <first line>`, as
    /// `vcs_log` rows.
    fn line(&self) -> String {
        let mut line = format!(
            "{} {}",
            &self.change_id[..self.change_id.len().min(session::SHORT_ID)],
            &self.commit_id[..self.commit_id.len().min(session::SHORT_ID)],
        );
        for (on, flag) in [
            (self.working_copy, "@"),
            (self.empty, "(empty)"),
            (self.conflict, "(conflict)"),
            (self.divergent, "(divergent)"),
            (self.immutable, "(immutable)"),
        ] {
            if on {
                line.push(' ');
                line.push_str(flag);
            }
        }
        if !self.bookmarks.is_empty() {
            line.push_str(&format!(" [{}]", self.bookmarks.join(", ")));
        }
        line.push(' ');
        line.push_str(first_line(&self.description));
        line
    }
}

fn first_line(description: &str) -> &str {
    match description.lines().next().map(str::trim) {
        Some(line) if !line.is_empty() => line,
        _ => "(no description set)",
    }
}

/// The paths the model gave, in the repo.
fn repo_paths(
    worker: &mut Worker,
    paths: &[String],
) -> Result<Vec<RepoPathBuf>, VcsError> {
    let root = worker.root().to_owned();
    let workspace = worker.workspace()?;
    paths
        .iter()
        .map(|path| session::repo_path(workspace, &root, path))
        .collect()
}

fn workspace_name(
    worker: &mut Worker,
) -> Result<jj_lib::ref_name::WorkspaceNameBuf, VcsError> {
    Ok(worker.workspace()?.workspace_name().to_owned())
}

fn wc_line(snapshot: &Snapshot) -> Result<(String, ChangeInfo), VcsError> {
    let info =
        ChangeInfo::of(snapshot.repo.as_ref(), &snapshot.wc, snapshot.wc.id())?;
    Ok((format!("Working copy (@): {}", info.line()), info))
}

pub(crate) fn status(worker: &mut Worker) -> Result<Report, VcsError> {
    let settings = worker.workspace()?.settings().clone();
    let snapshot = session::snapshot(worker)?;
    let repo = snapshot.repo.as_ref();
    let wc = &snapshot.wc;
    let (mut text, wc_info) = wc_line(&snapshot)?;
    let parents = block_on(wc.parents())?;
    let mut parent_infos = Vec::new();
    for parent in &parents {
        let info = ChangeInfo::of(repo, parent, wc.id())?;
        text.push_str(&format!("\nParent (@-):      {}", info.line()));
        parent_infos.push(info);
    }

    // The diff is for callers that draw it; the model asks vcs_diff.
    let parent_tree = block_on(wc.parent_tree(repo))?;
    let (diff_text, changes) = diff::unified(
        repo,
        &settings,
        &parent_tree,
        &wc.tree(),
        &jj_lib::matchers::EverythingMatcher,
    )?;
    let (diff_text, truncated) = diff::cut(diff_text);
    if changes.is_empty() {
        text.push_str("\nThe working copy has no changes.");
    } else {
        text.push_str("\nWorking copy changes:");
        for change in &changes {
            text.push_str(&format!(
                "\n{} {}",
                change.kind.letter(),
                change.path
            ));
        }
    }

    let conflicts: Vec<String> = wc
        .tree()
        .conflicts()
        .map(|(path, _)| path.as_internal_file_string().to_owned())
        .collect();
    if !conflicts.is_empty() {
        text.push_str(
            "\nUnresolved conflicts (some files have no markers; edit markers, \
             use vcs_restore to select a committed side, or vcs_resolve to \
             accept a markerless file's current contents):",
        );
        for path in &conflicts {
            text.push_str(&format!("\n{path}"));
        }
    }
    if !snapshot.too_large.is_empty() {
        text.push_str(
            "\nLeft out of @ (new files over 1 MiB are not snapshotted):",
        );
        for file in &snapshot.too_large {
            text.push_str(&format!(
                "\n{} ({:.1} MiB)",
                file.path,
                file.size as f64 / (1024. * 1024.)
            ));
        }
    }
    Ok(Report {
        text,
        details: json!({
            "working_copy": wc_info,
            "parents": parent_infos,
            "changes": changes,
            "conflicts": conflicts,
            "too_large": snapshot.too_large,
            "diff": diff_text,
            "truncated": truncated,
        }),
    })
}

/// What `@` changes against its parents, after a snapshot.
pub(crate) fn changes(
    worker: &mut Worker,
) -> Result<Vec<FileChange>, VcsError> {
    let snapshot = session::snapshot(worker)?;
    let wc = &snapshot.wc;
    let parent_tree = block_on(wc.parent_tree(snapshot.repo.as_ref()))?;
    diff::changed_paths(&parent_tree, &wc.tree(), &EverythingMatcher)
}

pub(crate) fn diff(
    worker: &mut Worker,
    change: Option<String>,
    paths: Vec<String>,
) -> Result<Report, VcsError> {
    let paths = repo_paths(worker, &paths)?;
    let settings = worker.workspace()?.settings().clone();
    let snapshot = session::snapshot(worker)?;
    let repo = snapshot.repo.as_ref();
    let commit = match &change {
        Some(rev) => resolve(repo, rev)?,
        None => snapshot.wc.clone(),
    };
    let parent_tree = block_on(commit.parent_tree(repo))?;
    let matcher = session::matcher(paths);
    let (text, files) = diff::unified(
        repo,
        &settings,
        &parent_tree,
        &commit.tree(),
        matcher.as_ref(),
    )?;
    let info = ChangeInfo::of(repo, &commit, snapshot.wc.id())?;
    let (diff, truncated) = diff::cut(text);
    let text = if diff.is_empty() {
        format!("No changes in {}.", info.line())
    } else if truncated {
        format!("{diff}{}", diff::CUT_NOTICE)
    } else {
        diff.clone()
    };
    Ok(Report {
        text,
        details: json!({
            "change": info,
            "files": files,
            "diff": diff,
            "truncated": truncated,
        }),
    })
}

pub(crate) fn log(worker: &mut Worker, limit: u32) -> Result<Report, VcsError> {
    let limit = limit.clamp(1, MAX_LOG_LIMIT) as usize;
    let snapshot = session::snapshot(worker)?;
    let repo = snapshot.repo.as_ref();
    let expression = ResolvedRevsetExpression::commit(snapshot.wc.id().clone())
        .ancestors()
        .minus(&ResolvedRevsetExpression::root());
    let revset = expression.evaluate(repo)?;
    let ids: Vec<CommitId> =
        block_on(revset.stream().take(limit + 1).collect::<Vec<_>>())
            .into_iter()
            .collect::<Result<_, _>>()?;
    let more = ids.len() > limit;
    let mut rows = Vec::new();
    let mut infos = Vec::new();
    for id in ids.iter().take(limit) {
        let commit = repo.store().get_commit(id)?;
        let info = ChangeInfo::of(repo, &commit, snapshot.wc.id())?;
        rows.push(info.line());
        infos.push(info);
    }
    let mut text = if rows.is_empty() {
        "No changes yet.".to_owned()
    } else {
        rows.join("\n")
    };
    if more {
        text.push_str(&format!(
            "\n\n[Showing the newest {limit} changes. Use limit={} for more]",
            (limit * 2).min(MAX_LOG_LIMIT as usize)
        ));
    }
    Ok(Report {
        text,
        details: json!({ "changes": infos, "more": more }),
    })
}

pub(crate) fn show(
    worker: &mut Worker,
    change: String,
) -> Result<Report, VcsError> {
    let settings = worker.workspace()?.settings().clone();
    let snapshot = session::snapshot(worker)?;
    let repo = snapshot.repo.as_ref();
    let commit = resolve(repo, &change)?;
    let info = ChangeInfo::of(repo, &commit, snapshot.wc.id())?;
    let author = commit.author();
    let mut text = format!(
        "Change ID: {}\nCommit ID: {}\nAuthor: {} <{}>",
        info.change_id, info.commit_id, author.name, author.email
    );
    let mut parents = Vec::new();
    for parent in block_on(commit.parents())? {
        let parent = ChangeInfo::of(repo, &parent, snapshot.wc.id())?;
        text.push_str(&format!("\nParent: {}", parent.line()));
        parents.push(parent);
    }
    let flags: Vec<&str> = [
        (info.working_copy, "working copy (@)"),
        (info.empty, "empty"),
        (info.conflict, "conflict"),
        (info.immutable, "immutable"),
    ]
    .into_iter()
    .filter_map(|(on, flag)| on.then_some(flag))
    .collect();
    if !flags.is_empty() {
        text.push_str(&format!("\nFlags: {}", flags.join(", ")));
    }
    text.push_str("\n\n");
    if commit.description().trim().is_empty() {
        text.push_str("    (no description set)\n");
    } else {
        for line in commit.description().trim_end().lines() {
            text.push_str(&format!("    {line}\n"));
        }
    }
    let parent_tree = block_on(commit.parent_tree(repo))?;
    let (diff_text, files) = diff::unified(
        repo,
        &settings,
        &parent_tree,
        &commit.tree(),
        &jj_lib::matchers::EverythingMatcher,
    )?;
    let (diff_text, truncated) = diff::cut(diff_text);
    if !diff_text.is_empty() {
        text.push('\n');
        text.push_str(&diff_text);
        if truncated {
            text.push_str(diff::CUT_NOTICE);
        }
    }
    Ok(Report {
        text: text.trim_end().to_owned(),
        details: json!({
            "change": info,
            "parents": parents,
            "author": { "name": author.name, "email": author.email },
            "files": files,
            "diff": diff_text,
            "truncated": truncated,
        }),
    })
}

/// A description as jj stores it: trailing whitespace trimmed, and one
/// final newline unless empty.
fn description(message: &str) -> String {
    let trimmed = message.trim_end();
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}\n")
    }
}

pub(crate) fn describe(
    worker: &mut Worker,
    message: String,
) -> Result<Report, VcsError> {
    let (snapshot, ()) = session::mutate(worker, "describe", |tx, wc| {
        session::write_commit(tx, |repo| {
            repo.rewrite_commit(wc)
                .set_description(description(&message))
        })?;
        Ok(())
    })?;
    let (line, info) = wc_line(&snapshot)?;
    Ok(Report {
        text: format!("Described the working copy.\n{line}"),
        details: json!({ "working_copy": info }),
    })
}

/// Describes `@` as `message` and starts a new change on top. With
/// `paths`, only those go into the commit: `@` keeps its parent's
/// version of everything else, and the new change on top holds the rest
/// of the work, so the files on disk do not move.
pub(crate) fn commit(
    worker: &mut Worker,
    message: String,
    paths: Option<Vec<String>>,
) -> Result<Report, VcsError> {
    if message.trim().is_empty() {
        return Err(VcsError::EmptyDescription);
    }
    let matcher = match &paths {
        None => None,
        Some(paths) if paths.is_empty() => return Err(VcsError::NoPaths),
        Some(paths) => Some(session::matcher(repo_paths(worker, paths)?)),
    };
    let name = workspace_name(worker)?;
    let settings = worker.workspace()?.settings().clone();
    let (snapshot, (committed, left)) =
        session::mutate(worker, "commit", |tx, wc| {
            let Some(matcher) = &matcher else {
                let committed = session::write_commit(tx, |repo| {
                    repo.rewrite_commit(wc)
                        .set_description(description(&message))
                })?;
                block_on(tx.repo_mut().rebase_descendants())?;
                block_on(tx.repo_mut().check_out(name, &committed))?;
                return Ok((committed, Vec::new()));
            };
            let parent = block_on(wc.parent_tree(tx.repo()))?;
            let all =
                diff::changed_paths(&parent, &wc.tree(), &EverythingMatcher)?;
            // The parent's tree, with the chosen paths as `@` has them.
            let chosen = block_on(restore_tree(
                &wc.tree(),
                &parent,
                "working copy".to_owned(),
                "parent".to_owned(),
                matcher.as_ref(),
            ))?;
            let picked =
                diff::changed_paths(&parent, &chosen, matcher.as_ref())?;
            if picked.is_empty() {
                let changed: Vec<&str> =
                    all.iter().map(|file| file.path.as_str()).collect();
                return Err(VcsError::NothingToCommit(
                    paths.clone().unwrap_or_default().join(", "),
                    if changed.is_empty() {
                        "nothing".to_owned()
                    } else {
                        changed.join(", ")
                    },
                ));
            }
            let committed = session::write_commit(tx, |repo| {
                repo.rewrite_commit(wc)
                    .set_tree(chosen.clone())
                    .set_description(description(&message))
            })?;
            block_on(tx.repo_mut().rebase_descendants())?;
            // The rest of the work, on top: `@`'s files as they were.
            let rest = block_on(
                tx.repo_mut()
                    .new_commit(vec![committed.id().clone()], wc.tree())
                    .write(),
            )?;
            block_on(tx.repo_mut().edit(name, &rest))?;
            let left: Vec<String> = all
                .into_iter()
                .filter(|file| !picked.iter().any(|p| p.path == file.path))
                .map(|file| file.path)
                .collect();
            Ok((committed, left))
        })?;
    let repo = snapshot.repo.as_ref();
    let commit = repo.store().get_commit(committed.id())?;
    // What the card draws: the change on its parent with its diff, and
    // what stays in the new `@` on top of it.
    let parent = match block_on(commit.parents())?.first() {
        Some(parent) => Some(ChangeInfo::of(repo, parent, snapshot.wc.id())?),
        None => None,
    };
    let (diff_text, files) = diff::unified(
        repo,
        &settings,
        &block_on(commit.parent_tree(repo))?,
        &commit.tree(),
        &EverythingMatcher,
    )?;
    let (diff_text, truncated) = diff::cut(diff_text);
    let (left_text, left_files) = diff::unified(
        repo,
        &settings,
        &commit.tree(),
        &snapshot.wc.tree(),
        &EverythingMatcher,
    )?;
    let (left_text, left_truncated) = diff::cut(left_text);
    let committed = ChangeInfo::of(repo, &commit, snapshot.wc.id())?;
    let (line, info) = wc_line(&snapshot)?;
    let mut text = format!("Committed change {}", committed.line());
    if !left.is_empty() {
        text.push_str(&format!(
            "\nLeft uncommitted in the working copy: {}",
            left.join(", ")
        ));
    }
    Ok(Report {
        text: format!("{text}\n{line}"),
        details: json!({
            "committed": committed,
            "working_copy": info,
            "left": left,
            "parent": parent,
            "files": files,
            "diff": diff_text,
            "truncated": truncated,
            "left_files": left_files,
            "left_diff": left_text,
            "left_truncated": left_truncated,
        }),
    })
}

pub(crate) fn new(
    worker: &mut Worker,
    message: Option<String>,
) -> Result<Report, VcsError> {
    let name = workspace_name(worker)?;
    let (snapshot, ()) = session::mutate(worker, "new", |tx, wc| {
        let child = block_on(
            tx.repo_mut()
                .new_commit(vec![wc.id().clone()], wc.tree())
                .set_description(description(message.as_deref().unwrap_or("")))
                .write(),
        )?;
        block_on(tx.repo_mut().edit(name, &child))?;
        Ok(())
    })?;
    let (line, info) = wc_line(&snapshot)?;
    Ok(Report {
        text: format!("Started a new change.\n{line}"),
        details: json!({ "working_copy": info }),
    })
}

/// The `tau.vcs.tool` value of what the host writes, not the model's
/// tools: a turn's snapshot, and a run's last commit. `vcs_undo` treats
/// it as someone else's.
pub(crate) const CHECKPOINT: &str = "checkpoint";

/// A change the host committed for the run: the one holding the files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct Committed {
    pub commit_id: String,
    pub change_id: String,
    /// `@` held changes, so a new commit was made for them.
    pub changed: bool,
    /// The paths the commit added, changed or removed.
    pub paths: Vec<String>,
}

/// Commits whatever `@` holds (described as `message` unless the model
/// described it), starts an empty working copy on top, and points
/// `bookmark` at the run's newest commit. With nothing in `@`, the
/// commit is `@`'s parent, and only the bookmark may move. For a run's
/// last commit, and for tests.
pub(crate) fn commit_all(
    worker: &mut Worker,
    message: String,
    bookmark: &str,
) -> Result<Committed, VcsError> {
    let name = workspace_name(worker)?;
    let record = crate::land::MovedOnto::load(worker)?;
    let mut followed = None;
    let (_, committed) = session::mutate(worker, CHECKPOINT, |tx, wc| {
        let (wc, onto) = crate::land::follow_bookmark(
            tx,
            wc,
            &name,
            bookmark,
            record.last(),
        )?;
        followed = onto;
        let wc = &wc;
        let committed = if block_on(wc.is_empty(tx.repo()))? {
            let parent = wc.parent_ids().first().ok_or(VcsError::NoParent)?;
            let parent = tx.repo().store().get_commit(parent)?;
            Committed {
                commit_id: parent.id().hex(),
                change_id: parent.change_id().reverse_hex(),
                changed: false,
                paths: Vec::new(),
            }
        } else {
            let text = if wc.description().trim().is_empty() {
                description(&message)
            } else {
                wc.description().to_owned()
            };
            let parent_tree = block_on(wc.parent_tree(tx.repo()))?;
            let paths = diff::changed_paths(
                &parent_tree,
                &wc.tree(),
                &jj_lib::matchers::EverythingMatcher,
            )?
            .into_iter()
            .map(|change| change.path)
            .collect();
            let committed = session::write_commit(tx, |repo| {
                repo.rewrite_commit(wc).set_description(text.clone())
            })?;
            block_on(tx.repo_mut().rebase_descendants())?;
            block_on(tx.repo_mut().check_out(name.clone(), &committed))?;
            Committed {
                commit_id: committed.id().hex(),
                change_id: committed.change_id().reverse_hex(),
                changed: true,
                paths,
            }
        };
        point_bookmark(tx, bookmark, &committed.commit_id, !committed.changed)?;
        Ok(committed)
    })?;
    record.save(followed.as_ref())?;
    Ok(committed)
}

/// Points `bookmark` at `commit` (hex) if it points elsewhere. `quiet`
/// marks an operation that does nothing else, for undo to pass over.
fn point_bookmark(
    tx: &mut Transaction,
    bookmark: &str,
    commit: &str,
    quiet: bool,
) -> Result<(), VcsError> {
    let head = CommitId::try_from_hex(commit).ok_or(VcsError::NotHex)?;
    let name = RefName::new(bookmark);
    if tx.repo().view().get_local_bookmark(name).as_normal() != Some(&head) {
        tx.repo_mut()
            .set_local_bookmark_target(name, RefTarget::normal(head));
        if quiet {
            tx.set_attribute(
                BOOKMARK_ATTRIBUTE.to_owned(),
                bookmark.to_owned(),
            );
        }
    }
    Ok(())
}

/// Where a turn left the code: a snapshot of `@`, which the model's
/// commits stack under (ADR 0014).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct TurnSnapshot {
    /// The snapshot: `@` as the turn left it. Later snapshots rewrite
    /// `@` under the same change id, so this commit is found by its id.
    pub commit_id: String,
    pub change_id: String,
    /// The run's newest commit, under `@`, which its bookmark names.
    pub head: String,
    /// The paths the turn added, changed or removed.
    pub paths: Vec<String>,
}

/// Ends a turn: snapshots `@` without committing it, and points
/// `bookmark` at the run's newest commit. `since` is the turn before's
/// snapshot, to tell what this turn changed; without it, `@`'s own
/// changes count.
pub(crate) fn end_turn(
    worker: &mut Worker,
    bookmark: &str,
    since: Option<&str>,
) -> Result<TurnSnapshot, VcsError> {
    let name = workspace_name(worker)?;
    let record = crate::land::MovedOnto::load(worker)?;
    let mut followed = None;
    let (_, turn) = session::mutate(worker, CHECKPOINT, |tx, snapped| {
        let (wc, onto) = crate::land::follow_bookmark(
            tx,
            snapped,
            &name,
            bookmark,
            record.last(),
        )?;
        followed = onto;
        let wc = &wc;
        let head = wc.parent_ids().first().ok_or(VcsError::NoParent)?.clone();
        let before = match since.and_then(CommitId::try_from_hex) {
            Some(id) => {
                let mut moves =
                    moved_since(tx.base_repo().operation(), &name, &id)?;
                // This operation's own step after an update.
                if wc.id() != snapped.id() {
                    moves.push((snapped.id().clone(), wc.id().clone()));
                }
                replay(
                    tx.repo(),
                    tx.repo().store().get_commit(&id)?.tree(),
                    &moves,
                )?
            }
            None => block_on(wc.parent_tree(tx.repo()))?,
        };
        let paths =
            diff::changed_paths(&before, &wc.tree(), &EverythingMatcher)?
                .into_iter()
                .map(|change| change.path)
                .collect();
        point_bookmark(tx, bookmark, &head.hex(), true)?;
        Ok(TurnSnapshot {
            commit_id: wc.id().hex(),
            change_id: wc.change_id().reverse_hex(),
            head: head.hex(),
            paths,
        })
    })?;
    record.save(followed.as_ref())?;
    Ok(turn)
}

/// How this workspace's run moved under it since the turn whose
/// snapshot is `since`, oldest first: for each landing (`land`) and
/// catch-up (`move_onto`, or a step after an update) since, in any
/// workspace, the run's newest commit before and after it, when it
/// changed. A turn's paths count from `since` with these moves replayed
/// on it ([`replay`]), so what they brought is not the turn's, and
/// nothing else the run's commits went through is followed: an undo or a
/// commit of the run's own is the turn's. The walk back stops where
/// `since` was this workspace's working copy, or where the workspace did
/// not exist yet.
fn moved_since(
    head: &Operation,
    name: &WorkspaceName,
    since: &CommitId,
) -> Result<Vec<(CommitId, CommitId)>, VcsError> {
    let mut moves = Vec::new();
    let mut op = head.clone();
    loop {
        let view = block_on(op.view())?;
        let after = match view.get_wc_commit_id(name) {
            None => break,
            Some(wc) if wc == since => break,
            Some(wc) => wc.clone(),
        };
        // Concurrent operations merge into one: follow the first.
        let Some(parent) = block_on(op.parents())?.into_iter().next() else {
            break;
        };
        let attributes = &op.metadata().attributes;
        if attributes.contains_key(LANDED_ATTRIBUTE)
            || attributes.contains_key(CAUGHT_UP_ATTRIBUTE)
        {
            let before =
                block_on(parent.view())?.get_wc_commit_id(name).cloned();
            if let Some(before) = before {
                moves.push((before, after));
            }
        }
        op = parent;
    }
    moves.reverse();
    Ok(moves)
}

/// `base` with each move's change applied on top, oldest first: the
/// run's newest commit (the working copy's parent) after it, against the
/// one before. `moves` name working-copy commits.
fn replay(
    repo: &dyn Repo,
    mut base: jj_lib::merged_tree::MergedTree,
    moves: &[(CommitId, CommitId)],
) -> Result<jj_lib::merged_tree::MergedTree, VcsError> {
    for (before, after) in moves {
        let head = |id: &CommitId| -> Result<Option<CommitId>, VcsError> {
            let commit = repo.store().get_commit(id)?;
            Ok(commit.parent_ids().first().cloned())
        };
        let (Some(from), Some(to)) = (head(before)?, head(after)?) else {
            continue;
        };
        if from == to {
            continue;
        }
        let from = repo.store().get_commit(&from)?.tree();
        let to = repo.store().get_commit(&to)?.tree();
        base = block_on(jj_lib::merged_tree::MergedTree::merge(
            jj_lib::merge::Merge::from_vec(vec![
                (base, "the turn before".to_owned()),
                (from, "the run's head before".to_owned()),
                (to, "the run's head after".to_owned()),
            ]),
        ))?;
    }
    Ok(base)
}

/// What `@` holds, after a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkingCopy {
    /// The paths `@` changes against its parent: none when all the work
    /// is committed.
    pub paths: Vec<String>,
    /// New files omitted by jj-lib's snapshot size limit. They must not
    /// disappear when a child workspace is removed.
    pub too_large: Vec<crate::TooLarge>,
    /// `@`'s parent, the run's newest commit, in hex.
    pub head: String,
}

impl WorkingCopy {
    pub fn is_committed(&self) -> bool {
        self.paths.is_empty() && self.too_large.is_empty()
    }
}

/// The paths `@` holds in conflict, after a snapshot, in path order:
/// what `vcs_status` lists as unresolved.
pub(crate) fn conflicts(worker: &mut Worker) -> Result<Vec<String>, VcsError> {
    let snapshot = session::snapshot(worker)?;
    Ok(snapshot
        .wc
        .tree()
        .conflicts()
        .map(|(path, _)| path.as_internal_file_string().to_owned())
        .collect())
}

pub(crate) fn working_copy(
    worker: &mut Worker,
) -> Result<WorkingCopy, VcsError> {
    let snapshot = session::snapshot(worker)?;
    let wc = &snapshot.wc;
    let head = wc.parent_ids().first().ok_or(VcsError::NoParent)?.hex();
    let parent_tree = block_on(wc.parent_tree(snapshot.repo.as_ref()))?;
    let paths =
        diff::changed_paths(&parent_tree, &wc.tree(), &EverythingMatcher)?
            .into_iter()
            .map(|change| change.path)
            .collect();
    Ok(WorkingCopy {
        paths,
        too_large: snapshot.too_large,
        head,
    })
}

pub(crate) fn restore(
    worker: &mut Worker,
    paths: Vec<String>,
    from: Option<String>,
) -> Result<Report, VcsError> {
    if paths.is_empty() {
        return Err(VcsError::NoPaths);
    }
    let paths = repo_paths(worker, &paths)?;
    let matcher = session::matcher(paths);
    let (snapshot, restored) = session::mutate(worker, "restore", |tx, wc| {
        let source = match &from {
            Some(rev) => resolve(tx.repo(), rev)?.tree(),
            None => block_on(wc.parent_tree(tx.repo()))?,
        };
        let new_tree = block_on(restore_tree(
            &source,
            &wc.tree(),
            "source".to_owned(),
            "working copy".to_owned(),
            matcher.as_ref(),
        ))?;
        let restored =
            diff::changed_paths(&wc.tree(), &new_tree, matcher.as_ref())?;
        if !restored.is_empty() {
            session::write_commit(tx, |repo| {
                repo.rewrite_commit(wc).set_tree(new_tree.clone())
            })?;
        }
        Ok(restored)
    })?;
    let (line, info) = wc_line(&snapshot)?;
    let mut text = if restored.is_empty() {
        "Nothing to restore: those paths already match.".to_owned()
    } else {
        let mut text = "Restored:".to_owned();
        for FileChange { path, .. } in &restored {
            text.push_str(&format!("\n{path}"));
        }
        text
    };
    text.push('\n');
    text.push_str(&line);
    Ok(Report {
        text,
        details: json!({ "restored": restored, "working_copy": info }),
    })
}

/// Explicitly accept jj-lib's clean materialization of selected conflicts.
/// Marked or non-file conflicts require an edit or a committed-side restore.
/// The native builder writes only after every selected conflict validates.
pub(crate) fn resolve_conflicts(
    worker: &mut Worker,
    paths: Vec<String>,
) -> Result<Report, VcsError> {
    if paths.is_empty() {
        return Err(VcsError::NoPaths);
    }
    let paths = repo_paths(worker, &paths)?;
    let matcher = session::matcher(paths);
    let (snapshot, resolved) = session::mutate(worker, "resolve", |tx, wc| {
        let tree = wc.tree();
        let mut builder = MergedTreeBuilder::new(tree.clone());
        let mut resolved = Vec::new();
        for (path, value) in tree.conflicts_matching(matcher.as_ref()) {
            let value = value?;
            let file =
                block_on(conflicts::try_materialize_file_conflict_value(
                    tree.store(),
                    &path,
                    &value,
                    tree.labels(),
                ))?
                .ok_or_else(|| {
                    VcsError::NeedsConflictEdit(
                        path.as_internal_file_string().to_owned(),
                    )
                })?;
            let MergeResult::Resolved(content) = files::merge_hunks(
                &file.contents,
                tree.store().merge_options(),
            ) else {
                return Err(VcsError::NeedsConflictEdit(
                    path.as_internal_file_string().to_owned(),
                ));
            };
            let id = block_on(
                tree.store().write_file(&path, &mut content.as_slice()),
            )?;
            builder.set_or_remove(
                path.clone(),
                Merge::normal(TreeValue::File {
                    id,
                    // The same native executable choice and copy metadata used
                    // to materialize this file in jj's working copy.
                    executable: file.executable.unwrap_or(false),
                    copy_id: file.copy_id.unwrap_or_else(CopyId::placeholder),
                }),
            );
            resolved.push(path.as_internal_file_string().to_owned());
        }
        if !resolved.is_empty() {
            let new_tree = block_on(builder.write_tree())?;
            session::write_commit(tx, |repo| {
                repo.rewrite_commit(wc).set_tree(new_tree.clone())
            })?;
        }
        Ok(resolved)
    })?;
    let (line, info) = wc_line(&snapshot)?;
    let text = if resolved.is_empty() {
        format!("No unresolved conflicts in those paths.\n{line}")
    } else {
        format!(
            "Accepted markerless file contents:\n{}\n{line}",
            resolved.join("\n")
        )
    };
    Ok(Report {
        text,
        details: json!({ "resolved": resolved, "working_copy": info }),
    })
}

pub(crate) fn undo(worker: &mut Worker) -> Result<Report, VcsError> {
    let name = workspace_name(worker)?;
    let (snapshot, undone) = session::mutate(worker, "undo", |tx, wc| {
        let base = tx.base_repo().clone();
        let target = undoable(base.operation(), &name)?;
        let parents = block_on(target.parents())?;
        let [parent] = parents.as_slice() else {
            return Err(VcsError::UndoMerge);
        };
        let bad = block_on(base.loader().load_at(&target))?;
        let good = block_on(base.loader().load_at(parent))?;
        block_on(tx.repo_mut().merge(&bad, &good))?;
        carry_edits(tx, &name, &bad, &good, wc)?;
        tx.set_attribute(UNDO_ATTRIBUTE.to_owned(), target.id().hex());
        Ok(target)
    })?;
    let metadata = undone.metadata();
    let tool = metadata
        .attributes
        .get(TOOL_ATTRIBUTE)
        .cloned()
        .unwrap_or_default();
    let mut id = undone.id().hex();
    id.truncate(session::SHORT_ID);
    let (line, info) = wc_line(&snapshot)?;
    Ok(Report {
        text: format!("Undid operation {id} (vcs_{tool}).\n{line}"),
        details: json!({
            "operation": undone.id().hex(),
            "tool": tool,
            "working_copy": info,
        }),
    })
}

/// Puts `@` back where the undone operation found it, with the file
/// edits made since carried over.
///
/// The merge alone does this when nothing was edited since. When files
/// were, a snapshot rewrote `@` after the operation, so both sides of
/// the merge moved `@`, and jj keeps the current one: the operation
/// would stay done (a `vcs_new` kept, a description kept, the original
/// commit back as a divergent twin). Instead, `@` becomes the commit
/// the operation started from, with each path edited since as it is
/// now, and the edited commit is abandoned.
fn carry_edits(
    tx: &mut Transaction,
    name: &WorkspaceName,
    bad: &ReadonlyRepo,
    good: &ReadonlyRepo,
    current: &Commit,
) -> Result<(), VcsError> {
    let (Some(bad_id), Some(good_id)) = (
        bad.view().get_wc_commit_id(name).cloned(),
        good.view().get_wc_commit_id(name).cloned(),
    ) else {
        return Ok(());
    };
    if current.id() == &bad_id || bad_id == good_id {
        return Ok(());
    }
    let store = tx.repo().store().clone();
    let bad_wc = store.get_commit(&bad_id)?;
    let good_wc = store.get_commit(&good_id)?;
    let edited: Vec<RepoPathBuf> = block_on(async {
        let mut stream = bad_wc
            .tree()
            .diff_stream(&current.tree(), &EverythingMatcher);
        let mut paths = Vec::new();
        while let Some(entry) = stream.next().await {
            paths.push(entry.path);
        }
        paths
    });
    let tree = block_on(restore_tree(
        &current.tree(),
        &good_wc.tree(),
        "edits since".to_owned(),
        "before the operation".to_owned(),
        &FilesMatcher::new(&edited),
    ))?;
    let restored = session::write_commit(tx, |repo| {
        repo.rewrite_commit(&good_wc).set_tree(tree.clone())
    })?;
    tx.repo_mut()
        .set_wc_commit(name.to_owned(), restored.id().clone())?;
    if current.id() != &good_id {
        tx.repo_mut().record_abandoned_commit(current);
    }
    Ok(())
}

/// The `tau.vcs.tool` values of the model's write tools: the
/// operations `vcs_undo` may undo.
const UNDOABLE: [&str; 6] =
    ["describe", "commit", "new", "restore", "resolve", "undo"];

/// The newest operation `vcs_undo` may undo: made by these tools in
/// this workspace, skipping snapshots and operations already undone.
fn undoable(
    head: &Operation,
    name: &WorkspaceName,
) -> Result<Operation, VcsError> {
    let mut undone: HashSet<OperationId> = HashSet::new();
    let mut op = head.clone();
    loop {
        let parents = block_on(op.parents())?;
        let [parent] = parents.as_slice() else {
            if parents.is_empty() {
                return Err(VcsError::NothingToUndo);
            }
            return Err(VcsError::ConcurrentOperations);
        };
        let metadata = op.metadata();
        if undone.remove(op.id())
            || metadata.is_snapshot
            || metadata.attributes.contains_key(BOOKMARK_ATTRIBUTE)
        {
            op = parent.clone();
            continue;
        }
        // Only the model's own tools: what the host writes in this
        // workspace (a turn's checkpoint, a catch-up with trunk, a
        // landing) is the host's, and undoing it would move commits the
        // host links to, or that other runs stand on.
        let ours = metadata.workspace_name.as_deref() == Some(name)
            && metadata
                .attributes
                .get(TOOL_ATTRIBUTE)
                .is_some_and(|tool| UNDOABLE.contains(&tool.as_str()));
        if !ours {
            return Err(VcsError::NotOurs(metadata.description.clone()));
        }
        if let Some(target) = metadata.attributes.get(UNDO_ATTRIBUTE) {
            let target = OperationId::try_from_hex(target)
                .ok_or(VcsError::BadUndoRecord)?;
            undone.insert(target);
            op = parent.clone();
            continue;
        }
        return Ok(op);
    }
}
