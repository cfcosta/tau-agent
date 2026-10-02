//! Private byte storage for opaque artifact references.
//!
//! Publication streams to a private staging file. A directory-wide lock
//! serializes quota checks and atomic, no-clobber publication across handles
//! and processes. No public method resolves an arbitrary artifact ID to bytes;
//! an authorized range API can be added here later.

use std::{
    fs::{self, File, OpenOptions, Permissions},
    io::{self, Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const CHUNK_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quotas {
    pub max_artifact_bytes: u64,
    pub total_bytes: u64,
}

impl Default for Quotas {
    fn default() -> Self {
        Self {
            max_artifact_bytes: 64 * 1024 * 1024,
            total_bytes: 512 * 1024 * 1024,
        }
    }
}

/// Immutable metadata. The ID is an opaque reference, not a file path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Artifact {
    id: String,
    digest: String,
    size_bytes: u64,
}

impl Artifact {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn digest(&self) -> &str {
        &self.digest
    }

    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    fn validate(&self) -> Result<()> {
        let id =
            Uuid::parse_str(&self.id).map_err(|_| Error::InvalidArtifact)?;
        if id.get_version_num() != 7 || id.hyphenated().to_string() != self.id {
            return Err(Error::InvalidArtifact);
        }
        if self.digest.len() != 64
            || !self.digest.bytes().all(|byte| {
                byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
            })
        {
            return Err(Error::InvalidArtifact);
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for Artifact {
    fn deserialize<D: Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Fields {
            id: String,
            digest: String,
            size_bytes: u64,
        }

        let fields = Fields::deserialize(deserializer)?;
        let artifact = Self {
            id: fields.id,
            digest: fields.digest,
            size_bytes: fields.size_bytes,
        };
        artifact.validate().map_err(serde::de::Error::custom)?;
        Ok(artifact)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("artifact storage I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("artifact publication was cancelled")]
    Cancelled,
    #[error("artifact exceeds its byte quota")]
    ArtifactTooLarge,
    #[error("artifact storage exceeds its total byte quota")]
    TotalQuotaExceeded,
    #[error("invalid artifact metadata")]
    InvalidArtifact,
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone)]
pub struct Bytes(Arc<Storage>);

struct Storage {
    objects: PathBuf,
    lock_path: PathBuf,
    quotas: Quotas,
}

impl Bytes {
    pub fn new(path: impl AsRef<Path>, quotas: Quotas) -> Result<Self> {
        let root = path.as_ref();
        fs::create_dir_all(root)?;
        fs::set_permissions(root, Permissions::from_mode(0o700))?;
        let objects = root.join("objects");
        fs::create_dir_all(&objects)?;
        fs::set_permissions(&objects, Permissions::from_mode(0o700))?;
        let lock_path = root.join(".publish.lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(&lock_path)?;
        lock.set_permissions(Permissions::from_mode(0o600))?;
        Ok(Self(Arc::new(Storage {
            objects,
            lock_path,
            quotas,
        })))
    }

    /// Publish raw bytes. A failure leaves no published object or reference.
    pub fn publish_reader<R: Read>(
        &self,
        mut reader: R,
        cancellation: &CancellationToken,
    ) -> Result<Artifact> {
        let mut staging = NamedTempFile::new_in(&self.0.objects)?;
        let mut digest = Sha256::new();
        let mut size_bytes = 0_u64;
        let mut chunk = [0_u8; CHUNK_BYTES];

        loop {
            if cancellation.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let count = match reader.read(&mut chunk) {
                Ok(count) => count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                    continue;
                }
                Err(error) => return Err(Error::Io(error)),
            };
            if cancellation.is_cancelled() {
                return Err(Error::Cancelled);
            }
            if count == 0 {
                break;
            }
            size_bytes = size_bytes
                .checked_add(count as u64)
                .ok_or(Error::ArtifactTooLarge)?;
            if size_bytes > self.0.quotas.max_artifact_bytes {
                return Err(Error::ArtifactTooLarge);
            }
            staging.write_all(&chunk[..count])?;
            digest.update(&chunk[..count]);
        }

        staging.as_file_mut().sync_all()?;
        staging
            .as_file()
            .set_permissions(Permissions::from_mode(0o400))?;
        staging.as_file().sync_all()?;

        let _lock = PublicationLock::acquire(&self.0.lock_path)?;
        if cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let retained_bytes = retained_bytes(&self.0.objects)?;
        if size_bytes > self.0.quotas.total_bytes.saturating_sub(retained_bytes)
        {
            return Err(Error::TotalQuotaExceeded);
        }

        let artifact = Artifact {
            id: Uuid::now_v7().to_string(),
            digest: digest
                .finalize()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
            size_bytes,
        };
        let path = self.0.objects.join(format!("{}.blob", artifact.id));
        staging
            .persist_noclobber(&path)
            .map_err(|error| Error::Io(error.error))?;
        if let Err(error) = File::open(&self.0.objects)
            .and_then(|directory| directory.sync_all())
        {
            let _ = fs::remove_file(&path);
            return Err(Error::Io(error));
        }
        Ok(artifact)
    }

    /// Restricted to this crate until the authorized range API is built.
    #[allow(dead_code)]
    pub(crate) fn open_object(&self, artifact: &Artifact) -> Result<File> {
        artifact.validate()?;
        let path = self.0.objects.join(format!("{}.blob", artifact.id));
        let file = File::open(path)?;
        if file.metadata()?.len() != artifact.size_bytes {
            return Err(Error::InvalidArtifact);
        }
        Ok(file)
    }
}

fn retained_bytes(objects: &Path) -> Result<u64> {
    let mut total = 0_u64;
    for entry in fs::read_dir(objects)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(id) =
            name.to_str().and_then(|name| name.strip_suffix(".blob"))
        else {
            continue; // Ignore private staging files left by a crashed writer.
        };
        let Ok(uuid) = Uuid::parse_str(id) else {
            continue;
        };
        if uuid.get_version_num() != 7 || uuid.hyphenated().to_string() != id {
            continue;
        }
        let metadata = entry.metadata()?;
        if !metadata.is_file() {
            return Err(Error::InvalidArtifact);
        }
        total = total
            .checked_add(metadata.len())
            .ok_or(Error::TotalQuotaExceeded)?;
    }
    Ok(total)
}

struct PublicationLock(File);

impl PublicationLock {
    fn acquire(path: &Path) -> Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        loop {
            // SAFETY: `file` owns a valid descriptor throughout this call.
            let status =
                unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
            if status == 0 {
                return Ok(Self(file));
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(Error::Io(error));
            }
        }
    }
}

impl Drop for PublicationLock {
    fn drop(&mut self) {
        // SAFETY: `self.0` remains open until after this destructor returns.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

#[cfg(test)]
mod tests;
