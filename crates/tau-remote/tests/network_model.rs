//! The phones and the computer over the real network, against a model
//! of what each side must end up with, through everything that happens
//! to them.
//!
//! The computer runs the real server, with a clock the test sets, on a
//! data directory kept across its restarts. One to three phones run the
//! real client the way the phone app does (`tau_ui_remote::remote`):
//! credentials kept across the app's restarts, the last message's
//! number and the [`Outbox`] only while it runs, the outbox numbered
//! from the phone's own clock. Every connection goes over TLS on the
//! loopback through a proxy the test drives, which can cut a
//! connection, cut it in the middle of a frame, or hold it.
//!
//! Pairing codes are opened, used, used again, replaced and left to go
//! stale; phones are revoked, connected or not; either side restarts;
//! either side's wall clock moves either way; a phone speaks an older
//! protocol, or an impostor answers in the computer's place. The
//! computer sends kept and transient messages, phones send requests.
//!
//! After each step everything that can arrive arrives, and:
//!
//! - each request a phone made is taken once by the computer, and the
//!   computer says it took one only when it did;
//! - each phone has every kept message, in order, with no gap and none
//!   twice, or a snapshot in their place;
//! - a revoked token, a pairing secret stale, used or replaced, and
//!   another computer's certificate are refused;
//! - `phones.json` holds no token, and the computer's paired phones are
//!   the model's, across its restarts;
//! - the computer says a phone left exactly when its last connection
//!   closed.
//!
//! Some of this does not hold yet, until the CRDT sync of ADR 0047: a wall
//! clock moved back reuses numbers on either side, since both number
//! from it; a phone revoked and paired again sends its unsent requests
//! as a new device; and the phone app loses its unsent requests when it
//! restarts. The ignored regressions at the end run each strictly. The
//! default model accepts exactly those cases, where it predicts them,
//! and counts them.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    future::Future,
    net::SocketAddr,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use futures_util::{SinkExt as _, StreamExt as _};
use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_remote::{
    Address,
    ClientError,
    Clock,
    Credentials,
    Down,
    Fingerprint,
    PairingSecret,
    Refusal,
    Sender,
    Server,
    ServerConfig,
    ServerEvent,
    ServerHandle,
    SystemClock,
    client,
    clock::count_from,
    devices::SECRET_LIFETIME,
    outbox::Outbox,
    wire::{Answer, Hello, VERSION},
};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{
        TcpListener,
        TcpStream,
        tcp::{OwnedReadHalf, OwnedWriteHalf},
    },
    runtime::Runtime,
    sync::{
        mpsc::{self, UnboundedReceiver},
        watch,
    },
    task::AbortHandle,
    time::timeout,
};
use tokio_rustls::{
    TlsConnector,
    rustls::{
        self,
        ClientConfig,
        DigitallySignedStruct,
        SignatureScheme,
        client::danger::{
            HandshakeSignatureValid,
            ServerCertVerified,
            ServerCertVerifier,
        },
        crypto::{CryptoProvider, ring},
        pki_types::{CertificateDer, ServerName, UnixTime},
    },
};
use tokio_tungstenite::{client_async, tungstenite::Message};

/// How long anything may take to arrive before the test fails.
const WAIT: Duration = Duration::from_secs(2);
/// How far a clock moves at once.
const JUMPS: [Duration; 4] = [
    Duration::from_millis(1),
    Duration::from_secs(1),
    Duration::from_secs(60),
    Duration::from_secs(86_400),
];
/// What phones call themselves: the same name twice, and one with only
/// spaces, which the computer names for them.
const NAMES: [&str; 3] = [" Pixel ", "Pixel", "  "];

fn block<F: Future>(rt: &Runtime, future: F) -> F::Output {
    rt.block_on(future)
}

/// `future`'s output, if it comes within [`WAIT`].
fn within<F: Future>(rt: &Runtime, future: F) -> Result<F::Output, ()> {
    rt.block_on(async { timeout(WAIT, future).await.map_err(drop) })
}

/// A clock the test moves: the wall clock either way, the monotonic one
/// forward only.
struct FakeClock {
    at: Mutex<(SystemTime, Instant)>,
}

impl FakeClock {
    fn new() -> Self {
        Self {
            at: Mutex::new((
                UNIX_EPOCH + Duration::from_secs(1_800_000_000),
                Instant::now(),
            )),
        }
    }

    /// Time passes: both clocks move.
    fn passes(&self, by: Duration) {
        let mut at = self.at.lock().unwrap();
        at.0 += by;
        at.1 += by;
    }

    /// Only the wall clock moves.
    fn jumps(&self, forward: bool, by: Duration) {
        let mut at = self.at.lock().unwrap();
        at.0 = if forward { at.0 + by } else { at.0 - by };
    }
}

impl Clock for FakeClock {
    fn now(&self) -> SystemTime {
        self.at.lock().unwrap().0
    }

    fn instant(&self) -> Instant {
        self.at.lock().unwrap().1
    }
}

// The proxy between the phones and the computer.

/// What the proxy does to one connection.
#[derive(Debug, Clone, Copy, Default)]
struct Faults {
    /// Holds what the phone sends.
    hold_up: bool,
    /// Holds what the computer sends.
    hold_down: bool,
    /// Passes half of the next bytes up, then cuts.
    half_up: bool,
    /// Passes half of the next bytes down, then cuts.
    half_down: bool,
    cut: bool,
}

impl Faults {
    fn held(&self, up: bool) -> bool {
        if up { self.hold_up } else { self.hold_down }
    }

    fn half(&self, up: bool) -> bool {
        if up { self.half_up } else { self.half_down }
    }
}

/// One connection through the proxy.
struct Link {
    faults: watch::Sender<Faults>,
}

impl Link {
    fn set(&self, change: impl FnOnce(&mut Faults)) {
        self.faults.send_modify(change);
    }

    fn cut(&self) {
        self.set(|faults| faults.cut = true);
    }
}

struct Proxy {
    address: Address,
    /// Where it sends the next connection.
    target: Arc<Mutex<SocketAddr>>,
    /// Each connection, as it comes.
    links: UnboundedReceiver<Arc<Link>>,
}

