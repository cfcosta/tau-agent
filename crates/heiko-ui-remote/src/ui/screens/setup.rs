//! Onboarding, flat and printed: under a bar with heiko's wordmark, the
//! stages as numbered squares (heiko's three, and plugins' steps around
//! the repositories) and where you are, each screen is a large title, a
//! short paragraph and ruled rows, with one primary block. Beside it a
//! diagram drawn from squares and 1 px lines: heiko as the offset-print
//! block, and the service a step connects to as an outlined square, the
//! line between them dashed until they are connected. A very large, very
//! faint 行 bleeds off the bottom right corner. On a phone the same
//! screens stack, without the 行.
//!
//! Nothing loops. Each screen's words rise into place when the screen
//! or its state changes (see [`crate::motion`]); reduced, they fade.

use gpui::{
    AnimationExt as _,
    AnyElement,
    App,
    Context,
    Div,
    Hsla,
    SharedString,
    Window,
    div,
    prelude::*,
    rems,
};
use heiko_ui_plugin::points::{LinkState, SetupAt, SetupView};

use crate::{
    assets::Icon,
    motion::{Frame, Link, Scene, SetupMotion, entrance},
    setup::{CloneState, SetupStep, Stage},
    theme::{Design as _, IconSize, Theme, Type, sp, weight},
    ui::{
        BlockSize,
        Material as _,
        SquareStyle,
        State,
        checkbox,
        icon,
        icon_button,
        link_action,
        logo,
        mark,
        mono,
        primary_block,
        section_header,
        square,
        ticks,
        wordmark,
    },
    workspace::Workspace,
};

/// The widest a screen's column of words gets beside its diagram, in
/// rems.
const WIDTH: f32 = 40.;

/// The widest a plugin's step gets when it has no diagram, in rems: two
/// columns of choices.
const STEP_WIDTH: f32 = 70.;

/// How many ticks a clone's progress is drawn with.
const CLONE_TICKS: usize = 24;

// What the frame shows.

/// The step and its state, and how its link stands, as onboarding
/// stands.
fn frame(ws: &Workspace, step: SetupStep) -> Frame {
    let (state, link) = match step {
        SetupStep::Welcome => (0, Link::Idle),
        SetupStep::Repos | SetupStep::Ready => (0, Link::Connected),
        SetupStep::Plugin { .. } => {
            match plugin_view(ws, step).and_then(|view| view.link) {
                // A step that connects heiko to a service: the provider's
                // sign-in, as it stands.
                Some(link) => match link.state {
                    LinkState::Idle => (0, Link::Idle),
                    LinkState::Waiting => (1, Link::Waiting),
                    LinkState::Connected => (2, Link::Connected),
                    LinkState::Declined => (3, Link::Declined),
                    LinkState::Blocked => (4, Link::Blocked),
                    LinkState::Failed => (5, Link::Idle),
                },
                None => (0, Link::Connected),
            }
        }
    };
    Frame {
        scene: Scene { step, state },
        link,
    }
}

/// What the plugin draws for `step`, one of its own, while it has
/// something to ask.
fn plugin_view(ws: &Workspace, step: SetupStep) -> Option<SetupView> {
    ws.plugin_setup_steps()
        .into_iter()
        .find_map(|(found, view)| (found == step).then_some(view))
}

/// Records what this frame of onboarding shows, before it is drawn, so
/// that what changed animates.
pub fn observe(ws: &mut Workspace, step: SetupStep, _: &Window, cx: &mut App) {
    let frame = frame(ws, step);
    let reduce = ws.reduce_motion() || cx.reduce_motion();
    let motion = &mut ws.setup_motion;
    motion.observe(frame);
    motion.reduce = reduce;
}

