//! The phone's side (decision 0013): in place of a host, a remote pairs
//! with the tau on a computer, applies what that tau's host applies, and
//! sends up what the person does here.
//!
//! The network runs on the remote's own tokio runtime; what it learns
//! comes back to GPUI over a channel, as the host's run events do.

use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, Mutex},
    time::Duration,
};

use gpui::{App, Entity, Task, WeakEntity};
use tau_remote::{
    Address,
    ClientError,
    Credentials,
    Down,
    Fingerprint,
    PairingCode,
    PairingSecret,
    Refusal,
    Sender,
    client,
};
use tokio::{
    runtime::Runtime,
    sync::{Notify, mpsc},
    task::AbortHandle,
};

use crate::{
    pairing::{Computer, PairRequest, PairStep, PairingUpdate, Progress},
    phones::{PhoneUp, reach},
    update::HostUpdate,
    workspace::{Workspace, WorkspaceEvent},
};

/// Tries in a row before the phone says it cannot reach the computer;
/// it keeps trying after.
const TRIES: u32 = 3;

/// What the phone brings that tau-ui cannot do itself.
#[derive(Clone)]
pub struct Platform {
    /// An app-private directory for the phone's credentials.
    pub dir: PathBuf,
    /// Opens the camera and reads a QR code: its text, None if the
    /// person backed out, or what went wrong, in words. Runs off the UI
    /// thread and may block.
    pub scan: Arc<dyn Fn() -> Result<Option<String>, String> + Send + Sync>,
    /// The phone's name, for the computer's list.
    pub name: String,
}

/// What the network learned, for the UI thread.
enum News {
    Pairing(PairingUpdate),
    Apply(HostUpdate),
    /// The camera closed without a code.
    ScanCancelled,
    Paired(Credentials, client::Connection),
    Connected(Sender, Computer),
    Revoked(Credentials),
}

struct State {
    runtime: Runtime,
    platform: Platform,
    news: mpsc::UnboundedSender<News>,
    credentials: Option<Credentials>,
    /// The connection's sender while connected.
    sender: Option<Sender>,
    /// The number of the last message applied, to resume from.
    last_seq: Arc<Mutex<Option<u64>>>,
    /// A typed address and code, until the certificate is compared.
    typed: Option<(Address, PairingSecret)>,
    /// A pairing under way, to stop on Cancel.
    pairing: Option<AbortHandle>,
    /// The session with the paired computer.
    session: Option<AbortHandle>,
    /// Wakes a session waiting to try again.
    retry: Arc<Notify>,
    /// A name typed before the phone was connected.
    name: Option<String>,
    _news: Option<Task<()>>,
}

/// Where the phone keeps its credentials.
fn credentials_path(dir: &Path) -> PathBuf {
    dir.join("remote.json")
}

fn load(dir: &Path) -> Option<Credentials> {
    let text = std::fs::read_to_string(credentials_path(dir)).ok()?;
    serde_json::from_str(&text).ok()
}

fn store(dir: &Path, credentials: Option<&Credentials>) -> std::io::Result<()> {
    let path = credentials_path(dir);
    match credentials {
        Some(credentials) => {
            std::fs::create_dir_all(dir)?;
            std::fs::write(path, serde_json::to_string(credentials)?)
        }
        None => match std::fs::remove_file(path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                Err(error)
            }
            _ => Ok(()),
        },
    }
}

fn computer(credentials: &Credentials) -> Computer {
    Computer {
        name: credentials.host.clone(),
        address: credentials.address.clone(),
        fingerprint: credentials.fingerprint,
        via: reach(&credentials.address.host).map(str::to_owned),
    }
}

