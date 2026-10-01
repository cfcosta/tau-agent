//! A terminal for tool output, on libghostty-vt
//! (`docs/decisions/0010-terminal-rendering.md`).
//!
//! - [`Terminal`] wraps a libghostty-vt terminal: feed it bytes with
//!   [`Terminal::write`], read its plain text with [`Terminal::text`]
//!   (what a model sees), a replayable VT rendering with
//!   [`Terminal::vt`], and a styled [`Screen`] with
//!   [`Terminal::snapshot`] (what a renderer draws).
//! - [`Command`] runs a program under a pseudo-terminal of a fixed size
//!   and streams what it writes, in order, as [`Event`]s, with a
//!   [`Terminal`] fed on its own thread for the plain text.
//! - [`view::TerminalView`] draws a terminal with GPUI: a drop-in view
//!   you feed bytes while a program runs and freeze when it ends, with
//!   scrollback, a scrollbar, selection and copy. [`Palette`] is the
//!   colors it draws in.
//!
//! The crate depends on no other tau crate.
//!
//! # Threads
//!
//! libghostty-vt is not thread-safe, so [`Terminal`] is neither `Send`
//! nor `Sync`: create it on the thread that uses it and keep it there.
//! [`Screen`] and everything [`Command`] produces are plain data and
//! cross threads freely. [`Command`]'s runner keeps its own
//! [`Terminal`] on a thread of its own.

mod error;
pub mod glyph;
pub mod layout;
mod palette;
mod run;
mod screen;
mod scroll;
mod selection;
mod terminal;
pub mod view;

// The -sys crate is a direct dependency only to turn on its
// `pkg-config` feature: the library comes from the Nix flake, never
// from git and Zig at build time.
use libghostty_vt_sys as _;

pub use crate::{
    error::Error,
    palette::Palette,
    run::{
        Command,
        DEFAULT_IDLE,
        DEFAULT_PREVIEW_INTERVAL,
        DEFAULT_REPLAY_CAP,
        Event,
        Finished,
        Killer,
        Replay,
        Run,
    },
    screen::{
        Color,
        Cursor,
        CursorShape,
        Line,
        Rgb,
        Screen,
        Style,
        TextRun,
        Underline,
    },
    scroll::{ScrollTo, Scroller},
    selection::{Point, Selection, text_of},
    terminal::{Options, Scroll, Size, Terminal},
    view::{TerminalEvent, TerminalView, ViewOptions},
};
