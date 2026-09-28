//! The rules tau-constitution checks in a repository: what each reads,
//! how strict it is and what it did, the calls and answers that wait for
//! a person, and the editor that writes and tries a rule.

use gpui::{AnyElement, Context, Div, Hsla, SharedString, div, prelude::*, px};

use crate::{
    assets::Icon,
    catalog::{Constitution, Rule},
    route::Route,
    rule_editor::{
        HandledKind,
        Mark,
        PLACES,
        Preset,
        ReviewItem,
        RulesStats,
        RulesTab,
        Trying,
    },
    theme::{Design as _, IconSize, Theme, Type, radius, sp, weight},
    ui::{self, components::ButtonKind, heading, icon, mono},
    view::usd,
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
    let constitution = ws.repo_named(repo).constitution.clone();
    let stats = ws.rules_stats(repo);
    let jev = ws.catalog.models.access.jev;
    let empty = constitution.rules.is_empty() && constitution.error.is_none();
    let content = div()
        .flex()
        .flex_col()
        .gap(sp(4.5))
        .child(header(repo, empty, compact, t, cx))
        .when_some(broken(&constitution, repo, t, cx), |screen, banner| {
            screen.child(banner)
        })
        .when(!jev && constitution.error.is_none(), |screen| {
            screen.child(no_key(compact, t, cx))
        })
        .when(empty, |screen| screen.child(nothing_yet(repo, t, cx)))
        .when(constitution.error.is_some(), |screen| {
            screen.children(last_good(ws, repo, compact, t))
        })
        .when(!empty && constitution.error.is_none(), |screen| {
            screen
                .child(stat_tiles(&stats, compact, t))
                .when(
                    stats.waiting > 0 && ws.rules_tab() == RulesTab::Rules,
                    |screen| {
                        screen.child(waiting_strip(ws, repo, &stats, t, cx))
                    },
                )
                .child(tabs(ws, &constitution, &stats, t, cx))
                .child(match ws.rules_tab() {
                    RulesTab::Rules => rules_list(
                        ws,
                        repo,
                        &constitution,
                        &stats,
                        focus,
                        compact,
                        t,
                        cx,
                    )
                    .into_any_element(),
                    RulesTab::Review => {
                        review(ws, repo, &constitution, compact, t, cx)
                            .into_any_element()
                    }
                })
                .when(ws.rules_tab() == RulesTab::Rules && !compact, |screen| {
                    screen.child(legend(t))
                })
        });
    div()
        .relative()
        .flex_1()
        .min_h(px(0.))
        .flex()
        .flex_col()
        .child(ui::screen("constitution", compact, content))
        .when_some(editor(ws, compact, t, cx), |screen, editor| {
            screen.child(editor)
        })
        .into_any_element()
}

/// The title, the repository, and what can be done: edit the file, add
/// a rule.
fn header(
    repo: &str,
    empty: bool,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let (file_repo, new_repo) = (repo.to_owned(), repo.to_owned());
    div()
        .flex()
        .items_start()
        .gap(sp(4.))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(sp(1.5))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(2.5))
                        .child(
                            div()
                                .typeset(Type::TITLE)
                                .font_weight(weight::STRONG)
                                .child("Constitution"),
                        )
                        .child(ui::tag(
                            repo.to_owned(),
                            Type::CAPTION,
                            t.text_soft,
                            t,
                        )),
                )
                .when(!compact, |title| {
                    title.child(ui::text(
                        "Rules Jev checks on what the model writes: tool calls \
                         and final answers. Changes apply to the next run.",
                        Type::BODY,
                        t.muted,
                    ))
                }),
        )
        .when(!compact, |row| {
            row.child(
                div()
                    .id("edit-constitution")
                    .child(ui::button(
                        "constitution.toml",
                        ButtonKind::Secondary,
                        t,
                    ))
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.edit_constitution(&file_repo, cx)
                    })),
            )
        })
        .when(!empty, |row| {
            row.child(
                div()
                    .id("new-rule")
                    .child(ui::button("New rule", ButtonKind::Primary, t))
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.open_rule_editor(&new_repo, None, cx)
                    })),
            )
        })
}

/// A constitution that does not read: why, where, and what to do.
fn broken(
    constitution: &Constitution,
    repo: &str,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Option<Div> {
    let error = constitution.error.clone()?;
    let repo = repo.to_owned();
    let lines = constitution.excerpt.iter().map(|(n, line)| {
        let at = constitution.error_line == Some(*n);
        div()
            .flex()
            .gap(sp(3.))
            .child(mono(format!("{n:>3}"), Type::CAPTION, t.dim))
            .child(mono(
                line.clone(),
                Type::CAPTION,
                if at { t.red } else { t.text_soft },
            ))
    });
    Some(
        div()
            .flex()
            .gap(sp(3.5))
            .p(sp(4.))
            .rounded(radius::BOX)
            .bg(t.red_soft)
            .border_1()
            .border_color(t.red_border)
            .child(icon(Icon::Blocked, IconSize::LARGE, t.red))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .gap(sp(2.))
                    .child(
                        div()
                            .font_weight(weight::STRONG)
                            .child(format!(
                                "constitution.toml doesn't read, so runs in {repo} fail at start"
                            )),
                    )
                    .child(ui::text(error, Type::SMALL, t.text_soft))
                    .when(!constitution.excerpt.is_empty(), |banner| {
                        banner.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(sp(0.5))
                                .px(sp(3.5))
                                .py(sp(2.5))
                                .rounded(radius::CONTROL)
                                .bg(t.bg)
                                .children(lines),
                        )
                    })
                    .child(
                        div().flex().gap(sp(2.)).child(
                            div()
                                .id("open-constitution")
                                .child(ui::button(
                                    "Open constitution.toml",
                                    ButtonKind::Primary,
                                    t,
                                ))
                                .on_click(cx.listener(move |ws, _, _, cx| {
                                    ws.edit_constitution(&repo, cx)
                                })),
                        ),
                    ),
            ),
    )
}

