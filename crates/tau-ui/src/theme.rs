//! The design language: every color, type size, radius, spacing step and
//! control size the interface uses. Screens and components take their
//! values from here and never write their own, so changing the look is a
//! change to this file.
//!
//! - Colors are fields of [`Theme`], read from the app's global.
//! - Type is a [`Type`]: size, weight, line height and face, set with
//!   [`Design::typeset`].
//! - Spacing is counted in steps of [`UNIT`] with [`sp`]: `sp(2.)` is two
//!   steps. Changing the unit makes the whole interface denser or looser.
//! - Corners come from [`radius`], icons from [`IconSize`], and the
//!   heights of buttons and fields from [`control`].

use gpui::{
    App,
    FontWeight,
    Global,
    Hsla,
    Pixels,
    Rgba,
    Styled,
    px,
    relative,
    rgb,
    rgba,
};

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
    /// Around the app when it is drawn in a frame (phone preview, fixed
    /// size).
    pub backdrop: Hsla,
    /// A quiet border, softer than [`Self::border`] on raised surfaces.
    pub border_soft: Hsla,
    /// A neutral blue-grey for secondary series in charts.
    pub slate: Hsla,
    /// Bars in a chart that were not picked.
    pub bar_idle: Hsla,
    /// Change ids, in the magenta jj writes them in.
    pub change: Hsla,
    /// Behind a plugin's badge and other small info marks.
    pub info_surface: Hsla,
    /// Behind an informational panel, such as the run plan.
    pub info_panel: Hsla,
    /// Behind and around the body of a blocked call.
    pub danger_surface: Hsla,
    pub danger_edge: Hsla,
    /// The colors a repository's mark can take.
    pub marks: [Hsla; 6],
    /// A command's terminal, drawn inside its card.
    pub term: TermLook,
    /// Onboarding's look: the handshake screens.
    pub setup: SetupLook,
    /// How surfaces stand out of the ground or sink into it.
    pub depth: Depth,
}

/// The milled look: surfaces lit from above. Panels and buttons rise
/// with a light top edge and a shadow under them; fields, meters and
/// the terminal sink into wells; the chrome casts a shadow onto what it
/// frames.
#[derive(Debug, Clone)]
pub struct Depth {
    /// The light along a raised surface's top edge.
    pub highlight: Hsla,
    /// The dark along a raised surface's bottom edge.
    pub shade: Hsla,
    /// The shadow a raised surface casts.
    pub drop: Hsla,
    /// The shadow a pressed or sunken surface holds inside.
    pub inner: Hsla,
    /// The seam where the chrome meets what it frames.
    pub seam: Hsla,
    /// A panel's top; it falls to [`Theme::card`].
    pub panel_top: Hsla,
    /// The chrome's top; it falls to [`Theme::panel`].
    pub chrome_top: Hsla,
    /// A well's floor: fields, meters, the terminal.
    pub well: Hsla,
    /// A key's top and bottom: buttons, chips, bubbles.
    pub key_top: Hsla,
    pub key_bottom: Hsla,
    pub key_border: Hsla,
    /// The accent key's top and bottom, and its lower edge.
    pub accent_top: Hsla,
    pub accent_bottom: Hsla,
    pub accent_edge: Hsla,
    /// The light the accent key and live dots throw.
    pub accent_glow: Hsla,
    /// The danger key's top and bottom.
    pub danger_top: Hsla,
    pub danger_bottom: Hsla,
}

impl Depth {
    fn graphite() -> Self {
        Self {
            highlight: c(rgba(0xffffff0f)),
            shade: c(rgba(0x00000080)),
            drop: c(rgba(0x00000066)),
            inner: c(rgba(0x000000b3)),
            seam: c(rgb(0x0c0d0f)),
            panel_top: c(rgb(0x1e1f24)),
            chrome_top: c(rgb(0x202126)),
            well: c(rgb(0x101114)),
            key_top: c(rgb(0x2c2d33)),
            key_bottom: c(rgb(0x212228)),
            key_border: c(rgb(0x34353c)),
            accent_top: c(rgb(0xf3bb67)),
            accent_bottom: c(rgb(0xd9922f)),
            accent_edge: c(rgba(0x78460a59)),
            accent_glow: c(rgba(0xe5a54a4d)),
            danger_top: c(rgb(0x241a19)),
            danger_bottom: c(rgb(0x1b1514)),
        }
    }
}

