//! tau-chatgpt's host half: the plan's client, from the saved sign-ins,
//! fills `tau.provider`.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use tau_agent::plugin::Plugin;
use tau_ai::{
    chatgpt::{AccountId, ChatGpt, Store},
    client::OpenAi,
    llm::Llm,
    refusal::Refusal,
};
use tau_ui_plugin::{
    Group,
    HostCx,
    HostHalf,
    PluginHost,
    PluginInfo,
    RunCtx,
    provider::{Lapse, ModelOption, PROVIDER, Provider},
    slots::Fills,
};

use crate::{ChatGptUi, LABEL, plan_account};

/// The host half for tau's registry.
#[derive(Debug, Clone, Copy, Default)]
pub struct ChatGptHost;

/// What the host half keeps: the provider it fills the slot with.
pub struct Host {
    provider: Arc<Plan>,
}

impl PluginHost for Host {
    async fn new(cx: &HostCx) -> anyhow::Result<Self> {
        Ok(Self {
            provider: Arc::new(Plan {
                config: cx.config_dir().map(ToOwned::to_owned),
                runtime: cx.runtime.clone(),
                client: Mutex::new(None),
            }),
        })
    }
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

    fn refusal(&self) -> Option<Refusal> {
        self.client()?.refusal()
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

    fn refusal(&self) -> Option<Refusal> {
        self.0.refusal()
    }

    fn cache_lapse<'a>(&'a self, path: &'a str) -> Lapse<'a> {
        Box::pin(async move { self.0.cache_lapse(path).await.ok()? })
    }
}

/// The plan's models, from tau-ai's model table, one per family, in its
/// order and named as it names them. Not the account's `GET /v1/models`:
/// that catalog leaves out models the plan runs.
fn plan_models() -> Vec<ModelOption> {
    tau_ai::model::plan_models()
        .into_iter()
        .map(|model| ModelOption {
            id: model.id.clone(),
            name: Some(model.name.clone()),
            context: model.context_window,
            reasoning: model.reasoning,
        })
        .collect()
}
