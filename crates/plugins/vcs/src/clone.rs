//! Cloning a remote repository without the git binary: gix speaks the
//! smart HTTP protocol itself, over reqwest and rustls.

use std::{
    io,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

use gix::{
    credentials::{
        helper::Action,
        protocol::{Context, Outcome},
    },
    sec::identity::Account,
};

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
        path: PathBuf,
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
    Prepare(#[from] gix::clone::Error),
    #[error(transparent)]
    Transfer(#[from] gix::clone::fetch::Error),
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
}

/// Clones `url` into a new bare repository at `into`, fetching every
/// branch and tag. A `token` answers the server's request for
/// credentials (GitHub reads it as the password of any user); nothing
/// else is asked, and the token is not written into the repository.
///
/// Blocks until the fetch is over. On failure `into` is removed.
// The credentials closure returns gix's error type, which is large.
#[allow(clippy::result_large_err)]
pub fn clone_bare(
    url: &str,
    token: Option<&str>,
    into: &Path,
) -> Result<(), CloneError> {
    // reqwest's rustls needs a process-wide provider; a second install
    // is refused, which is fine.
    let _ = tokio_rustls::rustls::crypto::ring::default_provider()
        .install_default();
    let cloned = (|| -> Result<(), TransferError> {
        if let Some(parent) = into.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut prepare = gix::prepare_clone_bare(url, into)?;
        if let Some(token) = token.map(str::to_owned) {
            prepare = prepare.configure_connection(move |connection| {
                let token = token.clone();
                connection.set_credentials(move |action| match action {
                    Action::Get(context) => Ok(Some(answer(&token, context))),
                    Action::Store(_) | Action::Erase(_) => Ok(None),
                });
                Ok(())
            });
        }
        prepare.fetch_only(gix::progress::Discard, &AtomicBool::new(false))?;
        Ok(())
    })();
    if cloned.is_err() {
        let _ = std::fs::remove_dir_all(into);
    }
    cloned.map_err(|source| CloneError::Clone {
        url: url.to_owned(),
        source: Box::new(source),
    })
}

/// Fetches every branch and tag of `url` into the bare repository at
/// `git_dir`, moving its branches to where the remote has them. A
/// `token` answers the server's request for credentials, as for
/// [`clone_bare`].
#[allow(clippy::result_large_err)]
pub(crate) fn fetch_into(
    git_dir: &Path,
    url: &str,
    token: Option<&str>,
) -> Result<(), CloneError> {
    let _ = tokio_rustls::rustls::crypto::ring::default_provider()
        .install_default();
    let repo = gix::open(git_dir).map_err(|source| CloneError::Open {
        path: git_dir.to_owned(),
        source: Box::new(source),
    })?;
    fetch_from(&repo, url, token).map_err(|source| CloneError::Fetch {
        url: url.to_owned(),
        source: Box::new(source),
    })
}

/// [`fetch_into`]'s transfer, once the repository is open.
// The credentials closure returns gix's error type, which is large.
#[allow(clippy::result_large_err)]
fn fetch_from(
    repo: &gix::Repository,
    url: &str,
    token: Option<&str>,
) -> Result<(), TransferError> {
    let remote = repo.remote_at(url)?.with_refspecs(
        ["+refs/heads/*:refs/heads/*", "+refs/tags/*:refs/tags/*"],
        gix::remote::Direction::Fetch,
    )?;
    let mut connection = remote.connect(gix::remote::Direction::Fetch)?;
    if let Some(token) = token.map(str::to_owned) {
        connection.set_credentials(move |action| match action {
            Action::Get(context) => Ok(Some(answer(&token, context))),
            Action::Store(_) | Action::Erase(_) => Ok(None),
        });
    }
    connection
        .prepare_fetch(gix::progress::Discard, Default::default())?
        .receive(gix::progress::Discard, &AtomicBool::new(false))?;
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
