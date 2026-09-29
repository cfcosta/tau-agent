//! `tau_terminal::Terminal`: plain text, recording, styled snapshots
//! and replay (`docs/decisions/0010-terminal-rendering.md`).

use hegel::{TestCase, generators as gs};
use tau_terminal::{Options, Rgb, Size, Terminal, Underline};

/// Characters plain output is made of: printable ASCII, the space,
/// and characters of two, three and four bytes, one of them two cells
/// wide. No control characters and no tab.
const PLAIN: [char; 12] = [
    'a', 'Z', '0', ' ', '~', '-', 'é', 'ß', '€', '日', '界', '🙂',
];

/// Plain output: lines of [`PLAIN`] characters, some longer than a
/// small terminal is wide, joined by `\n`, with or without a last
/// newline.
#[hegel::composite]
fn plain_output(tc: &TestCase) -> String {
    let lines: Vec<Vec<char>> = tc.draw(
        gs::vecs(gs::vecs(gs::sampled_from(PLAIN.to_vec())).max_size(30))
            .max_size(40),
    );
    let mut text = lines
        .iter()
        .map(|line| line.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    if tc.draw(gs::booleans()) {
        text.push('\n');
    }
    text
}

/// What a terminal's line discipline does to output: `\n` becomes
/// `\r\n`.
fn onlcr(text: &str) -> Vec<u8> {
    text.replace('\n', "\r\n").into_bytes()
}

/// Splits `bytes` at arbitrary points, inside UTF-8 characters too.
fn chunks(bytes: &[u8], tc: &TestCase) -> Vec<Vec<u8>> {
    let mut cuts: Vec<usize> = tc.draw(
        gs::vecs(gs::integers::<usize>().min_value(0).max_value(bytes.len()))
            .max_size(10),
    );
    cuts.extend([0, bytes.len()]);
    cuts.sort_unstable();
    cuts.dedup();
    cuts.windows(2)
        .map(|w| bytes[w[0]..w[1]].to_vec())
        .collect()
}

fn small(record: bool) -> Terminal {
    Terminal::new(Options {
        size: Size { cols: 12, rows: 5 },
        scrollback: 1 << 20,
        record,
    })
    .unwrap()
}

/// Plain output through a terminal reads back as itself: `text` of
/// output with no control characters but `\r\n` line ends is the
/// output with `\n` line ends, whatever the chunking, however the lines
/// wrap, and whether or not it ends with a newline.
#[hegel::test(test_cases = 200)]
fn plain_output_reads_back_as_itself(tc: TestCase) {
    let text = tc.draw(plain_output());
    let mut terminal = small(false);
    for chunk in chunks(&onlcr(&text), &tc) {
        terminal.write(&chunk).unwrap();
    }
    assert_eq!(terminal.text().unwrap(), text);
}

/// Recording loses nothing and repeats nothing: all the recorded text,
/// then `text`, is the whole output, even when a small scrollback
/// drops most rows.
#[hegel::test(test_cases = 200)]
fn recorded_text_then_text_is_the_whole_output(tc: TestCase) {
    let text = tc.draw(plain_output());
    let mut terminal = Terminal::new(Options {
        size: Size { cols: 12, rows: 5 },
        scrollback: 0,
        record: true,
    })
    .unwrap();
    let mut seen = String::new();
    for chunk in chunks(&onlcr(&text), &tc) {
        terminal.write(&chunk).unwrap();
        seen.push_str(&terminal.take_recorded());
    }
    seen.push_str(&terminal.text().unwrap());
    assert_eq!(seen, text);
}

/// A progress bar redrawn with `\r` leaves only its last frame, as a
/// person saw it.
#[test]
fn a_carriage_return_redraw_leaves_the_last_frame() {
    let mut terminal = small(false);
    terminal.write(b"10%\r50%\r100%\r\ndone\r\n").unwrap();
    assert_eq!(terminal.text().unwrap(), "100%\ndone\n");
}

/// Escape sequences never reach the text; their colors reach the
/// snapshot, resolved through the palette.
#[test]
fn colors_reach_the_snapshot_and_not_the_text() {
    let mut terminal = small(false);
    terminal
        .write(b"\x1b[1;31mred\x1b[0m \x1b[38;2;1;2;3mrgb\x1b[4m!")
        .unwrap();
    assert_eq!(terminal.text().unwrap(), "red rgb!");

    let screen = terminal.snapshot().unwrap();
    let runs = &screen.lines[0].runs;
    assert_eq!(runs[0].text, "red");
    assert!(runs[0].style.bold);
    let red = runs[0].style.fg.expect("red has a color");
    assert!(red.r > red.g && red.r > red.b, "{red:?}");
    assert_eq!(runs[1].text, " ");
    assert_eq!(runs[1].style.fg, None);
    assert_eq!(runs[2].text, "rgb");
    assert_eq!(runs[2].style.fg, Some(Rgb { r: 1, g: 2, b: 3 }));
    assert_eq!(runs[3].text, "!");
    assert_eq!(runs[3].style.underline, Underline::Single);
    assert_eq!(
        runs.iter().map(|run| run.col).collect::<Vec<_>>(),
        [0, 3, 4, 7]
    );
}

/// A wide character is a run of its own, two cells wide, so every run
/// starts at its column.
#[test]
fn a_wide_character_is_a_two_cell_run() {
    let mut terminal = small(false);
    terminal.write("ab日本c".as_bytes()).unwrap();
    let screen = terminal.snapshot().unwrap();
    let runs: Vec<_> = screen.lines[0]
        .runs
        .iter()
        .map(|run| (run.col, run.cells, run.text.as_str()))
        .collect();
    assert_eq!(
        runs,
        [(0, 2, "ab"), (2, 2, "日"), (4, 2, "本"), (6, 1, "c")]
    );
    assert_eq!(screen.text().lines().next(), Some("ab日本c"));
    let cursor = screen.cursor.unwrap();
    assert_eq!((cursor.col, cursor.row), (7, 0));
}

/// The snapshot has one line per row, marks soft wraps, and shows the
/// cursor where output goes.
#[test]
fn the_snapshot_covers_the_viewport() {
    let mut terminal = small(false);
    terminal.write(b"0123456789abcdef\r\nx").unwrap();
    let screen = terminal.snapshot().unwrap();
    assert_eq!((screen.cols, screen.rows), (12, 5));
    assert_eq!(screen.lines.len(), 5);
    assert!(screen.lines[0].wrapped);
    assert!(!screen.lines[1].wrapped);
    assert_eq!(screen.text(), "0123456789ab\ncdef\nx\n\n");
    let cursor = screen.cursor.unwrap();
    assert_eq!((cursor.col, cursor.row), (1, 2));
}

/// Without recording, the scrollback limit drops the oldest rows; with
/// it, every row is kept in the recorded text.
#[test]
fn the_scrollback_is_capped_and_recording_keeps_every_row() {
    let output: String = (0..20_000).map(|i| format!("line {i}\n")).collect();

    let mut capped = Terminal::new(Options {
        size: Size::TOOL,
        scrollback: 64 << 10,
        record: false,
    })
    .unwrap();
    capped.write(&onlcr(&output)).unwrap();
    let kept = capped.text().unwrap();
    assert!(
        kept.ends_with("line 19999\n"),
        "{}",
        &kept[kept.len() - 40..]
    );
    assert!(!kept.starts_with("line 0\n"));
    assert!(kept.lines().count() < 20_000);

    let mut recording = Terminal::new(Options {
        size: Size::TOOL,
        scrollback: 64 << 10,
        record: true,
    })
    .unwrap();
    recording.write(&onlcr(&output)).unwrap();
    let mut all = recording.take_recorded();
    all.push_str(&recording.text().unwrap());
    assert_eq!(all, output);
}

/// Rows a program pushes down with reverse index or insert line, above
/// the rows recorded so far, are still recorded once they scroll off.
#[test]
fn rows_inserted_at_the_top_are_recorded() {
    let mut terminal = Terminal::new(Options {
        size: Size { cols: 12, rows: 3 },
        scrollback: 0,
        record: true,
    })
    .unwrap();
    // Three rows, then a row inserted at the top, then enough to scroll
    // everything off.
    terminal.write(b"a\r\nb\r\nc").unwrap();
    terminal.write(b"\x1b[H\x1b[Ltop").unwrap();
    terminal.write(b"\x1b[3;1H\r\nd\r\ne\r\nf\r\ng").unwrap();
    let mut all = terminal.take_recorded();
    all.push_str(&terminal.text().unwrap());
    assert_eq!(all, "top\na\nb\nd\ne\nf\ng");
}

/// A tab becomes the spaces up to the next tab stop, and blank rows up
/// to the cursor are lines.
#[test]
fn tabs_are_spaces_and_blank_rows_count() {
    let mut terminal = small(false);
    terminal.write(b"a\tb\r\n\r\n\r\n").unwrap();
    assert_eq!(terminal.text().unwrap(), "a       b\n\n\n");
}

/// The VT rendering rebuilds the screen in a new terminal of the same
/// size.
#[test]
fn the_vt_rendering_rebuilds_the_screen() {
    let mut terminal = small(false);
    terminal
        .write(b"\x1b[32mgreen\x1b[0m\r\n\x1b[7minverse\x1b[0m\r\n10%\r99%")
        .unwrap();
    let vt = terminal.vt().unwrap();
    let mut copy = small(false);
    copy.write(&vt).unwrap();
    assert_eq!(copy.snapshot().unwrap(), terminal.snapshot().unwrap());
    assert_eq!(copy.text().unwrap(), terminal.text().unwrap());
}

/// Output on the alternate screen is gone once the program leaves it,
/// as in a real terminal.
#[test]
fn the_alternate_screen_leaves_no_text() {
    let mut terminal = small(true);
    terminal
        .write(b"before\r\n\x1b[?1049hfull screen\x1b[?1049lafter\r\n")
        .unwrap();
    let mut all = terminal.take_recorded();
    all.push_str(&terminal.text().unwrap());
    assert_eq!(all, "before\nafter\n");
}

/// Resizing reflows soft-wrapped lines; the text is unchanged.
#[test]
fn resizing_keeps_the_text() {
    let mut terminal = small(false);
    terminal.write(b"0123456789abcdef\r\nx\r\n").unwrap();
    terminal.resize(Size { cols: 20, rows: 5 }).unwrap();
    assert_eq!(terminal.size(), Size { cols: 20, rows: 5 });
    assert_eq!(terminal.text().unwrap(), "0123456789abcdef\nx\n");
}
