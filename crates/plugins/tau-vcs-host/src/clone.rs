//! Fetching from a remote without the git binary: gix speaks the smart
//! HTTP protocol itself, over reqwest and rustls. A project's Git store
//! keeps upstream's branches as `git clone` does, under
//! `refs/remotes/origin/`, and its tags as tags; jj imports them as
//! `<branch>@origin`.

use std::{collections::HashSet, io, path::Path, sync::atomic::AtomicBool};

use gix::{
    bstr::{BStr, BString, ByteSlice as _},
    credentials::{
        helper::Action,
        protocol::{Context, Outcome},
    },
    refs::{
        FullName,
        Target,
        transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog},
    },
    sec::identity::Account,
};

use crate::project::REMOTE;

/// Why a clone or a fetch failed, and from where.
#[derive(Debug, thiserror::Error)]
pub enum CloneError {
    #[error("Cannot clone {url}: {source}")]
    Clone {
        url: String,
        source: Box<TransferError>,
    },
    #[error("Cannot open {}: {source}", path.display())]
    Open {
        path: std::path::PathBuf,
        source: Box<gix::open::Error>,
    },
    #[error("Cannot fetch from {url}: {source}")]
    Fetch {
        url: String,
        source: Box<TransferError>,
    },
}

/// The step of a clone or a fetch that failed, as gix reports it.
#[derive(Debug, thiserror::Error)]
pub enum TransferError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Remote(#[from] gix::remote::init::Error),
    #[error(transparent)]
    Refspec(#[from] gix::refspec::parse::Error),
    #[error(transparent)]
    Connect(#[from] gix::remote::connect::Error),
    #[error(transparent)]
    PrepareFetch(#[from] gix::remote::fetch::prepare::Error),
    #[error(transparent)]
    Receive(#[from] gix::remote::fetch::Error),
    /// Making the remote-tracking branches, tags and `origin/HEAD` the
    /// remote's.
    #[error("Cannot mirror the remote's refs: {0}")]
    Mirror(Box<dyn std::error::Error + Send + Sync>),
}

/// Where upstream's branches live in the Git store.
pub(crate) const TRACKING: &str = "refs/remotes/origin/";

/// Upstream's default branch, as `git clone` records it.
pub(crate) const ORIGIN_HEAD: &str = "refs/remotes/origin/HEAD";

/// The refs a fetch writes: every branch of the remote as a
/// remote-tracking branch, and every tag under its own name.
const REFSPECS: [&str; 2] = [
    "+refs/heads/*:refs/remotes/origin/*",
    "+refs/tags/*:refs/tags/*",
];

/// Fetches every branch and tag of `url` into the Git store at
/// `git_dir`, and makes its remote-tracking branches, tags and
/// `origin/HEAD` the remote's: they move where the remote has them, and
/// go when the remote has deleted them. A `token` answers the server's
/// request for credentials (GitHub reads it as the password of any
/// user); nothing else is asked, and the token is not written into the
/// repository.
#[allow(clippy::result_large_err)]
pub(crate) fn fetch_into(
    git_dir: &Path,
    url: &str,
    token: Option<&str>,
) -> Result<(), CloneError> {
    let repo = open(git_dir).map_err(|source| CloneError::Open {
        path: git_dir.to_owned(),
        source: Box::new(source),
    })?;
    fetch(&repo, url, token).map_err(|source| CloneError::Fetch {
        url: url.to_owned(),
        source: Box::new(source),
    })
}

/// Opens the Git store at `git_dir` with a committer for its reflogs
/// when none is configured: a store with a working tree logs every ref
/// update, and gix refuses to without one.
// gix's error type, which is large, as the other calls here return it.
#[allow(clippy::result_large_err)]
pub(crate) fn open(
    git_dir: &Path,
) -> Result<gix::Repository, gix::open::Error> {
    let mut repo = gix::open(git_dir)?;
    // Fails only on a committer setting that does not parse, which the
    // ref edits then report.
    let _ = repo.committer_or_set_generic_fallback();
    Ok(repo)
}

/// [`fetch_into`]'s transfer, once the repository is open.
// The credentials closure returns gix's error type, which is large.
#[allow(clippy::result_large_err)]
pub(crate) fn fetch(
    repo: &gix::Repository,
    url: &str,
    token: Option<&str>,
) -> Result<(), TransferError> {
    // reqwest's rustls needs a process-wide provider; a second install
    // is refused, which is fine.
    let _ = tokio_rustls::rustls::crypto::ring::default_provider()
        .install_default();
    // `HEAD` names no local ref: it only asks the remote to list its
    // `HEAD`, which `mirror` follows.
    let remote = repo.remote_at(url)?.with_refspecs(
        REFSPECS.into_iter().chain(["HEAD"]),
        gix::remote::Direction::Fetch,
    )?;
    let mut connection = remote.connect(gix::remote::Direction::Fetch)?;
    if let Some(token) = token.map(str::to_owned) {
        connection.set_credentials(move |action| match action {
            Action::Get(context) => Ok(Some(answer(&token, context))),
            Action::Store(_) | Action::Erase(_) => Ok(None),
        });
    }
    let outcome = connection
        .prepare_fetch(gix::progress::Discard, Default::default())?
        .receive(gix::progress::Discard, &AtomicBool::new(false))?;
    let (branches, tags, head) = listed(&outcome.ref_map.remote_refs);
    mirror(repo, &branches, &tags, head).map_err(TransferError::Mirror)
}

/// What upstream's `HEAD` is, for [`mirror`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Head {
    /// Unknown: `origin/HEAD` stays as it is.
    Keep,
    /// A commit, not a branch: there is no default branch.
    Detached,
    /// The default branch, by its short name.
    Branch(BString),
}

/// The branches and tags the remote listed, by name, and its `HEAD`.
fn listed(
    remote: &[gix::protocol::handshake::Ref],
) -> (HashSet<BString>, HashSet<BString>, Head) {
    use gix::protocol::handshake::Ref;
    let (mut branches, mut tags, mut head) =
        (HashSet::new(), HashSet::new(), Head::Keep);
    for reference in remote {
        let (name, ..) = reference.unpack();
        if let Some(branch) = name.strip_prefix(b"refs/heads/") {
            branches.insert(branch.into());
        } else if let Some(tag) = name.strip_prefix(b"refs/tags/") {
            tags.insert(tag.into());
        }
        if name != "HEAD" {
            continue;
        }
        head = match reference {
            Ref::Symbolic { target, .. } | Ref::Unborn { target, .. } => {
                Head::of(target.as_bstr())
            }
            Ref::Direct { .. } | Ref::Peeled { .. } => Head::Detached,
        };
    }
    (branches, tags, head)
}

impl Head {
    /// The `HEAD` that names the ref `target`: a branch under
    /// `refs/heads/`, or no branch.
    pub(crate) fn of(target: &BStr) -> Self {
        match target.strip_prefix(b"refs/heads/") {
            Some(branch) => Self::Branch(branch.into()),
            None => Self::Detached,
        }
    }
}

/// Makes the Git store's upstream refs the ones listed: deletes the
/// remote-tracking branches and tags not in `branches` and `tags` (a
/// fetch or a copy only adds and moves refs), and makes `origin/HEAD`
/// follow `head`: `head`'s remote-tracking branch, or none when
/// upstream's `HEAD` is detached.
pub(crate) fn mirror(
    repo: &gix::Repository,
    branches: &HashSet<BString>,
    tags: &HashSet<BString>,
    head: Head,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut edits = Vec::new();
    let references = repo.references()?;
    for (prefix, keep) in [(TRACKING, branches), ("refs/tags/", tags)] {
        for reference in references.prefixed(prefix)? {
            let reference = reference?;
            let full = reference.name().as_bstr();
            let short = &full[prefix.len()..];
            if full != ORIGIN_HEAD && !keep.contains(short) {
                edits.push(RefEdit {
                    change: Change::Delete {
                        expected: PreviousValue::Any,
                        log: RefLog::AndReference,
                    },
                    name: reference.name().to_owned(),
                    deref: false,
                });
            }
        }
    }
    let change = match head {
        Head::Keep => None,
        Head::Detached => repo
            .try_find_reference(ORIGIN_HEAD)?
            .is_some()
            .then_some(Change::Delete {
                expected: PreviousValue::Any,
                log: RefLog::AndReference,
            }),
        Head::Branch(branch) => {
            let mut target = BString::from(TRACKING);
            target.extend_from_slice(&branch);
            Some(Change::Update {
                log: LogChange::default(),
                expected: PreviousValue::Any,
                new: Target::Symbolic(FullName::try_from(target)?),
            })
        }
    };
    if let Some(change) = change {
        edits.push(RefEdit {
            change,
            name: ORIGIN_HEAD.try_into()?,
            deref: false,
        });
    }
    repo.edit_references(edits)?;
    Ok(())
}

/// Makes [`REMOTE`] in the Git store at `git_dir` point at `url`, with
/// nothing else: a section left from before, with its refspecs, goes.
/// With no fetch refspec, a push never writes remote-tracking refs on
/// its own: tau writes the one for trunk once its push is through.
pub(crate) fn set_remote(
    git_dir: &Path,
    url: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let repo = gix::open(git_dir)?;
    let mut config = repo.config_snapshot().clone();
    while config
        .remove_section("remote", Some(REMOTE.into()))
        .is_some()
    {}
    config
        .new_section("remote", Some(REMOTE.into()))?
        .push("url", Some(url.into()))?;
    jj_lib::git::save_git_config(&config)?;
    Ok(())
}

fn answer(token: &str, context: Context) -> Outcome {
    Outcome {
        identity: Account {
            username: "x-access-token".into(),
            password: token.to_owned(),
            oauth_refresh_token: None,
        },
        next: context.into(),
    }
}
