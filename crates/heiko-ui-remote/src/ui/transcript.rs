//! The chat: messages, tool calls as lines, and what plugins say in
//! between.

use gpui::{
    AnyElement,
    Context,
    Div,
    Hsla,
    IntoElement,
    SharedString,
    Stateful,
    Window,
    div,
    prelude::*,
    rems,
};
use heiko_ui_kit::syntax::Lang;

use super::{
    Material as _,
    link,
    link_action,
    mark,
    mono,
    rich,
    stamp,
    stop_look,
};
use crate::{
    assets::State,
    route::Route,
    theme::{Design as _, Theme, Type, sp, weight},
    view::{
        Dropped,
        Item,
        LandedCard,
        PluginNote,
        RunView,
        Tone,
        ToolCard,
        ToolState,
        proposed_text,
        tokens,
    },
    workspace::Workspace,
};

/// The run's item at `index`, padded as the transcript lays it out: the
/// transcript is a list that builds only the items in view. `compact`
/// is the phone layout.
pub fn item(
    ws: &Workspace,
    run: &RunView,
    index: usize,
    t: &Theme,
    compact: bool,
    _window: &Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let edge = sp(if compact { 4. } else { 7. });
    let side = sp(if compact { 4. } else { 6. });
    // Past the items, the landing card, while a landing is open, where
    // a landed chat's work went, or a main chat's card for its last push.
    if index == run.items.len() {
        let card = match run.ending {
            Some(_) => super::ending::landed_card(ws, run, t, cx),
            None => super::interrupted::card(run, t, cx)
                .or_else(|| super::landing::card(ws, run, t, cx)),
        };
        // A main chat's: what waits to land on it, and conflicts left
        // on it.
        let queue = super::queue::cards(ws, run, t, cx);
        // The model asked and nothing back yet: it reasons.
        let open =
            matches!(run.items.last(), Some(Item::Thinking { secs: None, .. }));
        let waiting = ws
            .reasoning_for(&run.id)
            .filter(|_| !open)
            .map(|waited| reasoning("", None, Some(waited), t));
        return column(
            div()
                .pt(sp(3.))
                .pb(edge)
                .w_full()
                .flex()
                .flex_col()
                .gap(sp(4.))
                .children(waiting)
                .children(
                    queue
                        .into_iter()
                        .chain(card)
                        .chain(super::push::card(ws, run, t, cx))
                        .map(|card| card.w_full()),
                ),
            side,
            compact,
        )
        .into_any_element();
    }
    let Some(item) = run.items.get(index) else {
        return div().into_any_element();
    };
    // Calls one after another are the rows of one list; a refused one
    // stands apart, under its rule.
    let stacked = matches!(
        (index.checked_sub(1).and_then(|at| run.items.get(at)), item),
        (Some(Item::Tool(_)), Item::Tool(card))
            if !matches!(card.state, ToolState::Blocked { .. })
    );
    let above = if index == 0 {
        edge
    } else if stacked {
        sp(0.)
    } else {
        sp(4.)
    };
    column(
        div()
            .w_full()
            .pt(above)
            .when(index + 1 == run.items.len(), |item| item.pb(edge))
            .child(item_view(ws, run, index, item, t, compact, cx)),
        side,
        compact,
    )
    .into_any_element()
}

/// How wide the transcript's column is on a desktop: a reading measure,
/// centered in whatever room the window gives.
const COLUMN: gpui::Rems = rems(47.5);

/// `content` in the transcript's column: centered and at most
/// [`COLUMN`] wide on a desktop, edge to edge on a phone.
fn column(content: Div, side: gpui::Rems, compact: bool) -> Div {
    div()
        .w_full()
        .px(side)
        .flex()
        .justify_center()
        .child(content.when(!compact, |content| content.max_w(COLUMN)))
}