async fn proxy(target: SocketAddr) -> Proxy {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = Address {
        host: "127.0.0.1".into(),
        port: listener.local_addr().unwrap().port(),
    };
    let target = Arc::new(Mutex::new(target));
    let (found, links) = mpsc::unbounded_channel();
    let to = target.clone();
    tokio::spawn(async move {
        loop {
            let Ok((phone, _)) = listener.accept().await else {
                return;
            };
            let target = *to.lock().unwrap();
            let link = Arc::new(Link {
                faults: watch::channel(Faults::default()).0,
            });
            if found.send(link.clone()).is_err() {
                return;
            }
            tokio::spawn(async move {
                let Ok(computer) = TcpStream::connect(target).await else {
                    return;
                };
                let _ = phone.set_nodelay(true);
                let _ = computer.set_nodelay(true);
                let (phone_read, phone_write) = phone.into_split();
                let (computer_read, computer_write) = computer.into_split();
                tokio::join!(
                    pump(phone_read, computer_write, link.clone(), true),
                    pump(computer_read, phone_write, link, false),
                );
            });
        }
    });
    Proxy {
        address,
        target,
        links,
    }
}

/// Copies one way, holding, halving or cutting as the link says. An
/// end is held like bytes are; either way ending ends both.
async fn pump(
    mut from: OwnedReadHalf,
    mut to: OwnedWriteHalf,
    link: Arc<Link>,
    up: bool,
) {
    let mut faults = link.faults.subscribe();
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        let read = tokio::select! {
            read = from.read(&mut buffer) => read.unwrap_or(0),
            _ = faults.wait_for(|faults| faults.cut) => break,
        };
        let now = match faults
            .wait_for(|faults| faults.cut || !faults.held(up))
            .await
        {
            Ok(now) => *now,
            Err(_) => break,
        };
        if now.cut || read == 0 {
            break;
        }
        if now.half(up) {
            let _ = to.write_all(&buffer[..read / 2]).await;
            let _ = to.flush().await;
            break;
        }
        let wrote = tokio::select! {
            wrote = to.write_all(&buffer[..read]) => wrote.is_ok(),
            _ = faults.wait_for(|faults| faults.cut) => false,
        };
        if !wrote {
            break;
        }
    }
    link.cut();
}

// A phone speaking an older protocol: TLS and the WebSocket by hand.

/// Trusts any certificate: the test only wants the computer's answer.
#[derive(Debug)]
struct AnyCertificate(Arc<CryptoProvider>);

impl ServerCertVerifier for AnyCertificate {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// Says `hello` to the computer at `address`; its answer.
async fn say(address: &Address, hello: &Hello) -> Answer {
    let tcp = TcpStream::connect((address.host.as_str(), address.port))
        .await
        .unwrap();
    let provider = Arc::new(ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AnyCertificate(provider)))
        .with_no_client_auth();
    let tls = TlsConnector::from(Arc::new(config))
        .connect(ServerName::try_from("tau").unwrap(), tcp)
        .await
        .unwrap();
    let (mut socket, _) = client_async("wss://tau/", tls).await.unwrap();
    socket
        .send(Message::text(serde_json::to_string(hello).unwrap()))
        .await
        .unwrap();
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => {
                return serde_json::from_str(&text).unwrap();
            }
            Some(Ok(_)) => {}
            other => panic!("no answer to an older hello: {other:?}"),
        }
    }
}

// The model.

struct Computer {
    handle: ServerHandle,
    events: UnboundedReceiver<ServerEvent>,
}

async fn computer(dir: &Path, clock: Arc<dyn Clock>) -> Computer {
    let (handle, events) = Server::start(ServerConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        dir: dir.to_owned(),
        host: "desk".into(),
        clock,
    })
    .await
    .unwrap();
    Computer { handle, events }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Secret {
    Open,
    Used,
    /// A newer code took its place, the computer closed it, or it
    /// restarted.
    Replaced,
}

struct Opened {
    secret: PairingSecret,
    at: Instant,
    state: Secret,
}

/// A phone's connection.
struct Live {
    sender: Sender,
    downs: UnboundedReceiver<Down>,
    reader: AbortHandle,
    link: Arc<Link>,
    device: String,
    /// The proxy holds it both ways.
    held: bool,
    /// The computer still has it: not revoked, not restarted since.
    on_computer: bool,
    /// The first marker sent while it was there.
    since_marker: u64,
}

