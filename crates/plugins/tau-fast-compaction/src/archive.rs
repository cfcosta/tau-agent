//! Files that hold what pruning took out of the context, whole, for the
//! model to read back.

use std::{
    io::Write as _,
    os::unix::fs::OpenOptionsExt as _,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

/// Names files apart within a process.
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A path under `dir` no other archive of this process takes, and none
/// of another's: `tau-<kind>-<pid>-<nanos>-<n>.txt`. Nothing is written.
pub fn new_path(dir: &Path, kind: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    dir.join(format!("tau-{kind}-{}-{nanos}-{n}.txt", std::process::id()))
}

/// Writes `text` to a new file at `path` that only its owner can read.
/// Fails if the file exists.
pub fn write(path: &Path, text: &str) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()
}
