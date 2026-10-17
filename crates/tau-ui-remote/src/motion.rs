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
    /// A plugin's step.
    Plugin,
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
    /// One per stage of the step bar, which plugins' steps lengthen.
    pub segments: Tracked<Vec<Segment>>,
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
            segments: Tracked::new(vec![Segment::Ahead; 4]),
            composing: Tracked::new(false),
            clones: HashMap::new(),
            copies: 0,
            active: true,
            reduce: false,
        }
    }
}

/// What a frame of onboarding shows, for [`SetupMotion::observe`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub scene: Scene,
    pub mood: Mood,
    pub link: Link,
    pub service: Service,
    pub scale: Scale,
    pub segments: Vec<Segment>,
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
    /// answer comes later, from the `tau` app's `desktop_animations`.
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
    /// How large the interface is drawn: 1 is as designed.
    zoom: Option<f32>,
    /// Desktop notifications while tau's window is not focused; on
    /// unless `false`.
    notifications: Option<bool>,
}

/// Whether the interface settings at `path` leave desktop notifications
/// on: they are, unless the file says `"notifications": false`.
pub fn saved_notifications(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return true;
    };
    match serde_json::from_str::<InterfaceSettings>(&text) {
        Ok(settings) => settings.notifications.unwrap_or(true),
        Err(error) => {
            eprintln!("tau-ui: ignoring {}: {error}", path.display());
            true
        }
    }
}

/// The zoom saved in the interface settings at `path`, when the file is
/// there and says one.
pub fn saved_zoom(path: &Path) -> Option<f32> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<InterfaceSettings>(&text).ok()?.zoom
}

/// Saves `zoom` in the interface settings at `path`, keeping what else
/// the file says.
pub fn save_zoom(path: &Path, zoom: f32) -> std::io::Result<()> {
    let mut settings = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| {
            serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(
                &text,
            )
            .ok()
        })
        .unwrap_or_default();
    settings.insert("zoom".into(), serde_json::json!(zoom));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(&settings)
        .map_err(std::io::Error::other)?;
    std::fs::write(path, text + "\n")
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

#[cfg(test)]
mod tests {
    use hegel::generators as gs;

    use super::*;

