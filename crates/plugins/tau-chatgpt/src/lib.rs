//! The person's ChatGPT plan, as a plugin (ADR 0045): it fills the
//! `tau.provider` slot with the plan's client, while the active sign-in
//! has plan usage.

#[cfg(feature = "host")]
mod half;

use std::path::Path;

#[cfg(feature = "host")]
pub use half::{ChatGptHost, Over};
use tau_ai::chatgpt::{AccountId, AccountStatus, PlanUsage, Store};
use tau_ui_plugin::{Declaration, Manifest, UiPlugin, provider::PROVIDER};

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

/// tau-chatgpt with its UI: its row on the Plugins screen.
#[derive(Debug, Clone, Copy, Default)]
pub struct ChatGptUi;

impl UiPlugin for ChatGptUi {
    type State = ();
    type Data = ();
    type RepoData = ();
    type Settings = ();

    fn name(&self) -> &'static str {
        NAME
    }

    fn declaration(&self) -> Declaration {
        Declaration::new()
            .fills(&[PROVIDER.name()])
            .network(&["chatgpt.com", "auth.openai.com"])
            .secrets(&["chatgpt"])
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
    }
}
