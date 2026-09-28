//! The memory notes: search and suggestions on the left, the open note in
//! the middle, its links on the right. The phone shows the list, then the
//! note on its own.

use gpui::{
    AnyElement,
    Context,
    Hsla,
    PathBuilder,
    SharedString,
    canvas,
    div,
    point,
    prelude::*,
    px,
    relative,
};

use crate::{
    assets::Icon,
    catalog::Note,
    route::Route,
    theme::{
        Design as _,
        IconSize,
        SANS,
        SERIF,
        Theme,
        Type,
        radius,
        sp,
        weight,
    },
    ui::{self, components::ButtonKind, heading, icon, mono, rich_in},
    workspace::Workspace,
};

pub fn render(
    ws: &Workspace,
    note: Option<&str>,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let memory = &ws.catalog.memory;
    let open = note
        .and_then(|id| memory.note(id))
        .or_else(|| (!compact).then(|| memory.notes.first()).flatten());
    if compact {
        return match open {
            Some(note) => {
                ui::screen("memory-note", true, reader(ws, note, true, t, cx))
                    .into_any_element()
            }
            None => ui::screen("memory-list", true, list(ws, None, t, cx))
                .into_any_element(),
        };
    }
    div()
        .flex_1()
        .min_h(px(0.))
        .flex()
        .child(
            div()
                .id("memory-list")
                .w(px(320.))
                .flex_shrink_0()
                .overflow_y_scroll()
                .bg(t.panel)
                .border_r_1()
                .border_color(t.border)
                .p(sp(3.))
                .child(list(ws, open.map(|note| note.id.as_str()), t, cx)),
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
                        .px(sp(12.))
                        .py(sp(8.))
                        .child(reader(ws, note, false, t, cx)),
                )
                .child(
                    div()
                        .id("memory-links")
                        .w(px(320.))
                        .flex_shrink_0()
                        .overflow_y_scroll()
                        .bg(t.panel)
                        .border_l_1()
                        .border_color(t.border)
                        .p(sp(5.))
                        .child(neighborhood(ws, note, t, cx)),
                )
                .into_any_element(),
            None => ui::empty("No notes yet.", t).flex_1().into_any_element(),
        })
        .into_any_element()
}

fn list(
    ws: &Workspace,
    open: Option<&str>,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let memory = &ws.catalog.memory;
    let query = ws.memory_search.read(cx).text().to_lowercase();
    let notes = memory.notes.iter().filter(|note| {
        query.is_empty()
            || note.title.to_lowercase().contains(&query)
            || note.body.iter().any(|p| p.to_lowercase().contains(&query))
    });
    let pending: Vec<_> = ws.pending_proposals().collect();

    div()
        .flex()
        .flex_col()
        .gap(sp(3.))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(0.5))
                .child(mono(
                    format!("{} · {} notes", memory.path, memory.notes.len()),
                    Type::MICRO,
                    t.dim,
                ))
                .child(mono(
                    format!("docbert collection {}", memory.collection),
                    Type::MICRO,
                    t.dim,
                )),
        )
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
                .child(ws.memory_search.clone()),
        )
        .when(!pending.is_empty(), |list| {
            list.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(sp(2.))
                    .p(sp(3.))
                    .rounded(radius::BOX)
                    .bg(t.accent_soft)
                    .border_1()
                    .border_color(t.accent_border)
                    .child(
                        div()
                            .text_color(t.accent)
                            .font_weight(weight::EMPHASIS)
                            .child(format!(
                                "{} notes suggested",
                                pending.len()
                            )),
                    )
                    .children(pending.into_iter().enumerate().map(
                        |(n, (run, proposal))| {
                            let run_id = run.clone();
                            let title = proposal.title.clone();
                            div()
                                .flex()
                                .items_center()
                                .gap(sp(2.))
                                .child(
                                    div()
                                        .flex_1()
                                        .typeset(Type::CAPTION)
                                        .text_color(t.text_soft)
                                        .child(proposal.title.clone()),
                                )
                                .child(
                                    div()
                                        .id(("keep-suggested", n))
                                        .child(ui::button(
                                            "Keep",
                                            ButtonKind::Primary,
                                            t,
                                        ))
                                        .on_click(cx.listener(
                                            move |ws, _, _, cx| {
                                                ws.keep_note(
                                                    &run_id, &title, cx,
                                                )
                                            },
                                        )),
                                )
                        },
                    )),
            )
        })
        .children(notes.map(|note| {
            let active = open == Some(note.id.as_str());
            let route = Route::Memory {
                note: Some(note.id.clone()),
            };
            let backlinks = memory.backlinks(&note.id).count();
            div()
                .id(SharedString::from(format!("note-{}", note.id)))
                .flex()
                .flex_col()
                .gap(sp(1.))
                .p(sp(2.5))
                .rounded(radius::CONTROL)
                .cursor_pointer()
                .when(active, |row| row.bg(t.selected))
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
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.navigate(route.clone(), cx)
                }))
        }))
}

fn reader(
    ws: &Workspace,
    note: &Note,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(sp(4.5))
        .max_w(px(680.))
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
                crate::ui::tag(path.clone(), Type::CAPTION, t.muted, t)
                    .px(sp(2.))
                    .py(sp(0.75))
            }),
        ))
        .when(compact, |reader| {
            reader.child(neighborhood(ws, note, t, cx))
        })
}

/// The note's links out and in, as a small map and as lists.
fn neighborhood(
    ws: &Workspace,
    note: &Note,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let memory = &ws.catalog.memory;
    let out: Vec<(&Note, &str)> = note
        .links
        .iter()
        .filter_map(|link| {
            memory.note(&link.to).map(|to| (to, link.why.as_str()))
        })
        .collect();
    let back: Vec<(&Note, &str)> = memory.backlinks(&note.id).collect();

    // Links out sit above the note, backlinks below.
    // Fixed spots, staggered so labels never share a line: links out
    // above the note, backlinks below it.
    // Labels are 120 px boxes centred on their node, so nodes stay 60 px
    // or more inside the map's edges.
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

    let link_row = |prefix: &str,
                    index: usize,
                    target: &Note,
                    why: &str,
                    cx: &mut Context<Workspace>| {
        let route = Route::Memory {
            note: Some(target.id.clone()),
        };
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
            .on_click(
                cx.listener(move |ws, _, _, cx| ws.navigate(route.clone(), cx)),
            )
    };
    let out_rows: Vec<_> = out
        .iter()
        .enumerate()
        .map(|(i, (to, why))| link_row("out", i, to, why, cx))
        .collect();
    let back_rows: Vec<_> = back
        .iter()
        .enumerate()
        .map(|(i, (from, why))| link_row("back", i, from, why, cx))
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
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(1.5))
                .child(heading("Brought into runs", t))
                .child(
                    div()
                        .typeset(Type::CAPTION)
                        .text_color(t.text_soft)
                        .line_height(relative(1.5))
                        .child(format!(
                            "{} runs got this note at start.",
                            note.used_by_runs
                        )),
                ),
        )
}
