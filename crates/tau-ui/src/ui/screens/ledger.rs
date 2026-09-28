//! A run's context, before and after the last rewrite, and what pruning
//! decided for each tool call.

use gpui::{AnyElement, Context, SharedString, div, prelude::*, px, relative};
use tau_agent::tool::RunId;

use crate::{
    route::Route,
    theme::{Design as _, Theme, Type, radius, sp},
    ui::{self, bar, heading, key_values, mono},
    view::{Decision, tokens},
    workspace::Workspace,
};

pub fn render(
    ws: &Workspace,
    run: &RunId,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let Some(run) = ws.run(run) else {
        return ui::empty("That run is not in this workspace.", t)
            .into_any_element();
    };
    let Some((plugin, before, after, detail)) = run.last_rewrite() else {
        return ui::screen(
            "ledger",
            compact,
            ui::empty(
                format!(
                    "No plugin has rewritten the context of {} yet.",
                    run.title
                ),
                t,
            ),
        )
        .into_any_element();
    };
    let window = run.context.window.unwrap_or(before.max(1));
    let trigger = run.context.trigger;
    let meter = |label: &'static str, used: u64, fill| {
        div()
            .flex()
            .items_center()
            .gap(sp(3.))
            .child(
                div()
                    .w(px(56.))
                    .typeset(Type::CAPTION)
                    .text_color(t.muted)
                    .child(label),
            )
            .child(
                div()
                    .flex_1()
                    .relative()
                    .child(bar(
                        used as f32 / window as f32,
                        14.,
                        fill,
                        t.raised,
                    ))
                    .children(trigger.map(|trigger| {
                        div()
                            .absolute()
                            .top(px(-4.))
                            .left(relative(trigger))
                            .w(px(2.))
                            .h(px(22.))
                            .bg(t.accent)
                    })),
            )
            .child(
                mono(tokens(used), Type::CAPTION, t.text)
                    .w(px(48.))
                    .flex()
                    .justify_end(),
            )
    };
    let count = |decision: Decision| {
        run.ledger
            .iter()
            .filter(|entry| entry.decision == decision)
            .count()
    };
    let saved = before.saturating_sub(after);

    let table_rows = run.ledger.iter().map(|entry| {
        let (ink, fill) = match entry.decision {
            Decision::Pinned => (t.text, t.text),
            Decision::Keep => (t.text_soft, gpui::transparent_black()),
            Decision::DropResult => (t.blue, t.blue),
            Decision::DropCall => (t.dim, t.border_strong),
        };
        let odds =
            |p: Option<f32>| p.map_or("·".to_owned(), |p| format!("{p:.2}"));
        let route = Route::Run(run.id.clone());
        let decision = div()
            .flex()
            .items_center()
            .gap(sp(1.5))
            .typeset(Type::CAPTION)
            .text_color(ink)
            .child(
                div()
                    .size(px(8.))
                    .rounded(radius::HAIRLINE)
                    .bg(fill)
                    .border_1()
                    .border_color(ink),
            )
            .child(entry.decision.label());
        let input = mono(
            entry.input.clone(),
            Type::CAPTION,
            if entry.decision == Decision::DropCall {
                t.dim
            } else {
                t.text_soft
            },
        )
        .flex_1()
        .min_w(px(0.))
        .truncate();
        div()
            .id(SharedString::from(format!("ledger-{}", entry.call_id)))
            .flex()
            .items_center()
            .gap(sp(3.))
            .px(sp(4.))
            .py(sp(2.))
            .border_b_1()
            .border_color(t.border)
            .cursor_pointer()
            .hover(|style| style.bg(gpui::white().opacity(0.03)))
            .when(!compact, |row| {
                row.child(
                    mono(format!("t{}", entry.turn), Type::CAPTION, t.dim)
                        .w(px(40.)),
                )
            })
            .child(
                mono(entry.tool.clone(), Type::CAPTION, t.blue)
                    .w(px(if compact { 44. } else { 88. })),
            )
            .child(input)
            .when(!compact, |row| {
                row.child(
                    mono(tokens(entry.tokens), Type::CAPTION, t.muted)
                        .w(px(64.))
                        .flex()
                        .justify_end(),
                )
                .child(
                    mono(odds(entry.matters), Type::CAPTION, t.text)
                        .w(px(96.))
                        .flex()
                        .justify_end(),
                )
                .child(
                    mono(odds(entry.verbatim), Type::CAPTION, t.text)
                        .w(px(96.))
                        .flex()
                        .justify_end(),
                )
            })
            .child(
                div()
                    .w(px(if compact { 96. } else { 120. }))
                    .child(decision),
            )
            .on_click(
                cx.listener(move |ws, _, _, cx| ws.navigate(route.clone(), cx)),
            )
    });
    let head = |label: &str, width: f32, right: bool| {
        div()
            .w(px(width))
            .when(right, |cell| cell.flex().justify_end())
            .child(heading(label, t))
    };
    let table = ui::card(t)
        .when(!compact, |card| {
            card.child(
                div()
                    .flex()
                    .gap(sp(3.))
                    .px(sp(4.))
                    .py(sp(2.25))
                    .bg(t.panel)
                    .border_b_1()
                    .border_color(t.border)
                    .child(head("Turn", 40., false))
                    .child(head("Tool", 88., false))
                    .child(div().flex_1().child(heading("Input", t)))
                    .child(head("Size", 64., true))
                    .child(head("Still matters", 96., true))
                    .child(head("Verbatim", 96., true))
                    .child(head("Decision", 120., false)),
            )
        })
        .children(table_rows)
        .when(run.ledger.is_empty(), |card| {
            card.child(ui::empty(
                "The plugin reported no per-call decisions.",
                t,
            ))
        });

    let summary = key_values(
        [
            (
                "plugin".into(),
                mono(plugin.to_owned(), Type::CAPTION, t.text),
            ),
            (
                "pinned".into(),
                mono(
                    count(Decision::Pinned).to_string(),
                    Type::CAPTION,
                    t.text,
                ),
            ),
            (
                "kept".into(),
                mono(count(Decision::Keep).to_string(), Type::CAPTION, t.text),
            ),
            (
                "result dropped".into(),
                mono(
                    count(Decision::DropResult).to_string(),
                    Type::CAPTION,
                    t.text,
                ),
            ),
            (
                "call dropped".into(),
                mono(
                    count(Decision::DropCall).to_string(),
                    Type::CAPTION,
                    t.text,
                ),
            ),
            (
                "next request".into(),
                mono("1 full resend", Type::CAPTION, t.text),
            ),
        ],
        t,
    )
    .p(sp(4.))
    .border_1()
    .border_color(t.border)
    .rounded(radius::BOX)
    .when(!compact, |card| card.w(px(300.)).flex_shrink_0());

    ui::screen(
        "ledger",
        compact,
        div()
            .flex()
            .flex_col()
            .gap(sp(5.))
            .child(ui::screen_title(
                format!("Pruned {} tokens of {}'s tool history", tokens(saved), run.title),
                format!(
                    "{}Decisions only escalate: keep, then drop the result, then drop the call. Forks inherit this ledger.",
                    detail.map_or(String::new(), |detail| format!("{detail}. "))
                ),
                t,
            ))
            .child(
                div()
                    .flex()
                    .when(compact, |layout| layout.flex_col())
                    .gap(sp(7.))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(sp(3.))
                            .child(meter("Before", before, t.slate))
                            .child(meter("After", after, t.blue))
                            .child(
                                div().typeset(Type::CAPTION).text_color(t.muted).child(format!(
                                    "Window {}.{}",
                                    tokens(window),
                                    trigger.map_or(String::new(), |trigger| format!(" Pruning starts at {:.0}%.", trigger * 100.))
                                )),
                            ),
                    )
                    .child(summary),
            )
            .child(heading("Ledger", t))
            .child(table),
    )
    .into_any_element()
}
