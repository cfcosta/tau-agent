//! tau-schedule's interface half: what a chat has waiting, in its
//! inspector, with a way to cancel each, and the message it wakes on,
//! drawn as a card.

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
    assets::Icon,
    components::{icon, mono},
    theme::{Design as _, IconSize, Theme, Type, radius, sp, weight},
};
use tau_ui_plugin::{
    Manifest,
    UiPlugin,
    ViewCx,
    points::{self, AtMessage, AtRun},
};

use crate::{Act, NAME, Overview, Wake, Woken};

/// tau-schedule's interface half.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScheduleUi;

impl UiPlugin for ScheduleUi {
    type State = ();
    type Data = Overview;
    type RepoData = ();
    type Settings = ();
    type Ui = ();

    fn name(&self) -> &'static str {
        NAME
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
            .contribute_at(points::INSPECTOR, -150, panel)
            .contribute(points::USER_MESSAGE, card)
    }
}

/// Now, in seconds since the epoch.
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

/// `at` said from `now`: `in 6m`, `in 2h`, `now`; the interface needs no
/// time zone for it.
pub fn relative(at: i64, now: i64) -> String {
    let gap = at - now;
    match gap {
        ..60 => "now".to_owned(),
        60..3600 => format!("in {}m", gap / 60),
        3600..86_400 => format!("in {}h", gap / 3600),
        _ => format!("in {}d", gap / 86_400),
    }
}

/// The chat's wake-ups, soonest first: name, when, its note, and a way
/// for the person to cancel each. Nothing when it has none.
fn panel(at: &AtRun, view: &mut ViewCx<'_, ScheduleUi>) -> Option<AnyElement> {
    let run = at.run.id.to_string();
    let waiting: Vec<Wake> = view.data.of_run(&run).cloned().collect();
    if waiting.is_empty() {
        return None;
    }
    let t = view.theme().clone();
    let now = now();
    let rows: Vec<Div> = waiting
        .iter()
        .map(|wake| row(wake, now, view, &t))
        .collect();
    Some(
        div()
            .flex()
            .flex_col()
            .gap(sp(2.))
            .p(sp(3.))
            .border_b_1()
            .border_color(t.border)
            .child(
                div()
                    .typeset(Type::CAPTION)
                    .font_weight(weight::STRONG)
                    .text_color(t.text)
                    .child("Scheduled"),
            )
            .children(rows)
            .into_any_element(),
    )
}

/// One wake-up in the inspector.
fn row(
    wake: &Wake,
    now: i64,
    view: &mut ViewCx<'_, ScheduleUi>,
    t: &Theme,
) -> Div {
    let handle = view.handle.clone();
    let (run, name) = (wake.run.clone(), wake.name.clone());
    let cancel = div()
        .id(SharedString::from(format!(
            "scheduled-{}-{}-cancel",
            wake.run, wake.name
        )))
        .size(rems(1.25))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius::SMALL)
        .cursor_pointer()
        .hover(|style| style.bg(t.raised))
        .child(icon(Icon::Close, IconSize::SMALL, t.dim))
        .on_click(move |_, _, cx| {
            handle.act(
                Act::Cancel {
                    run: run.clone(),
                    name: name.clone(),
                },
                cx,
            )
        });
    div()
        .flex()
        .flex_col()
        .gap(sp(0.5))
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(2.))
                .child(icon(Icon::Clock, IconSize::SMALL, t.text_soft))
                .child(
                    div()
                        .flex_1()
                        .min_w(rems(0.))
                        .truncate()
                        .typeset(Type::CAPTION)
                        .text_color(t.text_soft)
                        .child(wake.name.clone()),
                )
                .child(
                    div()
                        .typeset(Type::MICRO)
                        .text_color(t.dim)
                        .child(relative(wake.at, now)),
                )
                .child(cancel),
        )
        .child(
            div()
                .typeset(Type::MICRO)
                .text_color(t.dim)
                .line_clamp(2)
                .child(wake.note.clone()),
        )
}

/// The message a chat woke on, as the card it is: each wake-up's name
/// and when, then its note.
fn card(
    at: &AtMessage,
    view: &mut ViewCx<'_, ScheduleUi>,
) -> Option<AnyElement> {
    let woken = Woken::parse(&at.text)?;
    let t = view.theme().clone();
    let blocks = woken.into_iter().enumerate().map(|(index, woken)| {
        div()
            .id(SharedString::from(format!("woken-{}-{index}", at.index)))
            .flex()
            .flex_col()
            .gap(sp(1.))
            .px(sp(3.))
            .py(sp(2.))
            .when(index > 0, |block| block.border_t_1().border_color(t.border))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(2.))
                    .child(icon(Icon::Clock, IconSize::SMALL, t.roles.agent))
                    .child(
                        div()
                            .flex_1()
                            .typeset(Type::CAPTION)
                            .text_color(t.text)
                            .child(format!("Scheduled · {}", woken.name)),
                    )
                    .child(mono(woken.due, Type::MICRO, t.dim)),
            )
            .child(
                div()
                    .typeset(Type::SMALL)
                    .text_color(t.text_soft)
                    .child(woken.note),
            )
    });
    Some(
        div()
            .max_w(rems(40.))
            .rounded(radius::BOX)
            .border_1()
            .border_color(t.border)
            .bg(t.card)
            .overflow_hidden()
            .children(blocks)
            .into_any_element(),
    )
}
