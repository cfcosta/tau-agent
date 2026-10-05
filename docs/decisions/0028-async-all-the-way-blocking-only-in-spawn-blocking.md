# 0028: Async all the way; blocking only in `spawn_blocking`

- Status: accepted.
- Date: 2026-10-05

## Context

On 2026-10-05, 190 `block_on` calls sat in tau's non-test code:

- `tau-ui`'s `Host` is synchronous: 185 of its methods are plain
  functions, which wrap the async store, network and `Vcs` calls in
  `self.runtime.block_on`. The interface reaches it through
  `off_thread`, which runs a method on tokio's blocking pool, so every
  request holds a whole thread while it waits for SQLite or the
  network.
- Some calls skip `off_thread`. `Host::catalog` blocks on SQLite
  (`plugin_spend`) and asks every plugin for its data, and it runs on
  GPUI's thread from `cx.spawn` and from `Push::Catalog`. While SQLite
  waits for its lock (up to the 5-second busy timeout), the window
  cannot draw.
- `tau-vcs`'s `Vcs` gives callers an async API: one thread per
  workspace owns jj-lib, which is blocking and whose futures are not
  `Send`, and runs each job in order. That thread sits idle between
  jobs, one per workspace. `Project`, the repository level, is
  synchronous ("call them off the async executor"), so its callers wrap
  it in `spawn_blocking`.
- Multi-step repository work holds std mutexes across blocking calls:
  `draining` is held for a whole landing.
- The plugins' host half (`tau-ui-plugin`: `starting`, `catalog`,
  `repo_data`, `act`) is synchronous and gets a runtime handle, so
  plugins `block_on` the store and Jev in it.
- Sign-in and GitHub calls run on threads of their own, each with its
  own runtime.

Nothing enforces where blocking may happen, so it spreads, and a slip
onto GPUI's thread freezes the interface.

## Decision

### Where code may block

- **GPUI's thread never blocks and never waits.** It draws state it
  already has and sends requests. A result comes back as an update.
- **tokio's workers run async code only:** no `block_on`, no blocking
  I/O, no long CPU work.
- **Blocking work runs in tokio's `spawn_blocking`,** called from async
  code, and nowhere else. That includes jj-lib: inside the blocking
  closure, its futures are driven with `pollster`, so their not being
  `Send` never reaches async code.
- **A blocking resource with state** (a jj workspace, a jj repository)
  is owned by an async lock, not by a thread. A job takes the lock's
  owned guard, then runs on `spawn_blocking` with it: jobs on one
  resource run one at a time, in the order they asked, and no thread
  waits idle between them. A job that panics drops its state, which the
  next job loads again, since jj-lib's state after a panic cannot be
  trusted.

### Every API between crates is async

- `Host`'s methods that do I/O are `async fn`; `block_on(x)` becomes
  `x.await`.
- `Vcs` keeps its async API, and its thread gives way to the lock and
  `spawn_blocking`.
- `Project` gets an async handle the same way, one lock per repository.
  jj transactions on one repository stop racing each other, and nothing
  else touches the repository.
- The plugins' host half is async: making its state, and every hook
  the host asks (`agent_plugins`, `starting`, `launcher`, `catalog`,
  `data`, `repo_data`, `act`). `HostCx` keeps the runtime's handle, to
  spawn background work on; the lint keeps anything from blocking on it.
  A sub-agent's plugins are built inside the `spawn` call, so `spawn`'s
  factory is async too.
- GitHub and sign-in run as tokio tasks.

### One bridge from the interface to the host

The interface asks with `on_host(host, workspace, |host| async {..},
done, failed)`: the future runs on the host's runtime, and GPUI awaits
its `JoinHandle`, which waits without blocking a thread, then applies
`done` or `failed` on its own thread. `off_thread` and `Host::block_on`
go away.

### The host pushes state; the interface never pulls

The host keeps what the interface shows (the catalog, plugin spend,
landing queues) in memory, and sends a `HostUpdate` when it changes,
on the channel the interface already drains for run events. The
interface only draws what arrived. Phones get the same updates.
`Push::Catalog` asks the host to rebuild the catalog, on its runtime,
and send it.

### A repository serializes its own work

Each repository has a `RepoState` with two async locks
(`tokio::sync::Mutex`):

- `starting`, held while a run starts, a landing rewrites the stack, or
  a sweep reads who owns what;
- `draining`, held while the main chat's landing queue changes or
  drains.

They replace the host-wide `starting` and `draining`, so work in one
repository never waits on another's. What the interface shows of a
repository's queue and the conflicts on its main chat is published as
it changes: each queue operation ends by sending `LandingQueue`, which
phones get too. A watch channel would carry the same, so there is none.
A std `MutexGuard` held across `.await` makes a spawned future not
`Send`, so the compiler finds each place a lock is held across waiting.

### Enforced by clippy

`clippy.toml` disallows `tokio::runtime::Runtime::block_on`,
`tokio::runtime::Handle::block_on`, `pollster::block_on` and
`futures::executor::block_on`. The jj jobs that run in
`spawn_blocking` and `tau-testing`'s helpers allow it where they use
it, each with its reason. While the migration runs, a crate not yet
migrated allows it crate-wide; removing that allow marks the crate
done.

## Consequences

- No request holds a thread while it waits, and nothing on GPUI's
  thread waits for SQLite, jj or the network.
- `Host` stops being callable from synchronous code. Tests drive it
  with `tau_testing::block_on`, as they drive the agent loop.
- jj operations on one repository run one at a time, which they
  effectively had to anyway; forecasts and landings queue behind each
  other instead of retrying on jj's divergence.
- No thread is kept per workspace or repository: blocking threads exist
  only while a job runs, from tokio's pool.
- The interface shows host state with the latency of an update instead
  of a synchronous read. It already works that way for runs.

## Migration

Each step lands on its own, with the tests passing:

1. The lint, with crate-wide allows where `block_on` is still used,
   and the bridge, `on_host`, beside `off_thread`.
2. `Vcs` on the lock and `spawn_blocking`; `Project`'s async handle;
   `tau-vcs` without `pollster` outside its jobs.
3. The plugins' host half to async. It comes before the host: once a
   host function runs on the runtime, a hook that still blocks would
   panic there. Until the host is async, its synchronous functions wait
   on these hooks, which is safe only because nothing on the runtime
   calls them.
4. `Host` to async: store reads and history, landing and the queue,
   repositories and updates, pull requests and pushes. Callers move to
   `on_host`; `off_thread` goes.
5. `RepoState`: the per-repository locks.
6. The catalog and spend pushed instead of read.
7. GitHub and sign-in as tokio tasks; the last crate-wide allow goes.
