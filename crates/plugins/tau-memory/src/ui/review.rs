//! The review page of what imports propose (ADR 0044): how many notes
//! wait and where they came from, the import going on, tabs for every
//! proposal, the person's, each repository's and those that need a
//! look, and a row per proposal to tick or untick. "Keep N notes" keeps
//! the ticked ones in view, "Keep none" drops those in view. New
//! proposals join as the import goes. It draws from memory's catalog
//! data and asks its host half, so it works on a phone too.

use gpui::{AnyElement, Entity, Hsla, SharedString, div, prelude::*, rems};
use tau_ui_kit::{
    assets::Icon,
    components::{self as ui, ButtonKind, Material as _, icon},
    input::TextInput,
    theme::{Design as _, IconSize, Theme, Tone, Type, radius, sp, weight},
};
use tau_ui_plugin::{Handle, Link, ViewCx};

use super::{MemoryUi, USER, page::kind_color, review_link};
use crate::import::{Act, Imports, Kept, Material, Pending, Request, Tool};

/// The widest the page's column gets.
const COLUMN: f32 = 70.;

/// The tab a page shows: every proposal, the person's, a repository's,
/// or those that need a look.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Tab {
    All,
    You,
    Repo(String),
    Look,
}

impl Tab {
    fn of(view: &ViewCx<'_, MemoryUi>) -> Self {
        if view.param("look").is_some() {
            return Self::Look;
        }
        match view.param("repo") {
            None | Some("" | "all") => Self::All,
            Some(USER) => Self::You,
            Some(repo) => Self::Repo(repo.to_owned()),
        }
    }

    fn holds(&self, pending: &Pending) -> bool {
        match self {
            Self::All => true,
            Self::You => pending.repo.is_none(),
            Self::Repo(repo) => pending.repo.as_deref() == Some(repo),
            Self::Look => pending.flags.need_a_look(),
        }
    }

    fn link(&self) -> Link {
        match self {
            Self::All => review_link(None),
            Self::You => review_link(Some(USER)),
            Self::Repo(repo) => review_link(Some(repo)),
            Self::Look => review_link(None).param("look", "1"),
        }
    }

    fn key(&self) -> String {
        match self {
            Self::All => "all".into(),
            Self::You => "you".into(),
            Self::Repo(repo) => format!("repo-{repo}"),
            Self::Look => "look".into(),
        }
    }
}

