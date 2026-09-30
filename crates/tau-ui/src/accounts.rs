//! How tau reaches a model: a ChatGPT plan through Sign in with ChatGPT
//! (`tau_ai::chatgpt`), or an OpenAI API key; where each is kept; and
//! signing in, switching accounts and signing out. Only what the user set
//! up in tau counts; the environment is never read, so tau does not run
//! on whatever key a shell exports.
//!
//! Runs use the active ChatGPT account's plan when it is signed in with
//! plan usage, else the saved API key. A plan error never moves a run to
//! the key: it stops, and says what to do.

use std::{
    cell::RefCell,
    io,
    path::{Path, PathBuf},
    rc::Rc,
};

use gpui::{App, Entity};
use tau_ai::chatgpt::{
    AccountId,
    AccountStatus,
    ChatGpt,
    ChatGptError,
    Loopback,
    PlanUsage,
    Revocation,
    SignedIn,
    Store,
};
use tokio::sync::{mpsc, oneshot};

use crate::{
    models::{AccessKind, AccountState, ChatGptAccount, DEFAULT_MODEL},
    setup::{ModelAccess, SetupStep, SetupUpdate},
    workspace::{Workspace, WorkspaceEvent},
};

/// How the host reaches a model.
#[derive(Clone, PartialEq, Eq)]
pub enum Access {
    /// The plan of this ChatGPT account, in tau's store of sign-ins.
    ChatGpt(AccountId),
    /// An OpenAI API key.
    ApiKey(String),
}

impl std::fmt::Debug for Access {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ChatGpt(account) => {
                f.debug_tuple("ChatGpt").field(account).finish()
            }
            Self::ApiKey(_) => f.write_str("ApiKey(..)"),
        }
    }
}

impl Access {
    pub fn kind(&self) -> AccessKind {
        match self {
            Self::ChatGpt(_) => AccessKind::ChatGpt,
            Self::ApiKey(_) => AccessKind::ApiKey,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::ChatGpt(_) => "ChatGPT plan",
            Self::ApiKey(_) => "OpenAI API key",
        }
    }

    /// How the model reads in onboarding: `gpt-5.5 · ChatGPT plan`.
    pub fn short_label(&self, model: &str) -> String {
        format!("{model} · {}", self.label())
    }
}

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

    pub fn api_key(&self) -> PathBuf {
        self.dir.join("openai-key")
    }

    pub fn github(&self) -> PathBuf {
        self.dir.join("github.json")
    }

    /// Where the TypeSafe key for Jev is kept.
    pub fn jev(&self) -> PathBuf {
        self.dir.join("typesafe-key")
    }

    /// The saved TypeSafe key, which tau-constitution checks with.
    pub fn jev_key(&self) -> Option<String> {
        std::fs::read_to_string(self.jev())
            .ok()
            .map(|key| key.trim().to_owned())
            .filter(|key| !key.is_empty())
    }

    /// Saves the TypeSafe key, or forgets it with `None`.
    pub fn set_jev_key(&self, key: Option<&str>) -> io::Result<()> {
        match key.map(str::trim).filter(|key| !key.is_empty()) {
            Some(key) => write_private(&self.jev(), key.as_bytes()),
            None => remove(&self.jev()),
        }
    }

    /// What runs use: the active ChatGPT account's plan when it may be
    /// used, else the saved API key.
    pub fn access(&self) -> Option<Access> {
        if let Some(account) = self.plan_account() {
            return Some(Access::ChatGpt(account));
        }
        self.saved_key().map(Access::ApiKey)
    }

    /// The active ChatGPT account, when it is signed in with plan usage.
    pub fn plan_account(&self) -> Option<AccountId> {
        let store = self.saved_store()?;
        let account = store.active().ok()??;
        let status = store.load(&account).ok()?.status();
        (status == AccountStatus::SignedIn(PlanUsage::Enabled))
            .then_some(account)
    }

    /// The active ChatGPT account, whatever its state.
    pub fn active_account(&self) -> Option<AccountId> {
        self.saved_store()?.active().ok()?
    }

    /// The saved ChatGPT sign-ins, by label, for the account picker.
    pub fn accounts(&self) -> Vec<ChatGptAccount> {
        let Some(store) = self.saved_store() else {
            return Vec::new();
        };
        let active = store.active().ok().flatten();
        store
            .accounts()
            .unwrap_or_default()
            .into_iter()
            .map(|saved| {
                let id = saved.id();
                ChatGptAccount {
                    active: active.as_ref() == Some(&id),
                    id: id.to_string(),
                    label: saved.label.clone(),
                    state: match saved.status() {
                        AccountStatus::SignedIn(PlanUsage::Enabled) => {
                            AccountState::Plan
                        }
                        AccountStatus::SignedIn(PlanUsage::Disabled) => {
                            AccountState::PlanDisabled
                        }
                        AccountStatus::SignedOut => AccountState::SignedOut,
                    },
                }
            })
            .collect()
    }

    /// Whether that kind of access is saved: a signed-in ChatGPT account
    /// (with plan usage or not), or an API key.
    pub fn has(&self, kind: AccessKind) -> bool {
        match kind {
            AccessKind::ChatGpt => self
                .accounts()
                .iter()
                .any(|account| account.state != AccountState::SignedOut),
            AccessKind::ApiKey => self.saved_key().is_some(),
        }
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

    fn saved_key(&self) -> Option<String> {
        std::fs::read_to_string(self.api_key())
            .ok()
            .map(|key| key.trim().to_owned())
            .filter(|key| !key.is_empty())
    }

    pub fn save_api_key(&self, key: &str) -> io::Result<()> {
        write_private(&self.api_key(), key.trim().as_bytes())
    }

    /// Forgets the API key. Forgetting none is fine.
    pub fn forget_api_key(&self) -> io::Result<()> {
        remove(&self.api_key())
    }
}