fn item_view(
    ws: &Workspace,
    run: &RunView,
    index: usize,
    item: &Item,
    t: &Theme,
    compact: bool,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    match item {
        // A plugin that reads the message as its own draws it.
        Item::User(text) => ws
            .drawn(
                heiko_ui_plugin::points::USER_MESSAGE,
                &heiko_ui_plugin::points::AtMessage {
                    run: run.info(),
                    text: text.clone(),
                    index,
                },
                cx,
            )
            .into_iter()
            .next()
            .unwrap_or_else(|| user(text, t, compact).into_any_element()),
        Item::Pictures(pictures) => {
            self::pictures(ws, run, pictures, t, compact, cx).into_any_element()
        }
        Item::Text(text) => div()
            .w_full()
            .typeset(Type::LEAD.leading(1.6))
            .text_color(t.text)
            .child(super::markdown(
                &format!("{}-{index}", run.id.0),
                text,
                t.text,
                t,
            ))
            .into_any_element(),
        Item::Thinking { text, secs } => {
            let live = (secs.is_none() && index + 1 == run.items.len())
                .then(|| ws.reasoning_for(&run.id))
                .flatten();
            thinking(ws, run, index, text, *secs, live, t, cx)
                .into_any_element()
        }
        Item::TurnEnd { turn } => {
            turn_end(ws, run, *turn, t, compact, cx).into_any_element()
        }
        Item::Tool(card) => {
            tool(ws, run, card, t, compact, cx).into_any_element()
        }
        Item::Plugin(note) => {
            plugin_note(ws, run, index, note, t, compact, cx).into_any_element()
        }
        Item::Anchor { plugin, key } => {
            let at = heiko_ui_plugin::points::AtAnchor {
                run: run.info(),
                key: key.clone(),
                index,
            };
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(sp(2.))
                .children(ws.drawn_of(
                    plugin,
                    heiko_ui_plugin::points::TRANSCRIPT,
                    &at,
                    cx,
                ))
                .into_any_element()
        }
        Item::Landed(card) if card.recovered => div()
            .flex()
            .flex_col()
            .gap(sp(2.))
            .child(super::interrupted::finished_landing(&card.title, t))
            .child(landed(ws, run, card, t, cx))
            .into_any_element(),
        Item::Landed(card) => landed(ws, run, card, t, cx).into_any_element(),
        Item::ForkReady { fork } => {
            fork_ready(ws, run, fork, t, cx).into_any_element()
        }
        // heiko's own message, unless a plugin reads it as its own (an
        // agent's events).
        Item::Heiko(text) => ws
            .drawn(
                heiko_ui_plugin::points::USER_MESSAGE,
                &heiko_ui_plugin::points::AtMessage {
                    run: run.info(),
                    text: text.clone(),
                    index,
                },
                cx,
            )
            .into_iter()
            .next()
            .unwrap_or_else(|| heiko_message(text, t).into_any_element()),
        // The plugin that named its rewrite draws it.
        Item::Rewrite {
            plugin,
            tokens,
            key,
            idle,
        } => key
            .as_ref()
            .and_then(|key| {
                ws.drawn_of(
                    plugin,
                    heiko_ui_plugin::points::REWRITE,
                    &heiko_ui_plugin::points::AtRewrite {
                        run: run.info(),
                        key: key.clone(),
                        tokens: *tokens,
                        index,
                    },
                    cx,
                )
                .into_iter()
                .next()
            })
            .unwrap_or_else(|| {
                rewrite(plugin, *tokens, *idle, t, compact).into_any_element()
            }),
        Item::Retry {
            attempt,
            delay,
            error,
        } => div()
            .flex()
            .items_baseline()
            .gap(sp(2.5))
            .child(
                mono(
                    format!("retry {attempt} in {:.1} s", delay.as_secs_f32()),
                    Type::CAPTION,
                    t.roles.live,
                )
                .flex_shrink_0(),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(rems(0.))
                    .typeset(Type::CAPTION)
                    .text_color(t.dim)
                    .child(error.clone()),
            )
            .into_any_element(),
        // A conversation that stopped by itself waits for the next
        // message: nothing to say.
        Item::Stop {
            stop: heiko_agent::event::StopReason::Stop,
            ..
        } => div().into_any_element(),
        Item::Stop {
            stop,
            turns,
            tokens: used,
            ..
        } => {
            let (color, label) = stop_look(stop, t);
            let state = match stop {
                heiko_agent::event::StopReason::Cancelled => State::Idle,
                heiko_agent::event::StopReason::Limit(_) => State::Limit,
                _ => State::Failing,
            };
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(sp(2.5))
                .pt(sp(2.5))
                .border_t_1()
                .border_color(t.hairline)
                .child(mark(state, t))
                .child(div().text_color(color).child(label))
                .child(mono(
                    format!("{turns} turns · {} tokens", tokens(*used)),
                    Type::CAPTION,
                    t.dim,
                ))
                .into_any_element()
        }
    }
}

