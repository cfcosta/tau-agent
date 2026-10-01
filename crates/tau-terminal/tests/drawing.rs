//! The pure parts of drawing a terminal: palette mapping, run batching
//! and shapes per row, wide characters, scroll math and selection text.

use hegel::{TestCase, generators as gs};
use tau_terminal::{
    Color,
    Line,
    Options,
    Palette,
    Point,
    Rgb,
    ScrollTo,
    Scroller,
    Selection,
    Size,
    Style,
    Terminal,
    glyph::{Glyph, glyph},
    layout::{char_at, layout},
    text_of,
};

fn small() -> Terminal {
    Terminal::new(Options {
        size: Size { cols: 12, rows: 5 },
        scrollback: 1 << 20,
        record: false,
    })
    .unwrap()
}

/// Every row of `terminal`: the scrollback, then the screen.
fn all_lines(terminal: &mut Terminal) -> Vec<Line> {
    let total = terminal.total_rows().unwrap();
    terminal.lines(0..total).unwrap()
}

/// Pieces of styled output: an SGR sequence (or none), then text of
/// characters drawn as text, as shapes, two cells wide, and blank.
#[hegel::composite]
fn styled_output(tc: &TestCase) -> Vec<u8> {
    const SGR: [&str; 9] = [
        "",
        "\x1b[0m",
        "\x1b[31m",
        "\x1b[1;32m",
        "\x1b[44m",
        "\x1b[7m",
        "\x1b[38;5;208m",
        "\x1b[48;2;10;20;30m",
        "\x1b[4m",
    ];
    const CHARS: [char; 10] =
        ['a', 'Z', ' ', '─', '│', '█', '▒', '日', 'é', '-'];
    let pieces: Vec<(usize, Vec<char>)> = tc.draw(
        gs::vecs(gs::tuples!(
            gs::integers::<usize>().max_value(SGR.len() - 1),
            gs::vecs(gs::sampled_from(CHARS.to_vec())).max_size(8),
        ))
        .max_size(12),
    );
    let mut out = Vec::new();
    for (sgr, text) in pieces {
        out.extend_from_slice(SGR[sgr].as_bytes());
        out.extend(text.iter().collect::<String>().bytes());
        if tc.draw(gs::booleans()) {
            out.extend_from_slice(b"\r\n");
        }
    }
    out
}

/// The palette maps the 16 ANSI colors to its own, the rest to the
/// xterm cube and gray ramp, and direct colors to themselves.
#[hegel::test(test_cases = 100)]
fn palette_entries_resolve(tc: TestCase) {
    let palette = Palette::default();
    let index: u8 = tc.draw(gs::integers::<u8>());
    let rgb = palette.resolve(Color::Palette(index));
    match index {
        0..=15 => assert_eq!(rgb, palette.ansi[usize::from(index)]),
        16..=231 => {
            let levels = [0, 95, 135, 175, 215, 255];
            let i = usize::from(index - 16);
            assert_eq!(
                (rgb.r, rgb.g, rgb.b),
                (levels[i / 36], levels[i / 6 % 6], levels[i % 6])
            );
        }
        _ => {
            assert!(rgb.r == rgb.g && rgb.g == rgb.b);
            assert_eq!(rgb.r, 8 + (index - 232) * 10);
        }
    }
    let direct = Rgb {
        r: tc.draw(gs::integers::<u8>()),
        g: tc.draw(gs::integers::<u8>()),
        b: tc.draw(gs::integers::<u8>()),
    };
    assert_eq!(palette.resolve(Color::Rgb(direct)), direct);
}

/// Inverse video swaps a style's colors, the palette's defaults
/// standing in for colors left unset.
#[hegel::test(test_cases = 100)]
fn inverse_swaps_the_colors(tc: TestCase) {
    let palette = Palette::default();
    let color = |tc: &TestCase| {
        tc.draw(gs::booleans())
            .then(|| Color::Palette(tc.draw(gs::integers::<u8>())))
    };
    let style = Style {
        fg: color(&tc),
        bg: color(&tc),
        ..Style::default()
    };
    let (fg, bg) = palette.colors(&style);
    let (inv_fg, inv_bg) = palette.colors(&Style {
        inverse: true,
        ..style
    });
    assert_eq!(inv_fg, bg.unwrap_or(palette.background));
    assert_eq!(inv_bg, Some(fg));
}