fn remove(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// Writes `bytes` to `path`, readable only by the user.
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?.write_all(bytes)
}

/// Called with what runs use now, after a sign-in, a switch or a
/// sign-out: `None` when nothing is left.
pub type Connected = Rc<dyn Fn(Option<Access>, &mut App)>;

/// A ChatGPT sign-in waiting for the browser; dropping it cancels it.
struct Pending {
    /// Takes a redirect URL pasted instead, once.
    paste: Option<oneshot::Sender<String>>,
    _cancel: oneshot::Sender<()>,
}

/// Carries out the model sign-ins the workspace asks for: ChatGPT in the
/// browser (or a pasted redirect), switching and signing out of ChatGPT
/// accounts, and API keys. Reports progress to the workspace's setup,
/// and calls `connected` once what runs use changed. One per workspace:
/// it holds the sign-in in progress.
#[derive(Default)]
pub struct SignIns {
    pending: Rc<RefCell<Option<Pending>>>,
}

/// How a sign-in on its own thread is going.
enum Progress {
    /// The page to open.
    Url(String),
    SignedIn(Result<SignedIn, String>),
}

impl SignIns {
    /// Handles `event` if it is one of these. Returns whether it was.
    pub fn handle(
        &self,
        event: &WorkspaceEvent,
        workspace: &Entity<Workspace>,
        credentials: &Credentials,
        model: Option<&str>,
        connected: &Connected,
        cx: &mut App,
    ) -> bool {
        match event {
            WorkspaceEvent::ChatGptSignIn { account, consent } => {
                self.sign_in(
                    account.as_deref().and_then(AccountId::parse),
                    *consent,
                    workspace,
                    credentials,
                    model,
                    connected,
                    cx,
                );
                true
            }
            WorkspaceEvent::ChatGptCallback { url } => {
                let paste = self
                    .pending
                    .borrow_mut()
                    .as_mut()
                    .and_then(|pending| pending.paste.take());
                if let Some(paste) = paste {
                    let _ = paste.send(url.clone());
                }
                true
            }
            WorkspaceEvent::SwitchChatGpt { account } => {
                match credentials.switch(account) {
                    Ok(()) => connected(credentials.access(), cx),
                    Err(error) => workspace.update(cx, |ws, cx| {
                        ws.show_alert("Could not switch accounts", error, cx)
                    }),
                }
                true
            }
            WorkspaceEvent::SignOut(AccessKind::ChatGpt) => {
                sign_out(workspace, credentials, connected, cx);
                true
            }
            WorkspaceEvent::ApiKey { key } => {
                match credentials.save_api_key(key) {
                    Ok(()) => {
                        let access = Access::ApiKey(key.trim().to_owned());
                        let label =
                            access.short_label(model.unwrap_or(DEFAULT_MODEL));
                        connected(Some(access), cx);
                        workspace.update(cx, |ws, cx| {
                            ws.update_setup(
                                SetupUpdate::Model(ModelAccess::Connected {
                                    label,
                                }),
                                cx,
                            )
                        });
                    }
                    Err(error) => fail(workspace, error.to_string(), cx),
                }
                true
            }
            _ => false,
        }
    }

