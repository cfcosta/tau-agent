//! A phone pairing with the tau on a computer (decision 0013): what to
//! do on the computer, the camera or a typed address, the phone's name
//! once paired, and what to check when the computer does not answer.
//!
//! These are phone screens; a desktop window (`--open pair`) shows them
//! in a column of a phone's width.

use gpui::{AnyElement, Context, Div, div, prelude::*, rems};

use crate::{
    assets::Icon,
    pairing::{Computer, PairStep, Progress},
    theme::{Design as _, IconSize, Theme, Type, radius, sp, weight},
    ui::{
        components::{
            ButtonKind,
            big_button,
            field,
            heading,
            key_values,
            label,
            lead,
            logo,
            mono,
            notice,
            panel,
            phone_bar,
            phone_body,
            tile,
            title,
        },
        icon,
    },
    workspace::Workspace,
};

pub fn render(
    ws: &Workspace,
    step: PairStep,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let body = match step {
        PairStep::Welcome => welcome(t, cx),
        PairStep::Scan => scan(ws, t, cx),
        PairStep::Address => address(ws, t, cx),
        PairStep::Paired => paired(ws, t, cx),
        PairStep::Unreachable => unreachable(ws, t, cx),
    };
    // The task screens have a way back; the others are where the phone
    // starts, or stands until it reaches the computer.
    let bar = matches!(step, PairStep::Scan | PairStep::Address)
        .then(|| phone_bar(ws, step.title(), t, cx));
    div()
        .size_full()
        .flex()
        .flex_col()
        .bg(t.bg)
        .children(bar)
        .child(
            phone_body("pair-body")
                .typeset(Type::PHONE)
                .when(!compact, |body| body.items_center())
                .child(
                    div()
                        .w_full()
                        .flex_1()
                        .when(!compact, |col| col.max_w(rems(26.25)).py(sp(8.)))
                        .flex()
                        .flex_col()
                        .gap(sp(6.))
                        .child(body),
                ),
        )
        .into_any_element()
}

fn heading_block(text: String, sub: &str, t: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(sp(2.5))
        .child(title(text, Type::HEADLINE))
        .child(lead(sub, Type::LEAD, t))
}

fn full(id: &'static str, button: Div) -> gpui::Stateful<Div> {
    div().id(id).w_full().flex().flex_col().child(button)
}

/// Takes what room is left, so the actions sit at the bottom, under
/// the thumb.
fn spacer() -> Div {
    div().flex_1()
}

fn welcome(t: &Theme, cx: &mut Context<Workspace>) -> Div {
    let steps = [
        "Open tau › Phones",
        "Turn on Allow phones",
        "Show the pairing code",
    ];
    div()
        .flex()
        .flex_col()
        .flex_1()
        .gap(sp(7.))
        .pt(sp(8.))
        .child(logo(t, 56.))
        .child(heading_block(
            "tau, from your phone".into(),
            "Connect to the tau running on your computer. Your runs work \
             there, with your repositories, tools and ChatGPT sign-in. This \
             phone shows and steers them.",
            t,
        ))
        .child(
            panel(3.5, t)
                .gap(sp(2.5))
                .child(heading("On your computer", t))
                .children(steps.into_iter().enumerate().map(|(n, step)| {
                    div()
                        .flex()
                        .gap(sp(2.5))
                        .child(mono((n + 1).to_string(), Type::BODY, t.accent))
                        .child(step)
                })),
        )
        .child(spacer())
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(2.5))
                .child(
                    full(
                        "pair-scan",
                        big_button(
                            "Scan pairing code",
                            Some(Icon::Camera),
                            ButtonKind::Primary,
                            t,
                        ),
                    )
                    .on_click(
                        cx.listener(|ws, _, _, cx| ws.scan_pairing_code(cx)),
                    ),
                )
                .child(
                    full(
                        "pair-type",
                        big_button(
                            "Enter the address instead",
                            None,
                            ButtonKind::Secondary,
                            t,
                        ),
                    )
                    .on_click(cx.listener(|ws, _, _, cx| ws.type_address(cx))),
                ),
        )
}

