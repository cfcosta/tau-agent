//! Onboarding, in the handshake style: tau's tile and the service it
//! connects to, joined by a line whose look says how the connection
//! stands, under a bar with the four stages. Faint rings and a glow sit
//! behind the handshake, amber while acting, green once connected and
//! red when blocked. On a phone the same screens stack, with smaller
//! tiles and full-width buttons.

use std::time::Duration;

use gpui::{
    Animation,
    AnimationExt as _,
    AnyElement,
    BoxShadow,
    ClipboardItem,
    Context,
    Div,
    Hsla,
    SharedString,
    div,
    linear_color_stop,
    linear_gradient,
    prelude::*,
    pulsating_between,
    px,
    relative,
    svg,
};

use crate::{
    assets::{Brand, Icon},
    route::Route,
    setup::{CloneState, DeviceCode, GitHub, ModelAccess, SetupStep},
    theme::{Design as _, IconSize, Theme, Type, radius, sp, weight},
    ui::{bar, checkbox, icon, icon_button, mono, text_link},
    workspace::{Workspace, WorkspaceEvent},
};

/// Where ChatGPT explains plan use and who can share it.
const PLAN_HELP_URL: &str = "https://help.openai.com";

/// The waiting dot's halo; the dot is half as wide.
const HALO: f32 = 24.;

/// The top bar's height, the same on a desktop and a phone.
const TOP_BAR: f32 = 56.;

/// How a connection stands, as the line between the tiles draws it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Link {
    /// Not started: dashed amber.
    Idle,
    /// Waiting on the other side: dim dashes and a pulsing dot.
    Waiting,
    /// Done: solid green with a check.
    Connected,
    /// Signed in, but something was not allowed: broken amber and "!".
    Declined,
    /// Refused for good: broken red and a cross.
    Blocked,
}

/// The mood of a screen: the color of its rings and glow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mood {
    Acting,
    Done,
    Refused,
}

impl Mood {
    fn color(self, t: &Theme) -> Hsla {
        match self {
            Self::Acting => t.accent,
            Self::Done => t.green,
            Self::Refused => t.red,
        }
    }
}

/// How big the handshake is and where it sits: a big one for sign-ins,
/// a small one over a list.
#[derive(Debug, Clone, Copy)]
struct Size {
    tile: f32,
    line: f32,
    /// From the top bar to the tiles' top.
    top: f32,
}

impl Size {
    fn big(compact: bool) -> Self {
        if compact {
            Self {
                tile: 64.,
                line: 96.,
                top: 28.,
            }
        } else {
            Self {
                tile: 112.,
                line: 250.,
                top: 140.,
            }
        }
    }

    fn small(compact: bool) -> Self {
        if compact {
            Self {
                tile: 52.,
                line: 80.,
                top: 24.,
            }
        } else {
            Self {
                tile: 72.,
                line: 170.,
                top: 40.,
            }
        }
    }

    /// The handshake's middle, from the window's top.
    fn center(self) -> f32 {
        TOP_BAR + self.top + self.tile / 2.
    }
}

/// A screen: its mood, where its handshake is, and what it holds.
struct Screen {
    mood: Mood,
    /// Where the rings center, from the window's top.
    center: f32,
    body: Div,
}

pub fn render(
    ws: &Workspace,
    step: SetupStep,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let screen = match step {
        SetupStep::Welcome => welcome(ws, compact, t, cx),
        SetupStep::GitHub => github(ws, compact, t, cx),
        SetupStep::Token => token(ws, compact, t, cx),
        SetupStep::Model => model(ws, compact, t, cx),
        SetupStep::Repos => repos(ws, compact, t, cx),
        SetupStep::Ready => ready(ws, compact, t, cx),
    };
    div()
        .size_full()
        .relative()
        .overflow_hidden()
        .flex()
        .flex_col()
        .bg(t.setup.ground)
        .text_color(t.text)
        .typeset(Type::BODY)
        .child(backdrop(screen.mood, screen.center, compact, t))
        .child(top_bar(ws, step, compact, t, cx))
        .child(
            div()
                .id("setup-body")
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scroll()
                .child(
                    screen
                        .body
                        .w_full()
                        .flex()
                        .flex_col()
                        .items_center()
                        .px(sp(if compact { 4. } else { 6. }))
                        .pb(sp(if compact { 8. } else { 12. })),
                ),
        )
        .into_any_element()
}

// The scene.

/// The rings and the glow, centered on the handshake. Radial gradients
/// are not in GPUI: the glow is a soft shadow around a circle.
fn backdrop(mood: Mood, center: f32, compact: bool, t: &Theme) -> Div {
    let look = &t.setup;
    let color = mood.color(t);
    let scale = if compact { look.compact_scale } else { 1. };
    let rings =
        look.ring_radii
            .iter()
            .zip(look.ring_alphas)
            .map(|(radius, alpha)| {
                let r = radius * scale;
                div()
                    .absolute()
                    .left(px(-r))
                    .top(px(-r))
                    .size(px(2. * r))
                    .rounded(radius::FULL)
                    .border_1()
                    .border_color(color.opacity(alpha))
            });
    let glow = 200. * scale;
    let alpha = if mood == Mood::Refused {
        look.glow_alpha * 0.8
    } else {
        look.glow_alpha
    };
    div()
        .absolute()
        .top(px(center))
        .left(relative(0.5))
        .size(px(0.))
        .child(
            div()
                .absolute()
                .left(px(-glow / 2.))
                .top(px(-glow / 2.))
                .size(px(glow))
                // A real radius: shadows do not round with `FULL`.
                .rounded(px(glow / 2.))
                .shadow(vec![
                    BoxShadow::new(px(0.), px(0.), color.opacity(alpha))
                        .blur_radius(px(glow))
                        .spread_radius(px(glow * 0.35)),
                ]),
        )
        .children(rings)
}

/// tau, the four stages as segments (done green, current amber), and
/// where you are. A phone shows the segments without their names.
fn top_bar(
    ws: &Workspace,
    step: SetupStep,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let look = &t.setup;
    let stage = (step != SetupStep::Welcome).then(|| step.stage());
    // GitHub can be skipped: then it is not done.
    let signed_in = ws.setup.user().is_some();
    let segments = SetupStep::STAGES.iter().enumerate().map(|(n, name)| {
        let reached = stage.is_some_and(|stage| n <= stage);
        let done = stage.is_some_and(|stage| n < stage) && (n > 0 || signed_in);
        let current = stage == Some(n);
        let color = if done {
            t.green
        } else if current {
            t.accent
        } else {
            look.track
        };
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(sp(1.75))
            .child(
                div()
                    .w(px(if compact { 28. } else { 64. }))
                    .h(px(3.))
                    .rounded(radius::HAIRLINE)
                    .bg(color),
            )
            .when(!compact, |segment| {
                segment.child(mono(
                    *name,
                    Type::MICRO,
                    if reached { t.dim } else { look.idle },
                ))
            })
    });
    let place = match stage {
        None => "welcome".to_owned(),
        Some(stage) if compact => {
            format!("{} · {}/4", SetupStep::STAGES[stage], stage + 1)
        }
        Some(stage) => format!("{} / 4", stage + 1),
    };
    let side = if compact { 96. } else { 140. };
    div()
        .h(px(TOP_BAR))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_between()
        .px(sp(if compact { 3. } else { 7. }))
        .child(
            div()
                .w(px(side))
                .flex()
                .items_center()
                .gap(sp(2.5))
                .when(ws.can_go_back() && compact, |row| {
                    row.child(
                        icon_button("task-back", Icon::Back, 36., t)
                            .on_click(cx.listener(|ws, _, _, cx| ws.back(cx))),
                    )
                })
                .child(tau_tile(22., t))
                .when(!compact, |row| {
                    row.child(mono("tau · setup", Type::CAPTION, look.faint))
                }),
        )
        .child(
            div()
                .flex()
                .items_start()
                .gap(sp(if compact { 1.5 } else { 2.5 }))
                .when(!compact, |row| row.pt(sp(3.)))
                .children(segments),
        )
        .child(div().w(px(side)).flex().justify_end().child(mono(
            place,
            Type::CAPTION,
            look.faint,
        )))
}