pub fn render(
    ws: &Workspace,
    step: SetupStep,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let stages = ws.setup_stages();
    let view = plugin_view(ws, step);
    let picture = picture(ws, step, view.as_ref(), compact, t);
    let body = match step {
        SetupStep::Welcome => welcome(ws, compact, t, cx),
        SetupStep::Repos => repos(ws, compact, t, cx),
        SetupStep::Plugin { plugin, step } => {
            plugin_step(ws, plugin, step, view.as_ref(), compact, t, cx)
        }
        SetupStep::Ready => ready(ws, compact, t, cx),
    };
    // A plugin's step with no diagram takes the width its choices need.
    let width = match (step, &picture) {
        (SetupStep::Plugin { .. }, None) => STEP_WIDTH,
        _ => WIDTH,
    };
    let page = if compact {
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(sp(7.))
            .px(sp(4.5))
            .pt(sp(7.))
            .pb(sp(8.))
            .children(picture.map(|picture| div().flex().child(picture)))
            .child(body)
    } else {
        // The welcome's diagram starts level with its title; a link's
        // sits beside the rows under it.
        let down = match step {
            SetupStep::Welcome => 0.,
            _ => 29.,
        };
        div()
            .w_full()
            .flex()
            .items_start()
            .gap(sp(10.))
            .pl(sp(30.))
            .pr(sp(14.))
            .pt(sp(24.))
            .pb(sp(12.))
            .child(body.w(rems(width)).min_w(rems(0.)))
            .children(picture.map(|picture| {
                div()
                    .flex_1()
                    .min_w(rems(0.))
                    .flex()
                    .justify_center()
                    .pt(sp(down))
                    .child(picture)
            }))
    };
    div()
        .size_full()
        .relative()
        .overflow_hidden()
        .flex()
        .flex_col()
        .bg(t.bg)
        .text_color(t.text)
        .typeset(Type::BODY)
        .when(!compact, |screen| screen.child(flourish(t)))
        .child(top_bar(ws, step, &stages, compact, t, cx))
        .child(
            div()
                .id("setup-body")
                .flex_1()
                .min_h(rems(0.))
                .overflow_y_scroll()
                .child(page),
        )
        .into_any_element()
}

// The frame.

/// The one flourish: a very large, very faint 行 in the display face,
/// bleeding off the bottom right corner.
fn flourish(t: &Theme) -> Div {
    div()
        .absolute()
        .right(rems(-7.5))
        .bottom(rems(-14.375))
        .typeset(Type::SECTION.sized(620.).leading(1.))
        .text_color(t.blue.opacity(0.06))
        .child("行")
}

/// A stage's square in the top bar: ticked in green once done, filled
/// blue over the offset while current, quiet while ahead.
fn stage_square(n: usize, at: Option<usize>, t: &Theme) -> Div {
    let number = (n + 1).to_string();
    match at {
        Some(at) if n < at => div()
            .size(rems(1.5))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .border_1()
            .border_color(t.green)
            .child(mark(State::Met, t)),
        Some(at) if n == at => {
            square(number, SquareStyle::Filled(t.roles.primary), t)
                .printed(3., t)
        }
        _ => square(number, SquareStyle::Quiet, t),
    }
}

