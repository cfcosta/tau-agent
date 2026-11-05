//! tau-chatgpt's host half: the plan's client, from the saved sign-ins,
//! fills `tau.provider`.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use serde_json::Value;
use tau_agent::plugin::Plugin;
use tau_ai::{
    chatgpt::{
        AccountId,
        AccountStatus,
        ChatGpt,
        ChatGptError,
        Loopback,
        PlanUsage,
        Revocation,
        SignedIn,
        Store,
    },
    client::OpenAi,
    llm::Llm,
};
use tau_ui_plugin::{
    Group,
    HostCx,
    HostHalf,
    PluginHost,
    PluginInfo,
    RunCtx,
    provider::{Lapse, ModelOption, PROVIDER, Provider, Stopped},
    slots::Fills,
};
use tokio::sync::oneshot;

use crate::{
    LABEL,
    plan_account,
    ui::{Account, AccountState, Act, ChatGptUi, Data, SignIn},
};

/// The host half for tau's registry.
#[derive(Debug, Clone, Copy, Default)]
pub struct ChatGptHost;

/// What the host half keeps: the provider it fills the slot with, and
/// how signing in stands.
pub struct Host {
    provider: Arc<Plan>,
    cx: HostCx,
    sign_in: Arc<Mutex<SignIn>>,
    /// The sign-in waiting for the browser; dropping it cancels it.
    pending: Arc<Mutex<Option<Pending>>>,
}

/// A sign-in waiting for the browser.
struct Pending {
    /// Takes a redirect pasted instead, once.
    paste: Option<oneshot::Sender<String>>,
    _cancel: oneshot::Sender<()>,
}

/// What a cancelled sign-in ends with.
const CANCELLED: &str = "sign-in cancelled";

impl PluginHost for Host {
    async fn new(cx: &HostCx) -> anyhow::Result<Self> {
        Ok(Self {
            provider: Arc::new(Plan {
                config: cx.config_dir().map(ToOwned::to_owned),
                runtime: cx.runtime.clone(),
                client: Mutex::new(None),
            }),
            cx: cx.clone(),
            sign_in: Arc::default(),
            pending: Arc::default(),
        })
    }
}

impl Host {
    /// The sign-ins, made on first use.
    fn chatgpt(&self) -> Result<ChatGpt, String> {
        let config = self
            .provider
            .config
            .as_ref()
            .ok_or("this host keeps no sign-ins")?;
        let store =
            Store::open(config.join("chatgpt")).map_err(|e| e.to_string())?;
        Ok(ChatGpt::new(store))
    }

    /// The saved sign-ins, if the store was made: reading never makes it.
    fn saved(&self) -> Option<Store> {
        let dir = self.provider.config.as_ref()?.join("chatgpt");
        dir.is_dir().then(|| Store::open(&dir).ok()).flatten()
    }

