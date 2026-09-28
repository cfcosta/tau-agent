//! The OpenAI client: one WebSocket transport per process, and one
//! session per run.
//!
//! A [`Session`] holds a run's fixed [`Settings`] and its lane. Each call
//! to [`Session::respond`] builds the full request from the transcript;
//! the lane decides whether it goes out as a delta. When a response
//! completes, its usage gets its cost from the model table.

use std::sync::Arc;

use crate::{
    cost::apply,
    event::AssistantEvent,
    message::{Message, Timestamp},
    model::{Model, ServiceTier, find},
    responses::{
        input::InputCache,
        request::{Settings, fields},
    },
    ws::{
        io::{
            connection::Connector,
            driver::{LaneHandle, Response, Stopped, Transport},
            tls::OpenAiConnector,
        },
        proto::{
            continuation::{Body, Fields},
            pool::{Limits, PoolStats},
        },
    },
};

/// The environment variable [`OpenAi::from_env`] reads.
pub const API_KEY_VAR: &str = "OPENAI_API_KEY";

/// `OPENAI_API_KEY` is not set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MissingApiKey;

impl std::fmt::Display for MissingApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{API_KEY_VAR} is not set")
    }
}

impl std::error::Error for MissingApiKey {}

/// A client for OpenAI's Responses WebSocket mode. Clones share one
/// connection pool. Must be created inside a tokio runtime.
#[derive(Debug, Clone)]
pub struct OpenAi {
    transport: Transport,
}

impl OpenAi {
    /// A client that authenticates with `api_key`.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::with_connector(OpenAiConnector::new(api_key), Limits::default())
    }

    /// A client that reads its key from `OPENAI_API_KEY`.
    pub fn from_env() -> Result<Self, MissingApiKey> {
        let key = std::env::var(API_KEY_VAR)
            .ok()
            .filter(|k| !k.is_empty())
            .ok_or(MissingApiKey)?;
        Ok(Self::new(key))
    }

    /// A client over any connector, such as a simulated network in tests.
    pub fn with_connector<C: Connector>(connector: C, limits: Limits) -> Self {
        Self {
            transport: Transport::start(connector, limits),
        }
    }

    /// Opens a session for one run. If the model is in the model table,
    /// whether it reasons comes from there.
    pub async fn session(
        &self,
        mut settings: Settings,
    ) -> Result<Session, Stopped> {
        let model = find(&settings.model);
        if let Some(model) = model {
            settings.reasoning_model = model.reasoning;
        }
        Ok(Session {
            lane: self.transport.open_lane().await?,
            fields: Arc::new(fields(&settings, None)),
            settings,
            model,
            input: InputCache::new(),
        })
    }

    /// The pool's counters, including every session's requests.
    pub async fn stats(&self) -> Result<PoolStats, Stopped> {
        self.transport.stats().await
    }
}

/// One run's conversation with the model.
#[derive(Debug)]
pub struct Session {
    lane: LaneHandle,
    settings: Settings,
    /// The request fields, built once: every request of the session
    /// shares them.
    fields: Arc<Fields>,
    model: Option<&'static Model>,
    input: InputCache,
}

impl Session {
    /// Asks for the next response to `transcript`. `timestamp` becomes
    /// the response's timestamp.
    pub fn respond(
        &mut self,
        transcript: &[Message],
        timestamp: Timestamp,
    ) -> SessionResponse {
        let request =
            Body::new(self.fields.clone(), self.input.input(transcript));
        SessionResponse {
            response: self.lane.request(
                request,
                self.settings.model.clone(),
                timestamp,
            ),
            model: self.model,
            tier: service_tier(self.settings.service_tier.as_deref()),
        }
    }

    /// A warm-up (`docs/reference/openai-websocket.md`, "Warm-up"): the
    /// session's instructions and tools with no input and `generate:
    /// false`. It produces no output; the lane's next request continues
    /// from it.
    pub fn warm_up(&self, timestamp: Timestamp) -> SessionResponse {
        let mut fields = (*self.fields).clone();
        fields.insert("generate".into(), false.into());
        let request = Body::new(Arc::new(fields), Vec::new());
        SessionResponse {
            response: self.lane.request(
                request,
                self.settings.model.clone(),
                timestamp,
            ),
            model: self.model,
            tier: service_tier(self.settings.service_tier.as_deref()),
        }
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }
}

/// The events of one response, with cost filled in on completion.
#[derive(Debug)]
pub struct SessionResponse {
    response: Response,
    model: Option<&'static Model>,
    tier: ServiceTier,
}

impl SessionResponse {
    /// The next event; `None` after the terminal event. A `Done` event's
    /// usage carries its cost when the model is in the model table.
    pub async fn next(&mut self) -> Option<AssistantEvent> {
        let mut event = self.response.next().await?;
        if let (AssistantEvent::Done { usage, .. }, Some(model)) =
            (&mut event, self.model)
        {
            apply(model, usage, self.tier);
        }
        Some(event)
    }
}

fn service_tier(tier: Option<&str>) -> ServiceTier {
    match tier {
        Some("flex") => ServiceTier::Flex,
        Some("priority" | "fast") => ServiceTier::PriorityOrFast,
        _ => ServiceTier::Default,
    }
}
