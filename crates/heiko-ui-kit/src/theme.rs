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
//!
//! The look is flat and printed: no gradients, no soft shadows, no
//! rounded boxes. Structure comes from rules ([`Theme::rule`] under a
//! section's title, [`Theme::hairline`] between rows), and the one
//! decoration is a hard offset behind a block ([`Theme::offset`]).

use gpui::{
    App,
    FontWeight,
    Global,
    Hsla,
    Pixels,
    Rems,
    Rgba,
    Styled,
    px,
    relative,
    rems,
    rgb,
    rgba,
};
use serde::{Deserialize, Serialize};

/// How something reads: the color a note, a status or a badge takes.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize,
)]
pub enum Tone {
    #[default]
    Info,
    Warn,
    Danger,
    Good,
    Quiet,
}

/// What a tool does (ADR 0060). It gives the tool's name its color, and
/// its plugin declares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    /// Reads what is there: a file, a listing, a change.
    Look,
    /// Looks for something: grep, find, a memory recall.
    Search,
    /// Changes something: a file, a commit, an issue, a note.
    Change,
    /// Runs something: a command, a script.
    Run,
    /// Reaches outside: the web, an MCP server, a forge.
    Reach,
    /// Speaks with someone: the person, or an agent.
    People,
}

impl ToolKind {
    pub const ALL: [Self; 6] = [
        Self::Look,
        Self::Search,
        Self::Change,
        Self::Run,
        Self::Reach,
        Self::People,
    ];

    /// The kind its name says: `change`.
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }

    /// Its name, as a plugin declares it: `change`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Look => "look",
            Self::Search => "search",
            Self::Change => "change",
            Self::Run => "run",
            Self::Reach => "reach",
            Self::People => "people",
        }
    }

    /// The kind of one of heiko's own tools, by its name; any MCP
    /// tool reaches. A plugin's tool declares its own.
    pub fn of(tool: &str) -> Option<Self> {
        Some(match tool {
            "read" | "ls" | "image" | "artifact_read" | "vcs_status"
            | "vcs_log" | "vcs_show" | "vcs_diff" | "pending_asks"
            | "scheduled" => Self::Look,
            "grep" | "find" => Self::Search,
            "edit" | "write" | "vcs_commit" | "vcs_describe" | "vcs_new"
            | "vcs_restore" | "vcs_undo" | "vcs_resolve" | "vcs_land"
            | "artifact_grant" | "artifact_prune" | "schedule"
            | "unschedule" => Self::Change,
            "exec_command" | "write_stdin" | "codemode" => Self::Run,
            "http" | "notify" => Self::Reach,
            "ask" | "agent_send" | "spawn" | "wait" => Self::People,
            _ if tool.starts_with("mcp__") => Self::Reach,
            _ => return None,
        })
    }
}

pub const SANS: &str = "Geist";
pub const MONO: &str = "Geist Mono";
/// The reading face of notes.
pub const SERIF: &str = "Newsreader 16pt 16pt";
/// The display face: section titles, a page's title, big figures and the
/// wordmark. It has one weight, and the Japanese glyphs the marks use.
pub const DISPLAY: &str = "RocknRoll One";

/// Below this width the window gets the one-column phone layout.
pub const PHONE_MAX: Pixels = px(720.);
/// Below this width the desktop layout drops the inspector.
pub const NARROW_MAX: Pixels = px(1100.);

/// The interface's colors: [`Theme::tokyo_night`].
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
    /// The heavy rule under a section's title.
    pub rule: Hsla,
    /// The thin rule between rows, and the offset behind a stamp.
    pub hairline: Hsla,
    /// The hard offset printed behind the primary action and a selected
    /// option.
    pub offset: Hsla,
    pub text: Hsla,
    /// Titles in the display face, big figures and the wordmark.
    pub bright: Hsla,
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
    /// How code is colored, by what each part of it is.
    pub syntax: SyntaxLook,
    /// The colors things take by what they are: a running run, a cost,
    /// a tool. A palette's own hues, spread over the interface.
    pub roles: Roles,
}

