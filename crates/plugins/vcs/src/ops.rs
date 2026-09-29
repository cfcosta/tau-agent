//! What each tool does, on the workspace's thread
//! (`docs/reference/vcs.md`, "Tools"). Each returns the text the model
//! sees and the `details` value callers get.

use std::collections::HashSet;

use anyhow::{anyhow, bail};
use futures_util::StreamExt as _;
use jj_lib::{
    backend::CommitId,
    commit::Commit,
    matchers::{EverythingMatcher, FilesMatcher},
    object_id::ObjectId as _,
    op_store::OperationId,
    operation::Operation,
    ref_name::WorkspaceName,
    repo::{ReadonlyRepo, Repo},
    repo_path::RepoPathBuf,
    revset::ResolvedRevsetExpression,
    rewrite::restore_tree,
    transaction::Transaction,
};
use pollster::block_on;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    diff::{self, FileChange},
    session::{
        self,
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

/// One change, as the tools describe it in `details`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeInfo {
    /// The full change id, in jj's `k`-`z` letters.
    pub change_id: String,
    /// The full commit id, in hex.
    pub commit_id: String,
    pub description: String,
    /// The change touches no files.
    pub empty: bool,
    pub conflict: bool,
    pub immutable: bool,
    /// The change is this workspace's working copy (`@`).
    pub working_copy: bool,
    /// More than one visible commit has this change id, so the change id
    /// names none of them: pass a commit id.
    pub divergent: bool,
    /// The local bookmarks on this commit, such as `main`, sorted.
    pub bookmarks: Vec<String>,
}

