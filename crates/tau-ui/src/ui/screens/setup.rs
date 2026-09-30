//! Onboarding: sign in with GitHub, connect a model, pick repositories
//! and start the first run. On a desktop the screens sit under a
//! stepper; on a phone they stack under a plain header.

use gpui::{
    AnyElement,
    ClipboardItem,
    Context,
    Div,
    div,
    prelude::*,
    px,
    relative,
};

use crate::{
    assets::Icon,
    route::Route,
    setup::{CloneState, DeviceCode, GitHub, ModelAccess, SetupStep},
    theme::{Design as _, IconSize, MONO, Theme, Type, radius, sp, weight},
    ui::{
        bar,
        components::{
            ButtonKind,
            big_button,
            checkbox,
            field,
            label,
            lead,
            logo,
            notice,
            panel,
            phone_bar,
            phone_body,
            text_link,
            title,
        },
        heading,
        icon,
        mono,
        prose,
    },
    workspace::{Workspace, WorkspaceEvent},
};

pub fn render(
    ws: &Workspace,
    step: SetupStep,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let body = match step {
        SetupStep::Welcome => welcome(ws, compact, t, cx),
        SetupStep::GitHub if compact => phone_github(ws, t, cx),
        SetupStep::GitHub => github(ws, t, cx),
        SetupStep::Token => token(ws, compact, t, cx),
        SetupStep::Model => model(ws, compact, t, cx),
        SetupStep::Repos => repos(ws, compact, t, cx),
        SetupStep::Ready => ready(ws, compact, t, cx),
    };
    if compact {
        return div()
            .size_full()
            .flex()
            .flex_col()
            // Each step has its own big title; the header says where it
            // belongs, except for the sign-in, which reads like a task.
            .child(phone_bar(
                ws,
                match step {
                    SetupStep::GitHub => step.title(),
                    _ => "Set up",
                },
                t,
                cx,
            ))
            .child(phone_body("setup-body").child(body))
            .into_any_element();
    }
    div()
        .size_full()
        .flex()
        .flex_col()
        .typeset(Type::BODY)
        .child(top_bar(ws, step, t))
        .child(
            div()
                .id("setup-body")
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scroll()
                .flex()
                .justify_center()
                .items_start()
                .px(sp(6.))
                .py(sp(14.))
                .child(body),
        )
        .into_any_element()
}

