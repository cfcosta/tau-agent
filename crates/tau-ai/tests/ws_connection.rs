//! Connection tasks (`tau_ai::ws::io::connection`) in a turmoil
//! simulation: a real WebSocket codec over a simulated network.

use std::{cell::RefCell, io, rc::Rc, sync::Arc};

use futures_util::{SinkExt, StreamExt};
use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_ai::ws::io::connection::{self, ConnectionEvent, Connector};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::{Message, http};

/// Connects to the host `server` on the simulated network.
struct SimConnector;

impl Connector for SimConnector {
    type Stream = turmoil::net::TcpStream;

    async fn connect(&self) -> io::Result<Self::Stream> {
        turmoil::net::TcpStream::connect(("server", 80)).await
    }

    fn request(&self) -> http::Request<()> {
        http::Request::builder()
            .uri("ws://server/v1/responses")
            .header("Host", "server")
            .header("Authorization", "Bearer test")
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header(
                "Sec-WebSocket-Key",
                tokio_tungstenite::tungstenite::handshake::client::generate_key(
                ),
            )
            .body(())
            .unwrap()
    }
}

/// What the server does after reading the client's frames.
#[derive(Debug, Clone)]
struct Script {
    /// Frames the client sends, in order.
    upstream: Vec<Value>,
    /// Frames the server sends back after reading all of `upstream`.
    downstream: Vec<Value>,
    /// Whether the server then sends a garbage (non-JSON) text frame,
    /// which the client must drop.
    garbage: bool,
}

#[hegel::composite]
fn frame(tc: TestCase) -> Value {
    json!({ "type": "response.output_text.delta", "delta": tc.draw(tau_testing::generators::text(16)) })
}

/// Frames flow both ways in order; `Opened` comes first and `Closed`
/// last, exactly once, when the server closes.
#[hegel::test(test_cases = 50)]
fn frames_flow_in_order(tc: TestCase) {
    let script = Script {
        upstream: tc.draw(gs::vecs(frame()).max_size(6)),
        downstream: tc.draw(gs::vecs(frame()).max_size(6)),
        garbage: tc.draw(gs::booleans()),
    };
    let seed = tc.draw(gs::integers::<u64>());
    let received: Rc<RefCell<Vec<Value>>> = Rc::default();

    let mut sim = turmoil::Builder::new().rng_seed(seed).build();
    {
        let script = script.clone();
        let received = received.clone();
        sim.host("server", move || {
            let script = script.clone();
            let received = received.clone();
            async move {
                let listener =
                    turmoil::net::TcpListener::bind(("0.0.0.0", 80)).await?;
                let (stream, _) = listener.accept().await?;
                let mut socket =
                    tokio_tungstenite::accept_async(stream).await?;
                for _ in 0..script.upstream.len() {
                    match socket.next().await {
                        Some(Ok(Message::Text(text))) => received
                            .borrow_mut()
                            .push(serde_json::from_str(&text)?),
                        other => panic!(
                            "server expected a text frame, got {other:?}"
                        ),
                    }
                }
                if script.garbage {
                    socket.send(Message::text("not json")).await?;
                }
                for frame in &script.downstream {
                    socket.send(Message::text(frame.to_string())).await?;
                }
                socket.close(None).await?;
                Ok(())
            }
        });
    }
    let events: Rc<RefCell<Vec<ConnectionEvent>>> = Rc::default();
    {
        let upstream = script.upstream.clone();
        let events = events.clone();
        sim.client("client", async move {
            let (tx, mut rx) = mpsc::unbounded_channel();
            let handle = connection::spawn(7, Arc::new(SimConnector), tx);
            // Queued before the connection opens; sent once it does.
            for frame in upstream {
                assert!(handle.send(frame));
            }
            while let Some(event) = rx.recv().await {
                let closed = matches!(event, ConnectionEvent::Closed { .. });
                events.borrow_mut().push(event);
                if closed {
                    break;
                }
            }
            Ok(())
        });
    }
    sim.run().unwrap();

    assert_eq!(*received.borrow(), script.upstream);
    let mut expected = vec![ConnectionEvent::Opened { connection: 7 }];
    expected.extend(script.downstream.iter().map(|frame| {
        ConnectionEvent::Frame {
            connection: 7,
            frame: frame.clone(),
        }
    }));
    expected.push(ConnectionEvent::Closed { connection: 7 });
    assert_eq!(*events.borrow(), expected);
}

/// A connection that cannot open reports `Closed` and nothing else.
#[test]
fn failed_connect_reports_closed_only() {
    let mut sim = turmoil::Builder::new().build();
    let events: Rc<RefCell<Vec<ConnectionEvent>>> = Rc::default();
    {
        let events = events.clone();
        // No host named `server` listens, so the connect fails.
        sim.host("server", || async {
            std::future::pending::<()>().await;
            Ok(())
        });
        sim.client("client", async move {
            let (tx, mut rx) = mpsc::unbounded_channel();
            let _handle = connection::spawn(3, Arc::new(SimConnector), tx);
            if let Some(event) = rx.recv().await {
                events.borrow_mut().push(event);
            }
            Ok(())
        });
    }
    sim.run().unwrap();
    assert_eq!(
        *events.borrow(),
        vec![ConnectionEvent::Closed { connection: 3 }]
    );
}

/// Dropping the handle closes the socket: the server sees the close.
#[test]
fn dropping_the_handle_closes_the_socket() {
    let mut sim = turmoil::Builder::new().build();
    let server_saw_close: Rc<RefCell<bool>> = Rc::default();
    {
        let seen = server_saw_close.clone();
        sim.host("server", move || {
            let seen = seen.clone();
            async move {
                let listener =
                    turmoil::net::TcpListener::bind(("0.0.0.0", 80)).await?;
                let (stream, _) = listener.accept().await?;
                let mut socket =
                    tokio_tungstenite::accept_async(stream).await?;
                while let Some(message) = socket.next().await {
                    if matches!(message, Ok(Message::Close(_)) | Err(_)) {
                        break;
                    }
                }
                *seen.borrow_mut() = true;
                Ok(())
            }
        });
        sim.client("client", async move {
            let (tx, mut rx) = mpsc::unbounded_channel();
            let handle = connection::spawn(1, Arc::new(SimConnector), tx);
            assert_eq!(
                rx.recv().await,
                Some(ConnectionEvent::Opened { connection: 1 })
            );
            drop(handle);
            assert_eq!(
                rx.recv().await,
                Some(ConnectionEvent::Closed { connection: 1 })
            );
            // The simulation ends with the client; give the server a moment
            // of simulated time to read the close frame.
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            Ok(())
        });
    }
    sim.run().unwrap();
    assert!(*server_saw_close.borrow());
}