impl Drop for Live {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

/// A connection the phone gave up on while the proxy held it: the
/// computer has it until the proxy lets it go.
struct Away {
    link: Arc<Link>,
    device: String,
    on_computer: bool,
}

struct Phone {
    name: &'static str,
    /// Its wall clock.
    wall: SystemTime,
    /// Kept across the app's restarts.
    credentials: Option<Credentials>,
    /// Lost with the app's restart, as `remote.rs` keeps it.
    outbox: Outbox,
    last_seq: Option<u64>,
    /// The computer's run that numbered `last_seq`.
    last_run: Option<u32>,
    /// The computer's log as far as the phone has it.
    seen: Vec<u64>,
    /// A snapshot is owed on the live connection.
    waiting: bool,
    /// It resumed from a number another run of the computer gave, and
    /// so shows what it should not, until its next snapshot (known).
    misled: bool,
    last_marker: Option<u64>,
    live: Option<Live>,
}

/// What came while waiting.
enum Arrival {
    Event(ServerEvent),
    Down(Down),
    Closed,
    Nothing,
}

struct Machine {
    /// Fails on the clock cases the model otherwise counts as known.
    strict: bool,
    clock: Arc<FakeClock>,
    computer: Computer,
    proxy: Proxy,
    impostor: Computer,
    impostor_at: SocketAddr,
    fingerprint: Fingerprint,
    phones: Vec<Phone>,
    /// What the computer sent, kept, in order; message `n` is `n`.
    log: Vec<u64>,
    /// The computer's runs: this one's number, its first and last
    /// message numbers.
    run: u32,
    run_first: u64,
    run_seq: u64,
    secrets: Vec<Opened>,
    /// The paired phones, as the computer must list them.
    paired: BTreeMap<String, String>,
    /// Every device ever paired, revoked ones too.
    devices: Vec<String>,
    /// Every token handed out, which `phones.json` must never hold.
    tokens: Vec<String>,
    /// Each request: who made it and its number.
    made: HashMap<u64, (usize, u64)>,
    /// The requests the computer took, and from which device.
    taken: HashMap<u64, String>,
    /// The last request number taken from each device, as the computer
    /// keeps it.
    last_up: HashMap<String, u64>,
    /// Requests the computer said it took and did not (known: clock).
    vanished: Vec<u64>,
    /// Requests taken again after their phone paired again (known).
    retaken: Vec<u64>,
    /// Requests lost with an app restart before they were sent.
    dropped: Vec<u64>,
    next_request: u64,
    /// The phones the computer has connected: welcomed, not left since.
    present: BTreeSet<String>,
    away: Vec<Away>,
    next_marker: u64,
    dir: tempfile::TempDir,
    _impostor_dir: tempfile::TempDir,
    rt: Runtime,
}

impl Machine {
    fn new(phones: usize, strict: bool) -> Self {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let impostor_dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(FakeClock::new());
        let (computer, impostor, proxy) = block(&rt, async {
            let computer = computer(dir.path(), clock.clone()).await;
            let impostor =
                self::computer(impostor_dir.path(), Arc::new(SystemClock))
                    .await;
            let proxy = proxy(computer.handle.local_addr()).await;
            (computer, impostor, proxy)
        });
        let run_first = count_from(clock.now());
        let wall = clock.now();
        Self {
            strict,
            fingerprint: computer.handle.fingerprint(),
            impostor_at: impostor.handle.local_addr(),
            clock,
            computer,
            proxy,
            impostor,
            phones: (0..phones)
                .map(|i| Phone {
                    name: NAMES[i % NAMES.len()],
                    wall,
                    credentials: None,
                    outbox: Outbox::new(count_from(wall)),
                    last_seq: None,
                    last_run: None,
                    seen: Vec::new(),
                    waiting: false,
                    misled: false,
                    last_marker: None,
                    live: None,
                })
                .collect(),
            log: Vec::new(),
            run: 0,
            run_first,
            run_seq: run_first,
            secrets: Vec::new(),
            paired: BTreeMap::new(),
            devices: Vec::new(),
            tokens: Vec::new(),
            made: HashMap::new(),
            taken: HashMap::new(),
            last_up: HashMap::new(),
            vanished: Vec::new(),
            retaken: Vec::new(),
            dropped: Vec::new(),
            next_request: 0,
            present: BTreeSet::new(),
            away: Vec::new(),
            next_marker: 0,
            dir,
            _impostor_dir: impostor_dir,
            rt,
        }
    }

    // Arrivals.

    fn on_event(&mut self, event: ServerEvent) {
        match event {
            ServerEvent::NeedSnapshot { conn, .. } => {
                self.computer.handle.snapshot(conn, vec![json!(self.log)]);
            }
            ServerEvent::Up { device, body, .. } => {
                let request = body.as_u64().unwrap();
                let (_, id) = self.made[&request];
                if let Some(earlier) =
                    self.taken.insert(request, device.id.clone())
                {
                    // Known: the phone was revoked and paired again, and
                    // the new device has no record of what the old took.
                    let repaired = earlier != device.id
                        && !self.paired.contains_key(&earlier);
                    assert!(
                        repaired && !self.strict,
                        "request {request} (number {id}) taken twice, from \
                         {earlier} then {}",
                        device.id
                    );
                    self.retaken.push(request);
                }
                let last = self.last_up.entry(device.id).or_default();
                *last = (*last).max(id);
            }
            ServerEvent::DevicesChanged(_) => {}
            ServerEvent::Left { device } => {
                assert_eq!(
                    self.on_computer(&device),
                    0,
                    "the computer says {device} left while it is connected"
                );
                assert!(
                    self.present.remove(&device),
                    "the computer says {device} left, which it did not have"
                );
            }
        }
    }

    fn on_down(&mut self, i: usize, down: Down) {
        match down {
            Down::Ack { up } => {
                // The computer says what it took before it answers.
                self.drain();
                let device =
                    self.phones[i].live.as_ref().unwrap().device.clone();
                let acked: Vec<(u64, u64)> = self.phones[i]
                    .outbox
                    .pending()
                    .map(|tau_remote::Up::Up { id, body }| {
                        (id, body.as_u64().unwrap())
                    })
                    .take_while(|(id, _)| *id <= up)
                    .collect();
                for (id, request) in acked {
                    if self.taken.contains_key(&request) {
                        continue;
                    }
                    let below = self
                        .last_up
                        .get(&device)
                        .is_some_and(|last| id <= *last);
                    assert!(
                        below && !self.strict,
                        "the computer said it took request {request} (number \
                         {id}) of phone {i}, and did not; it had taken up to \
                         {:?} from {device}",
                        self.last_up.get(&device)
                    );
                    self.vanished.push(request);
                }
                self.phones[i].outbox.taken(up);
            }
            Down::Message { seq, body } => {
                let phone = &mut self.phones[i];
                assert!(!phone.waiting, "a message before the snapshot");
                assert_eq!(
                    Some(seq),
                    phone.last_seq.map(|last| last + 1),
                    "phone {i}: a message out of order, repeated, or after a gap"
                );
                if !phone.misled {
                    assert_eq!(
                        body,
                        json!(phone.seen.len()),
                        "phone {i}: not the computer's next message"
                    );
                }
                phone.seen.push(body.as_u64().unwrap());
                phone.last_seq = Some(seq);
            }
            Down::Snapshot { seq, bodies } => {
                let phone = &mut self.phones[i];
                assert!(phone.waiting, "phone {i}: a snapshot not asked for");
                let [log]: [Value; 1] = bodies.try_into().unwrap();
                phone.seen = serde_json::from_value(log).unwrap();
                assert_eq!(phone.seen[..], self.log[..phone.seen.len()]);
                phone.last_seq = Some(seq);
                phone.last_run = Some(self.run);
                phone.waiting = false;
                phone.misled = false;
            }
            Down::Transient { body } => {
                let marker = body["marker"].as_u64().unwrap();
                let phone = &mut self.phones[i];
                let live = phone.live.as_ref().unwrap();
                assert!(
                    marker >= live.since_marker,
                    "phone {i} got a transient message sent before it came"
                );
                assert!(
                    phone.last_marker.is_none_or(|last| marker > last),
                    "phone {i}: a transient message twice or out of order"
                );
                phone.last_marker = Some(marker);
            }
        }
    }

