//! Colors, type and the width where the layout switches to phone mode.

use gpui::{App, Global, Hsla, Pixels, Rgba, px, rgb, rgba};

use crate::view::Tone;

pub const SANS: &str = "Geist";
pub const MONO: &str = "Geist Mono";
/// The reading face of notes.
pub const SERIF: &str = "Newsreader 16pt 16pt";

/// Below this width the window gets the one-column phone layout.
pub const PHONE_MAX: Pixels = px(720.);
/// Below this width the desktop layout drops the inspector.
pub const NARROW_MAX: Pixels = px(1100.);

/// The graphite theme from the mockups.
#[derive(Debug, Clone)]
pub struct Theme {
    /// The editor ground, behind the transcript.
    pub bg: Hsla,
    /// Title bar, sidebar, inspector, composer.
    pub panel: Hsla,
    /// Cards and code backgrounds.
    pub card: Hsla,
    pub raised: Hsla,
    pub selected: Hsla,
    pub border: Hsla,
    pub border_strong: Hsla,
    pub text: Hsla,
    pub text_soft: Hsla,
    pub muted: Hsla,
    pub dim: Hsla,
    pub accent: Hsla,
    pub accent_soft: Hsla,
    pub accent_border: Hsla,
    pub blue: Hsla,
    pub blue_soft: Hsla,
    pub blue_border: Hsla,
    pub green: Hsla,
    pub green_soft: Hsla,
    pub red: Hsla,
    pub red_soft: Hsla,
    pub red_border: Hsla,
    pub added_text: Hsla,
    pub removed_text: Hsla,
    pub scrim: Hsla,
}

fn c(color: Rgba) -> Hsla {
    color.into()
}

impl Theme {
    pub fn graphite() -> Self {
        Self {
            bg: c(rgb(0x141518)),
            panel: c(rgb(0x1b1c20)),
            card: c(rgb(0x18191d)),
            raised: c(rgb(0x23252c)),
            selected: c(rgb(0x2a2b31)),
            border: c(rgb(0x2c2d33)),
            border_strong: c(rgb(0x3a3b42)),
            text: c(rgb(0xe7e5e0)),
            text_soft: c(rgb(0xdcdad4)),
            muted: c(rgb(0xa3a19b)),
            dim: c(rgb(0x8b8983)),
            accent: c(rgb(0xe5a54a)),
            accent_soft: c(rgba(0xe5a54a1f)),
            accent_border: c(rgb(0x4a3b24)),
            blue: c(rgb(0x82aaff)),
            blue_soft: c(rgba(0x82aaff0d)),
            blue_border: c(rgb(0x2c3a52)),
            green: c(rgb(0x7fc98a)),
            green_soft: c(rgba(0x7fc98a1f)),
            red: c(rgb(0xf08a7e)),
            red_soft: c(rgba(0xf08a7e14)),
            red_border: c(rgb(0x5a2f2b)),
            added_text: c(rgb(0xa9dcb1)),
            removed_text: c(rgb(0xf3b3aa)),
            scrim: c(rgba(0x08080a99)),
        }
    }

    /// The color a tone is written in.
    pub fn tone(&self, tone: Tone) -> Hsla {
        match tone {
            Tone::Info => self.blue,
            Tone::Warn => self.accent,
            Tone::Danger => self.red,
            Tone::Good => self.green,
            Tone::Quiet => self.muted,
        }
    }
}

impl Global for Theme {}

/// The app's theme. [`crate::init`] sets it.
pub fn theme(cx: &App) -> &Theme {
    cx.global::<Theme>()
}
