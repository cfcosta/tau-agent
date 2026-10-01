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
