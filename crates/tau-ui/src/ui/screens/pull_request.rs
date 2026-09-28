//! A pull request from a finished run: the draft to check and send, then
//! the opened pull request.

use gpui::{
    AnyElement,
    Context,
    Div,
    FontWeight,
    div,
    prelude::*,
    px,
    relative,
};
use tau_agent::tool::RunId;

use crate::{
    assets::Icon,
    pull_request::{Checks, PrState, PullRequest},
    route::Route,
    theme::{MONO, Theme},
    ui::{
        chrome::logo,
        form::{
            big_button,
            checkbox,
            label,
            lead,
            notice,
            panel,
            phone_bar,
            phone_body,
            title,
        },
        heading,
        icon,
        mono,
        prose,
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
            .pt(px(80.))
            .child(notice(
                Icon::Spinner,
                "Writing the pull request from the run…",
                t.accent,
                13.,
                t,
            ))
            .into_any_element(),
        Some(pr) if pr.is_open() => opened(ws, run, pr, compact, t, cx),
        Some(pr) if compact => phone_draft(ws, run, pr, t, cx),
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
        .text_size(px(14.))
        .child(top_bar(ws, run, t, cx))
        .child(body)
        .into_any_element()
}

/// Where you are: the workspace, the run, the pull request.
fn top_bar(
    ws: &Workspace,
    run: &RunId,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let run_title = ws.run(run).map(|run| run.title.clone());
    let back = run.clone();
    div()
        .h(px(44.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(16.))
        .bg(t.panel)
        .border_b_1()
        .border_color(t.border)
        .child(logo(t, 26.))
        .child(div().text_color(t.muted).child(ws.name.clone()))
        .children(run_title.map(|title| {
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(div().text_color(t.dim).child("/"))
                .child(
                    div()
                        .id("pr-run")
                        .text_color(t.muted)
                        .cursor_pointer()
                        .hover(|style| style.text_color(t.text))
                        .child(title)
                        .on_click(cx.listener(move |ws, _, _, cx| {
                            ws.navigate(Route::Run(back.clone()), cx)
                        })),
                )
        }))
        .child(div().text_color(t.dim).child("/"))
        .child(div().font_weight(FontWeight::MEDIUM).child("Pull request"))
}

fn branch_chip(name: &str, t: &Theme) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .px(px(10.))
        .py(px(6.))
        .rounded(px(6.))
        .bg(t.raised)
        .child(icon(Icon::Fork, 13., t.text_soft))
        .child(mono(name.to_owned(), 13., t.text))
}

fn commit_counts(added: u32, removed: u32, t: &Theme) -> Div {
    mono(format!("+{added} \u{2212}{removed}"), 12., t.green).flex_shrink_0()
}

fn create_label(pr: &PullRequest) -> &'static str {
    match pr.state {
        PrState::Creating => "Creating…",
        _ => "Create pull request",
    }
}