    /// What arrived already, from the computer.
    fn drain(&mut self) {
        while let Ok(event) = self.computer.events.try_recv() {
            self.on_event(event);
        }
    }

    /// Waits for the computer to say something, or phone `i`.
    fn arrival(&mut self, i: Option<usize>) -> Arrival {
        let events = &mut self.computer.events;
        let downs = i.map(|i| &mut self.phones[i].live.as_mut().unwrap().downs);
        block(&self.rt, async {
            tokio::select! {
                biased;
                event = events.recv() => {
                    Arrival::Event(event.expect("the computer runs"))
                }
                down = async {
                    match downs {
                        Some(downs) => downs.recv().await,
                        None => std::future::pending().await,
                    }
                } => down.map_or(Arrival::Closed, Arrival::Down),
                () = tokio::time::sleep(WAIT) => Arrival::Nothing,
            }
        })
    }

    /// Takes in what phone `i` gets until its connection closes.
    fn until_closed(&mut self, i: usize) {
        loop {
            match self.arrival(Some(i)) {
                Arrival::Event(event) => self.on_event(event),
                Arrival::Down(down) => self.on_down(i, down),
                Arrival::Closed => break,
                Arrival::Nothing => {
                    panic!("phone {i}'s connection never closed")
                }
            }
        }
        self.phones[i].live = None;
        self.phones[i].waiting = false;
    }

    /// The connections the computer has from `device`, as the model
    /// counts them.
    fn on_computer(&self, device: &str) -> usize {
        let live = self
            .phones
            .iter()
            .filter_map(|phone| phone.live.as_ref())
            .filter(|live| live.on_computer && live.device == device)
            .count();
        let away = self
            .away
            .iter()
            .filter(|away| away.on_computer && away.device == device)
            .count();
        live + away
    }

    /// The computer must say `device` left, now its last connection
    /// closed.
    fn expect_left(&mut self, device: &str) {
        if self.on_computer(device) > 0 || !self.present.contains(device) {
            return;
        }
        while self.present.contains(device) {
            match self.arrival(None) {
                Arrival::Event(event) => self.on_event(event),
                _ => panic!("the computer never said {device} left"),
            }
        }
    }

    /// Phone `i`'s connection closed: the computer must say so.
    fn closed(&mut self, i: usize) {
        let live = self.phones[i].live.as_mut().unwrap();
        // The computer may see it close before the phone does.
        live.on_computer = false;
        let device = live.device.clone();
        self.until_closed(i);
        self.expect_left(&device);
    }

    fn next_link(&mut self) -> Arc<Link> {
        within(&self.rt, self.proxy.links.recv())
            .expect("the proxy saw the phone")
            .unwrap()
    }

    /// Phone `i` connected: it sends what the computer has not taken,
    /// as the app does.
    fn connected(&mut self, i: usize, connection: client::Connection) {
        let link = self.next_link();
        let device = connection.device().to_owned();
        let sender = connection.sender();
        let (to, downs) = mpsc::unbounded_channel();
        let reader = self
            .rt
            .spawn(async move {
                let mut connection = connection;
                while let Ok(Some(down)) = connection.recv().await {
                    if to.send(down).is_err() {
                        return;
                    }
                }
            })
            .abort_handle();
        self.present.insert(device.clone());
        let phone = &mut self.phones[i];
        for up in phone.outbox.pending() {
            sender.send(up).unwrap();
        }
        phone.live = Some(Live {
            sender,
            downs,
            reader,
            link,
            device,
            held: false,
            on_computer: true,
            since_marker: self.next_marker,
        });
    }

    /// Lets everything that can arrive arrive.
    fn settle(&mut self) {
        self.drain();
        let open =
            |phone: &Phone| phone.live.as_ref().is_some_and(|live| !live.held);
        for i in 0..self.phones.len() {
            while open(&self.phones[i])
                && (self.phones[i].waiting || !self.phones[i].outbox.is_empty())
            {
                match self.arrival(Some(i)) {
                    Arrival::Event(event) => self.on_event(event),
                    Arrival::Down(down) => self.on_down(i, down),
                    Arrival::Closed => {
                        panic!("phone {i}'s connection closed by itself")
                    }
                    Arrival::Nothing => panic!(
                        "phone {i} never settled: snapshot owed {}, {} unsent",
                        self.phones[i].waiting,
                        self.phones[i].outbox.len()
                    ),
                }
            }
        }
        // A transient message after everything: once it arrives, all
        // before it has.
        let marker = self.next_marker;
        self.next_marker += 1;
        self.computer
            .handle
            .broadcast_transient(json!({ "marker": marker }));
        for i in 0..self.phones.len() {
            while open(&self.phones[i])
                && self.phones[i].last_marker.is_none_or(|last| last < marker)
            {
                match self.arrival(Some(i)) {
                    Arrival::Event(event) => self.on_event(event),
                    Arrival::Down(down) => self.on_down(i, down),
                    Arrival::Closed => {
                        panic!("phone {i}'s connection closed by itself")
                    }
                    Arrival::Nothing => {
                        panic!("phone {i} never got the marker")
                    }
                }
            }
        }
        self.drain();
    }

    // What happens, each settled at the end.

    fn open_code(&mut self) {
        let code = self
            .computer
            .handle
            .pairing_code(self.proxy.address.clone());
        assert_eq!(code.fingerprint, self.fingerprint);
        for opened in &mut self.secrets {
            if opened.state == Secret::Open {
                opened.state = Secret::Replaced;
            }
        }
        self.secrets.push(Opened {
            secret: code.secret,
            at: self.clock.instant(),
            state: Secret::Open,
        });
    }

