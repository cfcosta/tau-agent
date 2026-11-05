//! The person's ChatGPT plan, as a plugin (ADR 0045): it fills the
//! `tau.provider` slot with the plan's client, while the active sign-in
//! has plan usage.

#[cfg(feature = "host")]
mod half;
pub mod plan_usage;
pub mod ui;

use std::path::Path;

#[cfg(feature = "host")]
pub use half::{ChatGptHost, Over, not_eligible, plan_models};
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

#[cfg(test)]
mod tests {
    use tau_ai::chatgpt::{ChatGpt, Credentials as Record, HostId};

    use super::*;

    /// A saved sign-in record, as the store keeps it.
    fn record(
        label: &str,
        subject: &str,
        scopes: &[&str],
        tokens: bool,
    ) -> Record {
        Record {
            label: label.into(),
            email: Some(label.into()),
            issuer: "https://auth.openai.com".into(),
            subject: subject.into(),
            client_id: "oaiapp_test".into(),
            ext_agent_host_id: HostId::new_uuid(),
            id_token: None,
            access_token: tokens.then(|| "at".into()),
            refresh_token: tokens.then(|| "rt".into()),
            token_type: None,
            expires_in: None,
            expires_at: Some(u64::MAX),
            earliest_refresh_at: None,
            scopes: scopes.iter().map(|s| (*s).to_owned()).collect(),
            saved_at: String::new(),
        }
    }

    const PLAN: &[&str] = &["chatgpt.tokens.use.direct", "openid"];

    #[test]
    fn runs_use_only_a_saved_plan_account() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path();
        assert_eq!(plan_account(config), None);
        assert!(!config.join("chatgpt").exists(), "reading makes nothing");
        // A key left from before is never read.
        std::fs::write(dir.path().join("openai-key"), "sk-saved\n").unwrap();
        assert_eq!(plan_account(config), None);

        // A sign-in with plan usage is what runs use.
        let store = ChatGpt::new(Store::open(config.join("chatgpt")).unwrap());
        let plan = record("a@example.com", "sub-a", PLAN, true);
        store.store().save(&plan).unwrap();
        store.store().set_active(&plan.id()).unwrap();
        assert_eq!(plan_account(config), Some(plan.id()));

        // One without it leaves nothing to run on.
        let declined = record("b@example.com", "sub-b", &["openid"], true);
        store.store().save(&declined).unwrap();
        store.store().set_active(&declined.id()).unwrap();
        assert_eq!(plan_account(config), None);
        assert_eq!(store.store().active().unwrap(), Some(declined.id()));

        // Signed out: nothing.
        store
            .store()
            .save(&record("b@example.com", "sub-b", &["openid"], false))
            .unwrap();
        assert_eq!(plan_account(config), None);
    }
}
