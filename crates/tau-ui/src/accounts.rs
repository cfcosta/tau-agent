//! Where tau keeps what the person signed in with: tau's config
//! directory, whose `chatgpt/` holds the ChatGPT sign-ins tau-chatgpt
//! makes and reads (ADR 0045). Only what the person set up in tau
//! counts; the environment is never read.

use std::path::{Path, PathBuf};

use tau_ai::chatgpt::{AccountId, ChatGpt, ChatGptError, Store};

/// Where tau keeps what the user signed in with, each readable only by
/// them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    /// Usually `$XDG_CONFIG_HOME/tau`.
    pub dir: PathBuf,
}

impl Credentials {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// tau's config directory: `$XDG_CONFIG_HOME/tau`.
    pub fn default_dir() -> Self {
        let dir = Store::default_dir()
            .ok()
            .and_then(|dir| dir.parent().map(Path::to_owned))
            .unwrap_or_else(|| PathBuf::from("."));
        Self::new(dir)
    }

    /// The ChatGPT sign-ins: `chatgpt/` (see `tau_ai::chatgpt::Store`).
    pub fn chatgpt_dir(&self) -> PathBuf {
        self.dir.join("chatgpt")
    }

    /// The ChatGPT sign-ins, made on first use.
    pub fn chatgpt(&self) -> Result<ChatGpt, ChatGptError> {
        Ok(ChatGpt::new(Store::open(self.chatgpt_dir())?))
    }

    /// The store of sign-ins, if one was made: reading never makes one.
    fn saved_store(&self) -> Option<Store> {
        let dir = self.chatgpt_dir();
        dir.is_dir().then(|| Store::open(dir).ok()).flatten()
    }

    /// What runs use: the active ChatGPT account, when it is signed in
    /// with plan usage.
    pub fn plan_account(&self) -> Option<AccountId> {
        tau_chatgpt::plan_account(&self.dir)
    }

    /// The active ChatGPT account, whatever its state.
    pub fn active_account(&self) -> Option<AccountId> {
        self.saved_store()?.active().ok()?
    }

    /// Makes `account` the one tau signs in with.
    pub fn switch(&self, account: &str) -> Result<(), String> {
        let id = AccountId::parse(account)
            .ok_or_else(|| format!("{account:?} is not an account id"))?;
        let store = self.chatgpt().map_err(|error| error.to_string())?;
        store
            .store()
            .set_active(&id)
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use tau_ai::chatgpt::{Credentials as Record, HostId};

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
    fn access_comes_only_from_what_was_saved() {
        let dir = tempfile::tempdir().unwrap();
        let credentials = Credentials::new(dir.path());
        assert_eq!(credentials.plan_account(), None);
        assert!(!credentials.chatgpt_dir().exists(), "reading makes nothing");
        // A key left from before is never read.
        std::fs::write(dir.path().join("openai-key"), "sk-saved\n").unwrap();
        assert_eq!(credentials.plan_account(), None);

        // A sign-in with plan usage is what runs use.
        let store = credentials.chatgpt().unwrap();
        let plan = record("a@example.com", "sub-a", PLAN, true);
        store.store().save(&plan).unwrap();
        store.store().set_active(&plan.id()).unwrap();
        assert_eq!(credentials.plan_account(), Some(plan.id()));

        // One without it leaves nothing to run on.
        let declined = record("b@example.com", "sub-b", &["openid"], true);
        store.store().save(&declined).unwrap();
        credentials.switch(declined.id().as_str()).unwrap();
        assert_eq!(credentials.plan_account(), None);
        assert_eq!(credentials.active_account(), Some(declined.id()));

        // Signed out: nothing.
        store
            .store()
            .save(&record("b@example.com", "sub-b", &["openid"], false))
            .unwrap();
        assert_eq!(credentials.plan_account(), None);
        assert!(credentials.switch("../nope").is_err());
    }
}
