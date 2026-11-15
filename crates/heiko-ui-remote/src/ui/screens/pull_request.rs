//! A pull request from a finished run: the draft to check and send, then
//! the opened pull request.

use gpui::{AnyElement, Context, Div, div, prelude::*, rems};
use heiko_agent::tool::RunId;

use crate::{
    assets::Icon,
    pull_request::{Checks, PrState, PullRequest},
    route::Route,
    theme::{Design as _, Theme, Tone, Type, sp},
    ui::{
        BlockSize,
        State,
        components::{
            ButtonKind,
            big_button,
            checkbox,
            link_action,
            mark,
            notice,
            phone_bar,
            phone_body,
            primary_block,
            section_header,
            stamp,
            title,
        },
        mono,
        prose,
        text,
    },
    workspace::Workspace,
};

pub fn render(
    ws: &Workspace,
    run: &RunId,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let pr = ws.pull_request(run);
    let body = match pr {
        None => div()
            .flex_1()
            .flex()
            .justify_center()
            .pt(sp(20.))
            .child(notice(
                Icon::Spinner,
                "Writing the pull request from the run…",
                t.accent,
                Type::SMALL,
                t,
            ))
            .into_any_element(),
        Some(pr) if pr.is_open() => opened(ws, run, pr, compact, t, cx),
        Some(pr) if compact => phone_draft(run, pr, t, cx),
        Some(pr) => draft(ws, run, pr, t, cx),
    };
    if compact {
        let body = if pr.is_some_and(|pr| !pr.is_open()) {
            // The draft lays out its own column, with the button at the
            // bottom.
            body
        } else {
            phone_body("pr-body").child(body).into_any_element()
        };
        return div()
            .size_full()
            .flex()
            .flex_col()
            .child(phone_bar(ws, "Pull request", t, cx))
            .child(body)
            .into_any_element();
    }
    div()
        .size_full()
        .flex()
        .flex_col()
        .typeset(Type::BODY)
        .child(body)
        .into_any_element()
}

/// The screen's head: its title in the display face over the heavy
/// rule, and at its right the run it comes from, which goes back to it.
fn head(
    ws: &Workspace,
    run: &RunId,
    heading: &'static str,
    tests: Option<&str>,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let back = run.clone();
    let from =
        ws.run(run).map(|view| {
            let turns = match view.turn {
                1 => "· 1 turn".to_owned(),
                n => format!("· {n} turns"),
            };
            div()
                .flex()
                .items_baseline()
                .flex_shrink_0()
                .gap(sp(1.5))
                .typeset(Type::SMALL)
                .text_color(t.dim)
                .child(
                    div()
                        .id("pr-run")
                        .child(link_action(view.title.clone(), t))
                        .on_click(cx.listener(move |ws, _, _, cx| {
                            ws.navigate(Route::Run(back.clone()), cx)
                        })),
                )
                .child(turns)
                .children(tests.map(|tests| {
                    div().flex().gap(sp(1.5)).child("·").child(
                        div().text_color(t.green).child(tests.to_owned()),
                    )
                }))
        });
    div()
        .flex()
        .items_baseline()
        .gap(sp(3.))
        .pb(sp(2.5))
        .border_b_2()
        .border_color(t.rule)
        .child(
            div()
                .flex_1()
                .min_w(rems(0.))
                .typeset(Type::PAGE_TITLE.sized(26.))
                .text_color(t.bright)
                .child(heading),
        )
        .children(from)
}

/// A branch's name, in mono, underlined.
fn branch(name: &str, color: gpui::Hsla, t: &Theme) -> Div {
    mono(name.to_owned(), Type::LEAD, color)
        .flex_shrink_0()
        .border_b_1()
        .border_color(t.slate)
}

/// A field's name, in small muted print over it.
fn field_label(name: &'static str, t: &Theme) -> Div {
    text(name, Type::CAPTION, t.dim)
}

/// A commit between hairlines: its title and line counts.
fn commit_row(
    title: &str,
    added: u32,
    removed: u32,
    style: Type,
    t: &Theme,
) -> Div {
    div()
        .flex()
        .items_baseline()
        .gap(sp(2.5))
        .py(sp(2.75))
        .border_b_1()
        .border_color(t.hairline)
        .child(
            div()
                .flex_1()
                .min_w(rems(0.))
                .typeset(style)
                .child(title.to_owned()),
        )
        .child(mono(format!("+{added}"), Type::SMALL, t.green).flex_shrink_0())
        .child(
            mono(format!("\u{2212}{removed}"), Type::SMALL, t.red)
                .flex_shrink_0(),
        )
}