/// One color per role, so a palette shows all its hues instead of one
/// accent everywhere.
#[derive(Debug, Clone)]
pub struct Roles {
    /// A running run, and a call in flight.
    pub live: Hsla,
    /// A run waiting to land or for a person.
    pub waiting: Hsla,
    /// Money.
    pub cost: Hsla,
    /// An agent's name.
    pub agent: Hsla,
    /// The model's name.
    pub model: Hsla,
    /// Memory: its notes and what recalls them.
    pub memory: Hsla,
    /// The constitution's rules.
    pub rules: Hsla,
    /// MCP servers.
    pub mcp: Hsla,
    /// The reasoning effort.
    pub reasoning: Hsla,
    /// A branch or bookmark.
    pub branch: Hsla,
    /// The context meter's fill.
    pub meter: Hsla,
    /// Links.
    pub link: Hsla,
    /// Code inline in prose.
    pub code: Hsla,
    /// The primary action's fill, and the text on it.
    pub primary: Hsla,
    pub on_primary: Hsla,
    /// Tools by their kind ([`ToolKind`]).
    pub look: Hsla,
    pub search: Hsla,
    pub change: Hsla,
    pub run: Hsla,
    pub reach: Hsla,
    pub people: Hsla,
}

impl Roles {
    fn tokyo_night() -> Self {
        Self {
            live: c(rgb(0xff9e64)),
            waiting: c(rgb(0x7aa2f7)),
            cost: c(rgb(0xe0af68)),
            agent: c(rgb(0xbb9af7)),
            model: c(rgb(0x7aa2f7)),
            memory: c(rgb(0x73daca)),
            rules: c(rgb(0xe0af68)),
            mcp: c(rgb(0x2ac3de)),
            reasoning: c(rgb(0xff9e64)),
            branch: c(rgb(0x9ece6a)),
            meter: c(rgb(0x2ac3de)),
            link: c(rgb(0x7dcfff)),
            code: c(rgb(0x7dcfff)),
            primary: c(rgb(0x7aa2f7)),
            on_primary: c(rgb(0x16161e)),
            look: c(rgb(0x7aa2f7)),
            search: c(rgb(0x73daca)),
            change: c(rgb(0xff9e64)),
            run: c(rgb(0x7dcfff)),
            reach: c(rgb(0xe0af68)),
            people: c(rgb(0xbb9af7)),
        }
    }

    /// The color a plugin's name is written in, where it speaks.
    pub fn plugin(&self, name: &str) -> Option<Hsla> {
        Some(match name {
            "heiko-reasoning" => self.reasoning,
            "heiko-memory" => self.memory,
            "heiko-constitution" => self.rules,
            "heiko-mcp" => self.mcp,
            "heiko-compaction" | "heiko-fast-compaction" => self.look,
            "heiko-codemode" => self.run,
            "heiko-ask" => self.agent,
            _ => return None,
        })
    }

    /// The color a tool of `kind` has its name written in.
    pub fn kind(&self, kind: ToolKind) -> Hsla {
        match kind {
            ToolKind::Look => self.look,
            ToolKind::Search => self.search,
            ToolKind::Change => self.change,
            ToolKind::Run => self.run,
            ToolKind::Reach => self.reach,
            ToolKind::People => self.people,
        }
    }
}

/// How code is colored (`syntax`): one color per kind of part, quiet
/// enough to sit in a card; plain code keeps the color it is set in.
#[derive(Debug, Clone)]
pub struct SyntaxLook {
    pub keyword: Hsla,
    pub string: Hsla,
    pub escape: Hsla,
    pub comment: Hsla,
    pub function: Hsla,
    pub ty: Hsla,
    pub number: Hsla,
    pub property: Hsla,
    pub attribute: Hsla,
    pub builtin: Hsla,
    pub punctuation: Hsla,
    pub tag: Hsla,
}

