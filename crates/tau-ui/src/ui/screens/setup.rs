//! Onboarding, in the handshake style: tau's tile and the service it
//! connects to, joined by a line whose look says how the connection
//! stands, under a bar with the four stages. Faint rings and a glow sit
//! behind the handshake, amber while acting, green once connected and
//! red when blocked. On a phone the same screens stack, with smaller
//! tiles and full-width buttons.
//!
//! It moves as the prototype does (see [`crate::motion`]): the rings
//! breathe, the line drifts, carries a comet while waiting and draws
//! itself green once connected; the handshake shrinks and rises between
//! steps; each screen's words rise into place. Every change follows a
//! change of state, never a timer.

use std::time::Duration;

use gpui::{
    Animation,
    AnimationExt as _,
    AnyElement,
    App,
    BoxShadow,
    ClipboardItem,
    Context,
    Div,
    Focusable as _,
    Hsla,
    SharedString,
    Transformation,
    Window,
    div,
    linear_color_stop,
    linear_gradient,
    prelude::*,
    px,
    radians,
    relative,
    size as area,
    svg,
};

use crate::{
    assets::{Brand, Icon},
    motion::{
        Frame,
        Link,
        Mood,
        Scale,
        Scene,
        Segment,
        Service,
        SetupMotion,
        after,
        breath,
        curve,
        entrance,
        lerp,
        loop_of,
        mix,
        pop,
        stagger,
    },
    route::Route,
    setup::{CloneState, DeviceCode, GitHub, ModelAccess, SetupStep},
    theme::{Design as _, IconSize, Theme, Type, radius, sp, weight},
    ui::{Material as _, bar, icon, icon_button, mono, text_link},
    workspace::{Workspace, WorkspaceEvent},
};

/// Where ChatGPT explains plan use and who can share it.
const PLAN_HELP_URL: &str = "https://help.openai.com";

/// The waiting dot's halo, when the comet does not run; the dot is half
/// as wide.
const HALO: f32 = 24.;

/// The top bar's height, the same on a desktop and a phone.
const TOP_BAR: f32 = 56.;

/// The comet's length and thickness.
const COMET: (f32, f32) = (70., 8.);

/// How big the handshake is and where it sits.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Size {
    tile: f32,
    line: f32,
    /// From the top bar to the tiles' top.
    top: f32,
}

impl Size {
    fn of(scale: Scale, compact: bool) -> Self {
        match (scale, compact) {
            (Scale::Big, false) => Self {
                tile: 112.,
                line: 250.,
                top: 140.,
            },
            (Scale::Big, true) => Self {
                tile: 64.,
                line: 96.,
                top: 28.,
            },
            (Scale::Small, false) => Self {
                tile: 72.,
                line: 170.,
                top: 40.,
            },
            (Scale::Small, true) => Self {
                tile: 52.,
                line: 80.,
                top: 24.,
            },
        }
    }

    fn between(from: Self, to: Self, t: f32) -> Self {
        Self {
            tile: lerp(from.tile, to.tile, t),
            line: lerp(from.line, to.line, t),
            top: lerp(from.top, to.top, t),
        }
    }

    /// The handshake's middle, from the window's top.
    fn center(self) -> f32 {
        TOP_BAR + self.top + self.tile / 2.
    }
}

/// Where the welcome's picture centers, from the window's top.
const WELCOME_CENTER: f32 = TOP_BAR + 196.;

// What the frame shows.

/// The handshake's other side, the line and the rings for `step`, as
/// onboarding stands.
fn frame(ws: &Workspace, step: SetupStep) -> Frame {
    let signed_in = ws.setup.user().is_some();
    let (state, mood, link, service, scale) = match step {
        SetupStep::Welcome => {
            (0, Mood::Acting, Link::Idle, Service::None, Scale::Big)
        }
        SetupStep::GitHub => match &ws.setup.github {
            GitHub::SignedIn { .. } => {
                (1, Mood::Done, Link::Connected, Service::GitHub, Scale::Big)
            }
            GitHub::Failed(_) => {
                (2, Mood::Refused, Link::Blocked, Service::GitHub, Scale::Big)
            }
            _ => (0, Mood::Acting, Link::Waiting, Service::GitHub, Scale::Big),
        },
        SetupStep::Token => match &ws.setup.github {
            GitHub::Failed(_) => {
                (2, Mood::Refused, Link::Blocked, Service::Token, Scale::Big)
            }
            GitHub::Checking => {
                (1, Mood::Acting, Link::Waiting, Service::Token, Scale::Big)
            }
            _ => (0, Mood::Acting, Link::Idle, Service::Token, Scale::Big),
        },
        SetupStep::Model => {
            let (state, mood, link) = match &ws.setup.model {
                ModelAccess::None => (0, Mood::Acting, Link::Idle),
                ModelAccess::Failed(_) => (5, Mood::Acting, Link::Idle),
                ModelAccess::SigningIn { .. } => {
                    (1, Mood::Acting, Link::Waiting)
                }
                ModelAccess::Connected { .. } => {
                    (2, Mood::Done, Link::Connected)
                }
                ModelAccess::PlanDisabled { .. } => {
                    (3, Mood::Acting, Link::Declined)
                }
                ModelAccess::NotEligible { .. } => {
                    (4, Mood::Refused, Link::Blocked)
                }
            };
            (state, mood, link, Service::ChatGpt, Scale::Big)
        }
        SetupStep::Repos => (
            0,
            Mood::Acting,
            Link::Connected,
            Service::Repos,
            Scale::Small,
        ),
        SetupStep::Ready => (
            0,
            Mood::Acting,
            Link::Connected,
            Service::FirstRun,
            Scale::Small,
        ),
    };
    let stage = (step != SetupStep::Welcome).then(|| step.stage());
    let segments = std::array::from_fn(|n| match stage {
        // GitHub can be skipped: then it is not done.
        Some(stage) if n < stage && (n > 0 || signed_in) => Segment::Done,
        Some(stage) if n == stage => Segment::Current,
        _ => Segment::Ahead,
    });
    Frame {
        scene: Scene { step, state },
        mood,
        link,
        service,
        scale,
        segments,
    }
}

/// Records what this frame of onboarding shows, before it is drawn, so
/// that what changed animates.
pub fn observe(ws: &mut Workspace, step: SetupStep, window: &Window, cx: &App) {
    let frame = frame(ws, step);
    let composing = ws.first_task.read(cx).focus_handle(cx).is_focused(window);
    let clones: Vec<(String, u32, bool)> = ws
        .setup
        .clones
        .iter()
        .map(|clone| match &clone.state {
            CloneState::Cloning { share, .. } => (
                clone.name.clone(),
                (share.clamp(0., 1.) * 1000.) as u32,
                false,
            ),
            CloneState::Ready => (clone.name.clone(), 1000, true),
            CloneState::Failed(_) => (clone.name.clone(), 1000, false),
        })
        .collect();
    let reduce = ws.reduce_motion() || cx.reduce_motion();
    let motion = &mut ws.setup_motion;
    motion.observe(frame);
    motion.composing.observe(composing);
    motion.observe_clones(
        clones
            .iter()
            .map(|(name, share, ready)| (name.as_str(), *share, *ready)),
    );
    motion.active = window.is_window_active();
    motion.reduce = reduce;
}

