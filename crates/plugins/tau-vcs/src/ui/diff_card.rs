//! The `vcs_diff` and `vcs_show` cards. Closed, a diff sums up its
//! files and lines; a show, its subject. Open, a diff lists its files,
//! each opening to its hunks, and a show leads with the message, ids,
//! author and parents, then lists its files the same way.

use gpui::{Div, SharedString, div, prelude::*, rems};
use tau_ui_kit::{
    assets::Icon,
    components::{dot, heading, icon, mono},
    diff::DiffKind,
    theme::{Design as _, IconSize, Theme, Type, radius, sp, weight},
};

use super::{
    Card,
    change_diff::{ChangeDiff, FileDiff, Hunk},
    change_log::Change,
    log_card,
};
use crate::ChangeKind;

/// A closed card's share of added and removed lines: five squares,
/// green for added, red for removed, grey for the rest of a rounding.
pub fn blocks(diff: &ChangeDiff, t: &Theme) -> Div {
    let (added, removed) = (diff.added(), diff.removed());
    let total = (added + removed).max(1) as f32;
    let green = (5. * added as f32 / total).round() as usize;
    let red = (5. * removed as f32 / total).round() as usize;
    div()
        .flex()
        .flex_shrink_0()
        .gap(sp(0.5))
        .children((0..5).map(|block| {
            let color = if block < green {
                t.green
            } else if block < green + red {
                t.red
            } else {
                t.border_strong
            };
            div().size(rems(0.4375)).rounded(radius::HAIRLINE).bg(color)
        }))
}

/// A diff's header: which change, and how many files.
pub fn files_summary(diff: &ChangeDiff, t: &Theme) -> Div {
    div()
        .flex_1()
        .min_w(rems(0.))
        .flex()
        .items_center()
        .gap(sp(1.5))
        .typeset(Type::CAPTION.mono())
        .child(log_card::short_id(&diff.change, t))
        .child(div().text_color(t.dim).child("·"))
        .child(div().text_color(t.text_soft).child(diff.file_count()))
}

/// A show's header: the change's type, subject and short id.
pub fn commit_summary(diff: &ChangeDiff, t: &Theme) -> Div {
    let change = &diff.change;
    div()
        .flex_1()
        .min_w(rems(0.))
        .flex()
        .items_center()
        .gap(sp(2.))
        .children(change.kind.as_ref().map(|_| kind_badge(change, t)))
        .child(
            div()
                .flex_1()
                .min_w(rems(0.))
                .truncate()
                .typeset(Type::SMALL)
                .text_color(t.text_soft)
                .child(subject(change)),
        )
        .child(log_card::short_id(change, t))
}

/// An open diff: its files.
pub fn files_body(
    card: &Card<'_>,
    diff: &ChangeDiff,
    t: &Theme,
    compact: bool,
) -> Div {
    div().flex().flex_col().py(sp(1.)).child(file_list(
        card,
        &diff.files,
        diff.truncated,
        &|_| false,
        t,
        compact,
    ))
}

