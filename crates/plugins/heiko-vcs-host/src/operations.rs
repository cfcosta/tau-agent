//! Telling heiko's operations from anyone else's (ADR 0043, decision 6):
//! every operation heiko writes in a repository carries the attribute
//! [`HEIKO_OP_ATTRIBUTE`] and a description that starts with
//! [`DESCRIPTION_PREFIX`], so `jj op log` shows `heiko: vcs_commit`,
//! `heiko: snapshot working copy` and so on, and [`is_heikos`] knows them.
//! They are all written by [`crate::colocate::commit`], the one place
//! heiko commits a transaction, and heiko makes its workspaces without the
//! operation jj-lib would write for them ([`crate::workspace_init`]).

#![allow(
    clippy::disallowed_methods,
    reason = "runs only inside a job in spawn_blocking (ADR 0028)"
)]

use jj_lib::{operation::Operation, transaction::Transaction};
use pollster::block_on;

use crate::error::VcsError;

/// The operation attribute every operation heiko writes carries. Its
/// value is the version of heiko-vcs-host that wrote it.
pub const HEIKO_OP_ATTRIBUTE: &str = "heiko.op";

/// How the description of every operation heiko writes starts.
pub const DESCRIPTION_PREFIX: &str = "heiko: ";

/// Marks `tx` as heiko's, and returns its description: `what`, after
/// [`DESCRIPTION_PREFIX`].
pub(crate) fn tag(tx: &mut Transaction, what: &str) -> String {
    tx.set_attribute(
        HEIKO_OP_ATTRIBUTE.to_owned(),
        env!("CARGO_PKG_VERSION").to_owned(),
    );
    format!("{DESCRIPTION_PREFIX}{what}")
}

/// Whether heiko wrote `op`: it carries [`HEIKO_OP_ATTRIBUTE`].
///
/// Two kinds of operation carry no one's tag. The root operation, which
/// every repository starts from, is no one's, and not heiko's. A merge,
/// one with several parents, is what jj-lib writes on loading a
/// repository whose operation log forked ("reconcile divergent
/// operations"), in heiko or in a `jj` command alike, whichever loads
/// first: it is heiko's when every operation it merges is. A person's
/// own operations come through as its parents.
pub fn is_heikos(op: &Operation) -> Result<bool, VcsError> {
    if op.metadata().attributes.contains_key(HEIKO_OP_ATTRIBUTE) {
        return Ok(true);
    }
    if op.parent_ids().len() < 2 {
        return Ok(false);
    }
    for parent in block_on(op.parents())? {
        if !is_heikos(&parent)? {
            return Ok(false);
        }
    }
    Ok(true)
}
