//! The notes: a search, a chip for each kind of note, and the notes in
//! one quiet column. A note opens in place of the list, its links beside
//! it when there is room for them and under it when there is not.
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
    rems,
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
        SERIF,
        Theme,
        Tone,
        Type,
        UNIT,
        radius,
        sp,
        weight,
    },
};
use tau_ui_plugin::{Handle, Link, ViewCx, points::AtAnchor};

use super::{Mark, MemoryUi, NoteView, Notebook, USER, notes_link};
use crate::{
    note::NoteType,
    record::{NAME, USER as USER_ID},
};

/// The widest the list's column gets.
const COLUMN: f32 = 860.;
/// The links column's width.
const LINKS: f32 = 320.;
/// The narrowest the note's column gets with the links beside it.
const NOTE_MIN: f32 = 560.;
/// The widest a note's text runs: a comfortable line.
const MEASURE: f32 = 680.;
/// The width of a row's right column: how linked the note is, and when
/// it was edited.
const ASIDE: f32 = 150.;

/// The page's state in one window: what the list is searched for.
pub struct Ui {
    pub(super) search: Entity<TextInput>,
}

/// How an open note's page is split, from the width it has.
struct Columns {
    /// The links column, when it fits beside the note.
    links: Option<Pixels>,
    /// The note's side padding.
    pad: gpui::Rems,
    /// The note's text width.
    text: Pixels,
}

impl Columns {
    fn for_width(width: f32) -> Self {
        let beside = width - LINKS >= NOTE_MIN;
        let note = width - if beside { LINKS } else { 0. };
        let pad = if note >= NOTE_MIN { 12. } else { 5. };
        let text = (note - 2. * UNIT * pad).clamp(120., MEASURE);
        Self {
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
        view.repo(repo).cloned().unwrap_or_default()
    }
}

/// A note's page.
fn note_link(repo: &str, id: &str) -> Link {
    notes_link(repo).param("note", id.to_owned())
}

/// The list, showing only notes of `kind`; every note without one.
fn kind_link(repo: &str, kind: Option<NoteType>) -> Link {
    match kind {
        Some(kind) => notes_link(repo).param("kind", kind.as_str()),
        None => notes_link(repo),
    }
}

/// A kind's name on its chip, for many notes.
fn plural(kind: NoteType) -> &'static str {
    match kind {
        NoteType::Fact => "Facts",
        NoteType::Convention => "Conventions",
        NoteType::Decision => "Decisions",
        NoteType::Gotcha => "Gotchas",
        NoteType::Procedure => "Procedures",
        NoteType::Case => "Cases",
        NoteType::Preference => "Preferences",
        NoteType::Index => "Indexes",
    }
}