    fn accounts(&self) -> Vec<Account> {
        let Some(store) = self.saved() else {
            return Vec::new();
        };
        let active = store.active().ok().flatten();
        store
            .accounts()
            .unwrap_or_default()
            .into_iter()
            .map(|saved| {
                let id = saved.id();
                Account {
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

    fn set(&self, sign_in: SignIn) {
        *self.sign_in.lock().expect("not poisoned") = sign_in;
        self.cx.refresh();
    }

    /// Signs in in the browser: `account` again, or a new sign-in;
    /// `consent` asks again for plan usage. The browser opens on the
    /// computer at the authorization page; the loopback listener, or a
    /// pasted redirect, finishes it.
    async fn sign_in(
        &self,
        account: Option<AccountId>,
        consent: bool,
    ) -> Result<(), String> {
        let chatgpt = self.chatgpt()?;
        // The listener starts before the browser opens.
        let loopback = Loopback::bind().await.map_err(|e| e.to_string())?;
        let attempt = chatgpt
            .start_sign_in(account.as_ref(), loopback.redirect_uri(), consent)
            .map_err(|e| e.to_string())?;
        let (paste, pasted) = oneshot::channel::<String>();
        let (cancel, cancelled) = oneshot::channel::<()>();
        // A new sign-in replaces one still waiting.
        *self.pending.lock().expect("not poisoned") = Some(Pending {
            paste: Some(paste),
            _cancel: cancel,
        });
        let url = attempt.url().to_owned();
        self.cx.open_url(url.clone());
        self.set(SignIn::Waiting { url });
        let (sign_in, pending, cx, provider) = (
            self.sign_in.clone(),
            self.pending.clone(),
            self.cx.clone(),
            self.provider.clone(),
        );
        self.cx.runtime.spawn(async move {
            let done: Result<SignedIn, String> = async {
                let callback = tokio::select! {
                    callback = loopback.wait(&attempt) => callback,
                    Ok(url) = pasted => attempt.callback(&url),
                    _ = cancelled => return Err(CANCELLED.to_owned()),
                };
                let callback = callback.map_err(|e| e.to_string())?;
                chatgpt
                    .finish_sign_in(&attempt, &callback)
                    .await
                    .map_err(|e| e.to_string())
            }
            .await;
            let state = match done {
                // A cancelled attempt, or one a newer sign-in replaced,
                // has nothing to say.
                Err(error) if error == CANCELLED => return,
                Err(error) => SignIn::Failed { error },
                Ok(signed_in) => match signed_in.plan_usage {
                    PlanUsage::Disabled => SignIn::PlanDisabled {
                        account: signed_in.label,
                    },
                    PlanUsage::Enabled => {
                        match eligibility(&chatgpt, &signed_in.account).await {
                            Some(detail) => SignIn::NotEligible {
                                account: signed_in.label,
                                detail,
                            },
                            None => SignIn::Connected {
                                account: signed_in.label,
                            },
                        }
                    }
                },
            };
            pending.lock().expect("not poisoned").take();
            // The provider reads the new sign-in as the next run opens.
            provider.client.lock().expect("not poisoned").take();
            *sign_in.lock().expect("not poisoned") = state;
            cx.refresh();
        });
        Ok(())
    }

    /// Signs the active account out: revokes its session and forgets its
    /// tokens, and says so when OpenAI did not confirm.
    async fn sign_out(&self) -> Result<(), String> {
        let chatgpt = self.chatgpt()?;
        let Some(account) = chatgpt.store().active().ok().flatten() else {
            return Ok(());
        };
        let revoked = chatgpt
            .sign_out(&account)
            .await
            .map_err(|e| e.to_string())?;
        if let Revocation::Unconfirmed { reason } = revoked {
            self.cx.alert(
                "Signed out of ChatGPT",
                format!(
                    "tau forgot this account's tokens, but OpenAI did not \
                     confirm the end of the session ({reason}). You can \
                     disconnect tau in ChatGPT Settings."
                ),
            );
        }
        self.set(SignIn::None);
        Ok(())
    }
}

/// Why OpenAI says plan use is not available to `account`, when it says
/// so (`Recovery::Restricted`): one `GET /v1/models`, whose listing is
/// not shown. `403 subscription_sharing_user_not_eligible · request
/// req_…`. None for any other answer.
async fn eligibility(chatgpt: &ChatGpt, account: &AccountId) -> Option<String> {
    let error = chatgpt.models(account).await.err()?;
    not_eligible(&error)
}

/// The refusal's words when `error` says plan use is not available to
/// the account. None for any other failure.
pub fn not_eligible(error: &ChatGptError) -> Option<String> {
    if error.recovery() != tau_ai::retry::Recovery::Restricted {
        return None;
    }
    Some(match error {
        ChatGptError::Api(api) => {
            let mut detail = api.status.to_string();
            if let Some(code) = api.code() {
                detail.push_str(&format!(" {code}"));
            }
            if let Some(id) = &api.request_id {
                detail.push_str(&format!(" · request {id}"));
            }
            detail
        }
        other => other.to_string(),
    })
}

impl HostHalf for ChatGptHost {
    type Plugin = ChatGptUi;
    type Host = Host;

    async fn agent_plugins(
        &self,
        _: &Host,
        _: &RunCtx,
        _: &(),
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        Ok(Vec::new())
    }

    async fn catalog(&self, host: &Host, _: &HostCx, _: &()) -> PluginInfo {
        PluginInfo {
            description: if host.provider.account().is_some() {
                "Runs agents on your ChatGPT plan"
            } else {
                "Runs agents on your ChatGPT plan: sign in on the Models \
                 screen"
            }
            .into(),
            group: Group::App,
            ..Default::default()
        }
    }

    fn fill(&self, host: &Host, fills: &mut Fills) {
        fills.fill(&PROVIDER, host.provider.clone() as Arc<dyn Provider>);
    }

    async fn data(&self, host: &Host, _: &HostCx) -> Data {
        let mut sign_in = host.sign_in.lock().expect("not poisoned").clone();
        // As the host starts, or after a switch: what the store says.
        if matches!(sign_in, SignIn::None)
            && let Some(account) = host.provider.account()
        {
            let label = host
                .accounts()
                .into_iter()
                .find(|saved| saved.id == account.to_string())
                .map_or_else(|| account.to_string(), |saved| saved.label);
            sign_in = SignIn::Connected { account: label };
        }
        Data {
            sign_in,
            accounts: host.accounts(),
        }
    }

    async fn act(
        &self,
        host: &Host,
        action: Value,
        cx: &HostCx,
    ) -> anyhow::Result<Option<Value>> {
        match serde_json::from_value(action)? {
            Act::SignIn { account, consent } => {
                let account = account.as_deref().and_then(AccountId::parse);
                if let Err(error) = host.sign_in(account, consent).await {
                    host.set(SignIn::Failed { error });
                }
            }
            Act::Callback { url } => {
                let paste = host
                    .pending
                    .lock()
                    .expect("not poisoned")
                    .as_mut()
                    .and_then(|pending| pending.paste.take());
                if let Some(paste) = paste {
                    let _ = paste.send(url);
                }
            }
            Act::Cancel => {
                // Dropping it stops its listener.
                host.pending.lock().expect("not poisoned").take();
                host.set(SignIn::None);
            }
            Act::Switch { account } => {
                let id = AccountId::parse(&account).ok_or_else(|| {
                    anyhow::anyhow!("{account:?} is not an account id")
                })?;
                let chatgpt =
                    host.chatgpt().map_err(|error| anyhow::anyhow!(error))?;
                chatgpt.store().set_active(&id)?;
                host.set(SignIn::None);
            }
            Act::SignOut => {
                if let Err(error) = host.sign_out().await {
                    cx.alert("Could not sign out", error);
                }
            }
        }
        cx.refresh();
        Ok(None)
    }
}

/// The plan of the active sign-in, read as each run opens: a sign-in, a
/// switch or a sign-out counts for the runs after it.
struct Plan {
    /// tau's config directory; none in a host that says none.
    config: Option<PathBuf>,
    runtime: tokio::runtime::Handle,
    /// The client of the account last used, kept while it stays active.
    client: Mutex<Option<(AccountId, OpenAi)>>,
}

impl Plan {
    fn account(&self) -> Option<AccountId> {
        plan_account(self.config.as_deref()?)
    }

    /// The client of the active account with plan usage, if any.
    fn client(&self) -> Option<OpenAi> {
        let account = self.account()?;
        let mut kept = self.client.lock().expect("not poisoned");
        if let Some((id, client)) = kept.as_ref()
            && *id == account
        {
            return Some(client.clone());
        }
        let store = Store::open(self.config.as_ref()?.join("chatgpt")).ok()?;
        // Clients are made inside the runtime they run on.
        let _guard = self.runtime.enter();
        let client = OpenAi::chatgpt(ChatGpt::new(store), account.clone());
        *kept = Some((account, client.clone()));
        Some(client)
    }
}

impl Provider for Plan {
    fn label(&self) -> Option<String> {
        self.account().map(|_| LABEL.to_owned())
    }

    fn llm(&self) -> Option<Arc<dyn Llm>> {
        Some(Arc::new(self.client()?))
    }

    fn models(&self) -> Vec<ModelOption> {
        if self.account().is_none() {
            return Vec::new();
        }
        plan_models()
    }

    fn stopped(&self) -> Option<Stopped> {
        crate::plan_usage::stopped(&self.client()?.refusal()?)
    }

    fn cache_lapse<'a>(&'a self, path: &'a str) -> Lapse<'a> {
        Box::pin(async move { self.client()?.cache_lapse(path).await.ok()? })
    }
}

/// A provider over a client given it, signed in whatever the saved
/// sign-ins say: for tests that answer from a fake server.
pub struct Over(OpenAi);

impl Over {
    pub fn new(client: OpenAi) -> Self {
        Self(client)
    }
}

impl Provider for Over {
    fn label(&self) -> Option<String> {
        Some(LABEL.to_owned())
    }

    fn llm(&self) -> Option<Arc<dyn Llm>> {
        Some(Arc::new(self.0.clone()))
    }

    fn models(&self) -> Vec<ModelOption> {
        plan_models()
    }

    fn stopped(&self) -> Option<Stopped> {
        crate::plan_usage::stopped(&self.0.refusal()?)
    }

    fn cache_lapse<'a>(&'a self, path: &'a str) -> Lapse<'a> {
        Box::pin(async move { self.0.cache_lapse(path).await.ok()? })
    }
}

/// The plan's models, from tau-ai's model table, one per family, in its
/// order and named as it names them. Not the account's `GET /v1/models`:
/// that catalog leaves out models the plan runs.
pub fn plan_models() -> Vec<ModelOption> {
    tau_ai::model::plan_models()
        .into_iter()
        .map(|model| ModelOption {
            id: model.id.clone(),
            name: Some(model.name.clone()),
            context: model.context_window,
            reasoning: model.reasoning,
            efforts: model.efforts.clone(),
        })
        .collect()
}
