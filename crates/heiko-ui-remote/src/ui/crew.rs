//! A main chat's crew tray (ADR 0031): its sub-agents, docked above the
//! composer, one lane each, with where it stands and a way into its
//! chat. Those that ended stay, faded, for the round that spawned them
//! ([`Workspace::crew`]).

use gpui::{Context, Div, SharedString, div, prelude::*, rems};

use crate::{
    assets::State,
    crew::{Member, Standing},
    route::Route,
    theme::{Design as _, Theme, Type, sp},
    ui,
    view::RunView,
    workspace::{Workspace, WorkspaceEvent},
};

/// How many ticks a lane has: one lit for each turn its sub-agent took,
/// up to all of them.
const LANE_TICKS: usize = 24;

/// The tray under `run`'s transcript, when it is a main chat with a
/// crew this round.
pub fn tray(
    ws: &Workspace,
    run: &RunView,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Option<Div> {
    if !ws.is_main(&run.id) {
        return None;
    }
    let mut crew = ws.crew(run);
    if crew.is_empty() {
        return None;
    }
    // Those at work first; the round's ended ones after, faded.
    crew.sort_by_key(|member| !member.standing.is_working());
    let working = crew.iter().filter(|m| m.standing.is_working()).count();
    let landed = crew
        .iter()
        .filter(|m| matches!(m.standing, Standing::Landed { .. }))
        .count();
    let failed = crew
        .iter()
        .filter(|m| matches!(m.standing, Standing::Failed))
        .count();
    let mut round = vec![match working {
        0 => "none working".to_owned(),
        n => format!("{n} working"),
    }];
    if landed > 0 {
        round.push(format!("{landed} landed"));
    }
    if failed > 0 {
        round.push(format!("{failed} failed"));
    }
    // A phone has room for those at work; the head counts the rest.
    let lanes: Vec<Div> = crew
        .into_iter()
        .filter(|member| !compact || member.standing.is_working())
        .map(|member| lane(member, compact, t, cx))
        .collect();
    // Its head is smaller than a section's, with heiko's mark and the
    // round's count beside the title.
    let head = div()
        .flex()
        .items_center()
        .gap(sp(2.5))
        .pb(sp(2.))
        .border_b_2()
        .border_color(t.rule)
        .child(ui::logo(t, 20.))
        .child(
            div()
                .typeset(Type::SECTION.sized(17.))
                .text_color(t.bright)
                .child("Crew"),
        )
        .child(
            div()
                .min_w(rems(0.))
                .truncate()
                .text_color(t.dim)
                .child(round.join(" · ")),
        );
    Some(div().flex().flex_col().child(head).children(lanes))
}

/// One sub-agent's lane: the mark of where it stands, its title, its
/// turns as ticks, its turn and what it does or how it ended, and the
/// ways into its chat and to stop it.
fn lane(
    member: Member<'_>,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let run = member.run;
    let ended = !member.standing.is_working();
    // What it does now: its tool's name, in the tool's color, and the
    // rest of the call's line.
    let doing = match &member.standing {
        Standing::Working {
            doing: Some(doing), ..
        } => Some(match doing.split_once(' ') {
            Some((tool, rest)) => (tool.to_owned(), format!(" {rest}")),
            None => (doing.clone(), String::new()),
        }),
        _ => None,
    };
    let (state, turns, line) = match &member.standing {
        Standing::Working { turn, doing, .. } => (
            State::Working,
            Some(*turn),
            match doing {
                Some(_) => format!("turn {turn} · "),
                None => format!("turn {turn}"),
            },
        ),
        Standing::WaitingToLand => {
            (State::Idle, None, "finished · waits to land".to_owned())
        }
        Standing::Landed { changes } => (
            State::Met,
            None,
            match changes {
                0 => "landed · no changes".to_owned(),
                1 => "landed · 1 change".to_owned(),
                n => format!("landed · {n} changes"),
            },
        ),
        Standing::Failed => {
            (State::Failing, None, "failed · changes dropped".to_owned())
        }
        Standing::Stopped => {
            (State::Idle, None, "stopped · changes dropped".to_owned())
        }
    };
    let open = run.id.clone();
    let peek = div()
        .id(SharedString::from(format!("crew-open-{}", run.id)))
        .child(ui::link_action(
            match member.standing {
                Standing::Working { .. } => "peek",
                Standing::Failed => "read why",
                Standing::Stopped => "open",
                _ => "read its answer",
            },
            t,
        ))
        .on_click(cx.listener(move |ws, _, _, cx| {
            ws.navigate(Route::Run(open.clone()), cx)
        }));
    let stop = member.standing.is_working().then(|| {
        let stop = run.id.clone();
        div()
            .id(SharedString::from(format!("crew-stop-{}", run.id)))
            .child(ui::link_action("stop", t))
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(WorkspaceEvent::Cancel { run: stop.clone() })
            }))
    });
    let title = div()
        .min_w(rems(0.))
        .truncate()
        .text_color(if ended { t.text_soft } else { t.text })
        .child(run.title.clone());
    let tone = if matches!(member.standing, Standing::Failed) {
        t.red
    } else {
        t.dim
    };
    let said = match doing {
        Some((tool, rest)) => {
            let color = t.roles.tool(&tool);
            div()
                .min_w(rems(0.))
                .flex()
                .child(
                    ui::mono(line, Type::CAPTION, tone)
                        .flex_shrink_0()
                        .whitespace_nowrap(),
                )
                .child(
                    ui::mono(tool, Type::CAPTION, color)
                        .flex_shrink_0()
                        .whitespace_nowrap(),
                )
                .child(
                    ui::mono(rest, Type::CAPTION, t.dim)
                        .flex_1()
                        .min_w(rems(0.))
                        .truncate(),
                )
        }
        None => ui::mono(line, Type::CAPTION, tone)
            .min_w(rems(0.))
            .truncate(),
    };
    let actions = div()
        .flex()
        .flex_shrink_0()
        .gap(sp(3.))
        .child(peek)
        .children(stop);
    let lane = div()
        .flex()
        .items_center()
        .gap(sp(3.))
        .min_h(rems(2.125))
        .border_b_1()
        .border_color(t.hairline)
        .typeset(Type::SMALL)
        .when(ended, |lane| lane.opacity(0.62))
        .child(ui::mark(state, t));
    if compact {
        // A phone's lane: the title, then its line under it.
        return lane
            .py(sp(1.5))
            .child(
                div()
                    .flex_1()
                    .min_w(rems(0.))
                    .flex()
                    .flex_col()
                    .child(title)
                    .child(said),
            )
            .child(actions);
    }
    // The title has a column, the line another; the ticks take what is
    // left between them.
    lane.child(title.w(rems(13.5)).flex_shrink_0())
        .child(div().flex_1().min_w(rems(4.)).children(turns.map(|turn| {
            ui::ticks(
                (turn as usize).min(LANE_TICKS),
                LANE_TICKS,
                t.roles.tool_run,
                t,
            )
        })))
        .child(div().w(rems(15.5)).flex_shrink_0().child(said))
        .child(actions)
}