/// An open show: the message, then the ids, author and parents, then
/// the files.
pub fn commit_body(
    card: &Card<'_>,
    diff: &ChangeDiff,
    t: &Theme,
    compact: bool,
) -> Div {
    let change = &diff.change;
    let info = &change.info;
    let body = info
        .description
        .trim()
        .split_once('\n')
        .map(|(_, body)| body.trim().to_owned())
        .filter(|body| !body.is_empty());
    let state = change.state();

    let message = div()
        .flex()
        .flex_col()
        .gap(sp(2.))
        .px(sp(4.))
        .pt(sp(3.5))
        .pb(sp(3.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(2.))
                .typeset(Type::CAPTION)
                .when_some(change.scope.clone(), |row, scope| {
                    row.child(dot(t.mark(&scope), 8.)).child(
                        div()
                            .text_color(t.text)
                            .font_weight(weight::EMPHASIS)
                            .child(scope),
                    )
                })
                .children(change.kind.as_ref().map(|_| kind_badge(change, t)))
                .children(log_card::bookmarks(change, t))
                .child(div().flex_1())
                .child(
                    mono(state.join(" · "), Type::MICRO, t.dim)
                        .flex_shrink_0()
                        .px(sp(1.5))
                        .border_1()
                        .border_color(t.border_strong)
                        .rounded(radius::SMALL),
                ),
        )
        .child(
            div()
                .typeset(Type::LEAD)
                .font_weight(weight::EMPHASIS)
                .text_color(if change.subject.is_empty() {
                    t.dim
                } else {
                    t.text
                })
                .child(subject(change)),
        )
        .children(body.map(|body| {
            div().typeset(Type::SMALL).text_color(t.muted).children(
                body.lines().map(|line| {
                    // A blank line between paragraphs keeps its height.
                    div().min_h(rems(1.25)).child(line.to_owned())
                }),
            )
        }));

    let field = |name: &'static str, value: Div| {
        div()
            .flex_1()
            .min_w(rems(0.))
            .flex()
            .items_center()
            .gap(sp(2.))
            .child(
                mono(name, Type::CAPTION, t.dim)
                    .w(rems(3.25))
                    .flex_shrink_0(),
            )
            .child(value.min_w(rems(0.)).truncate())
    };
    let author = diff.author.as_ref().map(|author| {
        field(
            "author",
            div()
                .typeset(Type::CAPTION.mono())
                .text_color(t.text_soft)
                .child(author.name.clone()),
        )
    });
    let parents: Vec<Div> = diff
        .parents
        .iter()
        .map(|parent| field("parent", parent_line(parent, t)))
        .collect();
    let mut right = parents.into_iter();
    let mut rows = vec![
        (
            field(
                "change",
                mono(info.change_id.clone(), Type::CAPTION, t.change),
            ),
            Some(field(
                "commit",
                mono(info.commit_id.clone(), Type::CAPTION, t.blue),
            )),
        ),
        (author.unwrap_or_else(|| div().flex_1()), right.next()),
    ];
    rows.extend(right.map(|parent| (div().flex_1(), Some(parent))));
    let ids = div()
        .flex()
        .flex_col()
        .gap(sp(1.5))
        .px(sp(4.))
        .py(sp(2.5))
        .border_t_1()
        .border_color(t.raised)
        .children(rows.into_iter().map(|(left, right)| {
            let row = div().flex().gap(sp(5.));
            // The phone stacks the two columns.
            if compact {
                row.flex_col().gap(sp(1.5)).child(left).children(right)
            } else {
                row.child(left)
                    .child(right.unwrap_or_else(|| div().flex_1()))
            }
        }));

    div()
        .flex()
        .flex_col()
        .child(message)
        .child(ids)
        .when(!diff.files.is_empty(), |body| {
            body.child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(2.))
                    .h(rems(1.875))
                    .px(sp(3.))
                    .border_t_1()
                    .border_color(t.raised)
                    .child(heading(
                        &format!("Files · {}", diff.file_count()),
                        t,
                    )),
            )
        })
        .child(div().pb(sp(1.)).child(file_list(
            card,
            &diff.files,
            diff.truncated,
            &|_| false,
            t,
            compact,
        )))
}

fn subject(change: &Change) -> String {
    if change.subject.is_empty() {
        "(no description set)".into()
    } else {
        change.subject.clone()
    }
}

pub(super) fn kind_badge(change: &Change, t: &Theme) -> Div {
    let (color, bg) = log_card::kind_colors(change.kind.as_deref(), t);
    mono(change.kind.clone().unwrap_or_default(), Type::MICRO, color)
        .flex_shrink_0()
        .px(sp(1.5))
        .rounded(radius::SMALL)
        .bg(bg)
}

/// A parent as the log draws a change: its scope's dot, short id and
/// subject.
fn parent_line(parent: &Change, t: &Theme) -> Div {
    div()
        .flex()
        .items_center()
        .gap(sp(1.5))
        .child(dot(
            parent.scope.as_deref().map_or(t.dim, |scope| t.mark(scope)),
            6.,
        ))
        .child(log_card::short_id(parent, t))
        .children(log_card::bookmarks(parent, t))
        .child(
            div()
                .min_w(rems(0.))
                .truncate()
                .typeset(Type::CAPTION)
                .text_color(t.dim)
                .child(subject(parent)),
        )
}