/// The color a kind's name is written in.
fn kind_color(kind: NoteType, t: &Theme) -> Hsla {
    match kind {
        NoteType::Fact => t.blue,
        NoteType::Convention => t.syntax.ty,
        NoteType::Decision => t.syntax.keyword,
        NoteType::Gotcha => t.syntax.number,
        NoteType::Procedure => t.green,
        NoteType::Case => t.syntax.property,
        NoteType::Preference => t.roles.memory,
        NoteType::Index => t.muted,
    }
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

/// The page: the notes of the `repo` parameter, those of the `kind`
/// parameter alone when it is set, or the `note` parameter's note open.
pub fn render(view: &mut ViewCx<'_, MemoryUi>) -> AnyElement {
    let t = view.theme().clone();
    let compact = view.compact;
    let Some(repo) = view.param("repo").map(str::to_owned) else {
        return ui::empty("Pick a repository to see its notes.", &t)
            .into_any_element();
    };
    let book = notebook(view, &repo);
    let open = view.param("note").and_then(|id| book.note(id)).cloned();
    let kind = view.param("kind").and_then(NoteType::parse);
    let search = view.read_ui().search.clone();
    let query = search.read(view.cx).text().to_lowercase();
    let handle = view.handle.clone();
    let Some(note) = open else {
        return ui::screen(
            "memory-list",
            compact,
            div().flex().justify_center().child(
                div().w_full().max_w(rems((COLUMN) / 16.)).child(list(
                    &handle, &repo, &book, &search, &query, kind, &t,
                )),
            ),
        )
        .into_any_element();
    };
    let back = back(&handle, &repo, &t);
    if compact {
        // The phone's screen padding on each side.
        let text = px((view.width - 2. * UNIT * 4.).min(MEASURE));
        return ui::screen(
            "memory-note",
            true,
            div()
                .flex()
                .flex_col()
                .gap(sp(4.))
                .child(back)
                .child(reader(
                    &handle, &repo, &book, &note, true, text, true, &t,
                )),
        )
        .into_any_element();
    }
    let columns = Columns::for_width(view.width);
    div()
        .flex_1()
        .min_h(rems(0.))
        .flex()
        .child(
            div()
                .id("memory-note")
                .flex_1()
                .min_w(rems(0.))
                .overflow_y_scroll()
                .px(columns.pad)
                .py(sp(6.))
                .flex()
                .flex_col()
                .items_center()
                .gap(sp(4.))
                .child(div().w(columns.text).child(back))
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
                    .child(neighborhood(&handle, &repo, &book, &note, &t)),
            )
        })
        .into_any_element()
}

/// The way back from a note to the list.
fn back(handle: &Handle, repo: &str, t: &Theme) -> impl IntoElement {
    let to = notes_link(repo);
    let handle = handle.clone();
    let hover = t.text;
    div()
        .id("memory-back")
        .flex()
        .items_center()
        .gap(sp(1.5))
        .typeset(Type::SMALL)
        .text_color(t.muted)
        .cursor_pointer()
        .hover(move |style| style.text_color(hover))
        .child(icon(Icon::Back, IconSize::COMPACT, t.muted))
        .child("All notes")
        .on_click(move |_, _, cx| handle.navigate(to.clone(), cx))
}

/// The list: the search and the kinds, then a row for each note that
/// matches both.
fn list(
    handle: &Handle,
    repo: &str,
    book: &Notebook,
    search: &Entity<TextInput>,
    query: &str,
    kind: Option<NoteType>,
    t: &Theme,
) -> impl IntoElement {
    let rows: Vec<_> = book
        .notes
        .iter()
        .filter(|note| kind.is_none_or(|kind| note.kind == kind))
        .filter(|note| {
            query.is_empty()
                || note.title.to_lowercase().contains(query)
                || note.body.iter().any(|p| p.to_lowercase().contains(query))
        })
        .map(|note| row(handle, repo, book, note, t))
        .collect();
    let field = div()
        .flex_1()
        .min_w(rems(13.75))
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
    let chips = std::iter::once((None, "All", book.notes.len(), t.dim)).chain(
        NoteType::ALL.into_iter().filter_map(|each| {
            let count =
                book.notes.iter().filter(|note| note.kind == each).count();
            (count > 0)
                .then(|| (Some(each), plural(each), count, kind_color(each, t)))
        }),
    );
    div()
        .flex()
        .flex_col()
        .gap(sp(4.5))
        .when(repo == USER, |list| {
            list.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(sp(1.))
                    .child(
                        div()
                            .typeset(Type::TITLE)
                            .font_weight(weight::STRONG)
                            .child("Your notes"),
                    )
                    .child(
                        div().typeset(Type::SMALL).text_color(t.muted).child(
                            "What runs learned about you and how you \
                                 work. Every repository's runs see these.",
                        ),
                    ),
            )
        })
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(sp(2.5))
                .child(field)
                .children(chips.map(|(each, name, count, color)| {
                    let selected = each == kind;
                    let to = kind_link(repo, each);
                    let handle = handle.clone();
                    let hover = t.raised;
                    div()
                        .id(SharedString::from(format!(
                            "memory-kind-{}",
                            each.map_or("all", NoteType::as_str)
                        )))
                        .flex()
                        .items_center()
                        .gap(sp(1.25))
                        .px(sp(2.75))
                        .py(sp(1.5))
                        .rounded(radius::BOX)
                        .typeset(Type::SMALL)
                        .cursor_pointer()
                        .when(selected, |chip| {
                            chip.bg(t.raised).text_color(t.text)
                        })
                        .when(!selected, |chip| {
                            chip.text_color(t.text_soft)
                                .hover(move |style| style.bg(hover))
                        })
                        .child(name)
                        .child(div().text_color(color).child(count.to_string()))
                        .on_click(move |_, _, cx| {
                            handle.navigate(to.clone(), cx)
                        })
                })),
        )
        .child(if rows.is_empty() {
            ui::empty(
                if book.notes.is_empty() {
                    "No notes yet."
                } else {
                    "No notes match."
                },
                t,
            )
            .into_any_element()
        } else {
            div().flex().flex_col().children(rows).into_any_element()
        })
}