/// heiko's wordmark, the stages as numbered squares joined
/// by hairlines, and where you are, over a heavy rule. A phone shows the
/// squares without their names.
fn top_bar(
    ws: &Workspace,
    step: SetupStep,
    stages: &[Stage],
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let at = step.stage(stages);
    let mut items: Vec<AnyElement> = Vec::new();
    for (n, stage) in stages.iter().enumerate() {
        if n > 0 {
            items.push(
                div()
                    .w(rems(if compact { 0.75 } else { 2.125 }))
                    .h(rems(0.0625))
                    .flex_shrink_0()
                    .bg(t.slate)
                    .into_any_element(),
            );
        }
        let current = at == Some(n);
        let ink = match at {
            Some(at) if n < at => t.text,
            Some(at) if n == at => t.bright,
            _ => t.dim,
        };
        items.push(
            div()
                .flex()
                .items_center()
                .flex_shrink_0()
                .gap(sp(2.25))
                .child(stage_square(n, at, t))
                .when(!compact, |item| {
                    item.child(
                        div()
                            .text_color(ink)
                            .when(current, |name| {
                                name.font_weight(weight::STRONG)
                            })
                            .child(stage.name.clone()),
                    )
                })
                .into_any_element(),
        );
    }
    let place = match at {
        None => "welcome".to_owned(),
        Some(stage) if compact => {
            format!("{} · {}/{}", stages[stage].name, stage + 1, stages.len())
        }
        Some(stage) => format!("{} / {}", stage + 1, stages.len()),
    };
    div()
        .flex_shrink_0()
        .mx(sp(if compact { 4.5 } else { 14. }))
        .pt(sp(if compact { 3. } else { 8.5 }))
        .pb(sp(4.))
        .flex()
        .items_center()
        .gap(sp(if compact { 2.5 } else { 3.5 }))
        .border_b_2()
        .border_color(t.rule)
        .when(ws.can_go_back() && compact, |bar| {
            bar.child(
                icon_button("task-back", Icon::Back, 36., t)
                    .on_click(cx.listener(|ws, _, _, cx| ws.back(cx))),
            )
        })
        .child(if compact {
            logo(t, 22.)
        } else {
            wordmark(24., t)
        })
        .child(div().flex_1())
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(if compact { 1.5 } else { 3.5 }))
                .children(items),
        )
        .child(div().flex_1())
        .child(
            div()
                .when(!compact, |place| place.min_w(rems(9.375)))
                .flex()
                .justify_end()
                .child(mono(place, Type::CAPTION, t.dim)),
        )
}

// The diagrams.