/// What the handshake's animations need to know, all copied so that an
/// animation can redraw it.
#[derive(Debug, Clone, Copy)]
struct Beat {
    link: Link,
    link_from: Link,
    link_epoch: u64,
    /// The link changed to what it is, rather than being so from the
    /// start.
    link_changed: bool,
    service: Service,
    service_epoch: u64,
    service_changed: bool,
    loops: bool,
    reduce: bool,
}

impl Beat {
    fn of(motion: &SetupMotion) -> Self {
        Self {
            link: motion.link.current,
            link_from: motion.link.previous,
            link_epoch: motion.link.epoch,
            link_changed: motion.link.changed(),
            service: motion.service.current,
            service_epoch: motion.service.epoch,
            service_changed: motion.service.changed(),
            loops: motion.loops(),
            reduce: motion.reduce,
        }
    }
}

pub fn render(
    ws: &Workspace,
    step: SetupStep,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let motion = &ws.setup_motion;
    let body = match step {
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
        .child(backdrop(motion, step, compact, t))
        .child(top_bar(ws, step, compact, t, cx))
        .child(
            div()
                .id("setup-body")
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scroll()
                .child(
                    body.w_full()
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

fn mood_color(mood: Mood, t: &Theme) -> Hsla {
    match mood {
        Mood::Acting => t.accent,
        Mood::Done => t.green,
        Mood::Refused => t.red,
    }
}

/// The rings and the glow in one mood's color. The rings breathe, out of
/// step, while loops run. Radial gradients are not in GPUI: the glow is
/// a soft shadow around a circle.
fn ring_set(
    set: &'static str,
    mood: Mood,
    scale: f32,
    loops: bool,
    t: &Theme,
) -> Div {
    let look = &t.setup;
    let color = mood_color(mood, t);
    let glow = 200. * scale;
    let strength = match mood {
        Mood::Refused => look.glow_alpha * 0.8,
        Mood::Done => look.glow_alpha * 1.2,
        Mood::Acting => look.glow_alpha,
    };
    let rings = look
        .ring_radii
        .iter()
        .zip(look.ring_alphas)
        .enumerate()
        .map(move |(n, (radius, alpha))| {
            let r = radius * scale;
            let ring = div()
                .absolute()
                .left(px(-r))
                .top(px(-r))
                .size(px(2. * r))
                .rounded(px(r))
                .border_1()
                .border_color(color.opacity(alpha));
            if !loops {
                return ring.into_any_element();
            }
            // Each ring a fifth of a breath ahead of the one inside it.
            ring.with_animation(
                SharedString::from(format!("ring-{set}-{n}")),
                loop_of(7000, n as f32 * 0.2),
                move |ring, phase| {
                    let v = breath(phase);
                    let r = r * (1. + 0.018 * v);
                    ring.left(px(-r))
                        .top(px(-r))
                        .size(px(2. * r))
                        .rounded(px(r))
                        .opacity(1. - 0.28 * v)
                },
            )
            .into_any_element()
        });
    div()
        .absolute()
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
                    BoxShadow::new(px(0.), px(0.), color.opacity(strength))
                        .blur_radius(px(glow))
                        .spread_radius(px(glow * 0.35)),
                ]),
        )
        .children(rings)
}

/// The rings and the glow, centered on the handshake. A new mood cross-
/// fades them; a new size moves them with the handshake; a connection
/// sends one green ring rippling out.
fn backdrop(
    motion: &SetupMotion,
    step: SetupStep,
    compact: bool,
    t: &Theme,
) -> Div {
    let scale = if compact { t.setup.compact_scale } else { 1. };
    let loops = motion.loops();
    let center_of = |scale: Scale| {
        if step == SetupStep::Welcome && !compact {
            WELCOME_CENTER
        } else {
            Size::of(scale, compact).center()
        }
    };
    let (from, to) = (
        center_of(motion.scale.previous),
        center_of(motion.scale.current),
    );
    let mood = &motion.mood;
    let now = ring_set("now", mood.current, scale, loops, t);
    let mut anchor = div()
        .absolute()
        .left(relative(0.5))
        .size(px(0.))
        .when(mood.within(800), |anchor| {
            anchor.child(
                ring_set("was", mood.previous, scale, loops, t).with_animation(
                    SharedString::from(format!("mood-out-{}", mood.epoch)),
                    after(0, 800, curve::ease_in_out()),
                    |set, e| set.opacity(1. - e),
                ),
            )
        })
        .child(if mood.changed() {
            now.with_animation(
                SharedString::from(format!("mood-in-{}", mood.epoch)),
                after(0, 800, curve::ease_in_out()),
                |set, e| set.opacity(e),
            )
            .into_any_element()
        } else {
            now.into_any_element()
        });
    let link = &motion.link;
    if link.current == Link::Connected
        && link.changed()
        && !motion.reduce
        && link.within(1500)
    {
        let green = t.green;
        let r = 150. * scale;
        anchor = anchor.child(
            div()
                .absolute()
                .border(px(2.))
                .border_color(green.opacity(0.8))
                .with_animation(
                    SharedString::from(format!("ripple-{}", link.epoch)),
                    Animation::new(Duration::from_millis(1400)),
                    move |ring, t| {
                        let ms = t * 1400.;
                        if ms < 300. {
                            return ring.opacity(0.);
                        }
                        let e = curve::ripple()((ms - 300.) / 1100.);
                        let r = r * lerp(0.35, 1.7, e);
                        ring.left(px(-r))
                            .top(px(-r))
                            .size(px(2. * r))
                            .rounded(px(r))
                            .opacity(0.75 * (1. - e))
                    },
                ),
        );
    }
    let layer = div().absolute().left_0().top_0().size_full();
    if motion.scale.changed() && from != to && !motion.reduce {
        layer.child(anchor.with_animation(
            SharedString::from(format!("scene-move-{}", motion.scale.epoch)),
            after(0, 550, curve::settle()),
            move |anchor, e| anchor.top(px(lerp(from, to, e))),
        ))
    } else {
        layer.child(anchor.top(px(to)))
    }
}

/// tau, the four stages as segments (done green, current amber), and
/// where you are. A segment whose state changed fills left to right in
/// its new color. A phone shows the segments without their names.
fn top_bar(
    ws: &Workspace,
    step: SetupStep,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let look = &t.setup;
    let motion = &ws.setup_motion;
    let segments = &motion.segments;
    let color = |segment: Segment| match segment {
        Segment::Done => t.green,
        Segment::Current => t.accent,
        Segment::Ahead => look.track,
    };
    let label = |segment: Segment| {
        if segment == Segment::Ahead {
            look.idle
        } else {
            t.dim
        }
    };
    let width = if compact { 28. } else { 64. };
    let reduce = motion.reduce;
    let items = SetupStep::STAGES.iter().enumerate().map(|(n, name)| {
        let (was, now) = (segments.previous[n], segments.current[n]);
        let (base, fill) = (color(was), color(now));
        let (from_label, to_label) = (label(was), label(now));
        let item = move |share: f32, fade: f32, label: Hsla| {
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(sp(1.75))
                .child(
                    div()
                        .relative()
                        .w(px(width))
                        .h(px(3.))
                        .rounded(radius::HAIRLINE)
                        .overflow_hidden()
                        .bg(base)
                        .child(
                            div()
                                .absolute()
                                .left_0()
                                .top_0()
                                .h_full()
                                .w(relative(share))
                                .bg(fill.opacity(fill.a * fade)),
                        ),
                )
                .when(!compact, |item| {
                    item.child(mono(*name, Type::MICRO, label))
                })
        };
        if was == now {
            return item(1., 1., to_label).into_any_element();
        }
        div()
            .with_animation(
                SharedString::from(format!("segment-{n}-{}", segments.epoch)),
                after(if reduce { 0 } else { 200 }, 450, curve::settle()),
                move |slot, e| {
                    // Reduced, the new color fades in over the old.
                    let (share, fade) = if reduce { (1., e) } else { (e, 1.) };
                    slot.child(item(share, fade, mix(from_label, to_label, e)))
                },
            )
            .into_any_element()
    });
    let place = match (step != SetupStep::Welcome).then(|| step.stage()) {
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
                .children(items),
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
        .accent_key(t)
        .text_color(t.setup.on_light)
        .typeset(Type::HERO.sized(size * 58. / 112.).weighted(weight::STRONG))
        .when(lit, |tile| {
            tile.shadow(vec![
                BoxShadow::new(px(0.), px(2.), gpui::white().opacity(0.4))
                    .inset(),
                BoxShadow::new(px(0.), px(-3.), t.depth.accent_edge).inset(),
                BoxShadow::new(px(0.), px(0.), t.accent.opacity(0.08))
                    .spread_radius(px((size / 11.).max(6.))),
                BoxShadow::new(px(0.), px(24.), t.accent.opacity(0.22))
                    .blur_radius(px(60.)),
            ])
        })
        .child("τ")
}

/// A company's mark, from its own artwork when it was built in; else a
/// plain outlined circle holding its place. See [`Brand`]. `turn` is in
/// radians, for an icon swapping in.
fn brand_mark(
    brand: Brand,
    size: f32,
    color: Hsla,
    turn: f32,
    t: &Theme,
) -> AnyElement {
    match brand.svg() {
        Some(_) => svg()
            .path(brand.path())
            .size(px(size))
            .flex_shrink_0()
            .text_color(color)
            .with_transformation(Transformation::rotate(radians(turn)))
            .into_any_element(),
        None => div()
            .size(px(size))
            .flex_shrink_0()
            .rounded(px(size / 2.))
            .border(px(1.5))
            .border_color(t.setup.faint)
            .into_any_element(),
    }
}

/// What stands for `service` in its tile, `tile` wide, at `scale` and
/// turned by `turn` radians.
fn service_icon(
    service: Service,
    tile: f32,
    scale: f32,
    turn: f32,
    t: &Theme,
) -> AnyElement {
    let glyph = |glyph: Icon, share: f32| {
        svg()
            .path(glyph.path())
            .size(px(tile * share))
            .flex_shrink_0()
            .text_color(t.text_soft)
            .with_transformation(
                Transformation::scale(area(scale, scale))
                    .with_rotation(radians(turn)),
            )
            .into_any_element()
    };
    match service {
        Service::GitHub => {
            brand_mark(Brand::GitHub, tile * 0.57 * scale, t.text, turn, t)
        }
        Service::ChatGpt | Service::None => {
            brand_mark(Brand::ChatGpt, tile * 0.57 * scale, t.text, turn, t)
        }
        Service::Token => glyph(Icon::Key, 0.41),
        Service::Repos => glyph(Icon::Repo, 0.42),
        Service::FirstRun => glyph(Icon::Runs, 0.42),
    }
}

/// The other side's tile, edged by how the link stands. It breathes with
/// the comet while tau waits on it, and its icon swaps in when the
/// service changes.
fn service_tile(size: f32, beat: Beat, t: &Theme) -> AnyElement {
    let look = &t.setup;
    let edge = match beat.link {
        Link::Waiting | Link::Declined => t.accent_border,
        Link::Connected if beat.service == Service::ChatGpt => look.green_edge,
        Link::Blocked => t.red_border,
        _ => t.border,
    };
    let inner = if beat.service_changed && !beat.reduce {
        let (service, tile, theme) = (beat.service, size, t.clone());
        div()
            .with_animation(
                SharedString::from(format!("swap-{}", beat.service_epoch)),
                Animation::new(Duration::from_millis(560)),
                move |slot, t| {
                    let ms = t * 560.;
                    if ms < 180. {
                        return slot.opacity(0.);
                    }
                    let e = curve::swap()((ms - 180.) / 380.);
                    slot.opacity(e.clamp(0., 1.)).child(service_icon(
                        service,
                        tile,
                        lerp(0.7, 1., e),
                        lerp(-8., 0., e).to_radians(),
                        &theme,
                    ))
                },
            )
            .into_any_element()
    } else {
        service_icon(beat.service, size, 1., 0., t)
    };
    let radius = size * 30. / 112.;
    let tile = div()
        .size(px(size))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(radius))
        .bevel(look.surface_top, look.tile, t)
        .border_1()
        .border_color(edge)
        .child(inner);
    if beat.link != Link::Waiting || !beat.loops {
        return tile.into_any_element();
    }
    let (quiet, lit, glow) = (t.border, look.waiting_edge, t.accent);
    tile.with_animation(
        "service-breath",
        loop_of(1400, 0.),
        move |tile, phase| {
            let v = breath(phase);
            tile.border_color(mix(quiet, lit, v)).shadow(vec![
                BoxShadow::new(px(0.), px(0.), glow.opacity(0.07 * v))
                    .spread_radius(px(6. * v)),
            ])
        },
    )
    .into_any_element()
}

/// Dashes 4 px long every 10 px, as the prototype's CSS draws them,
/// `offset` px along toward the service.
fn dash_row(line: f32, offset: f32, alpha: f32, t: &Theme) -> Div {
    let count = (line / 10.).ceil() as usize + 2;
    div()
        .absolute()
        .top_0()
        .left(px(offset - 10.))
        .flex()
        .gap(sp(1.5))
        .children((0..count).map(|_| {
            div()
                .w(px(4.))
                .h(px(2.))
                .flex_shrink_0()
                .bg(t.accent.opacity(alpha))
        }))
}

/// The dashes, clipped to the line; drifting while loops run.
fn dashes(line: f32, alpha: f32, drift: bool, t: &Theme) -> AnyElement {
    let track = div()
        .absolute()
        .left_0()
        .top_0()
        .w(px(line))
        .h(px(2.))
        .overflow_hidden();
    if !drift {
        return track
            .child(dash_row(line, 10., alpha, t))
            .into_any_element();
    }
    let theme = t.clone();
    track
        .with_animation("dash-drift", loop_of(1100, 0.), move |track, phase| {
            // 20 px a loop: two dashes' worth.
            track.child(dash_row(line, (phase * 20.) % 10., alpha, &theme))
        })
        .into_any_element()
}

/// The comet: a glowing streak with a fading tail, from τ to the service
/// every 1.4 s.
fn comet(line: f32, t: &Theme) -> AnyElement {
    let amber = t.accent;
    let (length, thick) = COMET;
    div()
        .absolute()
        .top_0()
        .w(px(length))
        .h(px(thick))
        .rounded(px(thick / 2.))
        .bg(linear_gradient(
            90.,
            linear_color_stop(amber.opacity(0.), 0.),
            linear_color_stop(amber.opacity(0.9), 1.),
        ))
        .shadow(vec![
            BoxShadow::new(px(0.), px(0.), amber.opacity(0.8))
                .blur_radius(px(16.)),
        ])
        .with_animation("comet", loop_of(1400, 0.), move |comet, t| {
            let x = lerp(-length, line, curve::travel()(t));
            let shown = if t < 0.15 {
                t / 0.15
            } else if t > 0.85 {
                (1. - t) / 0.15
            } else {
                1.
            };
            comet.left(px(x)).opacity(shown)
        })
        .into_any_element()
}

/// The ✓, !, or ✕ in the middle of the line. It pops when the link just
/// changed to it.
fn medallion(size: Size, beat: Beat, color: Hsla, t: &Theme) -> AnyElement {
    let side = if size.tile < 100. { 32. } else { 40. };
    let ground = t.setup.ground;
    let link = beat.link;
    let build = move |scale: f32| {
        let side = side * scale;
        let mark = match link {
            Link::Connected => icon(Icon::Check, IconSize(18. * scale), color)
                .into_any_element(),
            Link::Declined => div()
                .typeset(
                    Type::HEADING.sized(20. * scale).weighted(weight::STRONG),
                )
                .text_color(color)
                .child("!")
                .into_any_element(),
            _ => icon(Icon::Close, IconSize(18. * scale), color)
                .into_any_element(),
        };
        div()
            .size(px(side))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(side / 2.))
            .bg(ground)
            .border(px(2.))
            .border_color(color)
            .child(mark)
    };
    if !beat.link_changed {
        return build(1.).into_any_element();
    }
    let reduce = beat.reduce;
    // The connection pops after the line has drawn; a refusal sooner.
    let delay = if link == Link::Connected { 260. } else { 180. };
    div()
        .size(px(side))
        .flex()
        .items_center()
        .justify_center()
        .with_animation(
            SharedString::from(format!("medal-{}", beat.link_epoch)),
            Animation::new(Duration::from_millis(700)),
            move |slot, t| {
                let ms = t * 700.;
                if reduce {
                    return slot.opacity((ms / 200.).min(1.)).child(build(1.));
                }
                if ms < delay {
                    return slot.opacity(0.);
                }
                let (scale, shown) =
                    pop(((ms - delay) / 420.).min(1.), &curve::pop());
                slot.opacity(shown).child(build(scale))
            },
        )
        .into_any_element()
}

