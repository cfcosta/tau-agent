//! A run and one of its forks, side by side, under their checkpoints.

use gpui::{
    AnyElement,
    Context,
    Div,
    Hsla,
    SharedString,
    div,
    prelude::*,
    relative,
    rems,
};
use heiko_agent::tool::RunId;
use heiko_ui_plugin::{
    PluginStatus,
    points::{self, AtCard},
};

use crate::{
    route::Route,
    theme::{Design as _, Theme, Type, sp},
    ui::{self, BlockSize, State, landing, mono, rich, status_look},
    view::{
        CodeState,
        FileKind,
        FileStat,
        Origin,
        RunView,
        Tone,
        ToolCard,
        tokens,
    },
    workspace::Workspace,
};

pub fn render(
    ws: &Workspace,
    main: &RunId,
    fork: &RunId,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let (Some(main), Some(fork)) = (ws.run(main), ws.run(fork)) else {
        return ui::empty("That run is not in this workspace.", t)
            .into_any_element();
    };
    let at = match fork.origin {
        Origin::Fork { turn, .. } => turn,
        _ => main.turn,
    };
    let steer = fork.items.iter().find_map(|item| match item {
        crate::view::Item::User(text) => Some(text.clone()),
        _ => None,
    });

    let columns = div()
        .flex()
        .when(compact, |row| row.flex_col())
        .gap(sp(if compact { 6. } else { 10. }))
        .child(branch(ws, main, t, cx))
        .child(branch(ws, fork, t, cx));

    ui::screen(
        "compare",
        compact,
        div()
            .flex()
            .flex_col()
            .gap(sp(5.))
            .child(
                div()
                    .typeset(Type::PAGE_TITLE)
                    .text_color(t.bright)
                    .child(format!("{} and {}", main.title, fork.title)),
            )
            .child(checkpoints(main, fork, at, steer, t))
            .child(columns)
            .child(code(ws, main, fork, compact, t, cx)),
    )
    .into_any_element()
}

/// How wide the timeline's lines are, in rems: 3 px.
const LINE: f32 = 0.1875;
/// A turn's square on its line, in rems: 12 px.
const SQUARE: f32 = 0.75;
/// How far down the timeline each line is, in rems.
const MAIN_Y: f32 = 1.375;
const FORK_Y: f32 = 4.375;

/// The main line of turns as squares, and the fork leaving it at its
/// checkpoint.
fn checkpoints(
    main: &RunView,
    fork: &RunView,
    at: u32,
    steer: Option<String>,
    t: &Theme,
) -> impl IntoElement {
    let main_turns = main.turn.max(1);
    let fork_turns = fork.turn.saturating_sub(at);
    let fork_end = at + fork_turns.max(1);
    let total = main_turns.max(fork_end).max(2) as f32;
    let x = move |turn: u32| {
        0.01 + 0.8 * (turn.saturating_sub(1) as f32 / (total - 1.))
    };
    let (main_color, fork_color) = (t.blue, t.offset);

    let line = |from: u32, to: u32, y: f32, color: Hsla| {
        div()
            .absolute()
            .left(relative(x(from)))
            .w(relative(x(to) - x(from)))
            .top(rems(y))
            .h(rems(LINE))
            .bg(color)
    };
    let node = |turn: u32, y: f32, color: Hsla| {
        div()
            .absolute()
            .left(relative(x(turn)))
            .top(rems(y - (SQUARE - LINE) / 2.))
            .ml(rems(-SQUARE / 2.))
            .size(rems(SQUARE))
            .bg(color)
    };
    let label = |turn: u32, y: f32, text: String, color: Hsla| {
        mono(text, Type::CAPTION, color)
            .absolute()
            .left(relative(x(turn)))
            .top(rems(y - 0.4375))
            .ml(sp(4.))
            .whitespace_nowrap()
    };

    div()
        .relative()
        .h(rems(6.5))
        .w_full()
        .overflow_hidden()
        .child(line(1, main_turns, MAIN_Y, main_color))
        // The fork leaves the main line straight down, then runs on.
        .child(
            div()
                .absolute()
                .left(relative(x(at)))
                .top(rems(MAIN_Y))
                .ml(rems(-LINE / 2.))
                .w(rems(LINE))
                .h(rems(FORK_Y - MAIN_Y + LINE))
                .bg(fork_color),
        )
        .child(line(at, fork_end, FORK_Y, fork_color))
        .children((1..=main_turns).map(|turn| node(turn, MAIN_Y, main_color)))
        .children((1..=fork_turns).map(|n| node(at + n, FORK_Y, fork_color)))
        .children((1..=main_turns).map(|turn| {
            mono(
                if turn == at {
                    format!("t{turn} · fork")
                } else {
                    format!("t{turn}")
                },
                Type::MICRO,
                if turn == at { t.text } else { t.dim },
            )
            .absolute()
            .left(relative(x(turn)))
            .top_0()
            .ml(rems(-SQUARE / 2.))
            .whitespace_nowrap()
        }))
        .child(label(main_turns, MAIN_Y, main.title.clone(), main_color))
        .child(label(fork_end, FORK_Y, fork.title.clone(), fork_color))
        .when_some(steer, |graph, steer| {
            graph.child(
                mono(
                    format!("steer: \u{201c}{steer}\u{201d}"),
                    Type::CAPTION,
                    t.dim,
                )
                .absolute()
                .left(relative(x(at)))
                .top(rems(FORK_Y + 0.75))
                .ml(sp(4.))
                .whitespace_nowrap(),
            )
        })
}