/// The page's title.
pub fn title(_view: &mut ViewCx<'_, MemoryUi>) -> String {
    "Notes to keep".to_owned()
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Where the proposals came from, and what was left out or merged: "From
/// 501 Claude Code notes and 17 Codex summaries: 380 were about folders
/// you left out, 15 said the same as another."
pub fn summary(imports: &Imports) -> Option<String> {
    let tally = &imports.tally;
    let mut from = Vec::new();
    let read = |tool, material| tally.read_of(tool, material);
    let claude = read(Tool::ClaudeCode, Material::Saved);
    if claude > 0 {
        from.push(format!(
            "{} Claude Code {}",
            claude,
            if claude == 1 { "note" } else { "notes" }
        ));
    }
    let codex = read(Tool::Codex, Material::Saved);
    if codex > 0 {
        from.push(format!(
            "{} Codex {}",
            codex,
            if codex == 1 { "summary" } else { "summaries" }
        ));
    }
    let chats = read(Tool::ClaudeCode, Material::Chat)
        + read(Tool::Codex, Material::Chat);
    if chats > 0 {
        from.push(plural(chats, "chat", "chats"));
    }
    if from.is_empty() {
        return None;
    }
    let from = match from.as_slice() {
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
        [] => unreachable!("not empty"),
    };
    let mut then = Vec::new();
    if tally.left_out > 0 {
        then.push(format!(
            "{} about folders you left out",
            if tally.left_out == 1 {
                "1 was".to_owned()
            } else {
                format!("{} were", tally.left_out)
            }
        ));
    }
    if tally.merged > 0 {
        then.push(format!(
            "{} the same as another",
            if tally.merged == 1 {
                "1 said".to_owned()
            } else {
                format!("{} said", tally.merged)
            }
        ));
    }
    if tally.known > 0 {
        then.push(format!(
            "{} what a note says already",
            if tally.known == 1 {
                "1 said".to_owned()
            } else {
                format!("{} said", tally.known)
            }
        ));
    }
    if tally.refused > 0 {
        then.push(format!(
            "{} refused as instructions to the model",
            if tally.refused == 1 {
                "1 was".to_owned()
            } else {
                format!("{} were", tally.refused)
            }
        ));
    }
    Some(if then.is_empty() {
        format!("From {from}.")
    } else {
        format!("From {from}: {}.", then.join(", "))
    })
}

/// The page.
pub fn render(view: &mut ViewCx<'_, MemoryUi>) -> AnyElement {
    let t = view.theme().clone();
    let compact = view.compact;
    let imports = view.data.imports.clone();
    let tab = Tab::of(view);
    let handle = view.handle.clone();
    let search = view.read_ui().review_search.clone();
    let query = search.read(view.cx).text().to_lowercase();
    let kept = view.read_ui().kept.clone();
    let shown: Vec<&Pending> = imports
        .pending
        .iter()
        .filter(|pending| tab.holds(pending))
        .filter(|pending| {
            query.is_empty()
                || pending.title.to_lowercase().contains(&query)
                || pending.description.to_lowercase().contains(&query)
        })
        .collect();
    let content = div()
        .w_full()
        .max_w(rems(COLUMN))
        .flex()
        .flex_col()
        .gap(sp(5.))
        .child(head(&handle, &imports, compact, &t))
        .child(tabs(&handle, &imports, &tab, &search, &t))
        .children(kept.map(|kept| ui::text(said(&kept), Type::SMALL, t.muted)))
        .child(list(&handle, &shown, &imports, compact, &t))
        .children((!shown.is_empty()).then(|| foot(&handle, &shown, &t)))
        .children((!imports.failed.is_empty()).then(|| failed(&imports, &t)));
    ui::screen(
        "memory-review",
        compact,
        div().flex().justify_center().child(content),
    )
    .into_any_element()
}

/// What keeping did.
fn said(kept: &Kept) -> String {
    let mut line = format!("Kept {}.", plural(kept.kept, "note", "notes"));
    for (id, why) in &kept.refused {
        line.push_str(&format!(" {id} was not kept: {why}."));
    }
    line
}

/// How many wait, where they came from, and the import going on.
fn head(
    handle: &Handle,
    imports: &Imports,
    compact: bool,
    t: &Theme,
) -> impl IntoElement {
    let waiting = imports.pending.len();
    let title = div()
        .flex_1()
        .min_w(rems(0.))
        .flex()
        .flex_col()
        .gap(sp(1.5))
        .child(ui::mono("memory · import", Type::CAPTION, t.muted))
        .child(
            div()
                .typeset(Type::TITLE)
                .font_weight(weight::STRONG)
                .child(match waiting {
                    0 => "No notes to keep".to_owned(),
                    n => format!("{} to keep", plural(n, "note", "notes")),
                }),
        )
        .children(
            summary(imports).map(|line| ui::text(line, Type::SMALL, t.muted)),
        );
    let side: AnyElement = match &imports.progress {
        Some(progress) => {
            let stop = {
                let handle = handle.clone();
                div()
                    .id("memory-review-stop")
                    .debug_selector(|| "memory-review-stop".to_owned())
                    .cursor_pointer()
                    .text_color(t.roles.link)
                    .child("Stop")
                    .on_click(move |_, _, cx| handle.act(Act::Stop, cx))
            };
            div()
                .debug_selector(|| "memory-review-progress".to_owned())
                .flex()
                .flex_col()
                .gap(sp(1.5))
                .min_w(rems(18.75))
                .when(!compact, |side| side.ml_auto())
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .gap(sp(3.))
                        .typeset(Type::CAPTION)
                        .text_color(t.muted)
                        .child(progress.line.clone())
                        .child(format!("{:.0}%", progress.fraction * 100.)),
                )
                .child(ui::bar(progress.fraction, 6., t.accent, t.raised))
                .child(
                    div()
                        .flex()
                        .gap(sp(2.))
                        .typeset(Type::CAPTION)
                        .text_color(t.muted)
                        .child("New notes join this list as they come.")
                        .child(stop),
                )
                .into_any_element()
        }
        None if imports.any_found() => {
            let handle = handle.clone();
            div()
                .id("memory-review-import")
                .debug_selector(|| "memory-review-import".to_owned())
                .when(!compact, |side| side.ml_auto())
                .child(ui::button(
                    "Import from Claude Code or Codex…",
                    ButtonKind::Secondary,
                    t,
                ))
                .on_click(move |_, _, cx| {
                    handle.act(Act::Import(Request::default()), cx)
                })
                .into_any_element()
        }
        None => div().into_any_element(),
    };
    // On a phone, the import going on comes under the count.
    div()
        .flex()
        .gap(sp(4.))
        .when(compact, |head| head.flex_col())
        .when(!compact, |head| head.items_end())
        .child(title)
        .child(side)
}