/// While the file is broken: the rules as they were at its last good
/// read, dimmed, for reference.
fn last_good(
    ws: &Workspace,
    repo: &str,
    compact: bool,
    t: &Theme,
) -> Option<Div> {
    let rules = ws
        .last_good_rules
        .get(repo)
        .filter(|rules| !rules.is_empty())?;
    Some(
        div()
            .flex()
            .flex_col()
            .gap(sp(2.5))
            .child(ui::text(
                "The rules as they were at the last good read, for reference.",
                Type::SMALL,
                t.dim,
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .opacity(0.5)
                    .rounded(radius::BOX)
                    .border_1()
                    .border_color(t.border)
                    .bg(t.card)
                    .children(rules.iter().map(|rule| {
                        div()
                            .flex()
                            .items_center()
                            .gap(sp(4.))
                            .px(sp(4.))
                            .py(sp(3.))
                            .border_b_1()
                            .border_color(t.border)
                            .child(
                                mono(rule.id.clone(), Type::CAPTION, t.muted)
                                    .w(px(36.)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .child(rule.text.clone()),
                            )
                            .when(!compact, |row| {
                                row.child(
                                    div()
                                        .w(px(250.))
                                        .flex()
                                        .flex_wrap()
                                        .gap(sp(1.5))
                                        .children(
                                            rule.applies_to
                                                .iter()
                                                .map(|at| place(at, t)),
                                        ),
                                )
                            })
                    })),
            ),
    )
}

/// Without a TypeSafe key nothing is checked: say so, and where to fix it.
fn no_key(compact: bool, t: &Theme, cx: &mut Context<Workspace>) -> Div {
    div()
        .flex()
        .items_center()
        .gap(sp(3.5))
        .p(sp(3.5))
        .rounded(radius::BOX)
        .bg(t.accent_soft)
        .border_1()
        .border_color(t.accent_border)
        .child(icon(Icon::Key, IconSize::LARGE, t.accent))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(sp(0.75))
                .child(div().font_weight(weight::EMPHASIS).child("Rules aren't checked yet"))
                .when(!compact, |text| {
                    text.child(ui::text(
                        "tau-constitution asks Jev, TypeSafe's scoring model. Add \
                         a TypeSafe key and every run checks these rules.",
                        Type::SMALL,
                        t.muted,
                    ))
                }),
        )
        .child(
            div()
                .id("add-jev-key")
                .child(ui::button("Add TypeSafe key", ButtonKind::Primary, t))
                .on_click(cx.listener(|ws, _, window, cx| {
                    ws.ask_for_jev_key(window, cx)
                })),
        )
}

/// No rules: what a rule is, and how to write one.
fn nothing_yet(repo: &str, t: &Theme, cx: &mut Context<Workspace>) -> Div {
    let (new_repo, file_repo) = (repo.to_owned(), repo.to_owned());
    div().flex().justify_center().py(sp(12.)).child(
        div()
            .w(px(520.))
            .max_w_full()
            .flex()
            .flex_col()
            .items_center()
            .gap(sp(3.5))
            .child(
                div()
                    .size(px(48.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(radius::BOX)
                    .bg(t.raised)
                    .child(icon(Icon::Blocked, IconSize::LARGE, t.muted)),
            )
            .child(
                div()
                    .typeset(Type::LEAD)
                    .font_weight(weight::STRONG)
                    .child(format!("No rules for {repo}")),
            )
            .child(
                div().text_center().child(ui::text(
                    "A rule is one sentence, checked where you say: a shell \
                     command, an edit, a written file, or the final answer. Past \
                     its block mark the call is refused and the model gets the \
                     rule back.",
                    Type::BODY,
                    t.muted,
                )),
            )
            .child(
                div()
                    .flex()
                    .gap(sp(2.))
                    .mt(sp(1.5))
                    .child(
                        div()
                            .id("first-rule")
                            .child(ui::button("New rule", ButtonKind::Primary, t))
                            .on_click(cx.listener(move |ws, _, _, cx| {
                                ws.open_rule_editor(&new_repo, None, cx)
                            })),
                    )
                    .child(
                        div()
                            .id("write-constitution")
                            .child(ui::button(
                                "Write constitution.toml",
                                ButtonKind::Secondary,
                                t,
                            ))
                            .on_click(cx.listener(move |ws, _, _, cx| {
                                ws.edit_constitution(&file_repo, cx)
                            })),
                    ),
            ),
    )
}

fn stat_tiles(stats: &RulesStats, compact: bool, t: &Theme) -> Div {
    let tile = |value: String,
                name: &'static str,
                note: Option<String>,
                color: Hsla| {
        div()
            .flex_1()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .gap(sp(1.))
            .px(sp(3.5))
            .py(sp(3.))
            .rounded(radius::BOX)
            .bg(t.card)
            .border_1()
            .border_color(t.border)
            .child(ui::text(name, Type::CAPTION, t.muted))
            .child(mono(value, Type::LEAD, color))
            .when_some(note.filter(|_| !compact), |tile, note| {
                tile.child(ui::text(note, Type::CAPTION, t.dim))
            })
    };
    let runs = format!(
        "in {} {}",
        stats.runs,
        if stats.runs == 1 { "run" } else { "runs" }
    );
    div()
        .flex()
        .gap(sp(2.5))
        .child(tile(
            stats.checked.to_string(),
            "Checked",
            Some(runs),
            t.text,
        ))
        .child(tile(
            stats.blocked.to_string(),
            "Blocked",
            Some("the model got the rule".into()),
            if stats.blocked > 0 { t.red } else { t.text },
        ))
        .child(tile(
            stats.flagged.to_string(),
            "Flagged",
            Some(match stats.waiting {
                0 => "nothing waiting".to_owned(),
                n => format!("{n} waiting for you"),
            }),
            if stats.flagged > 0 { t.accent } else { t.text },
        ))
        .when(!compact, |row| {
            row.child(tile(
                stats.held_runs.to_string(),
                "Answers held",
                Some(format!("of {} runs", stats.runs)),
                t.text,
            ))
            .child(tile(
                if stats.cost > 0.0 && stats.cost < 0.01 {
                    format!("${:.5}", stats.cost)
                } else {
                    usd(stats.cost)
                },
                "Jev cost",
                Some("these runs".into()),
                t.text,
            ))
        })
}

/// Above the rules: something waits for a person.
fn waiting_strip(
    ws: &Workspace,
    repo: &str,
    stats: &RulesStats,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let first = ws.review_items(repo).into_iter().next();
    div()
        .id("waiting")
        .flex()
        .items_center()
        .gap(sp(3.))
        .px(sp(3.5))
        .py(sp(2.75))
        .rounded(radius::BOX)
        .bg(t.accent_soft)
        .border_1()
        .border_color(t.accent_border)
        .cursor_pointer()
        .child(icon(Icon::Warning, IconSize::BASE, t.accent))
        .child(
            div()
                .font_weight(weight::EMPHASIS)
                .child(match stats.waiting {
                    1 => "1 call needs a look".to_owned(),
                    n => format!("{n} calls need a look"),
                }),
        )
        .when_some(first, |strip, item| {
            strip.child(
                mono(
                    format!(
                        "{} {} · {} {:.2} · {}",
                        item.tool.as_deref().unwrap_or("final answer"),
                        item.shown,
                        item.rule,
                        item.score,
                        item.run_title
                    ),
                    Type::CAPTION,
                    t.text_soft,
                )
                .flex_1()
                .min_w(px(0.))
                .truncate(),
            )
        })
        .child(ui::button("Review", ButtonKind::Secondary, t))
        .on_click(
            cx.listener(|ws, _, _, cx| ws.set_rules_tab(RulesTab::Review, cx)),
        )
}

fn tabs(
    ws: &Workspace,
    constitution: &Constitution,
    stats: &RulesStats,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let tab = |which: RulesTab,
               name: &'static str,
               count: usize,
               badge: bool| {
        let on = ws.rules_tab() == which;
        div()
            .id(name)
            .h(px(40.))
            .flex()
            .items_center()
            .gap(sp(1.75))
            .border_b_2()
            .border_color(if on {
                t.accent
            } else {
                gpui::transparent_black()
            })
            .text_color(if on { t.text } else { t.muted })
            .when(on, |tab| tab.font_weight(weight::EMPHASIS))
            .cursor_pointer()
            .child(name)
            .map(|tab| {
                if badge && count > 0 {
                    tab.child(ui::count_pill(count, t))
                } else {
                    tab.child(mono(count.to_string(), Type::CAPTION, t.dim))
                }
            })
            .on_click(
                cx.listener(move |ws, _, _, cx| ws.set_rules_tab(which, cx)),
            )
    };
    div()
        .flex()
        .gap(sp(5.5))
        .border_b_1()
        .border_color(t.border)
        .child(tab(
            RulesTab::Rules,
            "Rules",
            constitution.rules.len(),
            false,
        ))
        .child(tab(RulesTab::Review, "Review", stats.waiting, true))
}

/// Where a rule reads: a tool's field, or the final answer.
fn place(text: &str, t: &Theme) -> Div {
    let glyph = if text == "final answer" {
        Icon::Chat
    } else {
        Icon::Runs
    };
    div()
        .flex()
        .items_center()
        .gap(sp(1.25))
        .flex_shrink_0()
        .px(sp(1.75))
        .py(sp(0.5))
        .rounded(radius::SMALL)
        .bg(t.raised)
        .child(icon(glyph, IconSize::SMALL, t.dim))
        .child(mono(text.to_owned(), Type::MICRO, t.text_soft))
}

/// A rule's last runs in a word: `2 blocked`, `1 flagged`.
fn activity(rule: &Rule, stats: &RulesStats, t: &Theme) -> (String, Hsla) {
    let (blocked, flagged, held) =
        stats.per_rule.get(&rule.id).copied().unwrap_or_default();
    let parts: Vec<String> =
        [(blocked, "blocked"), (held, "held"), (flagged, "flagged")]
            .into_iter()
            .filter(|(n, _)| *n > 0)
            .map(|(n, what)| format!("{n} {what}"))
            .collect();
    let color = if blocked > 0 {
        t.red
    } else if held + flagged > 0 {
        t.accent
    } else {
        t.dim
    };
    if parts.is_empty() {
        ("—".into(), color)
    } else {
        (parts.join(" · "), color)
    }
}

#[allow(clippy::too_many_arguments)]
fn rules_list(
    ws: &Workspace,
    repo: &str,
    constitution: &Constitution,
    stats: &RulesStats,
    focus: Option<&str>,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let rows = constitution.rules.iter().map(|rule| {
        let (act, act_color) = activity(rule, stats, t);
        let focused = focus == Some(rule.id.as_str());
        let menu_open = ws.rule_menu.as_deref() == Some(rule.id.as_str());
        let (edit_repo, edit_id) = (repo.to_owned(), rule.id.clone());
        let menu_id = rule.id.clone();
        let places = div()
            .flex()
            .flex_wrap()
            .gap(sp(1.5))
            .children(rule.applies_to.iter().map(|at| place(at, t)));
        let gauge = div()
            .flex()
            .flex_col()
            .gap(sp(1.5))
            .child(ui::strictness(
                rule.review.into(),
                rule.block.into(),
                None,
                if compact { None } else { Some(180.) },
                false,
                t,
            ))
            .child(mono(
                format!("flag {:.2} · block {:.2}", rule.review, rule.block),
                Type::MICRO,
                t.dim,
            ));
        let row = div()
            .id(SharedString::from(format!("rule-{}", rule.id)))
            .relative()
            .cursor_pointer()
            .border_b_1()
            .border_color(t.border)
            .when(focused || menu_open, |row| row.bg(t.selected))
            .hover(|style| style.bg(t.selected))
            .on_click(cx.listener(move |ws, _, _, cx| {
                ws.open_rule_editor(&edit_repo, Some(&edit_id), cx)
            }));
        let row = if compact {
            row.flex()
                .flex_col()
                .gap(sp(2.25))
                .px(sp(4.))
                .py(sp(3.5))
                .child(
                    div()
                        .flex()
                        .gap(sp(2.5))
                        .child(mono(rule.id.clone(), Type::CAPTION, t.muted))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .child(rule.text.clone()),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(2.))
                        .pl(sp(7.5))
                        .child(places)
                        .child(div().flex_1())
                        .child(mono(act, Type::MICRO, act_color)),
                )
                .child(div().pl(sp(7.5)).child(gauge))
        } else {
            row.flex()
                .items_center()
                .gap(sp(4.))
                .px(sp(4.))
                .py(sp(3.5))
                .child(
                    mono(rule.id.clone(), Type::CAPTION, t.muted)
                        .w(px(36.))
                        .flex_shrink_0(),
                )
                .child(div().flex_1().min_w(px(0.)).child(rule.text.clone()))
                .child(places.w(px(250.)).flex_shrink_0())
                .child(gauge.w(px(190.)).flex_shrink_0())
                .child(
                    mono(act, Type::CAPTION, act_color)
                        .w(px(110.))
                        .flex_shrink_0(),
                )
                .child(
                    div()
                        .id(SharedString::from(format!(
                            "rule-menu-{}",
                            rule.id
                        )))
                        .size(px(30.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(radius::CONTROL)
                        .when(menu_open, |button| button.bg(t.border_strong))
                        .hover(|style| style.bg(t.border_strong))
                        .child(mono("⋯", Type::BODY, t.muted))
                        .on_click(cx.listener(move |ws, _, _, cx| {
                            cx.stop_propagation();
                            ws.toggle_rule_menu(&menu_id, cx)
                        })),
                )
        };
        row.when(menu_open, |row| row.child(rule_menu(repo, &rule.id, t, cx)))
    });
    div()
        .flex()
        .flex_col()
        .rounded(radius::BOX)
        .border_1()
        .border_color(t.border)
        .bg(t.card)
        .when(!compact, |list| {
            list.child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(4.))
                    .h(px(34.))
                    .px(sp(4.))
                    .border_b_1()
                    .border_color(t.border)
                    .child(div().w(px(36.)).child(heading("ID", t)))
                    .child(div().flex_1().child(heading("Rule", t)))
                    .child(div().w(px(250.)).child(heading("Applies to", t)))
                    .child(div().w(px(190.)).child(heading("Strictness", t)))
                    .child(div().w(px(110.)).child(heading("Activity", t)))
                    .child(div().w(px(30.))),
            )
        })
        .children(rows)
}

/// A rule's ⋯ menu: edit it, or remove it.
fn rule_menu(
    repo: &str,
    id: &str,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let (edit_repo, edit_id) = (repo.to_owned(), id.to_owned());
    let (remove_repo, remove_id) = (repo.to_owned(), id.to_owned());
    let entry = |name: &'static str, color: Hsla| {
        div()
            .id(name)
            .px(sp(3.))
            .py(sp(2.))
            .rounded(radius::CONTROL)
            .text_color(color)
            .cursor_pointer()
            .hover(|style| style.bg(t.selected))
            .child(name)
    };
    div()
        .absolute()
        .right(sp(4.))
        .top(px(44.))
        .w(px(160.))
        .p(sp(1.))
        .flex()
        .flex_col()
        .bg(t.panel)
        .border_1()
        .border_color(t.border_strong)
        .rounded(radius::BOX)
        .shadow_lg()
        .child(entry("Edit", t.text).on_click(cx.listener(
            move |ws, _, _, cx| {
                cx.stop_propagation();
                ws.open_rule_editor(&edit_repo, Some(&edit_id), cx)
            },
        )))
        .child(entry("Remove", t.red).on_click(cx.listener(
            move |ws, _, _, cx| {
                cx.stop_propagation();
                ws.rule_menu = None;
                ws.remove_rule(&remove_repo, &remove_id, cx)
            },
        )))
}

fn legend(t: &Theme) -> Div {
    let swatch = |color: Hsla, name: &'static str| {
        div()
            .flex()
            .items_center()
            .gap(sp(1.5))
            .child(
                div()
                    .w(px(10.))
                    .h(px(6.))
                    .rounded(radius::HAIRLINE)
                    .bg(color),
            )
            .child(name)
    };
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(sp(4.))
        .typeset(Type::CAPTION)
        .text_color(t.dim)
        .child(swatch(t.border_strong, "runs"))
        .child(swatch(t.accent.opacity(0.55), "runs, flagged for you"))
        .child(swatch(t.red.opacity(0.6), "refused, the model gets the rule"))
        .child(div().flex_1())
        .child("Jev sees only the fields a rule names, as the model wrote them. Never tool output or file contents.")
}

