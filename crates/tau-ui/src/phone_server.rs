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
    }));
    if bridge.borrow().settings.allow {
        start(&bridge, workspace, cx);
    }
    show(&bridge, workspace, cx);

    // What the host applies goes to every phone. Onboarding stays here.
    let echo = bridge.clone();
    cx.subscribe(workspace, move |_, update: &HostUpdate, _| {
        if matches!(update, HostUpdate::Setup(_)) {
            return;
        }
        if let Some(server) = &echo.borrow().server {
            match serde_json::to_value(update) {
                Ok(body) => server.broadcast(body),
                Err(error) => eprintln!("tau-ui: cannot send an update: {error}"),
            }
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
    bridge
        .settings
        .listen
        .clone()
        .filter(|ip| addresses.iter().any(|address| &address.ip == ip))
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
    let started = state.runtime.block_on(Server::start(config));
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
            let snapshot = workspace.read(cx).snapshot();
            match serde_json::to_value(&snapshot) {
                Ok(body) => server.snapshot(conn, vec![body]),
                Err(error) => {
                    eprintln!("tau-ui: cannot send a snapshot: {error}")
                }
            }
        }
        ServerEvent::Up { device, body, .. } => {
            match serde_json::from_value::<PhoneUp>(body) {
                // The host acts on it as on its own interface's.
                Ok(PhoneUp::Event(event)) if event.from_phone() => {
                    workspace.update(cx, |_, cx| cx.emit(event));
                }
                Ok(PhoneUp::Event(_)) => {}
                Ok(PhoneUp::Name(name)) => {
                    if let Err(error) = server.rename(&device.id, &name) {
                        eprintln!("tau-ui: cannot rename a phone: {error}");
                    }
                }
                Err(error) => {
                    eprintln!("tau-ui: a phone sent what tau cannot read: {error}")
                }
            }
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
    let written = unsafe {
        libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len())
    };
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
