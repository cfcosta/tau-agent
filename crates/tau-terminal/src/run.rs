//! [`Command`]: run a program under a pseudo-terminal and stream what it
//! writes.
//!
//! The program gets a real terminal of a fixed size on stdout and
//! stderr, so it prints colors and progress bars, and stdin on
//! `/dev/null`, so nothing waits for input. It leads its own session
//! and process group with the terminal as its controlling terminal.
//!
//! Three pieces run a command:
//!
//! - a task reads the terminal's master side and waits for the process;
//! - a thread owns a recording [`Terminal`] (it is not `Send`), feeds it
//!   every chunk, and sends the [`Event`]s;
//! - the caller reads the events from [`Run::next`], and kills the
//!   process group with a [`Killer`] on a timeout or a cancel.

use std::{
    ffi::OsString,
    io,
    os::fd::{BorrowedFd, OwnedFd},
    path::PathBuf,
    process::{ExitStatus, Stdio},
    sync::mpsc as std_mpsc,
    time::{Duration, Instant},
};

use rustix::{
    fs::{Mode, OFlags},
    process::{Pid, Signal},
    pty::OpenptFlags,
    termios::Winsize,
};
use tokio::{io::unix::AsyncFd, sync::mpsc};

use crate::{
    error::Error,
    terminal::{Options, Size, Terminal},
};

/// How long the terminal must be idle, after the process has exited,
/// before the run ends: a grandchild still holding the terminal would
/// otherwise lose what it writes last.
pub const DEFAULT_IDLE: Duration = Duration::from_millis(100);

/// How often, at most, [`Event::Screen`] is sent while output arrives.
pub const DEFAULT_PREVIEW_INTERVAL: Duration = Duration::from_millis(100);

/// How much raw output [`Replay::Stream`] keeps. Past it, the replay is
/// a [`Replay::Snapshot`] of the end state instead.
pub const DEFAULT_REPLAY_CAP: usize = 4 << 20;

/// The environment every run gets, before [`Command::env`]: a terminal
/// type every system knows, and pagers that never wait for a key.
const ENV: [(&str, &str); 5] = [
    ("TERM", "xterm-256color"),
    ("PAGER", "cat"),
    ("GIT_PAGER", "cat"),
    ("MANPAGER", "cat"),
    ("LESS", "-FRX"),
];

/// How many bytes one read of the terminal takes, at most.
const READ_BUFFER: usize = 8192;

/// What a [`Run`] reports, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Bytes the program wrote, exactly, escape sequences included. The
    /// terminal turns each `\n` into `\r\n`. Every byte is sent once,
    /// in order.
    Output(Vec<u8>),
    /// Plain text of rows that scrolled off the screen since the last
    /// `Text`: final, since a program cannot change them any more. All
    /// the `Text` in order, then [`Finished::text`], is the whole
    /// output as plain text ([`Terminal::text`]).
    Text(String),
    /// The plain text of the rows still on screen (and any scrolled
    /// ones not yet sent as `Text`), to show progress. Sent after output
    /// arrives, at most once per preview interval, and it may still
    /// change.
    Screen(String),
    /// The run ended; always the last event.
    Exit(Result<Finished, String>),
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finished {
    /// The process's status; `None` if it could not be waited for.
    pub status: Option<ExitStatus>,
    /// The plain text not already sent as [`Event::Text`].
    pub text: String,
    /// What rebuilds the final screen when written to a new terminal of
    /// `size`.
    pub replay: Replay,
    /// Every byte the program wrote, counted.
    pub output_bytes: u64,
    pub size: Size,
}

/// What rebuilds a run's terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Replay {
    /// All the raw output, when it fit the replay cap.
    Stream(Vec<u8>),
    /// The final state as VT sequences ([`Terminal::vt`]): the screen
    /// and the scrollback the terminal kept, when the raw output was
    /// larger than the cap.
    Snapshot(Vec<u8>),
}

impl Replay {
    pub fn bytes(&self) -> &[u8] {
        match self {
            Replay::Stream(bytes) | Replay::Snapshot(bytes) => bytes,
        }
    }
}

