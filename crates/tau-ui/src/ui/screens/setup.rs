//! Onboarding: sign in with GitHub, connect a model, pick repositories
//! and start the first run. On a desktop the screens sit under a
//! stepper; on a phone they stack under a plain header.

use gpui::{
    AnyElement,
    ClipboardItem,
    Context,
    Div,
    FontWeight,
    div,
    prelude::*,
    px,
    relative,
};

use crate::{
    assets::Icon,
    route::Route,
    setup::{CloneState, DeviceCode, GitHub, ModelAccess, SetupStep},
    theme::{MONO, Theme},
    ui::{
        bar,
        chrome::logo,
        form::{
            big_button,
            checkbox,
            field,
            label,
            lead,
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
        .text_size(px(14.))
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
                .px(px(24.))
                .py(px(56.))
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
            icon(Icon::Check, 12., t.green).into_any_element()
        } else {
            mono((n + 1).to_string(), 11., color).into_any_element()
        };
        let item = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(px(13.))
            .text_color(color)
            .child(
                div()
                    .size(px(22.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(11.))
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
        .gap(px(12.))
        .px(px(24.))
        .bg(t.panel)
        .border_b_1()
        .border_color(t.border)
        .child(logo(t, 28.))
        .child(div().font_weight(FontWeight::SEMIBOLD).child("tau"))
        .child(div().text_color(t.dim).child("Set up"))
        .child(div().flex_1())
        .child(div().flex().items_center().gap(px(10.)).children(stages))
        .child(div().flex_1())
        .child(mono(format!("Step {} of 4", stage + 1), 12., t.dim))
}

/// A column of content: fixed width on a desktop, full width on a phone.
fn column(width: f32, gap: f32, compact: bool) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(gap))
        .when(compact, |col| col.w_full())
        .when(!compact, |col| col.w_full().max_w(px(width)))
}

fn heading_block(text: &str, sub: &str, compact: bool, t: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(title(text.to_owned(), if compact { 22. } else { 28. }))
        .child(lead(sub, if compact { 14. } else { 15. }, t))
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
            "A ChatGPT Plus or Pro sign-in, or an OpenAI API key.",
        ),
        (
            "Pick repositories",
            "Each one is cloned into tau's own storage and versioned with jj.",
        ),
    ];
    column(560., 28., compact)
        .when(!compact, |col| col.mt(px(40.)))
        .child(logo(t, 48.))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(12.))
                .child(title(
                    "Tau runs coding agents on your repositories",
                    if compact { 22. } else { 28. },
                ))
                .child(lead(
                    "Sign in with GitHub so tau can clone the repositories you \
                     pick and open pull requests from finished runs. Then \
                     connect a model, and start your first run.",
                    if compact { 14. } else { 15. },
                    t,
                )),
        )
        .child(panel(20., t).gap(px(14.)).children(
            steps.into_iter().enumerate().map(|(n, (name, detail))| {
                div()
                    .flex()
                    .items_start()
                    .gap(px(12.))
                    .child(mono(
                        (n + 1).to_string(),
                        14.,
                        if n == 0 { t.accent } else { t.dim },
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(name)
                            .child(
                                div()
                                    .text_size(px(13.))
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
                .gap(px(10.))
                .child(
                    div()
                        .id("continue-github")
                        .when(compact, |button| button.w_full())
                        .child(big_button(
                            "Continue with GitHub",
                            Some(Icon::Arrow),
                            true,
                            t,
                        ))
                        .on_click(
                            cx.listener(|ws, _, _, cx| ws.sign_in_github(cx)),
                        ),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(t.dim)
                        .child("Takes about a minute."),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(px(12.))
                .child(icon(Icon::Lock, 13., t.dim))
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

fn github_status(ws: &Workspace, size: f32, t: &Theme) -> Div {
    match &ws.setup.github {
        GitHub::Failed(error) => {
            notice(Icon::Warning, error.clone(), t.red, size, t)
        }
        GitHub::SignedIn { user } => notice(
            Icon::Check,
            format!("Signed in as @{user}"),
            t.green,
            size,
            t,
        ),
        _ => notice(
            Icon::Spinner,
            if size < 14. {
                "Waiting for you to approve on GitHub…"
            } else {
                "Waiting for approval…"
            },
            t.accent,
            size,
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
        Ok(code) => mono(code.code.clone(), 40., t.text),
        Err(status) => {
            div().text_size(px(20.)).text_color(t.muted).child(status)
        }
    };
    let copy = code.clone();
    let open = code.clone();
    let left = div()
        .flex_1()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .gap(px(20.))
        .child(title("Sign in with GitHub", 28.))
        .child(lead(
            "Open GitHub, enter this code, and approve the tau app. This \
             window moves on by itself once you have.",
            15.,
            t,
        ))
        .child(
            panel(24., t).child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(16.))
                    .py(px(8.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(t.muted)
                            .child("Your one-time code"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(12.))
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
                                        .rounded(px(8.))
                                        .border_1()
                                        .border_color(t.border_strong)
                                        .cursor_pointer()
                                        .child(icon(
                                            Icon::Copy,
                                            16.,
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
                        div().text_size(px(13.)).child(prose(
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
                .gap(px(10.))
                .child(
                    div()
                        .id("open-github")
                        .child(big_button(
                            "Open GitHub",
                            Some(Icon::Arrow),
                            true,
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
                        .child(big_button("I have approved it", None, false, t))
                        .on_click(cx.listener(|_, _, _, cx| {
                            cx.emit(WorkspaceEvent::GitHubCheck)
                        })),
                ),
        )
        .child(github_status(ws, 13., t))
        .child(
            div()
                .id("use-token")
                .child(text_link(
                    "Use a fine-grained personal access token instead",
                    13.,
                    t,
                ))
                .on_click(cx.listener(|ws, _, _, cx| {
                    ws.navigate(Route::Setup(SetupStep::Token), cx)
                })),
        );

    let rows = PERMISSIONS.iter().map(|(glyph, name, detail, level)| {
        div()
            .flex()
            .items_start()
            .gap(px(12.))
            .py(px(12.))
            .border_b_1()
            .border_color(t.border)
            .child(div().mt(px(2.)).child(icon(*glyph, 16., t.muted)))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(*name)
                    .child(
                        div()
                            .text_size(px(13.))
                            .line_height(relative(1.3))
                            .text_color(t.muted)
                            .child(*detail),
                    ),
            )
            .child(
                mono(*level, 12., t.text_soft)
                    .flex_shrink_0()
                    .px(px(8.))
                    .py(px(2.))
                    .rounded(px(4.))
                    .bg(t.raised),
            )
    });
    let right = div()
        .flex_1()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .gap(px(16.))
        .child(heading("What tau asks for", t))
        .child(
            panel(20., t).pt(px(8.)).children(rows).child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(12.))
                    .pt(px(12.))
                    .child(div().mt(px(2.)).child(icon(
                        Icon::Lock,
                        16.,
                        t.muted,
                    )))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_size(px(13.))
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
        .gap(px(32.))
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
        Ok(code) => mono(code.code.clone(), 26., t.text),
        Err(status) => {
            div().text_size(px(17.)).text_color(t.muted).child(status)
        }
    };
    div()
        .flex()
        .flex_col()
        .gap(px(18.))
        .child(heading_block(
            "Enter this code on GitHub",
            "Approve the tau app, then come back. This screen moves on by itself.",
            true,
            t,
        ))
        .child(
            panel(20., t).items_center().gap(px(12.)).child(shown).children(
                code.as_ref().map(|code| {
                    div()
                        .text_size(px(13.))
                        .text_color(t.muted)
                        .child(format!("expires in {}", code.expires))
                }),
            ),
        )
        .child(
            div()
                .id("phone-open-github")
                .child(big_button(
                    "Copy code and open GitHub",
                    Some(Icon::Arrow),
                    true,
                    t,
                ))
                .on_click(move |_, _, cx| {
                    if let Some(code) = &code {
                        open_device_page(code, cx)
                    }
                }),
        )
        .child(github_status(ws, 14., t))
        .child(heading("tau asks for", t))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .text_size(px(14.))
                .text_color(t.text_soft)
                .children(PERMISSIONS.iter().map(|(_, name, _, level)| {
                    let level = level.replace('&', "and");
                    div().child(format!("{}: {level}", name.replace("Repository contents", "Contents")))
                })),
        )
        .child(
            div()
                .text_size(px(13.))
                .text_color(t.muted)
                .child("Only on the repositories you pick next."),
        )
        .child(
            div()
                .id("phone-use-token")
                .child(text_link("Use a personal access token instead", 14., t))
                .on_click(cx.listener(|ws, _, _, cx| {
                    ws.navigate(Route::Setup(SetupStep::Token), cx)
                })),
        )
        .into_any_element()
}

fn token(
    ws: &Workspace,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let checking = ws.setup.github == GitHub::Checking;
    column(560., 20., compact)
        .when(!compact, |col| {
            col.child(
                div()
                    .id("token-back")
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .cursor_pointer()
                    .text_size(px(13.))
                    .text_color(t.blue)
                    .child(icon(Icon::Back, 14., t.blue))
                    .child("Back to GitHub sign-in")
                    .on_click(cx.listener(|ws, _, _, cx| {
                        ws.navigate(Route::Setup(SetupStep::GitHub), cx)
                    })),
            )
        })
        .child(title(
            "Use a personal access token",
            if compact { 22. } else { 28. },
        ))
        .child(lead(
            "For machines where the app cannot open a browser. Create a \
             fine-grained token limited to the repositories tau should reach.",
            if compact { 14. } else { 15. },
            t,
        ))
        .child(
            panel(20., t)
                .gap(px(16.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(label("Fine-grained token", t))
                        .child(field(&ws.github_token, true, t)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .text_size(px(13.))
                        .text_color(t.muted)
                        .child("Give it these repository permissions:")
                        .child(mono(
                            "Contents: read and write · Pull requests: read and \
                             write · Metadata: read",
                            13.,
                            t.text_soft,
                        )),
                ),
        )
        .when_some(
            match &ws.setup.github {
                GitHub::Failed(error) => Some(error.clone()),
                _ => None,
            },
            |col, error| col.child(notice(Icon::Warning, error, t.red, 13., t)),
        )
        .child(
            div().flex().child(
                div()
                    .id("check-token")
                    .when(compact, |button| button.w_full())
                    .child(big_button(
                        if checking { "Checking…" } else { "Check the token" },
                        None,
                        true,
                        t,
                    ))
                    .on_click(cx.listener(|ws, _, _, cx| {
                        ws.submit_token_from_button(cx)
                    })),
            ),
        )
        .into_any_element()
}

/// One way to pay for a model.
fn option(
    glyph: Icon,
    name: &str,
    detail: &str,
    badge: Option<&str>,
    selected: bool,
    t: &Theme,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(14.))
        .p(px(20.))
        .rounded(px(12.))
        .border_1()
        .border_color(if selected { t.accent } else { t.border })
        .bg(if selected { t.accent_soft } else { t.panel })
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(icon(glyph, 18., t.text_soft))
                .child(
                    div()
                        .flex_1()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(name.to_owned()),
                )
                .children(badge.map(|badge| {
                    div()
                        .px(px(8.))
                        .py(px(2.))
                        .rounded(px(10.))
                        .border_1()
                        .border_color(t.accent_border)
                        .text_size(px(11.))
                        .text_color(t.accent)
                        .child(badge.to_owned())
                })),
        )
        .child(
            div()
                .text_size(px(13.))
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
    let codex_state = match &ws.setup.model {
        ModelAccess::SigningIn {
            device: Some(code), ..
        } => Some(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(6.))
                .child(mono(code.code.clone(), 22., t.text))
                .child(div().text_size(px(13.)).child(prose(
                    &format!("Enter it at `{}`", code.url),
                    t.muted,
                    t,
                ))),
        ),
        ModelAccess::SigningIn { .. } => Some(notice(
            Icon::Spinner,
            "Finish signing in in your browser…",
            t.accent,
            13.,
            t,
        )),
        ModelAccess::Failed(error) => {
            Some(notice(Icon::Warning, error.clone(), t.red, 13., t))
        }
        _ => None,
    };
    let codex = option(
        Icon::Chat,
        "ChatGPT Plus or Pro",
        "Use your subscription through OpenAI Codex. Runs count against \
         your plan's limits, not an API bill.",
        Some("Recommended"),
        true,
        t,
    )
    .child(
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .id("codex-sign-in")
                    .child(big_button(
                        "Sign in with ChatGPT",
                        Some(Icon::Arrow),
                        true,
                        t,
                    ))
                    .on_click(
                        cx.listener(|ws, _, _, cx| ws.sign_in_codex(false, cx)),
                    ),
            )
            .child(
                div().flex().justify_center().child(
                    div()
                        .id("codex-device")
                        .child(text_link(
                            "No browser here? Use a device code",
                            13.,
                            t,
                        ))
                        .on_click(cx.listener(|ws, _, _, cx| {
                            ws.sign_in_codex(true, cx)
                        })),
                ),
            )
            .children(codex_state),
    );
    let api = option(
        Icon::Key,
        "OpenAI API key",
        "Pay per token on your OpenAI account. tau reads it from \
         `OPENAI_API_KEY` or stores it here.",
        None,
        false,
        t,
    )
    .child(
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(label("API key", t))
            .child(field(&ws.api_key, true, t))
            .child(
                div()
                    .id("use-key")
                    .child(big_button("Use this key", None, false, t))
                    .on_click(cx.listener(|ws, _, _, cx| {
                        ws.submit_api_key_from_button(cx)
                    })),
            ),
    );
    let options = div()
        .flex()
        .gap(px(16.))
        .when(compact, |row| row.flex_col())
        .child(codex.flex_1().min_w(px(0.)))
        .child(api.flex_1().min_w(px(0.)));
    column(960., 24., compact)
        .when_some(ws.setup.user(), |col, user| {
            col.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .text_size(px(13.))
                    .child(icon(Icon::Check, 14., t.green))
                    .child(prose(
                        &format!("Signed in to GitHub as `@{user}`"),
                        t.green,
                        t,
                    )),
            )
        })
        .child(heading_block(
            "Connect a model",
            "Runs talk to OpenAI's Responses API over one WebSocket per run. \
             Pick how tau pays for it.",
            compact,
            t,
        ))
        .child(options)
        .child(
            div()
                .flex()
                .items_start()
                .gap(px(10.))
                .px(px(14.))
                .py(px(12.))
                .rounded(px(8.))
                .border_1()
                .border_dashed()
                .border_color(t.border_strong)
                .text_size(px(13.))
                .text_color(t.muted)
                .line_height(relative(1.5))
                .child(div().mt(px(1.)).child(icon(Icon::Info, 15., t.muted)))
                .child(div().flex_1().min_w(px(0.)).child(
                    "Signed in to the Codex CLI already? tau does not reuse its \
                     sign-in: refreshing a shared token would sign the CLI out. \
                     Sign in here once instead.",
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
                .gap(px(14.))
                .min_h(px(56.))
                .px(px(16.))
                .border_b_1()
                .border_color(t.border)
                .cursor_pointer()
                .when(repo.selected, |row| row.bg(t.accent.opacity(0.05)))
                .child(checkbox(repo.selected, 18., t))
                .child(icon(Icon::Repo, 16., t.muted))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(mono(repo.name.clone(), 13., t.text))
                        .when(!repo.description.is_empty(), |col| {
                            col.child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(t.muted)
                                    .child(repo.description.clone()),
                            )
                        }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(icon(Icon::Fork, 12., t.dim))
                        .child(mono(repo.branch.clone(), 12., t.dim)),
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
        .gap(px(18.))
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
                .gap(px(8.))
                .h(px(44.))
                .px(px(12.))
                .rounded(px(8.))
                .border_1()
                .border_color(t.border_strong)
                .child(icon(Icon::Search, 16., t.dim))
                .child(ws.repo_filter.clone()),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .rounded(px(12.))
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
        .gap(px(16.))
        .when(!compact, |col| col.w(px(320.)).flex_shrink_0().pt(px(88.)))
        .child(
            panel(18., t)
                .gap(px(14.))
                .child(heading("Where they go", t))
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap(px(10.))
                        .child(icon(Icon::Folder, 16., t.muted))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .font_family(MONO)
                                .text_size(px(12.))
                                .text_color(t.text_soft)
                                .line_height(relative(1.6))
                                .children(paths),
                        ),
                )
                .child(
                    div()
                        .text_size(px(13.))
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
                .gap(px(8.))
                .child(
                    div()
                        .id("clone")
                        .child(
                            big_button(
                                match count {
                                    0 => "Pick a repository".to_owned(),
                                    1 => "Clone 1 repository".to_owned(),
                                    n => format!("Clone {n} repositories"),
                                },
                                None,
                                true,
                                t,
                            )
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
                        .flex()
                        .justify_center()
                        .text_size(px(12.))
                        .text_color(t.dim)
                        .child("You can add more later from Settings."),
                ),
        );
    div()
        .w_full()
        .max_w(px(1040.))
        .flex()
        .gap(px(28.))
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
            .gap(px(8.))
            .px(px(16.))
            .py(px(14.))
            .border_b_1()
            .border_color(t.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(mono(clone.name.clone(), 13., t.text).flex_1())
                    .child(
                        div().text_size(px(12.)).text_color(color).child(state),
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
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(10.))
            .py(px(5.))
            .rounded(px(6.))
            .bg(t.raised)
            .child(icon(glyph, 13., color))
            .child(mono(text, 12., color))
    };
    column(760., 24., compact)
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
                    .rounded(px(12.))
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
                .gap(px(12.))
                .p(px(14.))
                .rounded(px(12.))
                .border_1()
                .border_color(t.border_strong)
                .bg(t.panel)
                .child(
                    div()
                        .flex()
                        .items_start()
                        .h(px(if compact { 48. } else { 64. }))
                        .pt(px(2.))
                        .child(ws.first_task.clone()),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(8.))
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
                                .child(big_button("Start", None, true, t))
                                .on_click(cx.listener(|ws, _, _, cx| {
                                    ws.start_first_run_from_button(cx)
                                })),
                        ),
                ),
        )
        .into_any_element()
}