/// What the person wrote: large type between two hairlines, under a
/// small `you`. Attached files show as their names, not their content.
fn user(text: &str, t: &Theme, compact: bool) -> Div {
    let (said, files) = split_attachments(text);
    let style = if compact {
        Type::HEADING.leading(1.35)
    } else {
        Type::HEADLINE.sized(24.).leading(1.35)
    };
    super::bubble(t)
        .w_full()
        .flex()
        .flex_col()
        .gap(sp(1.5))
        .child(mono("you", Type::CAPTION, t.dim))
        .child(
            div()
                .typeset(style)
                .text_color(t.bright)
                .child(rich(said, t.bright, t)),
        )
        .when(!files.is_empty(), |said| {
            said.child(div().flex().flex_wrap().gap(sp(3.5)).children(
                files.into_iter().map(|name| {
                    mono(name.to_owned(), Type::CAPTION, t.text_soft)
                }),
            ))
        })
}

/// How tall a picture the person sent is drawn in the transcript.
const PICTURE: f32 = 12.;

/// The images the person sent with the message above, under it: each
/// as the window has it, fetched from the host the first time.
fn pictures(
    ws: &Workspace,
    run: &RunView,
    pictures: &[crate::view::Picture],
    t: &Theme,
    compact: bool,
    cx: &mut Context<Workspace>,
) -> Div {
    let height = rems(if compact { PICTURE * 0.75 } else { PICTURE });
    div().flex().flex_wrap().gap(sp(4.)).pb(sp(1.)).children(
        pictures.iter().map(|picture| {
            let frame = div().h(height).bg(t.card).lifted(t);
            match ws.picture(&run.id, picture, cx) {
                crate::pictures::Shown::Ready(image) => frame.child(
                    gpui::img(image)
                        .h(height)
                        .max_w(rems(if compact { 18. } else { 28. }))
                        .object_fit(gpui::ObjectFit::Contain),
                ),
                crate::pictures::Shown::Waiting(_) => frame
                    .w(height)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(mono("image", Type::MICRO, t.dim)),
                crate::pictures::Shown::Failed(why) => frame
                    .w(height)
                    .flex()
                    .items_center()
                    .justify_center()
                    .p(sp(2.))
                    .child(
                        div()
                            .typeset(Type::MICRO)
                            .text_color(t.muted)
                            .text_center()
                            .child(format!("Not available: {why}")),
                    ),
            }
        }),
    )
}

/// A message's own words, and the names of the files attached after
/// them.
fn split_attachments(text: &str) -> (&str, Vec<&str>) {
    const MARK: &str = "\n\n<attached file=\"";
    let Some(start) = text.find(MARK) else {
        return (text, Vec::new());
    };
    let names = text[start..]
        .match_indices("<attached file=\"")
        .filter_map(|(at, open)| {
            let rest = &text[start + at + open.len()..];
            rest.find('"').map(|end| &rest[..end])
        })
        .collect();
    (&text[..start], names)
}

/// Where one turn ends. Nothing shows there; on a desktop, hovering the
/// gap offers a fork from that turn. A phone forks from the run's sheet.
fn turn_end(
    ws: &Workspace,
    run: &RunView,
    turn: u32,
    t: &Theme,
    compact: bool,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let group = SharedString::from(format!("turn-{turn}"));
    let forkable = !compact && ws.can_fork_at(run, turn);
    let id = run.id.clone();
    div().relative().when(forkable, |gap| {
        gap.child(
            div()
                .group(group.clone())
                .absolute()
                .right_0()
                .top(rems(-0.625))
                .h(rems(1.25))
                .w(rems(15.))
                .flex()
                .items_center()
                .justify_end()
                .child(
                    div()
                        .id(SharedString::from(format!("fork-at-{turn}")))
                        .opacity(0.)
                        .group_hover(group, |style| style.opacity(1.))
                        .child(link(format!("Fork from turn {turn}"), t))
                        .on_click(cx.listener(move |ws, _, window, cx| {
                            ws.fork_from(&id, turn, window, cx)
                        })),
                ),
        )
    })
}