/// How many lines of a file's diff the screen shows before cutting.
const DIFF_LINES: usize = 60;

/// What the fork keeps, among its folding cards, while "Where they
/// differ" is hidden.
const DIFFER: &str = "compare:differ";

/// The code of both branches: what each changed after the fork point,
/// and how the fork's code differs from the run's.
fn code(
    ws: &Workspace,
    main: &RunView,
    fork: &RunView,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let section = div().flex().flex_col().gap(sp(4.));
    let code = match ws.branch_code(&main.id, &fork.id) {
        None | Some(CodeState::Loading) => {
            return section.child(ui::section_header("Code", None, t)).child(
                ui::text("Reading both branches' code…", Type::SMALL, t.dim),
            );
        }
        Some(CodeState::Unavailable(why)) => {
            return section
                .child(ui::section_header("Code", None, t))
                .child(ui::text(why.clone(), Type::SMALL, t.muted));
        }
        Some(CodeState::Ready(code)) => code,
    };
    let changed = |run: &RunView, files: &[FileStat]| {
        let (added, removed) = files
            .iter()
            .fold((0, 0), |(a, r), file| (a + file.added, r + file.removed));
        let count = match files.len() {
            0 => "no changes".to_owned(),
            1 => "1 file ·".to_owned(),
            n => format!("{n} files ·"),
        };
        // Stacked, a column says whose code it is.
        let count = if compact {
            format!("{} · {count}", run.title)
        } else {
            count
        };
        div()
            .flex_1()
            .min_w(rems(0.))
            .flex()
            .flex_col()
            .child(
                ui::section_header("Code", None, t).child(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .gap(sp(1.5))
                        .child(mono(count, Type::CAPTION, t.dim))
                        .when(!files.is_empty(), |meta| {
                            meta.child(mono(
                                format!("+{added}"),
                                Type::CAPTION,
                                t.green,
                            ))
                            .child(mono(
                                format!("\u{2212}{removed}"),
                                Type::CAPTION,
                                t.red,
                            ))
                        }),
                ),
            )
            .children(files.iter().map(|file| file_row(file, t)))
    };
    // Shown until the person hides it.
    let hidden = ws.card_open(&fork.id, DIFFER);
    let toggle = (!code.between.is_empty()).then(|| {
        let fork = fork.id.clone();
        div()
            .id("compare-differ-toggle")
            .flex_shrink_0()
            .child(ui::link_action(if hidden { "show" } else { "hide" }, t))
            .on_click(cx.listener(move |ws, _, _, cx| {
                ws.toggle_card(&fork, DIFFER, cx)
            }))
    });
    let between = if code.between.is_empty() {
        ui::text("Both branches have the same code.", Type::SMALL, t.muted)
    } else {
        div()
            .flex()
            .flex_col()
            .children(code.between.iter().map(|file| {
                let shown = &file.lines[..file.lines.len().min(DIFF_LINES)];
                let hidden = file.lines.len() - shown.len();
                div()
                    .flex()
                    .flex_col()
                    .child(file_row(&file.stat, t))
                    .child(heiko_ui_kit::diff::numbered(
                        shown,
                        // A diff that came without its numbers has none.
                        file.numbers.get(..shown.len()),
                        heiko_ui_kit::syntax::Lang::of_path(&file.stat.path),
                        t,
                    ))
                    .when(hidden > 0, |file| {
                        file.child(
                            mono(
                                format!("{hidden} more lines"),
                                Type::CAPTION,
                                t.dim,
                            )
                            .py(sp(2.)),
                        )
                    })
            }))
    };
    section
        .child(
            div()
                .flex()
                .when(compact, |row| row.flex_col())
                .gap(sp(if compact { 6. } else { 10. }))
                .child(changed(main, &code.main))
                .child(changed(fork, &code.fork)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .child(
                    ui::section_header(
                        "Where they differ",
                        Some(
                            match code.between.len() {
                                1 => "1 file".to_owned(),
                                n => format!("{n} files"),
                            }
                            .into(),
                        ),
                        t,
                    )
                    .children(toggle),
                )
                .when(!hidden, |differ| differ.child(between)),
        )
}

/// A changed file between hairlines: its kind, path and line counts.
fn file_row(file: &FileStat, t: &Theme) -> Div {
    let color = match file.kind {
        FileKind::Added => t.green,
        FileKind::Modified => t.blue,
        FileKind::Removed => t.red,
    };
    div()
        .flex()
        .items_baseline()
        .gap(sp(2.5))
        .py(sp(1.5))
        .border_b_1()
        .border_color(t.hairline)
        .child(mono(file.kind.letter().to_string(), Type::SMALL, color))
        .child(
            mono(file.path.clone(), Type::SMALL, t.text)
                .flex_1()
                .min_w(rems(0.))
                .truncate(),
        )
        .child(mono(format!("+{}", file.added), Type::SMALL, t.green))
        .child(mono(
            format!("\u{2212}{}", file.removed),
            Type::SMALL,
            t.red,
        ))
}

/// A word the branch ran with, underlined: its model, its effort.
fn word(text: impl Into<SharedString>, color: Hsla, t: &Theme) -> Div {
    div()
        .flex_shrink_0()
        .border_b_1()
        .border_color(t.slate)
        .text_color(color)
        .child(text.into())
}

/// A stat of a branch: its name in small mono over what it holds.
fn stat(name: &str, t: &Theme) -> Div {
    div()
        .flex_1()
        .min_w(rems(0.))
        .flex()
        .flex_col()
        .gap(sp(1.5))
        .child(mono(name.to_owned(), Type::CAPTION, t.dim))
}

/// A stat's number, in the display face.
fn figure(text: impl Into<SharedString>, t: &Theme) -> Div {
    ui::big_figure(text, t).typeset(Type::FIGURE.sized(24.))
}

/// How many ticks the tests' strip has.
const TEST_TICKS: usize = 26;

/// A test run's stat: how many passed of how many, over a strip of
/// ticks, the failed ones red at its end. `state` is `passed / total`;
/// anything else is shown as it is.
fn tests(state: &str, t: &Theme) -> Div {
    let counts = state.split_once(" / ").and_then(|(passed, total)| {
        let passed: usize = passed.trim().parse().ok()?;
        let total: usize = total.trim().parse().ok()?;
        (total > 0 && passed <= total).then_some((passed, total))
    });
    let Some((passed, total)) = counts else {
        return stat("tests", t).child(figure(state.to_owned(), t));
    };
    // Any failure shows as a tick, however few of many.
    let failed = ((total - passed) * TEST_TICKS).div_ceil(total);
    stat("tests", t)
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(sp(2.))
                .child(figure(passed.to_string(), t))
                .child(ui::text(format!("of {total}"), Type::LEAD, t.dim)),
        )
        .child(div().flex().max_w(rems(8.25)).gap(rems(0.125)).children(
            (0..TEST_TICKS).map(|n| {
                div().flex_1().h(rems(0.5)).bg(if n < TEST_TICKS - failed {
                    t.green
                } else {
                    t.red
                })
            }),
        ))
}