/// How onboarding looks: a near-black ground with faint rings and a glow
/// behind the handshake, light primary buttons, and dark tiles.
#[derive(Debug, Clone)]
pub struct SetupLook {
    /// Behind everything.
    pub ground: Hsla,
    /// A service's tile in the handshake, and small icon tiles.
    pub tile: Hsla,
    /// Chips, secondary buttons, code cells.
    pub surface: Hsla,
    /// Around chips and cards.
    pub surface_border: Hsla,
    /// Cards over the rings: see-through enough to show them.
    pub glass: Hsla,
    /// Between rows of a card.
    pub divider: Hsla,
    /// Behind a field inside a card.
    pub field: Hsla,
    /// A progress segment not reached yet.
    pub track: Hsla,
    /// The main action's fill, and the headline.
    pub light: Hsla,
    /// Text on [`Self::light`].
    pub on_light: Hsla,
    /// Secondary text on chips and steps.
    pub soft: Hsla,
    /// Footnotes and the top bar's words.
    pub faint: Hsla,
    /// The labels of progress segments not reached yet.
    pub idle: Hsla,
    /// A service tile's edge when connected.
    pub green_edge: Hsla,
    /// A service tile's edge at the top of its breath, while tau waits
    /// on it.
    pub waiting_edge: Hsla,
    /// The code of a refusal, under a blocked handshake.
    pub refusal: Hsla,
    /// How strong the rings are, from the inside out.
    pub ring_alphas: [f32; 5],
    /// The rings' radii on a desktop, from the inside out; a phone
    /// scales them by [`Self::compact_scale`].
    pub ring_radii: [f32; 5],
    pub compact_scale: f32,
    /// How strong the glow behind the handshake is.
    pub glow_alpha: f32,
}

impl SetupLook {
    fn graphite() -> Self {
        Self {
            ground: c(rgb(0x0b0c0e)),
            tile: c(rgb(0x17181c)),
            surface: c(rgb(0x131417)),
            surface_border: c(rgb(0x23242a)),
            glass: c(rgba(0x131417d9)),
            divider: c(rgb(0x1d1e23)),
            field: c(rgb(0x0f1012)),
            track: c(rgb(0x2a2b30)),
            light: c(rgb(0xf4f3ee)),
            on_light: c(rgb(0x111111)),
            soft: c(rgb(0xbdbbb5)),
            faint: c(rgb(0x6f6d68)),
            idle: c(rgb(0x55544f)),
            green_edge: c(rgb(0x2d4a33)),
            waiting_edge: c(rgb(0x6b5230)),
            refusal: c(rgb(0x7a5a55)),
            ring_alphas: [0.16, 0.10, 0.07, 0.05, 0.035],
            ring_radii: [150., 250., 360., 480., 610.],
            compact_scale: 0.45,
            glow_alpha: 0.10,
        }
    }
}

/// How a `bash` card's terminal looks: a darker ground of its own, and
/// the programs' own colors from a Ghostty-like palette, not remapped to
/// the theme's.
#[derive(Debug, Clone)]
pub struct TermLook {
    /// The colors programs pick, and the screen's ground and text.
    pub palette: tau_terminal::Palette,
    /// Around the screen.
    pub border: Hsla,
    /// Under the strip on top of the screen.
    pub divider: Hsla,
    /// The strip's text.
    pub label: Hsla,
    /// Behind the selected tab of the strip.
    pub tab: Hsla,
    /// The text the terminal is set in.
    pub text: Type,
    /// A row's height, in the text's sizes.
    pub leading: f32,
    /// Rows a closed screen shows at most.
    pub rows: usize,
}

const fn term_rgb(value: u32) -> tau_terminal::Rgb {
    tau_terminal::Rgb {
        r: (value >> 16) as u8,
        g: (value >> 8) as u8,
        b: value as u8,
    }
}