/// Each file as a row that opens to its hunks, then a note when the
/// diff was cut.
pub fn file_list(
    card: &Card<'_>,
    files: &[FileDiff],
    truncated: bool,
    conflicted: &dyn Fn(&str) -> bool,
    t: &Theme,
    compact: bool,
) -> Div {
    let rows: Vec<Div> = files
        .iter()
        .map(|file| {
            let open = card.ui.file_open(&card.run, &card.call_id, &file.path);
            div()
                .flex()
                .flex_col()
                .child(file_row(
                    card,
                    file,
                    open,
                    conflicted(&file.path),
                    t,
                    compact,
                ))
                .when(open, |column| column.child(hunks(file, t, compact)))
        })
        .collect();
    div()
        .flex()
        .flex_col()
        .when(files.is_empty(), |list| {
            list.child(
                div()
                    .px(sp(3.))
                    .py(sp(1.5))
                    .typeset(Type::CAPTION)
                    .text_color(t.dim)
                    .child("No changes."),
            )
        })
        .children(rows)
        .when(truncated, |list| {
            list.child(
                div()
                    .px(sp(3.))
                    .pt(sp(1.5))
                    .typeset(Type::CAPTION)
                    .text_color(t.dim)
                    .child(
                        "The diff was cut at 50 KB: files past the cut are \
                         not listed.",
                    ),
            )
        })
}

fn file_row(
    card: &Card<'_>,
    file: &FileDiff,
    open: bool,
    conflicted: bool,
    t: &Theme,
    compact: bool,
) -> impl IntoElement {
    let run_id = card.run.clone();
    let call_id = card.call_id.clone();
    let path = file.path.clone();
    let (letter, color) = match file.kind {
        ChangeKind::Added => ("A", t.green),
        ChangeKind::Modified => ("M", t.accent),
        ChangeKind::Removed => ("D", t.red),
    };
    let (dir, name) = file
        .path
        .rsplit_once('/')
        .map_or(("", file.path.as_str()), |(dir, name)| (dir, name));
    let total = (file.added + file.removed).max(1) as f32;
    div()
        .id(SharedString::from(format!(
            "file-{}-{}",
            card.call_id, file.path
        )))
        .flex()
        .items_center()
        .gap(sp(2.5))
        .h(rems(1.875))
        .px(sp(3.))
        .cursor_pointer()
        .hover(|row| row.bg(t.raised))
        .on_click(
            card.on_ui(move |ui| ui.toggle_file(&run_id, &call_id, &path)),
        )
        .child(icon(
            if open { Icon::Down } else { Icon::Chevron },
            IconSize::TINY,
            t.dim,
        ))
        .child(
            mono(letter, Type::MICRO, color)
                .size(rems(1.125))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(radius::SMALL)
                .bg(color.opacity(0.14))
                .font_weight(weight::EMPHASIS),
        )
        .child(
            // The folder gives way before the file's name does.
            div()
                .flex_1()
                .min_w(rems(0.))
                .flex()
                .typeset(Type::CAPTION.mono())
                .when(!dir.is_empty(), |path| {
                    path.child(
                        div()
                            .min_w(rems(0.))
                            .truncate()
                            .text_color(t.dim)
                            .child(format!("{dir}/")),
                    )
                })
                .child(
                    div()
                        .flex_shrink_0()
                        .text_color(t.text_soft)
                        .child(name.to_owned()),
                ),
        )
        .when(conflicted, |row| {
            row.child(mono("conflict", Type::MICRO, t.red).flex_shrink_0())
        })
        .when(file.binary, |row| {
            row.child(mono("binary", Type::MICRO, t.dim).flex_shrink_0())
        })
        .when(file.added > 0, |row| {
            row.child(
                mono(format!("+{}", file.added), Type::MICRO, t.green)
                    .flex_shrink_0(),
            )
        })
        .when(file.removed > 0, |row| {
            row.child(
                mono(format!("−{}", file.removed), Type::MICRO, t.red)
                    .flex_shrink_0(),
            )
        })
        .when(!compact, |row| {
            let width = 44.;
            row.child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .w(rems((width) / 16.))
                    .h(rems(0.25))
                    .rounded(radius::HAIRLINE)
                    .overflow_hidden()
                    .bg(t.raised)
                    .child(
                        div()
                            .h_full()
                            .w(rems((width * file.added as f32 / total) / 16.))
                            .bg(t.green),
                    )
                    .child(
                        div()
                            .h_full()
                            .w(rems(
                                (width * file.removed as f32 / total) / 16.,
                            ))
                            .bg(t.red),
                    ),
            )
        })
}