/// The tabs, with how many each holds, and the search.
fn tabs(
    handle: &Handle,
    imports: &Imports,
    on: &Tab,
    search: &Entity<TextInput>,
    t: &Theme,
) -> impl IntoElement {
    let count =
        |tab: &Tab| imports.pending.iter().filter(|p| tab.holds(p)).count();
    let mut all = vec![Tab::All, Tab::You];
    let mut repos: Vec<String> = imports
        .pending
        .iter()
        .filter_map(|p| p.repo.clone())
        .collect();
    repos.sort();
    repos.dedup();
    all.extend(repos.into_iter().map(Tab::Repo));
    all.push(Tab::Look);
    let warn = t.tone(Tone::Warn);
    let field = div()
        .w(rems(13.75))
        .flex()
        .items_center()
        .gap(sp(2.))
        .h(rems(2.125))
        .px(sp(3.))
        .bg(t.panel)
        .border_1()
        .border_color(t.border_strong)
        .rounded(radius::LARGE)
        .child(icon(Icon::Search, IconSize::COMPACT, t.dim))
        .child(div().flex_1().min_w(rems(0.)).child(search.clone()));
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(sp(2.))
        .children(all.into_iter().filter_map(|tab| {
            let n = count(&tab);
            // A repository's tab, and Need a look, only with something in.
            if n == 0 && matches!(tab, Tab::Repo(_) | Tab::Look) {
                return None;
            }
            let selected = &tab == on;
            let name = match &tab {
                Tab::All => "All".to_owned(),
                Tab::You => "About you".to_owned(),
                Tab::Repo(repo) => repo.clone(),
                Tab::Look => "Need a look".to_owned(),
            };
            let to = tab.link();
            let handle = handle.clone();
            let id = format!("memory-review-tab-{}", tab.key());
            let selector = id.clone();
            let hover = t.raised;
            Some(
                div()
                    .id(SharedString::from(id))
                    .debug_selector(move || selector.clone())
                    .flex()
                    .items_center()
                    .gap(sp(1.5))
                    .px(sp(3.))
                    .py(sp(1.5))
                    .rounded(radius::BOX)
                    .typeset(Type::SMALL)
                    .cursor_pointer()
                    .text_color(if tab == Tab::Look {
                        warn
                    } else if selected {
                        t.text
                    } else {
                        t.text_soft
                    })
                    .when(selected, |chip| chip.key(t))
                    .when(!selected, |chip| {
                        chip.hover(move |style| style.bg(hover))
                    })
                    .child(name)
                    .child(div().text_color(t.dim).child(n.to_string()))
                    .on_click(move |_, _, cx| handle.navigate(to.clone(), cx)),
            )
        }))
        .child(div().ml_auto().child(field))
}

