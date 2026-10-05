//! A tokio runtime for the interface's own async work that no host runs:
//! signing in to ChatGPT or GitHub, before or beside a host (ADR 0028).
//! The interface spawns on it and awaits the task's handle; nothing
//! waits on it.

use std::{future::Future, sync::LazyLock};

use tokio::{runtime::Runtime, task::JoinHandle};

static RUNTIME: LazyLock<Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("tau-interface")
        .enable_all()
        .build()
        .expect("the interface's runtime builds")
});

/// Runs `future` on the interface's runtime and waits for it: for the
/// process's entry point, before the interface runs.
#[allow(
    clippy::disallowed_methods,
    reason = "the process's entry point, before the interface runs (ADR 0028)"
)]
pub fn block_at_entry<T>(future: impl Future<Output = T>) -> T {
    RUNTIME.block_on(future)
}

/// Runs `future` on the interface's runtime.
pub(crate) fn spawn<T: Send + 'static>(
    future: impl Future<Output = T> + Send + 'static,
) -> JoinHandle<T> {
    RUNTIME.spawn(future)
}
