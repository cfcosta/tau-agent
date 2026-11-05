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
    responses::request::{ReasoningEffort, Settings},
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
    /// The efforts it takes, lowest first: none when it does not reason.
    #[serde(default)]
    pub efforts: Vec<ReasoningEffort>,
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

/// What the interface says once the provider stopped a run, and what it
/// offers to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stopped {
    pub title: String,
    pub message: String,
    /// The button that does something about it, with its label.
    pub action: Option<(String, StopAction)>,
}

/// What a [`Stopped`]'s button does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopAction {
    /// Opens a page in the person's browser: the plan's usage.
    Open(String),
    /// Opens the Models screen, where the provider's section signs in.
    Models,
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

    /// Why the provider last stopped a run, in its words, while runs
    /// use it: a usage limit, a sign-in it no longer takes, a
    /// restriction. None for nothing, or a temporary refusal the run
    /// already retried.
    fn stopped(&self) -> Option<Stopped>;

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
