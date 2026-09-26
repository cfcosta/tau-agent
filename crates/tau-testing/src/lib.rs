//! Scripted model, generators and replay for testing tau agents.

pub mod generators;
pub mod stream;

use std::future::Future;

/// Runs `future` to completion on a current-thread runtime with paused
/// time.
///
/// Timers advance instantly and tasks run in a repeatable order, so a
/// failing property replays exactly. Never use this with real sockets:
/// paused time jumps forward while the runtime waits on I/O.
pub fn block_on<F: Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .expect("building a current-thread runtime never fails")
        .block_on(future)
}
