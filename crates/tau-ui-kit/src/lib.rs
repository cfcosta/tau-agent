//! tau's design language, shared by `tau-ui` and every plugin's UI (ADR
//! 0017): the theme's tokens, the bundled icons and fonts, the shared
//! components, the text field, and marked-up text.
//!
//! Screens take their look from here and never write their own sizes or
//! colors; [`design::check`] keeps each crate that way.

pub mod assets;
pub mod components;
pub mod design;
pub mod diff;
pub mod format;
pub mod input;
pub mod markdown;
pub mod prose;
pub mod theme;

use gpui::App;

/// Sets up what the design language needs once per app: the fonts, the
/// theme and the text field's keys.
pub fn init(cx: &mut App) {
    if let Err(error) = assets::load_fonts(cx) {
        eprintln!("tau-ui-kit: could not load the bundled fonts: {error}");
    }
    cx.set_global(theme::Theme::graphite());
    input::bind_keys(cx);
}