/// The desktop's bar: tau, the four stages, and where you are.
fn top_bar(ws: &Workspace, step: SetupStep, t: &Theme) -> Div {
    let stage = step.stage();
    // GitHub can be skipped when a model is all the host can set up.
    let signed_in = ws.setup.user().is_some();
    let stages = SetupStep::STAGES.iter().enumerate().flat_map(|(n, name)| {
        let done = n < stage && (n > 0 || signed_in);
        let current = n == stage;
        let color = if done {
            t.green
        } else if current {
            t.text
        } else {
            t.dim
        };
        let ring = if done {
            t.green
        } else if current {
            t.accent
        } else {
            t.border_strong
        };
        let mark = if done {
            icon(Icon::Check, IconSize::SMALL, t.green).into_any_element()
        } else {
            mono((n + 1).to_string(), Type::MICRO, color).into_any_element()
        };
        let item = div()
            .flex()
            .items_center()
            .gap(sp(2.))
            .typeset(Type::SMALL)
            .text_color(color)
            .child(
                div()
                    .size(px(22.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(radius::FULL)
                    .border(px(1.5))
                    .border_color(ring)
                    .child(mark),
            )
            .child(*name)
            .into_any_element();
        let line = (n + 1 < SetupStep::STAGES.len()).then(|| {
            div()
                .w(px(28.))
                .h(px(1.))
                .bg(t.border_strong)
                .into_any_element()
        });
        [Some(item), line].into_iter().flatten()
    });
    div()
        .h(px(56.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(sp(3.))
        .px(sp(6.))
        .bg(t.panel)
        .border_b_1()
        .border_color(t.border)
        .child(logo(t, 28.))
        .child(div().font_weight(weight::STRONG).child("tau"))
        .child(div().text_color(t.dim).child("Set up"))
        .child(div().flex_1())
        .child(div().flex().items_center().gap(sp(2.5)).children(stages))
        .child(div().flex_1())
        .child(mono(
            format!("Step {} of 4", stage + 1),
            Type::CAPTION,
            t.dim,
        ))
}

/// A column of content: fixed width on a desktop, full width on a phone.
/// `gap` is in spacing steps.
fn column(width: f32, gap: f32, compact: bool) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(sp(gap))
        .when(compact, |col| col.w_full())
        .when(!compact, |col| col.w_full().max_w(px(width)))
}

fn heading_block(text: &str, sub: &str, compact: bool, t: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(sp(2.5))
        .child(title(
            text.to_owned(),
            if compact {
                Type::HEADLINE
            } else {
                Type::DISPLAY
            },
        ))
        .child(lead(sub, if compact { Type::BODY } else { Type::LEAD }, t))
}

fn welcome(
    ws: &Workspace,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let steps = [
        (
            "Sign in with GitHub",
            "To clone repositories and open pull requests. You choose which repositories.",
        ),
        (
            "Connect a model",
            "Your ChatGPT plan, through Sign in with ChatGPT.",
        ),
        (
            "Pick repositories",
            "Each one is cloned into tau's own storage and versioned with jj.",
        ),
    ];
    column(560., 7., compact)
        .when(!compact, |col| col.mt(sp(10.)))
        .child(logo(t, 48.))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(3.))
                .child(title("Tau runs coding agents on your repositories", if compact { Type::HEADLINE } else { Type::DISPLAY }, ))
                .child(lead(
                    "Sign in with GitHub so tau can clone the repositories you \
                     pick and open pull requests from finished runs. Then \
                     connect a model, and start your first run.",
                    if compact { Type::BODY } else { Type::LEAD },
                    t,
                )),
        )
        .child(panel(5., t).gap(sp(3.5)).children(
            steps.into_iter().enumerate().map(|(n, (name, detail))| {
                div()
                    .flex()
                    .items_start()
                    .gap(sp(3.))
                    .child(mono(
                        (n + 1).to_string(),
                        Type::BODY,
                        if n == 0 { t.accent } else { t.dim },
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .gap(sp(0.5))
                            .child(name)
                            .child(
                                div()
                                    .typeset(Type::SMALL)
                                    .text_color(t.muted)
                                    .child(detail),
                            ),
                    )
            }),
        ))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(sp(2.5))
                .child(
                    div()
                        .id("continue-github")
                        .when(compact, |button| button.w_full())
                        .child(big_button("Continue with GitHub", Some(Icon::Arrow), ButtonKind::Primary, t))
                        .on_click(
                            cx.listener(|ws, _, _, cx| ws.sign_in_github(cx)),
                        ),
                )
                .child(
                    div()
                        .typeset(Type::SMALL)
                        .text_color(t.dim)
                        .child("Takes about a minute."),
                )
                .child(skip_or_back(ws, Type::SMALL, t, cx)),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(2.))
                .typeset(Type::CAPTION)
                .child(icon(Icon::Lock, IconSize::COMPACT, t.dim))
                .child(div().flex_1().min_w(px(0.)).child(prose(
                    &format!(
                        "Tokens stay on this machine, in `{}`, readable only by you.",
                        ws.setup.config
                    ),
                    t.dim,
                    t,
                ))),
        )
        .into_any_element()
}

/// The code to enter, or what stands in for it while there is none.
fn code_or_status(github: &GitHub) -> Result<&DeviceCode, &'static str> {
    match github {
        GitHub::Waiting(code) => Ok(code),
        GitHub::SignedIn { .. } => Err("Signed in"),
        _ => Err("Asking GitHub for a code…"),
    }
}