/// Kills a run's process group. Cloneable and `Send`, so a timeout or a
/// cancel can use it from anywhere.
#[derive(Debug, Clone)]
pub struct Killer {
    pgid: Pid,
}

impl Killer {
    /// Sends `SIGKILL` to the whole process group. The run still ends
    /// through [`Event::Exit`], after the last output is read.
    pub fn kill(&self) {
        let _ = rustix::process::kill_process_group(self.pgid, Signal::KILL);
    }
}

/// A program to run under a pseudo-terminal. Needs a tokio runtime with
/// I/O enabled.
#[derive(Debug, Clone)]
pub struct Command {
    program: OsString,
    args: Vec<OsString>,
    cwd: Option<PathBuf>,
    env: Vec<(OsString, OsString)>,
    size: Size,
    idle: Duration,
    preview_interval: Duration,
    replay_cap: usize,
}

impl Command {
    pub fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
            size: Size::TOOL,
            idle: DEFAULT_IDLE,
            preview_interval: DEFAULT_PREVIEW_INTERVAL,
            replay_cap: DEFAULT_REPLAY_CAP,
        }
    }

    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn current_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }

    /// Sets a variable, over the defaults (`TERM=xterm-256color`, and
    /// `cat` as every pager).
    pub fn env(
        mut self,
        key: impl Into<OsString>,
        value: impl Into<OsString>,
    ) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// The terminal's size; [`Size::TOOL`] by default.
    pub fn size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }

    /// How long output must stop, after the process exits, before the
    /// run ends; [`DEFAULT_IDLE`] by default.
    pub fn idle(mut self, idle: Duration) -> Self {
        self.idle = idle;
        self
    }

    /// The least time between two [`Event::Screen`]s.
    pub fn preview_interval(mut self, interval: Duration) -> Self {
        self.preview_interval = interval;
        self
    }

    /// How much raw output to keep for [`Replay::Stream`].
    pub fn replay_cap(mut self, cap: usize) -> Self {
        self.replay_cap = cap;
        self
    }

    /// Starts the program.
    pub fn spawn(self) -> Result<Run, Error> {
        let master = rustix::pty::openpt(
            OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC,
        )
        .map_err(io::Error::from)?;
        rustix::pty::grantpt(&master).map_err(io::Error::from)?;
        rustix::pty::unlockpt(&master).map_err(io::Error::from)?;
        let name = rustix::pty::ptsname(&master, Vec::new())
            .map_err(io::Error::from)?;
        let slave: OwnedFd = rustix::fs::open(
            name.as_c_str(),
            OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io::Error::from)?;
        rustix::termios::tcsetwinsize(
            &slave,
            Winsize {
                ws_row: self.size.rows,
                ws_col: self.size.cols,
                ws_xpixel: 0,
                ws_ypixel: 0,
            },
        )
        .map_err(io::Error::from)?;

        let mut command = tokio::process::Command::new(&self.program);
        command.args(&self.args);
        if let Some(cwd) = &self.cwd {
            command.current_dir(cwd);
        }
        for (key, value) in ENV {
            command.env(key, value);
        }
        for (key, value) in &self.env {
            command.env(key, value);
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave));
        // SAFETY: runs in the child between fork and exec, and calls
        // only `setsid`, `sigaction` and `ioctl`, which are
        // async-signal-safe.
        unsafe {
            command.pre_exec(|| {
                // A session of its own makes the child its process
                // group's leader too, so killing the group by its pid
                // reaches every process it starts.
                rustix::process::setsid()?;
                // When the shell (the session's leader) exits, the kernel
                // hangs the terminal up for its process group; a
                // background job still printing would die of it.
                // Ignoring SIGHUP lets it finish, as it would with pipes.
                let mut ignore: libc::sigaction = std::mem::zeroed();
                ignore.sa_sigaction = libc::SIG_IGN;
                if libc::sigaction(libc::SIGHUP, &ignore, std::ptr::null_mut())
                    != 0
                {
                    return Err(io::Error::last_os_error());
                }
                // stdout (fd 1) is the terminal: make it the session's
                // controlling terminal, so `/dev/tty` works.
                let stdout = BorrowedFd::borrow_raw(1);
                rustix::process::ioctl_tiocsctty(stdout)?;
                Ok(())
            });
        }
        let child = command.spawn()?;
        // The parent's copies of the terminal's slave side go with the
        // command, so the master sees end of file once the process and
        // its children close theirs.
        drop(command);
        let pid = child.id().ok_or_else(|| {
            io::Error::other("the spawned process has no pid")
        })?;
        let pgid = Pid::from_raw(pid as i32)
            .ok_or_else(|| io::Error::other("the spawned process has pid 0"))?;

        // A program that reads `/dev/tty` (a password prompt) gets end
        // of file at once instead of waiting for input nobody sends: an
        // EOF character in the terminal's (canonical) input queue.
        let _ = rustix::io::write(&master, b"\x04");

        rustix::io::ioctl_fionbio(&master, true).map_err(io::Error::from)?;
        let master = AsyncFd::new(master)?;

        let (to_terminal, from_reader) = std_mpsc::channel();
        let (events, receiver) = mpsc::unbounded_channel();
        let options = Options {
            size: self.size,
            record: true,
            ..Options::default()
        };
        let preview_interval = self.preview_interval;
        let replay_cap = self.replay_cap;
        std::thread::Builder::new()
            .name("tau-terminal".into())
            .spawn(move || {
                feed(options, preview_interval, replay_cap, from_reader, events)
            })?;
        tokio::spawn(read(master, child, self.idle, to_terminal));

        Ok(Run {
            events: receiver,
            killer: Killer { pgid },
            pid,
        })
    }
}