// The handshake.

/// tau's tile: τ on amber, softly lit.
fn tau_tile(size: f32, t: &Theme) -> Div {
    let lit = size >= 48.;
    div()
        .size(px(size))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(size * 30. / 112.))
        .bg(t.accent)
        .text_color(t.setup.on_light)
        .typeset(Type::HERO.sized(size * 58. / 112.).weighted(weight::STRONG))
        .when(lit, |tile| {
            tile.shadow(vec![
                BoxShadow::new(px(0.), px(0.), t.accent.opacity(0.08))
                    .spread_radius(px((size / 11.).max(6.))),
                BoxShadow::new(px(0.), px(24.), t.accent.opacity(0.22))
                    .blur_radius(px(60.)),
            ])
        })
        .child("τ")
}

/// The other side's tile: dark, edged in `edge`.
fn service_tile(
    inner: impl IntoElement,
    size: f32,
    edge: Hsla,
    t: &Theme,
) -> Div {
    div()
        .size(px(size))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(size * 30. / 112.))
        .bg(t.setup.tile)
        .border_1()
        .border_color(edge)
        .child(inner)
}

/// A company's mark, from its own artwork when it was built in; else a
/// plain outlined circle holding its place. See [`Brand`].
fn brand_mark(brand: Brand, size: f32, color: Hsla, t: &Theme) -> AnyElement {
    match brand.svg() {
        Some(_) => svg()
            .path(brand.path())
            .size(px(size))
            .flex_shrink_0()
            .text_color(color)
            .into_any_element(),
        None => div()
            .size(px(size))
            .flex_shrink_0()
            .rounded(radius::FULL)
            .border(px(1.5))
            .border_color(t.setup.faint)
            .into_any_element(),
    }
}

/// The line between the tiles, with a word above it and one under it.
fn line(
    link: Link,
    size: Size,
    above: Option<&'static str>,
    below: Option<&'static str>,
    t: &Theme,
) -> Div {
    let look = &t.setup;
    let (color, below_color) = match link {
        Link::Idle => (t.accent, look.faint.opacity(0.9)),
        Link::Waiting | Link::Declined => (t.accent, t.accent),
        Link::Connected => (t.green, t.green),
        Link::Blocked => (t.red, t.red),
    };
    // Dashes 4 px long every 10 px, as CSS draws the artboard's.
    let dashes = |alpha: f32| {
        let count = (size.line / 10.).floor() as usize;
        div()
            .absolute()
            .left_0()
            .right_0()
            .flex()
            .gap(sp(1.5))
            .children((0..count).map(|_| {
                div()
                    .w(px(4.))
                    .h(px(2.))
                    .flex_shrink_0()
                    .bg(t.accent.opacity(alpha))
            }))
    };
    let medallion = |inner: AnyElement| {
        let side = if size.tile < 100. { 32. } else { 40. };
        div()
            .size(px(side))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(side / 2.))
            .bg(look.ground)
            .border(px(2.))
            .border_color(color)
            .child(inner)
    };
    let track = div()
        .w(px(size.line))
        .h(px(size.tile))
        .relative()
        .flex()
        .items_center()
        .justify_center();
    let track = match link {
        Link::Idle => track.child(dashes(0.55)),
        Link::Waiting => {
            let half = |from: Hsla, to: Hsla| {
                div().w(relative(0.5)).h(px(2.)).bg(linear_gradient(
                    90.,
                    linear_color_stop(from, 0.),
                    linear_color_stop(to, 1.),
                ))
            };
            let clear = t.accent.opacity(0.);
            track
                .child(dashes(0.35))
                .child(
                    div()
                        .absolute()
                        .top(px(size.tile / 2. - 1.))
                        .left(relative(0.3))
                        .w(relative(0.4))
                        .h(px(2.))
                        .flex()
                        .shadow(vec![
                            BoxShadow::new(
                                px(0.),
                                px(0.),
                                t.accent.opacity(0.5),
                            )
                            .blur_radius(px(14.)),
                        ])
                        .child(half(clear, t.accent))
                        .child(half(t.accent, clear))
                        .with_animation(
                            "setup-line-glow",
                            Animation::new(Duration::from_millis(2400))
                                .repeat()
                                .with_easing(pulsating_between(0.35, 1.)),
                            |glow, delta| glow.opacity(delta),
                        ),
                )
                .child(
                    // The dot, in its halo.
                    div()
                        .size(px(HALO))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(HALO / 2.))
                        .bg(t.accent.opacity(0.18))
                        .child(
                            div()
                                .size(px(HALO / 2.))
                                .rounded(px(HALO / 4.))
                                .bg(t.accent)
                                .shadow(vec![
                                    BoxShadow::new(
                                        px(0.),
                                        px(0.),
                                        t.accent.opacity(0.9),
                                    )
                                    .blur_radius(px(22.)),
                                ]),
                        )
                        .with_animation(
                            "setup-line-dot",
                            Animation::new(Duration::from_millis(2400))
                                .repeat()
                                .with_easing(pulsating_between(0.55, 1.)),
                            |dot, delta| dot.opacity(delta),
                        ),
                )
        }
        Link::Connected => track
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .h(px(2.))
                    .bg(t.green)
                    .shadow(vec![
                        BoxShadow::new(px(0.), px(0.), t.green.opacity(0.55))
                            .blur_radius(px(18.)),
                    ]),
            )
            .child(medallion(
                icon(Icon::Check, IconSize::XLARGE, t.green).into_any_element(),
            )),
        Link::Declined | Link::Blocked => {
            let mark = if link == Link::Declined {
                div()
                    .typeset(Type::HEADING.weighted(weight::STRONG))
                    .text_color(color)
                    .child("!")
                    .into_any_element()
            } else {
                icon(Icon::Close, IconSize::XLARGE, color).into_any_element()
            };
            track
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .w(relative(0.38))
                        .h(px(2.))
                        .bg(color.opacity(0.8)),
                )
                .child(
                    div()
                        .absolute()
                        .right_0()
                        .w(relative(0.38))
                        .h(px(2.))
                        .bg(color.opacity(0.25)),
                )
                .child(medallion(mark))
        }
    };
    let word = |text: &'static str, color: Hsla| {
        div()
            .absolute()
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(mono(text, Type::MICRO, color))
    };
    let inset = if size.tile < 100. { 4. } else { 16. };
    track
        .when_some(above, |track, text| {
            track.child(word(text, t.dim).top(px(inset)))
        })
        .when_some(below, |track, text| {
            track.child(word(text, below_color).bottom(px(inset)))
        })
}

