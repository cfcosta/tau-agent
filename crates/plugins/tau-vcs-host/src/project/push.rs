//! Pushing a project's work to the remote it came from, with `git`
//! through jj-lib (`docs/reference/vcs.md`, "Pushing", ADR 0023):
//! trunk's own commits as they are, so their commit and change ids stay,
//! and a chat's commits replayed onto the remote's trunk for a pull
//! request.
//!
//! jj-lib's push spawns `git`, the one place tau runs it. The token
//! reaches `git` through its environment only: a credential helper set
//! with `GIT_CONFIG_*` reads it from a variable of the child process.
//! It is never written to a file, and never in a command line, which
//! other users can read.
//!
//! A push follows the person's jj config as `jj git push` does
//! (jj-cli 0.46.0's `commands/git/push.rs`): `git.executable-path` is
//! the `git` it runs, `git.private-commits` names commits it refuses to
//! push, and `git.sign-on-push` signs the person's own commits it pushes.

#![allow(
    clippy::disallowed_methods,
    reason = "runs only inside a job in spawn_blocking (ADR 0028)"
)]

use std::{collections::HashSet, sync::Arc};

use futures_util::StreamExt as _;
use jj_lib::{
    backend::CommitId,
    commit::Commit,
    git::{
        GitProgress,
        GitPushOptions,
        GitRefUpdate,
        GitSidebandLineTerminator,
        GitSubprocessCallback,
        GitSubprocessOptions,
        import_refs,
        push_updates,
    },
    merge::{Diff, Merge},
    merged_tree::MergedTree,
    object_id::ObjectId as _,
    ref_name::{RefName, RemoteName, WorkspaceName},
    repo::{ReadonlyRepo, Repo as _},
    revset::{RevsetExpression, UserRevsetExpression},
    rewrite::CommitRewriter,
    settings::{SignBehavior, UserSettings},
};
use pollster::block_on;

use super::{ProjectRepo, StackChange, commit, import_options, range};
use crate::{
    error::VcsError,
    revsets::{ConfigRevsets, remote_heads},
    session::{SHORT_ID, short_commit},
};

/// The remote a project comes from and pushes to. Its branches are the
/// Git store's remote-tracking branches, `<branch>@origin` to jj; its
/// config holds only its URL, with no fetch refspec, so a push never
/// writes remote-tracking refs on its own: a pull request's branch
/// stays out until a fetch brings it.
pub const REMOTE: &str = "origin";

/// What `git` runs as the credential helper: it answers a request for
/// credentials with the token in `TAU_GIT_TOKEN`, and ignores `store`
/// and `erase`.
const HELPER: &str = "!f() { if [ \"$1\" = get ]; then echo \
                      username=x-access-token; echo \
                      \"password=$TAU_GIT_TOKEN\"; fi; }; f";

/// Where to push: the remote's URL, and the token that signs in to it
/// over HTTPS (GitHub reads it as the password of any user).
#[derive(Debug, Clone, Copy)]
pub struct Remote<'a> {
    pub url: &'a str,
    pub token: Option<&'a str>,
}

/// What [`ProjectRepo::push_trunk`] pushed: trunk's branch, the remote's
/// commit before and after (full hex ids), and the changes it took,
/// oldest first. Nothing was pushed when `changes` is empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pushed {
    pub branch: String,
    pub from: Option<String>,
    pub to: String,
    pub changes: Vec<StackChange>,
}

impl ProjectRepo {
    /// The remote's trunk as the last fetch or push left it: what
    /// `<trunk>@origin` names, a full commit id in hex. `None` when the
    /// remote has no such branch.
    pub fn upstream(&self) -> Result<Option<String>, VcsError> {
        let repo = self.load()?;
        let name = self.trunk_name()?;
        Ok(upstream(&repo, &name).map(|id| id.hex()))
    }

    /// Trunk's commits the remote does not have, oldest first: what
    /// trunk has that the remote's trunk, as the last fetch or push
    /// left it, lacks. The main chat's work waiting to be pushed.
    pub fn unpushed(&self) -> Result<Vec<StackChange>, VcsError> {
        let repo = self.load()?;
        let Some((name, local)) = self.trunk_bookmark(&repo) else {
            return Ok(Vec::new());
        };
        range(&repo, &local, upstream(&repo, &name).as_ref())
    }

