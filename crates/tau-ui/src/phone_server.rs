//! The computer's side of phones (decision 0013): once "Allow phones"
//! is on, tau listens for them and carries the window's mirror
//! ([`crate::mirror`]) both ways, reaching it only as the [`App`]
//! service.

use std::{
    cell::RefCell,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use gpui::{App, Entity, Task};
use serde::{Deserialize, Serialize};
use tau_remote::{
    Address,
    Server,
    ServerConfig,
    ServerEvent,
    ServerHandle,
    SystemClock,
    devices::SECRET_LIFETIME,
};
use tau_ui_plugin::app::{App as Window, Feed, Up};
use tokio::runtime::Handle;

use crate::{
    phones::{LocalAddress, Phones, PhonesRequest, ShownCode},
    workspace::{Workspace, WorkspaceEvent},
};

/// What the person picked, kept across launches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Settings {
    allow: bool,
    listen: Option<String>,
    port: u16,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            allow: false,
            listen: None,
            port: Address::DEFAULT_PORT,
        }
    }
}

impl Settings {
    fn path(dir: &Path) -> PathBuf {
        dir.join("settings.json")
    }

    async fn load(dir: &Path) -> Self {
        tokio::fs::read_to_string(Self::path(dir))
            .await
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    async fn save(&self, dir: &Path) -> std::io::Result<()> {
        tokio::fs::create_dir_all(dir).await?;
        let text = serde_json::to_string_pretty(self)?;
        tokio::fs::write(Self::path(dir), text).await
    }
}

/// Writes the settings on the host's runtime (ADR 0028), one save at a
/// time, so an older one never lands after a newer one.
#[derive(Clone, Default)]
struct Keeper {
    /// The settings not yet written: each save puts its own here, over
    /// any older ones, and whichever save runs next writes them.
    latest: Arc<Mutex<Option<Settings>>>,
    turn: Arc<tokio::sync::Mutex<()>>,
}

impl Keeper {
    async fn save(
        self,
        dir: PathBuf,
        settings: Settings,
    ) -> std::io::Result<()> {
        *self.latest.lock().expect("not poisoned") = Some(settings);
        let _turn = self.turn.lock().await;
        let latest = self.latest.lock().expect("not poisoned").take();
        match latest {
            Some(settings) => settings.save(&dir).await,
            // A later save wrote them already.
            None => Ok(()),
        }
    }
}

struct Bridge {
    runtime: Handle,
    /// The window phones mirror.
    window: Arc<dyn Window>,
    /// Where the certificate, the paired phones and the settings are.
    dir: PathBuf,
    host: String,
    settings: Settings,
    keeper: Keeper,
    server: Option<ServerHandle>,
    phones: Phones,
    /// Reads the server's events while it runs.
    listening: Option<Task<()>>,
    /// Counts down the pairing code on screen.
    countdown: Option<Task<()>>,
    /// Bumped by every start and stop: a server that finishes starting
    /// after a later one asked for is stopped, not kept.
    generation: u64,
}

/// Serves phones the mirror of `window` from `dir` (usually
/// `~/.config/tau/phones`), starting as soon as the settings are read
/// if phones were allowed; `workspace` shows the Phones screen.
pub fn serve(
    runtime: Handle,
    dir: PathBuf,
    window: Arc<dyn Window>,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    // Read on the host's runtime: the interface does not wait on files
    // (ADR 0028).
    let reading = runtime.spawn({
        let dir = dir.clone();
        async move { Settings::load(&dir).await }
    });
    let workspace = workspace.downgrade();
    cx.spawn(async move |cx| {
        let settings = reading.await.unwrap_or_default();
        let Some(workspace) = workspace.upgrade() else {
            return;
        };
        cx.update(|cx| serving(runtime, dir, window, settings, &workspace, cx));
    })
    .detach();
}

fn serving(
    runtime: Handle,
    dir: PathBuf,
    window: Arc<dyn Window>,
    settings: Settings,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    let bridge = Rc::new(RefCell::new(Bridge {
        runtime,
        window,
        dir,
        host: host_name(),
        phones: Phones {
            allowed: settings.allow,
            addresses: local_addresses(),
            listen: settings.listen.clone(),
            port: settings.port,
            ..Phones::default()
        },
        settings,
        keeper: Keeper::default(),
        server: None,
        listening: None,
        countdown: None,
        generation: 0,
    }));
    if bridge.borrow().settings.allow {
        start(&bridge, workspace, cx);
    }
    show(&bridge, workspace, cx);

    cx.subscribe(workspace, move |workspace, event: &WorkspaceEvent, cx| {
        if let WorkspaceEvent::Phones(request) = event {
            handle(&bridge, request.clone(), &workspace, cx);
        }
    })
    .detach();
}

fn handle(
    bridge: &Rc<RefCell<Bridge>>,
    request: PhonesRequest,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    match request {
        PhonesRequest::Allow(allow) => {
            bridge.borrow_mut().settings.allow = allow;
            save(bridge, workspace, cx);
            if allow {
                start(bridge, workspace, cx);
            } else {
                stop(bridge);
            }
        }
        PhonesRequest::ListenOn(ip) => {
            bridge.borrow_mut().settings.listen = Some(ip);
            save(bridge, workspace, cx);
            if bridge.borrow().server.is_some() {
                stop(bridge);
                start(bridge, workspace, cx);
            }
        }
        PhonesRequest::ShowCode => show_code(bridge, workspace, cx),
        PhonesRequest::HideCode => hide_code(bridge),
        PhonesRequest::Revoke(id) => {
            let state = bridge.borrow();
            if let Some(server) = state.server.clone() {
                let revoking = state
                    .runtime
                    .spawn(async move { server.revoke(&id).await });
                drop(state);
                report(
                    bridge,
                    revoking,
                    "Could not revoke the phone",
                    workspace,
                    cx,
                );
            }
        }
    }
    show(bridge, workspace, cx);
}

/// Saves the settings as they are now, off the interface's thread.
fn save(
    bridge: &Rc<RefCell<Bridge>>,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    let state = bridge.borrow();
    let saving = state.runtime.spawn(
        state
            .keeper
            .clone()
            .save(state.dir.clone(), state.settings.clone()),
    );
    drop(state);
    report(bridge, saving, "Could not save", workspace, cx);
}

/// Shows `failed` and the error, if `job` fails.
fn report<T, E: std::fmt::Display + Send + 'static>(
    bridge: &Rc<RefCell<Bridge>>,
    job: tokio::task::JoinHandle<Result<T, E>>,
    failed: &'static str,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) where
    T: Send + 'static,
{
    let (bridge, workspace) = (bridge.clone(), workspace.downgrade());
    cx.spawn(async move |cx| {
        let error = match job.await {
            Ok(Ok(_)) => return,
            Ok(Err(error)) => error.to_string(),
            Err(error) => error.to_string(),
        };
        let Some(workspace) = workspace.upgrade() else {
            return;
        };
        bridge.borrow_mut().phones.error = Some(format!("{failed}: {error}"));
        cx.update(|cx| show(&bridge, &workspace, cx));
    })
    .detach();
}

/// The IP to listen on: the one picked if the computer still has it,
/// else Tailscale's, else the first on the network.
fn listen_ip(bridge: &Bridge) -> Option<String> {
    let addresses = &bridge.phones.addresses;
    // Loopback is never offered, but kept if picked by hand: for a
    // tunnel, or a test.
    let loopback =
        |ip: &String| ip.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback());
    bridge
        .settings
        .listen
        .clone()
        .filter(|ip| {
            loopback(ip) || addresses.iter().any(|address| &address.ip == ip)
        })
        .or_else(|| {
            addresses
                .iter()
                .find(|address| address.label == "Tailscale")
                .or_else(|| addresses.first())
                .map(|address| address.ip.clone())
        })
}

