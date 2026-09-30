//! The OpenAI client: one WebSocket transport per process, and one
//! session per run.
//!
//! A [`Session`] holds a run's fixed [`Settings`] and its lane. Each call
//! to [`Session::respond`] builds the full request from the transcript;
//! the lane decides whether it goes out as a delta. When a response
//! completes, its usage gets its cost from the model table.

use std::sync::Arc;

use crate::{
    chatgpt::{AccountId, ChatGpt, ChatGptConnector, UNSUPPORTED_FIELDS},
    cost::apply,
    event::AssistantEvent,
    http::Dialer,
    message::{Message, Timestamp},
    model::{Model, ServiceTier, find},
    refusal::Refusal,
    responses::{
        input::InputCache,
        request::{ReasoningEffort, Settings, fields},
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{API_KEY_VAR} is not set")]
pub struct MissingApiKey;

/// A client for OpenAI's Responses WebSocket mode. Clones share one
/// connection pool. Must be created inside a tokio runtime.
#[derive(Debug, Clone)]
pub struct OpenAi {
    transport: Transport,
    endpoint: Endpoint,
}

/// Which Responses endpoint a client talks to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endpoint {
    /// `api.openai.com`, with an API key.
    Api,
    /// `api.openai.com`, with a ChatGPT sign-in that uses the plan: the
    /// same protocol, less [`UNSUPPORTED_FIELDS`].
    ChatGptPlan,
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

    /// A client that uses `account`'s ChatGPT plan (see
    /// [`crate::chatgpt`]). Its token is refreshed before each connection
    /// when it nears expiry; a sign-in without plan usage connects
    /// nothing.
    pub fn chatgpt<D: Dialer>(chatgpt: ChatGpt<D>, account: AccountId) -> Self {
        Self::with_connector(
            ChatGptConnector::new(chatgpt, account),
            single_lane_limits(),
        )
        .endpoint(Endpoint::ChatGptPlan)
    }

    /// A client over any connector, such as a simulated network in tests.
    pub fn with_connector<C: Connector>(connector: C, limits: Limits) -> Self {
        Self {
            transport: Transport::start(connector, limits),
            endpoint: Endpoint::Api,
        }
    }

    /// Adapts sessions to `endpoint`'s requirements. [`Self::chatgpt`]
    /// sets it; a test connector to a plan double needs it too.
    pub fn endpoint(mut self, endpoint: Endpoint) -> Self {
        self.endpoint = endpoint;
        self
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
        if self.endpoint == Endpoint::ChatGptPlan {
            settings.max_output_tokens = None;
        }
        Ok(Session {
            lane: self.transport.open_lane().await?,
            fields: Arc::new(session_fields(&settings, self.endpoint)),
            settings,
            model,
            input: InputCache::new(),
            endpoint: self.endpoint,
        })
    }

    /// The pool's counters, including every session's requests.
    pub async fn stats(&self) -> Result<PoolStats, Stopped> {
        self.transport.stats().await
    }

    /// Why the latest request or connection was refused, if no response
    /// has completed since (see [`crate::refusal`]). A run that stopped on
    /// a usage limit or a dead sign-in leaves it here for the interface.
    pub fn refusal(&self) -> Option<Refusal> {
        self.transport.refusal()
    }
}

/// One response stream per connection, for an endpoint that does not
/// take `stream_id`: lanes cannot share a connection.
pub fn single_lane_limits() -> Limits {
    Limits {
        max_lanes: 1,
        max_in_flight: 1,
        ..Limits::default()
    }
}

/// The request fields of a session on `endpoint`: on the plan route,
/// without the fields it does not take, whatever the settings asked for.
fn session_fields(settings: &Settings, endpoint: Endpoint) -> Fields {
    let mut fields = fields(settings, None);
    if endpoint == Endpoint::ChatGptPlan {
        for name in UNSUPPORTED_FIELDS {
            fields.remove(name);
        }
    }
    fields
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
    endpoint: Endpoint,
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

    /// Sets the reasoning effort of the requests that follow. The fields
    /// are built again only when it changes, so requests at one effort
    /// still share them.
    pub fn set_reasoning(&mut self, effort: Option<ReasoningEffort>) {
        if self.settings.reasoning == effort {
            return;
        }
        self.settings.reasoning = effort;
        self.fields = Arc::new(session_fields(&self.settings, self.endpoint));
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
