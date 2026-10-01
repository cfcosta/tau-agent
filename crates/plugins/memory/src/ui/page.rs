//! The notes: search on the left, the open note in the middle, its links
//! on the right when there is room for them, and under the note when
//! there is not. The phone shows the list, then the note on its own.
//!
//! Every column gets a width worked out from the window's. Left to size
//! the note's text itself, GPUI can measure it at one width and draw it
//! at another, and paragraphs then overlap.

use gpui::{
    AnyElement,
    Entity,
    Hsla,
    PathBuilder,
    Pixels,
    SharedString,
    canvas,
    div,
    point,
    prelude::*,
    px,
    relative,
};
use tau_ui_kit::{
    assets::Icon,
    components::{
        self as ui,
        Edge,
        Material as _,
        NoteHead,
        heading,
        icon,
        mono,
    },
    input::TextInput,
    prose::rich_in,
    theme::{
        Design as _,
        IconSize,
        SANS,
        SERIF,
        Theme,
        Tone,
        Type,
        radius,
        sp,
        weight,
    },
};
use tau_ui_plugin::{Handle, Link, ViewCx, points::AtAnchor};

use super::{Mark, MemoryUi, NoteView, Notebook, USER, notes_link};
use crate::plugin::{NAME, USER as USER_ID};

/// The list's width, when the window allows it.
const LIST: f32 = 320.;
/// The narrowest the list gets.
const LIST_MIN: f32 = 240.;
/// The links column's width.
const LINKS: f32 = 320.;
/// The narrowest the note's column gets with the links beside it.
const NOTE_MIN: f32 = 560.;
/// The widest a note's text runs: a comfortable line.
const MEASURE: f32 = 680.;

/// The page's state in one window: what the list is searched for.
pub struct Ui {
    pub(super) search: Entity<TextInput>,
}

/// How the page is split, from the width it has.
struct Columns {
    list: Pixels,
    /// The links column, when it fits beside the note.
    links: Option<Pixels>,
    /// The note's side padding.
    pad: Pixels,
    /// The note's text width.
    text: Pixels,
}

impl Columns {
    fn for_width(width: f32) -> Self {
        let list = (width * 0.4).clamp(LIST_MIN, LIST);
        let beside = width - list - LINKS >= NOTE_MIN;
        let note = width - list - if beside { LINKS } else { 0. };
        let pad = if note >= NOTE_MIN { 12. } else { 5. };
        let text = (note - 2. * f32::from(sp(pad))).clamp(120., MEASURE);
        Self {
            list: px(list),
            links: beside.then_some(px(LINKS)),
            pad: sp(pad),
            text: px(text),
        }
    }
}

/// The notebook a page is about: a repository's, or with [`USER`] as its
/// scope, the user's.
fn notebook(view: &ViewCx<'_, MemoryUi>, repo: &str) -> Notebook {
    if repo == USER {
        view.data.clone()
    } else {
        view.repos.get(repo).cloned().unwrap_or_default()
    }
}

/// A note's page.
fn note_link(repo: &str, id: &str) -> Link {
    notes_link(repo).param("note", id.to_owned())
}

/// The page's title: the open note's, or whose notes they are.
pub fn title(view: &mut ViewCx<'_, MemoryUi>) -> String {
    let repo = view.param("repo").unwrap_or_default().to_owned();
    let book = notebook(view, &repo);
    match view.param("note").and_then(|id| book.note(id)) {
        Some(note) => note.title.clone(),
        None if repo == USER => "Your notes".to_owned(),
        None => "Memory".to_owned(),
    }
}