fn draft(
    ws: &Workspace,
    run: &RunId,
    pr: &PullRequest,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let merge = if pr.mergeable {
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .text_color(t.green)
            .child(icon(Icon::Check, 14., t.green))
            .child(format!("No conflicts with {}", pr.base))
    } else {
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .text_color(t.red)
            .child(icon(Icon::Warning, 14., t.red))
            .child(format!("Conflicts with {}", pr.base))
    };
    let left = div()
        .flex_1()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .gap(px(20.))
        .px(px(40.))
        .py(px(32.))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(title("Open a pull request", 28.))
                .child(lead(&pr.summary, 15., t)),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .text_size(px(13.))
                .child(branch_chip(&pr.head, t))
                .child(icon(Icon::Arrow, 14., t.dim))
                .child(branch_chip(&pr.base, t))
                .child(merge),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(label("Title", t))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .h(px(44.))
                        .px(px(12.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(t.border_strong)
                        .bg(t.panel)
                        .child(ws.pr_title.clone()),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_h(px(0.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(label("Description, written from the run", t))
                .child(
                    div()
                        .id("pr-description")
                        .flex_1()
                        .min_h(px(220.))
                        .overflow_y_scroll()
                        .p(px(12.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(t.border_strong)
                        .bg(t.panel)
                        .font_family(MONO)
                        .text_size(px(12.5))
                        .line_height(relative(1.6))
                        .text_color(t.text_soft)
                        .child(pr.body.clone()),
                ),
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
    .map(|(is_draft, checked, text)| {
        let run = run.clone();
        div()
            .id(if is_draft {
                "pr-draft"
            } else {
                "pr-keep-pushing"
            })
            .flex()
            .items_center()
            .gap(px(10.))
            .min_h(px(36.))
            .cursor_pointer()
            .child(checkbox(checked, 18., t))
            .child(text)
            .on_click(cx.listener(move |ws, _, _, cx| {
                ws.toggle_pr_option(&run, is_draft, cx)
            }))
    });
    let create = run.clone();
    let right = div()
        .w(px(380.))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .gap(px(22.))
        .px(px(24.))
        .py(px(32.))
        .bg(t.panel)
        .border_l_1()
        .border_color(t.border)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(heading("Commits", t))
                .children(pr.commits.iter().map(|commit| {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .px(px(12.))
                        .py(px(10.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(t.border)
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .text_size(px(13.))
                                .child(commit.title.clone()),
                        )
                        .child(commit_counts(commit.added, commit.removed, t))
                }))
                .child(
                    div()
                        .text_size(px(12.))
                        .line_height(relative(1.5))
                        .child(prose(
                            "The run's turns are squashed into these commits by \
                             the change descriptions the model wrote with \
                             `vcs_commit`.",
                            t.dim,
                            t,
                        )),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(heading("Options", t))
                .children(options)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(label("Reviewers", t))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .h(px(40.))
                                .px(px(12.))
                                .rounded(px(8.))
                                .border_1()
                                .border_color(t.border_strong)
                                .bg(t.bg)
                                .text_size(px(13.))
                                .child(ws.reviewers.clone()),
                        ),
                ),
        )
        .child(div().flex_1())
        .when_some(
            match &pr.state {
                PrState::Failed(error) => Some(error.clone()),
                _ => None,
            },
            |col, error| col.child(notice(Icon::Warning, error, t.red, 13., t)),
        )
        .child(
            div()
                .id("create-pr")
                .child(big_button(
                    create_label(pr),
                    Some(Icon::PullRequest),
                    true,
                    t,
                ))
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.create_pull_request(&create, cx)
                })),
        );
    div()
        .flex_1()
        .min_h(px(0.))
        .flex()
        .child(left)
        .child(right)
        .into_any_element()
}

fn phone_draft(
    _ws: &Workspace,
    run: &RunId,
    pr: &PullRequest,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let status = match (&pr.tests, pr.mergeable) {
        (Some(tests), true) => format!("No conflicts · {tests}"),
        (None, true) => "No conflicts".to_owned(),
        (_, false) => format!("Conflicts with {}", pr.base),
    };
    let tone = if pr.mergeable { t.green } else { t.red };
    let toggle = run.clone();
    let create = run.clone();
    phone_body("pr-body")
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(title(pr.title.clone(), 20.))
                .child(mono(
                    format!("{} \u{2192} {}", pr.head, pr.base),
                    12.,
                    t.muted,
                )),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(px(14.))
                .text_color(tone)
                .child(icon(
                    if pr.mergeable {
                        Icon::Check
                    } else {
                        Icon::Warning
                    },
                    14.,
                    tone,
                ))
                .child(status),
        )
        .child(panel(16., t).gap(px(10.)).children(pr.commits.iter().map(
            |commit| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_size(px(14.))
                            .child(commit.title.clone()),
                    )
                    .child(commit_counts(commit.added, commit.removed, t))
            },
        )))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(label("Description", t))
                .child(
                    div()
                        .p(px(12.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(t.border_strong)
                        .bg(t.panel)
                        .text_size(px(13.))
                        .line_height(relative(1.55))
                        .child(prose(&pr.short_body(), t.text_soft, t)),
                ),
        )
        .child(
            div()
                .id("phone-pr-draft")
                .flex()
                .items_center()
                .gap(px(10.))
                .min_h(px(44.))
                .cursor_pointer()
                .child(checkbox(pr.draft, 20., t))
                .child("Open as a draft")
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.toggle_pr_option(&toggle, true, cx)
                })),
        )
        .child(div().flex_1())
        .when_some(
            match &pr.state {
                PrState::Failed(error) => Some(error.clone()),
                _ => None,
            },
            |col, error| col.child(notice(Icon::Warning, error, t.red, 14., t)),
        )
        .child(
            div()
                .id("phone-create-pr")
                .child(big_button(
                    create_label(pr),
                    Some(Icon::PullRequest),
                    true,
                    t,
                ))
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.create_pull_request(&create, cx)
                })),
        )
        .into_any_element()
}

