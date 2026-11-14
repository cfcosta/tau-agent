//! The tree `tau-plugin test` prints: a block per package, headed by how
//! it went, and under it a branch per test, with how many cases each
//! property ran and how long each test took. A package that passes
//! folds to its header unless asked `--verbose`. The failures are told
//! again in full at the end, then a summary of tests, cases and time.
//!
//! In a terminal the package going on is drawn in place as it runs: its
//! finished tests, and the one running, with a spinner and, for a
//! property, a bar of the cases it has run or the tries it has spent
//! shrinking a failure. Piped, nothing is redrawn and nothing coloured:
//! each block is printed once, when its package is done.

use std::{
    fmt::Write as _,
    io::{IsTerminal, Write as _},
    sync::{
        Arc,
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use tau_luau_plugins::{TestKind, TestResult};
use tau_luau_plugins_host::runtime::{Progress, Step};

/// How the tree is shown.
#[derive(Debug, Clone, Copy)]
pub struct Look {
    /// Whether it is coloured.
    pub color: bool,
    /// Whether the package going on is drawn in place as it runs.
    pub live: bool,
    /// Whether a package that passes shows its tests too.
    pub verbose: bool,
}

impl Look {
    /// What stdout can show: colour and drawing in place in a terminal
    /// that is not dumb, unless `NO_COLOR` is set; neither when piped.
    pub fn of_stdout(verbose: bool) -> Self {
        let terminal = std::io::stdout().is_terminal()
            && std::env::var("TERM").map_or(true, |term| term != "dumb");
        Self {
            color: terminal && std::env::var_os("NO_COLOR").is_none(),
            live: terminal,
            verbose,
        }
    }
}

/// The tree being printed, as the packages' tests run.
pub struct Tree {
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    ticker: Option<JoinHandle<()>>,
}

impl Tree {
    /// A tree with `look`, showing only the tests whose file or name
    /// holds `filter`, when one is given.
    pub fn new(look: Look, filter: Option<String>) -> Self {
        let state = Arc::new(Mutex::new(State {
            look,
            filter,
            began: Instant::now(),
            block: None,
            drawn: 0,
            frame: 0,
            failures: Vec::new(),
            passed: 0,
            failed: 0,
            cases: 0,
            shrinks: 0,
            properties: false,
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let ticker = look.live.then(|| {
            let (state, stop) = (state.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(TICK);
                    let mut state = state.lock().expect("not poisoned");
                    state.frame += 1;
                    state.redraw();
                }
            })
        });
        Self {
            state,
            stop,
            ticker,
        }
    }

    /// Begins the package `name`: where its tests' progress goes.
    pub fn begin(&self, name: &str) -> Progress {
        let mut state = self.state.lock().expect("not poisoned");
        state.block = Some(Block {
            name: name.to_owned(),
            began: Instant::now(),
            tests: Vec::new(),
            running: None,
        });
        drop(state);
        let state = self.state.clone();
        Progress::to(move |step| {
            state.lock().expect("not poisoned").step(step);
        })
    }

    /// Ends the package going on: what its tests came to, the ones the
    /// filter keeps, or why it did not load.
    pub fn end(&self, outcome: Result<&[TestResult], &str>) {
        self.state.lock().expect("not poisoned").end(outcome);
    }

    /// Tells every failure again, and the summary.
    pub fn finish(mut self) {
        self.stop_ticker();
        let state = self.state.lock().expect("not poisoned");
        let mut out = String::new();
        for failure in &state.failures {
            out.push('\n');
            out.push_str(&state.recap(failure));
        }
        out.push('\n');
        out.push_str(&state.summary());
        print!("{out}");
        let _ = std::io::stdout().flush();
    }

    fn stop_ticker(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(ticker) = self.ticker.take() {
            let _ = ticker.join();
        }
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        self.stop_ticker();
    }
}

/// How often the spinner turns.
const TICK: Duration = Duration::from_millis(80);
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
/// The widest a test's name is padded to, for its tail to line up.
const NAME_COLUMN: usize = 60;
/// How many cells a property's bar takes.
const BAR: usize = 10;

struct State {
    look: Look,
    filter: Option<String>,
    began: Instant,
    /// The package going on.
    block: Option<Block>,
    /// The lines drawn in place, which the next drawing replaces.
    drawn: usize,
    frame: usize,
    failures: Vec<Failure>,
    passed: usize,
    failed: usize,
    cases: u64,
    shrinks: u64,
    /// Whether any property or machine ran, for the summary to count
    /// cases.
    properties: bool,
}

/// A package's tests so far.
struct Block {
    name: String,
    began: Instant,
    tests: Vec<TestResult>,
    running: Option<Running>,
}

/// The test going on.
struct Running {
    file: String,
    name: String,
    kind: TestKind,
    /// The cases a property asks for.
    planned: Option<u64>,
    ran: u64,
    failed_at: Option<u64>,
    began: Instant,
}

/// What failed, to tell again at the end.
enum Failure {
    Test { package: String, test: TestResult },
    Load { package: String, why: String },
}

impl State {
    fn kept(&self, test: &TestResult) -> bool {
        self.filter.as_deref().is_none_or(|filter| {
            test.file.contains(filter) || test.name.contains(filter)
        })
    }

    fn step(&mut self, step: Step<'_>) {
        let kept = match step {
            Step::Ended(test) => self.kept(test),
            _ => true,
        };
        let Some(block) = &mut self.block else {
            return;
        };
        match step {
            Step::Began {
                file,
                name,
                kind,
                cases,
            } => {
                block.running = Some(Running {
                    file: file.to_owned(),
                    name: name.to_owned(),
                    kind,
                    planned: cases,
                    ran: 0,
                    failed_at: None,
                    began: Instant::now(),
                });
            }
            Step::Case { ran, failed_at } => {
                if let Some(running) = &mut block.running {
                    running.ran = ran;
                    running.failed_at = failed_at;
                }
            }
            Step::Ended(test) => {
                // A file's cases end one by one; it runs until its last.
                let file_going_on = block
                    .running
                    .as_ref()
                    .is_some_and(|running| running.kind == TestKind::Case);
                if !(file_going_on && test.kind == TestKind::Case) {
                    block.running = None;
                }
                if kept {
                    block.tests.push(test.clone());
                }
            }
        }
        // Drawn on the next tick: a property tells every case, far more
        // often than a terminal can show.
    }

    fn end(&mut self, outcome: Result<&[TestResult], &str>) {
        let Some(block) = self.block.take() else {
            return;
        };
        let took = block.began.elapsed();
        let lines = match outcome {
            Ok(tests) => {
                for test in tests {
                    if test.passed {
                        self.passed += 1;
                    } else {
                        self.failed += 1;
                        self.failures.push(Failure::Test {
                            package: block.name.clone(),
                            test: test.clone(),
                        });
                    }
                    if matches!(
                        test.kind,
                        TestKind::Property | TestKind::Machine
                    ) {
                        self.properties = true;
                        self.cases += test.cases.unwrap_or(0);
                        self.shrinks += test.shrinks.unwrap_or(0);
                    }
                }
                self.done_block(&block.name, tests, took)
            }
            Err(why) => {
                self.failures.push(Failure::Load {
                    package: block.name.clone(),
                    why: why.to_owned(),
                });
                vec![
                    Line::new()
                        .add(RED_BOLD, " ✗ ")
                        .add(BOLD, &block.name)
                        .add(RED, "  does not load"),
                ]
            }
        };
        let mut out = self.clear();
        for line in &lines {
            out.push_str(&line.paint(self.look.color, usize::MAX));
            out.push('\n');
        }
        print!("{out}");
        let _ = std::io::stdout().flush();
    }

    /// The lines of a package once done: its header, and its tests when
    /// one failed or every one is asked for.
    fn done_block(
        &self,
        name: &str,
        tests: &[TestResult],
        took: Duration,
    ) -> Vec<Line> {
        if tests.is_empty() {
            return vec![
                Line::new()
                    .add(DIM, " – ")
                    .add(BOLD, name)
                    .add(DIM, "  no tests"),
            ];
        }
        let passed = tests.iter().filter(|test| test.passed).count();
        let all = passed == tests.len();
        let mut lines = vec![
            Line::new()
                .add(
                    if all { GREEN_BOLD } else { RED_BOLD },
                    if all { " ✓ " } else { " ✗ " },
                )
                .add(BOLD, name)
                .add(DIM, &format!("  {passed}/{}  ", tests.len()))
                .add(DIM, &duration(took)),
        ];
        if all && !self.look.verbose {
            return lines;
        }
        let column =
            name_column(tests.iter().map(|test| label(&test.file, &test.name)));
        for (at, test) in tests.iter().enumerate() {
            lines.push(done_line(test, at + 1 == tests.len(), column));
        }
        lines
    }

    /// Draws the package going on in place of what was drawn before.
    fn redraw(&mut self) {
        if !self.look.live {
            return;
        }
        let Some(block) = &self.block else {
            return;
        };
        let (width, rows) = terminal_size();
        let spinner = SPINNER[self.frame % SPINNER.len()].to_string();
        let mut lines = vec![
            Line::new()
                .add(YELLOW, &format!(" {spinner} "))
                .add(BOLD, &block.name)
                .add(DIM, &format!("  {} done  ", block.tests.len()))
                .add(DIM, &duration(block.began.elapsed())),
        ];
        let names = block
            .tests
            .iter()
            .map(|test| label(&test.file, &test.name))
            .chain(
                block
                    .running
                    .iter()
                    .map(|running| label(&running.file, &running.name)),
            );
        let column = name_column(names).min(width.saturating_sub(34));
        let mut children: Vec<Line> = block
            .tests
            .iter()
            .enumerate()
            .map(|(at, test)| {
                let last =
                    block.running.is_none() && at + 1 == block.tests.len();
                done_line(test, last, column)
            })
            .collect();
        if let Some(running) = &block.running {
            children.push(running_line(running, &spinner, column));
        }
        // What does not fit the screen is cut from the top, or the
        // cursor could not climb back over it.
        let room = rows.saturating_sub(3).max(3);
        if children.len() > room {
            let cut = children.len() - (room - 1);
            children.drain(..cut);
            lines.push(Line::new().add(DIM, &format!("   ⋮  {cut} more")));
        }
        lines.extend(children);
        let mut out = self.clear();
        for line in &lines {
            out.push_str(&line.paint(self.look.color, width.saturating_sub(1)));
            out.push('\n');
        }
        self.drawn = lines.len();
        print!("{out}");
        let _ = std::io::stdout().flush();
    }

    /// What takes the cursor back over the lines drawn in place, and
    /// clears them.
    fn clear(&mut self) -> String {
        let drawn = std::mem::take(&mut self.drawn);
        if drawn == 0 {
            String::new()
        } else {
            format!("\x1b[{drawn}A\r\x1b[J")
        }
    }

    /// A failure in full: what failed, then why, a line at a time.
    fn recap(&self, failure: &Failure) -> String {
        let color = self.look.color;
        let mut out = String::new();
        let (title, why) = match failure {
            Failure::Test { package, test } => (
                format!("{package} › {}", label(&test.file, &test.name)),
                test.error.as_deref().unwrap_or("it failed"),
            ),
            Failure::Load { package, why } => {
                (format!("{package} does not load"), why.as_str())
            }
        };
        let head = Line::new()
            .add(RED_BOLD, " FAIL ")
            .add(BOLD, &format!(" {title}"));
        let _ = writeln!(out, "{}", head.paint(color, usize::MAX));
        if let Failure::Test { test, .. } = failure
            && let Some(counts) = counts(test)
        {
            let line = Line::new().add("", "      ").add(DIM, &counts);
            let _ = writeln!(out, "{}", line.paint(color, usize::MAX));
        }
        for (at, text) in why.lines().enumerate() {
            let style = if at == 0 {
                RED
            } else if text.starts_with("reproduce:") {
                BLUE
            } else {
                ""
            };
            let line = Line::new().add("", "      ").add(style, text);
            let _ = writeln!(out, "{}", line.paint(color, usize::MAX));
        }
        out
    }

    fn summary(&self) -> String {
        let color = self.look.color;
        let ran = self.passed + self.failed;
        let mut tests = Line::new().add(BOLD, " Tests  ");
        if self.passed > 0 || self.failed == 0 {
            tests = tests.add(GREEN_BOLD, &format!("{} passed", self.passed));
        }
        if self.failed > 0 {
            if self.passed > 0 {
                tests = tests.add(DIM, " · ");
            }
            tests = tests.add(RED_BOLD, &format!("{} failed", self.failed));
        }
        let mut lines = vec![
            Line::new().add(DIM, &format!(" {}", "─".repeat(44))),
            tests.add(DIM, &format!(" ({ran})")),
        ];
        if self.properties {
            let mut cases = Line::new()
                .add(BOLD, " Cases  ")
                .add("", &thousands(self.cases))
                .add(DIM, " generated");
            if self.shrinks > 0 {
                cases = cases.add(DIM, " · ").add(
                    MAGENTA,
                    &format!("{} shrinks", thousands(self.shrinks)),
                );
            }
            lines.push(cases);
        }
        lines.push(
            Line::new()
                .add(BOLD, " Time   ")
                .add("", &duration(self.began.elapsed())),
        );
        let mut out = String::new();
        for line in lines {
            let _ = writeln!(out, "{}", line.paint(color, usize::MAX));
        }
        out
    }
}

/// A finished test's line, under its package.
fn done_line(test: &TestResult, last: bool, column: usize) -> Line {
    let mut line = twig(last);
    line = if test.passed {
        line.add(GREEN, "✓ ")
    } else {
        line.add(RED, "✗ ")
    };
    let mut tail = Line::new();
    let ran = test.cases.unwrap_or(0);
    match test.kind {
        TestKind::Property | TestKind::Machine if test.passed => {
            let noun = if test.kind == TestKind::Machine {
                "runs"
            } else {
                "cases"
            };
            tail = tail.add(DIM, &format!(" {ran} {noun}"));
        }
        TestKind::Property | TestKind::Machine if test.cases.is_some() => {
            tail = tail.add(RED, &format!(" failed at case {ran}"));
            if let Some(shrinks) = test.shrinks.filter(|&shrinks| shrinks > 0) {
                tail = tail.add(DIM, &format!(", {shrinks} shrinks"));
            }
        }
        _ => {}
    }
    if let Some(millis) = test.millis {
        tail = tail.add(
            FAINT,
            &format!(" {}", duration(Duration::from_millis(millis))),
        );
    }
    // Only a tail is lined up; a name with none is not padded.
    let name = label(&test.file, &test.name);
    let name = if tail.0.is_empty() {
        name
    } else {
        pad(&name, column)
    };
    line.add(if test.passed { "" } else { RED }, &name)
        .then(tail)
}

/// The running test's line: a spinner, and how far a property has got.
fn running_line(running: &Running, spinner: &str, column: usize) -> Line {
    let line = twig(true)
        .add(YELLOW, &format!("{spinner} "))
        .add(BOLD, &pad(&label(&running.file, &running.name), column));
    match (running.kind, running.failed_at) {
        (TestKind::Property | TestKind::Machine, Some(at)) => line
            .add(RED, &format!(" failed at case {at}"))
            .add(YELLOW, &format!(", shrinking ({} tries)", running.ran - at)),
        (TestKind::Property | TestKind::Machine, None) => {
            let planned = running.planned.unwrap_or(0).max(1);
            let style = if running.kind == TestKind::Machine {
                CYAN
            } else {
                MAGENTA
            };
            #[allow(clippy::cast_precision_loss, reason = "a bar's fraction")]
            let fraction = running.ran as f64 / planned as f64;
            line.add("", " ")
                .add(style, &bar(fraction))
                .add(DIM, &format!(" {:>3}/{planned}", running.ran))
        }
        _ => {
            line.add(FAINT, &format!(" {}", duration(running.began.elapsed())))
        }
    }
}

fn twig(last: bool) -> Line {
    Line::new().add(FAINT, if last { "   └─ " } else { "   ├─ " })
}

/// What a test is called under its package: its file, and its name
/// unless that already names the file.
fn label(file: &str, name: &str) -> String {
    if file.is_empty() || name.starts_with("tests/") {
        name.to_owned()
    } else {
        format!("{file} › {name}")
    }
}

/// The column the tails of `names` line up at.
fn name_column(names: impl Iterator<Item = String>) -> usize {
    names
        .map(|name| name.chars().count())
        .max()
        .unwrap_or(0)
        .min(NAME_COLUMN)
}

fn pad(text: &str, width: usize) -> String {
    let len = text.chars().count();
    if len >= width {
        text.to_owned()
    } else {
        format!("{text}{}", " ".repeat(width - len))
    }
}

/// `fraction` of a bar of [`BAR`] cells, to an eighth of a cell.
fn bar(fraction: f64) -> String {
    const EIGHTHS: [char; 8] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a bar's eighths, clamped"
    )]
    let eighths = (fraction.clamp(0.0, 1.0) * (BAR * 8) as f64) as usize;
    let (full, part) = (eighths / 8, eighths % 8);
    let mut out = "█".repeat(full);
    if full < BAR {
        out.push(if part == 0 { '░' } else { EIGHTHS[part] });
        out.push_str(&"░".repeat(BAR - full - 1));
    }
    out
}

/// What a property's or machine's counts were, for its failure.
fn counts(test: &TestResult) -> Option<String> {
    let cases = test.cases?;
    let mut out = format!("failed at case {cases}");
    if let Some(shrinks) = test.shrinks.filter(|&shrinks| shrinks > 0) {
        let _ = write!(out, ", then shrunk in {shrinks} tries");
    }
    Some(out)
}

fn duration(took: Duration) -> String {
    let millis = took.as_millis();
    if millis < 1000 {
        format!("{millis}ms")
    } else {
        format!("{:.2}s", took.as_secs_f64())
    }
}

fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// The terminal's columns and rows, or a wide, tall one when it does not
/// say.
fn terminal_size() -> (usize, usize) {
    rustix::termios::tcgetwinsize(std::io::stdout())
        .ok()
        .filter(|size| size.ws_col > 0 && size.ws_row > 0)
        .map_or((120, 40), |size| {
            (usize::from(size.ws_col), usize::from(size.ws_row))
        })
}

const BOLD: &str = "1";
const DIM: &str = "2";
const FAINT: &str = "2;37";
const RED: &str = "31";
const GREEN: &str = "32";
const YELLOW: &str = "33";
const BLUE: &str = "34";
const MAGENTA: &str = "35";
const CYAN: &str = "36";
const RED_BOLD: &str = "1;31";
const GREEN_BOLD: &str = "1;32";

/// A line of text in pieces, each with its SGR style ("" for none).
#[derive(Default)]
struct Line(Vec<(&'static str, String)>);

impl Line {
    fn new() -> Self {
        Self::default()
    }

    fn add(mut self, style: &'static str, text: &str) -> Self {
        self.0.push((style, text.to_owned()));
        self
    }

    fn then(mut self, rest: Line) -> Self {
        self.0.extend(rest.0);
        self
    }

    /// The line as text, coloured when `color`, cut to `width` columns
    /// with an ellipsis.
    fn paint(&self, color: bool, width: usize) -> String {
        let mut out = String::new();
        let mut left = width;
        for (style, text) in &self.0 {
            if left == 0 {
                break;
            }
            let len = text.chars().count();
            let shown: String = if len > left {
                let mut cut: String =
                    text.chars().take(left.saturating_sub(1)).collect();
                cut.push('…');
                cut
            } else {
                text.clone()
            };
            left = left.saturating_sub(len.min(left));
            if color && !style.is_empty() {
                let _ = write!(out, "\x1b[{style}m{shown}\x1b[0m");
            } else {
                out.push_str(&shown);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_cut_to_its_width_with_an_ellipsis() {
        let line = Line::new().add(BOLD, "abc").add("", "defgh");
        assert_eq!(line.paint(false, 100), "abcdefgh");
        assert_eq!(line.paint(false, 5), "abcd…");
        assert_eq!(line.paint(false, 2), "a…");
        assert_eq!(line.paint(true, 3), "\x1b[1mabc\x1b[0m");
    }

    #[test]
    fn a_bar_fills_by_eighths() {
        assert_eq!(bar(0.0), "░".repeat(BAR));
        assert_eq!(bar(1.0), "█".repeat(BAR));
        assert_eq!(bar(0.55), format!("█████▌{}", "░".repeat(4)));
        assert_eq!(bar(0.5).chars().count(), BAR);
    }

    #[test]
    fn thousands_are_grouped() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn a_failed_property_says_where_it_failed_and_how_it_shrank() {
        let test = TestResult {
            file: "canvas".into(),
            name: "an edit replaces only its own text".into(),
            passed: false,
            error: Some("t.equal: got \"Xa\"".into()),
            kind: TestKind::Property,
            millis: Some(620),
            cases: Some(37),
            shrinks: Some(212),
        };
        let line = done_line(&test, true, 10).paint(false, usize::MAX);
        assert_eq!(
            line,
            "   └─ ✗ canvas › an edit replaces only its own text \
             failed at case 37, 212 shrinks 620ms"
        );
    }
}
