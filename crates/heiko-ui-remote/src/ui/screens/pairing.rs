//! A phone pairing with the heiko on a computer (decision 0013): what to
//! do on the computer, the camera or a typed address, the phone's name
//! once paired, and what to check when the computer does not answer.
//!
//! These are phone screens; a desktop window (`--open pair`) shows them
//! in a column of a phone's width.

use gpui::{
    AnyElement,
    Context,
    Div,
    Hsla,
    SharedString,
    div,
    prelude::*,
    rems,
};

use crate::{
    assets::Icon,
    pairing::{Computer, PairStep, Progress},
    theme::{Design as _, Theme, Tone, Type, sp, weight},
    ui::components::{
        BlockSize,
        State,
        field,
        icon_button,
        key_values,
        label,
        link_action,
        logo,
        mark,
        mono,
        notice,
        phone_body,
        primary_block,
        section_header,
        stamp,
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
        .then(|| title_bar(ws, step.title(), t, cx));
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

/// A task screen's head: the way back and its title in the display
/// face, over a heavy rule.
fn title_bar(
    ws: &Workspace,
    title: &'static str,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    div()
        .flex_shrink_0()
        .mx(sp(4.))
        .pt(sp(2.))
        .pb(sp(1.5))
        .flex()
        .items_center()
        .gap(sp(1.))
        .border_b_2()
        .border_color(t.rule)
        .when(ws.can_go_back(), |bar| {
            bar.child(
                // Its chevron on the screen's margin, the title close
                // beside it, as a phone's other headers.
                icon_button("task-back", Icon::Back, 44., t)
                    .w(rems(1.5))
                    .justify_start()
                    .on_click(cx.listener(|ws, _, _, cx| ws.back(cx))),
            )
        })
        .child(
            div()
                .typeset(Type::SECTION.sized(19.))
                .text_color(t.bright)
                .child(title),
        )
}

/// A screen's title in the display face, and the paragraph under it.
fn heading_block(text: String, sub: &str, t: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(sp(2.5))
        .child(
            div()
                .typeset(Type::PAGE_TITLE)
                .text_color(t.bright)
                .child(text),
        )
        .child(
            div()
                .typeset(Type::LEAD.leading(1.55))
                .text_color(t.text_soft)
                .child(sub.to_owned()),
        )
}

/// The screen's main action, as wide as the phone.
fn primary(
    id: &'static str,
    label: &'static str,
    t: &Theme,
) -> gpui::Stateful<Div> {
    div().id(id).w_full().flex().flex_col().child(primary_block(
        label,
        BlockSize::Large,
        t,
    ))
}

/// Another action: an underlined word, easy to hit.
fn secondary(
    id: &'static str,
    label: &'static str,
    t: &Theme,
) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .flex()
        .justify_center()
        .py(sp(2.))
        .child(link_action(label, t))
}

/// A ruled row: what stands at its left, and its lines.
fn row(lead: impl IntoElement, lines: Div, t: &Theme) -> Div {
    div()
        .flex()
        .items_start()
        .gap(sp(3.))
        .py(sp(2.75))
        .border_b_1()
        .border_color(t.hairline)
        .child(div().pt(sp(1.)).flex_shrink_0().child(lead))
        .child(
            lines
                .flex_1()
                .min_w(rems(0.))
                .flex()
                .flex_col()
                .gap(sp(0.5)),
        )
}

/// Something under way: the working mark and what it is, in blue.
fn working(text: impl Into<SharedString>, t: &Theme) -> Div {
    div()
        .flex()
        .items_center()
        .gap(sp(2.5))
        .text_color(t.blue)
        .child(mark(State::Working, t))
        .child(div().flex_1().min_w(rems(0.)).child(text.into()))
}

/// Takes what room is left, so the actions sit at the bottom, under
/// the thumb.
fn spacer() -> Div {
    div().flex_1()
}

fn welcome(t: &Theme, cx: &mut Context<Workspace>) -> Div {
    let steps = [
        "Open heiko › Phones",
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
            "heiko, from your phone".into(),
            "Connect to the heiko running on your computer. Your runs work \
             there, with your repositories, tools and sign-ins. This \
             phone shows and steers them.",
            t,
        ))
        .child(
            div()
                .flex()
                .flex_col()
                .child(section_header("On your computer", None, t))
                .children(steps.into_iter().enumerate().map(|(n, step)| {
                    row(
                        mono((n + 1).to_string(), Type::CAPTION, t.dim),
                        div().child(step),
                        t,
                    )
                })),
        )
        .child(spacer())
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(2.5))
                .child(primary("pair-scan", "Scan pairing code", t).on_click(
                    cx.listener(|ws, _, _, cx| ws.scan_pairing_code(cx)),
                ))
                .child(
                    secondary("pair-type", "Enter the address instead", t)
                        .on_click(
                            cx.listener(|ws, _, _, cx| ws.type_address(cx)),
                        ),
                ),
        )
}

