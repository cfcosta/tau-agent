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
    Lock,
    Repo,
    PullRequest,
    Key,
    Chat,
    Folder,
    Copy,
    Arrow,
    Info,
    Down,
    Close,
    Target,
    Pause,
    Pencil,
    Camera,
    Offline,
    Phone,
    /// Opens a page outside tau.
    External,
    /// Lands a run: on its parent, or into main.
    Land,
    /// Pushes main to GitHub.
    Push,
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
            Self::Lock => "icons/lock.svg",
            Self::Repo => "icons/repo.svg",
            Self::PullRequest => "icons/pull-request.svg",
            Self::Key => "icons/key.svg",
            Self::Chat => "icons/chat.svg",
            Self::Folder => "icons/folder.svg",
            Self::Copy => "icons/copy.svg",
            Self::Arrow => "icons/arrow.svg",
            Self::Info => "icons/info.svg",
            Self::Down => "icons/down.svg",
            Self::Close => "icons/close.svg",
            Self::Target => "icons/target.svg",
            Self::Pause => "icons/pause.svg",
            Self::Pencil => "icons/pencil.svg",
            Self::Camera => "icons/camera.svg",
            Self::Offline => "icons/offline.svg",
            Self::Phone => "icons/phone.svg",
            Self::External => "icons/external.svg",
            Self::Land => "icons/land.svg",
            Self::Push => "icons/push.svg",
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
            Self::Lock => {
                r#"<rect x="5" y="11" width="14" height="10" rx="2"/><path d="M8 11V7a4 4 0 018 0v4"/>"#
            }
            Self::Repo => {
                r#"<path d="M5 4h11a3 3 0 013 3v13H8a3 3 0 01-3-3z"/><path d="M5 17a3 3 0 013-3h11"/>"#
            }
            Self::PullRequest => {
                r#"<circle cx="6" cy="6" r="2"/><circle cx="6" cy="18" r="2"/><circle cx="18" cy="18" r="2"/><path d="M6 8v8M18 16V9a3 3 0 00-3-3h-4M13 3l-3 3 3 3"/>"#
            }
            Self::Key => {
                r#"<circle cx="8" cy="15" r="4"/><path d="M11 12l9-9M17 6l3 3"/>"#
            }
            Self::Chat => r#"<path d="M4 5h16v11H9l-5 4z"/>"#,
            Self::Folder => {
                r#"<path d="M3 7a2 2 0 012-2h4l2 2h8a2 2 0 012 2v9a2 2 0 01-2 2H5a2 2 0 01-2-2z"/>"#
            }
            Self::Copy => {
                r#"<rect x="9" y="9" width="11" height="11" rx="2"/><path d="M5 15V5a2 2 0 012-2h10"/>"#
            }
            Self::Arrow => r#"<path d="M5 12h14M13 6l6 6-6 6"/>"#,
            Self::Down => r#"<path d="M6 9l6 6 6-6"/>"#,
            Self::Close => r#"<path d="M6 6l12 12M18 6L6 18"/>"#,
            Self::Target => {
                r#"<circle cx="12" cy="12" r="9"/><circle cx="12" cy="12" r="5"/><circle cx="12" cy="12" r="1"/>"#
            }
            Self::Pause => r#"<path d="M9 5v14M15 5v14"/>"#,
            Self::Pencil => r#"<path d="M4 20h4L19 9l-4-4L4 16z"/>"#,
            Self::Camera => {
                r#"<path d="M14.5 4h-5L7 7H4a2 2 0 00-2 2v9a2 2 0 002 2h16a2 2 0 002-2V9a2 2 0 00-2-2h-3z"/><circle cx="12" cy="13" r="3"/>"#
            }
            Self::Phone => {
                r#"<rect x="6" y="2" width="12" height="20" rx="2.5"/><path d="M11 18h2"/>"#
            }
            Self::Offline => {
                r#"<path d="M2 2l20 20M8.5 16.5a5 5 0 017 0M5 12.9a10 10 0 015.2-2.8M19 12.9a10 10 0 00-2.3-1.6M12 20h.01"/>"#
            }
            Self::Land => r#"<path d="M12 20V8M7 12l5-5 5 5M5 4h14"/>"#,
            Self::Push => r#"<path d="M12 19V5M6 11l6-6 6 6"/>"#,
            Self::External => {
                r#"<path d="M14 4h6v6M20 4l-8.5 8.5M18 14.5V20H4V6h5.5"/>"#
            }
            Self::Info => {
                r#"<circle cx="12" cy="12" r="9"/><path d="M12 8h.01M11 12h1v5h1"/>"#
            }
        }
    }

    const ALL: [Self; 39] = [
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
        Self::Lock,
        Self::Repo,
        Self::PullRequest,
        Self::Key,
        Self::Chat,
        Self::Folder,
        Self::Copy,
        Self::Arrow,
        Self::Info,
        Self::Down,
        Self::Close,
        Self::Target,
        Self::Pause,
        Self::Pencil,
        Self::Camera,
        Self::Offline,
        Self::Phone,
        Self::External,
        Self::Land,
        Self::Push,
    ];

    fn svg(self) -> String {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">{}</svg>"#,
            self.body()
        )
    }
}