/// The words over and under the line.
type Words = (Option<&'static str>, Option<&'static str>);

/// The line between the tiles, with a word above it and one under it.
fn line(size: Size, beat: Beat, words: Words, t: &Theme) -> Div {
    let look = &t.setup;
    let link = beat.link;
    let (color, below_color) = match link {
        Link::Idle => (t.accent, look.faint),
        Link::Waiting | Link::Declined => (t.accent, t.accent),
        Link::Connected => (t.green, t.green),
        Link::Blocked => (t.red, t.red),
    };
    let fresh = beat.link_changed && !beat.reduce;
    let middle = size.tile / 2.;
    // A layer the width of the line, 2 px high, on its middle.
    let rail = || {
        div()
            .absolute()
            .left_0()
            .top(px(middle - 1.))
            .w(px(size.line))
            .h(px(2.))
    };
    let track = div()
        .w(px(size.line))
        .h(px(size.tile))
        .flex_shrink_0()
        .relative()
        .flex()
        .items_center()
        .justify_center();
    let track = match link {
        Link::Idle => {
            track.child(rail().child(dashes(size.line, 0.55, beat.loops, t)))
        }
        Link::Waiting => {
            let track =
                track.child(rail().child(dashes(size.line, 0.28, false, t)));
            if beat.loops {
                track.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(middle - COMET.1 / 2.))
                        .w(px(size.line))
                        .h(px(COMET.1))
                        .child(comet(size.line, t)),
                )
            } else {
                // Still: a lit dot in its halo.
                track.child(
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
                        ),
                )
            }
        }
        Link::Connected => {
            let green = t.green;
            let solid = move |share: f32| {
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .w(relative(share))
                    .h(px(2.))
                    .bg(green)
                    .shadow(vec![
                        BoxShadow::new(px(0.), px(0.), green.opacity(0.55))
                            .blur_radius(px(18.)),
                    ])
            };
            let drawn = if fresh {
                rail()
                    .with_animation(
                        SharedString::from(format!("draw-{}", beat.link_epoch)),
                        after(0, 300, curve::settle()),
                        move |rail, e| rail.child(solid(e)),
                    )
                    .into_any_element()
            } else {
                rail().child(solid(1.)).into_any_element()
            };
            track.child(drawn).child(medallion(size, beat, color, t))
        }
        Link::Declined | Link::Blocked => {
            // Broken: the halves pull apart; a refusal wider.
            let rest = if link == Link::Declined { 0.38 } else { 0.34 };
            let halves = move |rail: Div, share: f32| {
                rail.child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .w(relative(share))
                        .h(px(2.))
                        .bg(color.opacity(0.8)),
                )
                .child(
                    div()
                        .absolute()
                        .right_0()
                        .top_0()
                        .w(relative(share))
                        .h(px(2.))
                        .bg(color.opacity(0.25)),
                )
            };
            let broken = if fresh {
                rail()
                    .with_animation(
                        SharedString::from(format!(
                            "break-{}",
                            beat.link_epoch
                        )),
                        after(0, 400, curve::settle()),
                        move |rail, e| halves(rail, lerp(0.5, rest, e)),
                    )
                    .into_any_element()
            } else {
                halves(rail(), rest).into_any_element()
            };
            // Declined while waiting: the comet stops halfway and fades.
            let stopped = (fresh
                && link == Link::Declined
                && beat.link_from == Link::Waiting)
                .then(|| {
                    let amber = t.accent;
                    let (length, thick) = COMET;
                    div()
                        .absolute()
                        .top(px(middle - thick / 2.))
                        .left(px(size.line / 2. - length))
                        .w(px(length))
                        .h(px(thick))
                        .rounded(px(thick / 2.))
                        .bg(linear_gradient(
                            90.,
                            linear_color_stop(amber.opacity(0.), 0.),
                            linear_color_stop(amber.opacity(0.9), 1.),
                        ))
                        .with_animation(
                            SharedString::from(format!(
                                "halt-{}",
                                beat.link_epoch
                            )),
                            after(0, 450, curve::settle()),
                            |comet, e| comet.opacity(1. - e),
                        )
                });
            track
                .child(broken)
                .children(stopped)
                .child(medallion(size, beat, color, t))
        }
    };
    let inset = if size.tile < 100. { 4. } else { 16. };
    let word = |text: &'static str, color: Hsla| {
        div()
            .absolute()
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(mono(text, Type::MICRO, color))
    };
    let below = words.1.map(|text| {
        let word = word(text, below_color).bottom(px(inset));
        if fresh && link == Link::Connected {
            word.with_animation(
                SharedString::from(format!("linkword-{}", beat.link_epoch)),
                after(500, 320, curve::rise()),
                move |word, e| {
                    word.opacity(e).bottom(px(inset + 12. * (1. - e)))
                },
            )
            .into_any_element()
        } else {
            word.into_any_element()
        }
    });
    track
        .when_some(words.0, |track, text| {
            track.child(word(text, t.dim).top(px(inset)))
        })
        .children(below)
}

