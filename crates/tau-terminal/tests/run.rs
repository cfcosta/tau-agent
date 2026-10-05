//! `tau_terminal::Command`: programs under a pseudo-terminal
//! (`docs/decisions/0010-terminal-rendering.md`). Real processes, on a
//! normal, unpaused runtime.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]
#![cfg(unix)]

use std::{
    path::Path,
    time::{Duration, Instant},
};

use tau_terminal::{Command, Event, Finished, Replay, Size};

/// What a run produced: every event's payload, in order.
#[derive(Debug, Default)]
struct Collected {
    output: Vec<u8>,
    outputs: usize,
    text: String,
    screens: Vec<String>,
    finished: Option<Finished>,
}

impl Collected {
    /// All the plain text: the recorded rows, then the rest.
    fn all_text(&self) -> String {
        let finished = self.finished.as_ref().expect("the run finished");
        format!("{}{}", self.text, finished.text)
    }

    fn code(&self) -> Option<i32> {
        self.finished.as_ref()?.status?.code()
    }
}

fn sh(script: &str) -> Command {
    Command::new("/bin/sh").arg("-c").arg(script)
}

async fn collect(command: Command) -> Collected {
    let mut run = command.spawn().unwrap();
    let mut collected = Collected::default();
    while let Some(event) = run.next().await {
        match event {
            Event::Output(bytes) => {
                collected.outputs += 1;
                collected.output.extend_from_slice(&bytes);
            }
            Event::Text(text) => collected.text.push_str(&text),
            Event::Screen(text) => collected.screens.push(text),
            Event::Exit(finished) => {
                collected.finished = Some(finished.unwrap());
            }
        }
    }
    collected
}

