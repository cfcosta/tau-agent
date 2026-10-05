//! Files only their owner may read: credentials, keys, tokens.

use std::{
    fs,
    io::{self, Write as _},
    path::{Path, PathBuf},
};

/// Creates `dir` and its missing parents, each readable by its owner
/// only (0700).
pub fn create_private_dir(dir: &Path) -> io::Result<()> {
    if dir.as_os_str().is_empty() || dir.is_dir() {
        return Ok(());
    }
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)
}

/// Options that create files readable by their owner only (0600).
pub fn private_options() -> fs::OpenOptions {
    let mut options = fs::OpenOptions::new();
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options
}

/// Writes `bytes` to a new owner-only file beside `path`, creating the
/// directory owner-only too, and returns where.
pub fn write_private_temp(path: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
    let dir = path.parent().unwrap_or(Path::new("."));
    create_private_dir(dir)?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut suffix = [0u8; 6];
    getrandom::fill(&mut suffix).map_err(io::Error::other)?;
    let suffix: String = suffix.iter().map(|b| format!("{b:02x}")).collect();
    let temp = dir.join(format!(".{name}.{suffix}.tmp"));
    let written = (|| {
        let mut file =
            private_options().write(true).create_new(true).open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()
    })();
    match written {
        Ok(()) => Ok(temp),
        Err(error) => {
            let _ = fs::remove_file(&temp);
            Err(error)
        }
    }
}

/// Replaces `path` with `bytes` in one rename, owner-only: a crash never
/// leaves half a file, and no one else ever reads it.
pub fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temp = write_private_temp(path, bytes)?;
    fs::rename(&temp, path).inspect_err(|_| {
        let _ = fs::remove_file(&temp);
    })
}

/// [`write_private`] through tokio's files, for async code (ADR 0028):
/// the new contents go to an owner-only file beside `path`, which then
/// takes its place, so a reader sees the old contents or the new ones.
pub async fn write_private_async(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use tokio::io::AsyncWriteExt as _;
    let dir = path.parent().unwrap_or(Path::new("."));
    if !dir.as_os_str().is_empty() && !tokio::fs::try_exists(dir).await? {
        let mut builder = tokio::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        builder.mode(0o700);
        builder.create(dir).await?;
    }
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut suffix = [0u8; 6];
    getrandom::fill(&mut suffix).map_err(io::Error::other)?;
    let suffix: String = suffix.iter().map(|b| format!("{b:02x}")).collect();
    let temp = dir.join(format!(".{name}.{suffix}.tmp"));
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let written = async {
        let mut file = options.open(&temp).await?;
        file.write_all(bytes).await?;
        file.sync_all().await?;
        tokio::fs::rename(&temp, path).await
    }
    .await;
    if written.is_err() {
        let _ = tokio::fs::remove_file(&temp).await;
    }
    written
}

#[cfg(test)]
#[allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]
mod tests {
    use super::*;

    /// The async write leaves the contents, readable by their owner only,
    /// in a directory it made owner-only too, and no temporary file.
    #[test]
    fn an_async_write_is_private_and_whole() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deep").join("key");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime
            .block_on(write_private_async(&path, b"first"))
            .unwrap();
        runtime
            .block_on(write_private_async(&path, b"second"))
            .unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = |path: &Path| {
                fs::metadata(path).unwrap().permissions().mode() & 0o777
            };
            assert_eq!(mode(&path), 0o600);
            assert_eq!(mode(path.parent().unwrap()), 0o700);
        }
        let names: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["key"]);
    }
}
