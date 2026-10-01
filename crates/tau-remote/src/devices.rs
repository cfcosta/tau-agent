//! The phones paired with a host, kept in `phones.json`: by name, with
//! the SHA-256 of each one's token, never the token itself. And the one
//! pairing secret that may be open at a time.

use std::{
    fs,
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

impl Devices {
    pub(crate) fn load(dir: &Path) -> Result<Self, DevicesError> {
        let path = dir.join(FILE);
        let list = match fs::read(&path) {
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

    /// The list holds token digests: only its owner may read it.
    fn save(&self) -> Result<(), DevicesError> {
        let list = serde_json::to_vec_pretty(&self.list)?;
        tau_ai::files::write_private(&self.path, &list)?;
        Ok(())
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
    /// secret works once.
    pub(crate) fn pair(
        &mut self,
        secret: &str,
        name: &str,
    ) -> Result<Result<(Device, String), Refusal>, DevicesError> {
        let open = self.secret.as_ref().is_some_and(|(open, at)| {
            at.elapsed() < SECRET_LIFETIME
                && PairingSecret::typed(secret).as_ref() == Ok(open)
        });
        if !open {
            return Ok(Err(Refusal::BadSecret));
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
        });
        self.save()?;
        Ok(Ok((device, token)))
    }

    /// The device a token belongs to, marked as seen now.
    pub(crate) fn resume(
        &mut self,
        token: &str,
    ) -> Result<Option<Device>, DevicesError> {
        let hash = digest(token);
        let Some(stored) = self
            .list
            .iter_mut()
            .find(|stored| stored.token_sha256 == hash)
        else {
            return Ok(None);
        };
        stored.device.last_seen =
            Some(tau_ai::time::rfc3339(tau_ai::time::now_seconds()));
        let device = stored.device.clone();
        self.save()?;
        Ok(Some(device))
    }

    /// Forgets a device; its token is refused from now on.
    pub(crate) fn revoke(&mut self, id: &str) -> Result<bool, DevicesError> {
        let before = self.list.len();
        self.list.retain(|stored| stored.device.id != id);
        let removed = self.list.len() != before;
        if removed {
            self.save()?;
        }
        Ok(removed)
    }

    pub(crate) fn rename(
        &mut self,
        id: &str,
        name: &str,
    ) -> Result<bool, DevicesError> {
        let Some(stored) =
            self.list.iter_mut().find(|stored| stored.device.id == id)
        else {
            return Ok(false);
        };
        stored.device.name = name.trim().to_owned();
        self.save()?;
        Ok(true)
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
mod tests {
    use super::*;

    #[test]
    fn a_secret_pairs_once_and_the_token_resumes() {
        let dir = tempfile::tempdir().unwrap();
        let mut devices = Devices::load(dir.path()).unwrap();
        let secret = devices.open_secret();
        assert_eq!(
            devices.pair("22222222", "Pixel").unwrap().unwrap_err(),
            Refusal::BadSecret
        );
        let (device, token) = devices
            .pair(&secret.to_string().to_lowercase(), " Pixel ")
            .unwrap()
            .unwrap();
        assert_eq!(device.name, "Pixel");
        assert_eq!(
            devices.pair(secret.as_str(), "again").unwrap().unwrap_err(),
            Refusal::BadSecret
        );
        // What is kept is read back, and holds no token.
        let text = fs::read_to_string(dir.path().join(FILE)).unwrap();
        assert!(!text.contains(&token));
        let mut read = Devices::load(dir.path()).unwrap();
        assert_eq!(read.resume(&token).unwrap().unwrap().id, device.id);
        assert!(read.rename(&device.id, "Pixel 9").unwrap());
        assert_eq!(read.list()[0].name, "Pixel 9");
        assert!(read.revoke(&device.id).unwrap());
        assert_eq!(read.resume(&token).unwrap(), None);
    }
}
