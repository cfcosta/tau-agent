//! A main chat's crew tray (ADR 0031): its sub-agents, docked above the
//! composer, each with where it stands and a way into its chat. Those
//! that ended stay, faded, for the round that spawned them
//! ([`Workspace::crew`]).

use gpui::{Context, Div, SharedString, div, prelude::*, rems};

use crate::{
    assets::Icon,
    crew::{Member, Standing},
    route::Route,
    theme::{Design as _, IconSize, Theme, Type, radius, sp, weight},
    ui,
    view::{RunView, usd},
    workspace::{Workspace, WorkspaceEvent},
};

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
    let mut round = Vec::new();
    if landed > 0 {
        round.push((format!("{landed} landed"), t.change));
    }
    if failed > 0 {
        round.push((format!("{failed} failed"), t.red));
    }
    let head = div()
        .flex()
        .items_center()
        .gap(sp(2.))
        .px(sp(0.5))
        .child(ui::icon(
            Icon::SubAgent,
            IconSize::SMALL,
            if working > 0 { t.roles.live } else { t.dim },
        ))
        .child(div().font_weight(weight::EMPHASIS).child("Crew"))
        .child(div().typeset(Type::CAPTION).text_color(t.dim).child(
            match working {
                0 => "none working".to_owned(),
                n => format!("{n} working"),
            },
        ))
        .child(div().flex_1())
        .child(
            div().flex().gap(sp(1.5)).typeset(Type::CAPTION).children(
                round
                    .into_iter()
                    .map(|(said, ink)| div().text_color(ink).child(said)),
            ),
        );
    // A phone has room for those at work; the head counts the rest.
    let cards = crew
        .into_iter()
        .filter(|member| !compact || member.standing.is_working())
        .map(|member| member_card(member, t, cx));
    Some(
        div()
            .flex()
            .flex_col()
            .gap(sp(2.))
            .p(sp(2.5))
            .border_1()
            .border_color(t.border_strong)
            .rounded(radius::CARD)
            .bg(t.panel)
            .child(head)
            .child(
                div()
                    .grid()
                    .grid_cols(if compact { 1 } else { 3 })
                    .gap(sp(2.))
                    .children(cards),
            ),
    )
}

/// One sub-agent: a mark of where it stands, its title, a line on what
/// it does or how it ended, and the way into its chat.
fn member_card(
    member: Member<'_>,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let run = member.run;
    let ended = !member.standing.is_working();
    let (mark, line, ink) = match &member.standing {
        Standing::Working { turn, doing, cost } => {
            let mut line = format!("turn {turn}");
            if let Some(doing) = doing {
                line.push_str(" · ");
                line.push_str(doing);
            }
            line.push_str(" · ");
            line.push_str(&usd(*cost));
            (
                div()
                    .size(rems(0.375))
                    .rounded_full()
                    .bg(t.roles.live)
                    .into_any_element(),
                line,
                t.dim,
            )
        }
        Standing::WaitingToLand => (
            ui::icon(Icon::Clock, IconSize::TINY, t.roles.waiting)
                .into_any_element(),
            "finished · waits to land".to_owned(),
            t.dim,
        ),
        Standing::Landed { changes } => (
            ui::icon(Icon::Merge, IconSize::TINY, t.change).into_any_element(),
            match changes {
                0 => "landed · no changes".to_owned(),
                1 => "landed · 1 change".to_owned(),
                n => format!("landed · {n} changes"),
            },
            t.dim,
        ),
        Standing::Failed => (
            ui::icon(Icon::Close, IconSize::TINY, t.red).into_any_element(),
            "failed · changes dropped".to_owned(),
            t.red,
        ),
        Standing::Stopped => (
            ui::icon(Icon::Close, IconSize::TINY, t.dim).into_any_element(),
            "stopped · changes dropped".to_owned(),
            t.dim,
        ),
    };
    let open = run.id.clone();
    let peek = ui::text_link(
        match member.standing {
            Standing::Working { .. } => "Peek",
            Standing::Failed => "Read why",
            Standing::Stopped => "Open",
            _ => "Read its answer",
        },
        Type::SMALL,
        t,
    )
    .id(SharedString::from(format!("crew-open-{}", run.id)))
    .on_click(cx.listener(move |ws, _, _, cx| {
        ws.navigate(Route::Run(open.clone()), cx)
    }));
    let stop = member.standing.is_working().then(|| {
        let stop = run.id.clone();
        let hover = t.text;
        div()
            .id(SharedString::from(format!("crew-stop-{}", run.id)))
            .typeset(Type::SMALL)
            .text_color(t.text_soft)
            .cursor_pointer()
            .hover(move |style| style.text_color(hover))
            .child("Stop")
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(WorkspaceEvent::Cancel { run: stop.clone() })
            }))
    });
    div()
        .flex()
        .flex_col()
        .gap(sp(1.))
        .min_w(rems(0.))
        .px(sp(2.5))
        .py(sp(2.))
        .border_1()
        .border_color(t.border)
        .rounded(radius::BOX)
        .bg(t.bg)
        .when(ended, |card| card.opacity(0.62))
        .child(
            div().flex().items_center().gap(sp(1.5)).child(mark).child(
                div()
                    .flex_1()
                    .min_w(rems(0.))
                    .truncate()
                    .text_color(if ended { t.text_soft } else { t.text })
                    .child(run.title.clone()),
            ),
        )
        .child(div().min_w(rems(0.)).truncate().child(ui::mono(
            line,
            Type::MICRO,
            ink,
        )))
        .child(div().flex().gap(sp(3.)).child(peek).children(stop))
}
