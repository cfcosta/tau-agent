//! The Agents page and the New agent form.

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
    components::{
        ButtonKind,
        Material as _,
        bar,
        button,
        card,
        empty,
        field,
        icon,
        mono,
    },
    format::usd,
    prose::rich,
    theme::{Design as _, IconSize, Theme, Tone, Type, radius, sp, weight},
};
use tau_ui_plugin::{Link, ViewCx};

use super::{AgentsUi, NEW, OVERVIEW, mark, open, status_pill};
use crate::{
    Act,
    Agent,
    COMMAND,
    Status,
    shown::{now, relative},
};

/// What a click on "Allow more" lets an agent spend, on top of its cap.
const MORE: f64 = 1.0;

/// Every agent: those that need the person first, then a card each.
pub(super) fn overview(view: &mut ViewCx<'_, AgentsUi>) -> AnyElement {
    let t = view.theme().clone();
    let overview = view.data.clone();
    let new = {
        let handle = view.handle.clone();
        button("New agent", ButtonKind::Primary, &t)
            .id("new-agent")
            .on_click(move |_, _, cx| handle.navigate(Link::page(NEW), cx))
    };
    let head = div()
        .flex()
        .flex_wrap()
        .items_start()
        .gap(sp(4.))
        .child(
            div()
                .flex_1()
                .min_w(rems(16.))
                .flex()
                .flex_col()
                .gap(sp(2.))
                .child(div().typeset(Type::HEADLINE).text_color(t.text).child("Agents"))
                .child(div().typeset(Type::SMALL).max_w(rems(42.)).child(rich(
                    &format!(
                        "Each agent is one conversation that never ends, in a \
                         repository of its own. What it listens to arrives as \
                         messages; it works through codemode and rewrites its \
                         own Luau, tools and instructions as it learns. Agents \
                         run while tau is open and catch up when it starts. \
                         `/{COMMAND}` and what it should do makes one from any \
                         chat."
                    ),
                    t.muted,
                    &t,
                ))),
        )
        .child(new);
    let mut column = div()
        .flex()
        .flex_col()
        .gap(sp(6.))
        .p(sp(6.))
        .max_w(rems(68.))
        .child(head);
    let waiting: Vec<&Agent> = overview
        .agents
        .iter()
        .filter(|agent| agent.needs_you())
        .collect();
    if !waiting.is_empty() {
        column = column.child(needs_you(&waiting, view, &t));
    }
    if overview.agents.is_empty() {
        column = column.child(empty("No agents yet.", &t));
    } else {
        let now = now();
        let cards: Vec<AnyElement> = overview
            .agents
            .iter()
            .map(|agent| agent_card(agent, now, view, &t).into_any_element())
            .collect();
        column =
            column.child(div().flex().flex_wrap().gap(sp(4.)).children(cards));
    }
    column.into_any_element()
}

/// The agents that wait on the person, each with what to do about it.
fn needs_you(
    agents: &[&Agent],
    view: &mut ViewCx<'_, AgentsUi>,
    t: &Theme,
) -> Div {
    let rows = agents.iter().map(|agent| {
        let (line, action): (String, Option<AnyElement>) = match &agent.status {
            Status::AtCap => {
                let handle = view.handle.clone();
                let name = agent.name.clone();
                (
                    format!(
                        "Reached its {} for today; {} waiting.",
                        usd(agent.cap_today),
                        events(agent.queued),
                    ),
                    Some(
                        button(
                            format!("Allow {} more today", usd(MORE)),
                            ButtonKind::Primary,
                            t,
                        )
                        .id(SharedString::from(format!("allow-{}", agent.name)))
                        .on_click(move |_, _, cx| {
                            handle.act(
                                Act::AllowMore {
                                    agent: name.clone(),
                                    amount: MORE,
                                },
                                cx,
                            )
                        })
                        .into_any_element(),
                    ),
                )
            }
            Status::Broken { error } => {
                (format!("Nothing wakes it: {error}"), None)
            }
            _ => return div(),
        };
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(sp(3.))
            .px(sp(4.))
            .py(sp(3.))
            .border_b_1()
            .border_color(t.border)
            .child(mark(&agent.name, 20., t))
            .child(
                div()
                    .flex_1()
                    .min_w(rems(14.))
                    .flex()
                    .flex_col()
                    .gap(sp(0.5))
                    .child(mono(agent.name.clone(), Type::CAPTION, t.text_soft))
                    .child(
                        div()
                            .typeset(Type::SMALL)
                            .text_color(t.text)
                            .child(line),
                    ),
            )
            .children(action)
    });
    let rows: Vec<Div> = rows.collect();
    // What each agent left for the person: its notes and questions.
    let left: Vec<Div> = agents
        .iter()
        .filter(|agent| !agent.notices.is_empty())
        .map(|agent| {
            div()
                .flex()
                .items_start()
                .gap(sp(3.))
                .px(sp(4.))
                .py(sp(3.))
                .border_b_1()
                .border_color(t.border)
                .child(mark(&agent.name, 20., t))
                .child(
                    div()
                        .flex_1()
                        .min_w(rems(14.))
                        .flex()
                        .flex_col()
                        .gap(sp(3.))
                        .child(mono(
                            agent.name.clone(),
                            Type::CAPTION,
                            t.text_soft,
                        ))
                        .children(super::notices::rows(agent, view, t)),
                )
        })
        .collect();
    let count: usize = agents.iter().map(|agent| agent.waits_on_you()).sum();
    div()
        .flex()
        .flex_col()
        .gap(sp(2.5))
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(sp(2.))
                .child(
                    div()
                        .typeset(Type::SMALL)
                        .font_weight(weight::STRONG)
                        .text_color(t.text)
                        .child("Needs you"),
                )
                .child(
                    div()
                        .typeset(Type::CAPTION)
                        .text_color(t.dim)
                        .child(count.to_string()),
                ),
        )
        .child(
            card(t)
                .border_color(t.blue_border)
                .children(rows)
                .children(left),
        )
}