/// The model's reasoning, folded to one line, with a way to read it;
/// open, its words under the line.
#[allow(clippy::too_many_arguments)]
fn thinking(
    ws: &Workspace,
    run: &RunView,
    index: usize,
    text: &str,
    secs: Option<u64>,
    live: Option<std::time::Duration>,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let open = ws.note_open(&run.id, index);
    let line = reasoning(text, secs, live, t);
    let line = if text.trim().is_empty() {
        line
    } else {
        let run_id = run.id.clone();
        line.child(
            div()
                .id(SharedString::from(format!("reasoning-{index}")))
                .child(link_action(if open { "hide" } else { "show" }, t))
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.toggle_note(&run_id, index, cx)
                })),
        )
    };
    // While it goes, its latest line shows under the fold.
    let latest = live
        .filter(|_| !open)
        .and_then(|_| {
            text.lines().map(str::trim).rfind(|line| !line.is_empty())
        })
        .map(|line| line.chars().take(400).collect::<String>());
    div()
        .flex()
        .flex_col()
        .gap(sp(1.5))
        .child(line)
        .when_some(latest, |fold, line| {
            fold.child(
                div()
                    .typeset(Type::SMALL)
                    .text_color(t.dim)
                    .truncate()
                    .child(line),
            )
        })
        .when(open, |fold| {
            fold.child(
                div()
                    .typeset(Type::SMALL.leading(1.6))
                    .text_color(t.muted)
                    .child(text.trim().to_owned()),
            )
        })
}

/// The reasoning's one line: while it goes (`live`, how long it has),
/// the working mark, the seconds and the words so far; once it ended,
/// how long it took and how many words it had.
fn reasoning(
    text: &str,
    secs: Option<u64>,
    live: Option<std::time::Duration>,
    t: &Theme,
) -> Div {
    let words = text.split_whitespace().count();
    let mut parts = vec![
        if live.is_some() {
            "Reasoning"
        } else {
            "Reasoned"
        }
        .to_owned(),
    ];
    if let Some(secs) = live.map(|waited| waited.as_secs()).or(secs) {
        parts.push(duration(secs));
    }
    match words {
        0 => {}
        1 => parts.push("1 word".to_owned()),
        _ => parts.push(format!("{words} words")),
    }
    let said = match parts.split_first() {
        Some((word, rest)) if rest.is_empty() => word.clone(),
        Some((word, rest)) => format!("{word} {}", rest.join(" · ")),
        None => String::new(),
    };
    div()
        .flex()
        .items_center()
        .gap(sp(2.5))
        .typeset(Type::SMALL)
        .text_color(if live.is_some() { t.text_soft } else { t.dim })
        .when(live.is_some(), |row| row.child(mark(State::Working, t)))
        .child(said)
}

/// `secs` for a person: `18 s`, `2 m 05 s`.
fn duration(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs} s"),
        _ => format!("{} m {:02} s", secs / 60, secs % 60),
    }
}