/// A step of connecting, as a ruled row: met, under way in blue, or not
/// reached yet.
fn check(state: State, text: &str, detail: Option<String>, t: &Theme) -> Div {
    let (ink, strong): (Hsla, bool) = match state {
        State::Met => (t.bright, true),
        State::Working => (t.blue, false),
        _ => (t.dim, false),
    };
    row(
        mark(state, t),
        div()
            .child(
                div()
                    .text_color(ink)
                    .when(strong, |text| text.font_weight(weight::STRONG))
                    .child(text.to_owned()),
            )
            .children(detail.map(|d| mono(d, Type::CAPTION, t.dim))),
        t,
    )
}

/// The camera's frame: a flat dark plane with four blue corner
/// brackets. heiko's viewfinder opens over this screen and reads the
/// code; this stands in for it once it closes.
fn viewfinder(t: &Theme) -> Div {
    let corner = |top: bool, left: bool| {
        let side = rems(0.1875);
        let mark = div().absolute().size(rems(2.25)).border_color(t.blue);
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
        .bg(t.panel)
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
                        .text_color(t.text_soft)
                        .text_center()
                        .child("Point the camera at the code on your computer"),
                ),
        )
}

fn scan(ws: &Workspace, t: &Theme, cx: &mut Context<Workspace>) -> Div {
    let progress = &ws.pairing.progress();
    let (found, certificate, pairing) = match progress {
        Progress::Connecting { address } => (
            (State::Working, Some(address.to_string())),
            (State::Idle, None),
            State::Idle,
        ),
        Progress::Compare {
            address,
            fingerprint,
        }
        | Progress::Pairing {
            address,
            fingerprint,
        } => (
            (State::Met, Some(address.to_string())),
            (State::Met, Some(fingerprint.short())),
            State::Working,
        ),
        _ => ((State::Idle, None), (State::Idle, None), State::Idle),
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
            div()
                .flex()
                .flex_col()
                .child(section_header("Pairing", None, t))
                .child(check(found.0, "Found heiko", found.1, t))
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
                .child(primary("pair-rescan", "Scan again", t).on_click(
                    cx.listener(|ws, _, _, cx| ws.scan_pairing_code(cx)),
                ))
                // Without the camera, as when it is not allowed.
                .child(
                    secondary(
                        "pair-rescan-type",
                        "Enter the address instead",
                        t,
                    )
                    .on_click(cx.listener(|ws, _, _, cx| ws.type_address(cx))),
                )
        })
        .child(
            div()
                .typeset(Type::SMALL.leading(1.55))
                .text_color(t.dim)
                .child(
                    "The code works once and expires in a few minutes. If the \
                 certificate doesn't match, heiko stops here and tells you.",
                ),
        )
}