/// `n events`, said as a count.
fn events(count: usize) -> String {
    match count {
        0 => "no events".to_owned(),
        1 => "1 event".to_owned(),
        n => format!("{n} events"),
    }
}

/// One agent: what it is for, where it stands, what wakes it, and what
/// it spent of today's cap. A click opens its conversation.
fn agent_card(
    agent: &Agent,
    now: i64,
    view: &mut ViewCx<'_, AgentsUi>,
    t: &Theme,
) -> impl IntoElement {
    let caption = match (&agent.status, agent.next()) {
        (Status::Working, _) => format!("in a turn · {}", events(agent.queued)),
        (Status::Waiting, Some(next)) => {
            format!("waiting · next {}", relative(next, now))
        }
        (Status::Waiting, None) => "waiting".to_owned(),
        (Status::Paused, _) => {
            format!("paused · {} waiting", events(agent.queued))
        }
        (Status::AtCap, _) => {
            format!("at its cap · {} waiting", events(agent.queued))
        }
        (Status::Broken { .. }, _) => "broken".to_owned(),
    };
    let last = agent
        .last
        .as_ref()
        .map(|last| format!("{} · {}", last.line, relative(last.at, now)))
        .unwrap_or_else(|| "nothing yet".to_owned());
    let spent = agent.spent_share();
    let meter = if agent.status == Status::AtCap {
        t.roles.waiting
    } else {
        t.tone(super::tone(&agent.status))
    };
    let triggers = agent.triggers.iter().map(|trigger| {
        // A poll that fails shows red; its page says why.
        let failing = trigger.error.is_some();
        div()
            .flex()
            .items_center()
            .gap(sp(1.5))
            .px(sp(2.))
            .py(sp(0.75))
            .rounded(radius::CONTROL)
            .bg(t.raised)
            .typeset(Type::MICRO)
            .text_color(if failing { t.red } else { t.text_soft })
            .child(icon(
                trigger_icon(trigger.kind_label()),
                IconSize::TINY,
                if failing { t.red } else { t.dim },
            ))
            .child(trigger.label())
    });
    // A card opens its agent's conversation.
    let run = agent
        .main
        .as_deref()
        .map(|main| tau_agent::tool::RunId(std::sync::Arc::from(main)))
        .or_else(|| super::conversation_of(&agent.name, view));
    let handle = view.handle.clone();
    div()
        .id(SharedString::from(format!("agent-{}", agent.name)))
        .flex()
        .flex_col()
        .gap(sp(3.))
        .w(rems(21.))
        .flex_grow(1.)
        .p(sp(4.))
        .rounded(radius::BOX)
        .raised(t)
        .border_1()
        .border_color(t.border)
        .cursor_pointer()
        .hover(|style| style.border_color(t.border_strong))
        .on_click(move |_, _, cx| {
            if let Some(run) = &run {
                handle.open_run(run, cx);
            }
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(3.))
                .child(mark(&agent.name, 32., t))
                .child(
                    div()
                        .flex_1()
                        .min_w(rems(0.))
                        .flex()
                        .flex_col()
                        .gap(sp(0.5))
                        .child(mono(agent.name.clone(), Type::BODY, t.text))
                        .child(
                            div()
                                .typeset(Type::CAPTION)
                                .text_color(t.dim)
                                .truncate()
                                .child(caption),
                        ),
                )
                .child(status_pill(&agent.status, t)),
        )
        .child(
            div()
                .typeset(Type::SMALL)
                .text_color(t.muted)
                .line_clamp(3)
                .child(agent.purpose.clone()),
        )
        .when_some(agent.sandbox.clone(), |card, problem| {
            card.child(
                div()
                    .typeset(Type::CAPTION)
                    .text_color(t.tone(Tone::Warn))
                    .child(problem),
            )
        })
        .when(!agent.triggers.is_empty(), |card| {
            card.child(div().flex().flex_wrap().gap(sp(1.5)).children(triggers))
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(2.5))
                .pt(sp(2.5))
                .border_t_1()
                .border_color(t.border)
                .child(
                    div()
                        .flex_1()
                        .min_w(rems(0.))
                        .truncate()
                        .typeset(Type::CAPTION)
                        .text_color(t.text_soft)
                        .child(last),
                )
                .child(
                    div()
                        .flex()
                        .gap(sp(1.))
                        .typeset(Type::CAPTION)
                        .text_color(t.dim)
                        .child(
                            div()
                                .text_color(t.text)
                                .child(usd(agent.spend_today)),
                        )
                        .child(format!("of {}", usd(agent.cap_today))),
                ),
        )
        .child(bar(spent, 4., meter, t.raised))
}