/// Where a step of connecting stands.
#[derive(Clone, Copy, PartialEq)]
enum Mark {
    Done,
    Now,
    Later,
}

fn check(mark: Mark, text: &str, detail: Option<String>, t: &Theme) -> Div {
    let glyph = match mark {
        Mark::Done => icon(Icon::Check, IconSize::MEDIUM, t.green),
        Mark::Now => icon(Icon::Spinner, IconSize::MEDIUM, t.accent),
        Mark::Later => icon(Icon::Spinner, IconSize::MEDIUM, t.border_strong),
    };
    div().flex().items_start().gap(sp(2.5)).child(glyph).child(
        div()
            .flex_1()
            .min_w(rems(0.))
            .flex()
            .flex_col()
            .gap(sp(0.5))
            .text_color(match mark {
                Mark::Now => t.accent,
                Mark::Done => t.text,
                Mark::Later => t.dim,
            })
            .child(text.to_owned())
            .children(detail.map(|d| mono(d, Type::CAPTION, t.dim))),
    )
}

/// The camera's frame. tau's viewfinder opens over this screen and
/// reads the code; this stands in for it once it closes.
fn viewfinder(t: &Theme) -> Div {
    let corner = |top: bool, left: bool| {
        let side = rems(0.1875);
        let mark = div().absolute().size(rems(2.25)).border_color(t.accent);
        let mark = if top {
            mark.top_0().border_t(side)
        } else {
            mark.bottom_0().border_b(side)
        };
        if left {
            mark.left_0().border_l(side)
        } else {
            mark.right_0().border_r(side)
        }
    };
    div()
        .h(rems(20.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius::SHEET)
        .bg(t.backdrop)
        .border_1()
        .border_color(t.border)
        .child(
            div()
                .relative()
                .size(rems(13.75))
                .flex()
                .items_center()
                .justify_center()
                .p(sp(8.))
                .child(corner(true, true))
                .child(corner(true, false))
                .child(corner(false, true))
                .child(corner(false, false))
                .child(
                    div()
                        .typeset(Type::SMALL)
                        .text_color(t.muted)
                        .text_center()
                        .child("Point the camera at the code on your computer"),
                ),
        )
}

fn scan(ws: &Workspace, t: &Theme, cx: &mut Context<Workspace>) -> Div {
    let progress = &ws.pairing.progress;
    let (found, certificate, pairing) = match progress {
        Progress::Connecting { address } => (
            (Mark::Now, Some(address.to_string())),
            (Mark::Later, None),
            Mark::Later,
        ),
        Progress::Compare {
            address,
            fingerprint,
        }
        | Progress::Pairing {
            address,
            fingerprint,
        } => (
            (Mark::Done, Some(address.to_string())),
            (Mark::Done, Some(fingerprint.short())),
            Mark::Now,
        ),
        _ => ((Mark::Later, None), (Mark::Later, None), Mark::Later),
    };
    let failed = match progress {
        Progress::Failed(error) => Some(error.clone()),
        _ => None,
    };
    div()
        .flex()
        .flex_col()
        .gap(sp(4.5))
        .child(viewfinder(t))
        .child(
            panel(4., t)
                .gap(sp(3.))
                .child(check(found.0, "Found tau", found.1, t))
                .child(check(
                    certificate.0,
                    "Certificate matches the code",
                    certificate.1,
                    t,
                ))
                .child(check(pairing, "Pairing…", None, t)),
        )
        .when_some(failed, |col, error| {
            col.child(notice(Icon::Warning, error, t.red, Type::SMALL, t))
                .child(
                    full(
                        "pair-rescan",
                        big_button(
                            "Scan again",
                            Some(Icon::Camera),
                            ButtonKind::Primary,
                            t,
                        ),
                    )
                    .on_click(
                        cx.listener(|ws, _, _, cx| ws.scan_pairing_code(cx)),
                    ),
                )
                // Without the camera, as when it is not allowed.
                .child(
                    full(
                        "pair-rescan-type",
                        big_button(
                            "Enter the address instead",
                            None,
                            ButtonKind::Secondary,
                            t,
                        ),
                    )
                    .on_click(cx.listener(|ws, _, _, cx| ws.type_address(cx))),
                )
        })
        .child(
            div()
                .typeset(Type::CAPTION)
                .text_color(t.dim)
                .leading(1.5)
                .child(
                    "The code works once and expires in a few minutes. If the \
                 certificate doesn't match, tau stops here and tells you.",
                ),
        )
}

fn address(ws: &Workspace, t: &Theme, cx: &mut Context<Workspace>) -> Div {
    let connect = || {
        full(
            "pair-connect",
            big_button("Connect", None, ButtonKind::Primary, t),
        )
        .on_click(cx.listener(|ws, _, _, cx| ws.connect_typed(cx)))
    };
    let status = match &ws.pairing.progress {
        Progress::Idle | Progress::Scanning => connect().into_any_element(),
        Progress::Connecting { address } => notice(
            Icon::Spinner,
            format!("Connecting to {address}…"),
            t.accent,
            Type::SMALL,
            t,
        )
        .into_any_element(),
        Progress::Compare { fingerprint, .. } => panel(4., t)
            .gap(sp(3.))
            .child(
                div()
                    .font_weight(weight::STRONG)
                    .child("Does your computer show this certificate?"),
            )
            .child(
                div()
                    .typeset(Type::SMALL)
                    .text_color(t.muted)
                    .child("Compare it with Certificate under Pair a phone."),
            )
            .child(
                // Two columns of four, as the computer shows it.
                div().flex().flex_wrap().gap_y(sp(1.)).children(
                    fingerprint
                        .lines()
                        .into_iter()
                        .map(|line| mono(line, Type::CODE, t.text).w_1_2()),
                ),
            )
            .child(
                div()
                    .flex()
                    .gap(sp(2.5))
                    .child(
                        div()
                            .id("pair-trust")
                            .flex_1()
                            .flex()
                            .flex_col()
                            .child(big_button(
                                "It matches",
                                Some(Icon::Check),
                                ButtonKind::Primary,
                                t,
                            ))
                            .on_click(cx.listener(|ws, _, _, cx| {
                                ws.trust_certificate(cx)
                            })),
                    )
                    .child(
                        div()
                            .id("pair-distrust")
                            .flex_1()
                            .flex()
                            .flex_col()
                            .child(big_button(
                                "It doesn't",
                                None,
                                ButtonKind::Danger,
                                t,
                            ))
                            .on_click(cx.listener(|ws, _, _, cx| {
                                ws.distrust_certificate(cx)
                            })),
                    ),
            )
            .into_any_element(),
        Progress::Pairing { .. } => {
            notice(Icon::Spinner, "Pairing…", t.accent, Type::SMALL, t)
                .into_any_element()
        }
        Progress::Failed(error) => div()
            .flex()
            .flex_col()
            .gap(sp(3.))
            .child(notice(Icon::Warning, error.clone(), t.red, Type::SMALL, t))
            .child(connect())
            .into_any_element(),
    };
    div()
        .flex()
        .flex_col()
        .gap(sp(5.))
        .child(heading_block(
            "Pair by address".into(),
            "Type the address and the code your computer shows under Pair a \
             phone. With no QR code to check it against, you compare the \
             certificate yourself.",
            t,
        ))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(2.))
                .child(label("Address", t))
                .child(field(&ws.pair_address, true, t)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(2.))
                .child(label("Pairing code", t))
                .child(field(&ws.pair_code, true, t)),
        )
        .child(status)
}

