//! How tau reaches a model: a ChatGPT plan through Sign in with ChatGPT
//! (`tau_ai::chatgpt`), the only way; where sign-ins are kept; and
//! signing in, switching accounts and signing out. Only what the user set
//! up in tau counts; the environment is never read.
//!
//! Runs use the active ChatGPT account's plan when it is signed in with
//! plan usage, and nothing otherwise. A plan error stops the run and says
//! what to do.

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
    models::{AccountState, ChatGptAccount, DEFAULT_MODEL},
    setup::{ModelAccess, SetupStep, SetupUpdate},
    update::HostUpdate,
    workspace::{Workspace, WorkspaceEvent},
};

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

    pub fn github(&self) -> PathBuf {
        self.dir.join("github.json")
    }

    /// Where the TypeSafe key for Jev is kept.
    pub fn jev(&self) -> PathBuf {
        self.dir.join("typesafe-key")
    }

    /// The saved TypeSafe key, which tau-constitution checks with.
    pub async fn jev_key(&self) -> Option<String> {
        tokio::fs::read_to_string(self.jev())
            .await
            .ok()
            .map(|key| key.trim().to_owned())
            .filter(|key| !key.is_empty())
    }

    /// Saves the TypeSafe key, or forgets it with `None`.
    pub async fn set_jev_key(&self, key: Option<&str>) -> io::Result<()> {
        match key.map(str::trim).filter(|key| !key.is_empty()) {
            Some(key) => {
                tau_ai::files::write_private_async(&self.jev(), key.as_bytes())
                    .await
            }
            None => remove(&self.jev()).await,
        }
    }

    /// What runs use: the active ChatGPT account, when it is signed in
    /// with plan usage.
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

async fn remove(path: &Path) -> io::Result<()> {
    match tokio::fs::remove_file(path).await {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// Called with the account runs use now, after a sign-in, a switch or a
/// sign-out: `None` when none may be used.
pub type Connected = Rc<dyn Fn(Option<AccountId>, &mut App)>;

/// A ChatGPT sign-in waiting for the browser; dropping it cancels it.
struct Pending {
    /// Takes a redirect URL pasted instead, once.
    paste: Option<oneshot::Sender<String>>,
    _cancel: oneshot::Sender<()>,
}

/// Carries out the model sign-ins the workspace asks for: ChatGPT in the
/// browser (or a pasted redirect), and switching and signing out of
/// ChatGPT accounts. Reports progress to the workspace's setup,
/// and calls `connected` once what runs use changed. One per workspace:
/// it holds the sign-in in progress.
#[derive(Default)]
pub struct SignIns {
    pending: Rc<RefCell<Option<Pending>>>,
}

/// What a sign-in that was cancelled ends with.
const CANCELLED: &str = "sign-in cancelled";

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
            WorkspaceEvent::ChatGptCancel => {
                // Dropping it stops its listener.
                self.pending.borrow_mut().take();
                true
            }
            WorkspaceEvent::SwitchChatGpt { account } => {
                match credentials.switch(account) {
                    Ok(()) => connected(credentials.plan_account(), cx),
                    Err(error) => workspace.update(cx, |ws, cx| {
                        ws.apply(
                            HostUpdate::alert(
                                "Could not switch accounts",
                                error,
                            ),
                            cx,
                        )
                    }),
                }
                true
            }
            WorkspaceEvent::SignOut => {
                sign_out(workspace, credentials, connected, cx);
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
        crate::interface_runtime::spawn(async move {
            let signed_in = async {
                let chatgpt = chatgpt.map_err(|e| e.to_string())?;
                tokio::select! {
                    done = browser_sign_in(
                        &chatgpt, account, consent, pasted, &progress,
                    ) => done,
                    _ = cancelled => Err(CANCELLED.into()),
                }
            }
            .await;
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
                            ws.apply(
                                HostUpdate::Setup(SetupUpdate::Model(pending)),
                                cx,
                            )
                        });
                    }
                    // A cancelled attempt, or one a newer sign-in
                    // replaced, has nothing to say.
                    Progress::SignedIn(Err(error)) if error == CANCELLED => {}
                    Progress::SignedIn(done) => {
                        pending.borrow_mut().take();
                        let setup = match done {
                            Ok(signed_in) => {
                                connected(credentials.plan_account(), cx);
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
                            ws.apply(
                                HostUpdate::Setup(SetupUpdate::Model(setup)),
                                cx,
                            )
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
    let finished = crate::interface_runtime::spawn(async move {
        let chatgpt = chatgpt.map_err(|e| e.to_string())?;
        chatgpt.sign_out(&account).await.map_err(|e| e.to_string())
    });
    let workspace = workspace.downgrade();
    let (credentials, connected) = (credentials.clone(), connected.clone());
    cx.spawn(async move |cx| {
        let Ok(revoked) = finished.await else { return };
        cx.update(|cx| {
            let left = credentials.plan_account();
            connected(left.clone(), cx);
            let Some(workspace) = workspace.upgrade() else {
                return;
            };
            workspace.update(cx, |ws, cx| {
                match revoked {
                    Ok(Revocation::Unconfirmed { reason }) => ws.apply(HostUpdate::alert("Signed out of ChatGPT", format!(
                            "tau forgot this account's tokens, but OpenAI did \
                             not confirm the end of the session ({reason}). \
                             You can disconnect tau in ChatGPT Settings."
                        )), cx),
                    Ok(_) => {}
                    Err(error) => {
                        ws.apply(HostUpdate::alert("Could not sign out", error), cx)
                    }
                }
                if left.is_none() {
                    // Nothing left to run on: set it up again.
                    ws.apply(HostUpdate::Setup(SetupUpdate::Model(ModelAccess::None)), cx);
                    ws.start_setup(SetupStep::Model, cx);
                }
            });
        });
    })
    .detach();
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
        let accounts = credentials.accounts();
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0].state, AccountState::Plan);
        assert!(!accounts[0].active);
        assert_eq!(accounts[1].state, AccountState::PlanDisabled);
        assert!(accounts[1].active);

        // Signed out: nothing.
        store
            .store()
            .save(&record("b@example.com", "sub-b", &["openid"], false))
            .unwrap();
        assert_eq!(credentials.plan_account(), None);
        assert_eq!(credentials.accounts()[1].state, AccountState::SignedOut);
        assert!(credentials.switch("../nope").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn saved_keys_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let credentials = Credentials::new(dir.path().join("tau"));
        tau_testing::block_on_io(credentials.set_jev_key(Some("ts-x")))
            .unwrap();
        let mode = std::fs::metadata(credentials.jev())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
