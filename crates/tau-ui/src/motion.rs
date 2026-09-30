//! Onboarding's motion: the curves, and what changed on screen since the
//! last frame.
//!
//! Every transition starts from a real change of state. The workspace
//! records what onboarding shows each frame in a [`SetupMotion`]; a value
//! that changed gets a new epoch, and the elements that animate it put
//! the epoch in their id, so GPUI starts their animation then and only
//! then. Loops (the rings breathing, the dashes drifting, the comet) run
//! only while the window is focused and motion is not reduced.

use std::{
    collections::HashMap,
    path::Path,
    time::{Duration, Instant},
};

use gpui::{Animation, Hsla, Rgba};
use serde::Deserialize;

use crate::setup::SetupStep;

/// A CSS `cubic-bezier(x1, y1, x2, y2)` timing function.
pub fn cubic_bezier(
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
) -> impl Fn(f32) -> f32 + Clone + 'static {
    // Both coordinates as polynomials of the curve's parameter.
    let (cx, cy) = (3. * x1, 3. * y1);
    let (bx, by) = (3. * (x2 - x1) - cx, 3. * (y2 - y1) - cy);
    let (ax, ay) = (1. - cx - bx, 1. - cy - by);
    move |x: f32| {
        let x = x.clamp(0., 1.);
        let at = |a: f32, b: f32, c: f32, s: f32| ((a * s + b) * s + c) * s;
        // Newton's method, then halving if it strays.
        let mut s = x;
        for _ in 0..8 {
            let error = at(ax, bx, cx, s) - x;
            let slope = (3. * ax * s + 2. * bx) * s + cx;
            if error.abs() < 1e-5 || slope.abs() < 1e-6 {
                break;
            }
            s -= error / slope;
        }
        if !(0. ..=1.).contains(&s) || (at(ax, bx, cx, s) - x).abs() > 1e-3 {
            let (mut low, mut high) = (0., 1.);
            s = x;
            for _ in 0..24 {
                if at(ax, bx, cx, s) < x {
                    low = s;
                } else {
                    high = s;
                }
                s = (low + high) / 2.;
            }
        }
        at(ay, by, cy, s)
    }
}

/// The curves the prototype names, by what they do.
pub mod curve {
    use super::cubic_bezier;

    /// Things rising into place: `cubic-bezier(.2,.7,.2,1)`.
    pub fn rise() -> impl Fn(f32) -> f32 + Clone {
        cubic_bezier(0.2, 0.7, 0.2, 1.)
    }

    /// Lines drawing and bars filling, the handshake moving:
    /// `cubic-bezier(.3,.7,.2,1)`.
    pub fn settle() -> impl Fn(f32) -> f32 + Clone {
        cubic_bezier(0.3, 0.7, 0.2, 1.)
    }

    /// A pop past the end and back: `cubic-bezier(.3,1.4,.5,1)`.
    pub fn pop() -> impl Fn(f32) -> f32 + Clone {
        cubic_bezier(0.3, 1.4, 0.5, 1.)
    }

    /// A smaller overshoot, for an icon swapping in:
    /// `cubic-bezier(.3,1.3,.5,1)`.
    pub fn swap() -> impl Fn(f32) -> f32 + Clone {
        cubic_bezier(0.3, 1.3, 0.5, 1.)
    }

    /// A checkbox bouncing as it fills: `cubic-bezier(.3,1.5,.5,1)`.
    pub fn bounce() -> impl Fn(f32) -> f32 + Clone {
        cubic_bezier(0.3, 1.5, 0.5, 1.)
    }

    /// The comet's travel: `cubic-bezier(.45,0,.55,1)`.
    pub fn travel() -> impl Fn(f32) -> f32 + Clone {
        cubic_bezier(0.45, 0., 0.55, 1.)
    }

    /// A ring rippling out: `cubic-bezier(.2,.6,.3,1)`.
    pub fn ripple() -> impl Fn(f32) -> f32 + Clone {
        cubic_bezier(0.2, 0.6, 0.3, 1.)
    }

    /// CSS's `ease-in-out`.
    pub fn ease_in_out() -> impl Fn(f32) -> f32 + Clone {
        cubic_bezier(0.42, 0., 0.58, 1.)
    }
}

