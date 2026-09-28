//! A run and one of its forks, side by side, under their checkpoints.

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
use tau_agent::tool::RunId;

use crate::{
    route::Route,
    theme::Theme,
    ui::{self, dot, mono, pill, rich, status_look, transcript},
    view::{Origin, RunView, ToolBody, tokens, usd},
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
        .gap(px(16.))
        .child(branch(ws, main, t.accent, compact, t, cx))
        .child(branch(ws, fork, t.blue, compact, t, cx));

    ui::screen(
        "compare",
        compact,
        div()
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(ui::screen_title(
                format!("{} and {}", main.title, fork.title),
                "Every turn is stored. The fork shares everything up to its \
                 checkpoint and resends nothing it already shares.",
                t,
            ))
            .child(checkpoints(main, fork, at, steer, t))
            .child(columns),
    )
    .into_any_element()
}

/// The main line of turns, and the fork leaving it at its checkpoint.
fn checkpoints(
    main: &RunView,
    fork: &RunView,
    at: u32,
    steer: Option<String>,
    t: &Theme,
) -> impl IntoElement {
    let main_turns = main.turn.max(1);
    let fork_turns = fork.turn.saturating_sub(at);
    let total = main_turns.max(at + fork_turns).max(2) as f32;
    let x = move |turn: u32| {
        0.02 + 0.74 * (turn.saturating_sub(1) as f32 / (total - 1.))
    };
    let (main_y, fork_y) = (28., 84.);
    let (main_color, fork_color) = (t.accent, t.blue);

    let lines = canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let at_x = |frac: f32| bounds.left() + bounds.size.width * frac;
            let top = bounds.top();
            let mut path = PathBuilder::stroke(px(2.));
            path.move_to(point(at_x(x(1)), top + px(main_y)));
            path.line_to(point(at_x(x(main_turns)), top + px(main_y)));
            if let Ok(path) = path.build() {
                window.paint_path(path, main_color);
            }
            let mut path = PathBuilder::stroke(px(2.));
            let start = at_x(x(at));
            let bend = at_x(x(at + 1));
            path.move_to(point(start, top + px(main_y)));
            path.curve_to(
                point(bend, top + px(fork_y)),
                point(start, top + px(fork_y)),
            );
            path.line_to(point(
                at_x(x(at + fork_turns.max(1))),
                top + px(fork_y),
            ));
            if let Ok(path) = path.build() {
                window.paint_path(path, fork_color);
            }
        },
    )
    .absolute()
    .size_full();

    let node = move |turn: u32, y: f32, color: Hsla, filled: bool| {
        div()
            .absolute()
            .left(relative(x(turn)))
            .top(px(y - 6.))
            .ml(px(-6.))
            .size(px(12.))
            .rounded(px(6.))
            .border_2()
            .border_color(color)
            .bg(if filled { color } else { t.bg })
    };
    let label = move |turn: u32, y: f32, text: String, color: Hsla| {
        div()
            .absolute()
            .left(relative(x(turn)))
            .top(px(y - 9.))
            .ml(px(14.))
            .text_size(px(12.))
            .text_color(color)
            .whitespace_nowrap()
            .child(text)
    };

    div()
        .relative()
        .h(px(128.))
        .w_full()
        .child(lines)
        .children((1..=main_turns).map(|turn| {
            node(turn, main_y, main_color, turn == at || turn == main_turns)
        }))
        .children(
            (1..=fork_turns)
                .map(|n| node(at + n, fork_y, fork_color, n == fork_turns)),
        )
        .children((1..=main_turns).map(|turn| {
            div()
                .absolute()
                .left(relative(x(turn)))
                .top(px(0.))
                .ml(px(-8.))
                .child(mono(
                    if turn == at {
                        format!("t{turn} fork")
                    } else {
                        format!("t{turn}")
                    },
                    11.,
                    if turn == at { t.text } else { t.dim },
                ))
        }))
        .child(label(main_turns, main_y, main.title.clone(), t.text))
        .child(label(
            at + fork_turns.max(1),
            fork_y,
            fork.title.clone(),
            t.text,
        ))
        .when_some(steer, |graph, steer| {
            graph.child(
                div()
                    .absolute()
                    .left(relative(x(at)))
                    .top(px(104.))
                    .text_size(px(12.))
                    .text_color(t.dim)
                    .whitespace_nowrap()
                    .child(format!("steer: \u{201c}{steer}\u{201d}")),
            )
        })
}

