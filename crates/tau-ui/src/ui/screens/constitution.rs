//! The rules tau-constitution checks, and the calls it blocked or flagged
//! across the workspace's runs.

use gpui::{AnyElement, Context, SharedString, div, prelude::*, px, relative};

use crate::{
    assets::Icon,
    route::Route,
    theme::Theme,
    ui::{self, heading, icon, mono},
    view::ToolState,
    workspace::Workspace,
};

pub fn render(
    ws: &Workspace,
    focus: Option<&str>,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let constitution = &ws.catalog.constitution;
    // (run, card, rule) for every call a rule touched.
    let reviews: Vec<_> = ws
        .runs
        .iter()
        .flat_map(|run| run.reviews().map(move |card| (run, card)))
        .filter_map(|(run, card)| match &card.state {
            ToolState::Blocked { rule, .. }
            | ToolState::Flagged { rule, .. } => {
                Some((run, card, rule.as_str()))
            }
            _ => None,
        })
        .collect();
    let queue: Vec<_> = reviews
        .iter()
        .filter(|(run, card, _)| {
            matches!(card.state, ToolState::Flagged { .. })
                && !ws
                    .dismissed
                    .contains(&(run.id.clone(), card.call_id.clone()))
        })
        .collect();

    let queue_view = div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(heading(&format!("Review queue · {}", queue.len()), t))
        .child(
            div()
                .text_size(px(12.))
                .text_color(t.dim)
                .child("Calls that ran with a score between a rule's review and block thresholds."),
        )
        .when(queue.is_empty(), |queue| queue.child(ui::empty("Nothing to review.", t)))
        .children(queue.iter().enumerate().map(|(n, (run, card, rule))| {
            let score = match &card.state {
                ToolState::Flagged { score, .. } => score.clone(),
                _ => String::new(),
            };
            let open = Route::Run(run.id.clone());
            let dismiss = (run.id.clone(), card.call_id.clone());
            div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .p(px(12.))
                .border_1()
                .border_color(t.accent_border)
                .rounded(px(8.))
                .bg(t.card)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(icon(Icon::Warning, 14., t.accent))
                        .child(mono(card.tool.clone(), 12., t.blue))
                        .child(mono(card.summary.clone(), 12., t.text_soft).flex_1().min_w(px(0.)).truncate())
                        .child(mono(format!("{rule} {score}"), 12., t.accent)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(div().flex_1().text_size(px(12.)).text_color(t.muted).child(format!("in {}", run.title)))
                        .child(
                            div()
                                .id(("review-open", n))
                                .child(ui::button("Open run", t))
                                .on_click(cx.listener(move |ws, _, _, cx| ws.navigate(open.clone(), cx))),
                        )
                        .child(
                            div()
                                .id(("review-dismiss", n))
                                .child(ui::button("Looks fine", t))
                                .on_click(cx.listener(move |ws, _, _, cx| {
                                    ws.dismissed.insert(dismiss.clone());
                                    cx.notify();
                                })),
                        ),
                )
        }));

    let rules = div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(heading(
                    &format!("Rules · {}", constitution.rules.len()),
                    t,
                ))
                .child(div().flex_1())
                .child(mono(constitution.path.clone(), 12., t.dim)),
        )
        .children(constitution.rules.iter().map(|rule| {
            let blocked = reviews
                .iter()
                .filter(|(_, card, id)| {
                    *id == rule.id
                        && matches!(card.state, ToolState::Blocked { .. })
                })
                .count();
            let flagged = reviews
                .iter()
                .filter(|(_, card, id)| {
                    *id == rule.id
                        && matches!(card.state, ToolState::Flagged { .. })
                })
                .count();
            let focused = focus == Some(rule.id.as_str());
            let hot = blocked > 0 || focused;
            let route = Route::Constitution {
                rule: Some(rule.id.clone()),
            };
            let stat = match (blocked, flagged) {
                (0, 0) => "quiet".to_owned(),
                (b, 0) => format!("{b} blocked"),
                (0, f) => format!("{f} flagged"),
                (b, f) => format!("{b} blocked · {f} flagged"),
            };
            div()
                .id(SharedString::from(format!("rule-{}", rule.id)))
                .flex()
                .flex_col()
                .gap(px(6.))
                .p(px(12.))
                .border_1()
                .border_color(if focused {
                    t.accent
                } else if hot {
                    t.red_border
                } else {
                    t.border
                })
                .rounded(px(8.))
                .cursor_pointer()
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap(px(8.))
                        .child(mono(rule.id.clone(), 12., t.muted))
                        .child(
                            div()
                                .flex_1()
                                .line_height(relative(1.4))
                                .child(rule.text.clone()),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(6.))
                        .children(rule.applies_to.iter().map(|field| {
                            mono(field.clone(), 11., t.text_soft)
                                .px(px(6.))
                                .py(px(2.))
                                .rounded(px(4.))
                                .bg(t.raised)
                        }))
                        .child(div().flex_1())
                        .child(mono(
                            format!(
                                "review {:.2} · block {:.2}",
                                rule.review, rule.block
                            ),
                            11.,
                            t.dim,
                        )),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(if blocked > 0 { t.red } else { t.dim })
                        .child(stat),
                )
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.navigate(route.clone(), cx)
                }))
        }));

    let blocked = reviews
        .iter()
        .filter(|(_, card, _)| matches!(card.state, ToolState::Blocked { .. }))
        .count();
    ui::screen(
        "constitution",
        compact,
        div()
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(ui::screen_title(
                "Constitution",
                format!(
                    "{blocked} calls blocked and {} flagged across these runs. Checks read only what the model wrote, never tool output. A held stop may continue the run {} times at most.",
                    reviews.len() - blocked,
                    constitution.max_continuations
                ),
                t,
            ))
            .child(
                div()
                    .flex()
                    .when(compact, |layout| layout.flex_col())
                    .gap(px(24.))
                    .child(div().flex_1().min_w(px(0.)).child(rules))
                    .child(div().when(!compact, |side| side.w(px(420.)).flex_shrink_0()).child(queue_view)),
            ),
    )
    .into_any_element()
}
