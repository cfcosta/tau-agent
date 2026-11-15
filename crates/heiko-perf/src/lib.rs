//! Where heiko's time goes.
//!
//! - **Timings.** [`record`] adds one measurement to a row of a table
//!   kept for the whole process: a `group` (what kind of work) and a
//!   `name` (whose). [`span`] measures until it is dropped. A row keeps
//!   how often, how long in all, the longest, and how many fell in each
//!   of [`BOUNDS`].
//! - **Stalls.** The thread that must always answer, the interface's,
//!   calls [`beat`] every few milliseconds. [`watch`] starts a thread
//!   that sees when a beat is late by more than a frame or two: a stall,
//!   kept with what the thread said it was [`doing`].
//! - **The report.** [`report`] is the table and the stalls as they
//!   stand; [`watch`] writes it to a file every two seconds, so a run
//!   that is killed leaves its numbers.
//!
//! Recording is always on, and costs a lock and a map lookup. Only the
//! beat and the watching thread wait for [`watch`].

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Mutex,
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use serde::Serialize;

/// The upper bounds of a row's buckets, in milliseconds; a last bucket
/// holds what is over them all. 16 ms is a frame at 60 Hz, 100 ms what a
/// person notices, 1 s what they wait for.
pub const BOUNDS: [u64; 7] = [1, 4, 16, 50, 100, 500, 1000];

/// A beat this late is a stall: two frames at 60 Hz.
pub const STALL: Duration = Duration::from_millis(32);

/// How often the watched thread should beat.
pub const BEAT_EVERY: Duration = Duration::from_millis(8);

/// How many of the longest stalls a report keeps.
const KEPT_STALLS: usize = 20;

/// One row: the measurements of one name in one group.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Row {
    pub count: u64,
    pub total_ms: f64,
    pub max_ms: f64,
    /// How many fell under each of [`BOUNDS`], then over them all.
    pub buckets: [u64; BOUNDS.len() + 1],
}

impl Row {
    fn add(&mut self, took: Duration) {
        let ms = took.as_secs_f64() * 1000.;
        self.count += 1;
        self.total_ms += ms;
        self.max_ms = self.max_ms.max(ms);
        let at = BOUNDS
            .iter()
            .position(|bound| ms < *bound as f64)
            .unwrap_or(BOUNDS.len());
        self.buckets[at] += 1;
    }

    /// How many took `ms` milliseconds or longer; `ms` is one of
    /// [`BOUNDS`].
    pub fn over(&self, ms: u64) -> u64 {
        let from = BOUNDS
            .iter()
            .position(|bound| *bound == ms)
            .map_or(BOUNDS.len(), |at| at + 1);
        self.buckets[from..].iter().sum()
    }
}

/// A time the watched thread did not beat.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Stall {
    /// Seconds since the process started measuring.
    pub at_s: f64,
    pub ms: f64,
    /// What the thread said it was doing while it did not answer, in the
    /// order seen; empty when it said nothing.
    pub doing: Vec<&'static str>,
}

/// The table and the stalls, as they stand.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Report {
    /// Rows by group, then by name.
    pub groups: BTreeMap<String, BTreeMap<String, Row>>,
    pub stalls: u64,
    pub stalled_ms: f64,
    /// The longest stalls, longest first.
    pub worst: Vec<Stall>,
}

#[derive(Default)]
struct Kept {
    groups: BTreeMap<&'static str, BTreeMap<String, Row>>,
    stalls: u64,
    stalled_ms: f64,
    worst: Vec<Stall>,
}

fn kept() -> &'static Mutex<Kept> {
    static KEPT: OnceLock<Mutex<Kept>> = OnceLock::new();
    KEPT.get_or_init(Mutex::default)
}

fn started() -> Instant {
    static STARTED: OnceLock<Instant> = OnceLock::new();
    *STARTED.get_or_init(Instant::now)
}

/// Adds `took` to the row of `name` in `group`.
pub fn record(group: &'static str, name: &str, took: Duration) {
    let mut kept = kept().lock().expect("not poisoned");
    let rows = kept.groups.entry(group).or_default();
    match rows.get_mut(name) {
        Some(row) => row.add(took),
        None => rows.entry(name.to_owned()).or_default().add(took),
    }
}

/// Measures from now until it is dropped, into the row of `name` in
/// `group`; the watched thread is [`doing`] `name` meanwhile.
pub fn span(group: &'static str, name: &'static str) -> Span {
    Span {
        group,
        name,
        was: doing(name),
        started: Instant::now(),
    }
}

/// See [`span`].
#[derive(Debug)]
pub struct Span {
    group: &'static str,
    name: &'static str,
    was: &'static str,
    started: Instant,
}

impl Drop for Span {
    fn drop(&mut self) {
        record(self.group, self.name, self.started.elapsed());
        doing(self.was);
    }
}

fn label() -> &'static Mutex<&'static str> {
    static DOING: Mutex<&'static str> = Mutex::new("");
    &DOING
}