/// Whether the head merges into `base`: its mark and words.
fn merge(pr: &PullRequest, t: &Theme) -> Div {
    let (state, words, color) = if pr.mergeable {
        (
            State::Met,
            format!("no conflicts with {}", pr.base),
            t.green,
        )
    } else {
        (State::Failing, format!("conflicts with {}", pr.base), t.red)
    };
    div()
        .flex()
        .items_center()
        .flex_shrink_0()
        .gap(sp(2.))
        .child(mark(state, t))
        .child(text(words, Type::SMALL, color))
}

fn create_label(pr: &PullRequest) -> &'static str {
    match pr.state {
        PrState::Creating => "Creating…",
        _ => "Create pull request",
    }
}

/// Why creating it failed, if it did.
fn failure(pr: &PullRequest) -> Option<String> {
    match &pr.state {
        PrState::Failed(error) => Some(error.clone()),
        _ => None,
    }
}

fn draft(
    ws: &Workspace,
    run: &RunId,
    pr: &PullRequest,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let left = div()
        .flex_1()
        .min_w(rems(0.))
        .flex()
        .flex_col()
        .child(head(
            ws,
            run,
            "Open a pull request",
            pr.tests.as_deref(),
            t,
            cx,
        ))
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(3.))
                .mt(sp(5.))
                .child(branch(&pr.head, t.text, t))
                .child(mono("\u{2192}", Type::LEAD, t.dim))
                .child(branch(&pr.base, t.green, t))
                .child(div().flex_1())
                .child(merge(pr, t)),
        )
        .child(field_label("Title", t).mt(sp(6.5)))
        .child(
            div()
                .pt(sp(1.5))
                .pb(sp(2.5))
                .border_b_1()
                .border_color(t.slate)
                .typeset(Type::HEADLINE.sized(24.))
                .text_color(t.bright)
                .child(ws.pr_title.clone()),
        )
        .child(field_label("Description, written from the run", t).mt(sp(6.5)))
        .child(
            div()
                .id("pr-description")
                .flex_1()
                .min_h(rems(13.75))
                .overflow_y_scroll()
                .mt(sp(2.))
                .py(sp(3.5))
                .border_y_1()
                .border_color(t.hairline)
                .typeset(Type::CODE.leading(1.6))
                .text_color(t.text_soft)
                .child(pr.body.clone()),
        );

    let options = [
        (true, pr.draft, "Open as a draft"),
        (
            false,
            pr.keep_pushing,
            "Keep pushing new turns to this branch",
        ),
    ]
    .into_iter()
    .map(|(is_draft, checked, words)| {
        let run = run.clone();
        div()
            .id(if is_draft {
                "pr-draft"
            } else {
                "pr-keep-pushing"
            })
            .flex()
            .items_center()
            .gap(sp(3.))
            .py(sp(2.5))
            .border_b_1()
            .border_color(t.hairline)
            .cursor_pointer()
            .child(checkbox(checked, true, t))
            .child(words)
            .on_click(cx.listener(move |ws, _, _, cx| {
                ws.toggle_pr_option(&run, is_draft, cx)
            }))
    });
    let create = run.clone();
    let right = div()
        .w(rems(23.))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .child(
            section_header(
                "Commits",
                Some(pr.commits.len().to_string().into()),
                t,
            )
            .pt(sp(1.5)),
        )
        .children(pr.commits.iter().map(|commit| {
            commit_row(
                &commit.title,
                commit.added,
                commit.removed,
                Type::BODY,
                t,
            )
        }))
        .child(section_header("Options", None, t).mt(sp(9.5)))
        .children(options)
        .child(section_header("Reviewers", None, t).mt(sp(9.5)))
        .child(
            div()
                .py(sp(3.))
                .border_b_1()
                .border_color(t.hairline)
                .typeset(Type::TITLE)
                .child(ws.reviewers.clone()),
        )
        .child(div().flex_1().min_h(sp(6.)))
        .children(failure(pr).map(|error| {
            notice(Icon::Warning, error, t.red, Type::SMALL, t).mb(sp(4.))
        }))
        .child(
            div().flex().justify_end().child(
                div()
                    .id("create-pr")
                    .child(primary_block(create_label(pr), BlockSize::Large, t))
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.create_pull_request(&create, cx)
                    })),
            ),
        );
    div()
        .flex_1()
        .min_h(rems(0.))
        .flex()
        .gap(sp(13.))
        .px(sp(11.))
        .py(sp(8.5))
        .child(left)
        .child(right)
        .into_any_element()
}

