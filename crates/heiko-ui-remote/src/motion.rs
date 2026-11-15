//! Onboarding's motion: the curves, and what changed on screen since the
//! last frame.
//!
//! Every transition starts from a real change of state. The workspace
//! records what onboarding shows each frame in a [`SetupMotion`]; a value
//! that changed gets a new epoch, and the elements that animate it put
//! the epoch in their id, so GPUI starts their animation then and only
//! then. Onboarding is flat and still: the only motion left is each
//! screen's words rising into place, a plain fade when motion is reduced.

use std::{path::Path, time::Duration};

use gpui::Animation;
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

/// The curves onboarding moves along, by what they do.
pub mod curve {
    use super::cubic_bezier;

    /// Things rising into place: `cubic-bezier(.2,.7,.2,1)`.
    pub fn rise() -> impl Fn(f32) -> f32 + Clone {
        cubic_bezier(0.2, 0.7, 0.2, 1.)
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

/// A value on screen and the one before it. `epoch` counts the changes;
/// elements that animate the change put it in their id.
#[derive(Debug, Clone)]
pub struct Tracked<T> {
    pub current: T,
    pub previous: T,
    pub epoch: u64,
}

impl<T: Clone + PartialEq> Tracked<T> {
    pub fn new(value: T) -> Self {
        Self {
            previous: value.clone(),
            current: value,
            epoch: 0,
        }
    }

    /// Records `value`; returns whether it changed.
    pub fn observe(&mut self, value: T) -> bool {
        if value == self.current {
            return false;
        }
        self.previous = std::mem::replace(&mut self.current, value);
        self.epoch += 1;
        true
    }

    /// Whether the last observation changed it, rather than it being the
    /// first value seen.
    pub fn changed(&self) -> bool {
        self.previous != self.current
    }
}

/// How a connection stands, as the line between heiko and a service
/// draws it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    /// Not started: a dashed line.
    Idle,
    /// Waiting on the other side: a dashed line and the working mark.
    Waiting,
    /// Done: a solid green line and a tick.
    Connected,
    /// Signed in, but something was not allowed: a dashed line and the
    /// asking mark.
    Declined,
    /// Refused for good: a dashed red line and a cross.
    Blocked,
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
    pub link: Tracked<Link>,
    /// Whether motion is reduced: entrances are plain fades.
    pub reduce: bool,
}

impl Default for SetupMotion {
    fn default() -> Self {
        Self {
            scene: Tracked::new(Scene {
                step: SetupStep::Welcome,
                state: 0,
            }),
            link: Tracked::new(Link::Idle),
            reduce: false,
        }
    }
}

/// What a frame of onboarding shows, for [`SetupMotion::observe`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub scene: Scene,
    pub link: Link,
}

impl SetupMotion {
    /// Records a frame. The first one sets every value without a change
    /// to animate, except the scene, whose entrances always play.
    pub fn observe(&mut self, frame: Frame) {
        self.scene.observe(frame.scene);
        self.link.observe(frame.link);
    }
}

/// Every place a wish for less motion can come from, most direct first:
/// the first that says anything decides.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MotionPreference {
    /// `--reduce-motion` was passed.
    pub flag: bool,
    /// `HEIKO_REDUCE_MOTION`: `1`, `true`, `yes` or `on` asks for less
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
    /// answer comes later, from the `heiko` app's `desktop_animations`.
    pub fn from_startup(flag: bool, settings: &Path) -> Self {
        Self {
            flag,
            env: std::env::var("HEIKO_REDUCE_MOTION").ok(),
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
            eprintln!("heiko-ui: ignoring {}: {error}", path.display());
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
    /// `HEIKO_REDUCE_MOTION` it understands (each of its words, in any
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
                step: SetupStep::Plugin {
                    plugin: "provider",
                    step: "sign-in",
                },
                state: 1,
            },
            link: Link::Waiting,
        };
        let mut motion = SetupMotion::default();
        motion.observe(waiting.clone());
        let epochs = |m: &SetupMotion| (m.scene.epoch, m.link.epoch);
        let first = epochs(&motion);
        motion.observe(waiting.clone());
        motion.observe(waiting.clone());
        assert_eq!(epochs(&motion), first, "redraws start nothing");
        motion.observe(Frame {
            scene: Scene {
                state: 2,
                ..waiting.scene
            },
            link: Link::Connected,
        });
        assert_eq!(epochs(&motion), (first.0 + 1, first.1 + 1));
        assert_eq!(motion.link.previous, Link::Waiting);
        assert!(motion.link.changed());
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
}