/// A tool call, as one line of the transcript: the tool's name in the
/// color of its kind, its arguments, and what came back at the line's
/// end. Its body, when open, sits under it on a flat band. A refused
/// call stands under a red rule, with what it proposed.
fn tool(
    ws: &Workspace,
    run: &RunView,
    card: &ToolCard,
    t: &Theme,
    compact: bool,
    cx: &mut Context<Workspace>,
) -> Div {
    let dropped = card.dropped.is_some();
    // What plugins draw on the card, each given its own anchors on it.
    let at_card = |plugin: &str| crate::views::at_card(run, card, plugin);
    let names: Vec<String> =
        ws.plugin_names().into_iter().map(str::to_owned).collect();
    let contexts: Vec<(String, heiko_ui_plugin::points::AtCard)> = names
        .into_iter()
        .map(|name| {
            let at = at_card(&name);
            (name, at)
        })
        .collect();
    let context_of = |plugin: &str| {
        contexts
            .iter()
            .find(|(name, _)| name == plugin)
            .map(|(_, at)| at)
    };
    // The tool's own plugin draws the card; a call a rewrite dropped
    // shows only its arguments.
    let (drew, view) = ws
        .contributions_named(heiko_ui_plugin::points::CARD, context_of)
        .into_iter()
        .next()
        .filter(|_| !dropped)
        .map_or((None, Default::default()), |(plugin, view)| {
            (Some(plugin), view)
        });
    // The card's trees, drawn with the plugin that gave them.
    let mut part = |name: &str, node: Option<heiko_ui_plugin::tree::Node>| {
        let (plugin, node) = (drew?, node?);
        let at = context_of(plugin)?;
        let id = format!("{plugin}-card-{}-{name}", card.call_id);
        let origin = crate::plugins::Origin::at(at);
        Some(ws.plugin_tree(plugin, &id, &node, origin, cx))
    };
    let (head, shape, card_body) = (
        part("head", view.head),
        part("shape", view.shape),
        part("body", view.body),
    );
    let badges =
        ws.drawn_with(heiko_ui_plugin::points::CARD_BADGE, context_of, cx);
    let extras =
        ws.drawn_with(heiko_ui_plugin::points::CARD_BODY, context_of, cx);

    let blocked = matches!(card.state, ToolState::Blocked { .. });
    let failed =
        view.failed.is_some() || matches!(card.state, ToolState::Failed(_));
    let name = if blocked || failed {
        t.red
    } else {
        t.roles.tool(&card.tool)
    };
    // A result that wants attention is written in its tone.
    let quiet = match view.edge {
        Some(Tone::Danger) => t.red,
        Some(Tone::Warn) => t.roles.live,
        _ => t.dim,
    };
    let summary = match head {
        Some(head) => div()
            .flex_1()
            .min_w(rems(0.))
            .child(head)
            .into_any_element(),
        None => mono(card.summary.clone(), Type::SMALL, t.text_soft)
            .flex_1()
            .min_w(rems(0.))
            .truncate()
            .when(card.dropped == Some(Dropped::Call), |s| {
                s.line_through().text_color(t.muted)
            })
            .into_any_element(),
    };
    // A body that folds starts closed; its line opens it.
    let folds = view.folds && card_body.is_some();
    let open = !folds || ws.card_open(&run.id, &card.call_id);
    // A failed call whose shape already stamps how it ended (`exit
    // 100`) says it once: the shape stands for the failure.
    let label = match &view.failed {
        _ if failed && !blocked && shape.is_some() => None,
        Some(failure) => Some(stamp(failure.clone(), Tone::Danger, t)),
        None => Some(match (&card.state, view.label) {
            (ToolState::Done { .. }, Some(text)) => {
                result_label(&text, quiet, t)
            }
            _ => state_label(card, quiet, t),
        }),
    };
    let header = div()
        .id(SharedString::from(format!("tool-{}", card.call_id)))
        .flex()
        .items_center()
        .gap(sp(3.))
        .min_h(rems(if compact { 2.25 } else { 1.5625 }))
        .child(
            mono(card.tool.clone(), Type::SMALL, name)
                .flex_shrink_0()
                .min_w(rems(if compact { 4. } else { 7. }))
                .font_weight(weight::EMPHASIS),
        )
        .when_some(
            card.from_plugin.clone().filter(|_| !compact),
            |row, plugin| {
                row.child(mono(plugin, Type::MICRO, t.dim).flex_shrink_0())
            },
        )
        .child(summary)
        // The call's stamp or diffstat, then what came back.
        .children(shape.map(|shape| div().flex_shrink_0().child(shape)))
        .children(label)
        .children(badges)
        .when(folds, |row| {
            let run_id = run.id.clone();
            let call_id = card.call_id.clone();
            row.cursor_pointer()
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.toggle_card(&run_id, &call_id, cx)
                }))
                .child(
                    mono(if open { "−" } else { "+" }, Type::SMALL, t.dim)
                        .flex_shrink_0(),
                )
        });

    // What the plugin that refused a call draws goes beside its reason.
    let (verdict, extras) = if blocked {
        (extras, Vec::new())
    } else {
        (Vec::new(), extras)
    };
    let body: Option<AnyElement> = match &card.state {
        ToolState::Blocked { reason, .. } => Some(
            blocked_body(card, reason, verdict, t, compact).into_any_element(),
        ),
        ToolState::Flagged { .. } => None,
        _ => card_body.filter(|_| open).map(|body| {
            div()
                .w_full()
                .mt(sp(1.5))
                .mb(sp(2.))
                .bg(t.card)
                .child(body)
                .into_any_element()
        }),
    };

    // What a plugin cut from the result, and the file that holds the
    // whole output, which opens with the system's application; unless
    // the body says it itself.
    let cut =
        card.cut
            .as_ref()
            .filter(|_| !dropped && !view.inset)
            .map(|cut| {
                let path = std::path::PathBuf::from(&cut.archive);
                div()
                    .flex()
                    .items_center()
                    .gap(sp(2.))
                    .min_w(rems(0.))
                    .pb(sp(1.5))
                    .child(
                        mono(
                            format!("{} · full output", cut.label()),
                            Type::MICRO,
                            t.dim,
                        )
                        .flex_shrink_0(),
                    )
                    .when(!compact, |row| {
                        row.child(
                            div()
                                .id(SharedString::from(format!(
                                    "archive-{}",
                                    card.call_id
                                )))
                                .flex_1()
                                .min_w(rems(0.))
                                .cursor_pointer()
                                .hover(|style| style.underline())
                                .child(
                                    mono(
                                        cut.archive.clone(),
                                        Type::MICRO,
                                        t.blue,
                                    )
                                    .truncate(),
                                )
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.open_with_system(&path)
                                })),
                        )
                    })
            });

    div()
        .flex()
        .flex_col()
        .when(dropped, |card| card.opacity(0.7))
        .when(blocked, |card| {
            card.pt(sp(2.5)).border_t_2().border_color(t.red)
        })
        .child(header)
        .children(body)
        .children(cut)
        .children(extras)
}

