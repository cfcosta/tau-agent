//! Box-drawing and block characters drawn as shapes, so they meet
//! their neighbors with no gaps whatever the font and line height.

/// How thick one arm of a box-drawing character is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Weight {
    #[default]
    None,
    Light,
    Heavy,
    /// Two light lines side by side.
    Double,
}

/// The arms of a box-drawing character, from the cell's center to its
/// edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Arms {
    pub left: Weight,
    pub right: Weight,
    pub up: Weight,
    pub down: Weight,
}

/// A rectangle in a cell, in fractions of the cell's width and height
/// from its top-left corner, with an opacity for shades.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub alpha: f32,
}

/// What a character draws as, when it is drawn as a shape.
#[derive(Debug, Clone, PartialEq)]
pub enum Glyph {
    /// Lines from the center (box drawing, U+2500 to U+257F).
    Lines(Arms),
    /// Filled parts of the cell (block elements, U+2580 to U+259F).
    Rects(Vec<Rect>),
}

const fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
    Rect {
        x0,
        y0,
        x1,
        y1,
        alpha: 1.0,
    }
}

const FULL: Rect = rect(0.0, 0.0, 1.0, 1.0);
const UPPER_LEFT: Rect = rect(0.0, 0.0, 0.5, 0.5);
const UPPER_RIGHT: Rect = rect(0.5, 0.0, 1.0, 0.5);
const LOWER_LEFT: Rect = rect(0.0, 0.5, 0.5, 1.0);
const LOWER_RIGHT: Rect = rect(0.5, 0.5, 1.0, 1.0);

/// The shape `ch` draws as, or `None` for a character the font draws.
///
/// Covered: light, heavy and double straight lines, their corners, tees
/// and crosses where all arms have one weight (and the corners mixing
/// light and heavy), rounded corners (drawn square), half lines, and
/// every block element. Dashed lines and the other double forms are
/// left to the font.
pub fn glyph(ch: char) -> Option<Glyph> {
    use Weight::{Double as D, Heavy as H, Light as L, None as N};
    let arms = |left, right, up, down| {
        Some(Glyph::Lines(Arms {
            left,
            right,
            up,
            down,
        }))
    };
    let rects = |rects: &[Rect]| Some(Glyph::Rects(rects.to_vec()));
    let eighths = |n: f32| n / 8.0;
    match ch {
        '─' => arms(L, L, N, N),
        '━' => arms(H, H, N, N),
        '│' => arms(N, N, L, L),
        '┃' => arms(N, N, H, H),
        '┌' | '╭' => arms(N, L, N, L),
        '┍' => arms(N, H, N, L),
        '┎' => arms(N, L, N, H),
        '┏' => arms(N, H, N, H),
        '┐' | '╮' => arms(L, N, N, L),
        '┑' => arms(H, N, N, L),
        '┒' => arms(L, N, N, H),
        '┓' => arms(H, N, N, H),
        '└' | '╰' => arms(N, L, L, N),
        '┕' => arms(N, H, L, N),
        '┖' => arms(N, L, H, N),
        '┗' => arms(N, H, H, N),
        '┘' | '╯' => arms(L, N, L, N),
        '┙' => arms(H, N, L, N),
        '┚' => arms(L, N, H, N),
        '┛' => arms(H, N, H, N),
        '├' => arms(N, L, L, L),
        '┣' => arms(N, H, H, H),
        '┤' => arms(L, N, L, L),
        '┫' => arms(H, N, H, H),
        '┬' => arms(L, L, N, L),
        '┳' => arms(H, H, N, H),
        '┴' => arms(L, L, L, N),
        '┻' => arms(H, H, H, N),
        '┼' => arms(L, L, L, L),
        '╋' => arms(H, H, H, H),
        '═' => arms(D, D, N, N),
        '║' => arms(N, N, D, D),
        '╴' => arms(L, N, N, N),
        '╵' => arms(N, N, L, N),
        '╶' => arms(N, L, N, N),
        '╷' => arms(N, N, N, L),
        '╸' => arms(H, N, N, N),
        '╹' => arms(N, N, H, N),
        '╺' => arms(N, H, N, N),
        '╻' => arms(N, N, N, H),
        '▀' => rects(&[rect(0.0, 0.0, 1.0, 0.5)]),
        '▁'..='▇' => {
            let n = (ch as u32 - 0x2580) as f32;
            rects(&[rect(0.0, 1.0 - eighths(n), 1.0, 1.0)])
        }
        '█' => rects(&[FULL]),
        '▉'..='▏' => {
            let n = (0x2590 - ch as u32) as f32;
            rects(&[rect(0.0, 0.0, eighths(n), 1.0)])
        }
        '▐' => rects(&[rect(0.5, 0.0, 1.0, 1.0)]),
        '░' | '▒' | '▓' => {
            let alpha = (ch as u32 - 0x2590) as f32 * 0.25;
            rects(&[Rect { alpha, ..FULL }])
        }
        '▔' => rects(&[rect(0.0, 0.0, 1.0, eighths(1.0))]),
        '▕' => rects(&[rect(eighths(7.0), 0.0, 1.0, 1.0)]),
        '▖' => rects(&[LOWER_LEFT]),
        '▗' => rects(&[LOWER_RIGHT]),
        '▘' => rects(&[UPPER_LEFT]),
        '▙' => rects(&[UPPER_LEFT, LOWER_LEFT, LOWER_RIGHT]),
        '▚' => rects(&[UPPER_LEFT, LOWER_RIGHT]),
        '▛' => rects(&[UPPER_LEFT, UPPER_RIGHT, LOWER_LEFT]),
        '▜' => rects(&[UPPER_LEFT, UPPER_RIGHT, LOWER_RIGHT]),
        '▝' => rects(&[UPPER_RIGHT]),
        '▞' => rects(&[UPPER_RIGHT, LOWER_LEFT]),
        '▟' => rects(&[UPPER_RIGHT, LOWER_LEFT, LOWER_RIGHT]),
        _ => None,
    }
}
