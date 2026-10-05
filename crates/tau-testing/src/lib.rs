//! Scripted model, generators and replay for testing tau agents.

pub mod fake_chatgpt;
pub mod fake_openai;
pub mod generators;
pub mod git;
pub mod openai;
pub mod scripted;
pub mod stream;

use std::future::Future;

/// Runs `future` to completion on a current-thread runtime with paused
/// time. For real I/O (a store, files, sockets), see [`block_on_io`].
///
/// Timers advance instantly and tasks run in a repeatable order, so a
/// failing property replays exactly. Never use this with real sockets:
/// paused time jumps forward while the runtime waits on I/O.
#[allow(
    clippy::disallowed_methods,
    reason = "tests are synchronous entry points, and run async code through here (ADR 0027)"
)]
pub fn block_on<F: Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .expect("building a current-thread runtime never fails")
        .block_on(future)
}

thread_local! {
    static IO_RUNTIME: tokio::runtime::Runtime =
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("building a current-thread runtime never fails");
}

/// Runs `future` to completion on this thread's runtime with real time
/// and I/O: what a store, files and a blocking pool need.
///
/// Every call on a thread shares one runtime, so what one call opens
/// (an in-memory store lives in the runtime that opened it) still works
/// in the next.
#[allow(
    clippy::disallowed_methods,
    reason = "tests are synchronous entry points, and run async code through here (ADR 0027)"
)]
pub fn block_on_io<F: Future>(future: F) -> F::Output {
    IO_RUNTIME.with(|runtime| runtime.block_on(future))
}
