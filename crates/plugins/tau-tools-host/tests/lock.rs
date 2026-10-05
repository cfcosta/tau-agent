//! Per-path locks (`tau_tools_host::lock`), under paused time.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use tau_testing::block_on;
use tau_tools_host::lock::{key, lock};

/// Holders of one path's lock run one after another, in the order they
/// asked; a symlink and its target share the lock; another path's lock
/// is free meanwhile.
#[test]
fn one_path_at_a_time_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("file.txt");
    std::fs::write(&target, "x").unwrap();
    let link = dir.path().join("link.txt");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    assert_eq!(key(&link), key(&target));

    block_on(async {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut tasks = Vec::new();
        for (i, path) in [&target, &link, &target].into_iter().enumerate() {
            let log = log.clone();
            let path = path.clone();
            tasks.push(tokio::spawn(async move {
                let _guard = lock(&path).await;
                log.lock().unwrap().push(format!("start {i}"));
                tokio::time::sleep(Duration::from_millis(10)).await;
                log.lock().unwrap().push(format!("end {i}"));
            }));
            // Ask in order.
            tokio::task::yield_now().await;
        }
        // Another file is not held up.
        let other = dir.path().join("other.txt");
        let started = tokio::time::Instant::now();
        drop(lock(&other).await);
        assert_eq!(started.elapsed(), Duration::ZERO);
        for task in tasks {
            task.await.unwrap();
        }
        assert_eq!(
            *log.lock().unwrap(),
            ["start 0", "end 0", "start 1", "end 1", "start 2", "end 2"]
        );
    });
}

/// A file that does not exist yet is keyed by its path, and the lock is
/// reusable once released.
#[test]
fn missing_files_lock_by_path() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("new.txt");
    assert_eq!(key(&missing), missing);
    block_on(async {
        drop(lock(&missing).await);
        drop(lock(&missing).await);
    });
}

/// Taking the lock of a new path never frees a lock still held: while
/// one path's lock is held, locking other paths (which tidies the lock
/// table) leaves the held path locked.
#[test]
fn locking_other_paths_keeps_a_held_lock() {
    let dir = tempfile::tempdir().unwrap();
    let held = dir.path().join("held.txt");
    block_on(async {
        let guard = lock(&held).await;
        for i in 0..3 {
            drop(lock(&dir.path().join(format!("other{i}.txt"))).await);
        }
        let waiter = tokio::spawn({
            let held = held.clone();
            async move { drop(lock(&held).await) }
        });
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(!waiter.is_finished(), "the held lock was handed out twice");
        drop(guard);
        waiter.await.unwrap();
    });
}
