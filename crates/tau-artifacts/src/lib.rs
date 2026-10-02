//! Private byte storage for opaque artifact references.
//!
//! Publication streams to a private staging file. A directory-wide lock
//! serializes quota checks and atomic, no-clobber publication across handles
//! and processes. No public method resolves an arbitrary artifact ID to bytes;
//! callers must supply immutable metadata from an authorized scope.

use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions, Permissions},
    io::{self, Read, Seek, SeekFrom, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    sync::Arc,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const CHUNK_BYTES: usize = 64 * 1024;
pub const MAX_RANGE_BYTES: u32 = 65_536;
pub const DEFAULT_RANGE_BYTES: u32 = 32_768;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Encoding {
    #[default]
    Utf8,
    Base64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    pub id: String,
    pub offset: u64,
    pub next_offset: Option<u64>,
    pub size_bytes: u64,
    pub encoding: Encoding,
    pub data: String,
    pub eof: bool,
    /// True only when this one range contains the whole artifact from byte zero.
    pub complete: bool,
}

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
    #[error("artifact bytes are missing")]
    MissingArtifact,
    #[error("retention roots conflict or their bytes do not match")]
    InvalidRoot,
    #[error("artifact range limit must be between 1 and 65536 bytes")]
    InvalidLimit,
    #[error("artifact offset is beyond the end")]
    OffsetOutOfRange,
    #[error("UTF-8 offset starts inside a codepoint")]
    Utf8Start,
    #[error("artifact bytes are not valid UTF-8; use base64")]
    InvalidUtf8,
    #[error("range limit is too small for one UTF-8 codepoint")]
    Utf8LimitTooSmall,
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug)]
pub struct Bytes(Arc<Storage>);

#[derive(Debug)]
struct Storage {
    objects: PathBuf,
    lock_path: PathBuf,
    quotas: Quotas,
}

/// Holds the publication lock until a caller has durably recorded the grant.
/// Dropping it after an error leaves an orphan for explicit maintenance.
pub struct PublicationLease {
    _guard: PublicationLock,
    lock_path: PathBuf,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct PruneReport {
    pub removed_objects: u64,
    pub reclaimed_bytes: u64,
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
        reader: R,
        cancellation: &CancellationToken,
    ) -> Result<Artifact> {
        let (artifact, _lease) =
            self.publish_leased_reader(reader, cancellation)?;
        Ok(artifact)
    }

    /// Keep the publication lock across the subsequent grant write. A prune
    /// acquiring the same lock sees either the recorded grant or no object.
    pub fn publish_leased_reader<R: Read>(
        &self,
        mut reader: R,
        cancellation: &CancellationToken,
    ) -> Result<(Artifact, PublicationLease)> {
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
        Ok((
            artifact,
            PublicationLease {
                _guard: _lock,
                lock_path: self.0.lock_path.clone(),
            },
        ))
    }