/// The page: the notes of the `repo` parameter, with the `note`
/// parameter open.
pub fn render(view: &mut ViewCx<'_, MemoryUi>) -> AnyElement {
    let t = view.theme().clone();
    let compact = view.compact;
    let Some(repo) = view.param("repo").map(str::to_owned) else {
        return ui::empty("Pick a repository to see its notes.", &t)
            .into_any_element();
    };
    let book = notebook(view, &repo);
    let open = view
        .param("note")
        .and_then(|id| book.note(id))
        .or_else(|| (!compact).then(|| book.notes.first()).flatten())
        .cloned();
    let search = view.read_ui().search.clone();
    let query = search.read(view.cx).text().to_lowercase();
    let handle = view.handle.clone();
    if compact {
        // The phone's screen padding on each side.
        let text = px((view.width - 2. * f32::from(sp(4.))).min(MEASURE));
        return match open {
            Some(note) => ui::screen(
                "memory-note",
                true,
                reader(&handle, &repo, &book, &note, true, text, true, &t),
            )
            .into_any_element(),
            None => ui::screen(
                "memory-list",
                true,
                list(&handle, &repo, &book, &search, &query, None, &t),
            )
            .into_any_element(),
        };
    }
    let columns = Columns::for_width(view.width);
    div()
        .flex_1()
        .min_h(px(0.))
        .flex()
        .child(
            div()
                .id("memory-list")
                .w(columns.list)
                .flex_shrink_0()
                .overflow_y_scroll()
                .chrome(Edge::Left, &t)
                .border_r_1()
                .border_color(t.border)
                .p(sp(3.))
                .child(list(
                    &handle,
                    &repo,
                    &book,
                    &search,
                    &query,
                    open.as_ref().map(|note| note.id.as_str()),
                    &t,
                )),
        )
        .child(match open {
            Some(note) => div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .child(
                    div()
                        .id("memory-note")
                        .flex_1()
                        .min_w(px(0.))
                        .overflow_y_scroll()
                        .px(columns.pad)
                        .py(sp(8.))
                        .child(reader(
                            &handle,
                            &repo,
                            &book,
                            &note,
                            false,
                            columns.text,
                            columns.links.is_none(),
                            &t,
                        )),
                )
                .when_some(columns.links, |row, width| {
                    row.child(
                        div()
                            .id("memory-links")
                            .w(width)
                            .flex_shrink_0()
                            .overflow_y_scroll()
                            .chrome(Edge::Right, &t)
                            .border_l_1()
                            .border_color(t.border)
                            .p(sp(5.))
                            .child(neighborhood(
                                &handle, &repo, &book, &note, &t,
                            )),
                    )
                })
                .into_any_element(),
            None => ui::empty("No notes yet.", &t).flex_1().into_any_element(),
        })
        .into_any_element()
}

fn list(
    handle: &Handle,
    repo: &str,
    book: &Notebook,
    search: &Entity<TextInput>,
    query: &str,
    open: Option<&str>,
    t: &Theme,
) -> impl IntoElement {
    let notes = book.notes.iter().filter(|note| {
        query.is_empty()
            || note.title.to_lowercase().contains(query)
            || note.body.iter().any(|p| p.to_lowercase().contains(query))
    });
    let (heading_text, about) = if repo == USER {
        (
            "Your notes".to_owned(),
            "What runs learned about you and how you work. Every \
             repository's runs see these."
                .to_owned(),
        )
    } else {
        (
            "Memory".to_owned(),
            format!(
                "What runs in {repo} learned. Runs in other repositories \
                 never see these notes."
            ),
        )
    };
    div()
        .flex()
        .flex_col()
        .gap(sp(3.))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(2.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(2.))
                        .when(repo != USER, |row| {
                            row.child(ui::repo_mark(repo, 20., t))
                        })
                        .child(
                            div()
                                .typeset(Type::TITLE)
                                .font_weight(weight::STRONG)
                                .child(heading_text),
                        )
                        .child(mono(
                            format!("{} notes", book.notes.len()),
                            Type::CAPTION,
                            t.dim,
                        )),
                )
                .child(
                    div()
                        .typeset(Type::CAPTION)
                        .text_color(t.muted)
                        .line_height(relative(1.5))
                        .child(about),
                ),
        )
        .when(!book.path.is_empty(), |list| {
            list.child(mono(book.path.clone(), Type::MICRO, t.dim))
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(2.))
                .h(px(36.))
                .px(sp(2.5))
                .border_1()
                .border_color(t.border_strong)
                .rounded(radius::CONTROL)
                .child(icon(Icon::Search, IconSize::BASE, t.dim))
                .child(search.clone()),
        )
        .children(notes.map(|note| {
            let active = open == Some(note.id.as_str());
            let to = note_link(repo, &note.id);
            let handle = handle.clone();
            let backlinks = book.backlinks(&note.id).count();
            div()
                .id(SharedString::from(format!("note-{}", note.id)))
                .flex()
                .flex_col()
                .gap(sp(1.))
                .p(sp(2.5))
                .rounded(radius::CONTROL)
                .cursor_pointer()
                .when(active, |row| row.pressed(t))
                .when(!active, |row| {
                    row.hover(|style| style.bg(gpui::white().opacity(0.03)))
                })
                .child(
                    div()
                        .font_weight(weight::EMPHASIS)
                        .child(note.title.clone()),
                )
                .child(
                    div()
                        .typeset(Type::CAPTION)
                        .text_color(t.muted)
                        .line_height(relative(1.45))
                        .child(rich_in(note.snippet(), SANS, t.muted, t)),
                )
                .child(mono(
                    format!(
                        "{} links · {backlinks} backlinks",
                        note.links.len()
                    ),
                    Type::MICRO,
                    t.dim,
                ))
                .on_click(move |_, _, cx| handle.navigate(to.clone(), cx))
        }))
}

