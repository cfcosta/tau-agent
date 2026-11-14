//! Where plugin tests run: a pool of threads, each with a runtime of its
//! own, taking the costliest job waiting first.
//!
//! - **A thread a job.** A job runs to its end on one thread: hegel's
//!   error channel is the thread's (`hegel.rs`), and a VM is busy with
//!   the CPU while it runs, so jobs on threads of their own run at once.
//! - **Costliest first.** A job says what it costs, by what it took the
//!   last time or what it asks for, and the pool starts the costliest
//!   waiting: the longest tests start first, and do not end last.
//! - **One pool a process,** of [`jobs`] threads, made when first used:
//!   every package tested in the process shares it.

use std::{
    collections::{BinaryHeap, HashMap},
    future::Future,
    sync::{Condvar, Mutex, OnceLock},
};

/// How many jobs run at once, once the pool is made: [`set_jobs`], or
/// every core.
static JOBS: OnceLock<usize> = OnceLock::new();

/// What each test took the last time, in milliseconds, by
/// `package/file/name`: what a job costs when the runner kept it.
static TIMINGS: OnceLock<HashMap<String, u64>> = OnceLock::new();

static POOL: OnceLock<Pool> = OnceLock::new();

/// Runs `n` jobs at once, when called before the pool is first used.
/// Gives back whether it took.
pub fn set_jobs(n: usize) -> bool {
    JOBS.set(n.max(1)).is_ok()
}

/// How many jobs run at once.
pub fn jobs() -> usize {
    *JOBS.get_or_init(|| {
        std::thread::available_parallelism().map_or(1, usize::from)
    })
}

/// What each test took the last time, by `package/file/name`, for the
/// costliest to start first. Gives back whether it took.
pub fn set_timings(timings: HashMap<String, u64>) -> bool {
    TIMINGS.set(timings).is_ok()
}

/// What the test `name` of `file` in `package` took the last time.
pub fn took(package: &str, file: &str, name: &str) -> Option<u64> {
    TIMINGS.get()?.get(&key(package, file, name)).copied()
}

/// A test's key among the timings.
pub fn key(package: &str, file: &str, name: &str) -> String {
    format!("{package}/{file}/{name}")
}

/// Runs `job` on one of the pool's threads, once no costlier job waits,
/// and gives what it came to. The future `job` makes runs on that
/// thread's runtime, to its end.
pub async fn run<T, F, Fut>(cost: u64, job: F) -> T
where
    T: Send + 'static,
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = T> + 'static,
{
    let (tx, rx) = tokio::sync::oneshot::channel();
    pool().push(
        cost,
        Box::new(move |runtime: &tokio::runtime::Runtime| {
            #[allow(
                clippy::disallowed_methods,
                reason = "a pool thread is a synchronous entry point of its own (ADR 0028)"
            )]
            let _ = tx.send(runtime.block_on(job()));
        }),
    );
    // A job that panicked dropped its sender: the panic is the caller's.
    rx.await
        .expect("the test's job panicked on its pool thread")
}

type Job = Box<dyn FnOnce(&tokio::runtime::Runtime) + Send>;

struct Queued {
    cost: u64,
    /// Earlier first, among jobs of one cost.
    seq: std::cmp::Reverse<u64>,
    job: Job,
}

impl PartialEq for Queued {
    fn eq(&self, other: &Self) -> bool {
        (self.cost, self.seq) == (other.cost, other.seq)
    }
}

impl Eq for Queued {}

impl PartialOrd for Queued {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Queued {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.cost, self.seq).cmp(&(other.cost, other.seq))
    }
}

struct Pool {
    waiting: Mutex<(BinaryHeap<Queued>, u64)>,
    ready: Condvar,
}

fn pool() -> &'static Pool {
    POOL.get_or_init(|| {
        for at in 0..jobs() {
            std::thread::Builder::new()
                .name(format!("tau-test-{at}"))
                .spawn(work)
                .expect("a test thread starts");
        }
        Pool {
            waiting: Mutex::new((BinaryHeap::new(), 0)),
            ready: Condvar::new(),
        }
    })
}

impl Pool {
    fn push(&self, cost: u64, job: Job) {
        let mut waiting = self.waiting.lock().expect("not poisoned");
        let seq = waiting.1;
        waiting.1 += 1;
        waiting.0.push(Queued {
            cost,
            seq: std::cmp::Reverse(seq),
            job,
        });
        self.ready.notify_one();
    }

    fn take(&self) -> Job {
        let mut waiting = self.waiting.lock().expect("not poisoned");
        loop {
            if let Some(queued) = waiting.0.pop() {
                return queued.job;
            }
            waiting = self.ready.wait(waiting).expect("not poisoned");
        }
    }
}

/// A pool thread: takes jobs, the costliest first, for as long as the
/// process lives.
fn work() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("building a current-thread runtime never fails");
    loop {
        let job = pool().take();
        // A job that panics ends, not the thread: the pool keeps its size.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            job(&runtime);
        }));
    }
}