fn open_device_page(code: &DeviceCode, cx: &mut gpui::App) {
    cx.write_to_clipboard(ClipboardItem::new_string(code.code.clone()));
    cx.open_url(&format!("https://{}", code.url));
}

fn github_status(ws: &Workspace, style: Type, t: &Theme) -> Div {
    match &ws.setup.github {
        GitHub::Failed(error) => {
            notice(Icon::Warning, error.clone(), t.red, style, t)
        }
        GitHub::SignedIn { user } => notice(
            Icon::Check,
            format!("Signed in as @{user}"),
            t.green,
            style,
            t,
        ),
        _ => notice(
            Icon::Spinner,
            if style.size < Type::BODY.size {
                "Waiting for you to approve on GitHub…"
            } else {
                "Waiting for approval…"
            },
            t.accent,
            style,
            t,
        ),
    }
}

const PERMISSIONS: [(Icon, &str, &str, &str); 3] = [
    (
        Icon::Repo,
        "Repository contents",
        "Clone, and push the branches tau's runs create. Never your default branch.",
        "read & write",
    ),
    (
        Icon::PullRequest,
        "Pull requests",
        "Open pull requests from runs, and update their descriptions.",
        "read & write",
    ),
    (
        Icon::Fork,
        "Metadata",
        "List the repositories you choose and their default branches.",
        "read",
    ),
];

fn github(
    ws: &Workspace,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let code = code_or_status(&ws.setup.github).ok().cloned();
    let shown = match code_or_status(&ws.setup.github) {
        Ok(code) => mono(code.code.clone(), Type::CODE_HERO, t.text),
        Err(status) => div()
            .typeset(Type::HEADING)
            .text_color(t.muted)
            .child(status),
    };
    let copy = code.clone();
    let open = code.clone();
    let left = div()
        .flex_1()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .gap(sp(5.))
        .child(title("Sign in with GitHub", Type::DISPLAY))
        .child(lead(
            "Open GitHub, enter this code, and approve the tau app. This \
             window moves on by itself once you have.",
            Type::LEAD,
            t,
        ))
        .child(
            panel(6., t).child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(sp(4.))
                    .py(sp(2.))
                    .child(
                        div()
                            .typeset(Type::CAPTION)
                            .text_color(t.muted)
                            .child("Your one-time code"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(sp(3.))
                            .child(shown)
                            .when_some(copy, |row, code| {
                                row.child(
                                    div()
                                        .id("copy-code")
                                        .size(px(44.))
                                        .flex_shrink_0()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded(radius::BOX)
                                        .border_1()
                                        .border_color(t.border_strong)
                                        .cursor_pointer()
                                        .child(icon(
                                            Icon::Copy,
                                            IconSize::LARGE,
                                            t.text_soft,
                                        ))
                                        .on_click(move |_, _, cx| {
                                            cx.write_to_clipboard(
                                                ClipboardItem::new_string(
                                                    code.code.clone(),
                                                ),
                                            )
                                        }),
                                )
                            }),
                    )
                    .children(code.as_ref().map(|code| {
                        div().typeset(Type::SMALL).child(prose(
                            &format!(
                                "at `{}` · expires in {}",
                                code.url, code.expires
                            ),
                            t.muted,
                            t,
                        ))
                    })),
            ),
        )
        .child(
            div()
                .flex()
                .gap(sp(2.5))
                .child(
                    div()
                        .id("open-github")
                        .child(big_button(
                            "Open GitHub",
                            Some(Icon::Arrow),
                            ButtonKind::Primary,
                            t,
                        ))
                        .on_click(move |_, _, cx| {
                            if let Some(code) = &open {
                                open_device_page(code, cx)
                            }
                        }),
                )
                .child(
                    div()
                        .id("approved")
                        .child(big_button(
                            "I have approved it",
                            None,
                            ButtonKind::Secondary,
                            t,
                        ))
                        .on_click(cx.listener(|_, _, _, cx| {
                            cx.emit(WorkspaceEvent::GitHubCheck)
                        })),
                ),
        )
        .child(github_status(ws, Type::SMALL, t))
        .child(
            div()
                .id("use-token")
                .child(text_link(
                    "Use a fine-grained personal access token instead",
                    Type::SMALL,
                    t,
                ))
                .on_click(cx.listener(|ws, _, _, cx| {
                    ws.navigate(Route::Setup(SetupStep::Token), cx)
                })),
        )
        .child(skip_or_back(ws, Type::SMALL, t, cx));

    let rows = PERMISSIONS.iter().map(|(glyph, name, detail, level)| {
        div()
            .flex()
            .items_start()
            .gap(sp(3.))
            .py(sp(3.))
            .border_b_1()
            .border_color(t.border)
            .child(div().mt(sp(0.5)).child(icon(
                *glyph,
                IconSize::LARGE,
                t.muted,
            )))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .gap(sp(0.75))
                    .child(*name)
                    .child(
                        div()
                            .typeset(Type::SMALL)
                            .line_height(relative(1.3))
                            .text_color(t.muted)
                            .child(*detail),
                    ),
            )
            .child(
                crate::ui::tag(*level, Type::CAPTION, t.text_soft, t)
                    .px(sp(2.)),
            )
    });
    let right = div()
        .flex_1()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .gap(sp(4.))
        .child(heading("What tau asks for", t))
        .child(
            panel(5., t).pt(sp(2.)).children(rows).child(
                div()
                    .flex()
                    .items_start()
                    .gap(sp(3.))
                    .pt(sp(3.))
                    .child(div().mt(sp(0.5)).child(icon(
                        Icon::Lock,
                        IconSize::LARGE,
                        t.muted,
                    )))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .typeset(Type::SMALL)
                            .text_color(t.muted)
                            .line_height(relative(1.5))
                            .child(
                                "tau signs in as a GitHub App, installed \
                                     only on the repositories you pick in the \
                                     next steps. It never sees the others, and \
                                     you can remove it from GitHub's settings \
                                     at any time.",
                            ),
                    ),
            ),
        );
    div()
        .w_full()
        .max_w(px(960.))
        .flex()
        .gap(sp(8.))
        .child(left)
        .child(right)
        .into_any_element()
}