fn branch(
    ws: &Workspace,
    run: &RunView,
    color: Hsla,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let (stop_color, stop) = status_look(&run.status, t);
    let tests = run.items.iter().rev().find_map(|item| match item {
        crate::view::Item::Tool(card) if card.tool == "bash" => {
            match &card.body {
                ToolBody::Output(lines) => lines
                    .iter()
                    .rev()
                    .find(|line| line.contains("tests run"))
                    .and_then(|line| line.split(": ").nth(1))
                    .and_then(|counts| counts.split(" passed").next())
                    .map(|passed| (passed.to_owned(), line_total(lines))),
                _ => None,
            }
        }
        _ => None,
    });
    let failing = tests
        .as_ref()
        .is_some_and(|(passed, total)| Some(passed) != total.as_ref());
    let id = run.id.clone();
    let keep_id = run.id.clone();
    let diff = run.last_diff().map(|(card, lines)| {
        ui::card(t)
            .bg(t.card)
            .child(
                div()
                    .px(px(12.))
                    .py(px(8.))
                    .border_b_1()
                    .border_color(t.border)
                    .child(mono(card.summary.clone(), 12., t.text_soft)),
            )
            .child(transcript::diff(lines, t))
    });

    div()
        .flex_1()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .gap(px(14.))
        .p(px(if compact { 0. } else { 4. }))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(dot(color, 10.))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(15.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(run.title.clone()),
                )
                .child(pill(
                    if failing {
                        "tests failing".into()
                    } else {
                        stop
                    },
                    if failing { t.red } else { stop_color },
                    t.raised,
                )),
        )
        .child(
            div()
                .grid()
                .grid_cols(4)
                .gap(px(8.))
                .child(ui::stat("turns", run.turn.to_string(), t))
                .child(ui::stat("tokens", tokens(run.usage.tokens), t))
                .child(ui::stat("cost", usd(run.usage.cost), t))
                .child(ui::stat(
                    "tests",
                    tests.map_or("—".into(), |(passed, total)| {
                        format!("{passed} / {}", total.unwrap_or_default())
                    }),
                    t,
                )),
        )
        .children(run.last_text().map(|text| {
            div()
                .text_color(t.text_soft)
                .line_height(relative(1.6))
                .child(rich(text, t))
        }))
        .children(diff)
        .child(
            div()
                .flex()
                .gap(px(8.))
                .child(
                    div()
                        .id(SharedString::from(format!("keep-{}", run.id)))
                        .child(ui::primary_button("Keep this branch", t))
                        .on_click(cx.listener(move |ws, _, _, cx| {
                            ws.keep_branch(&keep_id, cx)
                        })),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("open-{}", run.id)))
                        .child(ui::button("Open transcript", t))
                        .on_click(cx.listener(move |ws, _, _, cx| {
                            ws.navigate(Route::Run(id.clone()), cx)
                        })),
                ),
        )
        .when(ws.kept_branch.as_ref() == Some(&run.id), |column| {
            column.child(
                div().text_size(px(12.)).text_color(t.green).child(
                    "Kept. The host decides what keeping a branch does.",
                ),
            )
        })
}

/// The total in a `N tests run` summary line.
fn line_total(lines: &[String]) -> Option<String> {
    lines.iter().rev().find_map(|line| {
        let before = line.split(" tests run").next()?;
        before.split_whitespace().last().map(str::to_owned)
    })
}