    fn close_code(&mut self) {
        self.computer.handle.close_pairing();
        for opened in &mut self.secrets {
            if opened.state == Secret::Open {
                opened.state = Secret::Replaced;
            }
        }
    }

    /// Phone `i` pairs with secret `pick`, or one never shown if none.
    fn pair(&mut self, i: usize, pick: Option<usize>) {
        let phone = &self.phones[i];
        assert!(phone.credentials.is_none() && phone.live.is_none());
        let name = phone.name;
        let (secret, good) = match pick {
            Some(k) => {
                let opened = &self.secrets[k];
                let fresh =
                    self.clock.instant().saturating_duration_since(opened.at)
                        < SECRET_LIFETIME;
                (opened.secret.clone(), opened.state == Secret::Open && fresh)
            }
            None => (PairingSecret::random(), false),
        };
        let paired = block(
            &self.rt,
            client::pair(&self.proxy.address, self.fingerprint, &secret, name),
        );
        match paired {
            Ok((connection, credentials)) => {
                assert!(
                    good,
                    "a stale, used, replaced or unknown secret paired"
                );
                let opened = &mut self.secrets[pick.unwrap()];
                opened.state = Secret::Used;
                let trimmed = name.trim();
                let shown = if trimmed.is_empty() { "Phone" } else { trimmed };
                assert!(!self.paired.contains_key(&credentials.device));
                self.paired
                    .insert(credentials.device.clone(), shown.to_owned());
                self.devices.push(credentials.device.clone());
                self.tokens.push(credentials.token.clone());
                self.phones[i].credentials = Some(credentials);
                // A phone that pairs needs everything.
                self.phones[i].waiting = true;
                self.connected(i, connection);
            }
            Err(ClientError::Refused(Refusal::BadSecret)) => {
                assert!(!good, "the open secret was refused");
                self.next_link();
            }
            Err(error) => panic!("pairing failed: {error}"),
        }
    }

    /// Phone `i` connects again with its token.
    fn resume(&mut self, i: usize) {
        let phone = &self.phones[i];
        assert!(phone.live.is_none());
        let credentials = phone.credentials.clone().unwrap();
        let last = phone.last_seq;
        let resumed = block(&self.rt, client::resume(&credentials, last));
        match resumed {
            Ok(connection) => {
                assert!(
                    self.paired.contains_key(&credentials.device),
                    "a revoked token was taken"
                );
                assert_eq!(connection.device(), credentials.device);
                // The feed has what it missed only if this run of the
                // computer numbered what it had.
                let ours = self.phones[i].last_run == Some(self.run);
                let in_range = last.is_some_and(|last| {
                    last == self.run_seq
                        || (self.run_first <= last && last < self.run_seq)
                });
                if connection.resumed() && !ours {
                    assert!(
                        in_range && !self.strict,
                        "phone {i} resumed from number {last:?}, which an \
                         earlier run of the computer gave (this run numbers \
                         from {} to {})",
                        self.run_first,
                        self.run_seq
                    );
                    self.phones[i].misled = true;
                } else {
                    assert_eq!(
                        connection.resumed(),
                        ours,
                        "phone {i} resumed from {last:?}: whether it is sent \
                         what it missed"
                    );
                }
                self.phones[i].last_run = Some(self.run);
                self.phones[i].waiting = !connection.resumed();
                self.connected(i, connection);
            }
            Err(ClientError::Refused(Refusal::UnknownToken)) => {
                assert!(
                    !self.paired.contains_key(&credentials.device),
                    "a paired phone's token was refused"
                );
                // The app forgets the computer and offers to pair again.
                self.phones[i].credentials = None;
                self.next_link();
            }
            Err(error) => panic!("phone {i} could not connect: {error}"),
        }
    }

    /// Phone `i` asks something; with `half`, the proxy cuts in the
    /// middle of it.
    fn request(&mut self, i: usize, half: bool) {
        let request = self.next_request;
        self.next_request += 1;
        let phone = &mut self.phones[i];
        let up = phone.outbox.push(json!(request));
        let tau_remote::Up::Up { id, .. } = &up;
        self.made.insert(request, (i, *id));
        let Some(live) = &phone.live else { return };
        if half {
            live.link.set(|faults| faults.half_up = true);
        }
        // A closed connection is found when the phone reads it.
        let _ = live.sender.send(up);
        if half {
            self.closed(i);
        }
    }

    /// The computer takes phone `i`'s next request, and its answer is
    /// lost with the connection.
    fn answer_lost(&mut self, i: usize) {
        let live = self.phones[i].live.as_ref().unwrap();
        live.link.set(|faults| faults.hold_down = true);
        let device = live.device.clone();
        let request = self.next_request;
        self.request(i, false);
        let (_, id) = self.made[&request];
        // Known: one numbered at or below the last it took, after the
        // phone's clock went back, the computer does not take.
        let stale = self.last_up.get(&device).is_some_and(|last| id <= *last);
        while !stale && !self.taken.contains_key(&request) {
            match self.arrival(None) {
                Arrival::Event(event) => self.on_event(event),
                _ => panic!("request {request} never reached the computer"),
            }
        }
        self.phones[i].live.as_ref().unwrap().link.cut();
        self.closed(i);
    }

    /// The computer sends a kept message; with `half`, phone `i`'s
    /// connection is cut in the middle of it.
    fn broadcast(&mut self, half: Option<usize>) {
        if let Some(i) = half {
            let live = self.phones[i].live.as_ref().unwrap();
            live.link.set(|faults| faults.half_down = true);
        }
        let message = self.log.len() as u64;
        self.log.push(message);
        self.run_seq += 1;
        self.computer.handle.broadcast(json!(message));
        if let Some(i) = half {
            self.closed(i);
        }
    }

    fn cut(&mut self, i: usize) {
        self.phones[i].live.as_ref().unwrap().link.cut();
        self.closed(i);
    }

