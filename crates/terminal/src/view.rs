//! Drawing a [`Screen`](crate::Screen) with GPUI.
//!
//! The terminal element that draws a [`Screen`](crate::Screen) in any
//! GPUI tree lives here. A renderer takes a snapshot on the thread that
//! owns the [`Terminal`](crate::Terminal) and draws it: backgrounds as
//! rectangles, each [`TextRun`](crate::TextRun) as one shaped line at
//! `col * cell_width`, then the cursor.
//!
//! What is here so far are the color conversions it draws with.

use gpui::{Hsla, Rgba};

use crate::screen::Rgb;

impl From<Rgb> for Rgba {
    fn from(color: Rgb) -> Self {
        Rgba {
            r: f32::from(color.r) / 255.0,
            g: f32::from(color.g) / 255.0,
            b: f32::from(color.b) / 255.0,
            a: 1.0,
        }
    }
}

impl From<Rgb> for Hsla {
    fn from(color: Rgb) -> Self {
        Rgba::from(color).into()
    }
}
