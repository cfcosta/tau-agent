//! The OpenAI client: one WebSocket transport per process, and one
//! session per run.
//!
//! Every client spends a ChatGPT plan ([`OpenAi::chatgpt`]): tau reaches
//! OpenAI only through Sign in with ChatGPT
//! (`docs/decisions/0012-chatgpt-sign-in-only.md`). A [`Session`] holds a
//! run's fixed [`Settings`] and its lane. Each call to
//! [`Session::respond`] builds the full request from the transcript; the
//! lane decides whether it goes out as a delta. When a response
//! completes, its usage gets its cost from the model table: the plan's
//! equivalent at API prices.

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
        },
        proto::{
            continuation::{Body, Fields},
            pool::{Affinity, Limits, PoolStats},
        },
    },
};

/// A client for OpenAI's Responses WebSocket mode on a ChatGPT plan.
/// Clones share one connection pool. Must be created inside a tokio
/// runtime.
#[derive(Debug, Clone)]
pub struct OpenAi {
    transport: Transport,
}

impl OpenAi {
    /// A client that uses `account`'s ChatGPT plan (see
    /// [`crate::chatgpt`]). Its token is refreshed before each connection
    /// when it nears expiry; a sign-in without plan usage connects
    /// nothing.
    pub fn chatgpt<D: Dialer>(chatgpt: ChatGpt<D>, account: AccountId) -> Self {
        Self::with_connector(
            ChatGptConnector::new(chatgpt, account),
            Limits::default(),
        )
    }

    /// A client over any connector, such as a simulated network in tests.
    /// Its requests follow the plan route's rules all the same.
    pub fn with_connector<C: Connector>(connector: C, limits: Limits) -> Self {
        Self {
            transport: Transport::start(connector, limits),
        }
    }

    /// Opens a session for one run. If the model is in the model table,
    /// whether it reasons comes from there. The session's lane goes by
    /// the settings' [`Lineage`](crate::responses::request::Lineage):
    /// onto a connection that already serves the run's conversation, or
    /// for a fork, its parent's (see [`crate::ws::proto::pool`]).
    pub async fn session(
        &self,
        mut settings: Settings,
    ) -> Result<Session, Stopped> {
        let model = find(&settings.model);
        if let Some(model) = model {
            settings.reasoning_model = model.reasoning;
        }
        let affinity = match &settings.lineage {
            Some(lineage) => Affinity::new(
                lineage.path.as_str(),
                lineage.parent.as_deref().map(Into::into),
            ),
            None => Affinity::default(),
        };
        Ok(Session {
            lane: self.transport.open_lane(affinity).await?,
            fields: Arc::new(session_fields(&settings)),
            settings,
            model,
            input: InputCache::new(),
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

/// The request fields of a session: without the fields the plan route
/// does not take, whatever the settings asked for.
fn session_fields(settings: &Settings) -> Fields {
    let mut fields = fields(settings);
    for name in UNSUPPORTED_FIELDS {
        fields.remove(name);
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
        self.fields = Arc::new(session_fields(&self.settings));
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