/// A running command.
#[derive(Debug)]
pub struct Run {
    events: mpsc::UnboundedReceiver<Event>,
    killer: Killer,
    pid: u32,
}

impl Run {
    /// The process's id, which is also its process group's and its
    /// session's.
    pub fn id(&self) -> u32 {
        self.pid
    }

    pub fn killer(&self) -> Killer {
        self.killer.clone()
    }

    /// The next event; `None` after [`Event::Exit`].
    pub async fn next(&mut self) -> Option<Event> {
        self.events.recv().await
    }
}

/// What the reader hands the terminal thread.
enum Feed {
    Output(Vec<u8>),
    Exited(Option<ExitStatus>),
}

/// Reads the terminal until end of file after the process exits, or
/// until it has been idle for `idle` once the process has exited, then
/// hands the status over. Closing the master when it returns hangs up
/// the session: what is left of it gets `SIGHUP`.
async fn read(
    master: AsyncFd<OwnedFd>,
    mut child: tokio::process::Child,
    idle: Duration,
    to_terminal: std_mpsc::Sender<Feed>,
) {
    let mut buffer = vec![0u8; READ_BUFFER];
    let mut status = None;
    let mut exited = false;
    let mut eof = false;
    let sleep = tokio::time::sleep(idle);
    tokio::pin!(sleep);

    let read_now = |buffer: &mut [u8]| -> io::Result<usize> {
        rustix::io::read(master.get_ref(), buffer).map_err(io::Error::from)
    };

    loop {
        if eof && exited {
            break;
        }
        tokio::select! {
            biased;
            ready = master.readable(), if !eof => {
                let Ok(mut guard) = ready else {
                    eof = true;
                    continue;
                };
                match guard.try_io(|fd| {
                    rustix::io::read(fd.get_ref(), &mut buffer).map_err(io::Error::from)
                }) {
                    // Linux reports a hung-up terminal as EIO.
                    Ok(Ok(0) | Err(_)) => eof = true,
                    Ok(Ok(n)) => {
                        let _ = to_terminal.send(Feed::Output(buffer[..n].to_vec()));
                        if exited {
                            sleep.as_mut().reset(tokio::time::Instant::now() + idle);
                        }
                    }
                    Err(_would_block) => {}
                }
            }
            waited = child.wait(), if !exited => {
                status = waited.ok();
                exited = true;
                sleep.as_mut().reset(tokio::time::Instant::now() + idle);
            }
            () = &mut sleep, if exited && !eof => {
                // Whatever is already buffered still counts.
                loop {
                    match read_now(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let _ = to_terminal.send(Feed::Output(buffer[..n].to_vec()));
                        }
                    }
                }
                break;
            }
        }
    }
    let _ = to_terminal.send(Feed::Exited(status));
}