    /// Pushes trunk to the remote's branch of the same name, as a
    /// fast-forward: the commits themselves, so they keep their commit
    /// and change ids. Refuses when one of them holds a conflict, and
    /// with [`VcsError::PushRejected`] when the remote's branch moved
    /// since the last fetch: fetching puts trunk's commits on top of the
    /// remote's, and the push can go again. Once pushed, the Git store's
    /// branch names the pushed commit, so nothing is ahead any more.
    pub fn push_trunk(&self, remote: Remote<'_>) -> Result<Pushed, VcsError> {
        let lock = self.lock()?;
        let repo = self.load()?;
        let (name, local) = self
            .trunk_bookmark(&repo)
            .ok_or_else(|| VcsError::NoChange(self.trunk_name_or_main()))?;
        let before = upstream(&repo, &name);
        let changes = range(&repo, &local, before.as_ref())?;
        let pushed = Pushed {
            branch: name.clone(),
            from: before.as_ref().map(|id| id.hex()),
            to: local.hex(),
            changes,
        };
        if pushed.changes.is_empty() {
            return Ok(pushed);
        }
        let mut conflicts: Vec<String> = Vec::new();
        for change in pushed.changes.iter().filter(|change| change.conflict) {
            let tree = commit(&repo, &change.commit_id)?.tree();
            for (path, _) in tree.conflicts() {
                let path = path.as_internal_file_string().to_owned();
                if !conflicts.contains(&path) {
                    conflicts.push(path);
                }
            }
        }
        if !conflicts.is_empty() {
            return Err(VcsError::ConflictedTrunk(conflicts.join(", ")));
        }
        let revsets = self.revsets()?;
        let pushing = RevsetExpression::commits(vec![local.clone()])
            .ancestors()
            .minus(&known(&repo, &revsets)?.ancestors());
        self.refuse_private(&repo, &revsets, &pushing)?;
        let repo = self.sign_before_push(&lock, repo, &revsets, &local)?;
        // Signing rewrote the commits it signed: trunk names the new
        // ones.
        let (_, local) = self
            .trunk_bookmark(&repo)
            .ok_or_else(|| VcsError::NoChange(self.trunk_name_or_main()))?;
        let pushed = Pushed {
            to: local.hex(),
            changes: range(&repo, &local, before.as_ref())?,
            ..pushed
        };
        self.push_ref(&repo, remote, &name, before.as_ref(), &local)?;
        // The remote has trunk now: so does the Git store's
        // remote-tracking branch, as a fetch would leave it, and jj's
        // record of it.
        let git = lock.git()?;
        let store = self.git()?;
        store
            .reference(
                format!("{}{name}", crate::clone::TRACKING),
                gix::ObjectId::from_bytes_or_panic(local.as_bytes()),
                gix::refs::transaction::PreviousValue::Any,
                format!("tau: push {name}"),
            )
            .map_err(|error| VcsError::PushRemote(error.to_string()))?;
        let mut tx = repo.start_transaction();
        block_on(import_refs(tx.repo_mut(), &import_options(false)))
            .map_err(VcsError::ImportBranches)?;
        crate::colocate::commit(tx, format!("push {name}"), &git)?;
        Ok(pushed)
    }