/// τ, the line, and the other side.
fn handshake(
    link: Link,
    size: Size,
    words: (Option<&'static str>, Option<&'static str>),
    other: impl IntoElement,
    edge: Hsla,
    t: &Theme,
) -> Div {
    div()
        .mt(px(size.top))
        .flex()
        .items_center()
        .justify_center()
        .child(tau_tile(size.tile, t))
        .child(line(link, size, words.0, words.1, t))
        .child(service_tile(other, size.tile, edge, t))
}

// Words.

/// The headline and the sentence under it, centered.
fn headline(
    title: &str,
    sub: &str,
    style: Type,
    width: f32,
    compact: bool,
    t: &Theme,
) -> Div {
    div()
        .w_full()
        .flex()
        .flex_col()
        .items_center()
        .gap(sp(3.5))
        .text_center()
        .child(
            div()
                .typeset(if compact {
                    Type::DISPLAY.weighted(weight::EMPHASIS)
                } else {
                    style
                })
                .leading(1.1)
                .text_color(t.setup.light)
                .child(title.to_owned()),
        )
        .child(
            div()
                .max_w(px(width))
                .typeset(if compact { Type::BODY } else { Type::SUBTITLE })
                .leading(1.6)
                .text_color(t.muted)
                .child(sub.to_owned()),
        )
}

fn footnote(text: impl Into<SharedString>, t: &Theme) -> Div {
    div()
        .text_center()
        .typeset(Type::SMALL)
        .text_color(t.setup.faint)
        .child(text.into())
}

// Buttons.

/// The main action, light on the dark ground.
fn light_button(
    label: impl Into<SharedString>,
    leading: Option<AnyElement>,
    arrow: bool,
    compact: bool,
    t: &Theme,
) -> Div {
    let look = &t.setup;
    div()
        .h(px(52.))
        .when(compact, |button| button.w_full())
        .flex()
        .items_center()
        .justify_center()
        .gap(sp(2.5))
        .px(sp(6.))
        .flex_shrink_0()
        .rounded(radius::CARD)
        .bg(look.light)
        .text_color(look.on_light)
        .typeset(Type::LEAD.weighted(weight::STRONG))
        .cursor_pointer()
        .hover(|style| style.opacity(0.92))
        .shadow(vec![
            BoxShadow::new(px(0.), px(12.), gpui::black().opacity(0.35))
                .blur_radius(px(32.)),
        ])
        .children(leading)
        .child(label.into())
        .when(arrow, |button| {
            button.child(icon(Icon::Arrow, IconSize::LARGE, look.on_light))
        })
}

/// "Continue with ChatGPT": the light button with ChatGPT's mark.
fn chatgpt_button(
    label: &'static str,
    width: f32,
    compact: bool,
    t: &Theme,
) -> Div {
    light_button(
        label,
        Some(brand_mark(Brand::ChatGpt, 22., t.setup.on_light, t)),
        false,
        compact,
        t,
    )
    .when(!compact, |button| button.w(px(width)))
}

/// A quiet action on a dark chip.
fn ghost_button(
    label: impl Into<SharedString>,
    trailing: Option<Icon>,
    compact: bool,
    t: &Theme,
) -> Div {
    div()
        .h(px(44.))
        .when(compact, |button| button.w_full())
        .flex()
        .items_center()
        .justify_center()
        .gap(sp(2.))
        .px(sp(4.))
        .flex_shrink_0()
        .rounded(radius::LARGE)
        .border_1()
        .border_color(t.border)
        .bg(t.setup.surface)
        .text_color(t.text_soft)
        .typeset(Type::BODY.weighted(weight::EMPHASIS))
        .cursor_pointer()
        .hover(|style| style.bg(t.setup.tile))
        .child(label.into())
        .children(
            trailing.map(|glyph| icon(glyph, IconSize::SMALL, t.text_soft)),
        )
}

/// Words that act, without a box.
fn text_button(label: impl Into<SharedString>, t: &Theme) -> Div {
    div()
        .h(px(40.))
        .flex()
        .items_center()
        .justify_center()
        .px(sp(1.5))
        .typeset(Type::BODY)
        .text_color(t.muted)
        .cursor_pointer()
        .hover(|style| style.text_color(t.text))
        .child(label.into())
}

/// A link that leaves tau, with its arrow.
fn external_link(label: &'static str, t: &Theme) -> Div {
    text_link(label, Type::BODY, t)
        .flex()
        .items_center()
        .gap(sp(1.5))
        .child(icon(Icon::External, IconSize::SMALL, t.blue))
}

/// A row of actions, or a column of them on a phone.
fn actions(compact: bool) -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .gap(sp(3.))
        .when(compact, |row| row.w_full().flex_col().gap(sp(2.)))
        .when(!compact, |row| row.flex_wrap())
}

/// A card on the ground, see-through to the rings.
fn glass(t: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .rounded(radius::TILE)
        .bg(t.setup.glass)
        .border_1()
        .border_color(t.setup.surface_border)
}

