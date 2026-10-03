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

use std::{collections::HashMap, ffi::OsString};

use jj_lib::{
    backend::CommitId,
    git::{
        GitImportOptions,
        GitProgress,
        GitPushOptions,
        GitRefUpdate,
        GitSidebandLineTerminator,
        GitSubprocessCallback,
        GitSubprocessOptions,
        REMOTE_NAME_FOR_LOCAL_GIT_REPO,
        import_refs,
        push_updates,
    },
    merge::{Diff, Merge},
    merged_tree::MergedTree,
    object_id::ObjectId as _,
    ref_name::{RefName, RemoteName},
    repo::ReadonlyRepo,
};
use pollster::block_on;

use super::{GIT, Project, StackChange, commit, range};
use crate::error::VcsError;

/// The remote tau pushes to, in the Git store's config: only its URL,
/// with no fetch refspec, so a push never writes remote-tracking refs
/// that jj would import as `<branch>@origin`, which would make the
/// pushed commits immutable.
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

/// What [`Project::push_trunk`] pushed: trunk's branch, the remote's
/// commit before and after (full hex ids), and the changes it took,
/// oldest first. Nothing was pushed when `changes` is empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pushed {
    pub branch: String,
    pub from: Option<String>,
    pub to: String,
    pub changes: Vec<StackChange>,
}

impl Project {
    /// The remote's trunk as the last fetch or push left it: what
    /// `<trunk>@git` names, a full commit id in hex. `None` when the
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
        let _repo = self.lock()?;
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
        self.push_ref(&repo, remote, &name, before.as_ref(), &local)?;
        // The remote has trunk now: so does the Git store's branch, as a
        // fetch would leave it, and jj's record of it.
        let store = self.git()?;
        store
            .reference(
                format!("refs/heads/{name}"),
                gix::ObjectId::from_bytes_or_panic(local.as_bytes()),
                gix::refs::transaction::PreviousValue::Any,
                format!("tau: push {name}"),
            )
            .map_err(|error| VcsError::PushRemote(error.to_string()))?;
        let mut tx = repo.start_transaction();
        let options = GitImportOptions {
            abandon_unreachable_commits: false,
            record_synthetic_predecessors: false,
            remote_auto_track_bookmarks: HashMap::new(),
        };
        block_on(import_refs(tx.repo_mut(), &options))
            .map_err(VcsError::ImportBranches)?;
        block_on(tx.commit(format!("tau: push {name}")))?;
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
    /// branch is not a bookmark here.
    pub fn push_branch(
        &self,
        remote: Remote<'_>,
        branch: &str,
        expected: Option<&str>,
        head: &str,
    ) -> Result<(), VcsError> {
        let _repo = self.lock()?;
        let repo = self.load()?;
        let id = |hex: &str| {
            CommitId::try_from_hex(hex)
                .ok_or_else(|| VcsError::NotCommitId(hex.to_owned()))
        };
        let expected = expected.map(id).transpose()?;
        self.push_ref(&repo, remote, branch, expected.as_ref(), &id(head)?)
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
        set_remote(&self.inner.root.join(GIT), remote.url)?;
        let oid =
            |id: &CommitId| gix::ObjectId::from_bytes_or_panic(id.as_bytes());
        let qualified = format!("refs/heads/{branch}");
        let update = GitRefUpdate {
            qualified_name: qualified.clone().into(),
            targets: Diff::new(before.map(oid), Some(oid(after))),
        };
        let stats = push_updates(
            repo,
            git_options(remote.token),
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
            RefName::new(branch)
                .to_remote_symbol(REMOTE_NAME_FOR_LOCAL_GIT_REPO),
        )
        .target
        .as_normal()
        .cloned()
}

/// Makes [`REMOTE`] in the Git store at `git_dir` point at `url`, with
/// nothing else: a section left from a clone, with its refspecs, goes.
fn set_remote(git_dir: &std::path::Path, url: &str) -> Result<(), VcsError> {
    let failed =
        |error: &dyn std::fmt::Display| VcsError::PushRemote(error.to_string());
    let repo = gix::open(git_dir).map_err(|error| failed(&error))?;
    let mut config = repo.config_snapshot().clone();
    while config
        .remove_section("remote", Some(REMOTE.into()))
        .is_some()
    {}
    config
        .new_section("remote", Some(REMOTE.into()))
        .map_err(|error| failed(&error))?
        .push("url", Some(url.into()))
        .map_err(|error| failed(&error))?;
    jj_lib::git::save_git_config(&config).map_err(|error| failed(&error))
}

/// How jj-lib runs `git push`: never asking on a terminal, and with the
/// token, if any, offered by a credential helper that reads it from the
/// child's environment. The empty helper before it drops the person's
/// own helpers, so none of them stores the token.
fn git_options(token: Option<&str>) -> GitSubprocessOptions {
    let mut environment: HashMap<OsString, OsString> = HashMap::new();
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
    GitSubprocessOptions {
        executable_path: "git".into(),
        environment,
    }
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
        let options = git_options(Some("ghs_secret"));
        let asked = "protocol=https\nhost=github.com\n\n";
        let filled = credential(home.path(), &options, "fill", asked);
        assert!(filled.contains("username=x-access-token\n"), "{filled}");
        assert!(filled.contains("password=ghs_secret\n"), "{filled}");
        credential(home.path(), &options, "approve", &format!("{filled}\n"));
        assert!(!home.path().join("creds").exists(), "the token was stored");
        // The same approval without tau's environment is stored: the
        // check above would see it.
        let plain = git_options(None);
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
        let options = git_options(None);
        assert_eq!(options.environment.len(), 1);
        assert_eq!(
            options.environment[std::ffi::OsStr::new("GIT_TERMINAL_PROMPT")],
            *"0"
        );
    }
}