    /// Every place a wish can come from, in every state, decides as the
    /// most direct one that says anything: the flag, then a
    /// `TAU_REDUCE_MOTION` it understands (each of its words, in any
    /// case, padded or not), then the saved setting, then the desktop's
    /// animations being off. An unclear environment says nothing.
    #[test]
    fn the_most_direct_wish_for_less_motion_decides() {
        const MORE: [&str; 4] = ["1", "true", "yes", "on"];
        const LESS: [&str; 4] = ["0", "false", "no", "off"];
        let mut envs: Vec<(Option<String>, Option<bool>)> = vec![(None, None)];
        for (words, says) in [(MORE, true), (LESS, false)] {
            for word in words {
                for written in
                    [word.to_owned(), word.to_uppercase(), format!(" {word}\t")]
                {
                    envs.push((Some(written), Some(says)));
                }
            }
        }
        for unclear in ["", "maybe", "2", "y", "of", "yess"] {
            envs.push((Some(unclear.to_owned()), None));
        }
        let three = [None, Some(true), Some(false)];
        for flag in [false, true] {
            for (env, env_says) in &envs {
                for saved in three {
                    for desktop_animations in three {
                        let preference = MotionPreference {
                            flag,
                            env: env.clone(),
                            saved,
                            desktop_animations,
                        };
                        let expected = if flag {
                            true
                        } else if let Some(says) = env_says {
                            *says
                        } else if let Some(saved) = saved {
                            saved
                        } else {
                            desktop_animations == Some(false)
                        };
                        assert_eq!(
                            preference.reduce(),
                            expected,
                            "{preference:?}"
                        );
                    }
                }
            }
        }
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
    fn notifications_are_on_unless_the_settings_turn_them_off() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("interface.json");
        assert!(saved_notifications(&path), "no file");
        std::fs::write(&path, r#"{"notifications": false}"#).unwrap();
        assert!(!saved_notifications(&path));
        std::fs::write(&path, r#"{"notifications": true}"#).unwrap();
        assert!(saved_notifications(&path));
        std::fs::write(&path, r#"{"reduce_motion": true}"#).unwrap();
        assert!(saved_notifications(&path), "no key");
        std::fs::write(&path, "not json").unwrap();
        assert!(saved_notifications(&path), "unreadable");
    }

    /// The named curves do what their names say: a pop overshoots, and
    /// an ease in and out is symmetric. (Ends and monotony are the
    /// properties' below.)
    #[test]
    fn the_named_curves_overshoot_and_ease_where_asked() {
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
            segments: vec![
                Segment::Done,
                Segment::Current,
                Segment::Ahead,
                Segment::Ahead,
            ],
        };
        let mut motion = SetupMotion::default();
        motion.observe(waiting.clone());
        let epochs = |m: &SetupMotion| {
            (m.scene.epoch, m.mood.epoch, m.link.epoch, m.scale.epoch)
        };
        let first = epochs(&motion);
        motion.observe(waiting.clone());
        motion.observe(waiting.clone());
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

    /// Any colour, channels in `[0, 1]`; built in RGB so the HSL round
    /// trip inside `mix` is what the properties exercise.
    #[hegel::composite]
    fn channels(tc: &hegel::TestCase) -> (f32, f32, f32, f32) {
        let channel = || gs::floats::<f32>().min_value(0.).max_value(1.);
        (
            tc.draw(channel()),
            tc.draw(channel()),
            tc.draw(channel()),
            tc.draw(channel()),
        )
    }

    fn rgba((r, g, b, a): (f32, f32, f32, f32)) -> Rgba {
        Rgba { r, g, b, a }
    }

    /// A mix moves along the straight line between its inputs in RGB:
    /// each channel is `t` of the way from `from`'s to `to`'s, through
    /// the HSL round trip, so a transition never overshoots either
    /// colour and black and white meet in grey. Its ends are its inputs
    /// (`t` is drawn at 0 and 1 too), and a colour mixed with itself
    /// stays itself (`to` is sometimes `from`).
    #[hegel::test(test_cases = 300)]
    fn a_mix_is_the_straight_line_between_its_inputs(tc: hegel::TestCase) {
        let from = rgba(tc.draw(channels()));
        let to = if tc.draw(gs::weighted_booleans(0.2)) {
            from
        } else {
            rgba(tc.draw(channels()))
        };
        let t = tc.draw(hegel::one_of!(
            gs::sampled_from(vec![0.0f32, 1.0]),
            gs::floats::<f32>().min_value(0.).max_value(1.),
        ));
        let mixed: Rgba = mix(from.into(), to.into(), t).into();
        for (m, f, o) in [
            (mixed.r, from.r, to.r),
            (mixed.g, from.g, to.g),
            (mixed.b, from.b, to.b),
            (mixed.a, from.a, to.a),
        ] {
            let along = f * (1. - t) + o * t;
            assert!((m - along).abs() < 1e-3, "{m} is not {along}");
        }
    }

    fn unit() -> impl hegel::generators::PrintableGenerator<f32> {
        gs::floats::<f32>().min_value(0.).max_value(1.)
    }

    /// A CSS timing function whose control points stay in the unit
    /// square is a monotone map of `[0, 1]` onto itself: it starts at 0,
    /// ends at 1 and never runs backwards, whatever the solver does
    /// with flat or steep stretches.
    #[hegel::test(test_cases = 300)]
    fn a_bezier_inside_the_unit_square_is_monotone_from_0_to_1(
        tc: hegel::TestCase,
    ) {
        let curve = cubic_bezier(
            tc.draw(unit()),
            tc.draw(unit()),
            tc.draw(unit()),
            tc.draw(unit()),
        );
        assert!(curve(0.).abs() < 2e-3 && (curve(1.) - 1.).abs() < 2e-3);
        let mut last = curve(0.);
        for step in 1..=50 {
            let value = curve(step as f32 / 50.);
            assert!((-2e-3..=1. + 2e-3).contains(&value), "{value}");
            assert!(value >= last - 2e-3, "{last} then {value}");
            last = value;
        }
    }

    /// A curve whose control points lie on the diagonal is no easing at
    /// all: it gives back the time it is given.
    #[hegel::test(test_cases = 100)]
    fn a_bezier_on_the_diagonal_is_the_identity(tc: hegel::TestCase) {
        let (first, second) = (tc.draw(unit()), tc.draw(unit()));
        let curve = cubic_bezier(first, first, second, second);
        let t = tc.draw(unit());
        assert!((curve(t) - t).abs() < 1e-3, "{t} gives {}", curve(t));
    }

    /// Time outside a curve's range is the nearest end.
    #[hegel::test(test_cases = 100)]
    fn a_bezier_holds_its_ends_outside_the_unit_interval(tc: hegel::TestCase) {
        let curve = cubic_bezier(
            tc.draw(unit()),
            tc.draw(unit()),
            tc.draw(unit()),
            tc.draw(unit()),
        );
        let early = tc.draw(gs::floats::<f32>().max_value(0.));
        let late = tc.draw(gs::floats::<f32>().min_value(1.));
        assert_eq!(curve(early), curve(0.));
        assert_eq!(curve(late), curve(1.));
    }

    /// A breath rises from 0 at the start of its loop to 1 in its
    /// middle, never falling on the way, and its second half mirrors the
    /// first, so it falls back to 0.
    #[hegel::test(test_cases = 200)]
    fn a_breath_rises_to_the_middle_and_mirrors_itself(tc: hegel::TestCase) {
        let phase = tc.draw(unit());
        let breath = breath(phase);
        assert!((-1e-6..=1. + 1e-6).contains(&breath));
        assert!((breath - super::breath(1. - phase)).abs() < 1e-4);
        let (a, b) = (
            tc.draw(gs::floats::<f32>().min_value(0.).max_value(0.5)),
            tc.draw(gs::floats::<f32>().min_value(0.).max_value(0.5)),
        );
        let (low, high) = (a.min(b), a.max(b));
        assert!(super::breath(low) <= super::breath(high) + 1e-6);
        assert!(super::breath(0.).abs() < 1e-6);
        assert!((super::breath(0.5) - 1.).abs() < 1e-6);
    }

    /// A pop starts at `.4` and unseen, its two legs meet at 60 %, where
    /// the scale is the peak, and it settles at 1, for every curve the
    /// screens pop along. (A curve that is nearly
    /// vertical may jump anywhere, so arbitrary ones are not asked.)
    #[hegel::test(test_cases = 100)]
    fn a_pop_is_continuous_at_its_peak(tc: hegel::TestCase) {
        let curves: Vec<Box<dyn Fn(f32) -> f32>> = vec![
            Box::new(curve::pop()),
            Box::new(curve::swap()),
            Box::new(curve::bounce()),
            Box::new(curve::rise()),
            Box::new(curve::ease_in_out()),
        ];
        let which =
            tc.draw(gs::integers::<usize>().max_value(curves.len() - 1));
        let curve = &curves[which];
        let (start, unseen) = pop(0., curve);
        assert!((start - 0.4).abs() < 1e-3 && unseen.abs() < 1e-3);
        let (before, _) = pop(0.6 - 1e-4, curve);
        let (after, opacity) = pop(0.6, curve);
        assert!((before - after).abs() < 0.02, "{before} then {after}");
        assert!((after - 1.12).abs() < 0.02 && opacity == 1.);
        assert_eq!(pop(1., curve).0, 1.);
        let t = tc.draw(unit());
        let (scale, opacity) = pop(t, curve);
        assert!(scale > 0. && (0. ..=1.).contains(&opacity));
    }

    /// `Tracked` against a model: the epoch counts the observations
    /// that differed from the value before them, and `previous` is the
    /// value that last gave way.
    #[hegel::test(test_cases = 200)]
    fn tracked_follows_a_model_of_its_observations(tc: hegel::TestCase) {
        let first = tc.draw(gs::integers::<u8>().max_value(3));
        let seen =
            tc.draw(gs::vecs(gs::integers::<u8>().max_value(3)).max_size(20));
        let mut tracked = Tracked::new(first);
        let (mut current, mut previous, mut epoch) = (first, first, 0);
        for value in seen {
            let changed = tracked.observe(value);
            assert_eq!(changed, value != current);
            if changed {
                previous = current;
                current = value;
                epoch += 1;
            }
            assert_eq!(
                (tracked.current, tracked.previous, tracked.epoch),
                (current, previous, epoch)
            );
            assert_eq!(tracked.changed(), previous != current);
        }
    }

    /// After any series of frames, the clones followed are exactly the
    /// ones the last frame listed, each at its latest progress and with
    /// the progress that last gave way, against a map of pairs: a clone
    /// that comes (or comes back) starts with nothing that gave way.
    #[hegel::test(test_cases = 200)]
    fn followed_clones_are_the_ones_last_listed(tc: hegel::TestCase) {
        let frame = || {
            gs::hashmaps(
                gs::sampled_from(vec!["a", "b", "c", "d"]),
                hegel::tuples!(
                    gs::integers::<u32>().max_value(1000),
                    gs::booleans()
                ),
            )
            .max_size(4)
        };
        let frames = tc.draw(gs::vecs(frame()).min_size(1).max_size(6));
        let mut motion = SetupMotion::default();
        // name -> (previous, current)
        type Progress = (u32, bool);
        let mut model: HashMap<&str, (Progress, Progress)> = HashMap::new();
        for listed in &frames {
            motion.observe_clones(
                listed
                    .iter()
                    .map(|(name, (share, ready))| (*name, *share, *ready)),
            );
            model = listed
                .iter()
                .map(|(name, now)| {
                    let pair = match model.get(name) {
                        Some((_, current)) if current != now => {
                            (*current, *now)
                        }
                        Some(same) => *same,
                        None => (*now, *now),
                    };
                    (*name, pair)
                })
                .collect();
        }
        for (name, (previous, current)) in &model {
            let tracked = &motion.clones[*name];
            assert_eq!(
                (tracked.previous, tracked.current),
                (*previous, *current)
            );
        }
        let last = frames.last().unwrap();
        let mut followed: Vec<&str> =
            motion.clones.keys().map(String::as_str).collect();
        followed.sort_unstable();
        let mut listed: Vec<&str> = last.keys().copied().collect();
        listed.sort_unstable();
        assert_eq!(followed, listed);
        for (name, progress) in last {
            assert_eq!(motion.clones[*name].current, *progress);
        }
    }
}
