//! The phone's side: read a computer's certificate, pair with it, and
//! connect again with the device token.

use std::{fmt, io, time::Duration};

use futures_util::{
    SinkExt as _,
    StreamExt as _,
    stream::{SplitSink, SplitStream},
};
use serde::{Deserialize, Serialize};
use tokio::{
    net::TcpStream,
    sync::mpsc::{self, UnboundedSender},
    time::timeout,
};
use tokio_rustls::{
    TlsConnector,
    client::TlsStream,
    rustls::pki_types::ServerName,
};
use tokio_tungstenite::{
    WebSocketStream,
    client_async,
    tungstenite::{self, Message},
};

use crate::{
    pairing::{Address, Fingerprint, PairingSecret},
    tls::{Pinned, SERVER_NAME},
    wire::{Answer, Down, Hello, Refusal, Up, VERSION},
};

/// How long reaching a computer may take.
const CONNECT: Duration = Duration::from_secs(5);
/// A host that sends nothing for this long, not even a ping, is gone.
const IDLE: Duration = Duration::from_secs(45);

type Socket = WebSocketStream<TlsStream<TcpStream>>;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("could not reach {address}: {source}")]
    Unreachable { address: Address, source: io::Error },
    #[error("{address} did not answer in time")]
    Timeout { address: Address },
    #[error("the computer's certificate is not the one expected ({})", found.short())]
    CertificateMismatch { found: Fingerprint },
    #[error("refused: {0}")]
    Refused(#[from] Refusal),
    #[error(transparent)]
    WebSocket(#[from] tungstenite::Error),
    #[error("a frame is not valid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("the computer answered out of turn: {0}")]
    Protocol(&'static str),
    #[error("the connection is closed")]
    Closed,
}

impl ClientError {
    /// Whether trying again later may work: the computer was not there,
    /// rather than saying no.
    pub fn is_unreachable(&self) -> bool {
        matches!(
            self,
            Self::Unreachable { .. }
                | Self::Timeout { .. }
                | Self::WebSocket(_)
                | Self::Closed
        )
    }
}

/// What a phone keeps to connect again.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credentials {
    /// The computer's name.
    pub host: String,
    pub address: Address,
    pub fingerprint: Fingerprint,
    /// This phone's device id.
    pub device: String,
    pub token: String,
}

/// Never prints the token.
impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("host", &self.host)
            .field("address", &self.address)
            .field("device", &self.device)
            .finish_non_exhaustive()
    }
}

/// Reads the certificate of whatever answers at `address`, trusting
/// nothing: for a person to compare with the computer's.
pub async fn probe(address: &Address) -> Result<Fingerprint, ClientError> {
    let pinned = Pinned::recording();
    tls(address, &pinned).await?;
    pinned.seen().ok_or(ClientError::Protocol("no certificate"))
}

/// Pairs with the computer at `address`, whose certificate must be
/// `fingerprint`, trading `secret` for a device token.
pub async fn pair(
    address: &Address,
    fingerprint: Fingerprint,
    secret: &PairingSecret,
    name: &str,
) -> Result<(Connection, Credentials), ClientError> {
    let hello = Hello::Pair {
        version: VERSION,
        secret: secret.to_string(),
        name: name.to_owned(),
    };
    let (connection, token) = open(address, fingerprint, &hello).await?;
    let token = token.ok_or(ClientError::Protocol("paired without a token"))?;
    let credentials = Credentials {
        host: connection.host.clone(),
        address: address.clone(),
        fingerprint,
        device: connection.device.clone(),
        token,
    };
    Ok((connection, credentials))
}

/// Connects again as a paired phone. With the number of the last
/// message it had, the computer sends what it missed, if it still can.
pub async fn resume(
    credentials: &Credentials,
    last_seq: Option<u64>,
) -> Result<Connection, ClientError> {
    let hello = Hello::Resume {
        version: VERSION,
        token: credentials.token.clone(),
        last_seq,
    };
    let (connection, _) =
        open(&credentials.address, credentials.fingerprint, &hello).await?;
    Ok(connection)
}

