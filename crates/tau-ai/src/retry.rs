//! Retry classification and backoff for the OpenAI Responses API.
//!
//! Pure and deterministic: no I/O, no wall clock, no global RNG. Callers
//! sample jitter themselves and inject [`SystemTime`] where a "now" is
//! needed, so every function here is a plain, testable computation.
//!
//! This module does **not** implement the WebSocket recovery ladder
//! (`previous_response_not_found`, `websocket_connection_limit_reached`,
//! reconnect-and-resend); the WebSocket layer handles that, invisibly to
//! callers (see `docs/reference/openai-websocket.md`). What reaches this
//! module is a failure the ladder did not absorb.
//!
//! ## Classification table
//!
//! [`classify`] looks at an OpenAI error's `code`, then its `type`
//! (called `kind` here to avoid shadowing Rust's `type` keyword), then
//! its HTTP status, in that order: **the first one that is on a known
//! list decides the class.** An unknown `code` falls through to `kind`;
//! an unknown `kind` falls through to `status`; an unknown or absent
//! `status` is [`Class::Fatal`]. This mirrors OpenAI's own error shape,
//! where `code` is the most specific field and is often `null`, `type`
//! is a coarser category, and the HTTP status is the fallback every
//! client can read even from a body it does not otherwise understand.
//!
//! We classify on `code` and `status` only, never on message text. pi's
//! retry classifier matches loose regexes over error text, where a bare
//! `500` anywhere in a message counts (`packages/ai/src/utils/retry.ts:26`,
//! see `docs/reference/pi-audit.md`). That misfires on quoted error text
//! ("the model said 500 things") and can't tell `rate_limit_exceeded`
//! (retry) from `insufficient_quota` (both HTTP 429, but quota failures
//! never succeed on retry).
//!
//! `code` (most specific, checked first):
//!
//! | `code`                       | Class            | Why                                                             |
//! | ----------------------------- | ---------------- | ---------------------------------------------------------------- |
//! | `rate_limit_exceeded`         | Retryable        | A transient per-request/per-minute throttle.                    |
//! | `server_error`                | Retryable        | OpenAI's generic server-side failure code.                      |
//! | `context_length_exceeded`     | ContextOverflow  | The input no longer fits; retrying as-is never helps.           |
//! | `insufficient_quota`          | Fatal            | Billing/quota exhausted; retrying never helps (pi's known bug). |
//! | `billing_not_active`          | Fatal            | Account billing issue; not transient.                           |
//! | `billing_hard_limit_reached`  | Fatal            | Hard spend limit; not transient.                                |
//! | `account_deactivated`         | Fatal            | Account state issue; not transient.                             |
//! | `invalid_api_key`             | Fatal            | Credential issue; retrying with the same key never helps.       |
//!
//! `kind` (the error's `type` field, checked when `code` is absent or
//! unknown):
//!
//! | `kind`                 | Class     | Why                                                    |
//! | ----------------------- | --------- | ------------------------------------------------------- |
//! | `server_error`          | Retryable | Same rationale as the `server_error` code.             |
//! | `api_error`             | Retryable | OpenAI's other generic server-side category.           |
//! | `invalid_request_error` | Fatal     | A malformed request; retrying the same body never helps. |
//!
//! `status` (checked when neither `code` nor `kind` is known):
//!
//! | HTTP status   | Class     | Why                                                                 |
//! | ------------- | --------- | -------------------------------------------------------------------- |
//! | 408           | Retryable | Request timeout.                                                    |
//! | 429           | Retryable | Rate limiting when no more specific `code` distinguishes it from quota. |
//! | 500-599       | Retryable | Server-side failure.                                                |
//! | other 4xx     | Fatal     | Client-side error; the same request will fail again.                |
//! | 409           | Fatal     | OpenAI's retry guidance covers 429 and 5xx only; a conflict here is treated like any other 4xx. |
//! | none of these | Fatal     | Nothing tells us it is safe to retry.                                |
//!
//! A [`Failure::Transport`] before the first event was emitted is
//! [`Class::Retryable`]; one that happens after the first event is
//! [`Class::Fatal`], because the stream already started and the agent
//! will see a proper `error` event carrying an [`Failure::Api`] instead.

use std::time::Duration;

/// A classified failure: either an OpenAI API error, or a transport
/// failure below the API layer (connect refused, TLS, socket closed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure<'a> {
    /// An OpenAI `error` event, or the `error` object of a failed
    /// response: its `code`, its `type` (`kind`, here), and the HTTP
    /// status of the response, when known.
    Api {
        code: Option<&'a str>,
        kind: Option<&'a str>,
        status: Option<u16>,
    },
    /// A transport failure: the connection could not be made, or was
    /// lost. `before_first_event` says whether any event had already
    /// been emitted for this turn when the failure happened.
    Transport { before_first_event: bool },
}

/// The result of [`classify`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Safe to retry with backoff.
    Retryable,
    /// The input no longer fits; compact once, then retry once (see
    /// `docs/reference/compaction.md`). Never combined with `Retryable`.
    ContextOverflow,
    /// Not safe to retry; fail the turn now.
    Fatal,
}