fn phone_github(
    ws: &Workspace,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let code = code_or_status(&ws.setup.github).ok().cloned();
    let shown = match code_or_status(&ws.setup.github) {
        Ok(code) => mono(code.code.clone(), Type::CODE_LARGE, t.text),
        Err(status) => {
            div().typeset(Type::TITLE).text_color(t.muted).child(status)
        }
    };
    div()
        .flex()
        .flex_col()
        .gap(sp(4.5))
        .child(heading_block(
            "Enter this code on GitHub",
            "Approve the tau app, then come back. This screen moves on by itself.",
            true,
            t,
        ))
        .child(
            panel(5., t).items_center().gap(sp(3.)).child(shown).children(
                code.as_ref().map(|code| {
                    div()
                        .typeset(Type::SMALL)
                        .text_color(t.muted)
                        .child(format!("expires in {}", code.expires))
                }),
            ),
        )
        .child(
            div()
                .id("phone-open-github")
                .child(big_button("Copy code and open GitHub", Some(Icon::Arrow), ButtonKind::Primary, t))
                .on_click(move |_, _, cx| {
                    if let Some(code) = &code {
                        open_device_page(code, cx)
                    }
                }),
        )
        .child(github_status(ws, Type::BODY, t))
        .child(heading("tau asks for", t))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(2.))
                .typeset(Type::BODY)
                .text_color(t.text_soft)
                .children(PERMISSIONS.iter().map(|(_, name, _, level)| {
                    let level = level.replace('&', "and");
                    div().child(format!("{}: {level}", name.replace("Repository contents", "Contents")))
                })),
        )
        .child(
            div()
                .typeset(Type::SMALL)
                .text_color(t.muted)
                .child("Only on the repositories you pick next."),
        )
        .child(
            div()
                .id("phone-use-token")
                .child(text_link("Use a personal access token instead", Type::BODY, t))
                .on_click(cx.listener(|ws, _, _, cx| {
                    ws.navigate(Route::Setup(SetupStep::Token), cx)
                })),
        )
        .child(skip_or_back(ws, Type::BODY, t, cx))
        .into_any_element()
}

