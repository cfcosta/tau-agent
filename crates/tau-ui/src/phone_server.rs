//! The computer's side of phones (decision 0013): once "Allow phones"
//! is on, tau listens for them, passes on what its host applies to the
//! Workspace, and hands the host what they ask for, as if asked here.

use std::{
    cell::RefCell,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};

use gpui::{App, Entity, Task};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_remote::{
    Address,
    Server,
    ServerConfig,
    ServerEvent,
    ServerHandle,
    devices::SECRET_LIFETIME,
};
use tokio::runtime::Handle;

use crate::{
    phones::{LocalAddress, PhoneUp, Phones, PhonesRequest, ShownCode},
    update::HostUpdate,
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

    fn load(dir: &Path) -> Self {
        std::fs::read_to_string(Self::path(dir))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(Self::path(dir), text)
    }
}

struct Bridge {
    runtime: Handle,
    /// Where the certificate, the paired phones and the settings are.
    dir: PathBuf,
    host: String,
    settings: Settings,
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

/// Serves phones for `workspace` from `dir` (usually
/// `~/.config/tau/phones`), starting now if phones were allowed.
pub fn serve(
    runtime: Handle,
    dir: PathBuf,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    let settings = Settings::load(&dir);
    let bridge = Rc::new(RefCell::new(Bridge {
        runtime,
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
        server: None,
        listening: None,
        countdown: None,
        generation: 0,
    }));
    if bridge.borrow().settings.allow {
        start(&bridge, workspace, cx);
    }
    show(&bridge, workspace, cx);

    // What the host applies goes to every phone.
    let echo = bridge.clone();
    cx.subscribe(workspace, move |_, update: &HostUpdate, _| {
        if let Some(server) = &echo.borrow().server
            && let Some(body) = to_phones(update)
        {
            server.broadcast(body);
        }
    })
    .detach();

    let requests = bridge;
    cx.subscribe(workspace, move |workspace, event: &WorkspaceEvent, cx| {
        if let WorkspaceEvent::Phones(request) = event {
            handle(&requests, request.clone(), &workspace, cx);
        }
    })
    .detach();
}

/// What phones are sent of `update`: all but onboarding, which stays
/// on the computer.
pub fn to_phones(update: &HostUpdate) -> Option<Value> {
    if matches!(update, HostUpdate::Setup(_)) {
        return None;
    }
    serde_json::to_value(update)
        .inspect_err(|error| {
            eprintln!("tau-ui: cannot send an update: {error}")
        })
        .ok()
}

/// What a phone that needs everything is sent: what `workspace` shows.
pub fn snapshot(workspace: &Workspace) -> Vec<Value> {
    to_phones(&workspace.snapshot()).into_iter().collect()
}

/// What a phone sent, if it is something a phone may ask.
pub fn from_phone(body: Value) -> Option<PhoneUp> {
    match serde_json::from_value::<PhoneUp>(body) {
        Ok(PhoneUp::Event(event)) if !event.from_phone() => None,
        Ok(up) => Some(up),
        Err(error) => {
            eprintln!("tau-ui: a phone sent what tau cannot read: {error}");
            None
        }
    }
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
            save(bridge);
            if allow {
                start(bridge, workspace, cx);
            } else {
                stop(bridge, workspace, cx);
            }
        }
        PhonesRequest::ListenOn(ip) => {
            bridge.borrow_mut().settings.listen = Some(ip);
            save(bridge);
            if bridge.borrow().server.is_some() {
                stop(bridge, workspace, cx);
                start(bridge, workspace, cx);
            }
        }
        PhonesRequest::ShowCode => show_code(bridge, workspace, cx),
        PhonesRequest::HideCode => hide_code(bridge),
        PhonesRequest::Revoke(id) => {
            let revoked = bridge
                .borrow()
                .server
                .as_ref()
                .map(|server| server.revoke(&id));
            if let Some(Err(error)) = revoked {
                bridge.borrow_mut().phones.error =
                    Some(format!("Could not revoke the phone: {error}"));
            }
        }
    }
    show(bridge, workspace, cx);
}

fn save(bridge: &Rc<RefCell<Bridge>>) {
    let mut bridge = bridge.borrow_mut();
    if let Err(error) = bridge.settings.save(&bridge.dir) {
        bridge.phones.error = Some(format!("Could not save: {error}"));
    }
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
    workspace.update(cx, |ws, _| ws.set_mirrored(true));

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

fn stop(
    bridge: &Rc<RefCell<Bridge>>,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    let mut state = bridge.borrow_mut();
    state.generation += 1;
    if let Some(server) = state.server.take() {
        server.stop();
    }
    state.listening = None;
    state.countdown = None;
    state.phones.listening = None;
    state.phones.code = None;
    workspace.update(cx, |ws, _| ws.set_mirrored(false));
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
            server.snapshot(conn, snapshot(workspace.read(cx)));
        }
        ServerEvent::Up { device, body, .. } => match from_phone(body) {
            // The host acts on it as on its own interface's.
            Some(PhoneUp::Event(event)) => {
                workspace.update(cx, |_, cx| cx.emit(event));
            }
            Some(PhoneUp::Name(name)) => {
                if let Err(error) = server.rename(&device.id, &name) {
                    eprintln!("tau-ui: cannot rename a phone: {error}");
                }
            }
            None => {}
        },
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
        picked.save(dir.path()).unwrap();
        assert_eq!(Settings::load(dir.path()), picked);
    }
}
