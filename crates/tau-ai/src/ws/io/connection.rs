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
    tungstenite::{Message, client::IntoClientRequest, http},
};

use crate::ws::proto::pool::ConnectionId;

/// Opens the byte stream a WebSocket runs over, and says how to upgrade
/// it. The default connects with TLS to OpenAI; tests connect on a
/// simulated network.
pub trait Connector: Send + Sync + 'static {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;

    fn connect(&self) -> impl Future<Output = io::Result<Self::Stream>> + Send;

    /// The upgrade request: the URL and headers such as `Authorization`.
    fn request(&self) -> http::Request<()>;
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
    /// of a connection, and always sent exactly once.
    Closed { connection: ConnectionId },
}

/// The driver's handle on a connection task. Dropping it closes the
/// connection.
#[derive(Debug, Clone)]
pub struct ConnectionHandle {
    outgoing: mpsc::UnboundedSender<Value>,
}

impl ConnectionHandle {
    /// Queues a frame. Frames queued before the connection opens are sent
    /// once it does. Returns false if the connection is already gone.
    pub fn send(&self, frame: Value) -> bool {
        self.outgoing.send(frame).is_ok()
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
        run(connection, connector, frames, &events).await;
        let _ = events.send(ConnectionEvent::Closed { connection });
    });
    ConnectionHandle { outgoing }
}

async fn run<C: Connector>(
    connection: ConnectionId,
    connector: Arc<C>,
    mut frames: mpsc::UnboundedReceiver<Value>,
    events: &mpsc::UnboundedSender<ConnectionEvent>,
) {
    let Ok(stream) = connector.connect().await else {
        return;
    };
    let Ok(request) = connector.request().into_client_request() else {
        return;
    };
    let Ok((mut socket, _)) = client_async(request, stream).await else {
        return;
    };
    if events.send(ConnectionEvent::Opened { connection }).is_err() {
        return;
    }
    loop {
        tokio::select! {
            outgoing = frames.recv() => match outgoing {
                Some(frame) => {
                    if socket.send(Message::text(frame.to_string())).await.is_err() {
                        return;
                    }
                }
                None => {
                    // The driver dropped the handle.
                    let _ = socket.close(None).await;
                    return;
                }
            },
            incoming = socket.next() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    if let Ok(frame) = serde_json::from_str::<Value>(&text)
                        && events.send(ConnectionEvent::Frame { connection, frame }).is_err()
                    {
                        return;
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                // Pings are answered by tungstenite; binary frames are not
                // part of the protocol.
                Some(Ok(_)) => {}
            },
        }
    }
}