/// The proposals in view, in a card, a section per scope.
fn list(
    handle: &Handle,
    shown: &[&Pending],
    imports: &Imports,
    compact: bool,
    t: &Theme,
) -> AnyElement {
    if shown.is_empty() {
        let text = if imports.pending.is_empty() {
            if imports.progress.is_some() {
                "Nothing yet: proposals show here as they come."
            } else if imports.any_found() {
                "Nothing waits. Import to read what Claude Code and Codex kept."
            } else {
                "Nothing waits, and neither Claude Code nor Codex is on this computer."
            }
        } else {
            "Nothing here."
        };
        return ui::empty(text, t).into_any_element();
    }
    let mut sections: Vec<(Option<String>, Vec<&Pending>)> = Vec::new();
    for pending in shown {
        match sections.iter_mut().find(|(repo, _)| *repo == pending.repo) {
            Some((_, rows)) => rows.push(pending),
            None => sections.push((pending.repo.clone(), vec![pending])),
        }
    }
    // The person's first, then the repositories by name.
    sections
        .sort_by(|a, b| a.0.is_some().cmp(&b.0.is_some()).then(a.0.cmp(&b.0)));
    ui::card(t)
        .children(sections.into_iter().enumerate().flat_map(
            |(n, (repo, rows))| {
                let header = div()
                    .flex()
                    .gap(sp(2.5))
                    .px(sp(4.))
                    .py(sp(2.5))
                    .bg(t.panel)
                    .when(n > 0, |header| header.border_t_1())
                    .border_b_1()
                    .border_color(t.border)
                    .typeset(Type::CAPTION)
                    .text_color(t.muted)
                    .child(
                        div()
                            .text_color(t.text)
                            .font_weight(weight::STRONG)
                            .child(
                                repo.clone()
                                    .unwrap_or_else(|| "About you".into()),
                            ),
                    )
                    .child(rows.len().to_string())
                    .when(repo.is_none(), |header| {
                        header.child("· saved for every repository")
                    })
                    .into_any_element();
                std::iter::once(header).chain(
                    rows.into_iter()
                        .map(|pending| row(handle, pending, compact, t)),
                )
            },
        ))
        .into_any_element()
}

/// The pill a flag shows as.
fn flag(text: String, color: Hsla) -> impl IntoElement {
    div()
        .flex_shrink_0()
        .px(sp(2.))
        .py(sp(0.75))
        .rounded(radius::CONTROL)
        .bg(color.opacity(0.14))
        .text_color(color)
        .typeset(Type::CAPTION)
        .child(text)
}

