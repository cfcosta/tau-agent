//! [`Palette`]: the colors a renderer resolves a [`Style`] through.

use crate::screen::{Color, Rgb, Style};

/// The colors a terminal is drawn in: the 16 ANSI colors a program
/// picks by number, the default foreground and background, and the
/// renderer's own marks (cursor, selection, scrollbar).
///
/// Entries 16 to 255 of the 256-color palette are the standard xterm
/// color cube and gray ramp, the same under every palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Palette {
    /// Black, red, green, yellow, blue, magenta, cyan, white, then
    /// their bright forms.
    pub ansi: [Rgb; 16],
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Rgb,
    /// Behind selected text (drawn translucent).
    pub selection: Rgb,
    /// The scrollbar's thumb.
    pub scrollbar: Rgb,
}

const fn hex(value: u32) -> Rgb {
    Rgb {
        r: (value >> 16) as u8,
        g: (value >> 8) as u8,
        b: value as u8,
    }
}

impl Default for Palette {
    /// Ghostty's built-in colors (Tomorrow Night) on a dark ground.
    fn default() -> Self {
        Self {
            ansi: [
                hex(0x1d1f21),
                hex(0xcc6666),
                hex(0xb5bd68),
                hex(0xf0c674),
                hex(0x81a2be),
                hex(0xb294bb),
                hex(0x8abeb7),
                hex(0xc5c8c6),
                hex(0x666666),
                hex(0xd54e53),
                hex(0xb9ca4a),
                hex(0xe7c547),
                hex(0x7aa6da),
                hex(0xc397d8),
                hex(0x70c0b1),
                hex(0xeaeaea),
            ],
            foreground: hex(0xffffff),
            background: hex(0x282c34),
            cursor: hex(0xffffff),
            selection: hex(0x5a6378),
            scrollbar: hex(0x4b5263),
        }
    }
}

/// The levels of the 6×6×6 color cube (entries 16 to 231).
const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];

impl Palette {
    /// Entry `index` of the 256-color palette.
    pub fn indexed(&self, index: u8) -> Rgb {
        match index {
            0..=15 => self.ansi[usize::from(index)],
            16..=231 => {
                let i = index - 16;
                Rgb {
                    r: CUBE[usize::from(i / 36)],
                    g: CUBE[usize::from(i / 6 % 6)],
                    b: CUBE[usize::from(i % 6)],
                }
            }
            232..=255 => {
                let level = 8 + (index - 232) * 10;
                Rgb {
                    r: level,
                    g: level,
                    b: level,
                }
            }
        }
    }

    pub fn resolve(&self, color: Color) -> Rgb {
        match color {
            Color::Palette(index) => self.indexed(index),
            Color::Rgb(rgb) => rgb,
        }
    }

    /// The colors a style draws in: its text color, and its background
    /// when it has one of its own (`None` is the palette's background,
    /// left undrawn). Inverse video swaps the two, defaults included.
    pub fn colors(&self, style: &Style) -> (Rgb, Option<Rgb>) {
        let fg = style.fg.map(|color| self.resolve(color));
        let bg = style.bg.map(|color| self.resolve(color));
        if style.inverse {
            (
                bg.unwrap_or(self.background),
                Some(fg.unwrap_or(self.foreground)),
            )
        } else {
            (fg.unwrap_or(self.foreground), bg)
        }
    }
}