/// Says what the watched thread does from now on, and gives what it
/// said before. Only that thread calls it.
pub fn doing(what: &'static str) -> &'static str {
    std::mem::replace(&mut *label().lock().expect("not poisoned"), what)
}

/// Nanoseconds since [`started`] at the last beat; zero before the first.
static BEAT: AtomicU64 = AtomicU64::new(0);

/// The watched thread is alive. It calls this every [`BEAT_EVERY`]; how
/// late each beat came is the row `beat lag` of the group `thread`.
pub fn beat() {
    let now = started().elapsed().as_nanos() as u64;
    let last = BEAT.swap(now.max(1), Ordering::Relaxed);
    if last != 0 {
        let lag = Duration::from_nanos(now.saturating_sub(last))
            .saturating_sub(BEAT_EVERY);
        record("thread", "beat lag", lag);
    }
}

/// Starts the thread that watches the beats and writes the report to
/// `path` every two seconds. Call it once, before the first [`beat`].
pub fn watch(path: PathBuf) {
    started();
    let spawned = std::thread::Builder::new()
        .name("heiko-perf".into())
        .spawn(move || {
            let mut stall: Option<(Instant, Vec<&'static str>)> = None;
            let mut seen = 0;
            let mut written = Instant::now();
            loop {
                std::thread::sleep(Duration::from_millis(4));
                let last = BEAT.load(Ordering::Relaxed);
                let now = started().elapsed().as_nanos() as u64;
                let late = Duration::from_nanos(now.saturating_sub(last));
                if last != 0 && last == seen && late > STALL {
                    let what = *label().lock().expect("not poisoned");
                    let (_, doing) = stall.get_or_insert_with(|| {
                        (Instant::now() - late, Vec::new())
                    });
                    if !what.is_empty() && doing.last() != Some(&what) {
                        doing.push(what);
                    }
                } else if let Some((since, doing)) = stall.take() {
                    stalled(since, doing);
                }
                seen = last;
                if written.elapsed() >= Duration::from_secs(2) {
                    written = Instant::now();
                    let _ = write(&path);
                }
            }
        });
    if let Err(error) = spawned {
        eprintln!("heiko-perf: no thread to watch with: {error}");
    }
}

/// Keeps a stall that began at `since` and has just ended.
fn stalled(since: Instant, doing: Vec<&'static str>) {
    let took = since.elapsed();
    let mut kept = kept().lock().expect("not poisoned");
    kept.stalls += 1;
    kept.stalled_ms += took.as_secs_f64() * 1000.;
    kept.worst.push(Stall {
        at_s: since.saturating_duration_since(started()).as_secs_f64(),
        ms: took.as_secs_f64() * 1000.,
        doing,
    });
    kept.worst.sort_by(|a, b| b.ms.total_cmp(&a.ms));
    kept.worst.truncate(KEPT_STALLS);
}

/// The table and the stalls, as they stand.
pub fn report() -> Report {
    let kept = kept().lock().expect("not poisoned");
    Report {
        groups: kept
            .groups
            .iter()
            .map(|(group, rows)| ((*group).to_owned(), rows.clone()))
            .collect(),
        stalls: kept.stalls,
        stalled_ms: kept.stalled_ms,
        worst: kept.worst.clone(),
    }
}

impl Report {
    /// The report as a table to read: each group's rows, the one that
    /// took longest in all first, then the stalls.
    pub fn table(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        for (group, rows) in &self.groups {
            let _ = writeln!(
                out,
                "{group:<44} {:>8} {:>10} {:>8} {:>8} {:>7} {:>7}",
                "count", "total ms", "mean ms", "max ms", ">=16ms", ">=100ms"
            );
            let mut rows: Vec<_> = rows.iter().collect();
            rows.sort_by(|a, b| b.1.total_ms.total_cmp(&a.1.total_ms));
            for (name, row) in rows {
                let _ = writeln!(
                    out,
                    "  {name:<42} {:>8} {:>10.1} {:>8.2} {:>8.1} {:>7} {:>7}",
                    row.count,
                    row.total_ms,
                    row.total_ms / row.count.max(1) as f64,
                    row.max_ms,
                    row.over(16),
                    row.over(100),
                );
            }
            out.push('\n');
        }
        let _ = writeln!(
            out,
            "stalls: {} ({:.0} ms in all)",
            self.stalls, self.stalled_ms
        );
        for stall in &self.worst {
            let _ = writeln!(
                out,
                "  {:>8.1} ms at {:>7.1} s  {}",
                stall.ms,
                stall.at_s,
                stall.doing.join(", ")
            );
        }
        out
    }
}

/// Writes [`report`] to `path` as JSON and beside it, as `.txt`, as a
/// table; each whole, to a file beside it first, then over it.
pub fn write(path: &std::path::Path) -> std::io::Result<()> {
    let report = report();
    let whole = |path: &std::path::Path, text: String| {
        let beside = path.with_extension("tmp");
        std::fs::write(&beside, text)?;
        std::fs::rename(beside, path)
    };
    whole(path, serde_json::to_string_pretty(&report)?)?;
    whole(&path.with_extension("txt"), report.table())
}

/// The file the report goes to, when the person asked for one: what
/// `HEIKO_PERF` names.
pub fn asked() -> Option<PathBuf> {
    std::env::var_os("HEIKO_PERF")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}
