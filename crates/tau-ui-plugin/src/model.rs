//! The person's model outside any run (ADR 0044): what a plugin's host
//! half asks, such as memory turning what another agent saved into
//! notes. It is the plan's client, and what it spends is the plugin's:
//! booked to it alone, in no run's totals.

use std::sync::{Arc, Mutex};

use tau_agent::plugin::AskError;
use tau_ai::{
    llm::Llm,
    message::{AssistantMessage, Message, Usage},
    responses::request::{ReasoningEffort, Settings, ToolDefinition},
    retry::RetryPolicy,
};
use tau_store::Store;
use tokio_util::sync::CancellationToken;

/// What a host half asks the model with: the model and how hard it
/// reasons, as a plugin's settings pick them from the plan's
/// (`tau_ai::model::plan_models`), and what it is told.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AskSettings {
    /// The model's id, such as `gpt-6-luna`.
    pub model: String,
    /// `None` leaves the effort to the model.
    pub reasoning: Option<ReasoningEffort>,
    pub instructions: Option<String>,
}

impl AskSettings {
    pub fn new(
        model: impl Into<String>,
        reasoning: Option<ReasoningEffort>,
    ) -> Self {
        Self {
            model: model.into(),
            reasoning,
            instructions: None,
        }
    }

    pub fn instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    /// The request's settings, offering `tools`.
    pub fn request(&self, tools: &[ToolDefinition]) -> Settings {
        Settings {
            model: self.model.clone(),
            reasoning: self.reasoning,
            instructions: self.instructions.clone(),
            tools: tools.to_vec(),
            ..Settings::default()
        }
    }
}

type Reach = Arc<dyn Fn() -> Option<Arc<dyn Llm>> + Send + Sync>;

/// The person's model as the host reaches it now: the plan's client
/// while they are signed in, none once they sign out. A host service
/// ([`crate::HostCx::model`]); clones share it.
#[derive(Clone)]
pub struct Model {
    reach: Reach,
    store: Store,
    retry: RetryPolicy,
}

impl Model {
    /// `reach` gives the client to ask now, or none while the person is
    /// signed out; `store` books what each plugin spends.
    pub fn new(
        reach: impl Fn() -> Option<Arc<dyn Llm>> + Send + Sync + 'static,
        store: Store,
    ) -> Self {
        Self {
            reach: Arc::new(reach),
            store,
            retry: RetryPolicy::default(),
        }
    }

    /// Retries failed requests under `retry` instead of the default.
    pub fn retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// Whether there is a model to ask now.
    pub fn signed_in(&self) -> bool {
        (self.reach)().is_some()
    }

    /// The model as `plugin` asks it.
    pub fn of(&self, plugin: &str) -> PluginModel {
        PluginModel {
            model: self.clone(),
            plugin: plugin.into(),
        }
    }
}

impl std::fmt::Debug for Model {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Model")
            .field("signed_in", &self.signed_in())
            .finish_non_exhaustive()
    }
}

/// The person's model as one plugin asks it, outside any run: what it
/// spends is booked to the plugin (`Backend::charge_plugin`), which the
/// Plugins screen's spend counts and the plan's limits see.
#[derive(Clone, Debug)]
pub struct PluginModel {
    model: Model,
    plugin: Arc<str>,
}

impl PluginModel {
    /// The plugin it asks for.
    pub fn plugin(&self) -> &str {
        &self.plugin
    }

    /// Asks the model once: a session of its own, offering `tools`,
    /// retried under the host's policy, until `cancel` is. As
    /// `PluginCtx::ask`, the final response can still be a failed one;
    /// an error means none came, or the person is signed out
    /// ([`AskError::Unavailable`]). Every attempt's usage is booked to
    /// the plugin, whatever came of it.
    pub async fn ask(
        &self,
        settings: &AskSettings,
        input: &[Message],
        tools: &[ToolDefinition],
        cancel: &CancellationToken,
    ) -> Result<AssistantMessage, AskError> {
        let Some(llm) = (self.model.reach)() else {
            return Err(AskError::Unavailable(
                "the person is not signed in".to_owned(),
            ));
        };
        let spent = Mutex::new(Usage::default());
        let charge = |usage: &Usage| {
            *spent.lock().expect("not poisoned") += usage;
        };
        let answer = tau_agent::plugin::ask_outside(
            llm.as_ref(),
            settings.request(tools),
            input,
            self.model.retry,
            cancel,
            &charge,
        )
        .await;
        let spent = spent.into_inner().expect("not poisoned");
        if spent != Usage::default() {
            let usage = tau_agent::runner::turn_usage(&spent, 0);
            if let Err(error) =
                self.model.store.charge_plugin(&self.plugin, usage).await
            {
                eprintln!(
                    "{}: what it asked the model is not booked: {error}",
                    self.plugin
                );
            }
        }
        answer
    }
}
