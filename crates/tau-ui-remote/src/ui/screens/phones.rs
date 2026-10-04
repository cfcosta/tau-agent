//! Phones, on the computer (decision 0013): "Allow phones", the address
//! tau listens on, the pairing code, and the phones paired.

use std::time::Instant;

use gpui::{AnyElement, Context, Div, SharedString, div, prelude::*, px};

use crate::{
    assets::Icon,
    phones::{Phones, PhonesRequest, ShownCode},
    theme::{Design as _, IconSize, Theme, Type, radius, sp, weight},
    ui::{
        components::{
            ButtonKind,
            big_button,
            button,
            chip,
            heading,
            key_values,
            mono,
            notice,
            panel,
            qr_code,
            screen,
            screen_title,
            switch,
            text,
        },
        icon,
    },
    workspace::Workspace,
};

pub fn render(
    ws: &Workspace,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let phones = &ws.phones;
    let left =
        div()
            .flex_1()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .gap(sp(4.))
            .child(allow(phones, t, cx))
            .when(phones.allowed, |col| col.child(listen_on(phones, t, cx)))
            .children(phones.error.clone().map(|error| {
                notice(Icon::Warning, error, t.red, Type::SMALL, t)
            }))
            .child(paired(phones, t, cx));
    let content = div()
        .flex()
        .flex_col()
        .gap(sp(5.))
        .child(screen_title(
            "Phones",
            "Reach tau from your phone while this app runs: on your \
             network, or through your VPN when you are away.",
            t,
        ))
        .child(
            div()
                .flex()
                .when(compact, |row| row.flex_col())
                .items_start()
                .gap(sp(6.))
                .child(left)
                .when(phones.allowed, |row| {
                    row.child(pair_card(phones, compact, t, cx))
                }),
        );
    screen("phones", compact, content).into_any_element()
}

fn allow(phones: &Phones, t: &Theme, cx: &mut Context<Workspace>) -> Div {
    let allowed = phones.allowed;
    panel(4., t).child(
        div()
            .flex()
            .items_center()
            .gap(sp(3.))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .gap(sp(0.5))
                    .child(
                        div()
                            .font_weight(weight::EMPHASIS)
                            .child("Allow phones"),
                    )
                    .child(text(
                        "Off by default. Phones reach tau only while this app \
                         is open.",
                        Type::SMALL,
                        t.muted,
                    )),
            )
            .child(
                div().id("allow-phones").child(switch(allowed, t)).on_click(
                    cx.listener(move |ws, _, _, cx| {
                        ws.ask_phones(PhonesRequest::Allow(!allowed), cx)
                    }),
                ),
            ),
    )
}

fn listen_on(phones: &Phones, t: &Theme, cx: &mut Context<Workspace>) -> Div {
    let chosen = phones
        .listening
        .as_ref()
        .map(|address| address.host.clone());
    div()
        .flex()
        .flex_col()
        .gap(sp(2.))
        .child(heading("Listen on", t))
        .child(div().flex().flex_wrap().gap(sp(2.)).children(
            phones.addresses.iter().map(|address| {
                let ip = address.ip.clone();
                let on = chosen.as_ref() == Some(&ip);
                div()
                    .id(SharedString::from(format!("listen-{ip}")))
                    .child(chip(
                        on.then_some(Icon::Check),
                        format!("{} · {}", address.label, address.ip),
                        Type::SMALL,
                        if on { t.accent } else { t.muted },
                        t,
                    ))
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.ask_phones(PhonesRequest::ListenOn(ip.clone()), cx)
                    }))
            }),
        ))
        .child(match &phones.listening {
            Some(address) => {
                text(format!("Listening on {address}."), Type::CAPTION, t.muted)
            }
            None => text("Not listening.", Type::CAPTION, t.muted),
        })
}

fn when(stamp: &str) -> String {
    // `2026-09-30T12:03:11Z` as `2026-09-30 12:03`.
    stamp
        .get(..16)
        .map_or_else(|| stamp.to_owned(), |minute| minute.replace('T', " "))
}

