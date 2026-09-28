//! Model settings: each agent's default, which models the picker shows,
//! how tau reaches them, and the price to ask about.

use gpui::{AnyElement, Context, SharedString, div, prelude::*, px};

use crate::{
    assets::Icon,
    models::{AccessKind, ModelOption},
    theme::{Design as _, IconSize, Theme, Type, radius, sp},
    ui::{self, ButtonKind, heading, mono},
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

    let mut options: Vec<&ModelOption> = models.options.iter().collect();
    options.sort_by_key(|option| option.tier());
    let columns = |row: gpui::Div| {
        row.grid()
            .grid_cols(if compact { 3 } else { 5 })
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
        .when(!compact, |row| {
            row.child("CONTEXT").child("$ IN / OUT PER M")
        })
        .child("AVAILABLE WITH")
        .child(div().flex().justify_end().child("SHOW"));
    let rows = options
        .into_iter()
        .map(|option| {
            let hidden = models.settings.is_hidden(&option.id);
            let id = option.id.clone();
            columns(div())
                .min_h(px(40.))
                .border_b_1()
                .border_color(t.border)
                .child(mono(
                    option.id.clone(),
                    Type::CAPTION,
                    if option.available { t.text } else { t.dim },
                ))
                .when(!compact, |row| {
                    row.child(mono(
                        option.context_label(),
                        Type::CAPTION,
                        t.muted,
                    ))
                    .child(mono(
                        option.price(),
                        Type::CAPTION,
                        t.muted,
                    ))
                })
                .child(ui::text(
                    if option.available {
                        available_with(ws)
                    } else {
                        "API key only"
                    },
                    Type::CAPTION,
                    if option.available { t.green } else { t.dim },
                ))
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
    let saved = |kind| access.saved.contains(&kind);
    let chatgpt = account_row(
        "account-chatgpt",
        Icon::Chat,
        "ChatGPT",
        if access.chatgpt {
            "Signed in · runs use it, and count against your plan"
        } else if saved(AccessKind::ChatGpt) {
            "Signed in · not in use"
        } else {
            "Not signed in"
        },
        access.chatgpt,
        if saved(AccessKind::ChatGpt) {
            ("Sign out", Some(AccessKind::ChatGpt))
        } else {
            ("Sign in", None)
        },
        t,
        cx,
    );
    let key = account_row(
        "account-api-key",
        Icon::Key,
        "OpenAI API key",
        if access.api_key {
            "Saved · runs use it, billed per token"
        } else if saved(AccessKind::ApiKey) {
            "Saved · used once you sign out of ChatGPT"
        } else {
            "None · every model, billed per token on your account"
        },
        access.api_key,
        if saved(AccessKind::ApiKey) {
            ("Remove", Some(AccessKind::ApiKey))
        } else {
            ("Add a key", None)
        },
        t,
        cx,
    );
    let access_card = ui::panel(4.5, t)
        .gap(sp(3.))
        .child(heading("Accounts", t))
        .child(chatgpt)
        .child(key);

    let limit = match models.settings.ask_above {
        Some(limit) => format!("${limit:.0} / M out"),
        None => "never".to_owned(),
    };
    let step = |id: &'static str, glyph: Icon, delta: i32| {
        div()
            .id(id)
            .size(px(26.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(radius::CONTROL)
            .border_1()
            .border_color(t.border)
            .cursor_pointer()
            .hover(|style| style.bg(gpui::white().opacity(0.04)))
            .child(ui::icon(glyph, IconSize::SMALL, t.text_soft))
            .on_click(
                cx.listener(move |ws, _, _, cx| ws.step_ask_above(delta, cx)),
            )
    };
    let spending = ui::panel(4.5, t)
        .gap(sp(3.))
        .child(heading("Spending", t))
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(2.))
                .child(div().flex_1().child("Ask before a model above"))
                .child(step("ask-lower", Icon::Back, -1))
                .child(
                    mono(limit, Type::CAPTION, t.text)
                        .px(sp(2.5))
                        .py(sp(1.25))
                        .rounded(radius::CONTROL)
                        .bg(t.raised),
                )
                .child(step("ask-higher", Icon::Chevron, 1)),
        )
        .child(ui::text(
            "Picking a pricier model shows its price and asks once. Each run \
             still stops at its own cost limit.",
            Type::CAPTION,
            t.muted,
        ));

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
        .child(access_card)
        .child(spending);
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

/// How the available models are reached, in words.
fn available_with(ws: &Workspace) -> &'static str {
    let access = &ws.catalog.models.access;
    match (access.chatgpt, access.api_key) {
        (true, true) => "ChatGPT or API key",
        (true, false) => "ChatGPT",
        _ => "API key",
    }
}

/// An account: what it is, whether runs use it, and signing in or out.
/// `action` signs out of `Some` kind, or connects one when `None`.
#[allow(clippy::too_many_arguments)]
fn account_row(
    id: &'static str,
    glyph: Icon,
    name: &'static str,
    status: &'static str,
    in_use: bool,
    action: (&'static str, Option<AccessKind>),
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> gpui::Div {
    let (label, kind) = action;
    div()
        .flex()
        .items_center()
        .gap(sp(2.5))
        .child(ui::icon(
            glyph,
            IconSize::LARGE,
            if in_use { t.green } else { t.dim },
        ))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(sp(0.5))
                .child(name)
                .child(ui::text(status, Type::CAPTION, t.muted)),
        )
        .child(
            div()
                .id(id)
                .child(ui::button(label, ButtonKind::Secondary, t))
                .on_click(cx.listener(move |ws, _, _, cx| match kind {
                    Some(kind) => ws.sign_out(kind, cx),
                    None => ws.connect_model(cx),
                })),
        )
}