/// heiko in a diagram: 並 on the offset-print block, `side` px wide.
fn heiko_block(side: f32, t: &Theme) -> Div {
    div()
        .size(rems(side / 16.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .typeset(Type::SECTION.sized(side * 0.46).leading(1.))
        .printed(side * 0.06, t)
        .child("並")
}

/// A service in a diagram: an outlined square `side` px wide, dashed
/// while heiko is not connected to it, holding `label` when it has one.
fn service_square(
    side: f32,
    label: Option<&'static str>,
    dashed: bool,
    t: &Theme,
) -> Div {
    div()
        .size(rems(side / 16.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(t.bg)
        .border_1()
        .border_color(t.rule)
        .when(dashed, |square| square.border_dashed())
        .children(label.map(|label| mono(label, Type::LEAD, t.rule)))
}

/// A 1 px line across, in `color`; dashed while not connected.
fn wire(color: Hsla, dashed: bool) -> Div {
    if dashed {
        // A line with no height draws no border.
        div()
            .h(rems(0.0625))
            .border_t_1()
            .border_dashed()
            .border_color(color)
    } else {
        div().h(rems(0.0625)).bg(color)
    }
}

/// The welcome's diagram: heiko in the middle, and what it connects to
/// around it, numbered as the steps are.
fn welcome_picture(t: &Theme) -> Div {
    const BLOCK: f32 = 100.;
    const SIDE: f32 = 58.;
    let at = |x: f32, y: f32| {
        div().absolute().left(rems(x / 16.)).top(rems(y / 16.))
    };
    let line = |x: f32, y: f32, w: f32, h: f32| {
        at(x, y).w(rems(w / 16.)).h(rems(h / 16.)).bg(t.slate)
    };
    // A name centered under `x`.
    let name = |x: f32, y: f32, text: &'static str, color: Hsla| {
        at(x - 60., y)
            .w(rems(7.5))
            .flex()
            .justify_center()
            .child(mono(text, Type::CAPTION, color))
    };
    div()
        .relative()
        .flex_shrink_0()
        .w(rems(26.25))
        .h(rems(18.75))
        .child(line(210., SIDE, 1., 82.))
        .child(line(SIDE, 190., 102., 1.))
        .child(line(160. + BLOCK, 190., 102., 1.))
        .child(at(160., 140.).child(heiko_block(BLOCK, t)))
        .child(at(181., 0.).child(service_square(SIDE, Some("3"), false, t)))
        .child(at(0., 161.).child(service_square(SIDE, Some("2"), false, t)))
        .child(at(362., 161.).child(service_square(SIDE, Some("1"), false, t)))
        .child(at(251., 21.).child(mono("repositories", Type::CAPTION, t.dim)))
        .child(name(29., 229., "GitHub", t.dim))
        .child(name(391., 229., "your plan", t.dim))
        .child(name(210., 256., "heiko", t.text))
}

/// The words over and under the line.
type Words = (Option<SharedString>, Option<SharedString>);

/// A step's diagram: heiko, the line and the service it connects to,
/// named under them. `link` is how the connection stands, when the step
/// says: the line is dashed until connected, in the state's color, with
/// the state's mark on it. With none, a plain line.
fn link_picture(
    link: Option<Link>,
    words: Words,
    service: impl Into<SharedString>,
    compact: bool,
    t: &Theme,
) -> Div {
    let (block, span) = if compact { (56., 96.) } else { (100., 220.) };
    let (color, dashed, state, word_color) = match link {
        None => (t.slate, false, None, t.dim),
        Some(Link::Idle) => (t.blue, true, None, t.roles.live),
        Some(Link::Waiting) => (t.blue, true, Some(State::Working), t.blue),
        Some(Link::Connected) => (t.green, false, Some(State::Met), t.green),
        Some(Link::Declined) => {
            (t.roles.live, true, Some(State::Asks), t.roles.live)
        }
        Some(Link::Blocked) => (t.red, true, Some(State::Failing), t.red),
    };
    let word = |text: Option<SharedString>, color: Hsla| {
        div()
            .h(rems(1.25))
            .flex()
            .items_center()
            .justify_center()
            .children(text.map(|text| mono(text, Type::CAPTION, color)))
    };
    // The line, on the middle of a mark's height, and the mark over it.
    let line = div()
        .relative()
        .w_full()
        .h(rems(0.875))
        .flex()
        .items_center()
        .justify_center()
        .child(
            wire(color, dashed)
                .absolute()
                .left_0()
                .right_0()
                .top(rems(0.40625)),
        )
        .children(
            state.map(|state| div().px(sp(1.)).bg(t.bg).child(mark(state, t))),
        );
    let name = |text: SharedString, color: Hsla| {
        div()
            .w(rems(block / 16.))
            .flex()
            .justify_center()
            .child(mono(text, Type::CAPTION, color))
    };
    div()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .gap(sp(4.))
        .child(
            div()
                .flex()
                .items_center()
                .child(heiko_block(block, t))
                .child(
                    // Clear of the block's offset.
                    div()
                        .ml(sp(1.5))
                        .w(rems(span / 16.))
                        .flex_shrink_0()
                        .flex()
                        .flex_col()
                        .gap(sp(1.5))
                        .child(word(words.0, t.dim))
                        .child(line)
                        .child(word(words.1, word_color)),
                )
                .child(service_square(
                    block,
                    None,
                    link.is_some_and(|link| link != Link::Connected),
                    t,
                )),
        )
        .child(
            div()
                .flex()
                .justify_between()
                .child(name("heiko".into(), t.text))
                .child(name(service.into(), t.text)),
        )
}

/// The diagram beside `step`: the welcome's on a desktop, and the link
/// of a plugin's step that connects heiko to a service or comes before
/// the repositories. `view` is what the plugin draws for the step.
fn picture(
    ws: &Workspace,
    step: SetupStep,
    view: Option<&SetupView>,
    compact: bool,
    t: &Theme,
) -> Option<Div> {
    match step {
        SetupStep::Welcome => (!compact).then(|| welcome_picture(t)),
        SetupStep::Repos | SetupStep::Ready => None,
        SetupStep::Plugin { .. } => {
            let view = view?;
            match &view.link {
                // The provider's sign-in.
                Some(link) => Some(link_picture(
                    Some(ws.setup_motion.link.current),
                    (
                        link.above.clone().map(SharedString::from),
                        link.below.clone().map(SharedString::from),
                    ),
                    "ChatGPT",
                    compact,
                    t,
                )),
                None if view.at != SetupAt::AfterRepositories => {
                    Some(link_picture(
                        None,
                        (None, None),
                        view.title.clone(),
                        compact,
                        t,
                    ))
                }
                None => None,
            }
        }
    }
}

// Entrances.

/// `element`, rising 12 px into place and fading in, `delay` ms after
/// its screen appears; reduced, a plain fade.
fn rise(
    motion: &SetupMotion,
    key: &str,
    delay: u64,
    element: impl IntoElement,
) -> AnyElement {
    let reduce = motion.reduce;
    div()
        .w_full()
        .child(element)
        .with_animation(
            SharedString::from(format!("rise-{}-{key}", motion.scene.epoch)),
            entrance(delay, reduce),
            move |slot, e| {
                let slot = slot.opacity(e.clamp(0., 1.));
                if reduce {
                    slot
                } else {
                    slot.relative().top(rems((12. * (1. - e)) / 16.))
                }
            },
        )
        .into_any_element()
}

// Words.

/// The title in the display face and the paragraph under it, rising in
/// turn.
fn headline(
    motion: &SetupMotion,
    (title, sub): (&str, &str),
    compact: bool,
    t: &Theme,
) -> Div {
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(sp(4.5))
        .child(rise(
            motion,
            "title",
            0,
            div()
                .typeset(Type::SECTION.sized(if compact { 28. } else { 48. }))
                .text_color(t.bright)
                .child(title.to_owned()),
        ))
        .child(rise(
            motion,
            "sub",
            40,
            div()
                .typeset(
                    (if compact { Type::BODY } else { Type::READING })
                        .leading(1.55),
                )
                .text_color(t.text_soft)
                .child(sub.to_owned()),
        ))
}

fn footnote(text: impl Into<SharedString>, t: &Theme) -> Div {
    div()
        .typeset(Type::SMALL)
        .text_color(t.dim)
        .child(text.into())
}

/// A row of actions, or a column of them on a phone.
fn actions(compact: bool) -> Div {
    div()
        .flex()
        .items_center()
        .gap(sp(6.))
        .when(compact, |row| row.w_full().flex_col().gap(sp(4.)))
        .when(!compact, |row| row.flex_wrap())
}

/// The screen's main action, as wide as the phone.
fn main_action(
    id: &'static str,
    label: impl Into<SharedString>,
    size: BlockSize,
    compact: bool,
    t: &Theme,
) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_col()
        .when(compact, |action| action.w_full())
        .child(
            primary_block(label, size, t)
                // A screen of onboarding has one thing to do: its block
                // is the largest heiko prints.
                .when(size == BlockSize::Large && !compact, |block| {
                    block.h(rems(2.875)).px(sp(5.5))
                }),
        )
}

/// A ruled, numbered row: what a step is called, and what it is for.
fn step_row(
    n: usize,
    name: &'static str,
    detail: &'static str,
    compact: bool,
    t: &Theme,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap(sp(3.5))
        .py(sp(3.))
        .border_b_1()
        .border_color(t.hairline)
        .child(square((n + 1).to_string(), SquareStyle::Quiet, t))
        .child(
            div()
                .flex_1()
                .min_w(rems(0.))
                .flex()
                .gap(sp(3.))
                .when(compact, |words| words.flex_col().gap(sp(0.5)))
                .child(
                    div()
                        .when(!compact, |name| name.w(rems(11.25)))
                        .flex_shrink_0()
                        .text_color(t.bright)
                        .font_weight(weight::STRONG)
                        .child(name),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(rems(0.))
                        .text_color(t.text_soft)
                        .child(detail),
                ),
        )
}

// The screens.

fn welcome(
    ws: &Workspace,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let motion = &ws.setup_motion;
    let steps = [
        ("Connect ChatGPT", "Runs use your ChatGPT plan."),
        (
            "Sign in with GitHub",
            "Clone what you pick, open pull requests.",
        ),
        (
            "Pick repositories",
            "Cloned into heiko's storage, versioned with jj.",
        ),
    ];
    let rows = steps
        .into_iter()
        .enumerate()
        .map(|(n, (name, detail))| step_row(n, name, detail, compact, t));
    div()
        .flex()
        .flex_col()
        .child(headline(
            motion,
            (
                "Coding agents on your repositories",
                "heiko runs agents on the repositories you choose, on your \
                 ChatGPT plan. Every turn is a change you can go back to, \
                 fork or compare.",
            ),
            compact,
            t,
        ))
        .child(rise(
            motion,
            "steps",
            80,
            div()
                .mt(sp(if compact { 8. } else { 11. }))
                .flex()
                .flex_col()
                .child(section_header(
                    "Three steps",
                    Some("takes about a minute".into()),
                    t,
                ))
                .children(rows),
        ))
        .child(rise(
            motion,
            "actions",
            120,
            actions(compact).mt(sp(9.)).child(
                main_action(
                    "get-started",
                    "Continue with ChatGPT →",
                    BlockSize::Large,
                    compact,
                    t,
                )
                .on_click(cx.listener(|ws, _, _, cx| ws.start_setup_steps(cx))),
            ),
        ))
        .child(rise(
            motion,
            "note",
            160,
            footnote(
                format!(
                    "Tokens stay in {}, readable only by you",
                    ws.setup.config
                ),
                t,
            )
            .mt(sp(7.)),
        ))
}

fn repos(
    ws: &Workspace,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let motion = &ws.setup_motion;
    let filter = ws.repo_filter.read(cx).text().to_owned();
    // The most recently pushed first, twenty of them until expanded.
    let (shown, hidden) = ws.setup.shown(&filter, ws.repos_expanded);
    let rows: Vec<_> = shown
        .into_iter()
        .enumerate()
        .map(|(n, repo)| {
            let name = repo.name.clone();
            div()
                .id(("repo", n))
                .w_full()
                .flex()
                .items_center()
                .gap(sp(3.5))
                .py(sp(2.75))
                .border_b_1()
                .border_color(t.hairline)
                .cursor_pointer()
                .child(checkbox(repo.selected, compact, t))
                .child(
                    div()
                        .flex_1()
                        .min_w(rems(0.))
                        .flex()
                        .flex_col()
                        .gap(sp(0.5))
                        .child(mono(
                            repo.name.clone(),
                            Type::SMALL,
                            if repo.selected { t.bright } else { t.text },
                        ))
                        .when(!repo.description.is_empty(), |col| {
                            col.child(
                                div()
                                    .typeset(Type::CAPTION)
                                    .text_color(t.dim)
                                    .child(repo.description.clone()),
                            )
                        }),
                )
                .child(mono(repo.branch.clone(), Type::CAPTION, t.dim))
                .on_click(
                    cx.listener(move |ws, _, _, cx| ws.toggle_repo(&name, cx)),
                )
        })
        .collect();
    let count = ws.setup.selected().count();
    let list = div()
        .w_full()
        .flex()
        .flex_col()
        .child(section_header(
            "Repositories",
            Some(format!("{count} of {} picked", ws.setup.repos.len()).into()),
            t,
        ))
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(2.5))
                .h(rems(2.75))
                .border_b_1()
                .border_color(t.hairline)
                .child(icon(Icon::Search, IconSize::MEDIUM, t.dim))
                .child(
                    div()
                        .flex_1()
                        .min_w(rems(0.))
                        .child(ws.repo_filter.clone()),
                ),
        )
        .children(rows)
        .when(hidden > 0 || ws.repos_expanded, |list| {
            let label = if ws.repos_expanded {
                "Show fewer".to_owned()
            } else if hidden == 1 {
                "Show 1 more".to_owned()
            } else {
                format!("Show {hidden} more")
            };
            list.child(
                div().flex().py(sp(2.75)).child(
                    div()
                        .id("repos-more")
                        .child(link_action(label, t))
                        .on_click(cx.listener(|ws, _, _, cx| {
                            ws.toggle_repos_expanded(cx)
                        })),
                ),
            )
        });
    div()
        .flex()
        .flex_col()
        .child(headline(
            motion,
            (
                "Pick repositories",
                "heiko clones each into its own storage, as a jj \
                 repository. Runs work there, never in your checkouts.",
            ),
            compact,
            t,
        ))
        .child(rise(
            motion,
            "list",
            80,
            list.mt(sp(if compact { 8. } else { 11. })),
        ))
        .child(rise(
            motion,
            "actions",
            120,
            actions(compact).mt(sp(9.)).child(
                main_action(
                    "clone",
                    match count {
                        0 => "Pick a repository".to_owned(),
                        1 => "Clone 1 repository".to_owned(),
                        n => format!("Clone {n} repositories"),
                    },
                    BlockSize::Large,
                    compact,
                    t,
                )
                .when(count == 0, |button| button.opacity(0.5))
                .on_click(cx.listener(move |ws, _, _, cx| {
                    if count > 0 {
                        ws.clone_selected(cx)
                    }
                })),
            ),
        ))
        .child(
            footnote(format!("Cloned into {}", ws.setup.storage), t).mt(sp(7.)),
        )
}

