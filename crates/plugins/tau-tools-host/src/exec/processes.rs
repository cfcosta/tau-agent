//! The processes a run started and left running (`exec_command`), kept
//! between its tool calls and turns, as Codex keeps its background
//! terminals (`unified_exec`).
//!
//! A process runs under a pseudo-terminal. A task reads its events as
//! they come and keeps what it wrote until a call reads it, so output
//! is not lost between polls. [`Processes`] keeps up to [`MAX_PROCESSES`]
//! of them by id; past that, the least recently used goes, one that
//! exited first, and the newest few stay. A process whose end a call
//! reported leaves. Dropping the store, or a process, kills its process
//! group.

use std::{
    collections::HashMap,
    sync::{
        Arc,
        Mutex,
        MutexGuard,
        atomic::{AtomicI32, Ordering},
    },
    time::Instant,
};

use tau_terminal::{Command, Event, Input, Killer, Size};
use tokio::sync::watch;

use super::clean::{Cleaner, Taken};

/// The most processes kept at once.
pub const MAX_PROCESSES: usize = 64;

/// How many of the most recently used are never pruned.
const PROTECTED: usize = 8;

/// The most raw output kept for the terminal view of one call, in bytes:
/// the newest.
pub const MAX_RAW: usize = 4 << 20;

/// How a process ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exit {
    /// Its exit code (128 + N for signal N).
    pub code: i32,
}

/// What a process wrote, and whether it is done.
#[derive(Debug)]
pub struct Shared {
    state: Mutex<State>,
    /// Bumped whenever the state changes.
    changed: watch::Sender<u64>,
}

#[derive(Debug, Default)]
struct State {
    cleaner: Cleaner,
    /// The raw bytes not handed to a call yet.
    raw: Vec<u8>,
    /// Raw bytes lost to [`MAX_RAW`].
    raw_lost: bool,
    exit: Option<Exit>,
    /// Every byte it wrote, counted.
    output_bytes: u64,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("not poisoned")
    }

    fn touch(&self) {
        self.changed.send_modify(|n| *n += 1);
    }
}

/// What a call read from a process.
#[derive(Debug, Default)]
pub struct Read {
    pub text: Taken,
    /// The raw output since the last read.
    pub raw: Vec<u8>,
    pub raw_lost: bool,
    pub exit: Option<Exit>,
    pub output_bytes: u64,
}

/// One process, as the store holds it. Dropping it kills the process
/// group.
pub struct Process {
    pub id: i32,
    pub command: String,
    pub size: Size,
    pub started: Instant,
    shared: Arc<Shared>,
    killer: Killer,
    input: Option<Input>,
    last_used: Mutex<Instant>,
    /// One call at a time reads or writes it.
    pub(super) turn: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for Process {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Process")
            .field("id", &self.id)
            .field("command", &self.command)
            .finish_non_exhaustive()
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if self.shared.state().exit.is_none() {
            self.killer.kill();
        }
    }
}

impl Process {
    /// Whether it reads input: it started on a terminal of its own
    /// (`tty`).
    pub fn has_input(&self) -> bool {
        self.input.is_some()
    }

    /// Writes `chars` to its terminal.
    pub async fn write(&self, chars: &str) -> std::io::Result<()> {
        match &self.input {
            Some(input) => input.write(chars.as_bytes()).await,
            None => Err(std::io::Error::other("no input")),
        }
    }

    pub fn has_exited(&self) -> bool {
        self.shared.state().exit.is_some()
    }

    /// A receiver that wakes when the process writes or ends.
    pub fn changes(&self) -> watch::Receiver<u64> {
        self.shared.changed.subscribe()
    }

    /// Everything unread, handed over.
    pub fn read(&self) -> Read {
        *self.last_used.lock().expect("not poisoned") = Instant::now();
        let mut state = self.shared.state();
        Read {
            text: state.cleaner.take(),
            raw: std::mem::take(&mut state.raw),
            raw_lost: std::mem::take(&mut state.raw_lost),
            exit: state.exit,
            output_bytes: state.output_bytes,
        }
    }

    /// Whether anything unread waits, or the process ended.
    pub fn ready(&self) -> bool {
        let state = self.shared.state();
        state.exit.is_some()
            || !state.raw.is_empty()
            || state.cleaner.has_unread()
    }