/// The lines jj writes around a conflict, as `diff` markers.
const CONFLICT_MARKERS: [&str; 5] =
    ["<<<<<<<", "%%%%%%%", r"\\\\\\\", "+++++++", ">>>>>>>"];

/// A file's hunks, each under its `@@` line, with old and new line
/// numbers; the phone leaves the numbers out.
fn hunks(file: &FileDiff, t: &Theme, compact: bool) -> Div {
    let number = |n: Option<u32>| {
        div()
            .w(rems(2.25))
            .flex_shrink_0()
            .pr(sp(1.5))
            .flex()
            .justify_end()
            .text_color(t.dim.opacity(0.6))
            .child(n.map(|n| n.to_string()).unwrap_or_default())
    };
    let gutter = if compact { sp(0.) } else { rems(4.5) };
    let note = |text: &'static str| {
        div()
            .pl(gutter + sp(4.5))
            .py(sp(1.5))
            .typeset(Type::CAPTION)
            .text_color(t.dim)
            .child(text)
    };
    // The file's lines colored by its language, each side of the change
    // as code of its own; one list across its hunks.
    let colors = tau_ui_kit::syntax::Lang::of_path(&file.path).map(|lang| {
        tau_ui_kit::diff::colors(
            &file
                .hunks
                .iter()
                .flat_map(|hunk| &hunk.lines)
                .map(|line| (line.kind, line.text.as_str()))
                .collect::<Vec<_>>(),
            lang,
        )
    });
    let context = t.syntax.faded(tau_ui_kit::diff::CONTEXT_OPACITY);
    let starts: Vec<usize> = file
        .hunks
        .iter()
        .scan(0, |at, hunk| {
            let start = *at;
            *at += hunk.lines.len();
            Some(start)
        })
        .collect();
    let hunk_view = |(h, hunk): (usize, &Hunk)| {
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .pl(gutter + sp(4.5))
                    .py(sp(0.75))
                    .bg(t.info_panel)
                    .text_color(t.slate)
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child(hunk.header.clone()),
            )
            .children(hunk.lines.iter().enumerate().map(|(i, line)| {
                let marker = line.kind == DiffKind::Added
                    && CONFLICT_MARKERS
                        .iter()
                        .any(|marker| line.text.starts_with(marker));
                let (sign, color, bg) = match line.kind {
                    // jj's conflict markers: what is left to edit out.
                    DiffKind::Added if marker => {
                        ("!", t.red, Some(t.red.opacity(0.22)))
                    }
                    DiffKind::Added => {
                        ("+", t.added_text, Some(t.green.opacity(0.12)))
                    }
                    DiffKind::Removed => {
                        ("−", t.removed_text, Some(t.red.opacity(0.12)))
                    }
                    DiffKind::Context => ("", t.dim, None),
                };
                div()
                    .flex()
                    .when_some(bg, |row, bg| row.bg(bg))
                    .when(marker, |row| row.font_weight(weight::STRONG))
                    .when(!compact, |row| {
                        row.child(number(line.old)).child(number(line.new))
                    })
                    .child(
                        div()
                            .w(rems(1.125))
                            .flex_shrink_0()
                            .flex()
                            .justify_center()
                            .text_color(color)
                            .child(sign),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(rems(0.))
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_color(color)
                            .child(tau_ui_kit::syntax::styled(
                                &line.text,
                                colors.as_ref().map(|colors| {
                                    colors[starts[h] + i].as_slice()
                                }),
                                color,
                                if line.kind == DiffKind::Context {
                                    &context
                                } else {
                                    &t.syntax
                                },
                            )),
                    )
            }))
    };
    div()
        .flex()
        .flex_col()
        .py(sp(0.5))
        .bg(t.bg)
        .border_t_1()
        .border_b_1()
        .border_color(t.raised)
        .font_family(tau_ui_kit::theme::MONO)
        .typeset(Type::CAPTION)
        .line_height(rems(1.25))
        .when(file.binary, |body| body.child(note("A binary file.")))
        .when(file.hunks.is_empty() && !file.binary, |body| {
            body.child(note("No lines changed."))
        })
        .children(file.hunks.iter().enumerate().map(hunk_view))
}
