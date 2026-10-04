//! The computer's side: it listens for phones over TLS, pairs them,
//! checks their tokens, and sends every phone the same numbered
//! messages.
//!
//! One task owns the state (the message count, the last messages sent,
//! the connections) and takes commands in order, so what one thread
//! asks happens in the order it asked. A phone that comes back with the
//! number of the last message it had gets what it missed, if the host
//! still has it; otherwise the app is asked for a snapshot.

use std::{
    collections::HashMap,
    io,
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc,
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::Value;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    time::timeout,
};
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::{accept_async, tungstenite::Message};

pub use crate::feed::REPLAY;
use crate::{
    devices::{Device, Devices, DevicesError},
    feed::{Feed, Joined},
    pairing::{Address, Fingerprint, PairingCode},
    tls::{Identity, TlsError},
    wire::{Answer, Down, Hello, Refusal, Up, VERSION},
};
/// How long a phone has to finish TLS, the WebSocket and its hello.
const HANDSHAKE: Duration = Duration::from_secs(10);
const PING: Duration = Duration::from_secs(15);
/// A phone not heard from for this long is gone.
const IDLE: Duration = Duration::from_secs(45);

pub struct ServerConfig {
    pub listen: SocketAddr,
    /// Where the certificate and the paired phones are kept.
    pub dir: PathBuf,
    /// The computer's name, as phones show it.
    pub host: String,
}

/// One phone's connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConnId(u64);

/// What the app is told.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerEvent {
    /// A phone needs everything: answer with [`ServerHandle::snapshot`].
    /// Messages broadcast before that are in the snapshot, and reach it
    /// no other way.
    NeedSnapshot { conn: ConnId, device: Device },
    /// A phone sent this.
    Up {
        conn: ConnId,
        device: Device,
        body: Value,
    },
    /// A phone paired, connected, was renamed or revoked.
    DevicesChanged(Vec<Device>),
}

#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error(transparent)]
    Tls(#[from] TlsError),
    #[error(transparent)]
    Devices(#[from] DevicesError),
    #[error("could not listen: {0}")]
    Io(#[from] io::Error),
}

pub struct Server;

impl Server {
    /// Listens on `config.listen`, on the current tokio runtime.
    pub async fn start(
        config: ServerConfig,
    ) -> Result<(ServerHandle, UnboundedReceiver<ServerEvent>), ServerError>
    {
        let identity = Identity::load_or_create(&config.dir, &config.host)?;
        let acceptor = TlsAcceptor::from(identity.server_config()?);
        let devices = Arc::new(Mutex::new(Devices::load(&config.dir)?));
        let listener = TcpListener::bind(config.listen).await?;
        let local_addr = listener.local_addr()?;
        let (commands, receiver) = mpsc::unbounded_channel();
        let (events, events_out) = mpsc::unbounded_channel();
        let state = State {
            feed: Feed::new(first_seq()),
            conns: HashMap::new(),
            devices: devices.clone(),
            host: config.host.clone(),
            events: events.clone(),
        };
        tokio::spawn(state.run(receiver));
        tokio::spawn(listen(listener, acceptor, commands.clone()));
        let handle = ServerHandle {
            inner: Arc::new(Inner {
                commands,
                events,
                devices,
                fingerprint: identity.fingerprint(),
                host: config.host,
                local_addr,
            }),
        };
        Ok((handle, events_out))
    }
}

/// Numbers start from the clock, so they only grow across restarts,
/// and a phone's old number is never taken for a new message.
fn first_seq() -> u64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    u64::try_from(millis).unwrap_or(u64::MAX >> 12) << 10
}

/// Drives a running server. Clones drive the same one.
#[derive(Clone)]
pub struct ServerHandle {
    inner: Arc<Inner>,
}

struct Inner {
    commands: UnboundedSender<Command>,
    events: UnboundedSender<ServerEvent>,
    devices: Arc<Mutex<Devices>>,
    fingerprint: Fingerprint,
    host: String,
    local_addr: SocketAddr,
}

impl ServerHandle {
    /// Sends `body` to every phone, numbered.
    pub fn broadcast(&self, body: Value) {
        self.command(Command::Broadcast(body));
    }

    /// Answers a [`ServerEvent::NeedSnapshot`].
    pub fn snapshot(&self, conn: ConnId, bodies: Vec<Value>) {
        self.command(Command::Snapshot(conn, bodies));
    }

    /// A new code to pair a phone with, reaching the computer at
    /// `address`; the one before stops working.
    pub fn pairing_code(&self, address: Address) -> PairingCode {
        let secret = self.devices().open_secret();
        PairingCode {
            address,
            host: self.inner.host.clone(),
            fingerprint: self.inner.fingerprint,
            secret,
        }
    }

    /// Stops the open pairing code working.
    pub fn close_pairing(&self) {
        self.devices().close_secret();
    }

    pub fn paired(&self) -> Vec<Device> {
        self.devices().list()
    }

    /// Forgets a phone: its token is refused, and its connections close.
    pub fn revoke(&self, id: &str) -> Result<bool, DevicesError> {
        let removed = self.devices().revoke(id)?;
        if removed {
            self.command(Command::Revoked(id.to_owned()));
            self.changed();
        }
        Ok(removed)
    }

    pub fn rename(&self, id: &str, name: &str) -> Result<bool, DevicesError> {
        let renamed = self.devices().rename(id, name)?;
        if renamed {
            self.changed();
        }
        Ok(renamed)
    }

    pub fn fingerprint(&self) -> Fingerprint {
        self.inner.fingerprint
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.inner.local_addr
    }

    /// Stops listening and closes every connection.
    pub fn stop(&self) {
        self.command(Command::Stop);
    }

    fn devices(&self) -> std::sync::MutexGuard<'_, Devices> {
        self.inner.devices.lock().expect("not poisoned")
    }

    fn changed(&self) {
        let list = self.devices().list();
        let _ = self.inner.events.send(ServerEvent::DevicesChanged(list));
    }

    fn command(&self, command: Command) {
        // A stopped server takes no more commands.
        let _ = self.inner.commands.send(command);
    }
}