/// Classify a failure per the table in the module documentation.
///
/// Never inspects message text, only `code`, `kind` and `status`.
pub fn classify(failure: &Failure<'_>) -> Class {
    match failure {
        Failure::Transport {
            before_first_event: true,
        } => Class::Retryable,
        Failure::Transport {
            before_first_event: false,
        } => Class::Fatal,
        Failure::Api { code, kind, status } => {
            if let Some(class) = code.and_then(classify_code) {
                return class;
            }
            if let Some(class) = kind.and_then(classify_kind) {
                return class;
            }
            match status {
                Some(status) => classify_status(*status),
                None => Class::Fatal,
            }
        }
    }
}

fn classify_code(code: &str) -> Option<Class> {
    match code {
        "rate_limit_exceeded" | "server_error" => Some(Class::Retryable),
        "context_length_exceeded" => Some(Class::ContextOverflow),
        "insufficient_quota"
        | "billing_not_active"
        | "billing_hard_limit_reached"
        | "account_deactivated"
        | "invalid_api_key" => Some(Class::Fatal),
        _ => None,
    }
}

fn classify_kind(kind: &str) -> Option<Class> {
    match kind {
        "server_error" | "api_error" => Some(Class::Retryable),
        "invalid_request_error" => Some(Class::Fatal),
        _ => None,
    }
}

fn classify_status(status: u16) -> Class {
    match status {
        408 | 429 => Class::Retryable,
        500..=599 => Class::Retryable,
        _ => Class::Fatal,
    }
}

/// Bounded exponential backoff with jitter.
///
/// `max_attempts` is the total number of attempts allowed (the initial
/// attempt counts as one), matching `docs/reference/agent-loop.md`'s "3
/// attempts, 2 s base" default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base: Duration,
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    /// 3 attempts, a 2 s base, and pi's 60 s `DEFAULT_MAX_AGENT_RETRY_DELAY_MS`
    /// cap (`packages/ai/src/utils/retry.ts`).
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base: Duration::from_secs(2),
            max_delay: Duration::from_secs(60),
        }
    }
}

impl RetryPolicy {
    /// The deterministic upper bound for `attempt` (1-based): `base *
    /// 2^(attempt - 1)`, capped at `max_delay`. Saturates instead of
    /// overflowing, and is cheap for any `attempt` up to `u32::MAX`:
    /// doubling a nonzero duration outgrows any `max_delay` representable
    /// in a `Duration` well within 64 steps, so the loop never needs more
    /// than that.
    fn cap_for_attempt(&self, attempt: u32) -> Duration {
        let exponent = attempt.saturating_sub(1);
        let mut cap = self.base;
        for _ in 0..exponent.min(64) {
            if cap >= self.max_delay {
                return self.max_delay;
            }
            cap = cap.saturating_mul(2);
        }
        cap.min(self.max_delay)
    }

    /// Delay before retry number `attempt` (1-based), given a jitter
    /// sample in `[0, 1)`.
    ///
    /// Uses "full jitter" (the AWS Architecture Blog's term): the delay
    /// is drawn uniformly from `[0, cap]`, where `cap` is
    /// [`Self::cap_for_attempt`]. `jitter` is that uniform sample,
    /// already scaled to `[0, 1)`; the caller supplies it so this
    /// function stays deterministic. `jitter` is clamped to `[0, 1]`
    /// defensively, so a caller-supplied `1.0` or a value outside range
    /// never panics or exceeds the cap.
    pub fn delay(&self, attempt: u32, jitter: f64) -> Duration {
        let cap = self.cap_for_attempt(attempt);
        scale_duration(cap, jitter.clamp(0.0, 1.0))
    }

    /// Whether another attempt is allowed after `attempts_made` attempts
    /// have already been made.
    pub fn allows(&self, attempts_made: u32) -> bool {
        attempts_made < self.max_attempts
    }

    /// Like [`Self::delay`], but honours a server-provided `Retry-After`
    /// hint when present: the delay is then `min(retry_after,
    /// max_delay)`, and `jitter` is ignored, since the server already
    /// told us how long to wait.
    pub fn delay_with_hint(
        &self,
        attempt: u32,
        jitter: f64,
        retry_after: Option<Duration>,
    ) -> Duration {
        match retry_after {
            Some(hint) => hint.min(self.max_delay),
            None => self.delay(attempt, jitter),
        }
    }
}

/// Scale a duration by a fraction in `[0, 1]`. Durations too large for
/// an `f64` round trip come back unscaled rather than panicking.
fn scale_duration(duration: Duration, fraction: f64) -> Duration {
    Duration::try_from_secs_f64(duration.as_secs_f64() * fraction)
        .map_or(duration, |scaled| scaled.min(duration))
}

/// Parse a `Retry-After`-family header into a [`Duration`].
///
/// Supports `retry-after-ms` (an integer number of milliseconds, as pi's
/// providers send it) and `retry-after` as an integer number of seconds.
/// The HTTP-date form of `retry-after` needs a date library we do not
/// depend on, so it returns `None`, as does any other header or value.
pub fn parse_retry_after(header_name: &str, value: &str) -> Option<Duration> {
    let trimmed = value.trim();
    if header_name.eq_ignore_ascii_case("retry-after-ms") {
        return trimmed.parse::<u64>().ok().map(Duration::from_millis);
    }
    if header_name.eq_ignore_ascii_case("retry-after") {
        return trimmed.parse::<u64>().ok().map(Duration::from_secs);
    }
    None
}