fn start(
    bridge: &Rc<RefCell<Bridge>>,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    let mut state = bridge.borrow_mut();
    let Some(ip) = listen_ip(&state) else {
        state.phones.error =
            Some("This computer has no address a phone could reach.".into());
        return;
    };
    let Ok(parsed) = ip.parse::<IpAddr>() else {
        state.phones.error = Some(format!("`{ip}` is not an address."));
        return;
    };
    let config = ServerConfig {
        listen: SocketAddr::new(parsed, state.settings.port),
        dir: state.dir.clone(),
        host: state.host.clone(),
        clock: Arc::new(SystemClock),
    };
    state.generation += 1;
    let generation = state.generation;
    // On the host's runtime: binding the port and the certificate take
    // their time, which the interface does not wait on (ADR 0028).
    let starting = state.runtime.spawn(Server::start(config));
    drop(state);
    let (bridge, workspace) = (bridge.clone(), workspace.downgrade());
    cx.spawn(async move |cx| {
        let started = match starting.await {
            Ok(started) => started.map_err(|error| error.to_string()),
            Err(error) => Err(error.to_string()),
        };
        let Some(workspace) = workspace.upgrade() else {
            return;
        };
        cx.update(|cx| listening(&bridge, generation, started, &workspace, cx));
    })
    .detach();
}

