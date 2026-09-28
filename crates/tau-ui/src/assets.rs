//! Icons and fonts, compiled into the binary so the app needs no files
//! next to it.

use std::borrow::Cow;

use anyhow::Result;
use gpui::{App, AssetSource, SharedString};

/// The icons the interface draws. Each is a 24×24 stroke icon; GPUI
/// paints it in the element's text color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Check,
    Spinner,
    Blocked,
    Warning,
    Plug,
    Chevron,
    Back,
    Panel,
    Send,
    Plus,
    Stop,
    Fork,
    SubAgent,
    Search,
    Settings,
    Runs,
    Memory,
    History,
    Paperclip,
}

impl Icon {
    pub fn path(self) -> &'static str {
        match self {
            Self::Check => "icons/check.svg",
            Self::Spinner => "icons/spinner.svg",
            Self::Blocked => "icons/blocked.svg",
            Self::Warning => "icons/warning.svg",
            Self::Plug => "icons/plug.svg",
            Self::Chevron => "icons/chevron.svg",
            Self::Back => "icons/back.svg",
            Self::Panel => "icons/panel.svg",
            Self::Send => "icons/send.svg",
            Self::Plus => "icons/plus.svg",
            Self::Stop => "icons/stop.svg",
            Self::Fork => "icons/fork.svg",
            Self::SubAgent => "icons/sub-agent.svg",
            Self::Search => "icons/search.svg",
            Self::Settings => "icons/settings.svg",
            Self::Runs => "icons/runs.svg",
            Self::Memory => "icons/memory.svg",
            Self::History => "icons/history.svg",
            Self::Paperclip => "icons/paperclip.svg",
        }
    }

    fn body(self) -> &'static str {
        match self {
            Self::Check => r#"<path d="M5 12l5 5L20 7"/>"#,
            Self::Spinner => r#"<path d="M12 3a9 9 0 109 9"/>"#,
            Self::Blocked => {
                r#"<circle cx="12" cy="12" r="9"/><path d="M5.6 5.6l12.8 12.8"/>"#
            }
            Self::Warning => {
                r#"<path d="M12 3l9 16H3z"/><path d="M12 10v4M12 17h.01"/>"#
            }
            Self::Plug => {
                r#"<path d="M9 2v6M15 2v6M6 8h12v4a6 6 0 01-12 0zM12 18v4"/>"#
            }
            Self::Chevron => r#"<path d="M9 6l6 6-6 6"/>"#,
            Self::Back => r#"<path d="M15 6l-6 6 6 6"/>"#,
            Self::Panel => {
                r#"<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M3 15h18"/>"#
            }
            Self::Send => r#"<path d="M12 19V5M5 12l7-7 7 7"/>"#,
            Self::Plus => r#"<path d="M12 5v14M5 12h14"/>"#,
            Self::Stop => {
                r#"<rect x="6" y="6" width="12" height="12" rx="2" fill="black"/>"#
            }
            Self::Fork => {
                r#"<circle cx="6" cy="5" r="2"/><circle cx="6" cy="19" r="2"/><circle cx="18" cy="8" r="2"/><path d="M6 7v10M18 10c0 4-6 4-12 7"/>"#
            }
            Self::SubAgent => {
                r#"<path d="M6 3v12a3 3 0 003 3h9M15 15l3 3-3 3"/>"#
            }
            Self::Search => {
                r#"<circle cx="11" cy="11" r="7"/><path d="M21 21l-4.3-4.3"/>"#
            }
            Self::Settings => {
                r#"<circle cx="12" cy="12" r="3"/><path d="M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9l2.1 2.1M17 17l2.1 2.1M4.9 19.1L7 17M17 7l2.1-2.1"/>"#
            }
            Self::Runs => r#"<path d="M4 6h16M4 12h16M4 18h10"/>"#,
            Self::Memory => {
                r#"<circle cx="6" cy="6" r="2.5"/><circle cx="18" cy="8" r="2.5"/><circle cx="10" cy="18" r="2.5"/><path d="M8 7l8 1M7 8l2 8M16 10l-4 6"/>"#
            }
            Self::History => {
                r#"<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>"#
            }
            Self::Paperclip => {
                r#"<path d="M21 11l-8.5 8.5a5 5 0 01-7-7L14 4a3.5 3.5 0 015 5l-8.5 8.5a2 2 0 01-3-3L15 7"/>"#
            }
        }
    }

    const ALL: [Self; 19] = [
        Self::Check,
        Self::Spinner,
        Self::Blocked,
        Self::Warning,
        Self::Plug,
        Self::Chevron,
        Self::Back,
        Self::Panel,
        Self::Send,
        Self::Plus,
        Self::Stop,
        Self::Fork,
        Self::SubAgent,
        Self::Search,
        Self::Settings,
        Self::Runs,
        Self::Memory,
        Self::History,
        Self::Paperclip,
    ];

    fn svg(self) -> String {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">{}</svg>"#,
            self.body()
        )
    }
}

/// Serves the icons to GPUI's `svg()` element.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(Icon::ALL
            .into_iter()
            .find(|icon| icon.path() == path)
            .map(|icon| Cow::Owned(icon.svg().into_bytes())))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(Icon::ALL
            .into_iter()
            .map(Icon::path)
            .filter(|icon| icon.starts_with(path))
            .map(SharedString::from)
            .collect())
    }
}

/// Geist and Geist Mono (SIL Open Font License, `assets/fonts/OFL.txt`).
const FONTS: [&[u8]; 5] = [
    include_bytes!("../assets/fonts/Geist-Regular.ttf"),
    include_bytes!("../assets/fonts/Geist-Medium.ttf"),
    include_bytes!("../assets/fonts/Geist-SemiBold.ttf"),
    include_bytes!("../assets/fonts/GeistMono-Regular.ttf"),
    include_bytes!("../assets/fonts/GeistMono-Medium.ttf"),
];

pub fn load_fonts(cx: &App) -> Result<()> {
    cx.text_system()
        .add_fonts(FONTS.into_iter().map(Cow::Borrowed).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_served_as_svg() {
        for icon in Icon::ALL {
            let bytes = Assets.load(icon.path()).unwrap().expect("icon");
            let svg = std::str::from_utf8(&bytes).unwrap();
            assert!(svg.starts_with("<svg"), "{icon:?}");
        }
        assert!(Assets.load("icons/missing.svg").unwrap().is_none());
    }
}