/// Laid out, a row's text and shapes never overlap, cover every drawn
/// character once, and keep one character per cell except for a wide
/// character; shapes are exactly the box-drawing and block characters;
/// adjacent backgrounds of one color are one rectangle.
#[hegel::test(test_cases = 200)]
fn a_row_lays_out_every_cell_once(tc: TestCase) {
    let output = tc.draw(styled_output());
    let mut terminal = small();
    terminal.write(&output).unwrap();
    let palette = Palette::default();
    for line in all_lines(&mut terminal) {
        let row = layout(&line, &palette);
        let mut covered = [false; 12];
        for text in &row.texts {
            let chars = text.text.chars().count();
            assert!(
                chars == usize::from(text.cells)
                    || (chars == 1 && text.cells == 2),
                "{text:?}"
            );
            for col in text.col..text.col + text.cells {
                assert!(
                    !covered[usize::from(col)],
                    "overlap at {col}: {row:?}"
                );
                covered[usize::from(col)] = true;
            }
            assert!(text.text.chars().all(|ch| glyph(ch).is_none()));
        }
        for shape in &row.shapes {
            assert!(!covered[usize::from(shape.col)], "{row:?}");
            covered[usize::from(shape.col)] = true;
            let (ch, _) = char_at(&line, shape.col).unwrap();
            assert_eq!(
                glyph(ch.chars().next().unwrap()),
                Some(shape.glyph.clone())
            );
        }
        // Every character that is not a blank is drawn.
        for run in &line.runs {
            if run.style.invisible {
                continue;
            }
            for (i, ch) in run.text.chars().enumerate() {
                if ch != ' '
                    && usize::from(run.cells) == run.text.chars().count()
                {
                    assert!(
                        covered[usize::from(run.col) + i],
                        "{ch:?} in {row:?}"
                    );
                }
            }
        }
        for pair in row.fills.windows(2) {
            assert!(
                !(pair[0].color == pair[1].color
                    && pair[0].col + pair[0].cells == pair[1].col),
                "unmerged {pair:?}"
            );
        }
    }
}

/// Runs of one style batch into one piece of text, split around shapes.
#[test]
fn runs_batch_and_split_around_shapes() {
    let mut terminal = small();
    terminal
        .write("\x1b[32mok ─ done\x1b[0m!".as_bytes())
        .unwrap();
    let line = &all_lines(&mut terminal)[0];
    let row = layout(line, &Palette::default());
    let texts: Vec<_> = row
        .texts
        .iter()
        .map(|text| (text.col, text.cells, text.text.as_str()))
        .collect();
    assert_eq!(texts, [(0, 3, "ok "), (4, 5, " done"), (9, 1, "!")]);
    assert_eq!(row.shapes.len(), 1);
    assert_eq!(row.shapes[0].col, 3);
    assert_eq!(row.texts[0].color, Palette::default().ansi[2]);
    assert!(matches!(row.shapes[0].glyph, Glyph::Lines(_)));
}

/// A wide character is its own run of two cells, and the cell after it
/// holds nothing of its own.
#[hegel::test(test_cases = 100)]
fn wide_characters_take_two_cells(tc: TestCase) {
    let text: String = tc
        .draw(
            gs::vecs(gs::sampled_from(vec!['a', '日', '🙂', ' '])).max_size(20),
        )
        .into_iter()
        .collect();
    let mut terminal = small();
    terminal.write(text.as_bytes()).unwrap();
    let lines = all_lines(&mut terminal);
    for line in &lines {
        let mut end = 0;
        for run in &line.runs {
            assert!(run.col >= end, "{line:?}");
            end = run.col + run.cells;
            if run.text.chars().any(|ch| ch == '日' || ch == '🙂') {
                assert_eq!((run.cells, run.text.chars().count()), (2, 1));
            }
        }
        assert!(end <= 12);
    }
    let joined: String = lines
        .iter()
        .map(|line| {
            let mut text = line.text_between(0, 12);
            if !line.wrapped {
                text = text.trim_end().to_owned() + "\n";
            }
            text
        })
        .collect();
    assert_eq!(joined.trim_end(), text.trim_end());
}

#[derive(Debug, Clone, hegel::PrettyPrintable)]
enum Op {
    Total(usize),
    Visible(usize),
    By(isize),
    Top,
    Bottom,
    Row(usize),
}

#[hegel::composite]
fn op(tc: &TestCase) -> Op {
    match tc.draw(gs::integers::<u8>().max_value(5)) {
        0 => Op::Total(tc.draw(gs::integers::<usize>().max_value(300))),
        1 => Op::Visible(
            tc.draw(gs::integers::<usize>().min_value(1).max_value(50)),
        ),
        2 | 3 => Op::By(
            tc.draw(gs::integers::<isize>().min_value(-80).max_value(80)),
        ),
        4 => match tc.draw(gs::integers::<u8>().max_value(2)) {
            0 => Op::Top,
            1 => Op::Bottom,
            _ => Op::Row(tc.draw(gs::integers::<usize>().max_value(400))),
        },
        _ => Op::Total(tc.draw(gs::integers::<usize>().max_value(20))),
    }
}

