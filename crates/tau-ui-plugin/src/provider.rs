//! The `tau.provider` slot (ADR 0045): what runs reach models through,
//! the models the picker offers, and why the provider last refused. The
//! kernel asks the slot each time a run opens its session
//! ([`ProviderLlm`]), so a sign-in or a pick counts for every run that
//! starts after it, and runs going on keep theirs.

use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use tau_ai::{
    llm::{Llm, LlmError, LlmSession},
    refusal::Refusal,
    responses::request::Settings,
    ws::io::driver::CacheLapse,
};

use crate::slots::{Fills, Slot};

/// The slot.
pub const PROVIDER: Slot<dyn Provider> = Slot::new("tau.provider");

/// One model the picker can offer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelOption {
    pub id: String,
    /// What the picker shows: the model table's name for it; `None`
    /// shows the id.
    pub name: Option<String>,
    pub context: u64,
    /// Whether it accepts a reasoning effort.
    pub reasoning: bool,
}

impl ModelOption {
    /// What the picker shows for it.
    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }

    /// `1.05M`, `400k`.
    pub fn context_label(&self) -> String {
        if self.context >= 1_000_000 {
            let millions = self.context as f64 / 1e6;
            let text = format!("{millions:.2}");
            format!("{}M", text.trim_end_matches('0').trim_end_matches('.'))
        } else {
            format!("{}k", self.context / 1000)
        }
    }
}

/// When a conversation's prompt cache lapses, as the provider knows it.
pub type Lapse<'a> =
    Pin<Box<dyn Future<Output = Option<CacheLapse>> + Send + 'a>>;

/// What runs reach models through.
pub trait Provider: Send + Sync {
    /// What runs use, as the picker's footer says it: `ChatGPT plan`.
    /// None while there is nothing to run on, such as signed out.
    fn label(&self) -> Option<String>;

    /// The client runs open their sessions with now; none while there is
    /// nothing to run on.
    fn llm(&self) -> Option<Arc<dyn Llm>>;

    /// The models the picker offers now: none while signed out.
    fn models(&self) -> Vec<ModelOption>;

    /// Why the provider last refused a run, while runs use it: a usage
    /// limit, a sign-in it no longer takes, a restriction.
    fn refusal(&self) -> Option<Refusal>;

    /// When the prompt cache of the conversation `path` lapses, when the
    /// provider can tell: what idle compaction waits for.
    fn cache_lapse<'a>(&'a self, path: &'a str) -> Lapse<'a>;
}

/// A client over whatever fills `tau.provider` as each session opens: the
/// one the host builds its agent with.
#[derive(Clone)]
pub struct ProviderLlm {
    slots: Arc<Mutex<Fills>>,
}

impl ProviderLlm {
    pub fn new(slots: Arc<Mutex<Fills>>) -> Self {
        Self { slots }
    }

    fn now(&self) -> Option<Arc<dyn Llm>> {
        self.slots
            .lock()
            .expect("not poisoned")
            .get(&PROVIDER)?
            .llm()
    }
}

impl Llm for ProviderLlm {
    fn supports_output_token_limit(&self) -> bool {
        self.now()
            .is_some_and(|llm| llm.supports_output_token_limit())
    }

    fn open(
        &self,
        settings: Settings,
    ) -> BoxFuture<'static, Result<Box<dyn LlmSession>, LlmError>> {
        match self.now() {
            Some(llm) => llm.open(settings),
            None => Box::pin(std::future::ready(Err(LlmError {
                message: "tau has no model to run on: sign in on the Models \
                          screen"
                    .to_owned(),
            }))),
        }
    }
}
