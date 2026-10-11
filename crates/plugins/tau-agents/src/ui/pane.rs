//! tau-agents' settings pane (ADR 0029): an agent's model and effort,
//! the most it may spend a day, and how long its events gather. Each
//! agent keeps them as its repository's copy, in tau's data, where the
//! agent cannot change them.

use gpui::{
    AnyElement,
    Div,
    InteractiveElement,
    IntoElement,
    ParentElement,
    SharedString,
    StatefulInteractiveElement,
    Styled,
    div,
    prelude::FluentBuilder,
    rems,
};
use tau_ui_kit::{
    components::{self as ui, Material as _},
    theme::{Design as _, Type, radius, sp},
};
use tau_ui_plugin::ViewCx;

use super::AgentsUi;
use crate::Settings;

/// The efforts an agent can run at.
const EFFORTS: [&str; 3] = ["low", "medium", "high"];

/// What it may spend a day, as the pane offers it.
const CAPS: [f64; 5] = [0.5, 1.0, 2.0, 5.0, 10.0];

/// How long its events gather, in seconds, as the pane offers it.
const WINDOWS: [(u64, &str); 4] =
    [(10, "10 s"), (30, "30 s"), (60, "1 min"), (300, "5 min")];

pub(super) fn render(view: &mut ViewCx<'_, AgentsUi>) -> AnyElement {
    let t = view.theme().clone();
    let settings = view.settings.clone();
    let scope = view.scope().map(str::to_owned);
    let segment = |id: String, label: String, on: bool, next: Settings| {
        let handle = view.handle.clone();
        let scope = scope.clone();
        div()
            .id(SharedString::from(id))
            .h(rems(1.75))
            .px(sp(2.5))
            .flex()
            .items_center()
            .justify_center()
            .rounded(radius::CONTROL)
            .typeset(Type::CAPTION)
            .cursor_pointer()
            .text_color(if on { t.text } else { t.muted })
            .when(on, |segment| segment.key(&t))
            .child(label)
            .on_click(move |_, _, cx| {
                handle.save_settings_in(scope.as_deref(), &next, cx)
            })
    };
    let row = |name: &'static str, caption: &'static str, choices: Div| {
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
                    .flex_col()
                    .gap(sp(0.75))
                    .child(name)
                    .child(ui::text(caption, Type::CAPTION, t.muted)),
            )
            .child(choices)
    };
    let group = |children: Vec<gpui::Stateful<Div>>| {
        div()
            .flex()
            .flex_wrap()
            .gap(sp(0.5))
            .p(sp(0.75))
            .rounded(radius::CONTROL)
            .well(&t)
            .children(children)
    };
    let model = settings.model();
    let models = tau_ai::model::plan_models()
        .into_iter()
        .map(|plan| {
            let id = plan.id.to_owned();
            segment(
                format!("agent-model-{id}"),
                id.clone(),
                model == id,
                Settings {
                    model: Some(id),
                    ..settings.clone()
                },
            )
        })
        .collect();
    let efforts = EFFORTS
        .into_iter()
        .map(|effort| {
            segment(
                format!("agent-effort-{effort}"),
                effort.to_owned(),
                settings.effort() == effort,
                Settings {
                    effort: Some(effort.to_owned()),
                    ..settings.clone()
                },
            )
        })
        .collect();
    let caps = CAPS
        .into_iter()
        .map(|cap| {
            segment(
                format!("agent-cap-{cap}"),
                format!("${cap:.2}"),
                (settings.daily_cap - cap).abs() < f64::EPSILON,
                Settings {
                    daily_cap: cap,
                    ..settings.clone()
                },
            )
        })
        .collect();
    let windows = WINDOWS
        .into_iter()
        .map(|(seconds, label)| {
            segment(
                format!("agent-window-{seconds}"),
                label.to_owned(),
                settings.window_secs == seconds,
                Settings {
                    window_secs: seconds,
                    ..settings.clone()
                },
            )
        })
        .collect();
    ui::card(&t)
        .child(row(
            "Model",
            "What its turns run on. A small model keeps it cheap.",
            group(models),
        ))
        .child(row("Effort", "How hard each turn thinks.", group(efforts)))
        .child(row(
            "Most it may spend a day",
            "At the cap it stops until tomorrow, or until you allow more.",
            group(caps),
        ))
        .child(row(
            "Gather events for",
            "An event waits this long for others, so a burst is one turn.",
            group(windows),
        ))
        .into_any_element()
}
