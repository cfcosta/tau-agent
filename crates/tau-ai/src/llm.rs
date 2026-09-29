//! The model interface the agent loop runs against.
//!
//! [`Llm`] opens one [`LlmSession`] per run, fixed to that run's
//! [`Settings`], save the reasoning effort, which
//! [`LlmSession::set_reasoning`] may change between requests. A session answers a transcript with a stream of
//! [`AssistantEvent`]s that follows the grammar in
//! [`event`](crate::event). Dropping the stream before its terminal
//! event cancels the request.
//!
//! [`OpenAi`](crate::client::OpenAi) implements it over the WebSocket
//! transport; `tau_testing::ScriptedModel` implements it from a script.

use futures_util::{
    FutureExt,
    StreamExt,
    future::BoxFuture,
    stream::{self, BoxStream},
};

use crate::{
    client::{OpenAi, Session},
    event::AssistantEvent,
    message::{Message, Timestamp, Usage},
    responses::request::{ReasoningEffort, Settings},
};

/// The events of one response.
pub type EventStream = BoxStream<'static, AssistantEvent>;

/// A session could not be opened.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct LlmError {
    pub message: String,
}

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

    /// Sets the reasoning effort of the requests that follow; `None`
    /// leaves it to the model. The other settings stay as they are.
    fn set_reasoning(&mut self, effort: Option<ReasoningEffort>);

    /// Asks for the next response to `transcript`. `timestamp` becomes
    /// the response's timestamp.
    fn respond(
        &mut self,
        transcript: &[Message],
        timestamp: Timestamp,
    ) -> EventStream;

    /// Prepares the session before its first turn, if the provider can:
    /// OpenAI's `generate: false`. Returns what it cost. A provider that
    /// cannot warm up does nothing.
    fn warm_up(
        &mut self,
        timestamp: Timestamp,
    ) -> BoxFuture<'static, Result<Usage, LlmError>> {
        let _ = timestamp;
        async { Ok(Usage::default()) }.boxed()
    }
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

    fn set_reasoning(&mut self, effort: Option<ReasoningEffort>) {
        Session::set_reasoning(self, effort);
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

    fn warm_up(
        &mut self,
        timestamp: Timestamp,
    ) -> BoxFuture<'static, Result<Usage, LlmError>> {
        let mut response = Session::warm_up(self, timestamp);
        async move {
            let mut failure = None;
            while let Some(event) = response.next().await {
                match event {
                    AssistantEvent::Done { usage, .. } => return Ok(usage),
                    AssistantEvent::Error { message, .. } => {
                        failure = Some(message)
                    }
                    _ => {}
                }
            }
            Err(LlmError {
                message: failure
                    .unwrap_or_else(|| "the warm-up did not finish".into()),
            })
        }
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use futures_util::stream;

    use super::*;

    struct Plain(Settings);

    impl LlmSession for Plain {
        fn settings(&self) -> &Settings {
            &self.0
        }

        fn set_reasoning(&mut self, effort: Option<ReasoningEffort>) {
            self.0.reasoning = effort;
        }

        fn respond(&mut self, _: &[Message], _: Timestamp) -> EventStream {
            stream::empty().boxed()
        }
    }

    /// A provider that cannot warm up does nothing, and it costs nothing.
    #[test]
    fn the_default_warm_up_does_nothing() {
        let mut session = Plain(Settings::default());
        let usage = futures_util::FutureExt::now_or_never(session.warm_up(0))
            .expect("ready at once")
            .unwrap();
        assert_eq!(usage, Usage::default());
    }
}