/// Connects `workspace` to the computer this phone paired with, or opens
/// pairing if it has none.
pub fn connect(
    platform: Platform,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            workspace.update(cx, |ws, cx| {
                ws.show_alert(
                    "tau could not start its network",
                    error.to_string(),
                    cx,
                )
            });
            return;
        }
    };
    let (news, mut arrivals) = mpsc::unbounded_channel();
    let credentials = load(&platform.dir);
    let name = platform.name.clone();
    let state = Rc::new(RefCell::new(State {
        runtime,
        platform,
        news,
        credentials: credentials.clone(),
        sender: None,
        last_seq: Arc::default(),
        typed: None,
        pairing: None,
        session: None,
        retry: Arc::default(),
        name: None,
        _news: None,
    }));

    let (reader, entity) = (state.clone(), workspace.downgrade());
    let task = cx.spawn(async move |cx| {
        while let Some(news) = arrivals.recv().await {
            if cx.update(|cx| arrive(&reader, &entity, news, cx)).is_none() {
                return;
            }
        }
    });
    state.borrow_mut()._news = Some(task);

    let events = state.clone();
    cx.subscribe(workspace, move |workspace, event: &WorkspaceEvent, cx| {
        act(&events, &workspace, event, cx)
    })
    .detach();

    workspace.update(cx, |ws, cx| {
        ws.set_phone_name(name, cx);
        match &credentials {
            Some(credentials) => {
                ws.update_pairing(
                    PairingUpdate::Progress(Progress::Connecting {
                        address: credentials.address.clone(),
                    }),
                    cx,
                );
            }
            None => ws.start_pairing(PairStep::Welcome, cx),
        }
    });
    if let Some(credentials) = credentials {
        start_session(&state, credentials, None);
    }
}

/// Applies what the network learned. None once the workspace is gone.
fn arrive(
    state: &Rc<RefCell<State>>,
    workspace: &WeakEntity<Workspace>,
    news: News,
    cx: &mut App,
) -> Option<()> {
    let workspace = workspace.upgrade()?;
    match news {
        News::Pairing(update) => {
            workspace.update(cx, |ws, cx| ws.update_pairing(update, cx))
        }
        // Onboarding is the computer's own.
        News::Apply(HostUpdate::Setup(_)) => {}
        News::Apply(update) => {
            workspace.update(cx, |ws, cx| ws.apply(update, cx))
        }
        News::ScanCancelled => workspace.update(cx, |ws, cx| {
            ws.update_pairing(PairingUpdate::Progress(Progress::Idle), cx);
            ws.back(cx);
        }),
        News::Paired(credentials, connection) => {
            let computer = computer(&credentials);
            let stored =
                store(&state.borrow().platform.dir, Some(&credentials));
            state.borrow_mut().credentials = Some(credentials.clone());
            start_session(state, credentials, Some(connection));
            workspace.update(cx, |ws, cx| {
                ws.update_pairing(PairingUpdate::Paired(computer), cx);
                if let Err(error) = stored {
                    ws.show_alert(
                        "Paired, but tau could not save it",
                        format!("The phone will pair again next time: {error}"),
                        cx,
                    );
                }
            });
        }
        News::Connected(sender, computer) => {
            let name = state.borrow_mut().name.take();
            if let Some(name) = name {
                send(&sender, PhoneUp::Name(name));
            }
            state.borrow_mut().sender = Some(sender);
            workspace.update(cx, |ws, cx| {
                ws.update_pairing(PairingUpdate::Connected(computer), cx)
            });
        }
        News::Revoked(credentials) => {
            let mut state = state.borrow_mut();
            state.credentials = None;
            state.sender = None;
            let _ = store(&state.platform.dir, None);
            drop(state);
            workspace.update(cx, |ws, cx| {
                ws.start_pairing(PairStep::Welcome, cx);
                ws.show_alert(
                    format!("{} no longer knows this phone", credentials.host),
                    "It was revoked on the computer. Pair it again with a new \
                     code.",
                    cx,
                );
            });
        }
    }
    Some(())
}

fn send(sender: &Sender, up: PhoneUp) {
    match serde_json::to_value(&up) {
        Ok(body) => {
            // A closed connection comes back through the session.
            let _ = sender.send(body);
        }
        Err(error) => eprintln!("tau: cannot send to the computer: {error}"),
    }
}