enum Command {
    Broadcast(Value),
    Snapshot(ConnId, Vec<Value>),
    Revoked(String),
    Stop,
    Hello {
        conn: ConnId,
        hello: Hello,
        out: UnboundedSender<Out>,
    },
    Up {
        conn: ConnId,
        id: u64,
        body: Value,
    },
    Closed(ConnId),
}

/// What a connection's task writes.
enum Out {
    Frame(String),
    Close,
}

struct Conn {
    device: String,
    out: UnboundedSender<Out>,
}

struct State {
    /// The messages sent, and which connections take new ones.
    feed: Feed<ConnId>,
    conns: HashMap<ConnId, Conn>,
    devices: Arc<Mutex<Devices>>,
    host: String,
    events: UnboundedSender<ServerEvent>,
}

impl State {
    async fn run(mut self, mut commands: UnboundedReceiver<Command>) {
        while let Some(command) = commands.recv().await {
            match command {
                Command::Broadcast(body) => self.broadcast(body),
                Command::Snapshot(conn, bodies) => self.snapshot(conn, bodies),
                Command::Revoked(id) => {
                    let feed = &mut self.feed;
                    self.conns.retain(|conn_id, conn| {
                        let keep = conn.device != id;
                        if !keep {
                            let _ = conn.out.send(Out::Close);
                            feed.leave(*conn_id);
                        }
                        keep
                    });
                }
                Command::Stop => break,
                Command::Hello { conn, hello, out } => {
                    self.hello(conn, hello, out)
                }
                Command::Up { conn, id, body } => self.up(conn, id, body),
                Command::Closed(conn) => {
                    self.conns.remove(&conn);
                    self.feed.leave(conn);
                }
            }
        }
        // Dropping the connections' senders closes them; the listener
        // sees the commands closed and stops.
    }

    fn broadcast(&mut self, body: Value) {
        let (down, to) = self.feed.broadcast(body);
        let frame = to_frame(&down);
        for conn in to.iter().filter_map(|conn| self.conns.get(conn)) {
            let _ = conn.out.send(Out::Frame(frame.clone()));
        }
    }

    fn snapshot(&mut self, id: ConnId, bodies: Vec<Value>) {
        let (Some(conn), Some(down)) =
            (self.conns.get(&id), self.feed.snapshot(id, bodies))
        else {
            return;
        };
        let _ = conn.out.send(Out::Frame(to_frame(&down)));
    }