/// An animation that waits `delay` ms, then runs `duration` ms along
/// `curve`, once. GPUI's animations have no delay of their own: the wait
/// is the start of a longer one.
pub fn after(
    delay: u64,
    duration: u64,
    curve: impl Fn(f32) -> f32 + 'static,
) -> Animation {
    let total = (delay + duration).max(1);
    let (delay, duration) = (delay as f32, duration.max(1) as f32);
    Animation::new(Duration::from_millis(total)).with_easing(move |t| {
        let ms = t * total as f32;
        if ms <= delay {
            0.
        } else {
            curve(((ms - delay) / duration).min(1.))
        }
    })
}

/// How long an entrance runs, rising into place.
pub const ENTRANCE_MS: u64 = 320;

/// How long an entrance's plain fade runs when motion is reduced.
pub const FADE_MS: u64 = 200;

/// An element entering its screen: `delay` ms, then a rise of
/// [`ENTRANCE_MS`]; reduced, a fade of [`FADE_MS`] with no wait, so
/// nothing is held back.
pub fn entrance(delay: u64, reduce: bool) -> Animation {
    if reduce {
        after(0, FADE_MS, |t| t)
    } else {
        after(delay, ENTRANCE_MS, curve::rise())
    }
}

/// When the `n`th of a row starts entering: the first at `start`, each
/// next one `step` ms after the one before.
pub fn stagger(start: u64, step: u64, n: usize) -> u64 {
    start + step * n as u64
}

/// A loop of `period` ms, phase-locked to the app's clock so that loops
/// meant to move together do, and `offset` of a period ahead.
pub fn loop_of(period: u64, offset: f32) -> Animation {
    Animation::new(Duration::from_millis(period))
        .repeat_synced()
        .with_easing(move |t| (t + offset).fract())
}

pub fn lerp(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t
}

/// `from` blended into `to`, `t` of the way, in RGB.
pub fn mix(from: Hsla, to: Hsla, t: f32) -> Hsla {
    let (a, b): (Rgba, Rgba) = (from.into(), to.into());
    Rgba {
        r: lerp(a.r, b.r, t),
        g: lerp(a.g, b.g, t),
        b: lerp(a.b, b.b, t),
        a: lerp(a.a, b.a, t),
    }
    .into()
}

/// A breath: 0 at the ends of a loop, 1 in its middle, eased in and out.
pub fn breath(phase: f32) -> f32 {
    (1. - (phase * std::f32::consts::TAU).cos()) / 2.
}

/// Keyframes for a pop: from `.4` to `1.12` at 60 %, then to 1, each leg
/// along `curve`; opacity reaches 1 at 60 %. Returns (scale, opacity).
pub fn pop(t: f32, curve: &impl Fn(f32) -> f32) -> (f32, f32) {
    if t < 0.6 {
        let leg = curve(t / 0.6);
        (lerp(0.4, 1.12, leg), leg.clamp(0., 1.))
    } else {
        (lerp(1.12, 1., curve((t - 0.6) / 0.4)), 1.)
    }
}

/// A value on screen and the one before it. `epoch` counts the changes;
/// elements that animate the change put it in their id.
#[derive(Debug, Clone)]
pub struct Tracked<T> {
    pub current: T,
    pub previous: T,
    pub epoch: u64,
    /// When it last changed.
    pub since: Instant,
}

impl<T: Clone + PartialEq> Tracked<T> {
    pub fn new(value: T) -> Self {
        Self {
            previous: value.clone(),
            current: value,
            epoch: 0,
            since: Instant::now(),
        }
    }

    /// Records `value`; returns whether it changed.
    pub fn observe(&mut self, value: T) -> bool {
        if value == self.current {
            return false;
        }
        self.previous = std::mem::replace(&mut self.current, value);
        self.epoch += 1;
        self.since = Instant::now();
        true
    }

    /// Whether the last observation changed it, rather than it being the
    /// first value seen.
    pub fn changed(&self) -> bool {
        self.previous != self.current
    }

    /// Whether it changed less than `ms` ago.
    pub fn within(&self, ms: u64) -> bool {
        self.changed() && self.since.elapsed() < Duration::from_millis(ms)
    }
}

/// The color of a screen's rings and glow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mood {
    Acting,
    Done,
    Refused,
}

/// How a connection stands, as the line between the tiles draws it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    /// Not started: amber dashes drifting toward the service.
    Idle,
    /// Waiting on the other side: dim dashes and a comet.
    Waiting,
    /// Done: solid green with a check.
    Connected,
    /// Signed in, but something was not allowed: broken amber and "!".
    Declined,
    /// Refused for good: broken red and a cross.
    Blocked,
}

