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
    outbox::Outbox,
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
    /// The computer took the requests up to this one.
    Taken(u64),
    /// The connection closed; the session connects again.
    Disconnected,
    Revoked(Credentials),
}

struct State {
    runtime: Runtime,
    platform: Platform,
    news: mpsc::UnboundedSender<News>,
    credentials: Option<Credentials>,
    /// The connection's sender while connected.
    sender: Option<Sender>,
    /// What the person asked here that the computer has not taken: sent
    /// again on each connection, until it is.
    outbox: Outbox,
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
    _news: Option<Task<()>>,
}

/// Where the phone keeps its credentials.
fn credentials_path(dir: &Path) -> PathBuf {
    dir.join("remote.json")
}

/// The credentials the phone kept in `dir`, if it paired.
pub fn load(dir: &Path) -> Option<Credentials> {
    let text = std::fs::read_to_string(credentials_path(dir)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Keeps `credentials` in `dir`, or forgets them. They take the old
/// ones' place in one rename, owner-only: a phone killed while it writes
/// keeps the old ones or the new, never half of either.
pub fn store(
    dir: &Path,
    credentials: Option<&Credentials>,
) -> std::io::Result<()> {
    let path = credentials_path(dir);
    match credentials {
        Some(credentials) => tau_ai::files::write_private(
            &path,
            serde_json::to_string(credentials)?.as_bytes(),
        ),
        None => match std::fs::remove_file(path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                Err(error)
            }
            _ => Ok(()),
        },
    }
}

/// Request numbers start from the clock, so they only grow across the
/// phone's restarts, and the computer never takes a new one for one it
/// had.
fn clock() -> u64 {
    tau_remote::clock::count_from(std::time::SystemTime::now())
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
    // The phone draws plugins' views as the computer builds them (ADR
    // 0046).
    workspace.update(cx, |ws, _| ws.draw_remote_views());
    let (news, mut arrivals) = mpsc::unbounded_channel();
    let credentials = load(&platform.dir);
    let name = platform.name.clone();
    let state = Rc::new(RefCell::new(State {
        runtime,
        platform,
        news,
        credentials: credentials.clone(),
        sender: None,
        outbox: Outbox::new(clock()),
        last_seq: Arc::default(),
        typed: None,
        pairing: None,
        session: None,
        retry: Arc::default(),
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

    let events = Rc::downgrade(&state);
    cx.subscribe(workspace, move |workspace, event: &WorkspaceEvent, cx| {
        if let Some(state) = events.upgrade() {
            act(&state, &workspace, event, cx)
        }
    })
    .detach();
    // The remote lives as long as its window: once the window is gone,
    // the phone reaches its computer no more.
    let stopping = Rc::downgrade(&state);
    cx.observe_release(workspace, move |_, _| {
        if let Some(state) = stopping.upgrade() {
            stop(&state);
        }
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

/// Stops the session, any pairing and the news from them, so the
/// remote's state, and its runtime with it, can go.
fn stop(state: &Rc<RefCell<State>>) {
    let mut state = state.borrow_mut();
    for work in [state.session.take(), state.pairing.take()]
        .into_iter()
        .flatten()
    {
        work.abort();
    }
    state.sender = None;
    let news = state._news.take();
    drop(state);
    drop(news);
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
            state.borrow_mut().sender = Some(sender.clone());
            // What the computer did not take yet, again, in order.
            for up in state.borrow().outbox.pending() {
                let _ = sender.send(up);
            }
            workspace.update(cx, |ws, cx| {
                ws.update_pairing(PairingUpdate::Connected(computer), cx)
            });
        }
        News::Taken(up) => {
            state.borrow_mut().outbox.taken(up);
            show_unsent(state, &workspace, cx);
        }
        News::Disconnected => state.borrow_mut().sender = None,
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

/// Sends `up` once connected, and again on each connection until the
/// computer takes it.
fn send(state: &Rc<RefCell<State>>, up: PhoneUp) {
    let Some(body) = to_computer(&up) else { return };
    let mut state = state.borrow_mut();
    let up = state.outbox.push(body);
    if let Some(sender) = &state.sender {
        // A closed connection comes back through the session, which
        // sends it again.
        let _ = sender.send(up);
    }
}

/// Says how many requests wait for the computer.
fn show_unsent(
    state: &Rc<RefCell<State>>,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    let unsent = state.borrow().outbox.len();
    workspace.update(cx, |ws, cx| {
        ws.update_pairing(PairingUpdate::Unsent(unsent), cx)
    });
}

/// What the phone sends the computer of `up`.
pub fn to_computer(up: &PhoneUp) -> Option<serde_json::Value> {
    serde_json::to_value(up)
        .inspect_err(|error| {
            eprintln!("tau: cannot send to the computer: {error}")
        })
        .ok()
}

/// The updates in what the computer sent, to apply in order. Onboarding
/// is the computer's own.
pub fn updates(down: Down) -> Vec<HostUpdate> {
    let bodies = match down {
        Down::Snapshot { bodies, .. } => bodies,
        Down::Message { body, .. } | Down::Transient { body } => vec![body],
        Down::Ack { .. } => Vec::new(),
    };
    bodies
        .into_iter()
        .filter_map(|body| {
            serde_json::from_value::<HostUpdate>(body)
                .inspect_err(|error| {
                    eprintln!(
                        "tau: the computer sent what this phone cannot read: \
                         {error}"
                    )
                })
                .ok()
        })
        .filter(|update| !matches!(update, HostUpdate::Setup(_)))
        .collect()
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
        // Sent now, or once the phone reaches the computer again.
        send(state, PhoneUp::Event(event.clone()));
        show_unsent(state, workspace, cx);
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
            send(state, PhoneUp::Name(name));
            show_unsent(state, workspace, cx);
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
                        if let Down::Ack { up } = down {
                            let _ = news.send(News::Taken(up));
                            continue;
                        }
                        let seq = down.seq();
                        for update in updates(down) {
                            let _ = news.send(News::Apply(update));
                        }
                        if let Some(seq) = seq {
                            *last_seq.lock().expect("not poisoned") = Some(seq);
                        }
                    }
                    let _ = news.send(News::Disconnected);
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

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use tau_remote::Fingerprint;

    use super::*;

    /// What a reader sees while the phone writes its credentials over and
    /// over is what a kill at that moment leaves: always whole.
    #[test]
    fn credentials_are_never_seen_half_written() {
        let dir = tempfile::tempdir().unwrap();
        let credentials = Credentials {
            host: "desk".into(),
            address: "127.0.0.1:7443".parse().unwrap(),
            fingerprint: Fingerprint([7; 32]),
            device: "d".repeat(22),
            token: "t".repeat(43),
        };
        store(dir.path(), Some(&credentials)).unwrap();
        let writing = Arc::new(AtomicBool::new(true));
        let writer = {
            let (dir, writing) = (dir.path().to_owned(), writing.clone());
            let credentials = credentials.clone();
            std::thread::spawn(move || {
                while writing.load(Ordering::SeqCst) {
                    store(&dir, Some(&credentials)).unwrap();
                }
            })
        };
        let torn = (0..2_000)
            .filter(|_| load(dir.path()).as_ref() != Some(&credentials))
            .count();
        writing.store(false, Ordering::SeqCst);
        writer.join().unwrap();
        assert_eq!(torn, 0, "a reader saw credentials half-written");
    }
}