/// A note in the list: its id, title and kind over the start of its text,
/// how linked it is over when it was edited.
fn row(
    handle: &Handle,
    repo: &str,
    book: &Notebook,
    note: &NoteView,
    t: &Theme,
) -> impl IntoElement {
    let to = note_link(repo, &note.id);
    let handle = handle.clone();
    let links = note.links.len() + book.backlinks(&note.id).count();
    let hover = t.card;
    div()
        .id(SharedString::from(format!("note-{}", note.id)))
        .flex()
        .gap(sp(5.))
        .px(sp(1.))
        .py(sp(3.))
        .border_b_1()
        .border_color(t.border)
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
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
                        .items_baseline()
                        .gap(sp(2.5))
                        .min_w(rems(0.))
                        .child(
                            mono(
                                note.id.clone(),
                                Type::MICRO.sized(11.5),
                                t.dim,
                            )
                            .flex_shrink_0()
                            .max_w(rems(12.5))
                            .truncate(),
                        )
                        .child(
                            div()
                                .min_w(rems(0.))
                                .truncate()
                                .text_color(t.text)
                                .font_weight(weight::EMPHASIS)
                                .child(note.title.clone()),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .typeset(Type::CAPTION)
                                .text_color(kind_color(note.kind, t))
                                .child(note.kind.as_str()),
                        ),
                )
                .child(
                    div()
                        .typeset(Type::SMALL)
                        .text_color(t.muted)
                        .truncate()
                        .child(note.snippet().replace('`', "")),
                ),
        )
        .child(
            div()
                .w(rems((ASIDE) / 16.))
                .flex_shrink_0()
                .flex()
                .flex_col()
                .items_end()
                .gap(sp(1.))
                .typeset(Type::SMALL.sized(12.5))
                .child(div().text_color(t.muted).child(match links {
                    0 => "no links".to_owned(),
                    1 => "1 link".to_owned(),
                    n => format!("{n} links"),
                }))
                .child(div().text_color(t.dim).child(note.edited.clone())),
        )
        .on_click(move |_, _, cx| handle.navigate(to.clone(), cx))
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
                .w(rems(7.5))
                .ml(sp(-15.))
                .mt(rems((-size / 2.) / 16.))
                .flex()
                .flex_col()
                .items_center()
                .child(
                    div()
                        .size(rems((size) / 16.))
                        .rounded(rems((size / 2.) / 16.))
                        .bg(color)
                        .border_2()
                        .border_color(t.panel),
                )
                .children(label.map(|label| {
                    div()
                        .mt(sp(1.))
                        .max_w(rems(7.5))
                        .truncate()
                        .typeset(Type::MICRO)
                        .text_color(t.muted)
                        .child(label)
                }))
        };
    let map = div()
        .relative()
        .h(rems(12.5))
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