fn phone_draft(
    run: &RunId,
    pr: &PullRequest,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let toggle = run.clone();
    let create = run.clone();
    phone_body("pr-body")
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(1.5))
                .child(title(pr.title.clone(), Type::HEADING))
                .child(mono(
                    format!("{} \u{2192} {}", pr.head, pr.base),
                    Type::CAPTION,
                    t.muted,
                )),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(sp(2.))
                .child(merge(pr, t))
                .children(pr.tests.as_ref().filter(|_| pr.mergeable).map(
                    |tests| text(format!("· {tests}"), Type::SMALL, t.green),
                )),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .child(section_header(
                    "Commits",
                    Some(pr.commits.len().to_string().into()),
                    t,
                ))
                .children(pr.commits.iter().map(|commit| {
                    commit_row(
                        &commit.title,
                        commit.added,
                        commit.removed,
                        Type::BODY,
                        t,
                    )
                })),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(2.))
                .child(field_label("Description", t))
                .child(
                    div()
                        .py(sp(3.))
                        .border_y_1()
                        .border_color(t.hairline)
                        .typeset(Type::SMALL)
                        .leading(1.55)
                        .child(prose(&pr.short_body(), t.text_soft, t)),
                ),
        )
        .child(
            div()
                .id("phone-pr-draft")
                .flex()
                .items_center()
                .gap(sp(3.))
                .min_h(rems(2.75))
                .cursor_pointer()
                .child(checkbox(pr.draft, true, t))
                .child("Open as a draft")
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.toggle_pr_option(&toggle, true, cx)
                })),
        )
        .child(div().flex_1())
        .children(
            failure(pr).map(|error| {
                notice(Icon::Warning, error, t.red, Type::BODY, t)
            }),
        )
        .child(
            div()
                .id("phone-create-pr")
                .child(big_button(
                    create_label(pr),
                    None,
                    ButtonKind::Primary,
                    t,
                ))
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.create_pull_request(&create, cx)
                })),
        )
        .into_any_element()
}

fn opened(
    ws: &Workspace,
    run: &RunId,
    pr: &PullRequest,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let PrState::Opened {
        number,
        url,
        checks,
    } = &pr.state
    else {
        return div().into_any_element();
    };
    let forge = ws.catalog.forge_name().to_owned();
    let (state, checks_text, checks_color) = match checks {
        Checks::Running => (
            State::Working,
            format!("checks are running on {forge}"),
            t.green,
        ),
        Checks::Passed => {
            (State::Met, format!("checks passed on {forge}"), t.green)
        }
        Checks::Failed => {
            (State::Failing, format!("checks failed on {forge}"), t.red)
        }
        Checks::None => (
            State::Idle,
            "no checks run on this repository".to_owned(),
            t.dim,
        ),
    };
    let url = url.clone();
    let back = run.clone();
    let content = div()
        .w_full()
        .when(!compact, |col| col.max_w(rems(42.)))
        .flex()
        .flex_col()
        .gap(sp(5.))
        .child(if compact {
            title("Pull request opened", Type::HEADLINE)
        } else {
            head(ws, run, "Pull request opened", pr.tests.as_deref(), t, cx)
        })
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(2.5))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(3.))
                        .child(
                            div()
                                .min_w(rems(0.))
                                .typeset(Type::HEADLINE.sized(if compact {
                                    18.
                                } else {
                                    24.
                                }))
                                .text_color(t.bright)
                                .child(pr.title.clone()),
                        )
                        .when(pr.draft, |row| {
                            row.child(stamp("Draft", Tone::Quiet, t))
                        }),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(sp(2.))
                        .child(mono(
                            format!("{}#{number}", pr.repo),
                            Type::SMALL,
                            t.text_soft,
                        ))
                        .child(branch(&pr.head, t.text, t).typeset(Type::CODE))
                        .child(mono("\u{2192}", Type::SMALL, t.dim))
                        .child(
                            branch(&pr.base, t.green, t).typeset(Type::CODE),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(2.))
                        .py(sp(2.75))
                        .border_y_1()
                        .border_color(t.hairline)
                        .child(mark(state, t))
                        .child(text(checks_text, Type::SMALL, checks_color)),
                ),
        )
        .child(div().typeset(Type::SMALL).leading(1.55).child(prose(
            &if pr.keep_pushing {
                format!(
                    "New turns on this run are pushed to `{}` and update \
                         the pull request.",
                    pr.head
                )
            } else {
                format!(
                    "`{}` stays as it is: later turns on this run are \
                         not pushed.",
                    pr.head
                )
            },
            t.muted,
            t,
        )))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(sp(4.5))
                .child(
                    div()
                        .id("view-pr")
                        .child(primary_block(
                            format!("View on {forge}"),
                            BlockSize::Large,
                            t,
                        ))
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                )
                .child(
                    div()
                        .id("back-to-run")
                        .child(link_action("back to the run", t))
                        .on_click(cx.listener(move |ws, _, _, cx| {
                            ws.navigate(Route::Run(back.clone()), cx)
                        })),
                ),
        );
    if compact {
        return content.into_any_element();
    }
    div()
        .flex_1()
        .min_h(rems(0.))
        .flex()
        .items_start()
        .px(sp(11.))
        .py(sp(8.5))
        .child(content)
        .into_any_element()
}