/// A child run's landing, in its parent's chat: what came, and what is
/// left to resolve, under a rule. The child's title opens its closed
/// chat.
fn landed(
    ws: &Workspace,
    run: &RunView,
    card: &LandedCard,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let from = card.from.clone();
    let count = match card.changes.len() {
        1 => "1 change".to_owned(),
        n => format!("{n} changes"),
    };
    div()
        .flex()
        .flex_col()
        .gap(sp(2.5))
        .pt(sp(2.5))
        .border_t_2()
        .border_color(t.change)
        .child(
            div()
                // A chat that goes on lands more than once (ADR 0034):
                // each card by its newest change.
                .id(SharedString::from(format!(
                    "landed-{}-{}",
                    card.from.0,
                    card.changes
                        .first()
                        .map_or("", |change| change.commit_id.as_str())
                )))
                .flex()
                .items_baseline()
                .gap(sp(3.))
                .cursor_pointer()
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.navigate(Route::Run(from.clone()), cx)
                }))
                .child(mono("landed", Type::SMALL, t.change).flex_shrink_0())
                .child(
                    div()
                        .flex_1()
                        .min_w(rems(0.))
                        .truncate()
                        .typeset(Type::LEAD)
                        .text_color(t.text)
                        .child(card.title.clone()),
                )
                .child(mono(count, Type::CAPTION, t.dim).flex_shrink_0()),
        )
        .when(
            !card.changes.is_empty() || !card.conflicts.is_empty(),
            |note| {
                let at = heiko_ui_plugin::points::AtChanges {
                    run: run.info(),
                    changes: card.changes.clone(),
                };
                note.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(sp(2.))
                        .children(ws.drawn(
                            heiko_ui_plugin::points::CHANGES,
                            &at,
                            cx,
                        ))
                        .when(!card.conflicts.is_empty(), |body| {
                            body.child(div().text_color(t.red).child(format!(
                                "Conflicts in {}: the next turn resolves them.",
                                card.conflicts.join(", ")
                            )))
                        }),
                )
            },
        )
}

/// A message heiko sent to start a turn itself (ADR 0014): resolving what
/// a landing or a merge left in conflict. Between two hairlines, as the
/// person's messages are, under a small `heiko`.
fn heiko_message(text: &str, t: &Theme) -> Div {
    super::bubble(t)
        .w_full()
        .flex()
        .flex_col()
        .gap(sp(1.5))
        .child(mono("heiko", Type::CAPTION, t.blue))
        .child(div().typeset(Type::LEAD).child(crate::ui::prose(
            text,
            t.text_soft,
            t,
        )))
}

/// A finished fork, waiting in its parent's chat: what it is, and Land
/// or Drop, under a rule. A closed fork (landed or dropped) shows
/// nothing.
fn fork_ready(
    ws: &Workspace,
    run: &RunView,
    fork: &heiko_agent::tool::RunId,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let Some(view) = ws.run(fork).filter(|_| !ws.is_closed(fork)) else {
        return div();
    };
    let (open, compare) = (fork.clone(), fork.clone());
    let main = run.id.clone();
    div()
        .flex()
        .flex_col()
        .gap(sp(2.5))
        .pt(sp(2.5))
        .border_t_2()
        .border_color(t.blue)
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(sp(3.))
                .child(mono("fork", Type::SMALL, t.blue).flex_shrink_0())
                .child(
                    div()
                        .flex_1()
                        .min_w(rems(0.))
                        .truncate()
                        .typeset(Type::LEAD)
                        .text_color(t.text)
                        .child(view.title.clone()),
                )
                .child(
                    mono(
                        format!("finished · turn {}", view.turn),
                        Type::CAPTION,
                        t.dim,
                    )
                    .flex_shrink_0(),
                ),
        )
        .children(super::landing::controls(ws, view, t, cx))
        .child(
            div()
                .flex()
                .gap(sp(4.))
                .typeset(Type::SMALL)
                .child(
                    div()
                        .id(SharedString::from(format!("fork-chat-{fork}")))
                        .child(link_action("open its chat", t))
                        .on_click(cx.listener(move |ws, _, _, cx| {
                            ws.navigate(Route::Run(open.clone()), cx)
                        })),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("fork-compare-{fork}")))
                        .child(link_action("compare", t))
                        .on_click(cx.listener(move |ws, _, _, cx| {
                            ws.navigate(
                                Route::Compare {
                                    main: main.clone(),
                                    fork: compare.clone(),
                                },
                                cx,
                            )
                        })),
                ),
        )
}

