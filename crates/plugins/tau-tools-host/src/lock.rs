//! Per-path locks for read-modify-write tools (`docs/reference/tools.md`,
//! "edit", "Serialization"), ported from pi's `file-mutation-queue.ts`.
//!
//! `edit` and `write` hold the lock of the file they change for the
//! whole read-modify-write, so parallel calls on one file run one after
//! another, in the order they asked, and calls on different files run
//! together. The key is the canonical path, so a symlink and its target
//! share a lock; a file that does not exist yet is keyed by its path.
//! The locks are process-wide: every tool instance shares them.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex, Weak},
};

use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

type Locks = Mutex<HashMap<PathBuf, Weak<AsyncMutex<()>>>>;

static LOCKS: LazyLock<Locks> = LazyLock::new(Locks::default);

/// Held while a file is being changed; dropping it lets the next caller
/// in.
#[derive(Debug)]
pub struct PathGuard {
    _guard: OwnedMutexGuard<()>,
}

/// The lock key for `path`: its canonical form, or `path` itself when it
/// cannot be resolved (it does not exist yet).
pub fn key(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned())
}

/// Waits for the lock of `path`, in the order callers asked for it
/// (tokio's mutex is fair).
pub async fn lock(path: &Path) -> PathGuard {
    let mutex = {
        let mut locks = LOCKS.lock().expect("not poisoned");
        let key = key(path);
        match locks.get(&key).and_then(Weak::upgrade) {
            Some(mutex) => mutex,
            None => {
                // Forget locks nobody holds any more, so the map does
                // not grow with every file ever touched.
                locks.retain(|_, weak| weak.strong_count() > 0);
                let mutex = Arc::new(AsyncMutex::new(()));
                locks.insert(key, Arc::downgrade(&mutex));
                mutex
            }
        }
    };
    PathGuard {
        _guard: mutex.lock_owned().await,
    }
}
