//! Model settings: each agent's default, which models the picker shows,
//! and the accounts tau reaches them and GitHub with.

use gpui::{AnyElement, Context, SharedString, div, prelude::*, px};

use crate::{
    assets::Icon,
    models::{AccessInfo, AccountState},
    plan_usage,
    theme::{Design as _, IconSize, Theme, Type, radius, sp},
    ui::{self, ButtonKind, Material as _, heading, mono},
    workspace::{PickerTarget, Workspace},
};

pub fn render(
    ws: &Workspace,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let models = &ws.catalog.models;
    let defaults = models
        .agents
        .iter()
        .map(|(agent, what)| {
            div()
                .flex()
                .items_center()
                .gap(sp(4.))
                .px(sp(4.))
                .py(sp(3.5))
                .border_b_1()
                .border_color(t.border)
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .gap(sp(0.75))
                        .child(agent.clone())
                        .child(ui::text(what.clone(), Type::CAPTION, t.muted)),
                )
                .child(ws.model_chip(
                    PickerTarget::Default(agent.clone()),
                    t,
                    cx,
                ))
        })
        .collect::<Vec<_>>();

    let columns = |row: gpui::Div| {
        row.grid()
            .grid_cols(if compact { 2 } else { 3 })
            .items_center()
            .px(sp(4.))
    };
    let header = columns(div())
        .h(px(32.))
        .bg(t.card)
        .border_b_1()
        .border_color(t.border)
        .typeset(Type::MICRO)
        .text_color(t.dim)
        .child("MODEL")
        .when(!compact, |row| row.child("CONTEXT"))
        .child(div().flex().justify_end().child("SHOW"));
    let rows = models
        .options
        .iter()
        .map(|option| {
            let hidden = models.settings.is_hidden(&option.id);
            let id = option.id.clone();
            columns(div())
                .min_h(px(40.))
                .border_b_1()
                .border_color(t.border)
                .child(mono(option.label().to_owned(), Type::CAPTION, t.text))
                .when(!compact, |row| {
                    row.child(mono(
                        option.context_label(),
                        Type::CAPTION,
                        t.muted,
                    ))
                })
                .child(
                    div().flex().justify_end().child(
                        div()
                            .id(SharedString::from(format!("show-{id}")))
                            .child(ui::switch(!hidden, t))
                            .on_click(cx.listener(move |ws, _, _, cx| {
                                ws.toggle_model_hidden(&id, cx)
                            })),
                    ),
                )
        })
        .collect::<Vec<_>>();

    let access = &models.access;
    let chatgpt = chatgpt_section(access, t, cx);
    let github_user = ws.setup.user().map(str::to_owned);
    let github = div()
        .flex()
        .items_center()
        .gap(sp(2.5))
        .child(ui::icon(
            Icon::Repo,
            IconSize::LARGE,
            if github_user.is_some() {
                t.green
            } else {
                t.dim
            },
        ))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(sp(0.5))
                .child("GitHub")
                .child(ui::text(
                    match &github_user {
                        Some(user) => {
                            format!(
                                "Signed in as @{user} · clones repositories"
                            )
                        }
                        None => {
                            "Not signed in · for cloning repositories".into()
                        }
                    },
                    Type::CAPTION,
                    t.muted,
                )),
        )
        .child(
            div()
                .id("account-github")
                .child(ui::button(
                    if github_user.is_some() {
                        "Sign out"
                    } else {
                        "Sign in"
                    },
                    ButtonKind::Secondary,
                    t,
                ))
                .on_click(cx.listener(move |ws, _, _, cx| {
                    if ws.setup.user().is_some() {
                        ws.sign_out_github(cx)
                    } else {
                        ws.connect_github(cx)
                    }
                })),
        );
    let jev = div()
        .flex()
        .items_center()
        .gap(sp(2.5))
        .child(ui::icon(
            Icon::Blocked,
            IconSize::LARGE,
            if access.jev { t.green } else { t.dim },
        ))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(sp(0.5))
                .child("TypeSafe (Jev)")
                .child(ui::text(
                    if access.jev {
                        "Saved · tau-constitution checks runs against each repository's rules"
                    } else {
                        "None · without it, repository rules are not checked"
                    },
                    Type::CAPTION,
                    t.muted,
                )),
        )
        .child(
            div()
                .id("account-jev")
                .child(ui::button(
                    if access.jev { "Remove" } else { "Add a key" },
                    ButtonKind::Secondary,
                    t,
                ))
                .on_click(cx.listener(|ws, _, window, cx| {
                    if ws.catalog.models.access.jev {
                        ws.forget_jev_key(cx)
                    } else {
                        ws.ask_for_jev_key(window, cx)
                    }
                })),
        );
    let access_card = ui::panel(4.5, t)
        .gap(sp(3.))
        .child(heading("Accounts", t))
        .child(chatgpt)
        .child(github)
        .child(jev);

    let main = div()
        .flex_1()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .gap(sp(4.5))
        .child(ui::screen_title(
            "Models",
            "What each agent runs on unless a run picks another, and which \
             models the picker offers.",
            t,
        ))
        .child(heading("Defaults", t))
        .child(ui::card(t).children(defaults))
        .child(heading("In the picker", t))
        .child(ui::card(t).child(header).children(rows));
    let side = div()
        .flex()
        .flex_col()
        .gap(sp(4.))
        .when(!compact, |side| {
            side.w(px(340.)).flex_shrink_0().pt(sp(14.5))
        })
        .child(access_card);
    ui::screen(
        "models",
        compact,
        div()
            .flex()
            .when(compact, |row| row.flex_col())
            .gap(sp(7.))
            .child(main)
            .child(side),
    )
    .into_any_element()
}