/// What the person did on the phone.
fn act(
    state: &Rc<RefCell<State>>,
    workspace: &Entity<Workspace>,
    event: &WorkspaceEvent,
    cx: &mut App,
) {
    let WorkspaceEvent::Pair(request) = event else {
        if !event.from_phone() {
            return;
        }
        let sender = state.borrow().sender.clone();
        match sender {
            Some(sender) => send(&sender, PhoneUp::Event(event.clone())),
            None => workspace.update(cx, |ws, cx| {
                ws.show_alert(
                    "Not connected",
                    "tau is still reaching your computer; try again in a \
                     moment.",
                    cx,
                )
            }),
        }
        return;
    };
    match request.clone() {
        PairRequest::Scan => scan(state),
        PairRequest::Typed { address, secret } => {
            state.borrow_mut().typed = Some((address.clone(), secret));
            let news = state.borrow().news.clone();
            spawn_pairing(state, async move {
                let update = match client::probe(&address).await {
                    Ok(fingerprint) => Progress::Compare {
                        address,
                        fingerprint,
                    },
                    Err(error) => Progress::Failed(failure(&error)),
                };
                let _ =
                    news.send(News::Pairing(PairingUpdate::Progress(update)));
            });
        }
        PairRequest::Trust(fingerprint) => {
            let typed = state.borrow_mut().typed.take();
            if let Some((address, secret)) = typed {
                pair(state, address, fingerprint, secret);
            }
        }
        PairRequest::Name(name) => {
            let sender = state.borrow().sender.clone();
            match sender {
                Some(sender) => send(&sender, PhoneUp::Name(name)),
                None => state.borrow_mut().name = Some(name),
            }
        }
        PairRequest::Retry => state.borrow().retry.notify_one(),
        PairRequest::Cancel => {
            if let Some(pairing) = state.borrow_mut().pairing.take() {
                pairing.abort();
            }
        }
    }
}

fn spawn_pairing(
    state: &Rc<RefCell<State>>,
    work: impl Future<Output = ()> + Send + 'static,
) {
    let mut state = state.borrow_mut();
    if let Some(earlier) = state.pairing.take() {
        earlier.abort();
    }
    state.pairing = Some(state.runtime.spawn(work).abort_handle());
}

/// Reads a pairing code with the camera, then pairs with it.
fn scan(state: &Rc<RefCell<State>>) {
    let (scan, news, name) = {
        let state = state.borrow();
        (
            state.platform.scan.clone(),
            state.news.clone(),
            state.platform.name.clone(),
        )
    };
    spawn_pairing(state, async move {
        let read = tokio::task::spawn_blocking(move || scan()).await;
        let progress = |progress| {
            let _ = news.send(News::Pairing(PairingUpdate::Progress(progress)));
        };
        let text = match read {
            Ok(Ok(Some(text))) => text,
            Ok(Ok(None)) => {
                let _ = news.send(News::ScanCancelled);
                return;
            }
            Ok(Err(error)) => return progress(Progress::Failed(error)),
            Err(error) => {
                return progress(Progress::Failed(error.to_string()));
            }
        };
        let code = match text.parse::<PairingCode>() {
            Ok(code) => code,
            Err(_) => {
                return progress(Progress::Failed(
                    "That is not a tau pairing code. On your computer, open \
                     tau › Phones and show the code."
                        .into(),
                ));
            }
        };
        progress(Progress::Connecting {
            address: code.address.clone(),
        });
        paired(&news, &code.address, code.fingerprint, &code.secret, &name)
            .await;
    });
}

/// Pairs with a typed address whose certificate the person compared.
fn pair(
    state: &Rc<RefCell<State>>,
    address: Address,
    fingerprint: Fingerprint,
    secret: PairingSecret,
) {
    let (news, name) = {
        let state = state.borrow();
        (state.news.clone(), state.platform.name.clone())
    };
    spawn_pairing(state, async move {
        paired(&news, &address, fingerprint, &secret, &name).await;
    });
}