/// From onboarding, a way past GitHub to the model; from the app, the
/// way back.
fn skip_or_back(
    ws: &Workspace,
    style: Type,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let from_app = ws.setup_goal.is_some();
    div()
        .id("skip-github")
        .child(text_link(
            if from_app {
                "Back"
            } else {
                "Skip GitHub for now"
            },
            style,
            t,
        ))
        .on_click(cx.listener(move |ws, _, _, cx| {
            if from_app {
                ws.leave_setup(cx)
            } else {
                ws.navigate(Route::Setup(SetupStep::Model), cx)
            }
        }))
}

fn token(
    ws: &Workspace,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let checking = ws.setup.github == GitHub::Checking;
    column(560., 5., compact)
        .when(!compact, |col| {
            col.child(
                div()
                    .id("token-back")
                    .flex()
                    .items_center()
                    .gap(sp(1.5))
                    .cursor_pointer()
                    .typeset(Type::SMALL)
                    .text_color(t.blue)
                    .child(icon(Icon::Back, IconSize::BASE, t.blue))
                    .child("Back to GitHub sign-in")
                    .on_click(cx.listener(|ws, _, _, cx| {
                        ws.navigate(Route::Setup(SetupStep::GitHub), cx)
                    })),
            )
        })
        .child(title("Use a personal access token", if compact { Type::HEADLINE } else { Type::DISPLAY }, ))
        .child(lead("For machines where the app cannot open a browser. Create a \
             fine-grained token limited to the repositories tau should reach.", if compact { Type::BODY } else { Type::LEAD }, t, ))
        .child(
            panel(5., t)
                .gap(sp(4.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(sp(2.))
                        .child(label("Fine-grained token", t))
                        .child(field(&ws.github_token, true, t)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(sp(1.5))
                        .typeset(Type::SMALL)
                        .text_color(t.muted)
                        .child("Give it these repository permissions:")
                        .child(mono(
                            "Contents: read and write · Pull requests: read and \
                             write · Metadata: read",
                            Type::SMALL,
                            t.text_soft,
                        )),
                ),
        )
        .when_some(
            match &ws.setup.github {
                GitHub::Failed(error) => Some(error.clone()),
                _ => None,
            },
            |col, error| col.child(notice(Icon::Warning, error, t.red, Type::SMALL, t)),
        )
        .child(
            div().flex().child(
                div()
                    .id("check-token")
                    .when(compact, |button| button.w_full())
                    .child(big_button(if checking { "Checking…" } else { "Check the token" }, None, ButtonKind::Primary, t))
                    .on_click(cx.listener(|ws, _, _, cx| {
                        ws.submit_token_from_button(cx)
                    })),
            ),
        )
        .into_any_element()
}

/// The way tau reaches a model: the ChatGPT plan.
fn option(glyph: Icon, name: &str, detail: &str, t: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(sp(3.5))
        .p(sp(5.))
        .rounded(radius::CARD)
        .border_1()
        .border_color(t.accent)
        .bg(t.accent_soft)
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(2.5))
                .child(icon(glyph, IconSize::XLARGE, t.text_soft))
                .child(
                    div()
                        .flex_1()
                        .font_weight(weight::STRONG)
                        .child(name.to_owned()),
                ),
        )
        .child(
            div()
                .typeset(Type::SMALL)
                .line_height(relative(1.55))
                .child(prose(detail, t.muted, t)),
        )
}