/// Whatever happens, the window stays within the rows, shows as many as
/// it can, follows the end exactly when it is there, and its thumb
/// stays on the track.
#[hegel::test(test_cases = 300)]
fn the_scroll_window_stays_in_bounds(tc: TestCase) {
    let mut scroller = Scroller::new(10);
    let ops: Vec<Op> = tc.draw(gs::vecs(op()).max_size(30));
    for op in ops {
        let was_following = scroller.follows();
        match op {
            Op::Total(total) => {
                scroller.set_total(total);
                if was_following {
                    assert_eq!(scroller.top(), scroller.max_top());
                }
            }
            Op::Visible(visible) => scroller.set_visible(visible),
            Op::By(rows) => {
                scroller.scroll_by(rows);
                assert_eq!(
                    scroller.follows(),
                    scroller.top() == scroller.max_top()
                );
            }
            Op::Top | Op::Bottom | Op::Row(_) => {
                let to = match op {
                    Op::Top => ScrollTo::Top,
                    Op::Bottom => ScrollTo::Bottom,
                    Op::Row(row) => ScrollTo::Row(row),
                    _ => unreachable!(),
                };
                scroller.scroll_to(to);
                if to == ScrollTo::Bottom {
                    assert!(scroller.follows());
                }
            }
        }
        assert!(scroller.top() <= scroller.max_top());
        assert_eq!(
            scroller.range().len(),
            scroller.visible().min(scroller.total())
        );
        if scroller.follows() {
            assert_eq!(scroller.top(), scroller.max_top());
        }
        match scroller.thumb(200.0, 16.0) {
            Some((offset, len)) => {
                assert!(scroller.scrolls());
                assert!(
                    offset >= 0.0
                        && len >= 16.0
                        && offset + len <= 200.0 + 1e-3
                );
                assert_eq!(scroller.top_at(offset, 200.0, len), scroller.top());
            }
            None => assert!(!scroller.scrolls()),
        }
    }
}

/// A selection reads the same whichever end the drag started from, and
/// one over every row reads as the whole text.
#[hegel::test(test_cases = 100)]
fn selections_read_the_text(tc: TestCase) {
    let output = tc.draw(styled_output());
    let mut terminal = small();
    terminal.write(&output).unwrap();
    let lines = all_lines(&mut terminal);
    let last = lines.len() - 1;
    let point = |tc: &TestCase| Point {
        row: tc.draw(gs::integers::<usize>().max_value(last)),
        col: tc.draw(gs::integers::<u16>().max_value(11)),
    };
    let (a, b) = (point(&tc), point(&tc));
    let forward = Selection { anchor: a, head: b };
    let backward = Selection { anchor: b, head: a };
    assert_eq!(forward.text(&lines, 0, 12), backward.text(&lines, 0, 12));

    let all = Selection {
        anchor: Point { row: 0, col: 0 },
        head: Point { row: last, col: 11 },
    };
    assert_eq!(all.text(&lines, 0, 12), text_of(&lines, 12));
}

/// Soft-wrapped rows join into one line; rows ending with a newline
/// lose their trailing blanks.
#[test]
fn copied_text_joins_wraps() {
    let mut terminal = small();
    terminal.write(b"0123456789abcdef\r\nx  \r\n").unwrap();
    let lines = all_lines(&mut terminal);
    assert_eq!(text_of(&lines, 12).trim_end(), "0123456789abcdef\nx");
    let middle = Selection {
        anchor: Point { row: 0, col: 10 },
        head: Point { row: 1, col: 1 },
    };
    assert_eq!(middle.text(&lines, 0, 12), "abcd");
}

/// Box-drawing lines and blocks are shapes; other characters are not.
#[test]
fn box_and_block_characters_are_shapes() {
    for ch in ['─', '│', '┌', '┼', '═', '╭', '█', '▀', '▁', '▏', '░', '▚']
    {
        assert!(glyph(ch).is_some(), "{ch}");
    }
    for ch in ['a', '-', '|', '+', '┄', '╔'] {
        assert!(glyph(ch).is_none(), "{ch}");
    }
    let Some(Glyph::Rects(rects)) = glyph('▄') else {
        panic!("a lower half block is a rectangle");
    };
    assert_eq!((rects[0].y0, rects[0].y1), (0.5, 1.0));
}
