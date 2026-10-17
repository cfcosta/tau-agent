//! Memory's step in first-run setup (`points::SETUP`, ADR 0044): shown
//! only when Claude Code or Codex is on this computer. For each tool,
//! what it saved and its chats, both ticked; where its folders go among
//! the repositories just picked, and what goes to the person's notes;
//! whether to bring folders tau does not have yet; and the button that
//! starts the import and ends the step. The import goes on after setup.

use gpui::{AnyElement, SharedString, div, prelude::*, rems};
use tau_ui_kit::{
    components::{self as ui, ButtonKind},
    theme::{Design as _, Theme, Type, radius, sp, weight},
};
use tau_ui_plugin::{
    ViewCx,
    points::{AtSetup, SetupRepo, SetupView},
};

use super::MemoryUi;
use crate::import::{Act, Material, Parts, Place, Request, Tool, ToolFound};

/// The step's name among memory's.
pub const STEP: &str = "memories";

/// What the person chose so far, kept per window: every part, and not
/// the folders tau does not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Choice {
    pub parts: Parts,
    pub others: bool,
}

/// The place a picked repository is.
fn place_of(repo: &SetupRepo) -> Place {
    Place {
        folder: repo.folder.as_ref().map(|f| f.display().to_string()),
        remote: repo.remote.clone(),
    }
}

