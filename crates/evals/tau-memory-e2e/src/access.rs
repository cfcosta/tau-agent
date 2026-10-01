//! How the evaluation reaches a model: a saved ChatGPT sign-in that uses
//! the plan, found the way tau-ui keeps it. There is no other way in.

use std::path::{Path, PathBuf};

use tau_ai::chatgpt::{AccountId, AccountStatus, PlanUsage, Store};

/// A ChatGPT sign-in with plan usage: the account, in the store at
/// `store`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Access {
    pub store: PathBuf,
    pub account: AccountId,
}

/// What runs use when no model is chosen, as in tau-ui.
pub const DEFAULT_MODEL: &str = "gpt-5.5";

impl Access {
    pub fn label(&self) -> String {
        format!("ChatGPT plan ({})", self.account)
    }
}

/// The access to use, first found of:
///
/// 1. `account`, a ChatGPT account named on the command line, in tau's
///    store (`chatgpt/` in `config`);
/// 2. tau's active ChatGPT account, when it is signed in with plan usage.
pub fn resolve(
    account: Option<AccountId>,
    config: Option<&Path>,
) -> Option<Access> {
    let store = config?.join("chatgpt");
    match account {
        Some(account) => Some(Access { store, account }),
        None => plan_account(&store).map(|account| Access { store, account }),
    }
}

/// The store's active account, if it may use the plan.
fn plan_account(dir: &Path) -> Option<AccountId> {
    if !dir.is_dir() {
        return None;
    }
    let store = Store::open(dir).ok()?;
    let account = store.active().ok()??;
    let status = store.load(&account).ok()?.status();
    (status == AccountStatus::SignedIn(PlanUsage::Enabled)).then_some(account)
}

/// tau's config directory, where tau-ui keeps what the user signed in
/// with.
pub fn config_dir() -> Option<PathBuf> {
    Store::default_dir()
        .ok()
        .and_then(|dir| dir.parent().map(Path::to_owned))
}