/// The icon of a trigger's kind.
fn trigger_icon(kind: &str) -> Icon {
    match kind {
        "schedule" => Icon::Clock,
        "command" => Icon::Terminal,
        "poll" => Icon::History,
        "stream" => Icon::Arrow,
        "mcp" => Icon::Plug,
        _ => Icon::Chat,
    }
}

/// The New agent form: a name, what it should do, its model, cap and
/// window. Once the host made it, its conversation opens.
pub(super) fn new_agent(view: &mut ViewCx<'_, AgentsUi>) -> AnyElement {
    let t = view.theme().clone();
    // The host made it: open its conversation as soon as the window
    // knows it.
    let making = view.ui.read(view.cx).making.clone();
    if let Some(name) = &making {
        let agent = view.data.agent(name).cloned();
        let ready = agent.as_ref().is_some_and(|agent| agent.main.is_some())
            || super::conversation_of(name, view).is_some();
        if ready {
            match agent {
                Some(agent) => open(&agent, view),
                None => {
                    if let Some(run) = super::conversation_of(name, view) {
                        view.handle.open_run(&run, view.cx);
                    }
                }
            }
            view.ui.update(view.cx, |ui, cx| ui.reset(cx));
        }
    }
    let ui = view.ui.read(view.cx);
    let (name, purpose) = (ui.name.clone(), ui.purpose.clone());
    let (model, daily_cap, window_secs) =
        (ui.model.clone(), ui.daily_cap, ui.window_secs);
    let error = ui.error.clone();
    let name_text = name.read(view.cx).text().to_owned();
    let label = |text: &'static str, caption: Option<&'static str>| {
        div()
            .flex()
            .flex_col()
            .gap(sp(0.75))
            .child(div().typeset(Type::SMALL).text_color(t.text).child(text))
            .children(caption.map(|caption| {
                div()
                    .typeset(Type::CAPTION)
                    .text_color(t.dim)
                    .child(caption)
            }))
    };
    let segment =
        |id: String,
         text: String,
         on: bool,
         set: Box<dyn Fn(&mut super::Ui) + 'static>| {
            let ui = view.ui.clone();
            div()
                .id(SharedString::from(id))
                .h(rems(1.875))
                .px(sp(3.))
                .flex()
                .items_center()
                .rounded(radius::CONTROL)
                .typeset(Type::CAPTION)
                .cursor_pointer()
                .text_color(if on { t.text } else { t.muted })
                .when(on, |segment| segment.key(&t))
                .child(text)
                .on_click(move |_, _, cx| {
                    ui.update(cx, |ui, cx| {
                        set(ui);
                        cx.notify();
                    })
                })
        };
    let segments = |children: Vec<gpui::Stateful<Div>>| {
        div()
            .flex()
            .flex_wrap()
            .gap(sp(0.5))
            .p(sp(0.75))
            .rounded(radius::CONTROL)
            .well(&t)
            .children(children)
    };
    let default_model = crate::default_model();
    let models: Vec<gpui::Stateful<Div>> = tau_ai::model::plan_models()
        .into_iter()
        .map(|plan| {
            let id = plan.id.to_owned();
            let on = model.as_deref().unwrap_or(&default_model) == id;
            let pick = (id != default_model).then(|| id.clone());
            segment(
                format!("new-agent-model-{id}"),
                id.clone(),
                on,
                Box::new(move |ui| ui.model = pick.clone()),
            )
        })
        .collect();
    let caps: Vec<gpui::Stateful<Div>> = [0.5, 1.0, 2.0, 5.0]
        .into_iter()
        .map(|cap| {
            segment(
                format!("new-agent-cap-{cap}"),
                format!("${cap:.2}"),
                (daily_cap - cap).abs() < f64::EPSILON,
                Box::new(move |ui| ui.daily_cap = cap),
            )
        })
        .collect();
    let windows: Vec<gpui::Stateful<Div>> =
        [(10, "10 s"), (30, "30 s"), (60, "1 min"), (300, "5 min")]
            .into_iter()
            .map(|(seconds, text)| {
                segment(
                    format!("new-agent-window-{seconds}"),
                    text.to_owned(),
                    window_secs == seconds,
                    Box::new(move |ui| ui.window_secs = seconds),
                )
            })
            .collect();
    let start = {
        let ui = view.ui.clone();
        button(
            if making.is_some() {
                "Starting it…"
            } else {
                "Start it"
            },
            ButtonKind::Primary,
            &t,
        )
        .id("start-agent")
        .on_click(move |_, _, cx| ui.update(cx, |ui, cx| ui.submit(cx)))
    };
    let cancel = {
        let handle = view.handle.clone();
        button("Cancel", ButtonKind::Secondary, &t)
            .id("cancel-agent")
            .on_click(move |_, _, cx| handle.navigate(Link::page(OVERVIEW), cx))
    };
    let taken = view.data.agent(&name_text).is_some()
        && making.as_deref() != Some(name_text.as_str());
    let invalid = !name_text.is_empty() && !crate::valid_name(&name_text);
    div()
        .flex()
        .flex_col()
        .gap(sp(5.))
        .p(sp(6.))
        .max_w(rems(42.))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(2.))
                .child(div().typeset(Type::HEADLINE).text_color(t.text).child("New agent"))
                .child(div().typeset(Type::SMALL).child(rich(
                    &format!(
                        "It gets a repository and a conversation of its own. \
                         What it should do is its first message; from there it \
                         sets up what wakes it, its tools and its instructions \
                         itself. `/{COMMAND}` and what it should do in any chat \
                         opens this form."
                    ),
                    t.muted,
                    &t,
                ))),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(2.))
                .child(label("Name", None))
                .child(field(&name, true, &t))
                .when(invalid, |column| {
                    column.child(div().typeset(Type::CAPTION).text_color(t.red).child(
                        "Lower-case letters, digits and dashes, starting with a letter.",
                    ))
                })
                .when(taken, |column| {
                    column.child(div().typeset(Type::CAPTION).text_color(t.red).child("An agent has this name already."))
                }),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(2.))
                .child(label("What it should do", None))
                .child(
                    div()
                        .min_h(rems(6.))
                        .px(sp(3.))
                        .py(sp(2.5))
                        .rounded(radius::BOX)
                        .well(&t)
                        .typeset(Type::BODY)
                        .child(purpose),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(2.))
                .child(label("Model", Some("A small model keeps a conversation that never ends cheap.")))
                .child(segments(models)),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(sp(6.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(sp(2.))
                        .child(label("Most it may spend a day", None))
                        .child(segments(caps)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(sp(2.))
                        .child(label("Gather events for", None))
                        .child(segments(windows)),
                ),
        )
        .children(error.map(|error| div().typeset(Type::SMALL).text_color(t.red).child(error)))
        .child(
            div()
                .flex()
                .justify_end()
                .gap(sp(2.5))
                .child(cancel)
                .child(start),
        )
        .child(div().flex().items_center().gap(sp(1.5)).child(icon(Icon::Info, IconSize::SMALL, t.dim)).child(
            div().typeset(Type::CAPTION).text_color(t.dim).child(
                "Only you can change what it may spend; it cannot.",
            ),
        ))
        .into_any_element()
}