/// What came back, at the end of a call's line.
fn label(text: String, color: Hsla) -> Div {
    mono(text, Type::SMALL, color)
        .flex_shrink_0()
        .max_w(rems(16.25))
        .truncate()
}

/// A result at the end of a call's line: its counts of lines added
/// (`+3`) in green and removed (`−1`) in red, the rest in `quiet`.
fn result_label(text: &str, quiet: Hsla, t: &Theme) -> Div {
    let count = |word: &str, signs: &[char]| {
        word.strip_prefix(signs).is_some_and(|digits| {
            !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
        })
    };
    div()
        .flex()
        .items_baseline()
        .flex_shrink_0()
        .gap(sp(1.5))
        .max_w(rems(16.25))
        .overflow_hidden()
        .whitespace_nowrap()
        .children(text.split_whitespace().map(|word| {
            let color = if count(word, &['+']) {
                t.green
            } else if count(word, &['−', '-']) {
                t.red
            } else {
                quiet
            };
            mono(word.to_owned(), Type::SMALL, color).flex_shrink_0()
        }))
}

/// How a call stands, at the end of its line, when its plugin says
/// nothing else there.
fn state_label(card: &ToolCard, quiet: Hsla, t: &Theme) -> Div {
    match &card.state {
        ToolState::Running => label("running".into(), t.roles.live),
        ToolState::Done { summary } => {
            result_label(summary.as_deref().unwrap_or_default(), quiet, t)
        }
        ToolState::Failed(error) => label(error.clone(), t.red),
        ToolState::Blocked { plugin, .. } => div()
            .flex()
            .items_baseline()
            .flex_shrink_0()
            .gap(sp(1.5))
            .child(
                div()
                    .typeset(Type::SMALL.display())
                    .text_color(t.red)
                    .child("止"),
            )
            .child(mono(format!("blocked by {plugin}"), Type::SMALL, t.red)),
        ToolState::Flagged { .. } => {
            label("ran · flagged for review".into(), t.roles.live)
        }
    }
}

/// Under a refused call's line: what the model proposed on a flat
/// dark-red band, then the reason it got back, with what the plugin
/// that refused it draws (its verdict) beside the reason.
fn blocked_body(
    card: &ToolCard,
    reason: &str,
    verdict: Vec<AnyElement>,
    t: &Theme,
    compact: bool,
) -> Div {
    let proposed = proposed_text(card.args()).join("\n");
    let lang = card
        .args()
        .get("path")
        .and_then(serde_json::Value::as_str)
        .and_then(Lang::of_path);
    div()
        .flex()
        .flex_col()
        .when(!proposed.is_empty(), |body| {
            body.child(offending(&proposed, lang, t))
        })
        .child(
            div()
                .flex()
                .when(compact, |row| row.flex_col())
                .when(!compact, |row| row.items_start())
                .gap(sp(if compact { 3. } else { 7. }))
                .pt(sp(3.))
                .child(
                    div()
                        .flex_1()
                        .min_w(rems(0.))
                        .flex()
                        .flex_col()
                        .gap(sp(0.5))
                        .typeset(Type::LEAD.leading(1.6))
                        .child(
                            div()
                                .text_color(t.text)
                                .child(rich(reason, t.text, t)),
                        )
                        .child(
                            div()
                                .text_color(t.dim)
                                .child("The reason went back to the model."),
                        ),
                )
                .when(!verdict.is_empty(), |row| {
                    row.child(
                        div()
                            .flex_shrink_0()
                            .when(!compact, |column| column.w(rems(13.125)))
                            .flex()
                            .flex_col()
                            .gap(sp(1.5))
                            .children(verdict),
                    )
                }),
        )
}

