//! The server phones connect to (decision 0013): once "Allow phones" is
//! on, it listens, pairs phones by a code, and carries the window's
//! mirror ([`App`]) to them and what they send back. It runs on the
//! host's runtime; nothing here waits on the window.

use std::{
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tau_remote::{
    Address,
    Device,
    Fingerprint,
    PairingCode,
    Server,
    ServerConfig,
    ServerEvent,
    ServerHandle,
    SystemClock,
    devices::SECRET_LIFETIME,
};
use tau_ui_plugin::app::{App, Feed, Up};
use tokio::{runtime::Handle, sync::mpsc, task::JoinHandle};

use crate::{Act, LocalAddress, Paired, PhonesData, ShownCode};

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

/// Writes the settings one save at a time, so an older one never lands
/// after a newer one.
#[derive(Default)]
struct Keeper {
    /// The settings not yet written: each save puts its own here, over
    /// any older ones, and whichever save runs next writes them.
    latest: Mutex<Option<Settings>>,
    turn: tokio::sync::Mutex<()>,
}

impl Keeper {
    async fn save(
        &self,
        dir: &Path,
        settings: Settings,
    ) -> std::io::Result<()> {
        *self.latest.lock().expect("not poisoned") = Some(settings);
        let _turn = self.turn.lock().await;
        let latest = self.latest.lock().expect("not poisoned").take();
        match latest {
            Some(settings) => settings.save(dir).await,
            // A later save wrote them already.
            None => Ok(()),
        }
    }
}

/// Phones on this computer: the server, while phones are allowed, and
/// what the Phones page shows of it.
#[derive(Clone)]
pub struct Phones {
    inner: Arc<Inner>,
}

struct Inner {
    runtime: Handle,
    /// Where the certificate, the paired phones and the settings are.
    dir: PathBuf,
    /// This computer's name, as phones show it.
    host: String,
    window: Arc<dyn App>,
    /// Tells the page that what it shows changed.
    changed: Arc<dyn Fn() + Send + Sync>,
    keeper: Keeper,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    settings: Settings,
    addresses: Vec<LocalAddress>,
    server: Option<ServerHandle>,
    listening: Option<Address>,
    fingerprint: Option<Fingerprint>,
    code: Option<(PairingCode, SystemTime)>,
    paired: Vec<Device>,
    error: Option<String>,
    /// Bumped by every start and stop: a server that finishes starting
    /// after a later one asked for is stopped, not kept.
    generation: u64,
    /// Reads the server's events while it runs.
    events: Option<JoinHandle<()>>,
    /// Hides the code once it expires.
    expiry: Option<JoinHandle<()>>,
}

impl Phones {
    /// Phones kept in `dir` (usually `~/.config/tau/phones`), mirroring
    /// `window`, on the current runtime; `changed` hears each change the
    /// page shows. It listens once [`Self::resume`]d, if phones were
    /// allowed.
    pub async fn open(
        dir: PathBuf,
        window: Arc<dyn App>,
        changed: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let settings = Settings::load(&dir).await;
        Self {
            inner: Arc::new(Inner {
                runtime: Handle::current(),
                dir,
                host: host_name(),
                window,
                changed,
                keeper: Keeper::default(),
                state: Mutex::new(State {
                    settings,
                    addresses: local_addresses(),
                    ..State::default()
                }),
            }),
        }
    }

    /// Listens for phones, if they were allowed.
    pub fn resume(&self) {
        if self.state().settings.allow {
            self.start();
        }
    }

    /// What the Phones page shows.
    pub fn data(&self) -> PhonesData {
        let state = self.state();
        PhonesData {
            allowed: state.settings.allow,
            addresses: state.addresses.clone(),
            listening: state.listening.as_ref().map(ToString::to_string),
            code: state.code.as_ref().map(|(code, expires)| ShownCode {
                text: code.to_string(),
                address: code.address.to_string(),
                secret: code.secret.to_string(),
                certificate: code.fingerprint.short(),
                lines: code.fingerprint.lines(),
                expires: millis(*expires),
            }),
            paired: state
                .paired
                .iter()
                .map(|device| Paired {
                    id: device.id.clone(),
                    name: device.name.clone(),
                    paired_at: device.paired_at.clone(),
                    last_seen: device.last_seen.clone(),
                })
                .collect(),
            error: state.error.clone(),
        }
    }

    /// Where it listens, and its certificate, once it does.
    pub fn listening(&self) -> Option<(Address, Fingerprint)> {
        let state = self.state();
        state.listening.clone().zip(state.fingerprint)
    }

    /// The pairing code on screen.
    pub fn code(&self) -> Option<PairingCode> {
        self.state().code.as_ref().map(|(code, _)| code.clone())
    }

    pub fn paired(&self) -> Vec<Device> {
        self.state().paired.clone()
    }

    /// What last went wrong.
    pub fn error(&self) -> Option<String> {
        self.state().error.clone()
    }

    /// Carries out what the person asked of the page.
    pub async fn act(&self, act: Act) {
        match act {
            Act::Allow { allow } => {
                self.state().settings.allow = allow;
                if allow {
                    self.start();
                } else {
                    self.stop();
                }
                self.save().await;
            }
            Act::ListenOn { ip } => {
                let running = {
                    let mut state = self.state();
                    state.settings.listen = Some(ip);
                    state.server.is_some()
                };
                if running {
                    self.stop();
                    self.start();
                }
                self.save().await;
            }
            Act::ShowCode => self.show_code(),
            Act::HideCode => self.hide_code(),
            Act::Revoke { id } => {
                let server = self.state().server.clone();
                if let Some(server) = server
                    && let Err(error) = server.revoke(&id).await
                {
                    self.failed("Could not revoke the phone", error);
                }
            }
        }
        self.changed();
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.inner.state.lock().expect("not poisoned")
    }

    fn changed(&self) {
        (self.inner.changed)();
    }

    fn failed(&self, what: &str, error: impl std::fmt::Display) {
        self.state().error = Some(format!("{what}: {error}"));
        self.changed();
    }

    /// Saves the settings as they are now.
    async fn save(&self) {
        let settings = self.state().settings.clone();
        if let Err(error) =
            self.inner.keeper.save(&self.inner.dir, settings).await
        {
            self.failed("Could not save", error);
        }
    }

    fn start(&self) {
        let mut state = self.state();
        let Some(ip) = listen_ip(&state) else {
            state.error = Some(
                "This computer has no address a phone could reach.".into(),
            );
            return;
        };
        let Ok(parsed) = ip.parse::<IpAddr>() else {
            state.error = Some(format!("`{ip}` is not an address."));
            return;
        };
        let config = ServerConfig {
            listen: SocketAddr::new(parsed, state.settings.port),
            dir: self.inner.dir.clone(),
            host: self.inner.host.clone(),
            clock: Arc::new(SystemClock),
        };
        state.generation += 1;
        let generation = state.generation;
        drop(state);
        let phones = self.clone();
        self.inner.runtime.spawn(async move {
            let started = Server::start(config)
                .await
                .map_err(|error| error.to_string());
            phones.take_up(generation, started);
            phones.changed();
        });
    }

    /// The server [`Self::start`] asked for is up, or could not start: it
    /// serves phones, unless a later start or stop asked for something
    /// else.
    fn take_up(
        &self,
        generation: u64,
        started: Result<
            (ServerHandle, mpsc::UnboundedReceiver<ServerEvent>),
            String,
        >,
    ) {
        let mut state = self.state();
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
                state.error = Some(format!("tau could not listen: {error}"));
                return;
            }
        };
        state.error = None;
        state.settings.listen = Some(ip.clone());
        state.listening = Some(Address {
            host: ip,
            port: server.local_addr().port(),
        });
        state.fingerprint = Some(server.fingerprint());
        state.paired = server.paired();
        state.server = Some(server.clone());
        // What the window applies goes to every phone; an answer to a
        // request goes to the phones there now, unnumbered and not kept.
        let sink = server.clone();
        self.inner.window.mirror(Some(Arc::new(move |feed: Feed| {
            if feed.transient {
                sink.broadcast_transient(feed.body);
            } else {
                sink.broadcast(feed.body);
            }
        })));
        let phones = self.clone();
        state.events = Some(self.inner.runtime.spawn(async move {
            while let Some(event) = events.recv().await {
                phones.on_event(&server, event);
            }
        }));
    }

    fn stop(&self) {
        let mut state = self.state();
        state.generation += 1;
        if let Some(server) = state.server.take() {
            server.stop();
        }
        for task in [state.events.take(), state.expiry.take()]
            .into_iter()
            .flatten()
        {
            task.abort();
        }
        state.listening = None;
        state.code = None;
        drop(state);
        self.inner.window.mirror(None);
    }

    fn on_event(&self, server: &ServerHandle, event: ServerEvent) {
        let window = &self.inner.window;
        match event {
            ServerEvent::NeedSnapshot { conn, .. } => {
                let server = server.clone();
                window.snapshot(Box::new(move |bodies| {
                    server.snapshot(conn, bodies)
                }));
            }
            ServerEvent::Up { device, body, .. } => {
                if let Up::Renamed(name) = window.up(&device.id, body) {
                    let server = server.clone();
                    self.inner.runtime.spawn(async move {
                        if let Err(error) =
                            server.rename(&device.id, &name).await
                        {
                            eprintln!(
                                "tau-phones: cannot rename a phone: {error}"
                            );
                        }
                    });
                }
            }
            // Its views are built for it no more.
            ServerEvent::Left { device } => window.left(&device),
            ServerEvent::DevicesChanged(paired) => {
                let mut state = self.state();
                // A phone that just paired used the code.
                if paired.len() > state.paired.len() {
                    state.code = None;
                    if let Some(expiry) = state.expiry.take() {
                        expiry.abort();
                    }
                }
                state.paired = paired;
                drop(state);
                self.changed();
            }
        }
    }

    fn show_code(&self) {
        let mut state = self.state();
        let (Some(server), Some(address)) =
            (state.server.clone(), state.listening.clone())
        else {
            return;
        };
        state.code = Some((
            server.pairing_code(address),
            SystemTime::now() + SECRET_LIFETIME,
        ));
        // At its end the code goes.
        let phones = self.clone();
        let expiry = self.inner.runtime.spawn(async move {
            tokio::time::sleep(SECRET_LIFETIME).await;
            phones.hide_code();
            phones.changed();
        });
        if let Some(earlier) = state.expiry.replace(expiry) {
            earlier.abort();
        }
    }

    fn hide_code(&self) {
        let mut state = self.state();
        if let Some(server) = &state.server {
            server.close_pairing();
        }
        state.code = None;
    }
}