fn paired(phones: &Phones, t: &Theme, cx: &mut Context<Workspace>) -> Div {
    let rows = phones.paired.iter().map(|device| {
        let id = device.id.clone();
        let seen = device.last_seen.as_deref().map_or_else(
            || "never connected".to_owned(),
            |seen| format!("last seen {}", when(seen)),
        );
        div()
            .flex()
            .items_center()
            .gap(sp(3.))
            .px(sp(4.))
            .py(sp(3.))
            .border_t_1()
            .border_color(t.border)
            .child(icon(Icon::Phone, IconSize::LARGE, t.muted))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .gap(sp(0.5))
                    .child(device.name.clone())
                    .child(text(
                        format!("paired {} · {seen}", when(&device.paired_at)),
                        Type::CAPTION,
                        t.muted,
                    )),
            )
            .child(
                div()
                    .id(SharedString::from(format!("revoke-{id}")))
                    .child(button("Revoke", ButtonKind::Danger, t))
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.ask_phones(PhonesRequest::Revoke(id.clone()), cx)
                    })),
            )
    });
    let empty = phones.paired.is_empty();
    panel(0., t)
        .overflow_hidden()
        .child(
            div()
                .px(sp(4.))
                .py(sp(3.))
                .font_weight(weight::STRONG)
                .child("Paired phones"),
        )
        .children(rows)
        .when(empty, |list| {
            list.child(
                div()
                    .px(sp(4.))
                    .py(sp(3.))
                    .border_t_1()
                    .border_color(t.border)
                    .child(text("No phones yet.", Type::SMALL, t.muted)),
            )
        })
        .child(
            div()
                .px(sp(4.))
                .py(sp(3.))
                .border_t_1()
                .border_color(t.border)
                .child(text(
                    "Revoking a phone refuses its key at once and closes its \
                     connection.",
                    Type::CAPTION,
                    t.dim,
                )),
        )
}

fn countdown(code: &ShownCode) -> String {
    let left = code.expires.saturating_duration_since(Instant::now());
    let seconds = left.as_secs();
    format!("in {}:{:02}", seconds / 60, seconds % 60)
}

fn pair_card(
    phones: &Phones,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let card = panel(4.5, t)
        .when(!compact, |card| card.w(px(330.)).flex_shrink_0())
        .when(compact, |card| card.w_full())
        .items_center()
        .gap(sp(3.5))
        .child(
            div()
                .self_start()
                .typeset(Type::SUBTITLE)
                .font_weight(weight::STRONG)
                .child("Pair a phone"),
        );
    let Some(shown) = &phones.code else {
        let listening = phones.listening.is_some();
        return card
            .child(text(
                "Show a code, then scan it from tau on your phone. It works \
                 once, for a few minutes.",
                Type::SMALL,
                t.muted,
            ))
            .when(listening, |card| {
                card.child(
                    div()
                        .id("show-code")
                        .w_full()
                        .flex()
                        .flex_col()
                        .child(big_button(
                            "Show pairing code",
                            None,
                            ButtonKind::Primary,
                            t,
                        ))
                        .on_click(cx.listener(|ws, _, _, cx| {
                            ws.ask_phones(PhonesRequest::ShowCode, cx)
                        })),
                )
            });
    };
    let code = &shown.code;
    card.children(qr_code(&code.to_string(), 200.))
        .child(
            div()
                .typeset(Type::CAPTION)
                .text_color(t.muted)
                .text_center()
                .child("In tau on your phone, tap Scan pairing code."),
        )
        .child(div().w_full().typeset(Type::CAPTION).child(key_values(
            [
                (
                    "Address".into(),
                    mono(code.address.to_string(), Type::CAPTION, t.text),
                ),
                (
                    "Code".into(),
                    mono(code.secret.to_string(), Type::CAPTION, t.text),
                ),
                (
                    "Certificate".into(),
                    mono(code.fingerprint.short(), Type::CAPTION, t.text),
                ),
                (
                    "Expires".into(),
                    mono(countdown(shown), Type::CAPTION, t.roles.live),
                ),
            ],
            t,
        )))
        .child(
            // Typing the address instead: the phone shows the whole
            // certificate, to compare with this.
            div()
                .w_full()
                .flex()
                .flex_wrap()
                .gap_x(sp(3.))
                .rounded(radius::BOX)
                .children(
                    code.fingerprint
                        .lines()
                        .into_iter()
                        .map(|line| mono(line, Type::MICRO, t.dim)),
                ),
        )
        .child(
            div()
                .id("hide-code")
                .w_full()
                .flex()
                .flex_col()
                .child(big_button("Hide code", None, ButtonKind::Secondary, t))
                .on_click(cx.listener(|ws, _, _, cx| {
                    ws.ask_phones(PhonesRequest::HideCode, cx)
                })),
        )
}