/// The code a refused call proposed, each line marked as an addition,
/// on a flat dark-red band. Lines do not wrap.
fn offending(text: &str, lang: Option<Lang>, t: &Theme) -> Div {
    let marks = vec!["+"; text.lines().count().max(1)].join("\n");
    let parts = lang
        .filter(|_| !text.is_empty())
        .map(|lang| heiko_ui_kit::syntax::highlight_cached(lang, text));
    let code = heiko_ui_kit::syntax::styled(
        text,
        parts.as_deref().map(Vec::as_slice),
        t.removed_text,
        &t.syntax,
    );
    div()
        .flex()
        .gap(sp(2.))
        .mt(sp(2.))
        .px(sp(3.5))
        .py(sp(2.25))
        .bg(t.danger_surface)
        .typeset(Type::SMALL.mono().leading(1.55))
        .text_color(t.removed_text)
        .child(div().flex_shrink_0().child(marks))
        .child(
            div()
                .flex_1()
                .min_w(rems(0.))
                .whitespace_nowrap()
                .overflow_x_hidden()
                .child(code),
        )
}

/// A plugin's note: a line of the transcript with no box. What it says,
/// then the plugin's name and its detail in small print, and the way to
/// the plugin's page. One that warns stands under a rule in its tone.
fn plugin_note(
    ws: &Workspace,
    run: &RunView,
    index: usize,
    note: &PluginNote,
    t: &Theme,
    compact: bool,
    cx: &mut Context<Workspace>,
) -> Stateful<Div> {
    let warns = matches!(note.tone, Tone::Danger | Tone::Warn);
    // A plugin speaks in its own color; a note that warns, in its tone.
    let ink = match note.tone {
        Tone::Danger | Tone::Warn | Tone::Good => t.tone(note.tone),
        _ => t.roles.plugin(&note.plugin).unwrap_or(t.dim),
    };
    let said = div()
        .flex_1()
        .min_w(rems(0.))
        .typeset(if warns {
            Type::LEAD.leading(1.6)
        } else {
            Type::SMALL.leading(1.5)
        })
        .child(rich(
            &note.text,
            if warns { t.text } else { t.text_soft },
            t,
        ));
    let by = div()
        .flex()
        .items_baseline()
        .flex_shrink_0()
        .gap(sp(2.5))
        .child(mono(note.plugin.clone(), Type::CAPTION, ink))
        .when_some(note.detail.clone(), |row, detail| {
            row.child(mono(detail, Type::CAPTION, t.dim))
        })
        .when_some(
            ws.plugin_route_named(&note.plugin, &run.id),
            |row, route| {
                row.child(
                    div()
                        .id(SharedString::from(format!("details-{index}")))
                        .typeset(Type::SMALL)
                        .child(link_action("details", t))
                        .on_click(cx.listener(move |ws, _, _, cx| {
                            cx.stop_propagation();
                            ws.navigate(route.clone(), cx)
                        })),
                )
            },
        );
    div()
        .id(SharedString::from(format!("note-{index}")))
        .flex()
        .when(compact, |note| note.flex_col())
        .when(!compact, |note| note.items_baseline())
        .gap(sp(if compact { 1.5 } else { 4. }))
        .when(warns, |note| {
            note.pt(sp(2.5)).border_t_2().border_color(ink)
        })
        .child(said)
        .child(by)
}

/// A rewrite no plugin draws, on a hairline across the transcript: who
/// rewrote the context, and what it saved when the run saw it. An idle
/// chat's compaction says so, as Claude Code does.
fn rewrite(
    plugin: &str,
    tokens_seen: Option<(u64, u64)>,
    idle: bool,
    t: &Theme,
    compact: bool,
) -> Div {
    let line = || div().flex_1().child(super::rule(t));
    let did = if idle {
        "compacted while idle, before the prompt cache expired"
    } else {
        "rewrote the context"
    };
    let what = match tokens_seen {
        Some((before, after)) => {
            format!("{did}: {} to {}", tokens(before), tokens(after))
        }
        None => did.to_owned(),
    };
    div()
        .flex()
        .items_center()
        .gap(sp(3.))
        .child(line())
        .when(!compact, |row| {
            row.child(mono(plugin.to_owned(), Type::CAPTION, t.blue))
        })
        .child(mono(what, Type::CAPTION, t.dim))
        .child(line())
}

#[cfg(test)]
mod attachment_tests {
    use super::split_attachments;

    #[test]
    fn attached_files_are_named_not_shown() {
        let text = "use them\n\n<attached file=\"a.md\">\nA\n</attached>\n\n<attached file=\"b.rs\">\nB\n</attached>";
        assert_eq!(split_attachments(text), ("use them", vec!["a.md", "b.rs"]));
        assert_eq!(split_attachments("plain"), ("plain", vec![]));
    }
}