/// Who is signed in to ChatGPT, and whether the plan may be used.
fn account_chip(
    account: &str,
    state: &'static str,
    color: Hsla,
    t: &Theme,
) -> Div {
    let look = &t.setup;
    let initial = account
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default();
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .justify_center()
        .gap(sp(3.))
        .py(sp(2.5))
        .pl(sp(2.5))
        .pr(sp(4.))
        .rounded(radius::FULL)
        .bg(look.surface)
        .border_1()
        .border_color(look.surface_border)
        .child(
            div()
                .size(px(30.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(radius::FULL)
                .bg(t.selected)
                .typeset(Type::SMALL)
                .child(initial),
        )
        .child(div().text_color(t.text).child(account.to_owned()))
        .child(
            div()
                .typeset(Type::CAPTION)
                .text_color(color)
                .child(format!("· {state}")),
        )
}

/// From onboarding, a way past GitHub to the model; from the app, the
/// way back.
fn skip_or_back(
    ws: &Workspace,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let from_app = ws.setup_goal.is_some();
    div()
        .id("skip-github")
        .child(text_button(
            if from_app {
                "Back"
            } else {
                "Skip GitHub for now"
            },
            t,
        ))
        .on_click(cx.listener(move |ws, _, _, cx| {
            if from_app {
                ws.leave_setup(cx)
            } else {
                ws.navigate(Route::Setup(SetupStep::Model), cx)
            }
        }))
}

// The screens.

fn welcome(
    ws: &Workspace,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Screen {
    let steps = [
        (
            "Sign in with GitHub",
            "Clone what you pick, open pull requests.",
        ),
        ("Connect ChatGPT", "Runs use your ChatGPT plan."),
        (
            "Pick repositories",
            "Cloned into tau's storage, versioned with jj.",
        ),
    ];
    let cards = steps.into_iter().enumerate().map(|(n, (name, detail))| {
        glass(t)
            .when(compact, |card| card.w_full())
            .when(!compact, |card| card.w(px(250.)))
            .gap(sp(1.5))
            .px(sp(4.5))
            .py(sp(4.))
            .child(mono(format!("0{}", n + 1), Type::MICRO, t.accent))
            .child(
                div()
                    .typeset(Type::BODY.weighted(weight::EMPHASIS))
                    .text_color(t.text)
                    .child(name),
            )
            .child(
                div()
                    .typeset(Type::SMALL)
                    .leading(1.5)
                    .text_color(t.dim)
                    .child(detail),
            )
    });
    let (hero, center) = if compact {
        let size = Size::big(true);
        (
            div().mt(px(size.top)).child(tau_tile(size.tile + 8., t)),
            size.center() + 4.,
        )
    } else {
        (orbit(t), TOP_BAR + 196.)
    };
    let body = div()
        .child(hero)
        .child(
            headline(
                "Coding agents on your repositories",
                "tau runs agents on the repositories you choose, on your \
                 ChatGPT plan. Every turn is a change you can go back to, \
                 fork or compare.",
                Type::HERO_LARGE,
                560.,
                compact,
                t,
            )
            .mt(sp(if compact { 8. } else { 18. })),
        )
        .child(
            div()
                .mt(sp(if compact { 7. } else { 9. }))
                .w_full()
                .flex()
                .flex_col()
                .items_center()
                .gap(sp(5.5))
                .child(
                    div()
                        .flex()
                        .justify_center()
                        .gap(sp(3.5))
                        .when(compact, |row| {
                            row.w_full().flex_col().gap(sp(2.5))
                        })
                        .children(cards),
                )
                .child(
                    actions(compact)
                        .child(
                            div()
                                .id("continue-github")
                                .when(compact, |button| button.w_full())
                                .child(light_button(
                                    "Continue with GitHub",
                                    Some(brand_mark(
                                        Brand::GitHub,
                                        18.,
                                        t.setup.on_light,
                                        t,
                                    )),
                                    true,
                                    compact,
                                    t,
                                ))
                                .on_click(cx.listener(|ws, _, _, cx| {
                                    ws.sign_in_github(cx)
                                })),
                        )
                        .child(skip_or_back(ws, t, cx)),
                )
                .child(footnote(
                    format!(
                        "Takes about a minute · tokens stay in {}, readable \
                         only by you",
                        ws.setup.config
                    ),
                    t,
                )),
        );
    Screen {
        mood: Mood::Acting,
        center,
        body,
    }
}

/// The welcome's picture: τ in the middle, and what it connects to
/// around it.
fn orbit(t: &Theme) -> Div {
    const R: f32 = 190.;
    const UP: f32 = 160.;
    const SAT: f32 = 72.;
    let (cx, cy) = (R + SAT / 2., UP + SAT / 2.);
    let faint = t.accent.opacity(0.05);
    let strong = t.accent.opacity(0.45);
    let sat = |x: f32, y: f32, inner: AnyElement, label: &'static str| {
        div()
            .absolute()
            .left(px(x - 60.))
            .top(px(y - SAT / 2.))
            .w(px(120.))
            .flex()
            .flex_col()
            .items_center()
            .gap(sp(2.5))
            .child(service_tile(inner, SAT, t.border, t))
            .child(mono(label, Type::MICRO, t.dim))
    };
    div()
        .relative()
        .w(px(2. * R + SAT))
        .h(px(cy + 64.))
        // The spokes.
        .child(
            div()
                .absolute()
                .left(px(cx - R))
                .top(px(cy))
                .w(px(2. * R))
                .h(px(1.))
                .flex()
                .child(div().flex_1().bg(linear_gradient(
                    90.,
                    linear_color_stop(faint, 0.),
                    linear_color_stop(strong, 1.),
                )))
                .child(div().flex_1().bg(linear_gradient(
                    90.,
                    linear_color_stop(strong, 0.),
                    linear_color_stop(faint, 1.),
                ))),
        )
        .child(
            div()
                .absolute()
                .left(px(cx))
                .top(px(cy - UP))
                .w(px(1.))
                .h(px(UP))
                .bg(linear_gradient(
                    180.,
                    linear_color_stop(faint, 0.),
                    linear_color_stop(strong, 1.),
                )),
        )
        .child(sat(
            cx - R,
            cy,
            brand_mark(Brand::GitHub, 40., t.text, t),
            "01 · GitHub",
        ))
        .child(sat(
            cx,
            cy - UP,
            icon(Icon::Repo, IconSize(32.), t.text_soft).into_any_element(),
            "03 · repositories",
        ))
        .child(sat(
            cx + R,
            cy,
            brand_mark(Brand::ChatGpt, 40., t.text, t),
            "02 · your plan",
        ))
        .child(
            div()
                .absolute()
                .left(px(cx - 60.))
                .top(px(cy - 60.))
                .child(tau_tile(120., t)),
        )
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

/// The code, a character to a cell. Clicking it copies it.
fn code_cells(code: &str, compact: bool, t: &Theme) -> Div {
    let (w, h, style) = if compact {
        (30., 42., Type::HEADLINE.mono())
    } else {
        (50., 64., Type::CODE_CELL)
    };
    div()
        .flex()
        .items_center()
        .gap(sp(if compact { 1. } else { 2. }))
        .children(code.chars().map(|ch| {
            if ch == '-' {
                div()
                    .w(px(if compact { 10. } else { 16. }))
                    .h(px(2.))
                    .bg(t.border_strong)
            } else {
                div()
                    .w(px(w))
                    .h(px(h))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(radius::CARD)
                    .bg(t.setup.surface)
                    .border_1()
                    .border_color(t.border)
                    .child(mono(ch.to_string(), style, t.setup.light))
            }
        }))
}

const PERMISSIONS: [(&str, &str, &str); 3] = [
    ("Contents", "read & write", "Read and write"),
    ("Pull requests", "read & write", "Read and write"),
    ("Metadata", "read", "Read-only"),
];

fn github(
    ws: &Workspace,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Screen {
    let size = Size::big(compact);
    let code = code_or_status(&ws.setup.github).ok().cloned();
    let (link, below, mood) = match &ws.setup.github {
        GitHub::SignedIn { .. } => (Link::Connected, "signed in", Mood::Done),
        GitHub::Failed(_) => (Link::Blocked, "failed", Mood::Refused),
        _ => (Link::Waiting, "waiting for approval", Mood::Acting),
    };
    let shown = match (&code, code_or_status(&ws.setup.github)) {
        (Some(code), _) => div()
            .id("copy-code")
            .cursor_pointer()
            .child(code_cells(&code.code, compact, t))
            .on_click({
                let code = code.code.clone();
                move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(
                        code.clone(),
                    ))
                }
            })
            .into_any_element(),
        (None, Err(status)) => div()
            .typeset(Type::HEADING)
            .text_color(t.muted)
            .child(status)
            .into_any_element(),
        (None, Ok(_)) => div().into_any_element(),
    };
    let open = code.clone();
    let status = match &ws.setup.github {
        GitHub::Failed(error) => Some(
            div()
                .max_w(px(560.))
                .text_center()
                .typeset(Type::SMALL)
                .text_color(t.red)
                .child(error.clone()),
        ),
        GitHub::SignedIn { user } => Some(
            div()
                .typeset(Type::SMALL)
                .text_color(t.green)
                .child(format!("Signed in as @{user}")),
        ),
        _ => None,
    };
    let permissions = div()
        .flex()
        .flex_wrap()
        .items_center()
        .justify_center()
        .gap_x(sp(4.5))
        .gap_y(sp(1.5))
        .typeset(Type::SMALL)
        .children(PERMISSIONS.iter().map(|(name, level, _)| {
            div()
                .flex()
                .gap(sp(1.))
                .child(div().text_color(t.text_soft).child(*name))
                .child(div().text_color(t.dim).child(format!("· {level}")))
        }))
        .when(!compact, |row| {
            row.child(div().text_color(t.border_strong).child("|"))
        })
        .child(
            div()
                .id("use-token")
                .child(text_link("Use a personal access token", Type::SMALL, t))
                .on_click(cx.listener(|ws, _, _, cx| {
                    ws.navigate(Route::Setup(SetupStep::Token), cx)
                })),
        );
    let body = div()
        .child(handshake(
            link,
            size,
            (Some("device sign-in"), Some(below)),
            brand_mark(Brand::GitHub, size.tile * 0.57, t.text, t),
            t.border,
            t,
        ))
        .child(
            headline(
                "Enter this code on GitHub",
                "Open GitHub, type the code, and approve the tau app. This \
                 screen moves on by itself.",
                Type::HERO_MEDIUM,
                560.,
                compact,
                t,
            )
            .mt(sp(if compact { 8. } else { 15. })),
        )
        .child(
            div()
                .mt(sp(if compact { 6. } else { 7. }))
                .w_full()
                .flex()
                .flex_col()
                .items_center()
                .gap(sp(6.))
                .child(shown)
                .child(
                    actions(compact)
                        .child(
                            div()
                                .id("open-github")
                                .when(compact, |button| button.w_full())
                                .child(light_button(
                                    "Copy code and open GitHub",
                                    None,
                                    true,
                                    compact,
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
                                .when(compact, |button| button.w_full())
                                .child(ghost_button(
                                    "I have approved it",
                                    None,
                                    compact,
                                    t,
                                ))
                                .on_click(cx.listener(|_, _, _, cx| {
                                    cx.emit(WorkspaceEvent::GitHubCheck)
                                })),
                        ),
                )
                .children(status)
                .child(permissions)
                .children(code.as_ref().map(|code| {
                    mono(
                        format!(
                            "{} · code expires in {} · only on the \
                             repositories you pick next",
                            code.url, code.expires
                        ),
                        Type::CAPTION,
                        t.setup.faint,
                    )
                    .text_center()
                }))
                .child(skip_or_back(ws, t, cx)),
        );
    Screen {
        mood,
        center: size.center(),
        body,
    }
}

fn token(
    ws: &Workspace,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Screen {
    let size = Size::big(compact);
    let look = &t.setup;
    let checking = ws.setup.github == GitHub::Checking;
    let failed = match &ws.setup.github {
        GitHub::Failed(error) => Some(error.clone()),
        _ => None,
    };
    let link = match (&failed, checking) {
        (Some(_), _) => Link::Blocked,
        (None, true) => Link::Waiting,
        _ => Link::Idle,
    };
    let rows = PERMISSIONS.iter().enumerate().map(|(n, (name, _, level))| {
        div()
            .flex()
            .justify_between()
            .py(sp(2.5))
            .when(n > 0, |row| row.border_t_1().border_color(look.divider))
            .child(
                div()
                    .typeset(Type::SMALL)
                    .text_color(t.text_soft)
                    .child(*name),
            )
            .child(mono(*level, Type::CAPTION, t.dim))
    });
    let form = div()
        .w_full()
        .max_w(px(560.))
        .flex()
        .flex_col()
        .gap(sp(3.5))
        .child(mono("FINE-GRAINED TOKEN", Type::MICRO, t.dim))
        .child(
            div()
                .flex()
                .items_center()
                .h(px(50.))
                .px(sp(4.))
                .rounded(radius::CARD)
                .border_1()
                .border_color(t.border_strong)
                .bg(look.surface)
                .shadow(vec![
                    BoxShadow::new(px(0.), px(0.), t.accent.opacity(0.10))
                        .spread_radius(px(4.)),
                ])
                .typeset(Type::BODY.mono())
                .child(ws.github_token.clone()),
        )
        .child(
            glass(t)
                .rounded(radius::CARD)
                .px(sp(4.))
                .py(sp(1.))
                .children(rows),
        )
        .when_some(failed, |form, error| {
            form.child(
                div().typeset(Type::SMALL).text_color(t.red).child(error),
            )
        });
    let body = div()
        .child(handshake(
            link,
            size,
            (Some("personal access token"), None),
            icon(Icon::Key, IconSize(size.tile * 0.41), t.text_soft),
            t.border,
            t,
        ))
        .child(
            headline(
                "Use a personal access token",
                "A fine-grained token works in place of the GitHub App. Give \
                 it these repository permissions, then paste it here.",
                Type::HERO_MEDIUM,
                560.,
                compact,
                t,
            )
            .mt(sp(if compact { 8. } else { 15. })),
        )
        .child(
            div()
                .mt(sp(if compact { 6. } else { 7. }))
                .w_full()
                .flex()
                .flex_col()
                .items_center()
                .gap(sp(5.))
                .child(form)
                .child(
                    actions(compact)
                        .child(
                            div()
                                .id("check-token")
                                .when(compact, |button| button.w_full())
                                .child(light_button(
                                    if checking {
                                        "Checking…"
                                    } else {
                                        "Check the token"
                                    },
                                    None,
                                    false,
                                    compact,
                                    t,
                                ))
                                .on_click(cx.listener(|ws, _, _, cx| {
                                    ws.submit_token_from_button(cx)
                                })),
                        )
                        .child(
                            div()
                                .id("token-back")
                                .child(text_button("Back to GitHub sign-in", t))
                                .on_click(cx.listener(|ws, _, _, cx| {
                                    ws.navigate(
                                        Route::Setup(SetupStep::GitHub),
                                        cx,
                                    )
                                })),
                        ),
                ),
        );
    Screen {
        mood: if link == Link::Blocked {
            Mood::Refused
        } else {
            Mood::Acting
        },
        center: size.center(),
        body,
    }
}

/// The model step, in whichever state the ChatGPT sign-in is.
fn model(
    ws: &Workspace,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Screen {
    let size = Size::big(compact);
    let (link, words, edge, mood, title, sub, below) = match &ws.setup.model {
        ModelAccess::None | ModelAccess::Failed(_) => (
            Link::Idle,
            (Some("plan usage"), Some("not connected")),
            t.border,
            Mood::Acting,
            "Connect tau to ChatGPT",
            "Runs use your ChatGPT plan through OpenAI's Responses API. One \
             sign-in in the browser; nothing to paste, no key to keep.",
            model_start(ws, compact, t, cx),
        ),
        ModelAccess::SigningIn { url } => (
            Link::Waiting,
            (Some("plan usage"), Some("waiting for the browser")),
            t.accent_border,
            Mood::Acting,
            "Finish in your browser",
            "A ChatGPT page just opened. Allow plan use there and this screen \
             moves on by itself.",
            model_waiting(ws, url.clone(), compact, t, cx),
        ),
        ModelAccess::Connected { .. } => (
            Link::Connected,
            (None, Some("connected")),
            t.setup.green_edge,
            Mood::Done,
            "You're using your ChatGPT plan",
            "Eligible usage in tau now counts toward your plan. Pick the model \
             runs start with; you can change it per run.",
            model_signed_in(ws, compact, t, cx),
        ),
        ModelAccess::PlanDisabled { account } => (
            Link::Declined,
            (None, Some("plan use off")),
            t.accent_border,
            Mood::Acting,
            "Allow tau to use your plan",
            "You're signed in, but plan use wasn't allowed. tau runs only on \
             your ChatGPT plan, so it can't start a run without it.",
            model_declined(account, compact, t, cx),
        ),
        ModelAccess::NotEligible { account, detail } => (
            Link::Blocked,
            (None, Some("not eligible")),
            t.red_border,
            Mood::Refused,
            "This account can't share its plan",
            "OpenAI says ChatGPT plan use isn't available for this account or \
             workspace. Usually because:",
            model_not_eligible(account, detail, compact, t, cx),
        ),
    };
    let body = div()
        .child(handshake(
            link,
            size,
            words,
            brand_mark(Brand::ChatGpt, size.tile * 0.57, t.text, t),
            edge,
            t,
        ))
        .child(
            headline(title, sub, Type::HERO, 560., compact, t)
                .mt(sp(if compact { 8. } else { 18. })),
        )
        .child(
            below
                .mt(sp(if compact { 7. } else { 11. }))
                .w_full()
                .flex()
                .flex_col()
                .items_center(),
        )
        .when(ws.setup_goal.is_some(), |body| {
            body.child(
                div()
                    .id("setup-back")
                    .mt(sp(4.))
                    .child(text_button("Back", t))
                    .on_click(cx.listener(|ws, _, _, cx| ws.leave_setup(cx))),
            )
        });
    Screen {
        mood,
        center: size.center(),
        body,
    }
}

fn model_start(
    ws: &Workspace,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let steps = [
        "Sign in to ChatGPT",
        "Pick a workspace",
        "Keep the name “tau”",
        "Allow plan use",
    ];
    let timeline = div()
        .flex()
        .items_center()
        .justify_center()
        .gap(sp(4.))
        .when(compact, |row| row.flex_col().items_start().gap(sp(2.)))
        .children(steps.iter().enumerate().flat_map(|(n, step)| {
            let item = div()
                .flex()
                .items_center()
                .gap(sp(2.5))
                .child(mono(format!("0{}", n + 1), Type::MICRO, t.accent))
                .child(
                    div()
                        .typeset(Type::SMALL)
                        .text_color(t.setup.soft)
                        .child(*step),
                )
                .into_any_element();
            let rule = (n + 1 < steps.len() && !compact).then(|| {
                div().w(px(28.)).h(px(1.)).bg(t.border).into_any_element()
            });
            [Some(item), rule].into_iter().flatten()
        }));
    let failed = match &ws.setup.model {
        ModelAccess::Failed(error) => Some(error.clone()),
        _ => None,
    };
    div()
        .gap(sp(6.5))
        .when_some(failed, |col, error| {
            col.child(
                div()
                    .max_w(px(560.))
                    .text_center()
                    .typeset(Type::SMALL)
                    .text_color(t.red)
                    .child(error),
            )
        })
        .child(
            div()
                .id("chatgpt-sign-in")
                .when(compact, |button| button.w_full())
                .child(
                    chatgpt_button("Continue with ChatGPT", 380., compact, t)
                        .h(px(54.)),
                )
                .on_click(cx.listener(|ws, _, _, cx| {
                    ws.sign_in_chatgpt(None, false, cx)
                })),
        )
        .child(timeline)
        .child(footnote(
            "Needs ChatGPT Plus or Pro · tau can't read your conversations",
            t,
        ))
}

fn model_waiting(
    ws: &Workspace,
    url: Option<String>,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let look = &t.setup;
    let again = match url {
        Some(url) => div()
            .id("chatgpt-open-again")
            .when(compact, |button| button.w_full())
            .child(ghost_button(
                "Open the page again",
                Some(Icon::External),
                compact,
                t,
            ))
            .on_click(move |_, _, cx| cx.open_url(&url))
            .into_any_element(),
        None => div()
            .typeset(Type::BODY)
            .text_color(t.muted)
            .child("Opening the sign-in page…")
            .into_any_element(),
    };
    let paste = glass(t)
        .w_full()
        .max_w(px(560.))
        .gap(sp(2.5))
        .px(sp(4.5))
        .py(sp(4.))
        .child(
            div()
                .typeset(Type::SMALL)
                .text_color(t.muted)
                .child("Signing in on another device? Paste the address the browser ended on."),
        )
        .child(
            div()
                .flex()
                .gap(sp(2.))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .items_center()
                        .h(px(42.))
                        .px(sp(3.))
                        .rounded(radius::LARGE)
                        .border_1()
                        .border_color(t.border)
                        .bg(look.field)
                        .typeset(Type::CAPTION.mono())
                        .child(ws.chatgpt_callback.clone()),
                )
                .child(
                    div()
                        .id("chatgpt-paste")
                        .h(px(42.))
                        .flex()
                        .items_center()
                        .px(sp(3.5))
                        .rounded(radius::LARGE)
                        .border_1()
                        .border_color(t.border)
                        .bg(t.panel)
                        .typeset(Type::SMALL)
                        .text_color(t.text)
                        .cursor_pointer()
                        .hover(|style| style.bg(t.raised))
                        .child("Finish")
                        .on_click(cx.listener(|ws, _, _, cx| {
                            ws.submit_chatgpt_callback_from_button(cx)
                        })),
                ),
        );
    div()
        .gap(sp(5.))
        .child(
            actions(compact).child(again).child(
                div()
                    .id("chatgpt-cancel")
                    .child(text_button("Cancel", t))
                    .on_click(cx.listener(|ws, _, _, cx| {
                        ws.cancel_chatgpt_sign_in(cx)
                    })),
            ),
        )
        .child(paste)
        .child(
            mono(
                "listening on this machine · nothing is saved until the \
                 browser comes back",
                Type::CAPTION,
                look.faint,
            )
            .text_center(),
        )
}

fn model_signed_in(
    ws: &Workspace,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let look = &t.setup;
    let models = &ws.catalog().models;
    let current = models.settings.default_for("coder").model;
    let shown = models.shown("");
    let pills: Vec<_> =
        shown
            .iter()
            .map(|option| {
                let picked = option.id == current;
                let id = option.id.clone();
                div()
                    .id(SharedString::from(format!("setup-model-{id}")))
                    .px(sp(3.))
                    .py(sp(1.75))
                    .rounded(radius::FULL)
                    .border_1()
                    .border_color(if picked { t.accent } else { t.border })
                    .bg(if picked {
                        t.accent.opacity(0.08)
                    } else {
                        look.surface
                    })
                    .cursor_pointer()
                    .hover(|style| style.border_color(t.accent_border))
                    .child(mono(
                        option.id.clone(),
                        Type::CODE,
                        if picked { t.accent } else { look.soft },
                    ))
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.pick_setup_model(&id, cx)
                    }))
            })
            .collect();
    let listing = if !pills.is_empty() {
        div()
            .flex()
            .flex_wrap()
            .justify_center()
            .gap(sp(2.))
            .max_w(px(700.))
            .children(pills)
    } else {
        let (text, color) = match &models.access.models_error {
            Some(error) => {
                (format!("Could not list your models: {error}"), t.red)
            }
            None => ("Listing your plan's models…".to_owned(), look.faint),
        };
        div()
            .max_w(px(560.))
            .text_center()
            .typeset(Type::SMALL)
            .text_color(color)
            .child(text)
    };
    let next = if ws.setup_goal.is_some() {
        "Done"
    } else if ws.setup.repos.is_empty() {
        "Start your first run"
    } else {
        "Continue to repositories"
    };
    div()
        .gap(sp(6.5))
        .children(models.access.active_account().map(|account| {
            account_chip(&account.label, "plan use allowed", t.green, t)
        }))
        .child(listing)
        .child(
            actions(compact)
                .gap(sp(4.5))
                .child(
                    div()
                        .id("model-continue")
                        .when(compact, |button| button.w_full())
                        .child(light_button(next, None, true, compact, t))
                        .on_click(cx.listener(|ws, _, _, cx| {
                            ws.continue_from_model(cx)
                        })),
                )
                .child(
                    div()
                        .id("manage-usage")
                        .child(external_link("Manage usage", t))
                        .on_click(
                            cx.listener(|ws, _, _, cx| ws.manage_usage(cx)),
                        ),
                ),
        )
}

fn model_declined(
    account: &str,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    div()
        .gap(sp(6.5))
        .child(account_chip(account, "plan use off", t.accent, t))
        .child(
            actions(compact)
                .child(
                    div()
                        .id("chatgpt-enable")
                        .when(compact, |button| button.w_full())
                        .child(light_button(
                            "Enable ChatGPT plan use",
                            None,
                            false,
                            compact,
                            t,
                        ))
                        .on_click(
                            cx.listener(|ws, _, _, cx| {
                                ws.enable_plan_usage(cx)
                            }),
                        ),
                )
                .child(
                    div()
                        .id("chatgpt-other")
                        .child(text_button("Use another account", t))
                        .on_click(cx.listener(|ws, _, _, cx| {
                            ws.sign_in_chatgpt(None, false, cx)
                        })),
                ),
        )
        .child(footnote(
            "Opens the same ChatGPT page, asking only for plan use · take it \
             back any time in ChatGPT settings",
            t,
        ))
}

fn model_not_eligible(
    account: &str,
    detail: &str,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let look = &t.setup;
    let reasons = [
        "The plan isn't Plus or Pro",
        "A workspace admin turned app access off",
        "Plan use isn't offered in your region yet",
    ];
    div()
        .gap(sp(6.))
        .when(!account.is_empty(), |col| {
            col.child(account_chip(account, "not eligible", t.red, t))
        })
        .child(
            div()
                .flex()
                .flex_wrap()
                .justify_center()
                .gap(sp(2.))
                .max_w(px(760.))
                .children(reasons.map(|reason| {
                    div()
                        .px(sp(3.))
                        .py(sp(1.75))
                        .rounded(radius::FULL)
                        .bg(look.surface)
                        .border_1()
                        .border_color(look.surface_border)
                        .typeset(Type::SMALL)
                        .text_color(look.soft)
                        .child(reason)
                })),
        )
        .child(
            actions(compact)
                .gap(sp(3.5))
                .child(
                    div()
                        .id("chatgpt-other")
                        .when(compact, |button| button.w_full())
                        .child(chatgpt_button(
                            "Use another ChatGPT account",
                            340.,
                            compact,
                            t,
                        ))
                        .on_click(cx.listener(|ws, _, _, cx| {
                            ws.sign_in_chatgpt(None, false, cx)
                        })),
                )
                .child(
                    div()
                        .id("plan-help")
                        .child(external_link("Learn more", t))
                        .on_click(|_, _, cx| cx.open_url(PLAN_HELP_URL)),
                ),
        )
        .child(
            mono(detail.to_owned(), Type::CAPTION, look.refusal).text_center(),
        )
}

fn repos(
    ws: &Workspace,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Screen {
    let size = Size::small(compact);
    let look = &t.setup;
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
                .gap(sp(3.5))
                .px(sp(4.5))
                .py(sp(3.))
                .when(n > 0, |row| row.border_t_1().border_color(look.divider))
                .cursor_pointer()
                .when(repo.selected, |row| row.bg(t.accent.opacity(0.05)))
                .hover(|style| style.bg(t.accent.opacity(0.03)))
                .child(checkbox(repo.selected, compact, t))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .gap(sp(0.5))
                        .child(mono(repo.name.clone(), Type::SMALL, t.text))
                        .when(!repo.description.is_empty(), |col| {
                            col.child(
                                div()
                                    .typeset(Type::CAPTION)
                                    .text_color(t.dim)
                                    .child(repo.description.clone()),
                            )
                        }),
                )
                .child(mono(repo.branch.clone(), Type::CAPTION, look.faint))
                .on_click(
                    cx.listener(move |ws, _, _, cx| ws.toggle_repo(&name, cx)),
                )
        })
        .collect();
    let count = ws.setup.selected().count();
    let panel = glass(t)
        .w_full()
        .max_w(px(640.))
        .rounded(radius::BUBBLE)
        .overflow_hidden()
        .shadow(vec![
            BoxShadow::new(px(0.), px(30.), gpui::black().opacity(0.45))
                .blur_radius(px(80.)),
        ])
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(2.5))
                .h(px(46.))
                .px(sp(4.5))
                .border_b_1()
                .border_color(look.surface_border)
                .child(icon(Icon::Search, IconSize::MEDIUM, t.dim))
                .child(
                    div().flex_1().min_w(px(0.)).child(ws.repo_filter.clone()),
                )
                .child(mono(
                    format!("{} repositories", ws.setup.repos.len()),
                    Type::CAPTION,
                    look.faint,
                )),
        )
        .children(rows);
    let body = div()
        .child(handshake(
            Link::Connected,
            size,
            (None, None),
            icon(Icon::Repo, IconSize(size.tile * 0.42), t.text_soft),
            t.border,
            t,
        ))
        .child(
            headline(
                "Pick repositories",
                "tau clones each into its own storage, as a jj repository. \
                 Runs work there, never in your checkouts.",
                Type::HERO_SMALL,
                620.,
                compact,
                t,
            )
            .mt(sp(if compact { 6. } else { 10. })),
        )
        .child(
            div()
                .mt(sp(if compact { 6. } else { 8. }))
                .w_full()
                .flex()
                .flex_col()
                .items_center()
                .gap(sp(5.))
                .child(panel)
                .child(
                    actions(compact)
                        .gap(sp(4.))
                        .child(
                            div()
                                .id("clone")
                                .when(compact, |button| button.w_full())
                                .child(
                                    light_button(
                                        match count {
                                            0 => "Pick a repository".to_owned(),
                                            1 => {
                                                "Clone 1 repository".to_owned()
                                            }
                                            n => format!(
                                                "Clone {n} repositories"
                                            ),
                                        },
                                        None,
                                        count > 0,
                                        compact,
                                        t,
                                    )
                                    .when(count == 0, |button| {
                                        button.opacity(0.5)
                                    }),
                                )
                                .on_click(cx.listener(move |ws, _, _, cx| {
                                    if count > 0 {
                                        ws.clone_selected(cx)
                                    }
                                })),
                        )
                        .child(
                            div()
                                .id("install-app")
                                .child(text_link(
                                    "Missing one? Give tau's GitHub App access",
                                    Type::SMALL,
                                    t,
                                ))
                                .on_click(|_, _, cx| {
                                    cx.open_url(&crate::github::install_url())
                                }),
                        ),
                )
                .child(footnote(
                    format!(
                        "Cloned into {} · every turn of a run is a commit you \
                         can go back to, fork from, or compare",
                        ws.setup.storage
                    ),
                    t,
                )),
        );
    Screen {
        mood: Mood::Acting,
        center: size.center(),
        body,
    }
}