/// What waits for a person, and what the rules handled on their own.
fn review(
    ws: &Workspace,
    repo: &str,
    constitution: &Constitution,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let items = ws.review_items(repo);
    let mut cards = Vec::with_capacity(items.len());
    for (n, item) in items.iter().enumerate() {
        let rule = constitution.rules.iter().find(|rule| rule.id == item.rule);
        cards.push(
            review_card(n, item, rule, repo, compact, t, cx).into_any_element(),
        );
    }
    let waiting = div()
        .flex_1()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .gap(sp(3.5))
        .child(heading(&format!("Waiting for you · {}", items.len()), t))
        .when(items.is_empty(), |list| {
            list.child(ui::empty(
                "Nothing waits for you. Flagged calls land here.",
                t,
            ))
        })
        .children(cards);
    let handled = ws.handled(repo);
    let side = div()
        .flex()
        .flex_col()
        .gap(sp(2.5))
        .when(!compact, |side| side.w(px(360.)).flex_shrink_0())
        .child(heading("Handled by the rules", t))
        .child(
            div()
                .flex()
                .flex_col()
                .rounded(radius::BOX)
                .border_1()
                .border_color(t.border)
                .bg(t.card)
                .when(handled.is_empty(), |list| {
                    list.child(div().p(sp(3.)).child(ui::text(
                        "Nothing yet.",
                        Type::CAPTION,
                        t.dim,
                    )))
                })
                .children(handled.into_iter().map(|done| {
                    let (glyph, color, what) = match done.what {
                        HandledKind::Blocked => {
                            (Icon::Blocked, t.red, "blocked")
                        }
                        HandledKind::Held => (Icon::Chat, t.accent, "held"),
                        HandledKind::LookedFine => {
                            (Icon::Check, t.green, "looked fine")
                        }
                    };
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(2.5))
                        .px(sp(3.5))
                        .py(sp(2.5))
                        .border_b_1()
                        .border_color(t.border)
                        .child(icon(glyph, IconSize::SMALL, color))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .flex()
                                .flex_col()
                                .gap(sp(0.5))
                                .child(
                                    mono(
                                        done.shown,
                                        Type::CAPTION,
                                        t.text_soft,
                                    )
                                    .truncate(),
                                )
                                .child(ui::text(
                                    done.run_title,
                                    Type::MICRO,
                                    t.dim,
                                )),
                        )
                        .child(mono(
                            format!("{} · {what}", done.rule),
                            Type::MICRO,
                            t.dim,
                        ))
                })),
        )
        .child(ui::text(
            "Blocked calls need nothing from you: the model got the rule and \
             changed the call. They show that a rule does its job.",
            Type::CAPTION,
            t.dim,
        ));
    div()
        .flex()
        .when(compact, |layout| layout.flex_col())
        .gap(sp(6.))
        .items_start()
        .child(waiting)
        .child(side)
}

