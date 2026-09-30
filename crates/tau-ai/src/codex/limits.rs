//! A ChatGPT plan's usage limits, as Codex reports them.
//!
//! Before each response, the Codex endpoint sends a `codex.rate_limits`
//! frame: the plan, how much of each usage window is used and when it
//! resets, and the account's credits. The client keeps the latest one
//! ([`crate::client::OpenAi::rate_limits`]). The API endpoint sends none.

use serde_json::Value;

/// The frame type that carries them.
pub const FRAME: &str = "codex.rate_limits";

/// The plan's limits as of the latest response.
#[derive(Debug, Clone, PartialEq)]
pub struct RateLimits {
    /// The ChatGPT plan, as Codex names it (`plus`, `pro`, …).
    pub plan: Option<String>,
    /// Whether a window is used up: requests wait until it resets.
    pub limit_reached: bool,
    /// The usage windows, shortest first; a plan may have one or two.
    pub windows: Vec<Window>,
    pub credits: Option<Credits>,
}

/// One usage window: a share of the plan's use over a span of time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Window {
    /// How much of the window is used, 0 to 100.
    pub used_percent: f64,
    /// The window's span, in minutes: 300 for five hours, 10080 for a
    /// week.
    pub minutes: u64,
    /// When the window resets, in seconds since the Unix epoch.
    pub resets_at: i64,
}

/// Credits the account can spend past its plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credits {
    pub has_credits: bool,
    pub unlimited: bool,
    /// The balance, as Codex writes it (a decimal string).
    pub balance: String,
}

impl RateLimits {
    /// Reads a `codex.rate_limits` frame; `None` for any other frame, or
    /// one that carries no limits.
    pub fn from_frame(frame: &Value) -> Option<Self> {
        if frame.get("type").and_then(Value::as_str) != Some(FRAME) {
            return None;
        }
        let limits = frame.get("rate_limits")?;
        let mut windows: Vec<Window> = ["primary", "secondary"]
            .into_iter()
            .filter_map(|key| Window::from_value(limits.get(key)?))
            .collect();
        windows.sort_by_key(|window| window.minutes);
        let credits = frame.get("credits").and_then(|credits| {
            Some(Credits {
                has_credits: credits.get("has_credits")?.as_bool()?,
                unlimited: credits.get("unlimited")?.as_bool()?,
                balance: credits
                    .get("balance")
                    .and_then(Value::as_str)
                    .unwrap_or("0")
                    .to_owned(),
            })
        });
        Some(Self {
            plan: frame
                .get("plan_type")
                .and_then(Value::as_str)
                .map(str::to_owned),
            limit_reached: limits
                .get("limit_reached")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            windows,
            credits,
        })
    }
}

impl Window {
    fn from_value(value: &Value) -> Option<Self> {
        Some(Self {
            used_percent: value.get("used_percent")?.as_f64()?,
            minutes: value.get("window_minutes")?.as_u64()?,
            resets_at: value.get("reset_at")?.as_i64()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// The frame Codex sent a Pro account on 2026-09-29, before a
    /// response.
    fn pro() -> Value {
        json!({
            "type": "codex.rate_limits",
            "plan_type": "pro",
            "rate_limits": {
                "allowed": true,
                "limit_reached": false,
                "primary": {
                    "used_percent": 0,
                    "window_minutes": 10080,
                    "reset_after_seconds": 482590,
                    "reset_at": 1791208742
                },
                "secondary": null
            },
            "code_review_rate_limits": null,
            "additional_rate_limits": null,
            "credits": {
                "has_credits": false,
                "unlimited": false,
                "balance": "0"
            },
            "promo": null
        })
    }

    #[test]
    fn a_real_frame_reads_as_its_plan_windows_and_credits() {
        let limits = RateLimits::from_frame(&pro()).unwrap();
        assert_eq!(limits.plan.as_deref(), Some("pro"));
        assert!(!limits.limit_reached);
        assert_eq!(
            limits.windows,
            [Window {
                used_percent: 0.0,
                minutes: 10080,
                resets_at: 1791208742,
            }]
        );
        assert_eq!(
            limits.credits,
            Some(Credits {
                has_credits: false,
                unlimited: false,
                balance: "0".into(),
            })
        );
    }

    #[test]
    fn two_windows_come_shortest_first() {
        let mut frame = pro();
        frame["rate_limits"]["secondary"] = json!({
            "used_percent": 61.5,
            "window_minutes": 300,
            "reset_at": 1791000000
        });
        let limits = RateLimits::from_frame(&frame).unwrap();
        let minutes: Vec<u64> =
            limits.windows.iter().map(|window| window.minutes).collect();
        assert_eq!(minutes, [300, 10080]);
        assert_eq!(limits.windows[0].used_percent, 61.5);
    }

    #[test]
    fn other_frames_and_bare_ones_carry_no_limits() {
        assert_eq!(
            RateLimits::from_frame(&json!({"type": "codex.rate_limits"})),
            None
        );
        assert_eq!(
            RateLimits::from_frame(&json!({"type": "response.created"})),
            None
        );
    }
}
