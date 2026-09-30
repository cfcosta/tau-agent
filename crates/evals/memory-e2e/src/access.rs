//! How the evaluation reaches a model: an OpenAI API key or a ChatGPT
//! sign-in that uses the plan, found the way tau-ui keeps them.

use std::path::{Path, PathBuf};

use tau_ai::chatgpt::{AccountId, AccountStatus, PlanUsage, Store};

/// How runs reach the model.
#[derive(Clone, PartialEq, Eq)]
pub enum Access {
    /// A ChatGPT sign-in with plan usage: the account, in the store at
    /// `store`.
    ChatGpt { store: PathBuf, account: AccountId },
    /// An OpenAI API key.
    ApiKey(String),
}

impl std::fmt::Debug for Access {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ChatGpt { account, .. } => {
                f.debug_tuple("ChatGpt").field(account).finish()
            }
            Self::ApiKey(_) => f.write_str("ApiKey(..)"),
        }
    }
}

/// What runs use when no model is chosen, as in tau-ui.
pub const DEFAULT_MODEL: &str = "gpt-5.5";

impl Access {
    pub fn label(&self) -> String {
        match self {
            Self::ChatGpt { account, .. } => {
                format!("ChatGPT plan ({account})")
            }
            Self::ApiKey(_) => "OpenAI API key".into(),
        }
    }
}

/// The access to use, first found of:
///
/// 1. `account`, a ChatGPT account named on the command line, in tau's
///    store (`chatgpt/` in `config`);
/// 2. `env_key`, the `OPENAI_API_KEY` environment variable, when set;
/// 3. tau's active ChatGPT account, when it is signed in with plan usage;
/// 4. tau's saved API key, `openai-key` in `config`.
pub fn resolve(
    account: Option<AccountId>,
    env_key: Option<String>,
    config: Option<&Path>,
) -> Option<Access> {
    let store = config.map(|config| config.join("chatgpt"));
    if let (Some(account), Some(store)) = (account, &store) {
        return Some(Access::ChatGpt {
            store: store.clone(),
            account,
        });
    }
    if let Some(key) = env_key.map(|key| key.trim().to_owned())
        && !key.is_empty()
    {
        return Some(Access::ApiKey(key));
    }
    let config = config?;
    if let Some(store) = store
        && let Some(account) = plan_account(&store)
    {
        return Some(Access::ChatGpt { store, account });
    }
    std::fs::read_to_string(config.join("openai-key"))
        .ok()
        .map(|key| key.trim().to_owned())
        .filter(|key| !key.is_empty())
        .map(Access::ApiKey)
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