fn millis(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH).map_or(0, |since| {
        i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
    })
}

/// The IP to listen on: the one picked if the computer still has it,
/// else Tailscale's, else the first on the network.
fn listen_ip(state: &State) -> Option<String> {
    let addresses = &state.addresses;
    // Loopback is never offered, but kept if picked by hand: for a
    // tunnel, or a test.
    let loopback =
        |ip: &String| ip.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback());
    state
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

/// Whether `ip` is in Tailscale's ranges.
fn tailscale(ip: &str) -> bool {
    match ip.parse::<IpAddr>() {
        // Tailscale's CGNAT range, 100.64.0.0/10.
        Ok(IpAddr::V4(v4)) => matches!(v4.octets(), [100, 64..=127, ..]),
        // Tailscale's IPv6 range, fd7a:115c:a1e0::/48.
        Ok(IpAddr::V6(v6)) => v6.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0],
        Err(_) => false,
    }
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
            let tailscale =
                interface.name.starts_with("tailscale") || tailscale(&ip);
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
#[allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]
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

    /// Tailscale's ranges, and none other.
    #[test]
    fn tailscale_is_its_ranges() {
        assert!(tailscale("100.84.12.7"));
        assert!(tailscale("fd7a:115c:a1e0::1"));
        assert!(!tailscale("100.128.0.1"));
        assert!(!tailscale("192.168.1.4"));
        assert!(!tailscale("not an address"));
    }
}