    /// Copies of `commits` (full hex ids, oldest first, each on the one
    /// before) replayed onto `onto`: each copy's files are the last
    /// copy's (`onto`'s for the first) with what its commit changed from
    /// its parent applied, by jj's three-way tree merge. A copy keeps its
    /// commit's description and author, and gets a change id of its own,
    /// so the copies never make the originals divergent once they come
    /// back in a fetch. They are written to the store only, in a
    /// transaction that is dropped: no operation records them, so they
    /// stay hidden, and `git` can push them. Returns their ids, oldest
    /// first; refuses with [`VcsError::WouldConflict`] and the paths of
    /// the first copy that would hold a conflict.
    pub fn replay(
        &self,
        commits: &[String],
        onto: &str,
    ) -> Result<Vec<String>, VcsError> {
        let repo = self.load()?;
        // The copies are what a push sends: one of a commit
        // `git.private-commits` names would be refused, so the commit is
        // refused here, before anything is made or sent.
        let originals = commits
            .iter()
            .map(|hex| Ok(commit(&repo, hex)?.id().clone()))
            .collect::<Result<Vec<_>, VcsError>>()?;
        let revsets = self.revsets()?;
        let sending = RevsetExpression::commits(originals)
            .minus(&known(&repo, &revsets)?.ancestors());
        self.refuse_private(&repo, &revsets, &sending)?;
        let mut tx = repo.start_transaction();
        let mut parent = commit(&repo, onto)?;
        let mut copies = Vec::new();
        for hex in commits {
            let original = commit(&repo, hex)?;
            let base = block_on(original.parent_tree(repo.as_ref()))?;
            let tree = block_on(MergedTree::merge(Merge::from_vec(vec![
                (parent.tree(), "the remote's trunk".to_owned()),
                (base, "the commit's parent".to_owned()),
                (original.tree(), "the commit".to_owned()),
            ])))?;
            if tree.has_conflict() {
                return Err(VcsError::WouldConflict(
                    tree.conflicts()
                        .map(|(path, _)| {
                            path.as_internal_file_string().to_owned()
                        })
                        .collect(),
                ));
            }
            parent = block_on(
                tx.repo_mut()
                    .new_commit(vec![parent.id().clone()], tree)
                    .set_description(original.description())
                    .set_author(original.author().clone())
                    .write(),
            )?;
            copies.push(parent.id().hex());
        }
        // Dropped, not committed: the copies stay out of every view.
        drop(tx);
        Ok(copies)
    }

    /// Points the remote's branch `branch` at `head` (a full commit id
    /// in hex) if the remote has it at `expected` (absent when `None`),
    /// a pull request's branch. Nothing of the project changes: the
    /// branch is not a bookmark here. With `git.sign-on-push`, the
    /// copies [`ProjectRepo::replay`] made that the person wrote are
    /// signed first, as copies again: returns the head pushed, `head`
    /// when nothing needed signing.
    pub fn push_branch(
        &self,
        remote: Remote<'_>,
        branch: &str,
        expected: Option<&str>,
        head: &str,
    ) -> Result<String, VcsError> {
        let _repo = self.lock()?;
        let repo = self.load()?;
        let id = |hex: &str| {
            CommitId::try_from_hex(hex)
                .ok_or_else(|| VcsError::NotCommitId(hex.to_owned()))
        };
        let expected = expected.map(id).transpose()?;
        let head = self.sign_copies(&repo, expected.as_ref(), &id(head)?)?;
        self.push_ref(&repo, remote, branch, expected.as_ref(), &head)?;
        Ok(head.hex())
    }

    /// The config's revsets, as `jj git push` in the main workspace
    /// reads them.
    fn revsets(&self) -> Result<ConfigRevsets, VcsError> {
        ConfigRevsets::new(
            &self.inner.settings,
            self.main_dir(),
            WorkspaceName::new(self.main_workspace()),
        )
    }

    /// Refuses a push that would send `sending` when one of those
    /// commits is one `git.private-commits` names (jj-cli's
    /// `CommitsValidator`). The error names the first, newest first, as
    /// jj does.
    fn refuse_private(
        &self,
        repo: &Arc<ReadonlyRepo>,
        revsets: &ConfigRevsets,
        sending: &Arc<UserRevsetExpression>,
    ) -> Result<(), VcsError> {
        let name = "git.private-commits";
        let (text, private) =
            revsets.setting(repo.as_ref(), &self.inner.settings, name)?;
        // Evaluated whole, as `jj git push` evaluates it: an
        // `at_operation()` in it sees that operation's commits.
        let private = revsets.evaluate(repo.as_ref(), name, &private)?;
        let is_private = private.containing_fn();
        for id in &evaluate(repo.as_ref(), revsets, sending)? {
            if block_on(is_private(id))? {
                let commit = repo.store().get_commit(id)?;
                return Err(VcsError::PrivateCommit {
                    commit: commit_name(&commit),
                    revset: text,
                });
            }
        }
        Ok(())
    }