impl SyntaxLook {
    fn tokyo_night() -> Self {
        Self {
            keyword: c(rgb(0xbb9af7)),
            string: c(rgb(0x9ece6a)),
            escape: c(rgb(0x89ddff)),
            comment: c(rgb(0x565f89)),
            function: c(rgb(0x7aa2f7)),
            ty: c(rgb(0x2ac3de)),
            number: c(rgb(0xff9e64)),
            property: c(rgb(0x73daca)),
            attribute: c(rgb(0xe0af68)),
            builtin: c(rgb(0xf7768e)),
            punctuation: c(rgb(0x89ddff)),
            tag: c(rgb(0xf7768e)),
        }
    }

    /// The same colors at `opacity`, for code around a change.
    pub fn faded(&self, opacity: f32) -> Self {
        let f = |color: Hsla| color.opacity(opacity);
        Self {
            keyword: f(self.keyword),
            string: f(self.string),
            escape: f(self.escape),
            comment: f(self.comment),
            function: f(self.function),
            ty: f(self.ty),
            number: f(self.number),
            property: f(self.property),
            attribute: f(self.attribute),
            builtin: f(self.builtin),
            punctuation: f(self.punctuation),
            tag: f(self.tag),
        }
    }

    /// The color of a kind of part.
    pub fn color(&self, kind: crate::syntax::Kind) -> Hsla {
        use crate::syntax::Kind;
        match kind {
            Kind::Keyword => self.keyword,
            Kind::String => self.string,
            Kind::Escape => self.escape,
            Kind::Comment => self.comment,
            Kind::Function => self.function,
            Kind::Type => self.ty,
            Kind::Number | Kind::Constant => self.number,
            Kind::Property => self.property,
            Kind::Attribute => self.attribute,
            Kind::Builtin => self.builtin,
            Kind::Operator | Kind::Punctuation => self.punctuation,
            Kind::Tag => self.tag,
        }
    }
}

