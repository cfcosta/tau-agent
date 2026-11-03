//! A clock that counts: the time since a start, drawn again each second
//! until it stops. What it counts from is the caller's state; only the
//! drawing reads the clock.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{Context, IntoElement, Render, Window};

use crate::{
    components::mono,
    format::clock,
    theme::{Type, theme},
};

/// The time since `since`, to `until` once it stops, in milliseconds
/// since 1970.
pub struct Ticker {
    since: i64,
    until: Option<i64>,
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

impl Ticker {
    pub fn new(since: i64, until: Option<i64>) -> Self {
        Self {
            since,
            until,
            ticking: false,
        }
    }

    /// Counts from `since` to `until` from now on.
    pub fn set(
        &mut self,
        since: i64,
        until: Option<i64>,
        cx: &mut Context<Self>,
    ) {
        if (self.since, self.until) != (since, until) {
            (self.since, self.until) = (since, until);
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
        let text = reading(self.since, self.until.unwrap_or_else(now));
        if self.until.is_none() && !self.ticking {
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
}
