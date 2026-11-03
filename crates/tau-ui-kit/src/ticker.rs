//! A clock that counts: the time since a start, drawn again each second
//! until it stops, or the time left until something is due. What it
//! counts from is the caller's state; only the drawing reads the clock.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{Context, IntoElement, Render, Window};

use crate::{
    components::mono,
    format::clock,
    theme::{Type, theme},
};

/// What a ticker counts, in milliseconds since 1970.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Count {
    /// The time since `since`, to `until` once it stops: `1:23`.
    Since { since: i64, until: Option<i64> },
    /// The time left until `at`: `in 5m`, then `now`.
    Due { at: i64 },
}

/// A clock that counts what [`Count`] says.
pub struct Ticker {
    count: Count,
    /// Whether a redraw a second from now is on its way.
    ticking: bool,
}

/// Now, in milliseconds since 1970.
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as i64)
}

/// What a ticker from `since` reads at `at`: `m:ss`, never below zero.
pub fn reading(since: i64, at: i64) -> String {
    clock(Duration::from_millis(at.saturating_sub(since).max(0) as u64))
}

/// What a countdown to `at` reads at `now`: `now` within a minute, then
/// the minutes, hours or days left.
pub fn due(at: i64, now: i64) -> String {
    let gap = at.saturating_sub(now) / 1000;
    match gap {
        ..60 => "now".to_owned(),
        60..3600 => format!("in {}m", gap / 60),
        3600..86_400 => format!("in {}h", gap / 3600),
        _ => format!("in {}d", gap / 86_400),
    }
}

impl Count {
    /// What it reads at `now`, and whether it still changes.
    fn read(self, now: i64) -> (String, bool) {
        match self {
            Self::Since { since, until } => {
                (reading(since, until.unwrap_or(now)), until.is_none())
            }
            Self::Due { at } => (due(at, now), at > now),
        }
    }
}

impl Ticker {
    pub fn new(count: Count) -> Self {
        Self {
            count,
            ticking: false,
        }
    }

    /// Counts what `count` says from now on.
    pub fn set(&mut self, count: Count, cx: &mut Context<Self>) {
        if self.count != count {
            self.count = count;
            cx.notify();
        }
    }
}

impl Render for Ticker {
    fn render(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (text, changes) = self.count.read(now());
        if changes && !self.ticking {
            self.ticking = true;
            cx.spawn(async move |ticker, cx| {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let _ = ticker.update(cx, |ticker, cx| {
                    ticker.ticking = false;
                    cx.notify();
                });
            })
            .detach();
        }
        mono(text, Type::CAPTION, theme(cx).text_soft)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ticker reads the minutes and seconds since its start, and none
    /// before it.
    #[test]
    fn it_reads_the_time_since_its_start() {
        assert_eq!(reading(1_000, 1_000), "0:00");
        assert_eq!(reading(1_000, 84_999), "1:23");
        assert_eq!(reading(5_000, 1_000), "0:00");
    }

    /// A countdown reads the time left in its largest whole unit, and
    /// `now` within a minute or once past; only a running clock changes.
    #[test]
    fn a_countdown_reads_the_time_left() {
        let minute = 60_000;
        assert_eq!(due(30_000, 0), "now");
        assert_eq!(due(5 * minute, 0), "in 5m");
        assert_eq!(due(90 * minute, 0), "in 1h");
        assert_eq!(due(50 * 60 * minute, 0), "in 2d");
        assert_eq!(due(0, minute), "now");
        assert!(Count::Due { at: minute }.read(0).1);
        assert!(!Count::Due { at: 0 }.read(minute).1);
        assert!(
            !Count::Since {
                since: 0,
                until: Some(1)
            }
            .read(5)
            .1
        );
    }
}