/// The server `start` asked for is up, or could not start: it serves
/// phones, unless a later start or stop asked for something else. The
/// workspace shows how it went.
fn listening(
    bridge: &Rc<RefCell<Bridge>>,
    generation: u64,
    started: Result<
        (
            ServerHandle,
            tokio::sync::mpsc::UnboundedReceiver<ServerEvent>,
        ),
        String,
    >,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    take_up(bridge, generation, started, workspace, cx);
    show(bridge, workspace, cx);
}

fn take_up(
    bridge: &Rc<RefCell<Bridge>>,
    generation: u64,
    started: Result<
        (
            ServerHandle,
            tokio::sync::mpsc::UnboundedReceiver<ServerEvent>,
        ),
        String,
    >,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    let mut state = bridge.borrow_mut();
    if state.generation != generation {
        if let Ok((server, _)) = started {
            server.stop();
        }
        return;
    }
    let Some(ip) = listen_ip(&state) else {
        return;
    };
    let (server, mut events) = match started {
        Ok(started) => started,
        Err(error) => {
            state.phones.error = Some(format!("tau could not listen: {error}"));
            return;
        }
    };
    state.phones.error = None;
    state.phones.listen = Some(ip.clone());
    state.phones.listening = Some(Address {
        host: ip,
        port: server.local_addr().port(),
    });
    state.phones.fingerprint = Some(server.fingerprint());
    state.phones.paired = server.paired();
    state.server = Some(server.clone());
    let sink = server.clone();
    state.window.mirror(Some(Arc::new(move |feed: Feed| {
        if feed.transient {
            sink.broadcast_transient(feed.body);
        } else {
            sink.broadcast(feed.body);
        }
    })));

    let (bridge_for_events, workspace_for_events) =
        (bridge.clone(), workspace.downgrade());
    state.listening = Some(cx.spawn(async move |cx| {
        while let Some(event) = events.recv().await {
            let Some(workspace) = workspace_for_events.upgrade() else {
                return;
            };
            cx.update(|cx| {
                on_event(&bridge_for_events, &server, event, &workspace, cx)
            });
        }
    }));
}

fn stop(bridge: &Rc<RefCell<Bridge>>) {
    let mut state = bridge.borrow_mut();
    state.generation += 1;
    if let Some(server) = state.server.take() {
        server.stop();
    }
    state.listening = None;
    state.countdown = None;
    state.phones.listening = None;
    state.phones.code = None;
    state.window.mirror(None);
}