fn opened(
    _ws: &Workspace,
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
    let (glyph, checks_text, checks_color) = match checks {
        Checks::Running => {
            (Icon::Spinner, "Checks are running on GitHub", t.accent)
        }
        Checks::Passed => (Icon::Check, "Checks passed on GitHub", t.green),
        Checks::Failed => (Icon::Warning, "Checks failed on GitHub", t.red),
    };
    let url = url.clone();
    let back = run.clone();
    let content = div()
        .w_full()
        .when(!compact, |col| col.max_w(px(620.)))
        .flex()
        .flex_col()
        .gap(px(22.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(
                    div()
                        .size(px(40.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(20.))
                        .bg(t.green.opacity(0.14))
                        .child(icon(Icon::PullRequest, 20., t.green)),
                )
                .child(title(
                    "Pull request opened",
                    if compact { 22. } else { 28. },
                )),
        )
        .child(
            panel(20., t)
                .gap(px(12.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex_1()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(pr.title.clone()),
                        )
                        .when(pr.draft, |row| {
                            row.child(
                                div()
                                    .px(px(8.))
                                    .py(px(2.))
                                    .rounded(px(10.))
                                    .border_1()
                                    .border_color(t.border_strong)
                                    .text_size(px(12.))
                                    .text_color(t.muted)
                                    .child("Draft"),
                            )
                        }),
                )
                .child(mono(
                    format!(
                        "{}#{number} · {} \u{2192} {}",
                        pr.repo, pr.head, pr.base
                    ),
                    13.,
                    t.text_soft,
                ))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .text_size(px(13.))
                        .text_color(checks_color)
                        .child(icon(glyph, 14., checks_color))
                        .child(checks_text),
                ),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(10.))
                .child(
                    div()
                        .id("view-pr")
                        .child(big_button(
                            "View on GitHub",
                            Some(Icon::Arrow),
                            true,
                            t,
                        ))
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                )
                .child(
                    div()
                        .id("back-to-run")
                        .child(big_button("Back to the run", None, false, t))
                        .on_click(cx.listener(move |ws, _, _, cx| {
                            ws.navigate(Route::Run(back.clone()), cx)
                        })),
                ),
        )
        .child(
            div()
                .text_size(px(13.))
                .line_height(relative(1.55))
                .child(prose(
                    &if pr.keep_pushing {
                        format!(
                            "New turns on this run are pushed to `{}` and update \
                             the pull request. Review comments can come back to \
                             the run as steering messages.",
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
                )),
        );
    if compact {
        return content.into_any_element();
    }
    div()
        .flex_1()
        .min_h(px(0.))
        .flex()
        .justify_center()
        .items_start()
        .px(px(24.))
        .py(px(80.))
        .child(content)
        .into_any_element()
}