fn review_card(
    n: usize,
    item: &ReviewItem,
    rule: Option<&Rule>,
    repo: &str,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let open = Route::Run(item.run.clone());
    let fine = (item.run.clone(), item.key.clone());
    let (adjust_repo, adjust_id) = (repo.to_owned(), item.rule.clone());
    let answer = item.tool.is_none();
    div()
        .flex()
        .flex_col()
        .gap(sp(3.))
        .p(sp(4.))
        .rounded(radius::BOX)
        .border_1()
        .border_color(t.accent_border)
        .bg(t.card)
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(2.5))
                .child(icon(if answer { Icon::Chat } else { Icon::Runs }, IconSize::BASE, t.muted))
                .child(mono(
                    item.tool.clone().unwrap_or_else(|| "final answer".into()),
                    Type::CAPTION,
                    t.blue,
                ))
                .child(ui::text(format!("in {}", item.run_title), Type::CAPTION, t.muted))
                .child(div().flex_1())
                .child(mono(format!("p {:.2}", item.score), Type::CAPTION, t.accent)),
        )
        .child(
            div()
                .px(sp(3.5))
                .py(sp(2.5))
                .rounded(radius::CONTROL)
                .bg(t.bg)
                .map(|body| {
                    if answer {
                        body.child(ui::text(item.shown.clone(), Type::BODY, t.text_soft))
                    } else {
                        body.child(mono(item.shown.clone(), Type::SMALL, t.text_soft))
                    }
                }),
        )
        .when_some(rule, |card, rule| {
            card.child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(3.5))
                    .px(sp(3.))
                    .py(sp(2.5))
                    .rounded(radius::CONTROL)
                    .border_1()
                    .border_color(t.border)
                    .child(mono(rule.id.clone(), Type::CAPTION, t.muted))
                    .child(div().flex_1().min_w(px(0.)).text_color(t.text_soft).child(rule.text.clone()))
                    .when(!compact, |row| {
                        row.child(ui::strictness(
                            rule.review.into(),
                            rule.block.into(),
                            Some(item.score),
                            Some(160.),
                            false,
                            t,
                        ))
                    }),
            )
        })
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(sp(2.))
                .when(!compact, |row| {
                    row.child(
                        div().flex_1().child(ui::text(
                            if answer {
                                "The answer stood, but its score is between this rule's flag and block marks."
                            } else {
                                "It ran: its score is between this rule's flag and block marks."
                            },
                            Type::CAPTION,
                            t.dim,
                        )),
                    )
                })
                .child(
                    div()
                        .id(("review-adjust", n))
                        .child(ui::button("Adjust rule", ButtonKind::Secondary, t))
                        .on_click(cx.listener(move |ws, _, _, cx| {
                            ws.open_rule_editor(&adjust_repo, Some(&adjust_id), cx)
                        })),
                )
                .child(
                    div()
                        .id(("review-open", n))
                        .child(ui::button("Open in run", ButtonKind::Secondary, t))
                        .on_click(cx.listener(move |ws, _, _, cx| ws.navigate(open.clone(), cx))),
                )
                .child(
                    div()
                        .id(("review-dismiss", n))
                        .child(ui::button("Looks fine", ButtonKind::Primary, t))
                        .on_click(cx.listener(move |ws, _, _, cx| {
                            ws.mark_reviewed(&fine.0, &fine.1, cx);
                        })),
                ),
        )
}