    /// With `git.sign-on-push`, signs the commits a push of `head` sends
    /// (what the remote's bookmarks do not reach) that are mutable,
    /// unsigned, and the person's own, rebasing what stands on them, in
    /// one operation (jj-cli's `sign_commits_before_push`). Returns the
    /// repository after.
    fn sign_before_push(
        &self,
        lock: &crate::lock::RepoLock,
        repo: Arc<ReadonlyRepo>,
        revsets: &ConfigRevsets,
        head: &CommitId,
    ) -> Result<Arc<ReadonlyRepo>, VcsError> {
        let settings = &self.inner.settings;
        if !settings.get_bool("git.sign-on-push")? {
            return Ok(repo);
        }
        let to_push = RevsetExpression::commits(remote_heads(
            repo.as_ref(),
            RemoteName::new(REMOTE),
        ))
        .range(&RevsetExpression::commits(vec![head.clone()]))
        .minus(&revsets.immutable_heads(repo.as_ref())?.ancestors());
        // Evaluated whole, as jj-cli's `sign_commits_before_push` does:
        // an `at_operation()` in `immutable_heads()` sees that
        // operation's commits.
        let to_push = revsets.evaluate(
            repo.as_ref(),
            "revset-aliases.immutable_heads()",
            &to_push,
        )?;
        let to_push: Vec<CommitId> =
            block_on(to_push.stream().collect::<Vec<_>>())
                .into_iter()
                .collect::<Result<_, _>>()?;
        let mut unsigned = HashSet::new();
        for id in to_push {
            if needs_signing(settings, &repo.store().get_commit(&id)?) {
                unsigned.insert(id);
            }
        }
        if unsigned.is_empty() {
            return Ok(repo);
        }
        let mut tx = repo.start_transaction();
        block_on(tx.repo_mut().transform_descendants(
            unsigned.iter().cloned().collect(),
            async |rewriter: CommitRewriter<'_>| {
                let sign = unsigned.contains(rewriter.old_commit().id());
                let builder = rewriter.reparent();
                let builder = if sign {
                    builder.set_sign_behavior(SignBehavior::Own)
                } else {
                    builder
                };
                builder.write().await?;
                Ok(())
            },
        ))?;
        block_on(tx.repo_mut().rebase_descendants())?;
        crate::colocate::commit(tx, "sign commits before pushing", &lock.git()?)
    }

    /// With `git.sign-on-push`, signs the copies under `head` that
    /// [`ProjectRepo::replay`] made, down to `expected` or the first
    /// commit jj records, where the person wrote them and they are
    /// unsigned: each a copy again, on the one before, in a transaction
    /// that is dropped, as `replay` writes them. Returns the new head.
    fn sign_copies(
        &self,
        repo: &Arc<ReadonlyRepo>,
        expected: Option<&CommitId>,
        head: &CommitId,
    ) -> Result<CommitId, VcsError> {
        let settings = &self.inner.settings;
        if !settings.get_bool("git.sign-on-push")? {
            return Ok(head.clone());
        }
        let mut copies = Vec::new();
        let mut at = repo.store().get_commit(head)?;
        while Some(at.id()) != expected
            && !block_on(repo.index().has_id(at.id()))?
        {
            let Some(parent) = at.parent_ids().first().cloned() else {
                break;
            };
            copies.push(at);
            at = repo.store().get_commit(&parent)?;
        }
        let mut tx = repo.start_transaction();
        let mut parent = at.id().clone();
        let mut moved = false;
        for copy in copies.iter().rev() {
            let sign = needs_signing(settings, copy);
            if !sign && !moved {
                parent = copy.id().clone();
                continue;
            }
            let builder = tx
                .repo_mut()
                .rewrite_commit(copy)
                .set_parents(vec![parent.clone()]);
            let builder = if sign {
                builder.set_sign_behavior(SignBehavior::Own)
            } else {
                builder
            };
            parent = block_on(builder.write())?.id().clone();
            moved = true;
        }
        // Dropped, not committed, as `replay`'s: the copies stay hidden.
        drop(tx);
        Ok(parent)
    }

    /// Pushes `after` to the remote's `refs/heads/<branch>`, if the
    /// remote has it at `before`. The caller holds the lock: the remote's
    /// URL is written to the Git store's config.
    fn push_ref(
        &self,
        repo: &ReadonlyRepo,
        remote: Remote<'_>,
        branch: &str,
        before: Option<&CommitId>,
        after: &CommitId,
    ) -> Result<(), VcsError> {
        crate::clone::set_remote(&self.git_dir(), remote.url)
            .map_err(|error| VcsError::PushRemote(error.to_string()))?;
        let oid =
            |id: &CommitId| gix::ObjectId::from_bytes_or_panic(id.as_bytes());
        let qualified = format!("refs/heads/{branch}");
        let update = GitRefUpdate {
            qualified_name: qualified.clone().into(),
            targets: Diff::new(before.map(oid), Some(oid(after))),
        };
        let stats = push_updates(
            repo,
            git_options(&self.inner.settings, remote.token)?,
            RemoteName::new(REMOTE),
            &[update],
            &mut Quiet,
            &GitPushOptions::default(),
        )
        .map_err(VcsError::Push)?;
        let branch = branch.to_owned();
        if !stats.rejected.is_empty() {
            return Err(VcsError::PushRejected { branch });
        }
        if let Some((_, reason)) = stats.remote_rejected.first() {
            let reason =
                reason.clone().unwrap_or_else(|| "no reason given".into());
            return Err(VcsError::PushRefused { branch, reason });
        }
        if !stats.pushed.iter().any(|name| name.as_str() == qualified) {
            return Err(VcsError::PushRefused {
                branch,
                reason: "git did not push it".into(),
            });
        }
        Ok(())
    }

    /// Trunk's name, or `main` when it cannot be read: for errors.
    fn trunk_name_or_main(&self) -> String {
        self.trunk_name().unwrap_or_else(|_| "main".into())
    }
}