fn computer_name(computer: Option<&Computer>) -> String {
    computer.map_or_else(|| "your computer".into(), |c| c.name.clone())
}

fn paired(ws: &Workspace, t: &Theme, cx: &mut Context<Workspace>) -> Div {
    let computer = ws.pairing.computer.as_ref();
    let details = computer.map(|c| {
        let mut rows = vec![
            ("Computer".into(), div().child(c.name.clone())),
            (
                "Address".into(),
                mono(c.address.to_string(), Type::CAPTION, t.text),
            ),
        ];
        if let Some(via) = &c.via {
            rows.push(("Reaches it".into(), div().child(via.clone())));
        }
        rows.push((
            "Certificate".into(),
            mono(c.fingerprint.short(), Type::CAPTION, t.text),
        ));
        panel(4., t).typeset(Type::SMALL).child(key_values(rows, t))
    });
    div()
        .flex()
        .flex_col()
        .flex_1()
        .gap(sp(6.))
        .pt(sp(8.))
        .child(tile(Icon::Check, t.green, t.green_soft, t.green))
        .child(heading_block(
            format!("Paired with {}", computer_name(computer)),
            "This phone now has its own key, kept in its keystore. You can \
             revoke it from your computer at any time.",
            t,
        ))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(2.))
                .child(label("This phone's name, as your computer lists it", t))
                .child(field(&ws.phone_name, false, t)),
        )
        .children(details)
        .child(spacer())
        .child(
            full(
                "pair-open",
                big_button("Open tau", None, ButtonKind::Primary, t),
            )
            .on_click(cx.listener(|ws, _, _, cx| ws.open_tau(cx))),
        )
}

