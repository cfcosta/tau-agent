//! Cloning a remote repository without the git binary: gix speaks the
//! smart HTTP protocol itself, over reqwest and rustls.

use std::{path::Path, sync::atomic::AtomicBool};

use anyhow::Context as _;
use gix::{
    credentials::{
        helper::Action,
        protocol::{Context, Outcome},
    },
    sec::identity::Account,
};

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
) -> anyhow::Result<()> {
    // reqwest's rustls needs a process-wide provider; a second install
    // is refused, which is fine.
    let _ = tokio_rustls::rustls::crypto::ring::default_provider()
        .install_default();
    let cloned = (|| -> anyhow::Result<()> {
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
    cloned.with_context(|| format!("Cannot clone {url}"))
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