fn on_event(
    bridge: &Rc<RefCell<Bridge>>,
    server: &ServerHandle,
    event: ServerEvent,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    match event {
        ServerEvent::NeedSnapshot { conn, .. } => {
            let server = server.clone();
            bridge.borrow().window.snapshot(Box::new(move |bodies| {
                server.snapshot(conn, bodies)
            }));
        }
        ServerEvent::Up { device, body, .. } => {
            let state = bridge.borrow();
            if let Up::Renamed(name) = state.window.up(&device.id, body) {
                let server = server.clone();
                state.runtime.spawn(async move {
                    if let Err(error) = server.rename(&device.id, &name).await {
                        eprintln!("tau-ui: cannot rename a phone: {error}");
                    }
                });
            }
        }
        // Its views are built for it no more.
        ServerEvent::Left { device } => {
            bridge.borrow().window.left(&device);
        }
        ServerEvent::DevicesChanged(paired) => {
            let mut state = bridge.borrow_mut();
            // A phone that just paired used the code.
            if paired.len() > state.phones.paired.len() {
                state.phones.code = None;
                state.countdown = None;
            }
            state.phones.paired = paired;
            drop(state);
            show(bridge, workspace, cx);
        }
    }
}

fn show_code(
    bridge: &Rc<RefCell<Bridge>>,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    let mut state = bridge.borrow_mut();
    let (Some(server), Some(address)) =
        (state.server.clone(), state.phones.listening.clone())
    else {
        return;
    };
    state.phones.code = Some(ShownCode {
        code: server.pairing_code(address),
        expires: Instant::now() + SECRET_LIFETIME,
    });
    // Once a second the countdown moves; at zero the code goes.
    let (bridge_for_ticks, workspace_for_ticks) =
        (bridge.clone(), workspace.downgrade());
    state.countdown = Some(cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            let Some(workspace) = workspace_for_ticks.upgrade() else {
                return;
            };
            let expired = bridge_for_ticks
                .borrow()
                .phones
                .code
                .as_ref()
                .is_none_or(|code| code.expires <= Instant::now());
            if expired {
                hide_code(&bridge_for_ticks);
            }
            cx.update(|cx| show(&bridge_for_ticks, &workspace, cx));
            if expired {
                return;
            }
        }
    }));
}

fn hide_code(bridge: &Rc<RefCell<Bridge>>) {
    let mut state = bridge.borrow_mut();
    if let Some(server) = &state.server {
        server.close_pairing();
    }
    state.phones.code = None;
}

/// Hands the Workspace what the Phones screen shows.
fn show(
    bridge: &Rc<RefCell<Bridge>>,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    let phones = bridge.borrow().phones.clone();
    workspace.update(cx, |ws, cx| ws.set_phones(phones, cx));
}

/// The computer's addresses a phone could reach, Tailscale's first:
/// IPv4, not loopback.
fn local_addresses() -> Vec<LocalAddress> {
    let mut addresses: Vec<LocalAddress> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|interface| !interface.is_loopback())
        .filter(|interface| interface.ip().is_ipv4())
        .map(|interface| {
            let ip = interface.ip().to_string();
            let tailscale = interface.name.starts_with("tailscale")
                || crate::phones::reach(&ip) == Some("over Tailscale");
            LocalAddress {
                label: if tailscale {
                    "Tailscale".into()
                } else {
                    interface.name.clone()
                },
                ip,
            }
        })
        .collect();
    addresses.sort_by_key(|address| address.label != "Tailscale");
    addresses.dedup_by(|a, b| a.ip == b.ip);
    addresses
}

/// This computer's name, as phones show it.
fn host_name() -> String {
    let mut buffer = [0u8; 256];
    // SAFETY: the buffer is valid for its whole length, and gethostname
    // writes at most that many bytes.
    let written =
        unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
    let end = buffer.iter().position(|byte| *byte == 0).unwrap_or(0);
    match std::str::from_utf8(&buffer[..end]) {
        Ok(name) if written == 0 && !name.is_empty() => name.to_owned(),
        _ => "this computer".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_start_with_phones_off() {
        let settings = Settings::default();
        assert!(!settings.allow);
        assert_eq!(settings.port, Address::DEFAULT_PORT);
        let dir = tempfile::tempdir().unwrap();
        let picked = Settings {
            allow: true,
            listen: Some("100.84.12.7".into()),
            port: 7443,
        };
        tau_testing::block_on_io(picked.save(dir.path())).unwrap();
        assert_eq!(
            tau_testing::block_on_io(Settings::load(dir.path())),
            picked
        );
    }
}