impl ChangeInfo {
    fn of(
        repo: &dyn Repo,
        commit: &Commit,
        wc: &CommitId,
    ) -> anyhow::Result<Self> {
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
) -> anyhow::Result<Vec<RepoPathBuf>> {
    let root = worker.root().to_owned();
    let workspace = worker.workspace()?;
    paths
        .iter()
        .map(|path| session::repo_path(workspace, &root, path))
        .collect()
}

fn workspace_name(
    worker: &mut Worker,
) -> anyhow::Result<jj_lib::ref_name::WorkspaceNameBuf> {
    Ok(worker.workspace()?.workspace_name().to_owned())
}

fn wc_line(snapshot: &Snapshot) -> anyhow::Result<(String, ChangeInfo)> {
    let info =
        ChangeInfo::of(snapshot.repo.as_ref(), &snapshot.wc, snapshot.wc.id())?;
    Ok((format!("Working copy (@): {}", info.line()), info))
}

pub(crate) fn status(worker: &mut Worker) -> anyhow::Result<Report> {
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
            "\nUnresolved conflicts (edit the markers out of these files):",
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

pub(crate) fn diff(
    worker: &mut Worker,
    change: Option<String>,
    paths: Vec<String>,
) -> anyhow::Result<Report> {
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

pub(crate) fn log(worker: &mut Worker, limit: u32) -> anyhow::Result<Report> {
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
) -> anyhow::Result<Report> {
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
) -> anyhow::Result<Report> {
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

pub(crate) fn commit(
    worker: &mut Worker,
    message: String,
) -> anyhow::Result<Report> {
    if message.trim().is_empty() {
        bail!("The description must not be empty");
    }
    let name = workspace_name(worker)?;
    let (snapshot, committed) = session::mutate(worker, "commit", |tx, wc| {
        let committed = session::write_commit(tx, |repo| {
            repo.rewrite_commit(wc)
                .set_description(description(&message))
        })?;
        block_on(tx.repo_mut().rebase_descendants())?;
        block_on(tx.repo_mut().check_out(name, &committed))?;
        Ok(committed)
    })?;
    let repo = snapshot.repo.as_ref();
    let committed = ChangeInfo::of(repo, &committed, snapshot.wc.id())?;
    let (line, info) = wc_line(&snapshot)?;
    Ok(Report {
        text: format!("Committed change {}\n{line}", committed.line()),
        details: json!({ "committed": committed, "working_copy": info }),
    })
}

pub(crate) fn new(
    worker: &mut Worker,
    message: Option<String>,
) -> anyhow::Result<Report> {
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

/// The `tau.vcs.tool` value of a turn's checkpoint. The host makes it,
/// not the model's tools, so `vcs_undo` treats it as someone else's.
pub(crate) const CHECKPOINT: &str = "checkpoint";

/// Where a turn left the code: the commit holding its files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct TurnCommit {
    pub commit_id: String,
    pub change_id: String,
    /// The turn changed files, so a new commit was made for it.
    pub changed: bool,
    /// The paths the turn added, changed or removed.
    pub paths: Vec<String>,
}

/// Ends a turn: snapshots the working copy and, if it changed anything,
/// commits it (described as `message` unless the model described it)
/// and starts an empty working copy on top. Returns the commit that
/// holds the turn's files: the new one, or the working copy's parent
/// when the turn changed nothing.
pub(crate) fn checkpoint(
    worker: &mut Worker,
    message: String,
) -> anyhow::Result<TurnCommit> {
    let name = workspace_name(worker)?;
    let (_, turn) = session::mutate(worker, CHECKPOINT, |tx, wc| {
        if block_on(wc.is_empty(tx.repo()))? {
            let parent = wc
                .parent_ids()
                .first()
                .ok_or_else(|| anyhow!("The working copy has no parent"))?;
            let parent = tx.repo().store().get_commit(parent)?;
            return Ok(TurnCommit {
                commit_id: parent.id().hex(),
                change_id: parent.change_id().reverse_hex(),
                changed: false,
                paths: Vec::new(),
            });
        }
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
        block_on(tx.repo_mut().check_out(name, &committed))?;
        Ok(TurnCommit {
            commit_id: committed.id().hex(),
            change_id: committed.change_id().reverse_hex(),
            changed: true,
            paths,
        })
    })?;
    Ok(turn)
}

pub(crate) fn restore(
    worker: &mut Worker,
    paths: Vec<String>,
    from: Option<String>,
) -> anyhow::Result<Report> {
    if paths.is_empty() {
        bail!("Name at least one path to restore (\".\" restores everything)");
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

pub(crate) fn undo(worker: &mut Worker) -> anyhow::Result<Report> {
    let name = workspace_name(worker)?;
    let (snapshot, undone) = session::mutate(worker, "undo", |tx, wc| {
        let base = tx.base_repo().clone();
        let target = undoable(base.operation(), &name)?;
        let parents = block_on(target.parents())?;
        let [parent] = parents.as_slice() else {
            bail!("The operation to undo is a merge; ask the user to undo it")
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
) -> anyhow::Result<()> {
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

/// The newest operation `vcs_undo` may undo: made by these tools in
/// this workspace, skipping snapshots and operations already undone.
fn undoable(
    head: &Operation,
    name: &WorkspaceName,
) -> anyhow::Result<Operation> {
    let mut undone: HashSet<OperationId> = HashSet::new();
    let mut op = head.clone();
    loop {
        let parents = block_on(op.parents())?;
        let [parent] = parents.as_slice() else {
            if parents.is_empty() {
                bail!("There is nothing to undo");
            }
            bail!(
                "The operation log has concurrent operations here; ask the \
                 user to undo from the operation log"
            );
        };
        let metadata = op.metadata();
        if undone.remove(op.id()) || metadata.is_snapshot {
            op = parent.clone();
            continue;
        }
        // A turn's checkpoint is the host's: undoing it would hide the
        // commit its turn links to.
        let ours = metadata.workspace_name.as_deref() == Some(name)
            && metadata
                .attributes
                .get(TOOL_ATTRIBUTE)
                .is_some_and(|tool| tool != CHECKPOINT);
        if !ours {
            bail!(
                "The last operation was not made by the vcs tools in this \
                 workspace (\"{}\"); vcs_undo only undoes its own operations",
                metadata.description
            );
        }
        if let Some(target) = metadata.attributes.get(UNDO_ATTRIBUTE) {
            let target = OperationId::try_from_hex(target)
                .ok_or_else(|| anyhow!("Bad undo record on operation"))?;
            undone.insert(target);
            op = parent.clone();
            continue;
        }
        return Ok(op);
    }
}