    /// Gives back what a call read but could not report, so the next one
    /// does.
    pub fn unread(&self, read: Read) {
        let mut state = self.shared.state();
        let mut raw = read.raw;
        raw.append(&mut state.raw);
        state.raw = raw;
        state.cleaner.give_back(read.text.text);
    }
}

/// A run's processes. Clones share them.
#[derive(Clone, Default)]
pub struct Processes(Arc<Inner>);

#[derive(Default)]
struct Inner {
    processes: Mutex<HashMap<i32, Arc<Process>>>,
    next: AtomicI32,
}

impl std::fmt::Debug for Processes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Processes").finish_non_exhaustive()
    }
}

impl Processes {
    pub fn new() -> Self {
        Self::default()
    }

    /// The process `id`, if the store has it.
    pub fn get(&self, id: i32) -> Option<Arc<Process>> {
        self.0
            .processes
            .lock()
            .expect("not poisoned")
            .get(&id)
            .cloned()
    }

    /// How many it holds.
    pub fn len(&self) -> usize {
        self.0.processes.lock().expect("not poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Drops `id`, and kills it if it still runs.
    pub fn remove(&self, id: i32) {
        self.0.processes.lock().expect("not poisoned").remove(&id);
    }

    /// Kills every process and forgets them.
    pub fn terminate_all(&self) {
        self.0.processes.lock().expect("not poisoned").clear();
    }

    /// Starts `command` and keeps it under a new id.
    pub fn start(
        &self,
        command: Command,
        text: String,
    ) -> Result<Arc<Process>, tau_terminal::Error> {
        let size = Size::TOOL;
        let mut run = command.size(size).spawn()?;
        let shared = Arc::new(Shared {
            state: Mutex::default(),
            changed: watch::Sender::new(0),
        });
        let process = Arc::new(Process {
            id: self.0.next.fetch_add(1, Ordering::SeqCst) + 1,
            command: text,
            size,
            started: Instant::now(),
            shared: shared.clone(),
            killer: run.killer(),
            input: run.input(),
            last_used: Mutex::new(Instant::now()),
            turn: tokio::sync::Mutex::new(()),
        });
        tokio::spawn(async move {
            while let Some(event) = run.next().await {
                match event {
                    Event::Output(bytes) => {
                        let mut state = shared.state();
                        state.cleaner.feed(&bytes);
                        state.output_bytes += bytes.len() as u64;
                        state.raw.extend_from_slice(&bytes);
                        if state.raw.len() > MAX_RAW {
                            let cut = state.raw.len() - MAX_RAW;
                            state.raw.drain(..cut);
                            state.raw_lost = true;
                        }
                    }
                    Event::Exit(end) => {
                        let code = end
                            .ok()
                            .and_then(|finished| finished.status)
                            .map_or(1, super::exit_code_of);
                        shared.state().exit = Some(Exit { code });
                    }
                    Event::Text(_) | Event::Screen(_) => continue,
                }
                shared.touch();
            }
            // The stream ended without an exit: the process is gone.
            let mut state = shared.state();
            if state.exit.is_none() {
                state.exit = Some(Exit { code: 1 });
            }
            drop(state);
            shared.touch();
        });
        let pruned = {
            let mut processes =
                self.0.processes.lock().expect("not poisoned");
            let pruned = prune(&mut processes);
            processes.insert(process.id, process.clone());
            pruned
        };
        // Killed outside the lock.
        drop(pruned);
        Ok(process)
    }
}

/// Removes the process to make room for one more, when the store is
/// full: the least recently used that is not among the newest
/// [`PROTECTED`], one that exited first.
fn prune(
    processes: &mut HashMap<i32, Arc<Process>>,
) -> Option<Arc<Process>> {
    if processes.len() < MAX_PROCESSES {
        return None;
    }
    let mut by_use: Vec<(i32, Instant, bool)> = processes
        .values()
        .map(|p| {
            (
                p.id,
                *p.last_used.lock().expect("not poisoned"),
                p.has_exited(),
            )
        })
        .collect();
    by_use.sort_by_key(|(_, used, _)| *used);
    let candidates = &by_use[..by_use.len().saturating_sub(PROTECTED)];
    let pick = candidates
        .iter()
        .find(|(_, _, exited)| *exited)
        .or(candidates.first())
        .map(|(id, _, _)| *id)?;
    processes.remove(&pick)
}