    fn hold(&mut self, i: usize) {
        let live = self.phones[i].live.as_mut().unwrap();
        live.link.set(|faults| {
            faults.hold_up = true;
            faults.hold_down = true;
        });
        live.held = true;
    }

    fn release(&mut self, i: usize) {
        let live = self.phones[i].live.as_mut().unwrap();
        live.link.set(|faults| {
            faults.hold_up = false;
            faults.hold_down = false;
        });
        live.held = false;
        if !live.on_computer {
            // What the computer said while held was goodbye.
            self.until_closed(i);
        }
    }

    /// Phone `i` gives up on a held connection, which the computer
    /// keeps until the proxy lets it go; the app connects again.
    fn give_up(&mut self, i: usize) {
        let live = self.phones[i].live.take().unwrap();
        self.phones[i].waiting = false;
        self.away.push(Away {
            link: live.link.clone(),
            device: live.device.clone(),
            on_computer: live.on_computer,
        });
        if self.phones[i].credentials.is_some() {
            self.resume(i);
        }
    }

    /// A connection given up on is let go.
    fn let_go(&mut self, k: usize) {
        let away = self.away.remove(k);
        away.link.cut();
        if away.on_computer {
            self.expect_left(&away.device);
        }
    }

    /// The computer revokes device `k`; with `asking`, while the phone
    /// sends a request.
    fn revoke(&mut self, k: usize, asking: bool) {
        let device = self.devices[k].clone();
        let paired = self.paired.contains_key(&device);
        let asker = self.phones.iter().position(|phone| {
            open(phone)
                && phone
                    .live
                    .as_ref()
                    .is_some_and(|live| live.device == device)
        });
        if let (true, Some(i)) = (asking, asker) {
            self.request(i, false);
        }
        let revoked =
            block(&self.rt, self.computer.handle.revoke(&device)).unwrap();
        assert_eq!(revoked, paired, "revoking {device}");
        if !revoked {
            return;
        }
        self.paired.remove(&device);
        for away in &mut self.away {
            if away.device == device {
                away.on_computer = false;
            }
        }
        let mut cut = Vec::new();
        for (i, phone) in self.phones.iter_mut().enumerate() {
            if let Some(live) = &mut phone.live
                && live.device == device
            {
                live.on_computer = false;
                if !live.held {
                    cut.push(i);
                }
            }
        }
        // A revoked phone's connections close.
        for i in cut {
            self.until_closed(i);
        }
        self.expect_left(&device);
    }

    fn restart_computer(&mut self) {
        let placeholder = Computer {
            handle: self.impostor.handle.clone(),
            events: mpsc::unbounded_channel().1,
        };
        let Computer { handle, mut events } =
            std::mem::replace(&mut self.computer, placeholder);
        handle.stop();
        drop(handle);
        // Stopped once it has said everything.
        while let Some(event) =
            within(&self.rt, events.recv()).expect("the computer stops")
        {
            if let ServerEvent::Up { .. } = event {
                self.on_event(event);
            }
        }
        // Restarting takes a moment.
        self.clock.passes(Duration::from_secs(1));
        self.computer =
            block(&self.rt, computer(self.dir.path(), self.clock.clone()));
        assert_eq!(self.computer.handle.fingerprint(), self.fingerprint);
        *self.proxy.target.lock().unwrap() = self.computer.handle.local_addr();
        self.run += 1;
        self.run_first = count_from(self.clock.now());
        self.run_seq = self.run_first;
        self.present.clear();
        for opened in &mut self.secrets {
            if opened.state == Secret::Open {
                opened.state = Secret::Replaced;
            }
        }
        for away in &mut self.away {
            away.on_computer = false;
        }
        let mut cut = Vec::new();
        for (i, phone) in self.phones.iter_mut().enumerate() {
            if let Some(live) = &mut phone.live {
                live.on_computer = false;
                if !live.held {
                    cut.push(i);
                }
            }
        }
        for i in cut {
            self.until_closed(i);
        }
    }

    /// The phone app quits and starts again: it keeps its credentials,
    /// and loses the rest.
    fn restart_phone(&mut self, i: usize) {
        let phone = &mut self.phones[i];
        let pending: Vec<u64> = phone
            .outbox
            .pending()
            .map(|tau_remote::Up::Up { body, .. }| body.as_u64().unwrap())
            .collect();
        assert!(
            pending.is_empty() || !self.strict,
            "phone {i}'s requests {pending:?} are lost with the app"
        );
        phone.wall += Duration::from_secs(1);
        phone.outbox = Outbox::new(count_from(phone.wall));
        phone.last_seq = None;
        phone.last_run = None;
        phone.seen.clear();
        phone.waiting = false;
        phone.misled = false;
        phone.last_marker = None;
        self.dropped.extend(pending);
        if let Some(live) = self.phones[i].live.take() {
            if live.held {
                self.away.push(Away {
                    link: live.link.clone(),
                    device: live.device.clone(),
                    on_computer: live.on_computer,
                });
            } else {
                let device = live.device.clone();
                let on_computer = live.on_computer;
                live.link.cut();
                drop(live);
                if on_computer {
                    self.expect_left(&device);
                }
            }
        }
    }

    /// Phone `i` speaks protocol `version`, older than the computer's.
    fn older_version(&mut self, i: usize, version: u32) {
        let open = self
            .secrets
            .iter()
            .find(|opened| opened.state == Secret::Open)
            .map(|opened| opened.secret.to_string());
        let hello = match (&self.phones[i].credentials, open) {
            (Some(credentials), _) => Hello::Resume {
                version,
                token: credentials.token.clone(),
                last_seq: self.phones[i].last_seq,
            },
            (None, secret) => Hello::Pair {
                version,
                secret: secret.unwrap_or_else(|| "22222222".into()),
                name: "Old".into(),
            },
        };
        let answer = block(&self.rt, say(&self.proxy.address, &hello));
        assert_eq!(
            answer,
            Answer::Refused {
                reason: Refusal::Version { speaks: VERSION }
            }
        );
        self.next_link();
    }