/// The ChatGPT sign-ins: each saved account, the active one marked, and
/// what can be done with it: sign in, switch, enable plan use, sign out.
fn chatgpt_section(
    access: &AccessInfo,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> gpui::Div {
    let active = access.active_account();
    let status = match active.map(|account| account.state) {
        _ if access.chatgpt => "Runs use your ChatGPT plan",
        Some(AccountState::PlanDisabled) => {
            "Signed in, but plan use isn't enabled · enable it to run tasks"
        }
        Some(AccountState::SignedOut) => "Signed out · sign in again",
        _ => "Not signed in",
    };
    let header = div()
        .flex()
        .items_center()
        .gap(sp(2.5))
        .child(ui::icon(
            Icon::Chat,
            IconSize::LARGE,
            if access.chatgpt { t.green } else { t.dim },
        ))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(sp(0.5))
                .child("ChatGPT")
                .child(ui::text(status, Type::CAPTION, t.muted)),
        )
        .child(
            div()
                .id("chatgpt-add")
                .child(ui::button(
                    if access.accounts.is_empty() {
                        "Continue with ChatGPT"
                    } else {
                        "Add account"
                    },
                    ButtonKind::Secondary,
                    t,
                ))
                .on_click(cx.listener(|ws, _, _, cx| {
                    ws.connect_model(cx);
                    ws.sign_in_chatgpt(None, false, cx);
                })),
        );
    let rows = access.accounts.iter().map(|account| {
        let id = account.id.clone();
        let state = match account.state {
            AccountState::Plan => "Plan use enabled",
            AccountState::PlanDisabled => "Plan use not enabled",
            AccountState::SignedOut => "Signed out",
        };
        div()
            .id(SharedString::from(format!("chatgpt-account-{id}")))
            .flex()
            .items_center()
            .gap(sp(2.))
            .px(sp(2.5))
            .py(sp(1.75))
            .rounded(radius::CONTROL)
            .when(account.active, |row| row.key(t))
            .when(!account.active, |row| {
                row.cursor_pointer()
                    .hover(|style| style.bg(gpui::white().opacity(0.04)))
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.switch_chatgpt(&id, cx)
                    }))
            })
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .gap(sp(0.25))
                    .child(
                        div()
                            .typeset(Type::SMALL)
                            .truncate()
                            .child(account.label.clone()),
                    )
                    .child(ui::text(state, Type::MICRO, t.dim)),
            )
            .child(if account.active {
                ui::badge("Active", t.green, t.border_strong).into_any_element()
            } else {
                ui::text("Switch", Type::CAPTION, t.blue).into_any_element()
            })
    });
    let actions = active.map(|account| {
        let id = account.id.clone();
        let main = match account.state {
            AccountState::Plan => div()
                .id("chatgpt-manage-usage")
                .child(ui::button(
                    plan_usage::MANAGE_USAGE,
                    ButtonKind::Secondary,
                    t,
                ))
                .on_click(cx.listener(|ws, _, _, cx| ws.manage_usage(cx))),
            AccountState::PlanDisabled => div()
                .id("chatgpt-enable")
                .child(ui::button(
                    "Enable ChatGPT plan use",
                    ButtonKind::Primary,
                    t,
                ))
                .on_click(cx.listener(|ws, _, _, cx| ws.enable_plan_usage(cx))),
            AccountState::SignedOut => div()
                .id("chatgpt-sign-in-again")
                .child(ui::button("Sign in again", ButtonKind::Primary, t))
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.connect_model(cx);
                    ws.sign_in_chatgpt(Some(id.clone()), false, cx);
                })),
        };
        div().flex().gap(sp(2.)).child(main).when(
            account.state != AccountState::SignedOut,
            |row| {
                row.child(
                    div()
                        .id("account-chatgpt")
                        .child(ui::button("Sign out", ButtonKind::Secondary, t))
                        .on_click(cx.listener(|ws, _, _, cx| ws.sign_out(cx))),
                )
            },
        )
    });
    div()
        .flex()
        .flex_col()
        .gap(sp(2.))
        .child(header)
        .when(!access.accounts.is_empty(), |section| {
            section.child(div().flex().flex_col().gap(sp(0.5)).children(rows))
        })
        .children(actions)
}