/// One proposal: its box, title, type and description, its flags, and
/// where it came from.
/// On a phone the flags go under the text, which has the row's width.
fn row(
    handle: &Handle,
    pending: &Pending,
    compact: bool,
    t: &Theme,
) -> AnyElement {
    let warn = t.tone(Tone::Warn);
    let mut flags = Vec::new();
    if pending.in_both {
        let other = if pending
            .origins
            .first()
            .is_some_and(|o| o.starts_with(Tool::Codex.label()))
        {
            Tool::ClaudeCode
        } else {
            Tool::Codex
        };
        flags.push(
            flag(format!("also in {}", other.label()), t.blue)
                .into_any_element(),
        );
    }
    if let Some(id) = &pending.flags.disagrees {
        flags.push(
            flag(format!("disagrees with {id}"), warn).into_any_element(),
        );
    }
    if let Some(id) = &pending.flags.replaces {
        flags.push(flag(format!("replaces {id}"), warn).into_any_element());
    }
    if pending.flags.secret {
        flags.push(flag("held a secret".into(), t.red).into_any_element());
    }
    let from = match pending.origins.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [first, rest @ ..] => {
            format!("{first}, and {}", plural(rest.len(), "more", "more"))
        }
    };
    let tick = {
        let handle = handle.clone();
        let (id, keep) = (pending.id.clone(), !pending.keep);
        let selector = format!("memory-review-tick-{}", pending.id);
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector.clone())
            .pt(sp(0.5))
            .cursor_pointer()
            .child(ui::checkbox(pending.keep, false, t))
            .on_click(move |_, _, cx| {
                handle.act(
                    Act::Tick {
                        id: id.clone(),
                        keep,
                    },
                    cx,
                )
            })
    };
    div()
        .flex()
        .items_start()
        .gap(sp(3.5))
        .px(sp(4.))
        .py(sp(3.5))
        .border_b_1()
        .border_color(t.border)
        .when(pending.flags.need_a_look(), |row| {
            row.bg(warn.opacity(0.05))
        })
        .child(tick)
        .child(
            div()
                .flex_1()
                .min_w(rems(0.))
                .flex()
                .flex_col()
                .gap(sp(1.))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_baseline()
                        .gap(sp(2.))
                        .child(
                            div()
                                .min_w(rems(0.))
                                .text_color(t.text)
                                .font_weight(weight::EMPHASIS)
                                .child(pending.title.clone()),
                        )
                        .child(
                            div()
                                .typeset(Type::CAPTION)
                                .text_color(kind_color(pending.kind, t))
                                .child(pending.kind.as_str()),
                        ),
                )
                .child(ui::text(
                    pending.description.clone(),
                    Type::SMALL,
                    t.muted,
                ))
                .child(ui::text(from, Type::CAPTION, t.dim))
                .when(compact && !flags.is_empty(), |text| {
                    text.child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(sp(1.5))
                            .children(std::mem::take(&mut flags)),
                    )
                }),
        )
        .when(!compact, |row| {
            row.child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .items_end()
                    .gap(sp(1.))
                    .children(flags),
            )
        })
        .into_any_element()
}

/// How many are ticked, and the buttons that keep them or drop all.
fn foot(handle: &Handle, shown: &[&Pending], t: &Theme) -> impl IntoElement {
    let ticked: Vec<String> = shown
        .iter()
        .filter(|p| p.keep)
        .map(|p| p.id.clone())
        .collect();
    let all: Vec<String> = shown.iter().map(|p| p.id.clone()).collect();
    let left = shown.len() - ticked.len();
    let none = {
        let handle = handle.clone();
        div()
            .id("memory-review-none")
            .debug_selector(|| "memory-review-none".to_owned())
            .child(ui::button("Keep none", ButtonKind::Secondary, t))
            .on_click(move |_, _, cx| {
                handle.act(Act::Drop { ids: all.clone() }, cx)
            })
    };
    let keep = {
        let handle = handle.clone();
        let label = match ticked.len() {
            1 => "Keep 1 note".to_owned(),
            n => format!("Keep {n} notes"),
        };
        let kind = if ticked.is_empty() {
            ButtonKind::Secondary
        } else {
            ButtonKind::Primary
        };
        div()
            .id("memory-review-keep")
            .debug_selector(|| "memory-review-keep".to_owned())
            .child(ui::button(label, kind, t))
            .on_click(move |_, _, cx| {
                if !ticked.is_empty() {
                    handle.act(
                        Act::Keep {
                            ids: ticked.clone(),
                        },
                        cx,
                    )
                }
            })
    };
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(sp(3.))
        .child(ui::text(
            format!(
                "{} to keep · {} left for you to decide",
                shown.len() - left,
                left
            ),
            Type::SMALL,
            t.muted,
        ))
        .child(div().ml_auto().flex().gap(sp(2.5)).child(none).child(keep))
}

/// The files that did not read.
fn failed(imports: &Imports, t: &Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(sp(1.5))
        .child(ui::heading(
            &format!(
                "Could not read {}",
                plural(imports.failed.len(), "file", "files")
            ),
            t,
        ))
        .children(
            imports
                .failed
                .iter()
                .map(|path| ui::mono(path.clone(), Type::CAPTION, t.muted)),
        )
}