/// τ, the line and the other side, at `size`.
fn handshake_at(size: Size, beat: Beat, words: Words, t: &Theme) -> Div {
    div()
        .pt(px(size.top))
        .flex()
        .items_center()
        .justify_center()
        .child(tau_tile(size.tile, t))
        .child(line(size, beat, words, t))
        .child(service_tile(size.tile, beat, t))
}

/// The handshake. It is the same from step to step: moving between a
/// big one and a small one, it shrinks or grows and slides into place.
fn handshake(
    ws: &Workspace,
    words: Words,
    compact: bool,
    t: &Theme,
) -> AnyElement {
    let motion = &ws.setup_motion;
    let beat = Beat::of(motion);
    let (from, to) = (
        Size::of(motion.scale.previous, compact),
        Size::of(motion.scale.current, compact),
    );
    if !motion.scale.changed() || from == to || motion.reduce {
        return handshake_at(to, beat, words, t).into_any_element();
    }
    let theme = t.clone();
    div()
        .with_animation(
            SharedString::from(format!("handshake-{}", motion.scale.epoch)),
            after(0, 550, curve::settle()),
            move |slot, e| {
                slot.child(handshake_at(
                    Size::between(from, to, e),
                    beat,
                    words,
                    &theme,
                ))
            },
        )
        .into_any_element()
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
        .flex()
        .flex_col()
        .items_center()
        .child(element)
        .with_animation(
            SharedString::from(format!("rise-{}-{key}", motion.scene.epoch)),
            entrance(delay, reduce),
            move |slot, e| {
                let slot = slot.opacity(e.clamp(0., 1.));
                if reduce {
                    slot
                } else {
                    slot.relative().top(px(12. * (1. - e)))
                }
            },
        )
        .into_any_element()
}

