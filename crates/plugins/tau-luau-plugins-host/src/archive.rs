//! `ctx.archive`: files that hold what a plugin cut from the context,
//! whole, for the model to read back (ADR 0050, for tau-fast-compaction).
//! Each is new, only its owner can read it, and it lies in the run's
//! archive directory.

use std::{
    io::Write as _,
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

/// Names files apart within a process.
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A path under `dir` no other archive of this process takes, and none
/// of another's: `tau-<kind>-<pid>-<nanos>-<n>.txt`, `kind` kept to
/// letters, digits and dashes. Nothing is written.
pub fn new_path(dir: &Path, kind: &str) -> PathBuf {
    let kind: String = kind
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(32)
        .collect();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    dir.join(format!("tau-{kind}-{}-{nanos}-{n}.txt", std::process::id()))
}

/// Writes `text` to the new file `path`, which must be one of
/// [`new_path`]'s in `dir`, readable by its owner alone. Fails if the
/// file exists.
pub fn write(dir: &Path, path: &str, text: &str) -> Result<(), String> {
    let path = Path::new(path);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if path.parent() != Some(dir)
        || !name.starts_with("tau-")
        || !name.ends_with(".txt")
    {
        return Err(format!(
            "{} is not an archive's path: ask ctx.archive.path for one",
            path.display()
        ));
    }
    let written = (|| {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()
    })();
    written.map_err(|error| {
        format!("cannot archive to {}: {error}", path.display())
    })
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn an_archive_is_new_and_its_owners_alone() {
        let scratch = tempfile::tempdir().unwrap();
        let dir = scratch.path().join("archive");
        let path = new_path(&dir, "out put/../x");
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("tau-outputx-"), "{name}");
        assert_ne!(path, new_path(&dir, "output"));
        let text = path.to_str().unwrap();
        write(&dir, text, "whole").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "whole");
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(&dir), 0o700);
        // It is never written over.
        assert!(write(&dir, text, "again").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "whole");
    }

    #[test]
    fn only_a_path_it_gave_is_written() {
        let scratch = tempfile::tempdir().unwrap();
        let dir = scratch.path().join("archive");
        let elsewhere = scratch.path().join("tau-x.txt");
        for path in [
            elsewhere.to_str().unwrap().to_owned(),
            dir.join("notes.txt").to_str().unwrap().to_owned(),
            dir.join("tau-x.sh").to_str().unwrap().to_owned(),
        ] {
            let refused = write(&dir, &path, "text").unwrap_err();
            assert!(refused.contains("ctx.archive.path"), "{refused}");
            assert!(!Path::new(&path).exists(), "{path}");
        }
    }
}