/// The remote's `branch` as the Git store has it, as the last fetch or
/// push left it.
fn upstream(repo: &ReadonlyRepo, branch: &str) -> Option<CommitId> {
    repo.view()
        .get_remote_bookmark(
            RefName::new(branch).to_remote_symbol(RemoteName::new(REMOTE)),
        )
        .target
        .as_normal()
        .cloned()
}

/// What a push need not send: the remote's bookmarks, as the last fetch
/// or push left them, and `immutable_heads()`, with their ancestors.
fn known(
    repo: &ReadonlyRepo,
    revsets: &ConfigRevsets,
) -> Result<Arc<UserRevsetExpression>, VcsError> {
    Ok(
        RevsetExpression::commits(remote_heads(repo, RemoteName::new(REMOTE)))
            .union(&revsets.immutable_heads(repo)?),
    )
}

/// The commits `expression` names in `repo`, newest first, its
/// `immutable_heads()` resolved as jj-cli's `CommitsValidator` resolves
/// it: without the repositories an `at_operation()` brings in.
fn evaluate(
    repo: &ReadonlyRepo,
    revsets: &ConfigRevsets,
    expression: &Arc<UserRevsetExpression>,
) -> Result<Vec<CommitId>, VcsError> {
    let revset = revsets
        .resolve(repo, "revset-aliases.immutable_heads()", expression)?
        .evaluate(repo)?;
    Ok(block_on(revset.stream().collect::<Vec<_>>())
        .into_iter()
        .collect::<Result<_, _>>()?)
}

/// Whether `commit` is one jj signs before pushing: unsigned, and the
/// person's own (`signing.behavior = "own"`).
fn needs_signing(settings: &UserSettings, commit: &Commit) -> bool {
    let mut sign = settings.sign_settings();
    sign.behavior = SignBehavior::Own;
    !commit.is_signed() && sign.should_sign(commit.store_commit())
}

/// A commit as an error names it: its change id, its commit id, and the
/// first line of its description.
fn commit_name(commit: &Commit) -> String {
    let mut change = commit.change_id().reverse_hex();
    change.truncate(SHORT_ID);
    let line = commit.description().lines().next().unwrap_or_default();
    let line = if line.is_empty() {
        "(no description set)"
    } else {
        line
    };
    format!("{change} {} \"{line}\"", short_commit(commit.id()))
}