fn model(
    ws: &Workspace,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let chatgpt_state = match &ws.setup.model {
        ModelAccess::SigningIn { url: Some(url) } => {
            let url = url.clone();
            Some(
                div()
                    .flex()
                    .flex_col()
                    .gap(sp(2.5))
                    .child(notice(
                        Icon::Spinner,
                        "Finish signing in in your browser…",
                        t.accent,
                        Type::SMALL,
                        t,
                    ))
                    .child(
                        div()
                            .id("chatgpt-open-again")
                            .child(text_link(
                                "The page did not open? Open it again",
                                Type::SMALL,
                                t,
                            ))
                            .on_click(move |_, _, cx| cx.open_url(&url)),
                    )
                    .child(label(
                        "Or paste the address the browser ended on",
                        t,
                    ))
                    .child(field(&ws.chatgpt_callback, true, t))
                    .child(
                        div()
                            .id("chatgpt-paste")
                            .child(big_button(
                                "Finish signing in",
                                None,
                                ButtonKind::Secondary,
                                t,
                            ))
                            .on_click(cx.listener(|ws, _, _, cx| {
                                ws.submit_chatgpt_callback_from_button(cx)
                            })),
                    ),
            )
        }
        ModelAccess::SigningIn { url: None } => Some(div().child(notice(
            Icon::Spinner,
            "Opening the sign-in page…",
            t.accent,
            Type::SMALL,
            t,
        ))),
        ModelAccess::PlanDisabled { account } => Some(
            div()
                .flex()
                .flex_col()
                .gap(sp(2.5))
                .child(notice(
                    Icon::Warning,
                    format!(
                        "Signed in as {account}, but ChatGPT plan use isn't \
                         enabled. Enable it to run tasks on your plan."
                    ),
                    t.accent,
                    Type::SMALL,
                    t,
                ))
                .child(
                    div()
                        .id("chatgpt-enable")
                        .child(big_button(
                            "Enable ChatGPT plan use",
                            None,
                            ButtonKind::Secondary,
                            t,
                        ))
                        .on_click(
                            cx.listener(|ws, _, _, cx| {
                                ws.enable_plan_usage(cx)
                            }),
                        ),
                ),
        ),
        ModelAccess::Failed(error) => Some(div().child(notice(
            Icon::Warning,
            error.clone(),
            t.red,
            Type::SMALL,
            t,
        ))),
        _ => None,
    };
    let chatgpt = option(
        Icon::Chat,
        "ChatGPT plan",
        "Sign in with ChatGPT to use your plan. Eligible usage counts \
         against your plan's limits.",
        t,
    )
    .child(
        div()
            .flex()
            .flex_col()
            .gap(sp(2.5))
            .child(
                div()
                    .id("chatgpt-sign-in")
                    .child(big_button(
                        "Continue with ChatGPT",
                        Some(Icon::Arrow),
                        ButtonKind::Primary,
                        t,
                    ))
                    .on_click(cx.listener(|ws, _, _, cx| {
                        ws.sign_in_chatgpt(None, false, cx)
                    })),
            )
            .children(chatgpt_state),
    );
    column(640., 6., compact)
        .when(ws.setup_goal.is_some(), |col| {
            col.child(
                div().flex().child(
                    div()
                        .id("setup-back")
                        .child(text_link("Back", Type::SMALL, t))
                        .on_click(
                            cx.listener(|ws, _, _, cx| ws.leave_setup(cx)),
                        ),
                ),
            )
        })
        .when_some(ws.setup.user(), |col, user| {
            col.child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(2.5))
                    .typeset(Type::SMALL)
                    .child(icon(Icon::Check, IconSize::BASE, t.green))
                    .child(prose(
                        &format!("Signed in to GitHub as `@{user}`"),
                        t.green,
                        t,
                    )),
            )
        })
        .child(heading_block(
            "Connect a model",
            "Runs use your ChatGPT plan through OpenAI's Responses API. \
             Sign in with ChatGPT and allow plan use.",
            compact,
            t,
        ))
        .child(chatgpt)
        .child(
            div()
                .flex()
                .items_start()
                .gap(sp(2.5))
                .px(sp(3.5))
                .py(sp(3.))
                .rounded(radius::BOX)
                .border_1()
                .border_dashed()
                .border_color(t.border_strong)
                .typeset(Type::SMALL)
                .text_color(t.muted)
                .line_height(relative(1.5))
                .child(div().mt(sp(0.25)).child(icon(
                    Icon::Info,
                    IconSize::MEDIUM,
                    t.muted,
                )))
                .child(div().flex_1().min_w(px(0.)).child(
                    "tau keeps each ChatGPT sign-in in its config directory, \
                     readable only by you. You can review what apps use of \
                     your plan in ChatGPT settings.",
                )),
        )
        .into_any_element()
}

