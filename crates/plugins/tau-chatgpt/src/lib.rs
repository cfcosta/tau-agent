//! The person's ChatGPT plan, as a plugin (ADR 0045): it fills the
//! `tau.provider` slot with the plan's client, while the active sign-in
//! has plan usage.

#[cfg(feature = "host")]
mod half;
pub mod plan_usage;
pub mod ui;

use std::path::Path;

#[cfg(feature = "host")]
pub use half::{ChatGptHost, Over, not_eligible};
use tau_ai::chatgpt::{AccountId, AccountStatus, PlanUsage, Store};
pub use ui::ChatGptUi;

pub const NAME: &str = "tau-chatgpt";

/// What runs use, said in the picker's footer.
pub const LABEL: &str = "ChatGPT plan";

/// The active ChatGPT account kept in `config` (tau's config directory),
/// when it is signed in with plan usage. Reading never makes the store.
pub fn plan_account(config: &Path) -> Option<AccountId> {
    let dir = config.join("chatgpt");
    let store = dir.is_dir().then(|| Store::open(&dir).ok()).flatten()?;
    let account = store.active().ok()??;
    let status = store.load(&account).ok()?.status();
    (status == AccountStatus::SignedIn(PlanUsage::Enabled)).then_some(account)
}
