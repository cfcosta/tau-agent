//! The model interface the agent loop runs against.
//!
//! [`Llm`] opens one [`LlmSession`] per run, fixed to that run's
//! [`Settings`]. A session answers a transcript with a stream of
//! [`AssistantEvent`]s that follows the grammar in
//! [`event`](crate::event). Dropping the stream before its terminal
//! event cancels the request.
//!
//! [`OpenAi`](crate::client::OpenAi) implements it over the WebSocket
//! transport; `tau_testing::ScriptedModel` implements it from a script.

use std::fmt;

use futures_util::{
    FutureExt,
    StreamExt,
    future::BoxFuture,
    stream::{self, BoxStream},
};

use crate::{
    client::{OpenAi, Session},
    event::AssistantEvent,
    message::{Message, Timestamp},
    responses::request::Settings,
};

/// The events of one response.
pub type EventStream = BoxStream<'static, AssistantEvent>;

/// A session could not be opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmError {
    pub message: String,
}

impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for LlmError {}

/// A model provider.
pub trait Llm: Send + Sync + 'static {
    /// Opens a session for one run.
    fn open(
        &self,
        settings: Settings,
    ) -> BoxFuture<'static, Result<Box<dyn LlmSession>, LlmError>>;
}

/// One run's conversation with the model.
pub trait LlmSession: Send + 'static {
    /// The run's settings, as the provider will send them.
    fn settings(&self) -> &Settings;

    /// Asks for the next response to `transcript`. `timestamp` becomes
    /// the response's timestamp.
    fn respond(
        &mut self,
        transcript: &[Message],
        timestamp: Timestamp,
    ) -> EventStream;
}

impl Llm for OpenAi {
    fn open(
        &self,
        settings: Settings,
    ) -> BoxFuture<'static, Result<Box<dyn LlmSession>, LlmError>> {
        let client = self.clone();
        async move {
            client
                .session(settings)
                .await
                .map(|session| Box::new(session) as Box<dyn LlmSession>)
                .map_err(|stopped| LlmError {
                    message: stopped.to_string(),
                })
        }
        .boxed()
    }
}

impl LlmSession for Session {
    fn settings(&self) -> &Settings {
        Session::settings(self)
    }

    fn respond(
        &mut self,
        transcript: &[Message],
        timestamp: Timestamp,
    ) -> EventStream {
        let response = Session::respond(self, transcript, timestamp);
        stream::unfold(response, |mut response| async move {
            response.next().await.map(|event| (event, response))
        })
        .boxed()
    }
}
