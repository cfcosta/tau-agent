//! Telling tau's operations from anyone else's (ADR 0043, decision 6):
//! every operation tau writes in a repository carries the attribute
//! [`TAU_OP_ATTRIBUTE`] and a description that starts with
//! [`DESCRIPTION_PREFIX`], so `jj op log` shows `tau: vcs_commit`,
//! `tau: snapshot working copy` and so on, and [`is_taus`] knows them.
//! They are all written by [`crate::colocate::commit`], the one place
//! tau commits a transaction, and tau makes its workspaces without the
//! operation jj-lib would write for them ([`crate::workspace_init`]).

#![allow(
    clippy::disallowed_methods,
    reason = "runs only inside a job in spawn_blocking (ADR 0028)"
)]

use jj_lib::{operation::Operation, transaction::Transaction};
use pollster::block_on;

use crate::error::VcsError;

/// The operation attribute every operation tau writes carries. Its
/// value is the version of tau-vcs-host that wrote it.
pub const TAU_OP_ATTRIBUTE: &str = "tau.op";

/// How the description of every operation tau writes starts.
pub const DESCRIPTION_PREFIX: &str = "tau: ";

/// Marks `tx` as tau's, and returns its description: `what`, after
/// [`DESCRIPTION_PREFIX`].
pub(crate) fn tag(tx: &mut Transaction, what: &str) -> String {
    tx.set_attribute(
        TAU_OP_ATTRIBUTE.to_owned(),
        env!("CARGO_PKG_VERSION").to_owned(),
    );
    format!("{DESCRIPTION_PREFIX}{what}")
}

/// Whether tau wrote `op`: it carries [`TAU_OP_ATTRIBUTE`].
///
/// Two kinds of operation carry no one's tag. The root operation, which
/// every repository starts from, is no one's, and not tau's. A merge,
/// one with several parents, is what jj-lib writes on loading a
/// repository whose operation log forked ("reconcile divergent
/// operations"), in tau or in a `jj` command alike, whichever loads
/// first: it is tau's when every operation it merges is. A person's
/// own operations come through as its parents.
pub fn is_taus(op: &Operation) -> Result<bool, VcsError> {
    if op.metadata().attributes.contains_key(TAU_OP_ATTRIBUTE) {
        return Ok(true);
    }
    if op.parent_ids().len() < 2 {
        return Ok(false);
    }
    for parent in block_on(op.parents())? {
        if !is_taus(&parent)? {
            return Ok(false);
        }
    }
    Ok(true)
}