fn unreachable(ws: &Workspace, t: &Theme, cx: &mut Context<Workspace>) -> Div {
    let pairing = &ws.pairing;
    let computer = pairing.computer.as_ref();
    let checks = [
        (
            "Is tau open on your computer?",
            "Phones reach tau only while the app runs.",
        ),
        ("Is Allow phones on?", "tau › Phones on your computer."),
        ("Away from home?", "Connect your VPN on this phone."),
    ];
    let tries = match pairing.tries {
        1 => "1 try".to_owned(),
        n => format!("{n} tries"),
    };
    div()
        .flex()
        .flex_col()
        .flex_1()
        .gap(sp(5.))
        .pt(sp(8.))
        .child(tile(Icon::Offline, t.red, t.red_soft, t.red_border))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(2.))
                .child(title(
                    format!("Can't reach {}", computer_name(computer)),
                    Type::HEADLINE,
                ))
                .children(computer.map(|c| {
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(sp(1.))
                        .typeset(Type::SMALL)
                        .text_color(t.muted)
                        .child(mono(c.address.to_string(), Type::SMALL, t.text_soft))
                        .child(format!("didn't answer after {tries}."))
                })),
        )
        .child(
            panel(0., t).overflow_hidden().children(
                checks.into_iter().enumerate().map(|(n, (ask, hint))| {
                    div()
                        .flex()
                        .flex_col()
                        .gap(sp(0.5))
                        .px(sp(3.5))
                        .py(sp(3.))
                        .when(n > 0, |row| row.border_t_1().border_color(t.border))
                        .child(ask)
                        .child(
                            div()
                                .typeset(Type::CAPTION)
                                .text_color(t.muted)
                                .child(hint),
                        )
                }),
            ),
        )
        .child(notice(
            Icon::Key,
            "If tau says this phone was revoked, pair it again with a new code.",
            t.red,
            Type::SMALL,
            t,
        ))
        .child(spacer())
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(2.5))
                .child(if pairing.busy() {
                    notice(
                        Icon::Spinner,
                        format!("Connecting to {}…", computer_name(computer)),
                        t.accent,
                        Type::SMALL,
                        t,
                    )
                    .into_any_element()
                } else {
                    full(
                        "pair-retry",
                        big_button("Try again", None, ButtonKind::Primary, t),
                    )
                    .on_click(
                        cx.listener(|ws, _, _, cx| ws.retry_connection(cx)),
                    )
                    .into_any_element()
                })
                .child(
                    full(
                        "pair-again",
                        big_button(
                            "Pair again",
                            Some(Icon::Camera),
                            ButtonKind::Secondary,
                            t,
                        ),
                    )
                    .on_click(
                        cx.listener(|ws, _, _, cx| ws.scan_pairing_code(cx)),
                    ),
                ),
        )
}