    /// Signs in to ChatGPT: `account` again, or a new registration with
    /// `None`; `consent` asks again for plan usage. The browser opens at
    /// the authorization page; the loopback listener, or a pasted
    /// redirect, finishes it.
    #[allow(clippy::too_many_arguments)]
    fn sign_in(
        &self,
        account: Option<AccountId>,
        consent: bool,
        workspace: &Entity<Workspace>,
        credentials: &Credentials,
        model: Option<&str>,
        connected: &Connected,
        cx: &mut App,
    ) {
        let (paste, pasted) = oneshot::channel();
        let (cancel, cancelled) = oneshot::channel();
        // A new sign-in replaces one still waiting.
        *self.pending.borrow_mut() = Some(Pending {
            paste: Some(paste),
            _cancel: cancel,
        });
        let (progress, mut updates) = mpsc::unbounded_channel();
        let chatgpt = credentials.chatgpt();
        std::thread::spawn(move || {
            let signed_in = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| error.to_string())
                .and_then(|runtime| {
                    runtime.block_on(async {
                        let chatgpt = chatgpt.map_err(|e| e.to_string())?;
                        tokio::select! {
                            done = browser_sign_in(
                                &chatgpt, account, consent, pasted, &progress,
                            ) => done,
                            _ = cancelled => Err("sign-in cancelled".into()),
                        }
                    })
                });
            let _ = progress.send(Progress::SignedIn(signed_in));
        });
        let workspace = workspace.downgrade();
        let (credentials, connected, pending) =
            (credentials.clone(), connected.clone(), self.pending.clone());
        let model = model.unwrap_or(DEFAULT_MODEL).to_owned();
        cx.spawn(async move |cx| {
            while let Some(update) = updates.recv().await {
                let Some(workspace) = workspace.upgrade() else {
                    return;
                };
                cx.update(|cx| match update {
                    Progress::Url(url) => {
                        cx.open_url(&url);
                        let pending = ModelAccess::SigningIn { url: Some(url) };
                        workspace.update(cx, |ws, cx| {
                            ws.update_setup(SetupUpdate::Model(pending), cx)
                        });
                    }
                    Progress::SignedIn(done) => {
                        pending.borrow_mut().take();
                        let setup = match done {
                            Ok(signed_in) => {
                                connected(credentials.access(), cx);
                                match signed_in.plan_usage {
                                    PlanUsage::Enabled => {
                                        ModelAccess::Connected {
                                            label: format!(
                                                "{model} · ChatGPT plan"
                                            ),
                                        }
                                    }
                                    PlanUsage::Disabled => {
                                        ModelAccess::PlanDisabled {
                                            account: signed_in.label,
                                        }
                                    }
                                }
                            }
                            Err(error) => ModelAccess::Failed(error),
                        };
                        workspace.update(cx, |ws, cx| {
                            ws.update_setup(SetupUpdate::Model(setup), cx)
                        });
                    }
                });
            }
        })
        .detach();
    }
}

/// The browser round trip: listen, report the page to open, then take
/// the callback from the browser or a pasted redirect.
async fn browser_sign_in(
    chatgpt: &ChatGpt,
    account: Option<AccountId>,
    consent: bool,
    pasted: oneshot::Receiver<String>,
    progress: &mpsc::UnboundedSender<Progress>,
) -> Result<SignedIn, String> {
    // The listener starts before the browser opens.
    let loopback = Loopback::bind().await.map_err(|e| e.to_string())?;
    let attempt = chatgpt
        .start_sign_in(account.as_ref(), loopback.redirect_uri(), consent)
        .map_err(|e| e.to_string())?;
    let _ = progress.send(Progress::Url(attempt.url().to_owned()));
    let callback = tokio::select! {
        callback = loopback.wait(&attempt) => callback,
        Ok(url) = pasted => attempt.callback(&url),
    };
    let callback = callback.map_err(|e| e.to_string())?;
    chatgpt
        .finish_sign_in(&attempt, &callback)
        .await
        .map_err(|e| e.to_string())
}