/// How jj-lib runs `git push`: the `git` the config names
/// (`git.executable-path`), never asking on a terminal, and with the
/// token, if any, offered by a credential helper that reads it from the
/// child's environment. The empty helper before it drops the person's
/// own helpers, so none of them stores the token.
fn git_options(
    settings: &UserSettings,
    token: Option<&str>,
) -> Result<GitSubprocessOptions, VcsError> {
    let mut options = GitSubprocessOptions::from_settings(settings)?;
    let environment = &mut options.environment;
    environment.insert("GIT_TERMINAL_PROMPT".into(), "0".into());
    if let Some(token) = token {
        for (key, value) in [
            ("GIT_CONFIG_COUNT", "2"),
            ("GIT_CONFIG_KEY_0", "credential.helper"),
            ("GIT_CONFIG_VALUE_0", ""),
            ("GIT_CONFIG_KEY_1", "credential.helper"),
            ("GIT_CONFIG_VALUE_1", HELPER),
            ("TAU_GIT_TOKEN", token),
        ] {
            environment.insert(key.into(), value.into());
        }
    }
    Ok(options)
}

/// Takes `git`'s progress and messages and shows none: the push's
/// outcome is what the person sees.
struct Quiet;

impl GitSubprocessCallback for Quiet {
    fn needs_progress(&self) -> bool {
        false
    }

    fn progress(&mut self, _: &GitProgress) -> std::io::Result<()> {
        Ok(())
    }

    fn local_sideband(
        &mut self,
        _: &[u8],
        _: Option<GitSidebandLineTerminator>,
    ) -> std::io::Result<()> {
        Ok(())
    }

    fn remote_sideband(
        &mut self,
        _: &[u8],
        _: Option<GitSidebandLineTerminator>,
    ) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// jj's defaults alone.
    fn settings() -> UserSettings {
        UserSettings::from_config_and_home_dir(
            jj_lib::config::StackedConfig::with_defaults(),
            None,
        )
        .unwrap()
    }

    /// `git credential` with `options`' environment and a global
    /// config that stores credentials in `home/creds`, as a person's
    /// `credential.helper = store` would; `input` on its stdin.
    fn credential(
        home: &std::path::Path,
        options: &GitSubprocessOptions,
        action: &str,
        input: &str,
    ) -> String {
        use std::io::Write as _;
        let config = home.join("gitconfig");
        let store = format!("store --file {}", home.join("creds").display());
        std::fs::write(&config, format!("[credential]\n\thelper = {store}\n"))
            .unwrap();
        let mut child = std::process::Command::new("git")
            .args(["credential", action])
            .env("GIT_CONFIG_GLOBAL", &config)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", home)
            .envs(&options.environment)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("git runs");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap()
    }

    /// The helper `git` runs answers with the token from the child's
    /// environment, and the person's own helpers are dropped, so none
    /// stores it once a push signs in.
    #[test]
    fn the_credential_helper_answers_with_the_token_and_stores_nothing() {
        let home = tempfile::tempdir().unwrap();
        let options = git_options(&settings(), Some("ghs_secret")).unwrap();
        let asked = "protocol=https\nhost=github.com\n\n";
        let filled = credential(home.path(), &options, "fill", asked);
        assert!(filled.contains("username=x-access-token\n"), "{filled}");
        assert!(filled.contains("password=ghs_secret\n"), "{filled}");
        credential(home.path(), &options, "approve", &format!("{filled}\n"));
        assert!(!home.path().join("creds").exists(), "the token was stored");
        // The same approval without tau's environment is stored: the
        // check above would see it.
        let plain = git_options(&settings(), None).unwrap();
        credential(home.path(), &plain, "approve", &format!("{filled}\n"));
        assert!(home.path().join("creds").exists());
        // The token is in the child's environment alone, in no setting.
        assert!(!options.environment.iter().any(|(key, value)| {
            key != "TAU_GIT_TOKEN"
                && value.to_string_lossy().contains("ghs_secret")
        }));
    }

    /// Without a token, nothing about credentials is set.
    #[test]
    fn no_token_sets_no_helper() {
        let options = git_options(&settings(), None).unwrap();
        assert_eq!(options.environment.len(), 1);
        assert_eq!(
            options.environment[std::ffi::OsStr::new("GIT_TERMINAL_PROMPT")],
            *"0"
        );
    }
}
