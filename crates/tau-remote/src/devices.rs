//! The phones paired with a host, kept in `phones.json`: by name, with
//! the SHA-256 of each one's token, never the token itself. And the one
//! pairing secret that may be open at a time.

use std::{
    io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{pairing::PairingSecret, wire::Refusal};

const FILE: &str = "phones.json";

/// How long a pairing secret stays good.
pub const SECRET_LIFETIME: Duration = Duration::from_secs(5 * 60);

/// A paired phone, as the host lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    /// RFC 3339, UTC.
    pub paired_at: String,
    /// When it last connected; RFC 3339, UTC.
    pub last_seen: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Stored {
    #[serde(flatten)]
    device: Device,
    /// Hexadecimal SHA-256 of the device token.
    token_sha256: String,
    /// The number of the last request taken from it (`crate::outbox`).
    #[serde(default)]
    last_up: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum DevicesError {
    #[error("could not read or write the paired phones: {0}")]
    Io(#[from] io::Error),
    #[error("the paired phones' file is not valid: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug)]
pub(crate) struct Devices {
    path: PathBuf,
    list: Vec<Stored>,
    secret: Option<(PairingSecret, Instant)>,
}

/// The devices, as the server keeps them, and their file: each change
/// is made in memory, then saved with [`DevicesFile::save`].
pub(crate) struct DevicesFile {
    devices: std::sync::Mutex<Devices>,
    /// The saves, one at a time, each writing the list as it is when
    /// its turn comes: so they never land out of order.
    saving: tokio::sync::Mutex<()>,
}

impl DevicesFile {
    pub(crate) async fn load(dir: &Path) -> Result<Self, DevicesError> {
        Ok(Self {
            devices: std::sync::Mutex::new(Devices::load(dir).await?),
            saving: tokio::sync::Mutex::new(()),
        })
    }

    /// The devices, to read or change in memory. Never held across a
    /// save.
    pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, Devices> {
        self.devices.lock().expect("not poisoned")
    }

    /// Writes the list as it is now, through tokio's files (ADR 0028).
    /// It holds token digests: only its owner may read it.
    pub(crate) async fn save(&self) -> Result<(), DevicesError> {
        let _turn = self.saving.lock().await;
        let (path, list) = {
            let devices = self.lock();
            (
                devices.path.clone(),
                serde_json::to_vec_pretty(&devices.list)?,
            )
        };
        tau_ai::files::write_private_async(&path, &list).await?;
        Ok(())
    }
}

impl Devices {
    pub(crate) async fn load(dir: &Path) -> Result<Self, DevicesError> {
        let path = dir.join(FILE);
        let list = match tokio::fs::read(&path).await {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            path,
            list,
            secret: None,
        })
    }

    pub(crate) fn list(&self) -> Vec<Device> {
        self.list
            .iter()
            .map(|stored| stored.device.clone())
            .collect()
    }

    /// A new pairing secret, in place of any open one.
    pub(crate) fn open_secret(&mut self) -> PairingSecret {
        let secret = PairingSecret::random();
        self.secret = Some((secret.clone(), Instant::now()));
        secret
    }

    pub(crate) fn close_secret(&mut self) {
        self.secret = None;
    }

    /// Trades the open secret for a new device and its token. The
    /// secret works once. Like every change here, it is made in memory:
    /// [`DevicesFile::save`] writes it.
    pub(crate) fn pair(
        &mut self,
        secret: &str,
        name: &str,
    ) -> Result<(Device, String), Refusal> {
        let open = self.secret.as_ref().is_some_and(|(open, at)| {
            at.elapsed() < SECRET_LIFETIME
                && PairingSecret::typed(secret).as_ref() == Ok(open)
        });
        if !open {
            return Err(Refusal::BadSecret);
        }
        self.secret = None;
        let token = random_text(32);
        let now = tau_ai::time::rfc3339(tau_ai::time::now_seconds());
        let name = name.trim();
        let device = Device {
            id: random_text(9),
            name: if name.is_empty() { "Phone" } else { name }.to_owned(),
            paired_at: now.clone(),
            last_seen: Some(now),
        };
        self.list.push(Stored {
            device: device.clone(),
            token_sha256: digest(&token),
            last_up: None,
        });
        Ok((device, token))
    }

    /// The device a token belongs to, marked as seen now.
    pub(crate) fn resume(&mut self, token: &str) -> Option<Device> {
        let hash = digest(token);
        let stored = self
            .list
            .iter_mut()
            .find(|stored| stored.token_sha256 == hash)?;
        stored.device.last_seen =
            Some(tau_ai::time::rfc3339(tau_ai::time::now_seconds()));
        Some(stored.device.clone())
    }

    /// Forgets a device; its token is refused from now on.
    pub(crate) fn revoke(&mut self, id: &str) -> bool {
        let before = self.list.len();
        self.list.retain(|stored| stored.device.id != id);
        self.list.len() != before
    }

    /// Whether request `up` from phone `id` is new, and so taken now: one
    /// taken already is not taken again.
    pub(crate) fn take(&mut self, id: &str, up: u64) -> bool {
        let Some(stored) =
            self.list.iter_mut().find(|stored| stored.device.id == id)
        else {
            return false;
        };
        if !crate::outbox::is_new(stored.last_up, up) {
            return false;
        }
        stored.last_up = Some(up);
        true
    }

    pub(crate) fn rename(&mut self, id: &str, name: &str) -> bool {
        let Some(stored) =
            self.list.iter_mut().find(|stored| stored.device.id == id)
        else {
            return false;
        };
        stored.device.name = name.trim().to_owned();
        true
    }
}

fn digest(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `len` random bytes, as base64url.
fn random_text(len: usize) -> String {
    let mut bytes = vec![0; len];
    crate::random(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
#[allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]
mod tests {
    use std::fs;

    use super::*;

    /// Runs `future` to its end, for the tests.
    fn run<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    /// The devices kept in `dir`, after `change` and a save.
    fn changed<T>(dir: &Path, change: impl FnOnce(&mut Devices) -> T) -> T {
        run(async {
            let file = DevicesFile::load(dir).await.unwrap();
            let made = change(&mut file.lock());
            file.save().await.unwrap();
            made
        })
    }

    #[test]
    fn a_secret_pairs_once_and_the_token_resumes() {
        let dir = tempfile::tempdir().unwrap();
        let file = run(DevicesFile::load(dir.path())).unwrap();
        let mut devices = file.lock();
        let secret = devices.open_secret();
        assert_eq!(
            devices.pair("22222222", "Pixel").unwrap_err(),
            Refusal::BadSecret
        );
        let (device, token) = devices
            .pair(&secret.to_string().to_lowercase(), " Pixel ")
            .unwrap();
        assert_eq!(device.name, "Pixel");
        assert_eq!(
            devices.pair(secret.as_str(), "again").unwrap_err(),
            Refusal::BadSecret
        );
        drop(devices);
        run(file.save()).unwrap();
        // What is kept is read back, and holds no token.
        let text = fs::read_to_string(dir.path().join(FILE)).unwrap();
        assert!(!text.contains(&token));
        changed(dir.path(), |read| {
            assert_eq!(read.resume(&token).unwrap().id, device.id);
            assert!(read.rename(&device.id, "Pixel 9"));
        });
        changed(dir.path(), |read| {
            assert_eq!(read.list()[0].name, "Pixel 9");
            assert!(read.revoke(&device.id));
        });
        changed(dir.path(), |read| {
            assert_eq!(read.resume(&token), None);
        });
    }

    /// A request taken stays taken across a restart of the host: sent
    /// again, it is not taken twice.
    #[test]
    fn a_request_is_taken_once_across_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let device = changed(dir.path(), |devices| {
            let secret = devices.open_secret();
            let (device, _) = devices.pair(secret.as_str(), "Pixel").unwrap();
            assert!(devices.take(&device.id, 7));
            assert!(!devices.take(&device.id, 7));
            device
        });
        changed(dir.path(), |read| {
            assert!(!read.take(&device.id, 7), "taken before the restart");
            assert!(!read.take(&device.id, 6));
            assert!(read.take(&device.id, 8));
        });
    }
}