/// What stands on the other side of the handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Service {
    /// The welcome: no handshake yet.
    None,
    GitHub,
    Token,
    ChatGpt,
    Repos,
    FirstRun,
}

/// The handshake's size: big for sign-ins, small over a list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scale {
    Big,
    Small,
}

/// A progress segment's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Segment {
    Ahead,
    Current,
    Done,
}

/// What a screen is showing: its step and the state it is in. A new one
/// replays its entrances.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scene {
    pub step: SetupStep,
    pub state: u8,
}

/// Everything onboarding's motion follows, as last drawn.
#[derive(Debug, Clone)]
pub struct SetupMotion {
    pub scene: Tracked<Scene>,
    pub mood: Tracked<Mood>,
    pub link: Tracked<Link>,
    pub service: Tracked<Service>,
    pub scale: Tracked<Scale>,
    pub segments: Tracked<[Segment; 4]>,
    /// Whether the first run's field has the keyboard.
    pub composing: Tracked<bool>,
    /// Each clone's progress in thousandths, and whether it is ready.
    pub clones: HashMap<String, Tracked<(u32, bool)>>,
    /// How many times the device code was copied: each flashes it.
    pub copies: u64,
    /// Whether the window has the focus: loops pause without it.
    pub active: bool,
    /// Whether motion is reduced: no loops, ripples or comets, and
    /// transitions are plain fades.
    pub reduce: bool,
}

impl Default for SetupMotion {
    fn default() -> Self {
        Self {
            scene: Tracked::new(Scene {
                step: SetupStep::Welcome,
                state: 0,
            }),
            mood: Tracked::new(Mood::Acting),
            link: Tracked::new(Link::Idle),
            service: Tracked::new(Service::None),
            scale: Tracked::new(Scale::Big),
            segments: Tracked::new([Segment::Ahead; 4]),
            composing: Tracked::new(false),
            clones: HashMap::new(),
            copies: 0,
            active: true,
            reduce: false,
        }
    }
}

/// What a frame of onboarding shows, for [`SetupMotion::observe`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    pub scene: Scene,
    pub mood: Mood,
    pub link: Link,
    pub service: Service,
    pub scale: Scale,
    pub segments: [Segment; 4],
}

impl SetupMotion {
    /// Records a frame. The first one sets every value without a change
    /// to animate, except the scene, whose entrances always play.
    pub fn observe(&mut self, frame: Frame) {
        self.scene.observe(frame.scene);
        self.mood.observe(frame.mood);
        self.link.observe(frame.link);
        self.service.observe(frame.service);
        self.scale.observe(frame.scale);
        self.segments.observe(frame.segments);
    }

    /// Records a clone's progress, forgetting clones no longer listed.
    pub fn observe_clones<'a>(
        &mut self,
        clones: impl Iterator<Item = (&'a str, u32, bool)>,
    ) {
        let mut seen = Vec::new();
        for (name, share, ready) in clones {
            seen.push(name.to_owned());
            match self.clones.get_mut(name) {
                Some(tracked) => {
                    tracked.observe((share, ready));
                }
                None => {
                    self.clones
                        .insert(name.to_owned(), Tracked::new((share, ready)));
                }
            }
        }
        self.clones.retain(|name, _| seen.contains(name));
    }

    /// Whether loops may run: the window is focused, and motion is not
    /// reduced.
    pub fn loops(&self) -> bool {
        self.active && !self.reduce
    }
}

/// Every place a wish for less motion can come from, most direct first:
/// the first that says anything decides.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MotionPreference {
    /// `--reduce-motion` was passed.
    pub flag: bool,
    /// `TAU_REDUCE_MOTION`: `1`, `true`, `yes` or `on` asks for less
    /// motion; `0`, `false`, `no` or `off` for all of it.
    pub env: Option<String>,
    /// `reduce_motion` in `interface.json`.
    pub saved: Option<bool>,
    /// The desktop's `enable-animations`, read through the settings
    /// portal.
    pub desktop_animations: Option<bool>,
}

impl MotionPreference {
    /// The flag, the environment and the saved setting; the desktop's
    /// answer comes later, from [`desktop_animations`].
    pub fn from_startup(flag: bool, settings: &Path) -> Self {
        Self {
            flag,
            env: std::env::var("TAU_REDUCE_MOTION").ok(),
            saved: saved_reduce_motion(settings),
            desktop_animations: None,
        }
    }