fn repos(
    ws: &Workspace,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let filter = ws.repo_filter.read(cx).text().to_owned();
    let rows: Vec<_> = ws
        .setup
        .matching(&filter)
        .enumerate()
        .map(|(n, repo)| {
            let name = repo.name.clone();
            div()
                .id(("repo", n))
                .flex()
                .items_center()
                .gap(sp(3.5))
                .min_h(px(56.))
                .px(sp(4.))
                .border_b_1()
                .border_color(t.border)
                .cursor_pointer()
                .when(repo.selected, |row| row.bg(t.accent.opacity(0.05)))
                .child(checkbox(repo.selected, false, t))
                .child(icon(Icon::Repo, IconSize::LARGE, t.muted))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .gap(sp(0.5))
                        .child(mono(repo.name.clone(), Type::SMALL, t.text))
                        .when(!repo.description.is_empty(), |col| {
                            col.child(
                                div()
                                    .typeset(Type::CAPTION)
                                    .text_color(t.muted)
                                    .child(repo.description.clone()),
                            )
                        }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(1.5))
                        .child(icon(Icon::Fork, IconSize::SMALL, t.dim))
                        .child(mono(repo.branch.clone(), Type::CAPTION, t.dim)),
                )
                .on_click(
                    cx.listener(move |ws, _, _, cx| ws.toggle_repo(&name, cx)),
                )
        })
        .collect();
    let selected: Vec<String> =
        ws.setup.selected().map(|repo| repo.name.clone()).collect();
    let count = selected.len();
    let left = div()
        .flex_1()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .gap(sp(4.5))
        .child(heading_block(
            "Pick repositories",
            "tau clones each one into its own storage. Runs work there, never \
             in your own checkouts.",
            compact,
            t,
        ))
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(2.))
                .h(px(44.))
                .px(sp(3.))
                .rounded(radius::BOX)
                .border_1()
                .border_color(t.border_strong)
                .child(icon(Icon::Search, IconSize::LARGE, t.dim))
                .child(ws.repo_filter.clone()),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .rounded(radius::CARD)
                .border_1()
                .border_color(t.border)
                .bg(t.card)
                .flex_shrink_0()
                .overflow_hidden()
                .children(rows),
        );
    let mut paths = vec![ws.setup.storage.clone()];
    paths.extend(selected);
    let right = div()
        .flex()
        .flex_col()
        .gap(sp(4.))
        .when(!compact, |col| col.w(px(320.)).flex_shrink_0().pt(sp(22.)))
        .child(
            panel(4.5, t)
                .gap(sp(3.5))
                .child(heading("Where they go", t))
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap(sp(2.5))
                        .child(icon(Icon::Folder, IconSize::LARGE, t.muted))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .font_family(MONO)
                                .typeset(Type::CAPTION)
                                .text_color(t.text_soft)
                                .line_height(relative(1.6))
                                .children(paths),
                        ),
                )
                .child(
                    div()
                        .typeset(Type::SMALL)
                        .text_color(t.muted)
                        .line_height(relative(1.55))
                        .child(
                            "Each is a jj repository with its Git store inside. \
                             Every turn of a run is a commit you can go back \
                             to, fork from, or compare.",
                        ),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(2.))
                .child(
                    div()
                        .id("clone")
                        .child(
                            big_button(match count {
                                    0 => "Pick a repository".to_owned(),
                                    1 => "Clone 1 repository".to_owned(),
                                    n => format!("Clone {n} repositories"),
                                }, None, ButtonKind::Primary, t)
                            .when(count == 0, |button| button.opacity(0.5)),
                        )
                        .on_click(cx.listener(move |ws, _, _, cx| {
                            if count > 0 {
                                ws.clone_selected(cx)
                            }
                        })),
                )
                .child(
                    div()
                        .id("install-app")
                        .flex()
                        .justify_center()
                        .child(text_link(
                            "Missing one? Give tau's GitHub App access to it",
                            Type::CAPTION,
                            t,
                        ))
                        .on_click(|_, _, cx| {
                            cx.open_url(&crate::github::install_url())
                        }),
                ),
        );
    div()
        .w_full()
        .max_w(px(1040.))
        .flex()
        .gap(sp(7.))
        .when(compact, |row| row.flex_col())
        .child(left)
        .child(right)
        .into_any_element()
}