async fn tls(
    address: &Address,
    pinned: &std::sync::Arc<Pinned>,
) -> Result<TlsStream<TcpStream>, ClientError> {
    let timed_out = || ClientError::Timeout {
        address: address.clone(),
    };
    let unreachable = |source| ClientError::Unreachable {
        address: address.clone(),
        source,
    };
    let tcp = timeout(
        CONNECT,
        TcpStream::connect((address.host.as_str(), address.port)),
    )
    .await
    .map_err(|_| timed_out())?
    .map_err(unreachable)?;
    let _ = tcp.set_nodelay(true);
    let name = ServerName::try_from(SERVER_NAME).expect("a valid DNS name");
    let connector = TlsConnector::from(pinned.client_config());
    match timeout(CONNECT, connector.connect(name, tcp)).await {
        Err(_) => Err(timed_out()),
        Ok(Ok(stream)) => Ok(stream),
        Ok(Err(error)) => match (pinned.seen(), pinned.expected()) {
            (Some(found), Some(expected)) if found != expected => {
                Err(ClientError::CertificateMismatch { found })
            }
            _ => Err(unreachable(error)),
        },
    }
}

async fn open(
    address: &Address,
    fingerprint: Fingerprint,
    hello: &Hello,
) -> Result<(Connection, Option<String>), ClientError> {
    let stream = tls(address, &Pinned::expecting(fingerprint)).await?;
    let (socket, _) = timeout(
        CONNECT,
        client_async(format!("wss://{SERVER_NAME}/"), stream),
    )
    .await
    .map_err(|_| ClientError::Timeout {
        address: address.clone(),
    })??;
    let (mut sink, mut stream) = socket.split();
    sink.send(Message::text(serde_json::to_string(hello)?))
        .await?;
    let answer = loop {
        let frame = timeout(CONNECT, stream.next()).await.map_err(|_| {
            ClientError::Timeout {
                address: address.clone(),
            }
        })?;
        match frame {
            Some(Ok(Message::Text(text))) => {
                break serde_json::from_str::<Answer>(&text)?;
            }
            Some(Ok(Message::Close(_))) | None => {
                return Err(ClientError::Closed);
            }
            Some(Err(error)) => return Err(error.into()),
            Some(Ok(_)) => {}
        }
    };
    let (host, device, token, resumed) = match answer {
        Answer::Refused { reason } => return Err(reason.into()),
        Answer::Welcome {
            host,
            device,
            token,
            resumed,
            ..
        } => (host, device, token, resumed),
    };
    let (sender, outgoing) = mpsc::unbounded_channel();
    tokio::spawn(write(sink, outgoing));
    let connection = Connection {
        address: address.clone(),
        stream,
        sender: Sender { sender },
        host,
        device,
        resumed,
    };
    Ok((connection, token))
}

/// Sends what the app gives it, until the app or the socket closes.
async fn write(
    mut sink: SplitSink<Socket, Message>,
    mut outgoing: mpsc::UnboundedReceiver<Up>,
) {
    while let Some(up) = outgoing.recv().await {
        let frame = serde_json::to_string(&up).expect("frames are plain JSON");
        if sink.send(Message::text(frame)).await.is_err() {
            return;
        }
    }
    let _ = sink.send(Message::Close(None)).await;
}

/// A connection to a computer.
pub struct Connection {
    address: Address,
    stream: SplitStream<Socket>,
    sender: Sender,
    host: String,
    device: String,
    resumed: bool,
}

impl Connection {
    /// The next message from the computer; None once it closed.
    pub async fn recv(&mut self) -> Result<Option<Down>, ClientError> {
        loop {
            let frame =
                timeout(IDLE, self.stream.next()).await.map_err(|_| {
                    ClientError::Timeout {
                        address: self.address.clone(),
                    }
                })?;
            match frame {
                Some(Ok(Message::Text(text))) => {
                    return Ok(Some(serde_json::from_str(&text)?));
                }
                Some(Ok(Message::Close(_))) | None => return Ok(None),
                Some(Err(error)) => return Err(error.into()),
                // Pings are answered as they are read.
                Some(Ok(_)) => {}
            }
        }
    }

    pub fn sender(&self) -> Sender {
        self.sender.clone()
    }

    /// The computer's name.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// This phone's device id.
    pub fn device(&self) -> &str {
        &self.device
    }

    /// Whether the computer sends what the phone missed, rather than a
    /// snapshot.
    pub fn resumed(&self) -> bool {
        self.resumed
    }
}

/// Sends to the computer; clones send on the same connection.
#[derive(Clone)]
pub struct Sender {
    sender: UnboundedSender<Up>,
}

impl Sender {
    /// Sends a request from the phone's [`Outbox`](crate::outbox::Outbox).
    pub fn send(&self, up: Up) -> Result<(), ClientError> {
        if self.sender.send(up).is_err() {
            return Err(ClientError::Closed);
        }
        Ok(())
    }
}
