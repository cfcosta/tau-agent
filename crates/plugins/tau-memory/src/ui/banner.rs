//! The Memory tab's ways into imports (ADR 0044): "Import from Claude
//! Code or Codex…" on the notes page and in a repository's menu, and a
//! banner on a repository's notes when the tools know it: proposals
//! wait for it, or there is what they kept that no import read.

use gpui::{AnyElement, div, prelude::*, rems};
use tau_ui_kit::{
    assets::Icon,
    components::{self as ui, ButtonKind, icon},
    theme::{Design as _, IconSize, Theme, Type, radius, sp, weight},
};
use tau_ui_plugin::{Handle, ViewCx, points::AtRepo};

use super::{MemoryUi, review_link};
use crate::import::{Act, Banner, Imports, Request, Tool};

/// What the way into an import says.
pub const IMPORT: &str = "Import from Claude Code or Codex…";

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Starts an import of every listed repository, and opens the review
/// page to watch it.
fn import(handle: &Handle, cx: &mut gpui::App) {
    handle.act(Act::Import(Request::default()), cx);
    handle.navigate(review_link(None), cx);
}

/// The notes page's button, when either tool is on this computer.
pub fn import_button(
    handle: &Handle,
    imports: &Imports,
    t: &Theme,
) -> Option<AnyElement> {
    if !imports.any_found() {
        return None;
    }
    let handle = handle.clone();
    Some(
        div()
            .id("memory-import")
            .debug_selector(|| "memory-import".to_owned())
            .child(ui::button(IMPORT, ButtonKind::Secondary, t))
            .on_click(move |_, _, cx| import(&handle, cx))
            .into_any_element(),
    )
}

/// The repository menu's entry, when either tool is on this computer.
pub fn menu_entry(
    _at: &AtRepo,
    view: &mut ViewCx<'_, MemoryUi>,
) -> Option<AnyElement> {
    if !view.data.imports.any_found() {
        return None;
    }
    let t = view.theme().clone();
    let handle = view.handle.clone();
    let hover = t.selected;
    Some(
        div()
            .id("menu-memory-import")
            .debug_selector(|| "menu-memory-import".to_owned())
            .flex()
            .items_center()
            .gap(sp(2.5))
            .h(rems(2.125))
            .px(sp(2.5))
            .rounded(radius::CONTROL)
            .cursor_pointer()
            .text_color(t.text)
            .hover(move |style| style.bg(hover))
            .child(icon(Icon::Memory, IconSize::BASE, t.muted))
            .child(IMPORT)
            .on_click(move |_, _, cx| import(&handle, cx))
            .into_any_element(),
    )
}

/// What the tools kept for the repository, in words: "Claude Code kept 9
/// notes for ~/work/docbert, and Codex 4 task summaries."
fn kept(banner: &Banner) -> String {
    let parts: Vec<String> = banner
        .saved
        .iter()
        .map(|(tool, n)| match tool {
            Tool::ClaudeCode => plural(*n, "note", "notes"),
            Tool::Codex => plural(*n, "task summary", "task summaries"),
        })
        .collect();
    let tools: Vec<&str> =
        banner.saved.iter().map(|(tool, _)| tool.label()).collect();
    match (tools.as_slice(), parts.as_slice()) {
        ([one], [what]) => format!("{one} kept {what} for {}.", banner.folder),
        ([first, second], [a, b]) => {
            format!("{first} kept {a} for {}, and {second} {b}.", banner.folder)
        }
        _ => format!("Claude Code or Codex worked in {}.", banner.folder),
    }
}

/// The banner on `repo`'s notes, when its tools know it.
pub fn banner(
    handle: &Handle,
    repo: &str,
    imports: &Imports,
    t: &Theme,
) -> Option<AnyElement> {
    let banner = imports.banners.get(repo)?.clone();
    let mut text = kept(&banner);
    if banner.ready > 0 {
        text.push_str(&format!(
            " {} ready to look at.",
            match banner.ready {
                1 => "1 note is".to_owned(),
                n => format!("{n} notes are"),
            }
        ));
    }
    let only = banner.places.clone();
    let primary = if banner.ready > 0 {
        let (handle, repo) = (handle.clone(), repo.to_owned());
        div()
            .id("memory-banner-review")
            .debug_selector(|| "memory-banner-review".to_owned())
            .child(ui::button(
                format!("Review {}", plural(banner.ready, "note", "notes")),
                ButtonKind::Primary,
                t,
            ))
            .on_click(move |_, _, cx| {
                handle.navigate(review_link(Some(&repo)), cx)
            })
            .into_any_element()
    } else {
        let (handle, only) = (handle.clone(), only.clone());
        div()
            .id("memory-banner-bring")
            .debug_selector(|| "memory-banner-bring".to_owned())
            .child(ui::button("Bring what they kept", ButtonKind::Primary, t))
            .on_click(move |_, _, cx| {
                handle.act(
                    Act::Import(Request {
                        only: only.clone(),
                        ..Request::default()
                    }),
                    cx,
                );
                handle.navigate(review_link(None), cx);
            })
            .into_any_element()
    };
    let chats = (banner.unread_chats > 0 && banner.ready > 0).then(|| {
        let (handle, only) = (handle.clone(), only.clone());
        div()
            .id("memory-banner-chats")
            .debug_selector(|| "memory-banner-chats".to_owned())
            .child(ui::button(
                format!(
                    "Read its {} too",
                    plural(banner.unread_chats, "chat", "chats")
                ),
                ButtonKind::Secondary,
                t,
            ))
            .on_click(move |_, _, cx| {
                handle.act(
                    Act::Import(Request {
                        only: only.clone(),
                        ..Request::default()
                    }),
                    cx,
                )
            })
    });
    let later = {
        let (handle, repo) = (handle.clone(), repo.to_owned());
        div()
            .id("memory-banner-later")
            .debug_selector(|| "memory-banner-later".to_owned())
            .cursor_pointer()
            .px(sp(3.))
            .typeset(Type::CAPTION)
            .text_color(t.muted)
            .child("Not now")
            .on_click(move |_, _, cx| {
                handle.act(Act::NotNow { repo: repo.clone() }, cx)
            })
    };
    Some(
        div()
            .debug_selector(|| "memory-banner".to_owned())
            .flex()
            .flex_col()
            .gap(sp(3.5))
            .px(sp(5.5))
            .py(sp(5.))
            .rounded(radius::CARD)
            .bg(t.panel)
            .border_1()
            .border_color(t.accent_border)
            .child(
                div()
                    .typeset(Type::SUBTITLE)
                    .font_weight(weight::STRONG)
                    .child("Claude Code and Codex know this repository"),
            )
            .child(ui::text(text, Type::SMALL, t.muted))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(sp(2.5))
                    .child(primary)
                    .children(chats)
                    .child(later),
            )
            .into_any_element(),
    )
}