/// A note, its text `width` wide; with its links under it when `links`
/// is set.
#[allow(clippy::too_many_arguments)]
fn reader(
    handle: &Handle,
    repo: &str,
    book: &Notebook,
    note: &NoteView,
    compact: bool,
    width: Pixels,
    links: bool,
    t: &Theme,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(sp(4.5))
        .w(width)
        .flex_shrink_0()
        .child(mono(
            format!(
                "{} · {} · edited {}",
                note.id, note.written_by, note.edited
            ),
            Type::CAPTION,
            t.dim,
        ))
        .child(
            div()
                .font_family(SERIF)
                .typeset(if compact {
                    Type::READING_TITLE_COMPACT
                } else {
                    Type::READING_TITLE
                })
                .font_weight(weight::EMPHASIS)
                .line_height(relative(1.2))
                .child(note.title.clone()),
        )
        .children(note.body.iter().map(|paragraph| {
            div()
                .typeset(if compact {
                    Type::READING_COMPACT
                } else {
                    Type::READING
                })
                .line_height(relative(1.65))
                .child(rich_in(paragraph, SERIF, t.text_soft, t))
        }))
        .child(div().flex().flex_wrap().gap(sp(2.)).children(
            note.paths.iter().map(|path| {
                ui::tag(path.clone(), Type::CAPTION, t.muted, t)
                    .px(sp(2.))
                    .py(sp(0.75))
            }),
        ))
        .when(links, |reader| {
            reader.child(neighborhood(handle, repo, book, note, t))
        })
}

/// The note's links out and in, as a small map and as lists.
fn neighborhood(
    handle: &Handle,
    repo: &str,
    book: &Notebook,
    note: &NoteView,
    t: &Theme,
) -> impl IntoElement {
    let out: Vec<(&NoteView, &str)> = note
        .links
        .iter()
        .filter_map(|link| {
            book.note(&link.to).map(|to| (to, link.why.as_str()))
        })
        .collect();
    let back: Vec<(&NoteView, &str)> = book.backlinks(&note.id).collect();

    // Fixed spots, staggered so labels never share a line: links out
    // above the note, backlinks below it. Labels are 120 px boxes
    // centred on their node, so nodes stay 60 px or more inside the
    // map's edges.
    const ABOVE: [(f32, f32); 3] = [(0.22, 0.2), (0.78, 0.26), (0.5, 0.06)];
    const BELOW: [(f32, f32); 3] = [(0.22, 0.7), (0.5, 0.82), (0.78, 0.66)];
    let out_spots: Vec<(f32, f32)> =
        ABOVE.iter().copied().take(out.len()).collect();
    let back_spots: Vec<(f32, f32)> =
        BELOW.iter().copied().take(back.len()).collect();
    let all: Vec<(f32, f32)> =
        out_spots.iter().chain(&back_spots).copied().collect();
    let edge = t.border_strong;
    let lines = canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let at = |(x, y): (f32, f32)| {
                point(
                    bounds.left() + bounds.size.width * x,
                    bounds.top() + bounds.size.height * y,
                )
            };
            for spot in &all {
                let mut path = PathBuilder::stroke(px(1.5));
                path.move_to(at((0.5, 0.5)));
                path.line_to(at(*spot));
                if let Ok(path) = path.build() {
                    window.paint_path(path, edge);
                }
            }
        },
    )
    .absolute()
    .size_full();
    let node =
        |(x, y): (f32, f32), color: Hsla, size: f32, label: Option<String>| {
            div()
                .absolute()
                .left(relative(x))
                .top(relative(y))
                .w(px(120.))
                .ml(sp(-15.))
                .mt(px(-size / 2.))
                .flex()
                .flex_col()
                .items_center()
                .child(
                    div()
                        .size(px(size))
                        .rounded(px(size / 2.))
                        .bg(color)
                        .border_2()
                        .border_color(t.panel),
                )
                .children(label.map(|label| {
                    div()
                        .mt(sp(1.))
                        .max_w(px(120.))
                        .truncate()
                        .typeset(Type::MICRO)
                        .text_color(t.muted)
                        .child(label)
                }))
        };
    let map = div()
        .relative()
        .h(px(200.))
        .w_full()
        .child(lines)
        .child(node((0.5, 0.5), t.accent, 18., None))
        .children(out.iter().zip(&out_spots).map(|((to, _), spot)| {
            node(*spot, t.blue, 12., Some(to.title.clone()))
        }))
        .children(back.iter().zip(&back_spots).map(|((from, _), spot)| {
            node(*spot, t.slate, 12., Some(from.title.clone()))
        }));
    let link_row =
        |prefix: &str, index: usize, target: &NoteView, why: &str| {
            let to = note_link(repo, &target.id);
            let handle = handle.clone();
            div()
                .id(SharedString::from(format!("{prefix}-{index}")))
                .flex()
                .flex_col()
                .gap(sp(0.5))
                .py(sp(1.5))
                .cursor_pointer()
                .child(
                    div()
                        .text_color(t.text)
                        .hover(|style| style.text_color(t.blue))
                        .child(target.title.clone()),
                )
                .child(
                    div()
                        .typeset(Type::CAPTION)
                        .text_color(t.muted)
                        .child(format!("why: {why}")),
                )
                .on_click(move |_, _, cx| handle.navigate(to.clone(), cx))
        };
    let out_rows: Vec<_> = out
        .iter()
        .enumerate()
        .map(|(i, (to, why))| link_row("out", i, to, why))
        .collect();
    let back_rows: Vec<_> = back
        .iter()
        .enumerate()
        .map(|(i, (from, why))| link_row("back", i, from, why))
        .collect();
    div()
        .flex()
        .flex_col()
        .gap(sp(4.5))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(2.))
                .child(heading("Neighborhood", t))
                .child(map),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(0.5))
                .child(heading(&format!("Links out · {}", out.len()), t))
                .children(out_rows),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(0.5))
                .child(heading(&format!("Backlinks · {}", back.len()), t))
                .children(back_rows),
        )
}