/// The terminal thread: feeds every chunk to a recording [`Terminal`]
/// and sends the events, in order.
fn feed(
    options: Options,
    preview_interval: Duration,
    replay_cap: usize,
    from_reader: std_mpsc::Receiver<Feed>,
    events: mpsc::UnboundedSender<Event>,
) {
    let mut terminal = match Terminal::new(options) {
        Ok(terminal) => terminal,
        Err(error) => {
            // Output is still streamed; only the text is missing.
            while let Ok(feed) = from_reader.recv() {
                match feed {
                    Feed::Output(bytes) => {
                        let _ = events.send(Event::Output(bytes));
                    }
                    Feed::Exited(_) => break,
                }
            }
            let _ = events.send(Event::Exit(Err(error.to_string())));
            return;
        }
    };
    let mut raw = Vec::new();
    let mut raw_complete = true;
    let mut output_bytes = 0u64;
    let mut last_preview: Option<Instant> = None;
    let mut preview_due = false;
    let mut failure: Option<String> = None;

    loop {
        let next = if preview_due {
            let wait = last_preview
                .map(|at| preview_interval.saturating_sub(at.elapsed()))
                .unwrap_or_default();
            match from_reader.recv_timeout(wait) {
                Ok(feed) => Some(feed),
                Err(std_mpsc::RecvTimeoutError::Timeout) => None,
                Err(std_mpsc::RecvTimeoutError::Disconnected) => {
                    Some(Feed::Exited(None))
                }
            }
        } else {
            Some(from_reader.recv().unwrap_or(Feed::Exited(None)))
        };

        match next {
            Some(Feed::Output(bytes)) => {
                output_bytes += bytes.len() as u64;
                if raw_complete {
                    if raw.len() + bytes.len() <= replay_cap {
                        raw.extend_from_slice(&bytes);
                    } else {
                        raw_complete = false;
                        raw = Vec::new();
                    }
                }
                if failure.is_none()
                    && let Err(error) = terminal.write(&bytes)
                {
                    failure = Some(error.to_string());
                }
                let _ = events.send(Event::Output(bytes));
                let recorded = terminal.take_recorded();
                if !recorded.is_empty() {
                    let _ = events.send(Event::Text(recorded));
                }
                preview_due = true;
            }
            Some(Feed::Exited(status)) => {
                let finished = match failure {
                    Some(error) => Err(error),
                    None => finish(
                        &terminal,
                        status,
                        raw,
                        raw_complete,
                        output_bytes,
                    ),
                };
                let _ = events.send(Event::Exit(finished));
                return;
            }
            None => {}
        }

        if preview_due
            && last_preview.is_none_or(|at| at.elapsed() >= preview_interval)
        {
            if let Ok(text) = terminal.text() {
                let _ = events.send(Event::Screen(text));
            }
            last_preview = Some(Instant::now());
            preview_due = false;
        }
    }
}

fn finish(
    terminal: &Terminal,
    status: Option<ExitStatus>,
    raw: Vec<u8>,
    raw_complete: bool,
    output_bytes: u64,
) -> Result<Finished, String> {
    let text = terminal.text().map_err(|error| error.to_string())?;
    let replay = if raw_complete {
        Replay::Stream(raw)
    } else {
        Replay::Snapshot(terminal.vt().map_err(|error| error.to_string())?)
    };
    Ok(Finished {
        status,
        text,
        replay,
        output_bytes,
        size: terminal.size(),
    })
}