impl TermLook {
    fn graphite() -> Self {
        Self {
            palette: tau_terminal::Palette {
                ansi: [
                    term_rgb(0x1d1f24),
                    term_rgb(0xf0655a),
                    term_rgb(0x5fd068),
                    term_rgb(0xf2c14e),
                    term_rgb(0x5aa9f0),
                    term_rgb(0xc678dd),
                    term_rgb(0x3fc5c5),
                    term_rgb(0xd6d4ce),
                    term_rgb(0x8a8883),
                    term_rgb(0xff7b70),
                    term_rgb(0x7ee08a),
                    term_rgb(0xffd36b),
                    term_rgb(0x7cbcff),
                    term_rgb(0xd898ec),
                    term_rgb(0x62d8d8),
                    term_rgb(0xf2f0ea),
                ],
                foreground: term_rgb(0xd6d4ce),
                background: term_rgb(0x0e0f11),
                cursor: term_rgb(0xd6d4ce),
                selection: term_rgb(0x2c3a52),
                scrollbar: term_rgb(0x3a3c44),
            },
            border: c(rgb(0x23252c)),
            divider: c(rgb(0x1d1f24)),
            label: c(rgb(0x6f6d68)),
            tab: c(rgb(0x23252c)),
            text: Type::CAPTION.mono(),
            leading: 1.5,
            rows: 12,
        }
    }
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
            backdrop: c(rgb(0x0b0c0e)),
            border_soft: c(rgb(0x30323a)),
            slate: c(rgb(0x5c6b88)),
            bar_idle: c(rgb(0x3d4452)),
            change: c(rgb(0xc49bf0)),
            info_surface: c(rgb(0x1f2633)),
            info_panel: c(rgb(0x171a21)),
            danger_surface: c(rgb(0x1d1716)),
            danger_edge: c(rgb(0x3a2724)),
            marks: [
                c(rgb(0xe5a54a)),
                c(rgb(0x82aaff)),
                c(rgb(0x7fc98a)),
                c(rgb(0xc49bf0)),
                c(rgb(0xf0a37e)),
                c(rgb(0x7ec8c8)),
            ],
            term: TermLook::graphite(),
            setup: SetupLook::graphite(),
            depth: Depth::graphite(),
        }
    }

    /// A repository's mark color, by its name, so it keeps its color
    /// whatever else is listed.
    pub fn mark(&self, name: &str) -> Hsla {
        let hash = name.bytes().fold(0x811c_9dc5_u32, |hash, byte| {
            (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
        });
        self.marks[hash as usize % self.marks.len()]
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

/// One step of spacing. Every gap, padding and margin is a number of
/// these.
pub const UNIT: f32 = 4.;

/// `steps` steps of spacing: `sp(2.)` is 8 px at the default unit.
pub fn sp(steps: f32) -> Pixels {
    px(UNIT * steps)
}

/// A text style: size, and optionally weight, line height and the mono
/// face.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Type {
    pub size: f32,
    pub weight: Option<FontWeight>,
    /// A multiple of the size.
    pub line_height: Option<f32>,
    pub mono: bool,
}

impl Type {
    const fn new(size: f32) -> Self {
        Self {
            size,
            weight: None,
            line_height: None,
            mono: false,
        }
    }

    const fn weight(mut self, weight: FontWeight) -> Self {
        self.weight = Some(weight);
        self
    }

    const fn monospace(mut self) -> Self {
        self.mono = true;
        self
    }

    /// Small print: counters, tags, table headings.
    pub const MICRO: Self = Self::new(11.);
    /// Secondary lines: details, metadata, captions.
    pub const CAPTION: Self = Self::new(12.);
    /// Code and diffs in a block.
    pub const CODE: Self = Self::new(12.5).monospace();
    /// The desktop's base size, and supporting text.
    pub const SMALL: Self = Self::new(13.);
    /// Text in forms and task screens.
    pub const BODY: Self = Self::new(14.);
    /// The phone's base size.
    pub const PHONE: Self = Self::new(14.5);
    /// The paragraph under a big title.
    pub const LEAD: Self = Self::new(15.);
    /// A run's name in its header; a phone's task bar.
    pub const SUBTITLE: Self = Self::new(16.);
    /// The phone's screen titles.
    pub const TITLE: Self = Self::new(17.);
    /// A desktop screen's title.
    pub const HEADING: Self = Self::new(20.);
    /// A task screen's title on a phone.
    pub const HEADLINE: Self = Self::new(22.);
    /// A task screen's title on a desktop.
    pub const DISPLAY: Self = Self::new(28.);
    /// A note's title in the reader, on a phone.
    pub const READING_TITLE_COMPACT: Self = Self::new(26.);
    /// A note's title in the reader.
    pub const READING_TITLE: Self = Self::new(34.);
    /// A note's text in the reader, on a phone.
    pub const READING_COMPACT: Self = Self::new(16.);
    /// A note's text in the reader.
    pub const READING: Self = Self::new(18.);
    /// A code to read out and type, on a phone.
    pub const CODE_LARGE: Self = Self::new(26.).monospace();
    /// A code to read out and type, on a desktop.
    pub const CODE_HERO: Self = Self::new(40.).monospace();
    /// One character of a code, in its own cell, during onboarding.
    pub const CODE_CELL: Self = Self::new(30.).monospace();
    /// Onboarding's headline under the handshake.
    pub const HERO: Self = Self::new(44.).weight(FontWeight::MEDIUM);
    /// The welcome's headline.
    pub const HERO_LARGE: Self = Self::new(48.).weight(FontWeight::MEDIUM);
    /// Onboarding's headline over a form.
    pub const HERO_MEDIUM: Self = Self::new(40.).weight(FontWeight::MEDIUM);
    /// Onboarding's headline over a list.
    pub const HERO_SMALL: Self = Self::new(36.).weight(FontWeight::MEDIUM);

    /// This style at another size: for text that scales with what holds
    /// it, as τ does with its tile.
    pub const fn sized(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    /// This style in the mono face.
    pub const fn mono(self) -> Self {
        self.monospace()
    }

    /// This style at another weight.
    pub const fn weighted(self, weight: FontWeight) -> Self {
        self.weight(weight)
    }

    /// This style with a line height, as a multiple of the size.
    pub const fn leading(mut self, line_height: f32) -> Self {
        self.line_height = Some(line_height);
        self
    }
}

/// Font weights, by what they are for.
pub mod weight {
    use gpui::FontWeight;

    /// Names and labels that should stand out a little: the current
    /// crumb, a row's title.
    pub const EMPHASIS: FontWeight = FontWeight::MEDIUM;
    /// Titles, headings and primary buttons.
    pub const STRONG: FontWeight = FontWeight::SEMIBOLD;
}

/// Corner radii.
pub mod radius {
    use gpui::{Pixels, px};

    /// Thin bars and meters.
    pub const HAIRLINE: Pixels = px(2.);
    /// The tops of chart bars.
    pub const BAR: Pixels = px(3.);
    /// Checkboxes, small chips.
    pub const SMALL: Pixels = px(4.);
    /// Icon badges and step buttons.
    pub const TAG: Pixels = px(5.);
    /// Buttons, inline chips, list rows.
    pub const CONTROL: Pixels = px(6.);
    /// Fields, cards, boxes.
    pub const BOX: Pixels = px(8.);
    /// Transcript bubbles and grouped rows.
    pub const LARGE: Pixels = px(10.);
    /// Panels on task screens.
    pub const CARD: Pixels = px(12.);
    /// The phone's run cards.
    pub const TILE: Pixels = px(14.);
    pub const BUBBLE: Pixels = px(16.);
    /// The tops of bottom sheets.
    pub const SHEET: Pixels = px(18.);
    /// The phone preview's frame.
    pub const DEVICE: Pixels = px(28.);
    /// Fully round, for pills and circles.
    pub const FULL: Pixels = px(9999.);
}

/// Icon sizes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IconSize(pub f32);

impl IconSize {
    pub const TINY: Self = Self(11.);
    pub const SMALL: Self = Self(12.);
    pub const COMPACT: Self = Self(13.);
    pub const BASE: Self = Self(14.);
    pub const MEDIUM: Self = Self(15.);
    pub const LARGE: Self = Self(16.);
    pub const XLARGE: Self = Self(18.);
    pub const HUGE: Self = Self(20.);
}

/// The heights of controls.
pub mod control {
    use gpui::{Pixels, px};

    /// Buttons in headers and lists.
    pub const SMALL: Pixels = px(30.);
    /// Rows you can tap, and small fields.
    pub const MEDIUM: Pixels = px(36.);
    /// Big buttons and fields; the smallest touch target on a phone.
    pub const LARGE: Pixels = px(44.);
}

/// Setting design values on any element.
pub trait Design: Styled + Sized {
    /// Sets the text's size, and its weight, line height and face when
    /// the style has them.
    fn typeset(mut self, style: Type) -> Self {
        self = self.text_size(px(style.size));
        if let Some(weight) = style.weight {
            self = self.font_weight(weight);
        }
        if let Some(line_height) = style.line_height {
            self = self.line_height(relative(line_height));
        }
        if style.mono {
            self = self.font_family(MONO);
        }
        self
    }

    /// Sets a line height as a multiple of the size.
    fn leading(self, line_height: f32) -> Self {
        self.line_height(relative(line_height))
    }
}

impl<T: Styled + Sized> Design for T {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spacing_is_counted_in_units() {
        assert_eq!(sp(0.), px(0.));
        assert_eq!(sp(2.), px(2. * UNIT));
        assert_eq!(sp(-1.5), px(-1.5 * UNIT));
    }

    #[test]
    fn the_type_scale_grows_and_codes_are_mono() {
        let scale = [
            Type::MICRO,
            Type::CAPTION,
            Type::CODE,
            Type::SMALL,
            Type::BODY,
            Type::PHONE,
            Type::LEAD,
            Type::SUBTITLE,
            Type::TITLE,
            Type::HEADING,
            Type::HEADLINE,
            Type::DISPLAY,
        ];
        assert!(scale.windows(2).all(|pair| pair[0].size < pair[1].size));
        for code in [Type::CODE, Type::CODE_LARGE, Type::CODE_HERO] {
            assert!(code.mono);
        }
        assert!(Type::BODY.mono().mono && !Type::BODY.mono);
    }
}