/// A plugin's step: its headline and what it draws under it, and a way
/// past it, which heiko draws.
fn plugin_step(
    ws: &Workspace,
    plugin: &'static str,
    step: &'static str,
    view: Option<&SetupView>,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let motion = &ws.setup_motion;
    let skip = div().flex().child(
        div()
            .id("setup-skip")
            .debug_selector(|| "setup-skip".to_owned())
            .child(link_action(
                if view.is_some() {
                    "Skip for now"
                } else {
                    "Continue"
                },
                t,
            ))
            .on_click(cx.listener(move |ws, _, _, cx| {
                ws.plugin_setup_step_done(plugin, cx)
            })),
    );
    let Some(view) = view else {
        // The step has nothing to ask any more: only the way on.
        return div().flex().flex_col().child(skip);
    };
    let reads = crate::plugins::registry()
        .get(plugin)
        .map_or(&[][..], |erased| erased.reads());
    // What the plugin says it reads outside repositories.
    let reach = (!reads.is_empty()).then(|| {
        div()
            .debug_selector(|| "setup-reach".to_owned())
            .w_full()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(sp(1.5))
            .typeset(Type::CAPTION)
            .text_color(t.dim)
            .child("Reads outside your repositories:")
            .children(
                reads
                    .iter()
                    .map(|path| mono(path.clone(), Type::CAPTION, t.text_soft)),
            )
    });
    div()
        .flex()
        .flex_col()
        .child(headline(motion, (&view.heading, &view.detail), compact, t))
        .child(
            div()
                .mt(sp(if compact { 7. } else { 9. }))
                .w_full()
                .flex()
                .flex_col()
                .gap(sp(6.))
                .child(rise(
                    motion,
                    &format!("{plugin}-{step}"),
                    80,
                    ws.plugin_tree(
                        plugin,
                        &format!("{plugin}-setup-{step}"),
                        &view.body,
                        crate::plugins::Origin::default(),
                        cx,
                    ),
                ))
                .children(reach.map(|reach| rise(motion, "reach", 120, reach)))
                .when(!view.required, |body| {
                    body.child(rise(motion, "skip", 160, skip))
                }),
        )
}