fn ready(
    ws: &Workspace,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let clones = ws.setup.clones.iter().map(|clone| {
        let (state, share, color) = match &clone.state {
            CloneState::Cloning { share, detail } => (
                if detail.is_empty() {
                    "cloning".to_owned()
                } else {
                    format!("cloning · {detail}")
                },
                *share,
                t.accent,
            ),
            CloneState::Ready => ("ready".to_owned(), 1.0, t.green),
            CloneState::Failed(error) => (error.clone(), 1.0, t.red),
        };
        div()
            .flex()
            .flex_col()
            .gap(sp(2.))
            .px(sp(4.))
            .py(sp(3.5))
            .border_b_1()
            .border_color(t.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(2.5))
                    .child(
                        mono(clone.name.clone(), Type::SMALL, t.text).flex_1(),
                    )
                    .child(
                        div()
                            .typeset(Type::CAPTION)
                            .text_color(color)
                            .child(state),
                    ),
            )
            .child(bar(share, 4., color, t.border))
    });
    let repo = ws
        .setup
        .clones
        .first()
        .map(|clone| clone.name.clone())
        .unwrap_or_else(|| ws.name.clone());
    let chip = |glyph: Icon, text: String, color| {
        crate::ui::chip(Some(glyph), text, Type::CAPTION, color, t)
    };
    column(760., 6., compact)
        .child(heading_block(
            "Start your first run",
            "Describe a task. tau works on a new change in the repository, and \
             you can open a pull request when you like the result.",
            compact,
            t,
        ))
        .when(!ws.setup.clones.is_empty(), |col| {
            col.child(
                div()
                    .flex()
                    .flex_col()
                    .rounded(radius::CARD)
                    .border_1()
                    .border_color(t.border)
                    .bg(t.card)
                    .flex_shrink_0()
                    .overflow_hidden()
                    .children(clones),
            )
        })
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(3.))
                .p(sp(3.5))
                .rounded(radius::CARD)
                .border_1()
                .border_color(t.border_strong)
                .bg(t.panel)
                .child(
                    div()
                        .flex()
                        .items_start()
                        .h(px(if compact { 48. } else { 64. }))
                        .pt(sp(0.5))
                        .child(ws.first_task.clone()),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(sp(2.))
                        .child(chip(Icon::Repo, repo, t.text_soft))
                        .child(chip(
                            Icon::Chat,
                            ws.setup
                                .model_label()
                                .unwrap_or("no model yet")
                                .to_owned(),
                            t.muted,
                        ))
                        .child(div().flex_1())
                        .child(
                            div()
                                .id("first-run")
                                .when(compact, |button| button.w_full())
                                .child(big_button(
                                    "Start",
                                    None,
                                    ButtonKind::Primary,
                                    t,
                                ))
                                .on_click(cx.listener(|ws, _, _, cx| {
                                    ws.start_first_run_from_button(cx)
                                })),
                        ),
                ),
        )
        .into_any_element()
}