/// Signs the active ChatGPT account out on its own thread: revokes its
/// session, forgets its tokens, and says so when OpenAI did not confirm.
/// Runs go on with what is left, if anything.
fn sign_out(
    workspace: &Entity<Workspace>,
    credentials: &Credentials,
    connected: &Connected,
    cx: &mut App,
) {
    let Some(account) = credentials.active_account() else {
        return;
    };
    let chatgpt = credentials.chatgpt();
    let (done, finished) = oneshot::channel();
    std::thread::spawn(move || {
        let revoked = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())
            .and_then(|runtime| {
                let chatgpt = chatgpt.map_err(|e| e.to_string())?;
                runtime
                    .block_on(chatgpt.sign_out(&account))
                    .map_err(|e| e.to_string())
            });
        let _ = done.send(revoked);
    });
    let workspace = workspace.downgrade();
    let (credentials, connected) = (credentials.clone(), connected.clone());
    cx.spawn(async move |cx| {
        let Ok(revoked) = finished.await else { return };
        cx.update(|cx| {
            let left = credentials.access();
            connected(left.clone(), cx);
            let Some(workspace) = workspace.upgrade() else {
                return;
            };
            workspace.update(cx, |ws, cx| {
                match revoked {
                    Ok(Revocation::Unconfirmed { reason }) => ws.show_alert(
                        "Signed out of ChatGPT",
                        format!(
                            "tau forgot this account's tokens, but OpenAI did \
                             not confirm the end of the session ({reason}). \
                             You can disconnect tau in ChatGPT Settings."
                        ),
                        cx,
                    ),
                    Ok(_) => {}
                    Err(error) => {
                        ws.show_alert("Could not sign out", error, cx)
                    }
                }
                if left.is_none() {
                    // Nothing left to run on: set it up again.
                    ws.update_setup(SetupUpdate::Model(ModelAccess::None), cx);
                    ws.start_setup(SetupStep::Model, cx);
                }
            });
        });
    })
    .detach();
}

fn fail(workspace: &Entity<Workspace>, error: String, cx: &mut App) {
    workspace.update(cx, |ws, cx| {
        ws.update_setup(SetupUpdate::Model(ModelAccess::Failed(error)), cx)
    });
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
        assert_eq!(credentials.access(), None);
        assert!(!credentials.chatgpt_dir().exists(), "reading makes nothing");
        std::fs::write(credentials.api_key(), "  \n").unwrap();
        assert_eq!(credentials.access(), None, "a blank key is no key");
        credentials.save_api_key("sk-saved\n").unwrap();
        assert_eq!(
            credentials.access(),
            Some(Access::ApiKey("sk-saved".into()))
        );

        // A sign-in with plan usage comes first.
        let store = credentials.chatgpt().unwrap();
        let plan = record("a@example.com", "sub-a", PLAN, true);
        store.store().save(&plan).unwrap();
        store.store().set_active(&plan.id()).unwrap();
        assert_eq!(credentials.access(), Some(Access::ChatGpt(plan.id())));

        // One without it leaves the key in use.
        let declined = record("b@example.com", "sub-b", &["openid"], true);
        store.store().save(&declined).unwrap();
        credentials.switch(declined.id().as_str()).unwrap();
        assert_eq!(
            credentials.access().map(|a| a.kind()),
            Some(AccessKind::ApiKey)
        );
        let accounts = credentials.accounts();
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0].state, AccountState::Plan);
        assert!(!accounts[0].active);
        assert_eq!(accounts[1].state, AccountState::PlanDisabled);
        assert!(accounts[1].active);
        assert!(credentials.has(AccessKind::ChatGpt));

        // Signed out, and without a key: nothing.
        store
            .store()
            .save(&record("b@example.com", "sub-b", &["openid"], false))
            .unwrap();
        credentials.forget_api_key().unwrap();
        credentials.forget_api_key().unwrap();
        credentials.switch(declined.id().as_str()).unwrap();
        assert_eq!(credentials.access(), None);
        assert_eq!(credentials.accounts()[1].state, AccountState::SignedOut);
        assert!(credentials.switch("../nope").is_err());
    }

    #[test]
    fn labels_name_the_way_runs_pay() {
        let plan = Access::ChatGpt(AccountId::parse("a").unwrap());
        assert_eq!(plan.short_label("gpt-5.5"), "gpt-5.5 · ChatGPT plan");
        assert_eq!(
            Access::ApiKey("sk".into()).short_label("gpt-5.5"),
            "gpt-5.5 · OpenAI API key"
        );
        assert!(
            !format!("{:?}", Access::ApiKey("sk-1".into())).contains("sk-1")
        );
    }

    #[cfg(unix)]
    #[test]
    fn saved_keys_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let credentials = Credentials::new(dir.path().join("tau"));
        credentials.save_api_key("sk-x").unwrap();
        let mode = std::fs::metadata(credentials.api_key())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