fn process_alive(pid: i32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

async fn wait_until_dead(pid: i32, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while process_alive(pid) {
        assert!(Instant::now() < deadline, "pid {pid} still alive");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// stdout and stderr are the terminal; stdin is not.
#[tokio::test(flavor = "multi_thread")]
async fn stdout_and_stderr_are_a_terminal_and_stdin_is_not() {
    let run = collect(sh(
        "[ -t 1 ] && echo out-tty; [ -t 2 ] && echo err-tty >&2; [ -t 0 ] || echo in-not-tty",
    ))
    .await;
    assert_eq!(run.all_text(), "out-tty\nerr-tty\nin-not-tty\n");
    assert_eq!(run.code(), Some(0));
}

/// The terminal is 120 by 40, `TERM` is `xterm-256color`, and pagers
/// are `cat`.
#[tokio::test(flavor = "multi_thread")]
async fn the_terminal_has_a_fixed_size_and_environment() {
    let run = collect(sh(
        "stty size < /dev/tty; echo \"$TERM $PAGER $GIT_PAGER $MANPAGER $LESS\"",
    ))
    .await;
    assert_eq!(run.all_text(), "40 120\nxterm-256color cat cat cat -FRX\n");
    let finished = run.finished.unwrap();
    assert_eq!(finished.size, Size::TOOL);
}

/// A launcher runs first, with the program and its arguments after its
/// own words.
#[tokio::test(flavor = "multi_thread")]
async fn a_launcher_runs_the_program_after_its_words() {
    let run = collect(
        Command::new("/bin/sh")
            .arg("-c")
            .arg("echo \"$0 $LAUNCHED\"")
            .arg("zero")
            .through(["/usr/bin/env", "LAUNCHED=yes"]),
    )
    .await;
    assert_eq!(run.all_text(), "zero yes\n");
    assert_eq!(run.code(), Some(0));
}

/// The terminal is the process's controlling terminal, so `/dev/tty`
/// opens.
#[tokio::test(flavor = "multi_thread")]
async fn the_terminal_is_the_controlling_terminal() {
    let run = collect(sh(": > /dev/tty && echo has-tty")).await;
    assert_eq!(run.all_text(), "has-tty\n");
}

/// Colors a program prints because it sees a terminal reach the raw
/// bytes, and not the text.
#[tokio::test(flavor = "multi_thread")]
async fn colors_reach_the_bytes_and_not_the_text() {
    let run = collect(sh(
        "if [ -t 1 ]; then printf '\\033[31mred\\033[0m\\n'; else echo plain; fi",
    ))
    .await;
    assert!(
        run.output.windows(5).any(|w| w == b"\x1b[31m"),
        "{:?}",
        String::from_utf8_lossy(&run.output)
    );
    assert_eq!(run.all_text(), "red\n");
}

/// A progress bar redrawn with `\r` leaves only its last frame in the
/// text, while the bytes keep every frame; the terminal's `\r\n` line
/// ends come back as `\n`.
#[tokio::test(flavor = "multi_thread")]
async fn a_progress_bar_leaves_its_last_frame() {
    let run = collect(sh("printf '10%%\\r50%%\\r100%%\\n'; echo done")).await;
    assert_eq!(run.all_text(), "100%\ndone\n");
    assert_eq!(run.output, b"10%\r50%\r100%\r\ndone\r\n");
}

/// A program reading stdin sees end of file at once, and so does one
/// reading `/dev/tty`: neither waits for input nobody sends.
#[tokio::test(flavor = "multi_thread")]
async fn a_prompt_does_not_wait() {
    let started = Instant::now();
    let run = collect(sh(
        "read a; echo \"stdin $?\"; read b < /dev/tty; echo \"tty $?\"",
    ))
    .await;
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(run.all_text(), "stdin 1\ntty 1\n");
}

/// Every byte arrives once, in order, and the recorded text plus the
/// rest is the whole output, however far it scrolled.
#[tokio::test(flavor = "multi_thread")]
async fn every_byte_arrives_in_order_and_the_text_is_whole() {
    let run = collect(sh("seq 1 30000")).await;
    let expected: String = (1..=30_000).map(|i| format!("{i}\n")).collect();
    assert_eq!(run.output, expected.replace('\n', "\r\n").into_bytes());
    assert_eq!(run.all_text(), expected);
    assert!(!run.text.is_empty(), "scrolled rows are sent as they go");
    let finished = run.finished.unwrap();
    assert_eq!(finished.output_bytes, run.output.len() as u64);
    assert_eq!(finished.replay, Replay::Stream(run.output.clone()));
}

/// Past the replay cap, the replay is the final screen as VT sequences.
#[tokio::test(flavor = "multi_thread")]
async fn past_the_cap_the_replay_is_a_snapshot() {
    let run = collect(sh("seq 1 3000").replay_cap(1000)).await;
    let finished = run.finished.as_ref().unwrap();
    let Replay::Snapshot(vt) = &finished.replay else {
        panic!("expected a snapshot, got {:?}", finished.replay);
    };
    let mut terminal =
        tau_terminal::Terminal::new(tau_terminal::Options::default()).unwrap();
    terminal.write(vt).unwrap();
    let text = terminal.text().unwrap();
    assert!(
        text.ends_with("2999\n3000\n"),
        "{:?}",
        &text[text.len().saturating_sub(40)..]
    );
}

/// The exit status is reported, and a signal shows as one.
#[tokio::test(flavor = "multi_thread")]
async fn the_exit_status_is_reported() {
    assert_eq!(collect(sh("echo x; exit 3")).await.code(), Some(3));
    let killed = collect(sh("kill -KILL $$")).await;
    use std::os::unix::process::ExitStatusExt;
    let status = killed.finished.unwrap().status.unwrap();
    assert_eq!(status.signal(), Some(9));
}

/// Killing the run kills its whole process group, grandchildren
/// included, and the run still ends with what was printed before.
#[tokio::test(flavor = "multi_thread")]
async fn killing_the_run_kills_the_group() {
    let dir = tempfile::tempdir().unwrap();
    let pidfile = dir.path().join("pid");
    let mut run = sh(&format!(
        "echo before; sleep 20 & echo $! > {}; wait",
        pidfile.display()
    ))
    .spawn()
    .unwrap();
    let killer = run.killer();
    let started = Instant::now();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        killer.kill();
    });
    let mut text = String::new();
    let mut finished = None;
    while let Some(event) = run.next().await {
        match event {
            Event::Text(t) => text.push_str(&t),
            Event::Exit(f) => finished = Some(f.unwrap()),
            _ => {}
        }
    }
    assert!(started.elapsed() < Duration::from_secs(5));
    let finished = finished.unwrap();
    text.push_str(&finished.text);
    assert_eq!(text, "before\n");
    let pid: i32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    wait_until_dead(pid, Duration::from_secs(2)).await;
}

/// Output a grandchild writes just after the process exits is still
/// read.
#[tokio::test(flavor = "multi_thread")]
async fn late_output_from_a_grandchild_is_read() {
    let run = collect(sh("(sleep 0.05; echo late) & echo early")).await;
    assert_eq!(run.all_text(), "early\nlate\n");
}

/// Screens arrive while a slow command runs, with what it printed.
#[tokio::test(flavor = "multi_thread")]
async fn screens_show_progress() {
    let run = collect(sh("echo one; sleep 0.3; echo two")).await;
    assert!(
        run.screens.iter().any(|screen| screen == "one\n"),
        "{:?}",
        run.screens
    );
    assert_eq!(run.all_text(), "one\ntwo\n");
}