/// The repository's short name: `hello` of `octocat/hello`.
fn short(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The step, when either tool is on this computer.
pub fn step(
    at: &AtSetup,
    view: &mut ViewCx<'_, MemoryUi>,
) -> Option<SetupView> {
    let imports = view.data.imports.clone();
    if !imports.any_found() {
        return None;
    }
    let t = view.theme().clone();
    let choice = view.read_ui().choice;
    let names: Vec<&str> =
        imports.found.iter().map(|f| f.tool.label()).collect();
    let on = match names.as_slice() {
        [one] => format!("{one} is"),
        _ => format!("{} are", names.join(" and ")),
    };
    let cards = div().w_full().flex().flex_wrap().gap(sp(5.)).children(
        imports
            .found
            .iter()
            .map(|found| card(found, &at.repos, choice, view, &t)),
    );
    let others = {
        // Weak: setup's motion keeps what it drew a while.
        let ui = view.ui.downgrade();
        let handle = view.handle.clone();
        div()
            .id("memory-setup-others")
            .debug_selector(|| "memory-setup-others".to_owned())
            .flex()
            .items_center()
            .gap(sp(2.5))
            .cursor_pointer()
            .typeset(Type::SMALL)
            .text_color(t.muted)
            .child(ui::checkbox(choice.others, false, &t))
            .child("Also bring folders tau does not have yet, to use when you add them")
            .on_click(move |_, _, cx| {
                let _ = ui.update(cx, |ui, cx| {
                    ui.choice.others = !ui.choice.others;
                    cx.notify();
                });
                handle.refresh(cx);
            })
    };
    let start = {
        let handle = view.handle.clone();
        let only: Vec<Place> = at.repos.iter().map(place_of).collect();
        div()
            .id("memory-setup-start")
            .debug_selector(|| "memory-setup-start".to_owned())
            .child(ui::big_button(
                "Read and show me the notes",
                None,
                if choice.parts.any() {
                    ButtonKind::Primary
                } else {
                    ButtonKind::Secondary
                },
                &t,
            ))
            .on_click(move |_, _, cx| {
                if choice.parts.any() {
                    handle.act(
                        Act::Import(Request {
                            parts: choice.parts,
                            only: only.clone(),
                            others: choice.others,
                        }),
                        cx,
                    );
                }
                handle.setup_done(cx);
            })
    };
    let body = div()
        .w_full()
        .flex()
        .flex_col()
        .gap(sp(5.))
        .child(cards)
        .child(
            div()
                .w_full()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(sp(3.5))
                .child(others)
                .child(
                    div()
                        .ml_auto()
                        .flex()
                        .items_center()
                        .gap(sp(3.))
                        .child(ui::text(
                            "Only chats in the folders above",
                            Type::CAPTION,
                            t.muted,
                        ))
                        .child(start),
                ),
        )
        .child(ui::text(
            "Read only: tau never changes Claude Code's or Codex's files. What \
             they wrote is treated as notes to keep, never as instructions.",
            Type::CAPTION,
            t.muted,
        ))
        .into_any_element();
    Some(SetupView {
        step: STEP,
        title: "Memories".into(),
        heading: "Bring what your other agents learned".into(),
        detail: format!(
            "{on} on this computer. tau can turn what they remember into its \
             own notes, for the repositories you picked and about you. You see \
             every note before it is kept."
        ),
        body,
    })
}

/// One tool: what it has, what to read of it, and where it goes.
fn card(
    found: &ToolFound,
    picked: &[SetupRepo],
    choice: Choice,
    view: &ViewCx<'_, MemoryUi>,
    t: &Theme,
) -> AnyElement {
    let tool = found.tool;
    let folders = found.folders.iter().filter(|f| f.saved > 0).count();
    let (saved, saved_detail, chats, chats_detail) = match tool {
        Tool::ClaudeCode => (
            format!("Its memory · {}", plural(found.saved, "note", "notes")),
            format!(
                "Notes it saved in {}. Copied as they are, without the model.",
                plural(folders, "folder", "folders")
            ),
            format!(
                "Its chats · {}",
                plural(found.chats, "conversation", "conversations")
            ),
            "Your model reads them and writes down what was decided, fixed or \
             preferred."
                .to_owned(),
        ),
        Tool::Codex => (
            format!(
                "Its memories · {}",
                plural(found.saved, "summary", "summaries")
            ),
            "What Codex kept from past tasks, grouped by folder. Your model \
             splits them into notes."
                .to_owned(),
            format!(
                "Its chats · {}",
                plural(found.chats, "session", "sessions")
            ),
            "Read in full by your model. It keeps going after setup."
                .to_owned(),
        ),
    };
    let what = |material: Material, title: String, detail: String| {
        let ui = view.ui.downgrade();
        let handle = view.handle.clone();
        let on = choice.parts.reads(tool, material);
        let id = format!(
            "memory-setup-{}-{}",
            match tool {
                Tool::ClaudeCode => "claude",
                Tool::Codex => "codex",
            },
            match material {
                Material::Saved => "saved",
                Material::Chat => "chats",
            }
        );
        let selector = id.clone();
        div()
            .id(SharedString::from(id))
            .debug_selector(move || selector.clone())
            .flex()
            .items_start()
            .gap(sp(3.))
            .px(sp(3.5))
            .py(sp(3.))
            .rounded(radius::BOX)
            .bg(t.setup.field)
            .border_1()
            .border_color(t.setup.surface_border)
            .cursor_pointer()
            .child(ui::checkbox(on, false, t))
            .child(
                div()
                    .flex_1()
                    .min_w(rems(0.))
                    .flex()
                    .flex_col()
                    .gap(sp(0.75))
                    .child(
                        div()
                            .text_color(t.text)
                            .font_weight(weight::EMPHASIS)
                            .child(title),
                    )
                    .child(ui::text(detail, Type::CAPTION, t.muted)),
            )
            .on_click(move |_, _, cx| {
                let _ = ui.update(cx, |ui, cx| {
                    ui.choice.parts.set(tool, material, !on);
                    cx.notify();
                });
                handle.refresh(cx);
            })
    };
    // Where its folders go: each picked repository that claims one.
    let unit = match tool {
        Tool::ClaudeCode => ("note", "notes"),
        Tool::Codex => ("summary", "summaries"),
    };
    let mut rows: Vec<(String, String, bool)> = Vec::new();
    let mut claimed = vec![false; found.folders.len()];
    for repo in picked {
        let place = place_of(repo);
        let mut count = 0;
        let mut label = None;
        for (n, folder) in found.folders.iter().enumerate() {
            if folder.place.matches(
                place.folder.as_deref().map(std::path::Path::new),
                place.remote.as_deref(),
            ) {
                claimed[n] = true;
                count += folder.saved;
                label.get_or_insert_with(|| folder.label.clone());
            }
        }
        if let Some(label) = label {
            rows.push((
                label,
                format!(
                    "{} · {}",
                    short(&repo.name),
                    plural(count, unit.0, unit.1)
                ),
                true,
            ));
        }
    }
    if found.about_you > 0 {
        rows.push((
            "about you, in any folder".into(),
            format!("About you · {}", plural(found.about_you, unit.0, unit.1)),
            true,
        ));
    }
    let left = claimed.iter().filter(|claimed| !**claimed).count();
    if left > 0 {
        rows.push((
            plural(left, "other folder", "other folders"),
            if choice.others {
                "kept for when you add them".into()
            } else {
                "left out".into()
            },
            false,
        ));
    }
    let map =
        div()
            .flex()
            .flex_col()
            .gap(sp(1.5))
            .children(rows.into_iter().map(|(from, to, goes)| {
                div()
                    .flex()
                    .items_center()
                    .gap(sp(3.))
                    .typeset(Type::CAPTION)
                    .child(
                        ui::mono(from, Type::CAPTION, t.muted)
                            .flex_1()
                            .min_w(rems(0.))
                            .truncate(),
                    )
                    .child(div().text_color(t.muted).child("→"))
                    .child(
                        div()
                            .flex_1()
                            .min_w(rems(0.))
                            .truncate()
                            .text_color(if goes { t.text } else { t.muted })
                            .child(to),
                    )
            }));
    div()
        .flex_1()
        .min_w(rems(22.))
        .flex()
        .flex_col()
        .gap(sp(4.))
        .p(sp(5.5))
        .rounded(radius::CARD)
        .bg(t.setup.glass)
        .border_1()
        .border_color(t.setup.surface_border)
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(3.))
                .child(ui::repo_mark(
                    // Codex's mark is its X.
                    match tool {
                        Tool::ClaudeCode => "Claude Code",
                        Tool::Codex => "X Codex",
                    },
                    34.,
                    t,
                ))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(sp(0.5))
                        .child(
                            div()
                                .typeset(Type::BODY)
                                .font_weight(weight::STRONG)
                                .text_color(t.text)
                                .child(tool.label()),
                        )
                        .child(ui::mono(tool.root(), Type::CAPTION, t.muted)),
                ),
        )
        .child(what(Material::Saved, saved, saved_detail))
        .child(what(Material::Chat, chats, chats_detail))
        .child(map)
        .into_any_element()
}