/// How a `bash` card's terminal looks: a darker ground of its own, and
/// the programs' own colors from a Ghostty-like palette, not remapped to
/// the theme's.
#[derive(Debug, Clone)]
pub struct TermLook {
    /// The colors programs pick, and the screen's ground and text.
    pub palette: heiko_terminal::Palette,
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

const fn term_rgb(value: u32) -> heiko_terminal::Rgb {
    heiko_terminal::Rgb {
        r: (value >> 16) as u8,
        g: (value >> 8) as u8,
        b: value as u8,
    }
}

impl TermLook {
    fn tokyo_night() -> Self {
        Self {
            palette: heiko_terminal::Palette {
                ansi: [
                    term_rgb(0x15161e),
                    term_rgb(0xf7768e),
                    term_rgb(0x9ece6a),
                    term_rgb(0xe0af68),
                    term_rgb(0x7aa2f7),
                    term_rgb(0xbb9af7),
                    term_rgb(0x7dcfff),
                    term_rgb(0xa9b1d6),
                    term_rgb(0x414868),
                    term_rgb(0xf7768e),
                    term_rgb(0x9ece6a),
                    term_rgb(0xe0af68),
                    term_rgb(0x7aa2f7),
                    term_rgb(0xbb9af7),
                    term_rgb(0x7dcfff),
                    term_rgb(0xc0caf5),
                ],
                foreground: term_rgb(0xc0caf5),
                background: term_rgb(0x16161e),
                cursor: term_rgb(0xc0caf5),
                selection: term_rgb(0x283457),
                scrollbar: term_rgb(0x3b4261),
            },
            border: c(rgb(0x292e42)),
            divider: c(rgb(0x1f2335)),
            label: c(rgb(0x737aa2)),
            tab: c(rgb(0x292e42)),
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
    /// Tokyo Night: ink-blue grounds, and the palette's blue, orange,
    /// purple, cyan and green each given a role.
    pub fn tokyo_night() -> Self {
        Self {
            bg: c(rgb(0x1a1b26)),
            panel: c(rgb(0x16161e)),
            card: c(rgb(0x16161e)),
            raised: c(rgb(0x292e42)),
            selected: c(rgb(0x292e42)),
            border: c(rgb(0x292e42)),
            border_strong: c(rgb(0x3b4261)),
            rule: c(rgb(0xc0caf5)),
            hairline: c(rgb(0x292e42)),
            offset: c(rgb(0xf7768e)),
            text: c(rgb(0xc0caf5)),
            bright: c(rgb(0xe6eaff)),
            text_soft: c(rgb(0xa9b1d6)),
            muted: c(rgb(0x9aa5ce)),
            dim: c(rgb(0x737aa2)),
            accent: c(rgb(0x7aa2f7)),
            accent_soft: c(rgba(0x7aa2f71f)),
            accent_border: c(rgb(0x3d59a1)),
            blue: c(rgb(0x7aa2f7)),
            blue_soft: c(rgba(0x7aa2f70d)),
            blue_border: c(rgb(0x3b4261)),
            green: c(rgb(0x9ece6a)),
            green_soft: c(rgba(0x9ece6a1f)),
            red: c(rgb(0xf7768e)),
            red_soft: c(rgba(0xf7768e14)),
            red_border: c(rgb(0x5c2f3c)),
            added_text: c(rgb(0xb9e08f)),
            removed_text: c(rgb(0xf9a3b4)),
            scrim: c(rgba(0x0b0b1299)),
            backdrop: c(rgb(0x101018)),
            border_soft: c(rgb(0x292e42)),
            slate: c(rgb(0x565f89)),
            bar_idle: c(rgb(0x3b4261)),
            change: c(rgb(0xbb9af7)),
            info_surface: c(rgb(0x1f2335)),
            info_panel: c(rgb(0x1f2335)),
            danger_surface: c(rgb(0x221a26)),
            danger_edge: c(rgb(0x5c2f3c)),
            marks: [
                c(rgb(0x7aa2f7)),
                c(rgb(0xbb9af7)),
                c(rgb(0xff9e64)),
                c(rgb(0x73daca)),
                c(rgb(0xe0af68)),
                c(rgb(0x7dcfff)),
            ],
            term: TermLook::tokyo_night(),
            syntax: SyntaxLook::tokyo_night(),
            roles: Roles::tokyo_night(),
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

    /// The hard offset behind something outlined in `color`, such as a
    /// slip: the same hue, much darker.
    pub fn under(&self, color: Hsla) -> Hsla {
        Hsla {
            h: color.h,
            s: color.s * 0.7,
            l: color.l * 0.32,
            a: color.a,
        }
    }

    /// The color a tone is written in.
    pub fn tone(&self, tone: Tone) -> Hsla {
        match tone {
            Tone::Info => self.blue,
            Tone::Warn => self.roles.live,
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

/// `steps` steps of spacing: `sp(2.)` is half a rem, 8 px at the
/// default rem size. Spacing is in rems, so it grows with the zoom.
pub const fn sp(steps: f32) -> Rems {
    rems(UNIT * steps / 16.)
}

/// The face a text style is set in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Face {
    /// The face of what holds the text: [`SANS`], unless set otherwise.
    Inherited,
    /// [`MONO`]: code, paths, ids, counts and times.
    Mono,
    /// [`DISPLAY`]: titles and big figures.
    Display,
}

/// A text style: size, and optionally weight, line height and face.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Type {
    pub size: f32,
    pub weight: Option<FontWeight>,
    /// A multiple of the size.
    pub line_height: Option<f32>,
    pub face: Face,
}

impl Type {
    const fn new(size: f32) -> Self {
        Self {
            size,
            weight: None,
            line_height: None,
            face: Face::Inherited,
        }
    }

    const fn weight(mut self, weight: FontWeight) -> Self {
        self.weight = Some(weight);
        self
    }

    const fn monospace(mut self) -> Self {
        self.face = Face::Mono;
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
    /// A section's title, over its heavy rule, in the display face.
    pub const SECTION: Self = Self::new(20.).display().leading(1.2);
    /// A page's or a chat's title, in the display face.
    pub const PAGE_TITLE: Self = Self::new(22.).display().leading(1.2);
    /// A big number or time, in the display face.
    pub const FIGURE: Self = Self::new(30.).display().leading(1.);

    /// This style at another size: for text that scales with what holds
    /// it, as the wordmark does.
    pub const fn sized(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    /// This style in the mono face.
    pub const fn mono(self) -> Self {
        self.monospace()
    }

    /// This style in the display face, which has one weight.
    pub const fn display(mut self) -> Self {
        self.face = Face::Display;
        self
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

/// Corner radii. The look is square: every corner a surface or a
/// control names is 0. Only a selected row is eased a little, and marks
/// that are dots stay round.
pub mod radius {
    use gpui::{Rems, rems};

    /// Thin bars and meters.
    pub const HAIRLINE: Rems = rems(0.);
    /// The tops of chart bars.
    pub const BAR: Rems = rems(0.);
    /// Checkboxes, small chips.
    pub const SMALL: Rems = rems(0.);
    /// Icon badges and step buttons.
    pub const TAG: Rems = rems(0.);
    /// Buttons, inline chips, list rows.
    pub const CONTROL: Rems = rems(0.);
    /// Fields, cards, boxes.
    pub const BOX: Rems = rems(0.);
    /// Transcript bubbles and grouped rows.
    pub const LARGE: Rems = rems(0.);
    /// Panels on task screens.
    pub const CARD: Rems = rems(0.);
    /// The phone's run cards.
    pub const TILE: Rems = rems(0.);
    pub const BUBBLE: Rems = rems(0.);
    /// The tops of bottom sheets.
    pub const SHEET: Rems = rems(0.);
    /// The phone preview's frame.
    pub const DEVICE: Rems = rems(0.);
    /// The selected row of the sidebar, and a repository's mark: 3 px.
    pub const SELECTED: Rems = rems(0.1875);
    /// Fully round, for dots and rings.
    pub const FULL: Rems = rems(624.9375);
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
    use gpui::{Rems, rems};

    /// Buttons in headers and lists.
    pub const SMALL: Rems = rems(1.875);
    /// Rows you can tap, and small fields.
    pub const MEDIUM: Rems = rems(2.25);
    /// Big buttons and fields; the smallest touch target on a phone.
    pub const LARGE: Rems = rems(2.75);
}

/// Setting design values on any element.
pub trait Design: Styled + Sized {
    /// Sets the text's size, and its weight, line height and face when
    /// the style has them.
    fn typeset(mut self, style: Type) -> Self {
        self = self.text_size(rems(style.size / 16.));
        if let Some(weight) = style.weight {
            self = self.font_weight(weight);
        }
        if let Some(line_height) = style.line_height {
            self = self.line_height(relative(line_height));
        }
        match style.face {
            Face::Inherited => self,
            Face::Mono => self.font_family(MONO),
            Face::Display => self.font_family(DISPLAY),
        }
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
            assert_eq!(code.face, Face::Mono);
        }
        assert_eq!(Type::BODY.mono().face, Face::Mono);
        assert_eq!(Type::BODY.face, Face::Inherited);
    }

    #[test]
    fn titles_and_figures_are_in_the_display_face() {
        for style in [Type::SECTION, Type::PAGE_TITLE, Type::FIGURE] {
            assert_eq!(style.face, Face::Display);
            assert_eq!(style.weight, None);
        }
        const { assert!(Type::SECTION.size < Type::PAGE_TITLE.size) };
        const { assert!(Type::PAGE_TITLE.size < Type::FIGURE.size) };
    }

    #[test]
    fn the_look_is_square_and_flat() {
        for corner in [
            radius::HAIRLINE,
            radius::BAR,
            radius::SMALL,
            radius::TAG,
            radius::CONTROL,
            radius::BOX,
            radius::LARGE,
            radius::CARD,
            radius::TILE,
            radius::BUBBLE,
            radius::SHEET,
            radius::DEVICE,
        ] {
            assert_eq!(corner, rems(0.));
        }
        assert_eq!(radius::SELECTED, rems(0.1875));
        let t = Theme::tokyo_night();
        assert_eq!(t.rule, c(rgb(0xc0caf5)));
        assert_eq!(t.hairline, c(rgb(0x292e42)));
        assert_eq!(t.offset, c(rgb(0xf7768e)));
        // A slip's offset is its outline's hue, much darker.
        let under = t.under(t.roles.memory);
        assert_eq!(under.h, t.roles.memory.h);
        assert!(under.l < t.roles.memory.l / 2.);
    }
}
