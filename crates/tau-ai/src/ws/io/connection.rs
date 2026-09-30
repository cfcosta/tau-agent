//! One WebSocket connection as a task: it connects, then passes frames
//! between the socket and the driver until either side closes.
//!
//! Frames are JSON objects both ways. A text frame that is not JSON is
//! dropped: the Responses protocol never sends one, and the driver has no
//! way to route it.

use std::{future::Future, io, sync::Arc};

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::mpsc,
};
use tokio_tungstenite::{
    client_async,
    tungstenite::{self, Message, client::IntoClientRequest, http},
};

use crate::{
    chatgpt::{ApiError, ChatGptError},
    refusal::Refusal,
    ws::proto::{continuation::Body, pool::ConnectionId},
};

/// Opens the byte stream a WebSocket runs over, and says how to upgrade
/// it. The default connects with TLS to OpenAI; tests connect on a
/// simulated network.
pub trait Connector: Send + Sync + 'static {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;

    fn connect(&self) -> impl Future<Output = io::Result<Self::Stream>> + Send;

    /// The upgrade request: the URL and headers such as `Authorization`.
    fn request(&self) -> http::Request<()>;

    /// Whether requests name their lane with `stream_id`, so several lanes
    /// share a connection. An endpoint that may not take `stream_id` (the
    /// ChatGPT plan route, unverified) returns `false`; its client must
    /// then allow one lane per connection, and frames go to the lane their
    /// connection carries.
    ///
    /// `connect` may fail with an `io::Error` that wraps a
    /// [`ChatGptError`]; the connection reports it as a [`Refusal`].
    fn tags_lanes(&self) -> bool {
        true
    }
}

/// What a connection task reports to the driver.
#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionEvent {
    /// The connection is open and frames can flow.
    Opened { connection: ConnectionId },
    /// A frame arrived.
    Frame {
        connection: ConnectionId,
        frame: Value,
    },
    /// The connection closed or could not open. Always the last event
    /// of a connection, and always sent exactly once. `refusal` says why
    /// it could not open when OpenAI or the sign-in said so: a refused
    /// upgrade (its status, code, request id and body), or a sign-in
    /// that could not give a token.
    Closed {
        connection: ConnectionId,
        refusal: Option<Refusal>,
    },
}

/// A frame to send.
#[derive(Debug, Clone, PartialEq)]
pub enum Outgoing {
    Json(Value),
    /// A request on a lane. It is serialized by the connection task, so
    /// a full resend costs the driver, which every lane shares, nothing.
    Request {
        body: Body,
        /// `None` on an endpoint that does not take `stream_id`.
        stream_id: Option<Value>,
    },
}

impl From<Value> for Outgoing {
    fn from(frame: Value) -> Self {
        Self::Json(frame)
    }
}

impl Outgoing {
    fn into_text(self) -> String {
        match self {
            Self::Json(frame) => frame.to_string(),
            Self::Request {
                body,
                stream_id: Some(stream_id),
            } => body.to_frame(&[("stream_id", &stream_id)]),
            Self::Request {
                body,
                stream_id: None,
            } => body.to_frame(&[]),
        }
    }
}

/// The driver's handle on a connection task. Dropping it closes the
/// connection.
#[derive(Debug, Clone)]
pub struct ConnectionHandle {
    outgoing: mpsc::UnboundedSender<Outgoing>,
}

impl ConnectionHandle {
    /// Queues a frame. Frames queued before the connection opens are sent
    /// once it does. Returns false if the connection is already gone.
    pub fn send(&self, frame: impl Into<Outgoing>) -> bool {
        self.outgoing.send(frame.into()).is_ok()
    }
}

/// Starts a connection task and returns its handle.
pub fn spawn<C: Connector>(
    connection: ConnectionId,
    connector: Arc<C>,
    events: mpsc::UnboundedSender<ConnectionEvent>,
) -> ConnectionHandle {
    let (outgoing, frames) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let refusal = run(connection, connector, frames, &events).await;
        let _ = events.send(ConnectionEvent::Closed {
            connection,
            refusal,
        });
    });
    ConnectionHandle { outgoing }
}

/// Runs the connection until it closes. Returns why it could not open,
/// when it was refused.
async fn run<C: Connector>(
    connection: ConnectionId,
    connector: Arc<C>,
    mut frames: mpsc::UnboundedReceiver<Outgoing>,
    events: &mpsc::UnboundedSender<ConnectionEvent>,
) -> Option<Refusal> {
    let stream = match connector.connect().await {
        Ok(stream) => stream,
        Err(error) => return connect_refusal(&error),
    };
    let request = connector.request().into_client_request().ok()?;
    let mut socket = match client_async(request, stream).await {
        Ok((socket, _)) => socket,
        Err(tungstenite::Error::Http(response)) => {
            return Some(upgrade_refusal(&response));
        }
        Err(_) => return None,
    };
    if events.send(ConnectionEvent::Opened { connection }).is_err() {
        return None;
    }
    loop {
        tokio::select! {
            outgoing = frames.recv() => match outgoing {
                Some(frame) => {
                    if socket.send(Message::text(frame.into_text())).await.is_err() {
                        return None;
                    }
                }
                None => {
                    // The driver dropped the handle.
                    let _ = socket.close(None).await;
                    return None;
                }
            },
            incoming = socket.next() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    if let Ok(frame) = serde_json::from_str::<Value>(&text)
                        && events.send(ConnectionEvent::Frame { connection, frame }).is_err()
                    {
                        return None;
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return None,
                // Pings are answered by tungstenite; binary frames are not
                // part of the protocol.
                Some(Ok(_)) => {}
            },
        }
    }
}

/// A connect failure the sign-in explained; `None` for a plain network
/// failure.
fn connect_refusal(error: &io::Error) -> Option<Refusal> {
    let chatgpt = error.get_ref()?.downcast_ref::<ChatGptError>()?;
    Refusal::from_chatgpt(chatgpt)
}

/// The refusal of an upgrade answered with an HTTP status instead of 101.
fn upgrade_refusal(response: &http::Response<Option<Vec<u8>>>) -> Refusal {
    let request_id = response
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = response.body().as_deref().unwrap_or_default();
    Refusal::from_api(&ApiError::new(
        response.status().as_u16(),
        request_id,
        body,
    ))
}