    /// Another computer answers in this one's place: phone `i` must not
    /// talk to it.
    fn impostor(&mut self, i: usize) {
        *self.proxy.target.lock().unwrap() = self.impostor_at;
        let phone = &self.phones[i];
        let tried = match &phone.credentials {
            Some(credentials) => {
                block(&self.rt, client::resume(credentials, phone.last_seq))
                    .map(drop)
            }
            None => {
                let secret = self
                    .secrets
                    .last()
                    .map_or_else(PairingSecret::random, |opened| {
                        opened.secret.clone()
                    });
                block(
                    &self.rt,
                    client::pair(
                        &self.proxy.address,
                        self.fingerprint,
                        &secret,
                        phone.name,
                    ),
                )
                .map(drop)
            }
        };
        *self.proxy.target.lock().unwrap() = self.computer.handle.local_addr();
        let Err(ClientError::CertificateMismatch { found }) = tried else {
            panic!("another computer's certificate was taken: {tried:?}");
        };
        assert_eq!(found, self.impostor.handle.fingerprint());
        self.next_link();
        assert!(
            self.impostor.events.try_recv().is_err(),
            "the impostor heard the phone"
        );
    }

    fn phone(&self, tc: &TestCase, fits: impl Fn(&Phone) -> bool) -> usize {
        let fit: Vec<usize> = (0..self.phones.len())
            .filter(|i| fits(&self.phones[*i]))
            .collect();
        if fit.is_empty() {
            tc.reject();
        }
        tc.draw(gs::sampled_from(fit))
    }
}

fn idle(phone: &Phone) -> bool {
    phone.live.is_none()
}

fn open(phone: &Phone) -> bool {
    phone.live.as_ref().is_some_and(|live| !live.held)
}

fn held(phone: &Phone) -> bool {
    phone.live.as_ref().is_some_and(|live| live.held)
}

#[hegel::state_machine]
impl Machine {
    #[rule]
    fn computer_opens_a_code(&mut self, _tc: TestCase) {
        self.open_code();
        self.settle();
    }

    #[rule]
    fn computer_closes_the_code(&mut self, _tc: TestCase) {
        self.close_code();
        self.settle();
    }

    #[rule(weight = 3)]
    fn phone_pairs(&mut self, tc: TestCase) {
        let i =
            self.phone(&tc, |phone| idle(phone) && phone.credentials.is_none());
        // Mostly as a person does: the computer shows a new code, and
        // the phone reads it.
        let pick = if tc.draw(gs::integers::<u8>().max_value(2)) > 0 {
            self.open_code();
            Some(self.secrets.len() - 1)
        } else {
            // The last code shown (maybe used, stale or closed), any
            // earlier one, or one never shown.
            let k: usize = tc.draw(
                gs::integers().min_value(0).max_value(self.secrets.len()),
            );
            match self.secrets.len() {
                0 => None,
                shown if tc.draw(gs::booleans()) => Some(shown - 1),
                shown => (k < shown).then_some(k),
            }
        };
        self.pair(i, pick);
        self.settle();
    }

    #[rule(weight = 3)]
    fn phone_connects(&mut self, tc: TestCase) {
        let i =
            self.phone(&tc, |phone| idle(phone) && phone.credentials.is_some());
        self.resume(i);
        self.settle();
    }

    #[rule(weight = 4)]
    fn phone_asks(&mut self, tc: TestCase) {
        let i = self.phone(&tc, |_| true);
        let half = open(&self.phones[i]) && tc.draw(gs::booleans());
        self.request(i, half);
        self.settle();
    }

    #[rule(weight = 2)]
    fn computers_answer_is_lost(&mut self, tc: TestCase) {
        let i = self.phone(&tc, open);
        self.answer_lost(i);
        self.settle();
    }

    #[rule(weight = 4)]
    fn computer_sends(&mut self, tc: TestCase) {
        let half = if tc.draw(gs::booleans()) {
            let fit: Vec<usize> = (0..self.phones.len())
                .filter(|i| open(&self.phones[*i]) && !self.phones[*i].waiting)
                .collect();
            (!fit.is_empty()).then(|| tc.draw(gs::sampled_from(fit)))
        } else {
            None
        };
        self.broadcast(half);
        self.settle();
    }

    #[rule]
    fn connection_drops(&mut self, tc: TestCase) {
        let i = self.phone(&tc, |phone| phone.live.is_some());
        self.cut(i);
        self.settle();
    }

    #[rule(weight = 2)]
    fn connection_stalls(&mut self, tc: TestCase) {
        let i = self.phone(&tc, open);
        self.hold(i);
        self.settle();
    }

    #[rule]
    fn stalled_connection_goes_on(&mut self, tc: TestCase) {
        let i = self.phone(&tc, held);
        self.release(i);
        self.settle();
    }

    #[rule(weight = 2)]
    fn phone_gives_up_on_a_stalled_connection(&mut self, tc: TestCase) {
        let i = self.phone(&tc, held);
        self.give_up(i);
        // The computer finds the old one dead after the phone is back.
        if tc.draw(gs::booleans()) {
            self.let_go(self.away.len() - 1);
        }
        self.settle();
    }

    #[rule(weight = 2)]
    fn given_up_connection_dies(&mut self, tc: TestCase) {
        if self.away.is_empty() {
            tc.reject();
        }
        let k =
            tc.draw(gs::integers().min_value(0).max_value(self.away.len() - 1));
        self.let_go(k);
        self.settle();
    }

    #[rule(weight = 2)]
    fn computer_revokes(&mut self, tc: TestCase) {
        if self.devices.is_empty() {
            tc.reject();
        }
        let k = tc.draw(
            gs::integers()
                .min_value(0)
                .max_value(self.devices.len() - 1),
        );
        let asking = tc.draw(gs::booleans());
        self.revoke(k, asking);
        self.settle();
    }

    #[rule(weight = 2)]
    fn computer_restarts(&mut self, _tc: TestCase) {
        self.restart_computer();
        self.settle();
    }

    #[rule]
    fn phone_app_restarts(&mut self, tc: TestCase) {
        let i = self.phone(&tc, |_| true);
        self.restart_phone(i);
        self.settle();
    }