    fn hello(&mut self, id: ConnId, hello: Hello, out: UnboundedSender<Out>) {
        if hello.version() != VERSION {
            return refuse(&out, Refusal::Version { speaks: VERSION });
        }
        let mut devices = self.devices.lock().expect("not poisoned");
        let (device, token, last_seq) = match hello {
            Hello::Pair { secret, name, .. } => {
                match devices.pair(&secret, &name) {
                    Ok(Ok((device, token))) => (device, Some(token), None),
                    Ok(Err(refusal)) => return refuse(&out, refusal),
                    Err(_) => return close(&out),
                }
            }
            Hello::Resume {
                token, last_seq, ..
            } => match devices.resume(&token) {
                Ok(Some(device)) => (device, None, last_seq),
                Ok(None) => return refuse(&out, Refusal::UnknownToken),
                Err(_) => return close(&out),
            },
        };
        let list = devices.list();
        drop(devices);
        let joined = self.feed.join(id, last_seq);
        let _ = out.send(Out::Frame(to_frame(&Answer::Welcome {
            version: VERSION,
            host: self.host.clone(),
            device: device.id.clone(),
            token,
            resumed: matches!(joined, Joined::Replay(_)),
        })));
        if let Joined::Replay(missed) = &joined {
            for down in missed {
                let _ = out.send(Out::Frame(to_frame(down)));
            }
        }
        self.conns.insert(
            id,
            Conn {
                device: device.id.clone(),
                out,
            },
        );
        let _ = self.events.send(ServerEvent::DevicesChanged(list));
        if joined == Joined::NeedSnapshot {
            let _ = self
                .events
                .send(ServerEvent::NeedSnapshot { conn: id, device });
        }
    }

    /// A phone's request: taken once, however often it comes, and
    /// answered each time, so the phone stops sending it.
    fn up(&self, conn: ConnId, up: u64, body: Value) {
        let Some(Conn { device: id, out }) = self.conns.get(&conn) else {
            return;
        };
        let mut devices = self.devices.lock().expect("not poisoned");
        let Ok(taken) = devices.take(id, up) else {
            // Not saved: not taken, so the phone sends it again.
            return;
        };
        let device = devices.list().into_iter().find(|device| &device.id == id);
        drop(devices);
        if let (true, Some(device)) = (taken, device) {
            let _ = self.events.send(ServerEvent::Up { conn, device, body });
        }
        let _ = out.send(Out::Frame(to_frame(&Down::Ack { up })));
    }
}

fn close(out: &UnboundedSender<Out>) {
    let _ = out.send(Out::Close);
}

fn refuse(out: &UnboundedSender<Out>, reason: Refusal) {
    let _ = out.send(Out::Frame(to_frame(&Answer::Refused { reason })));
    close(out);
}

fn to_frame(frame: &impl serde::Serialize) -> String {
    serde_json::to_string(frame).expect("frames are plain JSON")
}

async fn listen(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    commands: UnboundedSender<Command>,
) {
    let next = AtomicU64::new(0);
    loop {
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            () = commands.closed() => return,
        };
        let Ok((tcp, _)) = accepted else {
            continue;
        };
        let conn = ConnId(next.fetch_add(1, Ordering::Relaxed));
        tokio::spawn(serve(tcp, acceptor.clone(), commands.clone(), conn));
    }
}

/// One phone: TLS, the WebSocket, its hello, then frames both ways.
async fn serve(
    tcp: TcpStream,
    acceptor: TlsAcceptor,
    commands: UnboundedSender<Command>,
    conn: ConnId,
) {
    let _ = tcp.set_nodelay(true);
    let Ok(Ok(tls)) = timeout(HANDSHAKE, acceptor.accept(tcp)).await else {
        return;
    };
    let Ok(Ok(socket)) = timeout(HANDSHAKE, accept_async(tls)).await else {
        return;
    };
    let (mut sink, mut stream) = socket.split();
    let hello = match timeout(HANDSHAKE, stream.next()).await {
        Ok(Some(Ok(Message::Text(text)))) => serde_json::from_str(&text),
        _ => return,
    };
    let Ok(hello) = hello else {
        return;
    };
    let (out, mut outgoing) = mpsc::unbounded_channel();
    if commands.send(Command::Hello { conn, hello, out }).is_err() {
        return;
    }
    let mut ping = tokio::time::interval(PING);
    ping.tick().await;
    let mut heard = Instant::now();
    loop {
        tokio::select! {
            out = outgoing.recv() => match out {
                Some(Out::Frame(text)) => {
                    if sink.send(Message::text(text)).await.is_err() {
                        break;
                    }
                }
                Some(Out::Close) | None => {
                    let _ = sink.send(Message::Close(None)).await;
                    break;
                }
            },
            frame = stream.next() => match frame {
                Some(Ok(Message::Text(text))) => {
                    heard = Instant::now();
                    if let Ok(Up::Up { id, body }) = serde_json::from_str(&text) {
                        let _ = commands.send(Command::Up { conn, id, body });
                    }
                }
                Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                Some(Ok(_)) => heard = Instant::now(),
            },
            _ = ping.tick() => {
                if heard.elapsed() > IDLE
                    || sink.send(Message::Ping(Vec::new().into())).await.is_err()
                {
                    break;
                }
            }
        }
    }
    let _ = commands.send(Command::Closed(conn));
}