fn branch(
    ws: &Workspace,
    run: &RunView,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let (stop_color, stop) = status_look(&run.status, t);
    // What the newest card with one says of the branch: its tests.
    let tests_stat = run.items.iter().rev().find_map(|item| match item {
        crate::view::Item::Tool(card) => card_stat(ws, run, card),
        _ => None,
    });
    let failing = tests_stat.as_ref().filter(|stat| stat.tone == Tone::Danger);
    let id = run.id.clone();
    let keep_id = run.id.clone();
    // A branch on another model than coder's default can make it one.
    let make_default = Some(Workspace::model_of(run)).filter(|choice| {
        ws.catalog.models.default_for("coder").model != choice.model
    });
    let diff = run.last_diff().map(|(card, lines, numbers)| {
        div()
            .flex()
            .flex_col()
            .gap(sp(1.))
            .child(mono(card.summary.clone(), Type::CAPTION, t.dim))
            .child(heiko_ui_kit::diff::numbered(
                &lines,
                Some(&numbers),
                card.args()
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .and_then(heiko_ui_kit::syntax::Lang::of_path),
                t,
            ))
    });
    let (mark, state, state_color) = match failing {
        Some(stat) => (
            State::Failing,
            SharedString::from(format!("{} failing", stat.name)),
            t.red,
        ),
        None => (ui::status_mark_state(&run.status), stop, stop_color),
    };
    // Which model the branch ran on: what a comparison is about.
    let choice = Workspace::model_of(run);
    let kept = ws.kept_branch.as_ref() == Some(&run.id);

    div()
        .flex_1()
        .min_w(rems(0.))
        .flex()
        .flex_col()
        .gap(sp(3.))
        .child(
            ui::section_header(run.title.clone(), None, t).child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(sp(2.))
                    .child(ui::mark(mark, t))
                    .child(ui::text(state, Type::SMALL, state_color)),
            ),
        )
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(sp(2.))
                .typeset(Type::BODY)
                .child(word(choice.model.clone(), t.blue, t))
                .child(div().text_color(t.dim).child("reasoning"))
                .child(word(choice.effort.label(), t.text, t)),
        )
        .child(
            div()
                .flex()
                .items_start()
                .gap(sp(4.))
                .py(sp(2.5))
                .border_y_1()
                .border_color(t.hairline)
                .child(stat("turns", t).child(figure(run.turn.to_string(), t)))
                .child(
                    stat("tokens", t)
                        .child(figure(tokens(run.usage.tokens), t)),
                )
                .child(match &tests_stat {
                    Some(stat) => tests(&stat.state, t),
                    None => stat("tests", t).child(figure("—", t)),
                }),
        )
        .children(run.last_text().map(|text| {
            div()
                .typeset(Type::LEAD)
                .leading(1.6)
                .child(rich(text, t.text, t))
        }))
        .children(diff)
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(sp(3.5))
                .pt(sp(1.))
                .typeset(Type::SMALL)
                .child(
                    div()
                        .id(SharedString::from(format!("keep-{}", run.id)))
                        .child(ui::primary_block(
                            "keep this branch",
                            BlockSize::Small,
                            t,
                        ))
                        .on_click(cx.listener(move |ws, _, _, cx| {
                            ws.keep_branch(&keep_id, cx)
                        })),
                )
                .when(kept, |row| row.child(ui::stamp("kept", Tone::Good, t)))
                .child(
                    div()
                        .id(SharedString::from(format!("open-{}", run.id)))
                        .child(ui::link_action("open transcript", t))
                        .on_click(cx.listener(move |ws, _, _, cx| {
                            ws.navigate(Route::Run(id.clone()), cx)
                        })),
                )
                .when_some(make_default, |row, choice| {
                    let label =
                        format!("make {} coder\u{2019}s default", choice.model);
                    row.child(
                        div()
                            .id(SharedString::from(format!(
                                "default-{}",
                                run.id
                            )))
                            .child(ui::link_action(label, t))
                            .on_click(cx.listener(move |ws, _, _, cx| {
                                ws.set_default_model(
                                    "coder",
                                    choice.clone(),
                                    cx,
                                )
                            })),
                    )
                }),
        )
        .children(landing::controls(ws, run, t, cx))
}

/// What a plugin says of `card` for the branch (`CARD_STAT`): the
/// first to answer.
fn card_stat(
    ws: &Workspace,
    run: &RunView,
    card: &ToolCard,
) -> Option<PluginStatus> {
    let contexts: Vec<(&str, AtCard)> = ws
        .plugin_names()
        .into_iter()
        .map(|name| (name, crate::views::at_card(run, card, name)))
        .collect();
    let at = |plugin: &str| {
        contexts
            .iter()
            .find(|(name, _)| *name == plugin)
            .map(|(_, at)| at)
    };
    let (_, stat) = ws
        .contributions_named(points::CARD_STAT, at)
        .into_iter()
        .next()?;
    Some(stat)
}