    #[rule]
    fn time_passes_on_the_computer(&mut self, tc: TestCase) {
        let by = tc.draw(gs::sampled_from(vec![
            Duration::from_secs(1),
            SECRET_LIFETIME - Duration::from_secs(1),
            SECRET_LIFETIME,
        ]));
        self.clock.passes(by);
        self.settle();
    }

    #[rule]
    fn computers_clock_moves(&mut self, tc: TestCase) {
        let forward = tc.draw(gs::booleans());
        let by = tc.draw(gs::sampled_from(JUMPS.to_vec()));
        self.clock.jumps(forward, by);
        self.settle();
    }

    #[rule]
    fn phones_clock_moves(&mut self, tc: TestCase) {
        let i = self.phone(&tc, |_| true);
        let forward = tc.draw(gs::booleans());
        let by = tc.draw(gs::sampled_from(JUMPS.to_vec()));
        let wall = &mut self.phones[i].wall;
        *wall = if forward { *wall + by } else { *wall - by };
        self.settle();
    }

    #[rule]
    fn phone_speaks_an_older_protocol(&mut self, tc: TestCase) {
        let i = self.phone(&tc, |_| true);
        let version =
            tc.draw(gs::integers().min_value(0).max_value(VERSION - 1));
        self.older_version(i, version);
        self.settle();
    }

    #[rule]
    fn impostor_answers(&mut self, tc: TestCase) {
        let i = self.phone(&tc, idle);
        self.impostor(i);
        self.settle();
    }

    /// A phone connected, with nothing held, has every message and no
    /// request left.
    #[invariant(always_run)]
    fn connected_phones_have_everything(&self, _tc: TestCase) {
        for (i, phone) in self.phones.iter().enumerate() {
            if !open(phone) {
                continue;
            }
            assert!(!phone.waiting);
            if !phone.misled {
                assert_eq!(phone.seen, self.log, "phone {i} misses messages");
            }
            assert!(phone.outbox.is_empty(), "phone {i} has requests left");
        }
    }

    /// The computer lists the phones paired, and keeps no token.
    #[invariant(always_run)]
    fn the_computer_keeps_the_paired_phones(&self, _tc: TestCase) {
        let listed: BTreeMap<String, String> = self
            .computer
            .handle
            .paired()
            .into_iter()
            .map(|device| (device.id, device.name))
            .collect();
        assert_eq!(listed, self.paired);
        let Ok(text) =
            std::fs::read_to_string(self.dir.path().join("phones.json"))
        else {
            assert!(self.devices.is_empty(), "no phones.json");
            return;
        };
        for token in &self.tokens {
            assert!(
                !text.contains(token.as_str()),
                "phones.json holds a token"
            );
        }
        let kept: Vec<Value> = serde_json::from_str(&text).unwrap();
        for device in kept {
            assert!(device.get("token").is_none(), "phones.json holds a token");
        }
    }

    /// The computer has as connected exactly the phones it has a
    /// connection from.
    #[invariant(always_run)]
    fn the_computer_knows_who_is_connected(&self, _tc: TestCase) {
        let connected: BTreeSet<String> = self
            .devices
            .iter()
            .filter(|device| self.on_computer(device) > 0)
            .cloned()
            .collect();
        assert_eq!(self.present, connected);
    }
}

fn run(tc: TestCase, steps: i64) {
    let phones: usize = tc.draw(gs::integers().min_value(1).max_value(3));
    hegel::stateful::machine(Machine::new(phones, false))
        .steps(steps)
        .run(tc);
}

#[hegel::test(test_cases = 300)]
fn phones_and_the_computer_agree_over_the_network(tc: TestCase) {
    run(tc, 40);
}

#[ignore = "nightly"]
#[hegel::test(profile = "nightly_slow")]
fn phones_and_the_computer_agree_over_the_network_nightly(tc: TestCase) {
    run(tc, 80);
}

// Regressions the model found, as it shrank them. They fail today; the
// CRDT sync of ADR 0047 fixes them.

/// A phone that sent a request.
fn paired_and_asked() -> Machine {
    let mut machine = Machine::new(1, true);
    machine.open_code();
    machine.pair(0, Some(0));
    machine.request(0, false);
    machine.settle();
    machine
}

#[test]
#[ignore = "fixed by the CRDT sync of ADR 0047: a phone's clock moved back \
            numbers its requests at or below the last the computer took, \
            which acks them and never takes them"]
fn a_request_after_the_phones_clock_went_back_is_taken() {
    let mut machine = paired_and_asked();
    // Back by as long as the app takes to start again: its outbox
    // numbers from where the last one did.
    machine.phones[0].wall -= Duration::from_secs(1);
    machine.restart_phone(0);
    machine.request(0, false);
    machine.resume(0);
    machine.settle();
}

#[test]
#[ignore = "fixed by the CRDT sync of ADR 0047: the computer's clock moved \
            back across a restart lets a phone resume from an earlier run's \
            number and be told it missed nothing"]
fn a_phone_from_before_the_computers_clock_went_back_gets_a_snapshot() {
    let mut machine = paired_and_asked();
    machine.broadcast(None);
    machine.settle();
    // Back by as long as the computer takes to start again.
    machine.clock.jumps(false, Duration::from_secs(1));
    machine.restart_computer();
    machine.broadcast(None);
    machine.settle();
    machine.resume(0);
    machine.settle();
}

#[test]
#[ignore = "fixed by the CRDT sync of ADR 0047: a phone revoked and paired \
            again sends its unsent requests as a new device, and one whose \
            answer was lost is taken twice"]
fn a_request_is_not_taken_again_after_pairing_again() {
    let mut machine = paired_and_asked();
    machine.answer_lost(0);
    machine.revoke(0, false);
    machine.resume(0);
    machine.open_code();
    machine.pair(0, Some(1));
    machine.settle();
}

#[test]
#[ignore = "fixed by the CRDT sync of ADR 0047: the phone app loses its \
            unsent requests when it restarts"]
fn a_request_made_while_away_survives_the_apps_restart() {
    let mut machine = paired_and_asked();
    machine.cut(0);
    machine.request(0, false);
    machine.restart_phone(0);
}