// Words.

/// The headline and the sentence under it, centered, rising in turn.
fn headline(
    motion: &SetupMotion,
    delay: u64,
    (title, sub): (&str, &str),
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
        .child(rise(
            motion,
            "title",
            delay,
            div()
                .typeset(if compact {
                    Type::DISPLAY.weighted(weight::EMPHASIS)
                } else {
                    style
                })
                .leading(1.1)
                .text_color(t.setup.light)
                .child(title.to_owned()),
        ))
        .child(rise(
            motion,
            "sub",
            delay + 40,
            div()
                .max_w(px(width))
                .typeset(if compact { Type::BODY } else { Type::SUBTITLE })
                .leading(1.6)
                .text_color(t.muted)
                .child(sub.to_owned()),
        ))
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
        .bevel(look.light, look.light_foot, t)
        .text_color(look.on_light)
        .typeset(Type::LEAD.weighted(weight::STRONG))
        .cursor_pointer()
        .hover(|style| style.opacity(0.92))
        .shadow(vec![
            BoxShadow::new(px(0.), px(1.), gpui::white()).inset(),
            BoxShadow::new(px(0.), px(-2.), gpui::black().opacity(0.12))
                .inset(),
            BoxShadow::new(px(0.), px(2.), gpui::black().opacity(0.5))
                .blur_radius(px(3.)),
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
        Some(brand_mark(Brand::ChatGpt, 22., t.setup.on_light, 0., t)),
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
        .bevel(t.setup.surface_top, t.setup.surface, t)
        .text_color(t.text_soft)
        .typeset(Type::BODY.weighted(weight::EMPHASIS))
        .cursor_pointer()
        .hover(|style| style.border_color(t.border_strong))
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
        .lifted(t)
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
        .bevel(look.surface_top, look.surface, t)
        .border_1()
        .border_color(look.surface_border)
        .child(
            div()
                .size(px(30.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(radius::FULL)
                .sunk(look.field, t)
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
) -> Div {
    let motion = &ws.setup_motion;
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
    let hero = if compact {
        let size = Size::of(Scale::Big, true);
        div().pt(px(size.top)).child(tau_tile(size.tile + 8., t))
    } else {
        orbit(t)
    };
    div()
        .child(hero)
        .child(
            headline(
                motion,
                0,
                (
                    "Coding agents on your repositories",
                    "tau runs agents on the repositories you choose, on your \
                     ChatGPT plan. Every turn is a change you can go back to, \
                     fork or compare.",
                ),
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
                .child(rise(
                    motion,
                    "cards",
                    80,
                    div()
                        .flex()
                        .justify_center()
                        .gap(sp(3.5))
                        .when(compact, |row| {
                            row.w_full().flex_col().gap(sp(2.5))
                        })
                        .children(cards),
                ))
                .child(rise(
                    motion,
                    "actions",
                    120,
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
                                        0.,
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
                ))
                .child(rise(
                    motion,
                    "note",
                    160,
                    footnote(
                        format!(
                            "Takes about a minute · tokens stay in {}, \
                             readable only by you",
                            ws.setup.config
                        ),
                        t,
                    ),
                )),
        )
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
            .child(
                div()
                    .size(px(SAT))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(SAT * 30. / 112.))
                    .bevel(t.setup.surface_top, t.setup.tile, t)
                    .border_1()
                    .border_color(t.border)
                    .child(inner),
            )
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
                .child(div().w(relative(0.5)).h_full().bg(linear_gradient(
                    90.,
                    linear_color_stop(faint, 0.),
                    linear_color_stop(strong, 1.),
                )))
                .child(div().w(relative(0.5)).h_full().bg(linear_gradient(
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
            brand_mark(Brand::GitHub, 40., t.text, 0., t),
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
            brand_mark(Brand::ChatGpt, 40., t.text, 0., t),
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

fn open_device_page(code: &DeviceCode, cx: &mut App) {
    cx.write_to_clipboard(ClipboardItem::new_string(code.code.clone()));
    cx.open_url(&format!("https://{}", code.url));
}

/// The code, a character to a cell. The characters rise in turn as the
/// code arrives, and flash once each time it is copied.
fn code_cells(
    code: &str,
    motion: &SetupMotion,
    compact: bool,
    t: &Theme,
) -> Div {
    let (w, h, style) = if compact {
        (30., 42., Type::HEADLINE.mono())
    } else {
        (50., 64., Type::CODE_CELL)
    };
    let (top, surface, border, flash) =
        (t.setup.surface_top, t.setup.surface, t.border, t.accent);
    let copies = motion.copies;
    let reduce = motion.reduce;
    div()
        .flex()
        .items_center()
        .gap(sp(if compact { 1. } else { 2. }))
        .children(code.chars().enumerate().map(|(n, ch)| {
            let cell = if ch == '-' {
                div()
                    .w(px(if compact { 10. } else { 16. }))
                    .h(px(2.))
                    .bg(t.border_strong)
                    .into_any_element()
            } else {
                let cell = div()
                    .w(px(w))
                    .h(px(h))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(radius::CARD)
                    .bevel(top, surface, t)
                    .border_1()
                    .border_color(border)
                    .child(mono(ch.to_string(), style, t.setup.light));
                if copies > 0 {
                    cell.with_animation(
                        SharedString::from(format!("flash-{copies}-{n}")),
                        after(stagger(0, 25, n), 600, curve::ease_in_out()),
                        move |cell, e| {
                            // Back to its bevel once the flash is over.
                            if e >= 1. {
                                return cell;
                            }
                            cell.bg(mix(flash.opacity(0.35), surface, e))
                                .border_color(mix(flash, border, e))
                        },
                    )
                    .into_any_element()
                } else {
                    cell.into_any_element()
                }
            };
            div().child(cell).with_animation(
                SharedString::from(format!("code-{code}-{n}")),
                entrance(stagger(0, 40, n), reduce),
                move |slot, e| {
                    let slot = slot.opacity(e.clamp(0., 1.));
                    if reduce {
                        slot
                    } else {
                        slot.relative().top(px(12. * (1. - e)))
                    }
                },
            )
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
) -> Div {
    let motion = &ws.setup_motion;
    let code = code_or_status(&ws.setup.github).ok().cloned();
    let below = match &ws.setup.github {
        GitHub::SignedIn { .. } => "signed in",
        GitHub::Failed(_) => "failed",
        _ => "waiting for approval",
    };
    let shown = match (&code, code_or_status(&ws.setup.github)) {
        (Some(code), _) => div()
            .id("copy-code")
            .cursor_pointer()
            .child(code_cells(&code.code, motion, compact, t))
            .on_click({
                let code = code.code.clone();
                cx.listener(move |ws, _, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(
                        code.clone(),
                    ));
                    ws.setup_motion.copies += 1;
                    cx.notify();
                })
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
    div()
        .child(handshake(
            ws,
            (Some("device sign-in"), Some(below)),
            compact,
            t,
        ))
        .child(
            headline(
                motion,
                0,
                (
                    "Enter this code on GitHub",
                    "Open GitHub, type the code, and approve the tau app. \
                     This screen moves on by itself.",
                ),
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
                .child(rise(
                    motion,
                    "actions",
                    80,
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
                                .on_click(cx.listener(move |ws, _, _, cx| {
                                    if let Some(code) = &open {
                                        open_device_page(code, cx);
                                        ws.setup_motion.copies += 1;
                                        cx.notify();
                                    }
                                })),
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
                ))
                .children(status)
                .child(rise(motion, "permissions", 120, permissions))
                .children(code.as_ref().map(|code| {
                    rise(
                        motion,
                        "where",
                        160,
                        mono(
                            format!(
                                "{} · code expires in {} · only on the \
                                 repositories you pick next",
                                code.url, code.expires
                            ),
                            Type::CAPTION,
                            t.setup.faint,
                        )
                        .text_center(),
                    )
                }))
                .child(skip_or_back(ws, t, cx)),
        )
}

fn token(
    ws: &Workspace,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let motion = &ws.setup_motion;
    let look = &t.setup;
    let checking = ws.setup.github == GitHub::Checking;
    let failed = match &ws.setup.github {
        GitHub::Failed(error) => Some(error.clone()),
        _ => None,
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
    div()
        .child(handshake(
            ws,
            (Some("personal access token"), None),
            compact,
            t,
        ))
        .child(
            headline(
                motion,
                0,
                (
                    "Use a personal access token",
                    "A fine-grained token works in place of the GitHub App. \
                     Give it these repository permissions, then paste it \
                     here.",
                ),
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
                .child(rise(motion, "form", 80, form))
                .child(rise(
                    motion,
                    "actions",
                    120,
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
                )),
        )
}

/// The model step, in whichever state the ChatGPT sign-in is.
fn model(
    ws: &Workspace,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let motion = &ws.setup_motion;
    // Once connected, the words wait for the line to draw.
    let (words, words_at, title, sub, below) = match &ws.setup.model {
        ModelAccess::None | ModelAccess::Failed(_) => (
            (Some("plan usage"), Some("not connected")),
            0,
            "Connect tau to ChatGPT",
            "Runs use your ChatGPT plan through OpenAI's Responses API. One \
             sign-in in the browser; nothing to paste, no key to keep.",
            model_start(ws, compact, t, cx),
        ),
        ModelAccess::SigningIn { url } => (
            (Some("plan usage"), Some("waiting for the browser")),
            0,
            "Finish in your browser",
            "A ChatGPT page just opened. Allow plan use there and this screen \
             moves on by itself.",
            model_waiting(ws, url.clone(), compact, t, cx),
        ),
        ModelAccess::Connected { .. } => (
            (None, Some("connected")),
            350,
            "You're using your ChatGPT plan",
            "Eligible usage in tau now counts toward your plan. Pick the model \
             runs start with; you can change it per run.",
            model_signed_in(ws, compact, t, cx),
        ),
        ModelAccess::PlanDisabled { account } => (
            (None, Some("plan use off")),
            0,
            "Allow tau to use your plan",
            "You're signed in, but plan use wasn't allowed. tau runs only on \
             your ChatGPT plan, so it can't start a run without it.",
            model_declined(motion, account, compact, t, cx),
        ),
        ModelAccess::NotEligible { account, detail } => (
            (None, Some("not eligible")),
            0,
            "This account can't share its plan",
            "OpenAI says ChatGPT plan use isn't available for this account or \
             workspace. Usually because:",
            model_not_eligible(motion, account, detail, compact, t, cx),
        ),
    };
    div()
        .child(handshake(ws, words, compact, t))
        .child(
            headline(
                motion,
                words_at,
                (title, sub),
                Type::HERO,
                560.,
                compact,
                t,
            )
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
        })
}

fn model_start(
    ws: &Workspace,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let motion = &ws.setup_motion;
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
        .child(rise(
            motion,
            "sign-in",
            80,
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
        ))
        .child(rise(motion, "timeline", 120, timeline))
        .child(rise(
            motion,
            "note",
            160,
            footnote(
                "Needs ChatGPT Plus or Pro · tau can't read your \
                 conversations",
                t,
            ),
        ))
}

fn model_waiting(
    ws: &Workspace,
    url: Option<String>,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let motion = &ws.setup_motion;
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
        .child(div().typeset(Type::SMALL).text_color(t.muted).child(
            "Signing in on another device? Paste the address the browser \
             ended on.",
        ))
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
                        .sunk(look.field, t)
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
        .child(rise(
            motion,
            "actions",
            80,
            actions(compact).child(again).child(
                div()
                    .id("chatgpt-cancel")
                    .child(text_button("Cancel", t))
                    .on_click(cx.listener(|ws, _, _, cx| {
                        ws.cancel_chatgpt_sign_in(cx)
                    })),
            ),
        ))
        .child(rise(motion, "paste", 120, paste))
        .child(rise(
            motion,
            "note",
            160,
            mono(
                "listening on this machine · nothing is saved until the \
                 browser comes back",
                Type::CAPTION,
                look.faint,
            )
            .text_center(),
        ))
}

fn model_signed_in(
    ws: &Workspace,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let motion = &ws.setup_motion;
    let look = &t.setup;
    let models = &ws.catalog().models;
    let current = models.settings.default_for("coder").model;
    let shown = models.shown("");
    let pills: Vec<_> = shown
        .iter()
        .enumerate()
        .map(|(n, option)| {
            let picked = option.id == current;
            let id = option.id.clone();
            let pill = div()
                .id(SharedString::from(format!("setup-model-{id}")))
                .relative()
                .px(sp(3.))
                .py(sp(1.75))
                .rounded(radius::FULL)
                .border_1()
                .border_color(if picked { t.accent } else { t.border })
                .when(picked, |chip| chip.bg(t.accent.opacity(0.08)))
                .when(!picked, |chip| {
                    chip.bevel(look.surface_top, look.surface, t)
                })
                .cursor_pointer()
                .hover(|style| style.border_color(t.accent_border).top(px(-1.)))
                .child(mono(
                    option.id.clone(),
                    Type::CODE,
                    if picked { t.accent } else { look.soft },
                ))
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.pick_setup_model(&id, cx)
                }));
            // Rising one after another, 60 ms apart.
            let reduce = motion.reduce;
            div()
                .child(pill)
                .with_animation(
                    SharedString::from(format!(
                        "pill-{}-{n}",
                        motion.scene.epoch
                    )),
                    entrance(stagger(550, 60, n), reduce),
                    move |slot, e| {
                        let slot = slot.opacity(e.clamp(0., 1.));
                        if reduce {
                            slot
                        } else {
                            slot.relative().top(px(12. * (1. - e)))
                        }
                    },
                )
                .into_any_element()
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
        div()
            .max_w(px(560.))
            .text_center()
            .typeset(Type::SMALL)
            .text_color(look.faint)
            .child("Every model is hidden; the Models screen shows them again.")
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
            rise(
                motion,
                "account",
                450,
                account_chip(&account.label, "plan use allowed", t.green, t),
            )
        }))
        .child(listing)
        .child(rise(
            motion,
            "actions",
            900,
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
        ))
}

fn model_declined(
    motion: &SetupMotion,
    account: &str,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    div()
        .gap(sp(6.5))
        .child(rise(
            motion,
            "account",
            80,
            account_chip(account, "plan use off", t.accent, t),
        ))
        .child(rise(
            motion,
            "actions",
            120,
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
        ))
        .child(rise(
            motion,
            "note",
            160,
            footnote(
                "Opens the same ChatGPT page, asking only for plan use · take \
                 it back any time in ChatGPT settings",
                t,
            ),
        ))
}

fn model_not_eligible(
    motion: &SetupMotion,
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
            col.child(rise(
                motion,
                "account",
                80,
                account_chip(account, "not eligible", t.red, t),
            ))
        })
        .child(rise(
            motion,
            "reasons",
            120,
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
                        .bevel(look.surface_top, look.surface, t)
                        .border_1()
                        .border_color(look.surface_border)
                        .typeset(Type::SMALL)
                        .text_color(look.soft)
                        .child(reason)
                })),
        ))
        .child(rise(
            motion,
            "actions",
            160,
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
        ))
        .child(rise(
            motion,
            "refusal",
            200,
            mono(detail.to_owned(), Type::CAPTION, look.refusal).text_center(),
        ))
}

/// A checkbox: 18 px, a little smaller while empty, filling with a small
/// bounce.
fn tick_box(name: &str, checked: bool, reduce: bool, t: &Theme) -> AnyElement {
    const SIDE: f32 = 18.;
    let (ink, floor, theme) = (t.setup.on_light, t.setup.field, t.clone());
    let build = move |scale: f32| {
        let tile = div()
            .size(px(SIDE * scale))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded(radius::TAG);
        if checked {
            tile.accent_key(&theme).child(icon(
                Icon::Check,
                IconSize::SMALL,
                ink,
            ))
        } else {
            tile.sunk(floor, &theme)
        }
    };
    let slot = div()
        .size(px(SIDE))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center();
    if !checked {
        return slot.child(build(0.92)).into_any_element();
    }
    if reduce {
        return slot.child(build(1.)).into_any_element();
    }
    slot.with_animation(
        SharedString::from(format!("tick-{name}")),
        after(0, 180, curve::bounce()),
        move |slot, e| slot.child(build(lerp(0.92, 1., e))),
    )
    .into_any_element()
}

fn repos(
    ws: &Workspace,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let motion = &ws.setup_motion;
    let look = &t.setup;
    let filter = ws.repo_filter.read(cx).text().to_owned();
    let rows: Vec<_> = ws
        .setup
        .matching(&filter)
        .enumerate()
        .map(|(n, repo)| {
            let name = repo.name.clone();
            let row = div()
                .id(("repo", n))
                .w_full()
                .flex()
                .items_center()
                .gap(sp(3.5))
                .px(sp(4.5))
                .py(sp(3.))
                .when(n > 0, |row| row.border_t_1().border_color(look.divider))
                .cursor_pointer()
                .when(repo.selected, |row| row.bg(t.accent.opacity(0.05)))
                .hover(|style| style.bg(t.accent.opacity(0.03)))
                .child(tick_box(&repo.name, repo.selected, motion.reduce, t))
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
                );
            // The rows slide in one after another.
            rise(
                motion,
                &format!("row-{}", repo.name),
                460 + 50 * n.min(10) as u64,
                row,
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
    div()
        .child(handshake(ws, (None, None), compact, t))
        .child(
            headline(
                motion,
                300,
                (
                    "Pick repositories",
                    "tau clones each into its own storage, as a jj \
                     repository. Runs work there, never in your checkouts.",
                ),
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
                .child(rise(motion, "panel", 400, panel))
                .child(rise(
                    motion,
                    "actions",
                    800,
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
                ))
                .child(footnote(
                    format!(
                        "Cloned into {} · every turn of a run is a commit \
                         you can go back to, fork from, or compare",
                        ws.setup.storage
                    ),
                    t,
                )),
        )
}

/// A clone's progress bar: it follows the clone's real progress, easing
/// to each new share, with a sheen running along it while it clones.
fn clone_bar(
    name: &str,
    motion: &SetupMotion,
    color: Hsla,
    cloning: bool,
    t: &Theme,
) -> AnyElement {
    let track = t.setup.divider;
    let Some(tracked) = motion.clones.get(name) else {
        return bar(1., 4., color, track).into_any_element();
    };
    let (from, to) = (
        tracked.previous.0 as f32 / 1000.,
        tracked.current.0 as f32 / 1000.,
    );
    let sheen = cloning && motion.loops();
    let light = t.setup.light;
    let build = move |share: f32| {
        div()
            .relative()
            .h(px(4.))
            .w_full()
            .rounded(radius::FULL)
            .bg(track)
            .child(
                div()
                    .relative()
                    .h_full()
                    .w(relative(share.clamp(0., 1.)))
                    .rounded(radius::FULL)
                    .overflow_hidden()
                    .bg(color)
                    .when(sheen, |fill| {
                        fill.child(
                            div()
                                .absolute()
                                .top_0()
                                .h_full()
                                .w(relative(0.3))
                                .bg(linear_gradient(
                                    90.,
                                    linear_color_stop(light.opacity(0.), 0.),
                                    linear_color_stop(light.opacity(0.45), 1.),
                                ))
                                .with_animation(
                                    "sheen",
                                    loop_of(1600, 0.),
                                    |sheen, p| {
                                        sheen.left(relative(p * 1.3 - 0.3))
                                    },
                                ),
                        )
                    }),
            )
    };
    if from == to || motion.reduce {
        return build(to).into_any_element();
    }
    div()
        .w_full()
        .with_animation(
            SharedString::from(format!("bar-{name}-{}", tracked.epoch)),
            after(0, 400, curve::settle()),
            move |slot, e| slot.child(build(lerp(from, to, e))),
        )
        .into_any_element()
}

fn ready(
    ws: &Workspace,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let motion = &ws.setup_motion;
    let look = &t.setup;
    let clones = ws.setup.clones.iter().map(|clone| {
        let (state, color, cloning) = match &clone.state {
            CloneState::Cloning { share, detail } => (
                if detail.is_empty() {
                    format!("cloning · {:.0}%", share * 100.)
                } else {
                    format!("cloning · {detail}")
                },
                t.accent,
                true,
            ),
            CloneState::Ready => ("ready".to_owned(), t.green, false),
            CloneState::Failed(error) => (error.clone(), t.red, false),
        };
        let card = glass(t)
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
            .child(clone_bar(&clone.name, motion, color, cloning, t));
        // Done: the card flashes green, once.
        let finished = motion.clones.get(&clone.name).filter(|tracked| {
            tracked.current.1 && !tracked.previous.1 && !motion.reduce
        });
        match finished {
            Some(tracked) => {
                let (green, glass, edge) =
                    (t.green, look.glass, look.surface_border);
                card.with_animation(
                    SharedString::from(format!(
                        "done-{}-{}",
                        clone.name, tracked.epoch
                    )),
                    after(0, 900, curve::ease_in_out()),
                    move |card, e| {
                        card.border_color(mix(green, edge, e)).bg(mix(
                            green.opacity(0.14),
                            glass,
                            e,
                        ))
                    },
                )
                .into_any_element()
            }
            None => card.into_any_element(),
        }
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
    // The ring grows while the field has the keyboard.
    let ring = |focused: bool| if focused { 7. } else { 4. };
    let composing = &motion.composing;
    let (ring_from, ring_to) =
        (ring(composing.previous), ring(composing.current));
    let (amber, shade) = (t.accent, gpui::black());
    let halo = move |spread: f32| {
        vec![
            BoxShadow::new(
                px(0.),
                px(0.),
                amber.opacity(0.012 * spread + 0.03),
            )
            .spread_radius(px(spread)),
            BoxShadow::new(px(0.), px(30.), shade.opacity(0.5))
                .blur_radius(px(80.)),
        ]
    };
    let composer = div()
        .w_full()
        .max_w(px(720.))
        .flex()
        .flex_col()
        .rounded(radius::SHEET)
        .sunk(look.field, t)
        .border_1()
        .border_color(t.border_strong)
        .shadow(halo(ring_to))
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
    let composer = if composing.changed() && !motion.reduce {
        composer
            .with_animation(
                SharedString::from(format!("focus-{}", composing.epoch)),
                after(0, 250, curve::settle()),
                move |composer, e| {
                    composer.shadow(halo(lerp(ring_from, ring_to, e)))
                },
            )
            .into_any_element()
    } else {
        composer.into_any_element()
    };
    div()
        .child(handshake(ws, (None, None), compact, t))
        .child(
            headline(
                motion,
                300,
                (
                    "Start your first run",
                    "Describe a task. tau works on a new change in the \
                     repository; open a pull request when you like the \
                     result.",
                ),
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
                    col.child(rise(
                        motion,
                        "clones",
                        380,
                        div()
                            .flex()
                            .flex_wrap()
                            .justify_center()
                            .gap(sp(3.5))
                            .when(compact, |row| {
                                row.w_full().flex_col().gap(sp(2.5))
                            })
                            .children(clones),
                    ))
                })
                .child(rise(motion, "composer", 440, composer))
                .child(rise(
                    motion,
                    "note",
                    500,
                    footnote(
                        "⏎ to start · every turn is a commit you can go back \
                         to, fork from, or compare",
                        t,
                    ),
                )),
        )
}