mod brand {
    include!(concat!(env!("OUT_DIR"), "/brand.rs"));
}

/// Another company's mark, shown where tau signs in to it. The files are
/// their owners' approved artwork, dropped into `assets/brand/` and
/// embedded at build time (see `build.rs`); tau never draws its own
/// version. Without the file, [`Brand::svg`] is `None` and a neutral
/// placeholder stands in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Brand {
    ChatGpt,
    GitHub,
}

impl Brand {
    const ALL: [Self; 2] = [Self::ChatGpt, Self::GitHub];

    pub fn path(self) -> &'static str {
        match self {
            Self::ChatGpt => "brand/chatgpt-mark.svg",
            Self::GitHub => "brand/github-mark.svg",
        }
    }

    /// The mark's SVG, when its file was there at build time.
    pub fn svg(self) -> Option<&'static [u8]> {
        match self {
            Self::ChatGpt => brand::CHATGPT_MARK,
            Self::GitHub => brand::GITHUB_MARK,
        }
    }
}

/// Serves the icons, and the brand marks that are present, to GPUI's
/// `svg()` element.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(mark) = Brand::ALL.into_iter().find(|b| b.path() == path) {
            return Ok(mark.svg().map(Cow::Borrowed));
        }
        Ok(Icon::ALL
            .into_iter()
            .find(|icon| icon.path() == path)
            .map(|icon| Cow::Owned(icon.svg().into_bytes())))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let marks = Brand::ALL
            .into_iter()
            .filter(|mark| mark.svg().is_some())
            .map(Brand::path);
        Ok(Icon::ALL
            .into_iter()
            .map(Icon::path)
            .chain(marks)
            .filter(|icon| icon.starts_with(path))
            .map(SharedString::from)
            .collect())
    }
}

/// Geist and Geist Mono (SIL Open Font License, `assets/fonts/OFL.txt`),
/// and Newsreader for reading notes (`assets/fonts/OFL-Newsreader.txt`).
const FONTS: [&[u8]; 10] = [
    include_bytes!("../assets/fonts/Geist-Regular.ttf"),
    include_bytes!("../assets/fonts/Geist-Medium.ttf"),
    include_bytes!("../assets/fonts/Geist-SemiBold.ttf"),
    // Italics for the emphasis in replies, from the same release (v1.7.2).
    include_bytes!("../assets/fonts/Geist-Italic.ttf"),
    include_bytes!("../assets/fonts/Geist-MediumItalic.ttf"),
    include_bytes!("../assets/fonts/Geist-SemiBoldItalic.ttf"),
    include_bytes!("../assets/fonts/GeistMono-Regular.ttf"),
    include_bytes!("../assets/fonts/GeistMono-Medium.ttf"),
    include_bytes!("../assets/fonts/Newsreader-Regular.ttf"),
    include_bytes!("../assets/fonts/Newsreader-Medium.ttf"),
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

    #[test]
    fn brand_marks_are_served_only_when_present() {
        for mark in Brand::ALL {
            let served = Assets.load(mark.path()).unwrap();
            assert_eq!(served.is_some(), mark.svg().is_some(), "{mark:?}");
        }
    }
}