/// The rule editor: a drawer on the right, the whole screen on a phone.
fn editor(
    ws: &Workspace,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Option<AnyElement> {
    let draft = ws.rule_draft()?;
    let problem = ws.draft_problem(cx);
    let shown_problem = problem.filter(|_| draft.tried_to_save);
    let title = match &draft.editing {
        Some(id) => format!("Edit {id}"),
        None => "New rule".to_owned(),
    };
    let save_label = if draft.editing.is_some() {
        "Save"
    } else {
        "Add rule"
    };

    // Where it applies: the usual places, then any other field picked.
    let mut offered: Vec<(String, &str)> = PLACES
        .iter()
        .map(|(place, what)| ((*place).to_owned(), *what))
        .collect();
    for place in &draft.places {
        if !offered.iter().any(|(known, _)| known == place) {
            offered.push((place.clone(), "another tool's field"));
        }
    }
    let places = offered.into_iter().map(|(place, what)| {
        let on = draft.places.contains(&place);
        let toggle = place.clone();
        let label = match place.split_once('.') {
            Some((tool, field)) => div()
                .flex()
                .gap(sp(1.5))
                .child(mono(tool.to_owned(), Type::SMALL, t.blue))
                .child(mono("·", Type::SMALL, t.dim))
                .child(mono(field.to_owned(), Type::SMALL, t.text_soft)),
            None => div().child("Final answer"),
        };
        div()
            .id(SharedString::from(format!("place-{place}")))
            .flex()
            .items_center()
            .gap(sp(2.5))
            .min_h(px(if compact { 44. } else { 34. }))
            .px(sp(3.))
            .rounded(radius::CONTROL)
            .cursor_pointer()
            .when(on, |row| row.bg(t.accent_soft))
            .hover(|style| style.bg(t.selected))
            .child(ui::checkbox(on, false, t))
            .child(div().flex_1().child(label))
            .child(ui::text(what, Type::CAPTION, t.dim))
            .on_click(
                cx.listener(move |ws, _, _, cx| ws.toggle_place(&toggle, cx)),
            )
    });
    let applies = div()
        .flex()
        .flex_col()
        .gap(sp(2.))
        .child(label_row("Applies to", "what Jev reads for this rule", t))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(0.5))
                .p(sp(1.5))
                .rounded(radius::BOX)
                .border_1()
                .border_color(
                    if shown_problem.is_some() && draft.places.is_empty() {
                        t.red_border
                    } else {
                        t.border
                    },
                )
                .bg(t.bg)
                .children(places)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(2.))
                        .px(sp(3.))
                        .pt(sp(1.5))
                        .pb(sp(1.))
                        .child(div().flex_1().child(ui::field(
                            &ws.rule_on,
                            true,
                            t,
                        )))
                        .child(
                            div()
                                .id("add-place")
                                .child(ui::button(
                                    "Add field",
                                    ButtonKind::Secondary,
                                    t,
                                ))
                                .on_click(cx.listener(|ws, _, _, cx| {
                                    ws.add_other_place(cx);
                                })),
                        ),
                ),
        );

    let presets = Preset::ALL.into_iter().map(|preset| {
        let on = draft.preset() == Some(preset);
        div()
            .id(preset.label())
            .flex_1()
            .h(px(if compact { 40. } else { 28. }))
            .flex()
            .items_center()
            .justify_center()
            .rounded(radius::CONTROL)
            .cursor_pointer()
            .typeset(Type::SMALL)
            .text_color(if on { t.text } else { t.muted })
            .when(on, |segment| {
                segment.bg(t.selected).font_weight(weight::EMPHASIS)
            })
            .child(preset.label())
            .on_click(
                cx.listener(move |ws, _, _, cx| ws.set_preset(preset, cx)),
            )
    });
    let stepper = |name: &'static str, mark: Mark, value: f64, color: Hsla| {
        let step = |id: &'static str, glyph: &'static str, delta: f64| {
            div()
                .id(id)
                .size(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(radius::CONTROL)
                .border_1()
                .border_color(t.border_strong)
                .cursor_pointer()
                .hover(|style| style.bg(t.selected))
                .child(mono(glyph, Type::SMALL, t.text_soft))
                .on_click(
                    cx.listener(move |ws, _, _, cx| ws.nudge(mark, delta, cx)),
                )
        };
        let (down, up) = match mark {
            Mark::Review => ("review-down", "review-up"),
            Mark::Block => ("block-down", "block-up"),
        };
        div()
            .flex()
            .items_center()
            .gap(sp(2.))
            .child(ui::text(name, Type::CAPTION, color))
            .child(step(down, "−", -0.05))
            .child(mono(format!("{value:.2}"), Type::SMALL, t.text).w(px(36.)))
            .child(step(up, "+", 0.05))
    };
    let zone = |head: String, color: Hsla, body: &'static str| {
        div()
            .flex_1()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .gap(sp(0.75))
            .child(ui::text(head, Type::CAPTION, color))
            .child(ui::text(body, Type::CAPTION, t.dim))
    };
    let strictness = div()
        .flex()
        .flex_col()
        .gap(sp(2.))
        .child(label_row("Strictness", "how sure Jev must be that the rule is broken", t))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(3.5))
                .p(sp(4.))
                .rounded(radius::BOX)
                .border_1()
                .border_color(t.border)
                .bg(t.bg)
                .child(
                    div()
                        .flex()
                        .gap(sp(1.5))
                        .p(sp(0.75))
                        .rounded(radius::BOX)
                        .bg(t.card)
                        .children(presets),
                )
                .child(div().px(sp(2.)).py(sp(2.)).child(ui::strictness(
                    draft.review,
                    draft.block,
                    None,
                    None,
                    true,
                    t,
                )))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(sp(5.))
                        .child(stepper("Flag at", Mark::Review, draft.review, t.accent))
                        .child(stepper("Block at", Mark::Block, draft.block, t.red)),
                )
                .when(!compact, |panel| {
                    panel.child(
                        div()
                            .flex()
                            .gap(sp(3.))
                            .child(zone(format!("Below {:.2}", draft.review), t.text_soft, "The call runs."))
                            .child(zone(
                                format!("{:.2} to {:.2}", draft.review, draft.block),
                                t.accent,
                                "It runs, and waits in Review for you.",
                            ))
                            .child(zone(
                                format!("{:.2} and up", draft.block),
                                t.red,
                                "Refused. The model gets the rule and tries again.",
                            )),
                    )
                }),
        );

    let trial = try_section(ws, draft, t, cx);
    let body = div()
        .id("rule-editor-body")
        .flex_1()
        .min_h(px(0.))
        .overflow_y_scroll()
        .p(sp(5.))
        .flex()
        .flex_col()
        .gap(sp(5.5))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(2.))
                .child(label_row(
                    "Rule",
                    "one sentence, the way you would tell a person",
                    t,
                ))
                .child(ui::field(&ws.rule_text, false, t)),
        )
        .child(applies)
        .when_some(shown_problem, |body, problem| {
            body.child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(2.))
                    .child(icon(Icon::Warning, IconSize::SMALL, t.red))
                    .child(ui::text(problem, Type::SMALL, t.red)),
            )
        })
        .child(strictness)
        .child(trial);

    let save = div()
        .id("save-rule")
        .child(ui::button(save_label, ButtonKind::Primary, t))
        .when(problem.is_some(), |button| button.opacity(0.5))
        .on_click(cx.listener(|ws, _, _, cx| ws.save_rule(cx)));
    let cancel = div()
        .id("cancel-rule")
        .child(ui::button("Cancel", ButtonKind::Secondary, t))
        .on_click(cx.listener(|ws, _, _, cx| ws.close_rule_editor(cx)));
    let panel = div()
        .flex()
        .flex_col()
        .bg(t.panel)
        .child(
            div()
                .flex_shrink_0()
                .h(px(56.))
                .flex()
                .items_center()
                .gap(sp(2.5))
                .px(sp(5.))
                .border_b_1()
                .border_color(t.border)
                .child(
                    div()
                        .flex_1()
                        .typeset(Type::LEAD)
                        .font_weight(weight::STRONG)
                        .child(title),
                )
                .when(compact, |bar| {
                    bar.child(phone_save(save_label, problem, t, cx))
                }),
        )
        .child(body)
        .when(!compact, |panel| {
            panel.child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(sp(2.5))
                    .px(sp(5.))
                    .py(sp(3.5))
                    .border_t_1()
                    .border_color(t.border)
                    .child(div().flex_1().child(ui::text(
                        match (shown_problem, &draft.editing) {
                            (Some(problem), _) => problem.to_owned(),
                            (None, Some(id)) => {
                                format!("Saves over {id} in constitution.toml")
                            }
                            (None, None) => {
                                "Adds it to constitution.toml".to_owned()
                            }
                        },
                        Type::CAPTION,
                        if shown_problem.is_some() {
                            t.red
                        } else {
                            t.dim
                        },
                    )))
                    .child(cancel)
                    .child(save),
            )
        });
    Some(if compact {
        div()
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .child(panel.flex_1())
            .into_any_element()
    } else {
        div()
            .absolute()
            .inset_0()
            .flex()
            .child(
                div().id("rule-scrim").flex_1().bg(t.scrim).on_click(
                    cx.listener(|ws, _, _, cx| ws.close_rule_editor(cx)),
                ),
            )
            .child(
                panel
                    .w(px(560.))
                    .h_full()
                    .border_l_1()
                    .border_color(t.border_strong)
                    .shadow_lg(),
            )
            .into_any_element()
    })
}