    /// What the flag, the environment or the saved setting said, if any
    /// of them said anything.
    pub fn stated(&self) -> Option<bool> {
        if self.flag {
            return Some(true);
        }
        let env = self.env.as_deref().map(str::trim).map(str::to_lowercase);
        match env.as_deref() {
            Some("1" | "true" | "yes" | "on") => Some(true),
            Some("0" | "false" | "no" | "off") => Some(false),
            _ => self.saved,
        }
    }

    /// Whether to reduce motion.
    pub fn reduce(&self) -> bool {
        self.stated()
            .unwrap_or(self.desktop_animations == Some(false))
    }
}

/// The interface's own settings, next to the models': `interface.json`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct InterfaceSettings {
    reduce_motion: Option<bool>,
}

/// `reduce_motion` from the interface settings at `path`, when the file
/// is there and says it.
pub fn saved_reduce_motion(path: &Path) -> Option<bool> {
    let text = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str::<InterfaceSettings>(&text) {
        Ok(settings) => settings.reduce_motion,
        Err(error) => {
            eprintln!("tau-ui: ignoring {}: {error}", path.display());
            None
        }
    }
}

/// Whether the desktop animates, from GNOME's `enable-animations`
/// through the XDG settings portal; `None` without a portal or the key.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub async fn desktop_animations() -> Option<bool> {
    let settings = ashpd::desktop::settings::Settings::new().await.ok()?;
    settings
        .read::<bool>("org.gnome.desktop.interface", "enable-animations")
        .await
        .ok()
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
pub async fn desktop_animations() -> Option<bool> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_most_direct_wish_for_less_motion_decides() {
        let none = MotionPreference::default();
        assert!(!none.reduce(), "full motion when nothing asks");
        let desktop_off = MotionPreference {
            desktop_animations: Some(false),
            ..none.clone()
        };
        assert!(desktop_off.reduce());
        let saved_on = MotionPreference {
            saved: Some(false),
            ..desktop_off.clone()
        };
        assert!(!saved_on.reduce(), "the saved setting beats the desktop");
        let env_on = MotionPreference {
            env: Some(" Yes ".into()),
            ..saved_on.clone()
        };
        assert!(env_on.reduce(), "the environment beats the setting");
        let env_off = MotionPreference {
            env: Some("0".into()),
            saved: Some(true),
            ..none.clone()
        };
        assert!(!env_off.reduce());
        let env_unclear = MotionPreference {
            env: Some("maybe".into()),
            saved: Some(true),
            ..none.clone()
        };
        assert!(env_unclear.reduce(), "an unclear value says nothing");
        let flag = MotionPreference {
            flag: true,
            env: Some("off".into()),
            ..none
        };
        assert!(flag.reduce(), "the flag beats everything");
    }

    #[test]
    fn the_saved_setting_is_read_when_there() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("interface.json");
        assert_eq!(saved_reduce_motion(&path), None, "no file");
        std::fs::write(&path, r#"{"reduce_motion": true}"#).unwrap();
        assert_eq!(saved_reduce_motion(&path), Some(true));
        std::fs::write(&path, "{}").unwrap();
        assert_eq!(saved_reduce_motion(&path), None, "no key");
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(saved_reduce_motion(&path), None, "unreadable");
    }

    #[test]
    fn cubic_beziers_meet_their_ends_and_overshoot_where_asked() {
        for curve in [
            cubic_bezier(0.2, 0.7, 0.2, 1.),
            cubic_bezier(0.45, 0., 0.55, 1.),
            cubic_bezier(0.3, 1.4, 0.5, 1.),
        ] {
            assert!(curve(0.).abs() < 1e-3);
            assert!((curve(1.) - 1.).abs() < 1e-3);
        }
        let linear = cubic_bezier(0.25, 0.25, 0.75, 0.75);
        assert!((linear(0.3) - 0.3).abs() < 1e-3);
        let pop = curve::pop();
        assert!((0..100).any(|n| pop(n as f32 / 100.) > 1.01), "overshoots");
        let ease = curve::ease_in_out();
        assert!((ease(0.5) - 0.5).abs() < 1e-3, "symmetric");
    }

    #[test]
    fn delayed_animations_hold_then_run() {
        let animation = after(100, 300, |t| t);
        let at = |ms: f32| (animation.easing)(ms / 400.);
        assert_eq!(at(50.), 0.);
        assert!((at(250.) - 0.5).abs() < 1e-3);
        assert_eq!(at(400.), 1.);
    }

    #[test]
    fn rows_enter_one_after_another() {
        assert_eq!(stagger(550, 60, 0), 550);
        assert_eq!(stagger(550, 60, 3), 730);
        assert_eq!(stagger(0, 40, 7), 280);
    }

    #[test]
    fn entrances_wait_their_turn_unless_motion_is_reduced() {
        let late = entrance(200, false);
        let total = (200 + ENTRANCE_MS) as f32;
        assert_eq!(late.duration, Duration::from_millis(200 + ENTRANCE_MS));
        assert_eq!((late.easing)(150. / total), 0., "still waiting");
        assert!((late.easing)(300. / total) > 0.5, "rising fast");
        assert!(((late.easing)(1.) - 1.).abs() < 1e-3);
        let reduced = entrance(200, true);
        assert_eq!(reduced.duration, Duration::from_millis(FADE_MS));
        assert!(((reduced.easing)(0.5) - 0.5).abs() < 1e-6, "a linear fade");
    }

    #[test]
    fn a_pop_overshoots_then_settles() {
        let linear = |t: f32| t;
        assert_eq!(pop(0., &linear), (0.4, 0.));
        let (peak, shown) = pop(0.6, &linear);
        assert!((peak - 1.12).abs() < 1e-4 && shown == 1.);
        assert!((pop(1., &linear).0 - 1.).abs() < 1e-4);
    }

    #[test]
    fn breaths_rise_and_fall() {
        assert!(breath(0.).abs() < 1e-6);
        assert!((breath(0.5) - 1.).abs() < 1e-6);
        assert!(breath(1.).abs() < 1e-5);
    }

    #[test]
    fn tracked_values_count_their_changes() {
        let mut link = Tracked::new(Link::Idle);
        assert!(!link.changed() && link.epoch == 0);
        assert!(!link.observe(Link::Idle));
        assert!(link.observe(Link::Waiting));
        assert!(link.observe(Link::Connected));
        assert_eq!((link.previous, link.epoch), (Link::Waiting, 2));
        assert!(link.changed() && link.within(60_000));
    }

    #[test]
    fn only_a_changed_frame_starts_transitions() {
        let waiting = Frame {
            scene: Scene {
                step: SetupStep::Model,
                state: 1,
            },
            mood: Mood::Acting,
            link: Link::Waiting,
            service: Service::ChatGpt,
            scale: Scale::Big,
            segments: [
                Segment::Done,
                Segment::Current,
                Segment::Ahead,
                Segment::Ahead,
            ],
        };
        let mut motion = SetupMotion::default();
        motion.observe(waiting);
        let epochs = |m: &SetupMotion| {
            (m.scene.epoch, m.mood.epoch, m.link.epoch, m.scale.epoch)
        };
        let first = epochs(&motion);
        motion.observe(waiting);
        motion.observe(waiting);
        assert_eq!(epochs(&motion), first, "redraws start nothing");
        motion.observe(Frame {
            scene: Scene {
                state: 2,
                ..waiting.scene
            },
            mood: Mood::Done,
            link: Link::Connected,
            ..waiting
        });
        let (scene, mood, link, scale) = epochs(&motion);
        assert_eq!(
            (scene, mood, link),
            (first.0 + 1, first.1 + 1, first.2 + 1)
        );
        assert_eq!(scale, first.3, "the size did not change");
        assert_eq!(motion.link.previous, Link::Waiting);
        assert!(motion.link.within(60_000));
    }

    #[test]
    fn loops_stop_in_the_background_and_when_reduced() {
        let mut motion = SetupMotion::default();
        assert!(motion.loops());
        motion.active = false;
        assert!(!motion.loops());
        motion.active = true;
        motion.reduce = true;
        assert!(!motion.loops());
    }

    #[test]
    fn clones_are_followed_by_name() {
        let mut motion = SetupMotion::default();
        motion.observe_clones([("a", 100, false), ("b", 0, false)].into_iter());
        motion.observe_clones([("a", 1000, true)].into_iter());
        let a = &motion.clones["a"];
        assert_eq!((a.previous, a.current), ((100, false), (1000, true)));
        assert!(!motion.clones.contains_key("b"));
    }

    #[test]
    fn colors_mix_through_rgb() {
        let (black, white) = (gpui::black(), gpui::white());
        let grey: Rgba = mix(black, white, 0.5).into();
        assert!((grey.r - 0.5).abs() < 1e-3 && (grey.a - 1.).abs() < 1e-3);
    }
}