fn address(ws: &Workspace, t: &Theme, cx: &mut Context<Workspace>) -> Div {
    let connect = || {
        primary("pair-connect", "Connect", t)
            .on_click(cx.listener(|ws, _, _, cx| ws.connect_typed(cx)))
    };
    let status = match &ws.pairing.progress() {
        Progress::Idle | Progress::Scanning => connect().into_any_element(),
        Progress::Connecting { address } => {
            working(format!("Connecting to {address}…"), t).into_any_element()
        }
        Progress::Compare { fingerprint, .. } => div()
            .flex()
            .flex_col()
            .gap(sp(3.))
            .child(section_header("Certificate", None, t))
            .child(
                div()
                    .text_color(t.bright)
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
                primary("pair-trust", "It matches", t).mt(sp(2.)).on_click(
                    cx.listener(|ws, _, _, cx| ws.trust_certificate(cx)),
                ),
            )
            .child(secondary("pair-distrust", "It doesn't", t).on_click(
                cx.listener(|ws, _, _, cx| ws.distrust_certificate(cx)),
            ))
            .into_any_element(),
        Progress::Pairing { .. } => working("Pairing…", t).into_any_element(),
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
        div()
            .flex()
            .flex_col()
            .gap(sp(3.))
            .typeset(Type::SMALL)
            .child(section_header("Computer", None, t))
            .child(key_values(rows, t))
    });
    div()
        .flex()
        .flex_col()
        .flex_1()
        .gap(sp(6.))
        .pt(sp(8.))
        .child(div().flex().child(stamp("paired", Tone::Good, t)))
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
            primary("pair-open", "Open heiko", t)
                .on_click(cx.listener(|ws, _, _, cx| ws.open_heiko(cx))),
        )
}

fn unreachable(ws: &Workspace, t: &Theme, cx: &mut Context<Workspace>) -> Div {
    let pairing = &ws.pairing;
    let computer = pairing.computer.as_ref();
    let checks = [
        (
            "Is heiko open on your computer?",
            "Phones reach heiko only while the app runs.",
        ),
        ("Is Allow phones on?", "heiko › Phones on your computer."),
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
        .child(div().flex().child(stamp("no answer", Tone::Danger, t)))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(2.))
                .child(
                    div().typeset(Type::PAGE_TITLE).text_color(t.bright).child(
                        format!("Can't reach {}", computer_name(computer)),
                    ),
                )
                .children(computer.map(|c| {
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(sp(1.))
                        .typeset(Type::SMALL)
                        .text_color(t.muted)
                        .child(mono(
                            c.address.to_string(),
                            Type::SMALL,
                            t.text_soft,
                        ))
                        .child(format!("didn't answer after {tries}."))
                })),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .child(section_header("Check", None, t))
                .children(checks.into_iter().enumerate().map(
                    |(n, (ask, hint))| {
                        row(
                            mono((n + 1).to_string(), Type::CAPTION, t.dim),
                            div()
                                .child(div().text_color(t.bright).child(ask))
                                .child(
                                    div()
                                        .typeset(Type::CAPTION)
                                        .text_color(t.muted)
                                        .child(hint),
                                ),
                            t,
                        )
                    },
                )),
        )
        .child(
            div()
                .typeset(Type::SMALL.leading(1.55))
                .text_color(t.dim)
                .child(
                    "If heiko says this phone was revoked, pair it again \
                     with a new code.",
                ),
        )
        .child(spacer())
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(2.5))
                .child(if pairing.busy() {
                    working(
                        format!("Connecting to {}…", computer_name(computer)),
                        t,
                    )
                    .into_any_element()
                } else {
                    primary("pair-retry", "Try again", t)
                        .on_click(
                            cx.listener(|ws, _, _, cx| ws.retry_connection(cx)),
                        )
                        .into_any_element()
                })
                .child(secondary("pair-again", "Pair again", t).on_click(
                    cx.listener(|ws, _, _, cx| ws.scan_pairing_code(cx)),
                )),
        )
}