fn ready(
    ws: &Workspace,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Screen {
    let size = Size::small(compact);
    let look = &t.setup;
    let clones = ws.setup.clones.iter().map(|clone| {
        let (state, share, color) = match &clone.state {
            CloneState::Cloning { share, detail } => (
                if detail.is_empty() {
                    format!("cloning · {:.0}%", share * 100.)
                } else {
                    format!("cloning · {detail}")
                },
                *share,
                t.accent,
            ),
            CloneState::Ready => ("ready".to_owned(), 1.0, t.green),
            CloneState::Failed(error) => (error.clone(), 1.0, t.red),
        };
        glass(t)
            .when(compact, |card| card.w_full())
            .when(!compact, |card| card.w(px(300.)))
            .gap(sp(2.5))
            .px(sp(4.))
            .py(sp(3.5))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(sp(2.5))
                    .child(
                        mono(clone.name.clone(), Type::SMALL, t.text)
                            .min_w(px(0.)),
                    )
                    .child(mono(state, Type::CAPTION, color).flex_shrink_0()),
            )
            .child(bar(share, 4., color, look.divider))
    });
    let repo = ws
        .setup
        .clones
        .first()
        .map(|clone| clone.name.clone())
        .unwrap_or_else(|| ws.name.clone());
    let chip = |text: String| {
        mono(text, Type::CAPTION, t.text_soft)
            .px(sp(2.5))
            .py(sp(1.5))
            .rounded(radius::BOX)
            .bg(t.panel)
    };
    let model = ws
        .setup
        .model_label()
        .map(|label| label.split(" · ").next().unwrap_or(label).to_owned());
    let on_plan = model.is_some();
    let composer = div()
        .w_full()
        .max_w(px(720.))
        .flex()
        .flex_col()
        .rounded(radius::SHEET)
        .bg(look.surface)
        .border_1()
        .border_color(t.border_strong)
        .shadow(vec![
            BoxShadow::new(px(0.), px(0.), t.accent.opacity(0.08))
                .spread_radius(px(5.)),
            BoxShadow::new(px(0.), px(30.), gpui::black().opacity(0.5))
                .blur_radius(px(80.)),
        ])
        .child(
            div()
                .h(px(if compact { 88. } else { 108. }))
                .px(sp(5.))
                .py(sp(4.5))
                .typeset(Type::SUBTITLE)
                .leading(1.55)
                .child(ws.first_task.clone()),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(sp(2.5))
                .pl(sp(4.))
                .pr(sp(3.))
                .pt(sp(2.5))
                .pb(sp(3.))
                .child(chip(repo))
                .child(chip(model.unwrap_or_else(|| "no model yet".to_owned())))
                .when(on_plan, |row| {
                    row.child(
                        div()
                            .typeset(Type::CAPTION)
                            .text_color(t.green)
                            .child("· on your ChatGPT plan"),
                    )
                })
                .child(div().flex_1())
                .child(
                    div()
                        .id("first-run")
                        .when(compact, |button| button.w_full())
                        .h(px(42.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(sp(2.))
                        .px(sp(4.5))
                        .rounded(radius::LARGE)
                        .bg(t.accent)
                        .text_color(look.on_light)
                        .typeset(Type::BODY.weighted(weight::STRONG))
                        .cursor_pointer()
                        .hover(|style| style.opacity(0.9))
                        .child("Start run")
                        .child(icon(
                            Icon::Arrow,
                            IconSize::LARGE,
                            look.on_light,
                        ))
                        .on_click(cx.listener(|ws, _, _, cx| {
                            ws.start_first_run_from_button(cx)
                        })),
                ),
        );
    let body = div()
        .child(handshake(
            Link::Connected,
            size,
            (None, None),
            icon(Icon::Runs, IconSize(size.tile * 0.42), t.text_soft),
            t.border,
            t,
        ))
        .child(
            headline(
                "Start your first run",
                "Describe a task. tau works on a new change in the repository; \
                 open a pull request when you like the result.",
                Type::HERO_SMALL,
                620.,
                compact,
                t,
            )
            .mt(sp(if compact { 6. } else { 10. })),
        )
        .child(
            div()
                .mt(sp(if compact { 6. } else { 8. }))
                .w_full()
                .flex()
                .flex_col()
                .items_center()
                .gap(sp(5.5))
                .when(!ws.setup.clones.is_empty(), |col| {
                    col.child(
                        div()
                            .flex()
                            .flex_wrap()
                            .justify_center()
                            .gap(sp(3.5))
                            .when(compact, |row| {
                                row.w_full().flex_col().gap(sp(2.5))
                            })
                            .children(clones),
                    )
                })
                .child(composer)
                .child(footnote(
                    "⏎ to start · every turn is a commit you can go back to, \
                     fork from, or compare",
                    t,
                )),
        );
    Screen {
        mood: Mood::Acting,
        center: size.center(),
        body,
    }
}