fn ready(
    ws: &Workspace,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let motion = &ws.setup_motion;
    // A clone's row: its name and how it stands, over its progress as
    // ticks.
    let clones = ws.setup.clones.iter().map(|clone| {
        let (state, color, lit) = match &clone.state {
            CloneState::Cloning { share, detail } => {
                let share = share.clamp(0., 1.);
                let lit = (share * CLONE_TICKS as f32).round() as usize;
                (
                    if detail.is_empty() {
                        format!("cloning · {:.0}%", share * 100.)
                    } else {
                        format!("cloning · {detail}")
                    },
                    t.blue,
                    // Any share above none lights a tick.
                    if share > 0. { lit.max(1) } else { 0 },
                )
            }
            CloneState::Ready => ("ready".to_owned(), t.green, CLONE_TICKS),
            CloneState::Failed(error) => (error.clone(), t.red, CLONE_TICKS),
        };
        div()
            .flex()
            .flex_col()
            .gap(sp(2.))
            .py(sp(2.75))
            .border_b_1()
            .border_color(t.hairline)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(sp(2.5))
                    .child(
                        mono(clone.name.clone(), Type::SMALL, t.text)
                            .min_w(rems(0.)),
                    )
                    .child(mono(state, Type::CAPTION, color).flex_shrink_0()),
            )
            .child(ticks(lit, CLONE_TICKS, color, t))
    });
    let repo = ws
        .setup
        .clones
        .first()
        .map(|clone| clone.name.clone())
        .unwrap_or_else(|| ws.name.clone());
    let models = &ws.catalog().models;
    let model = models
        .access
        .signed_in
        .then(|| models.default_for("coder").model);
    let on_plan = model.is_some();
    let composer = div()
        .w_full()
        .flex()
        .flex_col()
        .well(t)
        .child(
            div()
                .h(rems(if compact { 5.5 } else { 6.75 }))
                .px(sp(4.))
                .py(sp(3.5))
                .typeset(Type::SUBTITLE.leading(1.55))
                .child(ws.first_task.clone()),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(sp(3.))
                .px(sp(4.))
                .pb(sp(3.5))
                .child(mono(repo, Type::CAPTION, t.text_soft))
                .child(mono(
                    model.unwrap_or_else(|| "no model yet".to_owned()),
                    Type::CAPTION,
                    t.text_soft,
                ))
                .when(on_plan, |row| {
                    // What runs use, as the provider says it.
                    row.child(
                        div().typeset(Type::CAPTION).text_color(t.green).child(
                            format!(
                                "on your {}",
                                ws.catalog().models.access.label
                            ),
                        ),
                    )
                })
                .child(div().flex_1())
                .child(
                    main_action(
                        "first-run",
                        "Start run",
                        BlockSize::Medium,
                        compact,
                        t,
                    )
                    .on_click(
                        cx.listener(|ws, _, _, cx| ws.start_first_run(cx)),
                    ),
                ),
        );
    div()
        .flex()
        .flex_col()
        .child(headline(
            motion,
            (
                "Start your first run",
                "Describe a task. heiko works on a new change in the \
                 repository; open a pull request when you like the result.",
            ),
            compact,
            t,
        ))
        .when(!ws.setup.clones.is_empty(), |col| {
            col.child(rise(
                motion,
                "clones",
                80,
                div()
                    .mt(sp(if compact { 8. } else { 11. }))
                    .flex()
                    .flex_col()
                    .child(section_header("Cloning", None, t))
                    .children(clones),
            ))
        })
        .child(rise(
            motion,
            "composer",
            120,
            div().mt(sp(8.)).child(composer),
        ))
        .child(rise(
            motion,
            "note",
            160,
            footnote("⏎ to start", t).mt(sp(5.)),
        ))
}
