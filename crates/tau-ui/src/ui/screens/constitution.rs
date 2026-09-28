//! The rules tau-constitution checks, and the calls it blocked or flagged
//! across the workspace's runs.

use gpui::{AnyElement, Context, SharedString, div, prelude::*, px, relative};

use crate::{
    assets::Icon,
    route::Route,
    theme::{Design as _, IconSize, Theme, Type, radius, sp},
    ui::{self, components::ButtonKind, heading, icon, mono},
    view::ToolState,
    workspace::Workspace,
};

pub fn render(
    ws: &Workspace,
    repo: &str,
    focus: Option<&str>,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let constitution = &ws.repo_named(repo).constitution;
    // (run, card, rule) for every call a rule touched.
    let reviews: Vec<_> = ws
        .runs
        .iter()
        .filter(|run| ws.repo_of(run) == repo)
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
        .gap(sp(2.5))
        .child(heading(&format!("Review queue · {}", queue.len()), t))
        .child(
            div()
                .typeset(Type::CAPTION)
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
                .gap(sp(2.5))
                .p(sp(3.))
                .border_1()
                .border_color(t.accent_border)
                .rounded(radius::BOX)
                .bg(t.card)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(2.))
                        .child(icon(Icon::Warning, IconSize::BASE, t.accent))
                        .child(mono(card.tool.clone(), Type::CAPTION, t.blue))
                        .child(mono(card.summary.clone(), Type::CAPTION, t.text_soft).flex_1().min_w(px(0.)).truncate())
                        .child(mono(format!("{rule} {score}"), Type::CAPTION, t.accent)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(2.))
                        .child(div().flex_1().typeset(Type::CAPTION).text_color(t.muted).child(format!("in {}", run.title)))
                        .child(
                            div()
                                .id(("review-open", n))
                                .child(ui::button("Open run", ButtonKind::Secondary, t))
                                .on_click(cx.listener(move |ws, _, _, cx| ws.navigate(open.clone(), cx))),
                        )
                        .child(
                            div()
                                .id(("review-dismiss", n))
                                .child(ui::button("Looks fine", ButtonKind::Secondary, t))
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
        .gap(sp(2.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(2.5))
                .child(heading(
                    &format!("Rules · {}", constitution.rules.len()),
                    t,
                ))
                .child(div().flex_1())
                .child(mono(file_name(&constitution.path), Type::CAPTION, t.dim)),
        )
        .when(constitution.rules.is_empty() && constitution.error.is_none(), |list| {
            list.child(ui::empty(
                "No rules yet. Add one below: tau checks it on every run in this repository.",
                t,
            ))
        })
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
                repo: repo.to_owned(),
                rule: Some(rule.id.clone()),
            };
            let (remove_repo, remove_id) = (repo.to_owned(), rule.id.clone());
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
                .gap(sp(1.5))
                .p(sp(3.))
                .border_1()
                .border_color(if focused {
                    t.accent
                } else if hot {
                    t.red_border
                } else {
                    t.border
                })
                .rounded(radius::BOX)
                .cursor_pointer()
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap(sp(2.))
                        .child(mono(rule.id.clone(), Type::CAPTION, t.muted))
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
                        .gap(sp(1.5))
                        .children(rule.applies_to.iter().map(|field| {
                            crate::ui::tag(
                                field.clone(),
                                Type::MICRO,
                                t.text_soft,
                                t,
                            )
                        }))
                        .child(div().flex_1())
                        .child(mono(
                            format!(
                                "review {:.2} · block {:.2}",
                                rule.review, rule.block
                            ),
                            Type::MICRO,
                            t.dim,
                        )),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .flex_1()
                                .typeset(Type::CAPTION)
                                .text_color(if blocked > 0 { t.red } else { t.dim })
                                .child(stat),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("remove-{}", rule.id)))
                                .child(ui::text_link("Remove", Type::CAPTION, t))
                                .on_click(cx.listener(move |ws, _, _, cx| {
                                    cx.stop_propagation();
                                    ws.remove_rule(&remove_repo, &remove_id, cx)
                                })),
                        ),
                )
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.navigate(route.clone(), cx)
                }))
        }))
        .child(new_rule(ws, repo, t, cx));

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
            .gap(sp(5.))
            .child(ui::screen_title(
                "Constitution",
                format!(
                    "{blocked} calls blocked and {} flagged across {repo}'s runs. Checks read only what the model wrote, never tool output. A final answer that breaks a rule goes back {} times at most.",
                    reviews.len() - blocked,
                    constitution.max_continuations
                ),
                t,
            ))
            .when_some(constitution.error.clone(), |screen, error| {
                screen.child(ui::notice(
                    Icon::Warning,
                    format!("Runs in {repo} stop at start until this is fixed: {error}"),
                    t.red,
                    Type::SMALL,
                    t,
                ))
            })
            .when(!ws.catalog.models.access.jev, |screen| {
                screen.child(
                    div()
                        .id("jev-missing")
                        .cursor_pointer()
                        .child(ui::notice(
                            Icon::Key,
                            "Rules are not checked yet: tau asks Jev, which needs a TypeSafe key. Add one on the Models screen.",
                            t.accent,
                            Type::SMALL,
                            t,
                        ))
                        .on_click(cx.listener(|ws, _, _, cx| ws.navigate(Route::Models, cx))),
                )
            })
            .child(
                div()
                    .flex()
                    .when(compact, |layout| layout.flex_col())
                    .gap(sp(6.))
                    .child(div().flex_1().min_w(px(0.)).child(rules))
                    .child(div().when(!compact, |side| side.w(px(420.)).flex_shrink_0()).child(queue_view)),
            ),
    )
    .into_any_element()
}

/// The last part of a path, for a label.
fn file_name(path: &str) -> String {
    std::path::Path::new(path).file_name().map_or_else(
        || path.to_owned(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// The form that adds a rule: its words, and where it applies.
fn new_rule(
    ws: &Workspace,
    repo: &str,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let repo = repo.to_owned();
    div()
        .flex()
        .flex_col()
        .gap(sp(2.))
        .mt(sp(2.))
        .p(sp(3.))
        .border_1()
        .border_dashed()
        .border_color(t.border_strong)
        .rounded(radius::BOX)
        .child(heading("New rule", t))
        .child(ui::field(&ws.rule_text, false, t))
        .child(ui::field(&ws.rule_on, true, t))
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(2.))
                .child(
                    div()
                        .flex_1()
                        .typeset(Type::CAPTION)
                        .text_color(t.dim)
                        .child("Flagged for review at 0.50, blocked at 0.80. Tune them in the file."),
                )
                .child(
                    div()
                        .id("add-rule")
                        .child(ui::button("Add rule", ButtonKind::Primary, t))
                        .on_click(cx.listener(move |ws, _, _, cx| ws.add_rule(&repo, cx))),
                ),
        )
}