/// On a phone, the editor's bar carries Cancel and Add.
fn phone_save(
    label: &'static str,
    problem: Option<&str>,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    div()
        .flex()
        .gap(sp(2.))
        .child(
            div()
                .id("phone-cancel-rule")
                .child(ui::button("Cancel", ButtonKind::Secondary, t))
                .on_click(cx.listener(|ws, _, _, cx| ws.close_rule_editor(cx))),
        )
        .child(
            div()
                .id("phone-save-rule")
                .child(ui::button(label, ButtonKind::Primary, t))
                .when(problem.is_some(), |button| button.opacity(0.5))
                .on_click(cx.listener(|ws, _, _, cx| ws.save_rule(cx))),
        )
}

/// A field's name, and what it is for.
fn label_row(name: &'static str, hint: &'static str, t: &Theme) -> Div {
    div()
        .flex()
        .items_baseline()
        .gap(sp(2.5))
        .child(div().font_weight(weight::EMPHASIS).child(name))
        .child(ui::text(hint, Type::CAPTION, t.dim))
}

/// Trying the rule on the repository's latest calls, with Jev.
fn try_section(
    ws: &Workspace,
    draft: &crate::rule_editor::RuleDraft,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let jev = ws.catalog.models.access.jev;
    let button = div()
        .id("try-rule")
        .child(ui::button(
            match draft.trying {
                Trying::Not => "Try on recent calls",
                _ => "Try again",
            },
            ButtonKind::Secondary,
            t,
        ))
        .when(!jev, |button| button.opacity(0.5))
        .on_click(cx.listener(|ws, _, _, cx| ws.try_rule(cx)));
    let body: AnyElement = match &draft.trying {
        Trying::Not => ui::text(
            if jev {
                "Jev scores the latest calls and answers this rule would read, \
                 as a check would. Nothing runs again."
            } else {
                "Trying a rule asks Jev: add a TypeSafe key first."
            },
            Type::CAPTION,
            t.dim,
        )
        .into_any_element(),
        Trying::Asking => div()
            .flex()
            .items_center()
            .gap(sp(2.))
            .child(icon(Icon::Spinner, IconSize::SMALL, t.accent))
            .child(ui::text("Asking Jev…", Type::CAPTION, t.muted))
            .into_any_element(),
        Trying::Failed(error) => {
            ui::text(error.clone(), Type::CAPTION, t.red).into_any_element()
        }
        Trying::Done { trials, .. } if trials.is_empty() => ui::text(
            "No calls in this repository's runs that the rule reads yet.",
            Type::CAPTION,
            t.dim,
        )
        .into_any_element(),
        Trying::Done { trials, cost } => {
            let rows = trials.iter().map(|trial| {
                let (color, verdict) = if trial.score >= draft.block {
                    (t.red, "would block")
                } else if trial.score >= draft.review {
                    (t.accent, "would flag")
                } else {
                    (t.muted, "would run")
                };
                div()
                    .flex()
                    .items_center()
                    .gap(sp(3.))
                    .px(sp(3.))
                    .py(sp(2.25))
                    .border_b_1()
                    .border_color(t.border)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .gap(sp(2.))
                            .child(mono(
                                trial
                                    .tool
                                    .clone()
                                    .unwrap_or_else(|| "answer".into()),
                                Type::CAPTION,
                                t.blue,
                            ))
                            .child(
                                mono(
                                    trial.shown.clone(),
                                    Type::CAPTION,
                                    t.text_soft,
                                )
                                .truncate(),
                            ),
                    )
                    .child(ui::strictness(
                        draft.review,
                        draft.block,
                        Some(trial.score),
                        Some(90.),
                        false,
                        t,
                    ))
                    .child(
                        mono(
                            format!("{:.2} {verdict}", trial.score),
                            Type::CAPTION,
                            color,
                        )
                        .w(px(120.))
                        .flex_shrink_0(),
                    )
            });
            div()
                .flex()
                .flex_col()
                .gap(sp(2.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .rounded(radius::BOX)
                        .border_1()
                        .border_color(t.border)
                        .bg(t.bg)
                        .children(rows),
                )
                .child(ui::text(
                    format!(
                        "{} {} · {}. Only what a check shows went to Jev; nothing ran again.",
                        trials.len(),
                        if trials.len() == 1 { "call" } else { "calls" },
                        usd(*cost)
                    ),
                    Type::CAPTION,
                    t.dim,
                ))
                .into_any_element()
        }
    };
    div()
        .flex()
        .flex_col()
        .gap(sp(2.5))
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(2.5))
                .child(div().flex_1().child(label_row(
                    "Try it",
                    "before it runs for real",
                    t,
                )))
                .child(button),
        )
        .child(body)
}