    /// Mark trusted, complete retained-history roots and sweep published
    /// objects. This function cannot decide whether caller roots are complete.
    /// The caller must collect them while holding this lease, under the same
    /// lock used by publication and grant recording.
    pub fn prune_with_roots(
        &self,
        roots: impl IntoIterator<Item = Artifact>,
        lease: &PublicationLease,
    ) -> Result<PruneReport> {
        if lease.lock_path != self.0.lock_path {
            return Err(Error::InvalidRoot);
        }
        let mut marked = HashMap::<String, Artifact>::new();
        for artifact in roots {
            artifact.validate()?;
            if let Some(previous) =
                marked.insert(artifact.id.clone(), artifact.clone())
                && previous != artifact
            {
                return Err(Error::InvalidRoot);
            }
        }
        // Check every root before unlinking a single object. An unreadable or
        // damaged retained object makes the entire maintenance pass fail closed.
        for artifact in marked.values() {
            let mut file =
                self.open_object(artifact).map_err(|_| Error::InvalidRoot)?;
            let mut digest = Sha256::new();
            let mut chunk = [0_u8; CHUNK_BYTES];
            loop {
                let count = file.read(&mut chunk)?;
                if count == 0 {
                    break;
                }
                digest.update(&chunk[..count]);
            }
            let actual: String = digest
                .finalize()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            if actual != artifact.digest {
                return Err(Error::InvalidRoot);
            }
        }
        let mut objects = Vec::new();
        let mut seen = HashSet::new();
        for entry in fs::read_dir(&self.0.objects)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(id) =
                name.to_str().and_then(|name| name.strip_suffix(".blob"))
            else {
                continue; // Staging files are never swept: an old writer may still own one.
            };
            let uuid =
                Uuid::parse_str(id).map_err(|_| Error::InvalidArtifact)?;
            if uuid.get_version_num() != 7
                || uuid.hyphenated().to_string() != id
                || !seen.insert(id.to_owned())
            {
                return Err(Error::InvalidArtifact);
            }
            let metadata = fs::symlink_metadata(entry.path())?;
            if !metadata.is_file() {
                return Err(Error::InvalidArtifact);
            }
            if !marked.contains_key(id) {
                objects.push((entry.path(), metadata.len()));
            }
        }
        let mut report = PruneReport::default();
        for (path, size) in objects {
            fs::remove_file(path)?;
            report.removed_objects += 1;
            report.reclaimed_bytes += size;
        }
        File::open(&self.0.objects)?.sync_all()?;
        Ok(report)
    }

    /// Lock before collecting persisted roots; keep this through pruning.
    pub fn lock_publication(&self) -> Result<PublicationLease> {
        Ok(PublicationLease {
            _guard: PublicationLock::acquire(&self.0.lock_path)?,
            lock_path: self.0.lock_path.clone(),
        })
    }

    fn open_object(&self, artifact: &Artifact) -> Result<File> {
        artifact.validate()?;
        let path = self.0.objects.join(format!("{}.blob", artifact.id));
        let file = File::open(path).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                Error::MissingArtifact
            } else {
                Error::Io(error)
            }
        })?;
        if file.metadata()?.len() != artifact.size_bytes {
            return Err(Error::InvalidArtifact);
        }
        Ok(file)
    }

    /// Read at most `limit` bytes from metadata resolved by the caller's
    /// grant scope. This function does no authorization of its own.
    pub fn read_range(
        &self,
        artifact: &Artifact,
        offset: u64,
        limit: u32,
        encoding: Encoding,
        cancellation: &CancellationToken,
    ) -> Result<Range> {
        if !(1..=MAX_RANGE_BYTES).contains(&limit) {
            return Err(Error::InvalidLimit);
        }
        if cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let mut file = self.open_object(artifact)?;
        if offset > artifact.size_bytes {
            return Err(Error::OffsetOutOfRange);
        }
        let count =
            (artifact.size_bytes - offset).min(u64::from(limit)) as usize;
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = vec![0; count];
        file.read_exact(&mut bytes).map_err(|error| {
            if error.kind() == io::ErrorKind::UnexpectedEof {
                Error::InvalidArtifact
            } else {
                Error::Io(error)
            }
        })?;
        if cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if file.metadata()?.len() != artifact.size_bytes {
            return Err(Error::InvalidArtifact);
        }
        let used = match encoding {
            Encoding::Base64 => count,
            Encoding::Utf8 => {
                if bytes.first().is_some_and(|byte| byte & 0xc0 == 0x80) {
                    return Err(Error::Utf8Start);
                }
                match std::str::from_utf8(&bytes) {
                    Ok(_) => count,
                    Err(error) if error.error_len().is_some() => {
                        return Err(Error::InvalidUtf8);
                    }
                    Err(error)
                        if offset + count as u64 == artifact.size_bytes =>
                    {
                        let _ = error;
                        return Err(Error::InvalidUtf8);
                    }
                    Err(error) if error.valid_up_to() == 0 => {
                        return Err(Error::Utf8LimitTooSmall);
                    }
                    Err(error) => error.valid_up_to(),
                }
            }
        };
        let next = offset + used as u64;
        let eof = next == artifact.size_bytes;
        let data = match encoding {
            Encoding::Utf8 => String::from_utf8(bytes[..used].to_vec())
                .map_err(|_| Error::InvalidUtf8)?,
            Encoding::Base64 => STANDARD.encode(&bytes),
        };
        Ok(Range {
            id: artifact.id.clone(),
            offset,
            next_offset: (!eof).then_some(next),
            size_bytes: artifact.size_bytes,
            encoding,
            data,
            eof,
            complete: offset == 0 && eof,
        })
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
        let uuid = Uuid::parse_str(id).map_err(|_| Error::InvalidArtifact)?;
        if uuid.get_version_num() != 7 || uuid.hyphenated().to_string() != id {
            return Err(Error::InvalidArtifact);
        }
        let metadata = fs::symlink_metadata(entry.path())?;
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