async fn paired(
    news: &mpsc::UnboundedSender<News>,
    address: &Address,
    fingerprint: Fingerprint,
    secret: &PairingSecret,
    name: &str,
) {
    let _ =
        news.send(News::Pairing(PairingUpdate::Progress(Progress::Pairing {
            address: address.clone(),
            fingerprint,
        })));
    let news_of = match client::pair(address, fingerprint, secret, name).await {
        Ok((connection, credentials)) => News::Paired(credentials, connection),
        Err(error) => News::Pairing(PairingUpdate::Progress(Progress::Failed(
            failure(&error),
        ))),
    };
    let _ = news.send(news_of);
}

/// What went wrong, as the person can act on it.
fn failure(error: &ClientError) -> String {
    match error {
        ClientError::CertificateMismatch { .. } => {
            "The computer's certificate is not the one in the code, so tau \
             stopped before sending it."
                .into()
        }
        ClientError::Refused(Refusal::BadSecret) => {
            "The code was used or has expired. Show a new one on your \
             computer."
                .into()
        }
        ClientError::Refused(Refusal::Version { .. }) => {
            "This phone and your computer run different versions of tau. \
             Update both."
                .into()
        }
        error if error.is_unreachable() => format!(
            "{error}. Is Allow phones on, and is this phone on the same \
             network or VPN?"
        ),
        error => error.to_string(),
    }
}

/// Follows the paired computer: applies what it sends, and connects
/// again when the connection drops, from the last message applied.
fn start_session(
    state: &Rc<RefCell<State>>,
    credentials: Credentials,
    first: Option<client::Connection>,
) {
    let mut state = state.borrow_mut();
    if let Some(earlier) = state.session.take() {
        earlier.abort();
    }
    state.sender = None;
    let (news, last_seq, retry) = (
        state.news.clone(),
        state.last_seq.clone(),
        state.retry.clone(),
    );
    let work = async move {
        let computer = computer(&credentials);
        let mut first = first;
        let mut failures = 0u32;
        loop {
            let connected = match first.take() {
                Some(connection) => Ok(connection),
                None => {
                    let from = *last_seq.lock().expect("not poisoned");
                    client::resume(&credentials, from).await
                }
            };
            match connected {
                Ok(mut connection) => {
                    failures = 0;
                    let _ = news.send(News::Connected(
                        connection.sender(),
                        computer.clone(),
                    ));
                    while let Ok(Some(down)) = connection.recv().await {
                        let seq = down.seq();
                        let bodies = match down {
                            Down::Snapshot { bodies, .. } => bodies,
                            Down::Message { body, .. } => vec![body],
                        };
                        for body in bodies {
                            match serde_json::from_value::<HostUpdate>(body) {
                                Ok(update) => {
                                    let _ = news.send(News::Apply(update));
                                }
                                Err(error) => eprintln!(
                                    "tau: the computer sent what this phone \
                                     cannot read: {error}"
                                ),
                            }
                        }
                        *last_seq.lock().expect("not poisoned") = Some(seq);
                    }
                }
                Err(ClientError::Refused(Refusal::UnknownToken)) => {
                    let _ = news.send(News::Revoked(credentials));
                    return;
                }
                Err(_) => {
                    failures += 1;
                    // Past the third, each failure says so again, for
                    // a person who asked to try again.
                    if failures >= TRIES {
                        let _ = news.send(News::Pairing(
                            PairingUpdate::Unreachable {
                                computer: computer.clone(),
                                tries: failures,
                            },
                        ));
                    }
                }
            }
            // Waits longer each time, up to half a minute, or until the
            // person asks to try again.
            let wait = Duration::from_secs(1 << failures.min(5));
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                () = retry.notified() => {}
            }
        }
    };
    state.session = Some(state.runtime.spawn(work).abort_handle());
}