/// A recalled note's page: the user's for a note of the user's scope.
fn recalled_link(repo: &str, id: &str) -> Link {
    match id.strip_prefix(USER_ID) {
        Some(id) => note_link(USER, id),
        None => note_link(repo, id),
    }
}

/// One of memory's notes in a transcript: the notes a run started with,
/// each opening its page; what it saved; what failed.
pub fn mark(
    at: &AtAnchor,
    view: &mut ViewCx<'_, MemoryUi>,
) -> Option<AnyElement> {
    let mark = view.state?.marks.get(&at.key)?.clone();
    let t = view.theme().clone();
    let (text, tone, body) = match mark {
        Mark::Recalled(notes) => {
            let handle = view.handle.clone();
            let chips = div().flex().flex_wrap().gap(sp(1.5)).children(
                notes.iter().enumerate().map(|(n, (id, title))| {
                    let to = recalled_link(&at.run.repo, id);
                    let handle = handle.clone();
                    div()
                        .id(SharedString::from(format!(
                            "recalled-{}-{n}",
                            at.index
                        )))
                        .px(sp(2.25))
                        .py(sp(0.75))
                        .rounded(radius::CARD)
                        .border_1()
                        .border_color(t.border)
                        .typeset(Type::CAPTION)
                        .text_color(t.text_soft)
                        .cursor_pointer()
                        .hover(|style| style.border_color(t.border_strong))
                        .child(title.clone())
                        .on_click(move |_, _, cx| {
                            handle.navigate(to.clone(), cx)
                        })
                }),
            );
            (
                match notes.len() {
                    1 => "recalled 1 note for this task".to_owned(),
                    n => format!("recalled {n} notes for this task"),
                },
                Tone::Quiet,
                Some(chips.into_any_element()),
            )
        }
        Mark::Saved(n) => (
            match n {
                1 => "saved 1 note".to_owned(),
                n => format!("saved {n} notes"),
            },
            Tone::Good,
            None,
        ),
        Mark::Failed(message) => (message, Tone::Danger, None),
    };
    Some(
        ui::note(
            SharedString::from(format!("memory-{}-{}", at.run.id.0, at.key)),
            NoteHead {
                plugin: NAME.into(),
                icon: Icon::Memory,
                tone,
                text,
                detail: None,
            },
            None,
            None,
            None,
            body,
            view.compact,
            &t,
        )
        .into_any_element(),
    )
}
