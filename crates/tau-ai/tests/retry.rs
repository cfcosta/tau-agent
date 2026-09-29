//! Retry classification and backoff: see `docs/reference/agent-loop.md`
//! ("Retries") and `crates/tau-ai/src/retry.rs`'s module doc for the
//! classification table this file tests exhaustively.

use std::time::Duration;

use hegel::{TestCase, generators as gs};
use tau_ai::retry::{Class, Failure, classify, parse_retry_after};
use tau_testing::generators;

/// Every code, kind and status in the classification table, plus the
/// negatives that matter: `rate_limit_exceeded`/429 is retryable, not
/// overflow; `insufficient_quota` is fatal even at 429 (pi's bug, see
/// `docs/reference/pi-audit.md`); a transport error after the first
/// event is fatal even though one before it is retryable; and a known
/// `code` or `kind` wins over a status that would otherwise disagree.
const CASES: &[(Failure<'static>, Class)] = &[
    // A known `kind` wins over a status that alone would be retryable.
    (
        Failure::Api {
            code: None,
            kind: Some("invalid_request_error"),
            status: Some(500),
        },
        Class::Fatal,
    ),
    // `code`, alone and in combination with a status.
    (
        Failure::Api {
            code: Some("rate_limit_exceeded"),
            kind: None,
            status: Some(429),
        },
        Class::Retryable,
    ),
    (
        Failure::Api {
            code: Some("rate_limit_exceeded"),
            kind: None,
            status: None,
        },
        Class::Retryable,
    ),
    (
        Failure::Api {
            code: Some("server_error"),
            kind: None,
            status: Some(500),
        },
        Class::Retryable,
    ),
    (
        Failure::Api {
            code: Some("context_length_exceeded"),
            kind: None,
            status: Some(400),
        },
        Class::ContextOverflow,
    ),
    (
        Failure::Api {
            code: Some("context_length_exceeded"),
            kind: None,
            status: None,
        },
        Class::ContextOverflow,
    ),
    (
        // Negative: a rate limit is never overflow, even though both
        // are 429s and pi has had this exact confusion.
        Failure::Api {
            code: Some("insufficient_quota"),
            kind: None,
            status: Some(429),
        },
        Class::Fatal,
    ),
    (
        Failure::Api {
            code: Some("billing_not_active"),
            kind: None,
            status: Some(403),
        },
        Class::Fatal,
    ),
    (
        Failure::Api {
            code: Some("billing_hard_limit_reached"),
            kind: None,
            status: Some(429),
        },
        Class::Fatal,
    ),
    (
        Failure::Api {
            code: Some("account_deactivated"),
            kind: None,
            status: Some(403),
        },
        Class::Fatal,
    ),
    (
        Failure::Api {
            code: Some("invalid_api_key"),
            kind: None,
            status: Some(401),
        },
        Class::Fatal,
    ),
    (
        // Precedence: a known `code` wins even when `status` alone would
        // classify differently (400 alone is Fatal).
        Failure::Api {
            code: Some("rate_limit_exceeded"),
            kind: None,
            status: Some(400),
        },
        Class::Retryable,
    ),
    // `kind`, used only when `code` is absent or unknown.
    (
        Failure::Api {
            code: None,
            kind: Some("server_error"),
            status: Some(500),
        },
        Class::Retryable,
    ),
    (
        Failure::Api {
            code: None,
            kind: Some("api_error"),
            status: None,
        },
        Class::Retryable,
    ),
    (
        Failure::Api {
            code: None,
            kind: Some("invalid_request_error"),
            status: Some(400),
        },
        Class::Fatal,
    ),
    (
        // Precedence: a known `kind` wins even when `status` alone would
        // classify differently.
        Failure::Api {
            code: None,
            kind: Some("server_error"),
            status: Some(400),
        },
        Class::Retryable,
    ),
    (
        // An unknown `code` falls through to a known `kind`.
        Failure::Api {
            code: Some("some_future_code"),
            kind: Some("server_error"),
            status: None,
        },
        Class::Retryable,
    ),
    // `status`, used only when neither `code` nor `kind` is known.
    (
        Failure::Api {
            code: None,
            kind: None,
            status: Some(408),
        },
        Class::Retryable,
    ),
    (
        Failure::Api {
            code: None,
            kind: None,
            status: Some(429),
        },
        Class::Retryable,
    ),
    (
        Failure::Api {
            code: None,
            kind: None,
            status: Some(500),
        },
        Class::Retryable,
    ),
    (
        Failure::Api {
            code: None,
            kind: None,
            status: Some(599),
        },
        Class::Retryable,
    ),
    (
        Failure::Api {
            code: None,
            kind: None,
            status: Some(400),
        },
        Class::Fatal,
    ),
    (
        Failure::Api {
            code: None,
            kind: None,
            status: Some(404),
        },
        Class::Fatal,
    ),
    (
        Failure::Api {
            code: None,
            kind: None,
            status: Some(409),
        },
        Class::Fatal,
    ),
    (
        // An unknown `code`, with no `kind`, falls through to `status`.
        Failure::Api {
            code: Some("some_future_code"),
            kind: None,
            status: Some(503),
        },
        Class::Retryable,
    ),
    (
        Failure::Api {
            code: None,
            kind: None,
            status: None,
        },
        Class::Fatal,
    ),
    // Transport failures.
    (
        Failure::Transport {
            before_first_event: true,
        },
        Class::Retryable,
    ),
    (
        // Negative: once the stream has started, a transport drop is
        // fatal for this module; the agent sees a proper `error` event
        // instead (see the module doc comment).
        Failure::Transport {
            before_first_event: false,
        },
        Class::Fatal,
    ),
];

#[test]
fn classify_table() {
    for (failure, expected) in CASES {
        assert_eq!(classify(failure), *expected, "{failure:?}");
    }
}

/// Full jitter, against a reference in integer nanoseconds: the delay is
/// `jitter × min(max_delay, base × 2^(attempt−1))`, to within the
/// rounding of one `f64` multiplication.
#[hegel::test(test_cases = 500)]
fn delay_matches_full_jitter_reference(tc: TestCase) {
    let policy = tc.draw(generators::retry_policy());
    let attempt = tc.draw(gs::integers::<u32>().min_value(1).max_value(80));
    let jitter =
        tc.draw(gs::floats::<f64>().min_value(0.0).max_value_exclusive(1.0));
    let base = policy.base.as_nanos();
    let cap = (0..attempt - 1)
        .fold(base, |d, _| d.saturating_mul(2).min(u128::MAX / 4))
        .min(policy.max_delay.as_nanos());
    let expected = cap as f64 * jitter;
    let got = policy.delay(attempt, jitter).as_nanos() as f64;
    tc.note(&format!("cap = {cap} ns, expected = {expected} ns"));
    assert!((got - expected).abs() <= 1_000.0, "{got} vs {expected}");
}

/// Backoff delay for attempt `n` lies in `[0, base * 2^(n-1)]`, capped at
/// `max_delay`, and never panics however large `attempt` is.
#[hegel::test(test_cases = 500)]
fn delay_is_bounded(tc: TestCase) {
    let policy = tc.draw(generators::retry_policy());
    let attempt =
        tc.draw(gs::integers::<u32>().min_value(1).max_value(u32::MAX));
    let jitter = tc.draw(
        gs::floats::<f64>()
            .min_value(0.0)
            .max_value(1.0)
            .allow_nan(false)
            .allow_infinity(false),
    );

    let delay = policy.delay(attempt, jitter);

    let exponent = attempt.saturating_sub(1).min(1024);
    let unclamped_cap = policy.base.as_secs_f64() * 2f64.powi(exponent as i32);
    let cap = unclamped_cap.min(policy.max_delay.as_secs_f64());
    tc.note(&format!("cap = {cap}s, delay = {}s", delay.as_secs_f64()));

    // A small epsilon absorbs `Duration`'s nanosecond rounding.
    assert!(delay.as_secs_f64() <= cap + 1e-6);
    assert!(delay <= policy.max_delay);
}

/// The deterministic upper bound (full jitter at `jitter = 1.0`) never
/// shrinks as `attempt` grows: `cap(n) <= cap(n + 1)`.
#[hegel::test(test_cases = 500)]
fn upper_bound_is_monotone_in_attempt(tc: TestCase) {
    let policy = tc.draw(generators::retry_policy());
    let attempt =
        tc.draw(gs::integers::<u32>().min_value(1).max_value(u32::MAX - 1));

    let cap_n = policy.delay(attempt, 1.0);
    let cap_next = policy.delay(attempt + 1, 1.0);

    assert!(cap_next >= cap_n);
}

/// `allows` is a model of "at most `max_attempts` attempts total": a
/// loop that keeps asking permission and counting attempts stops after
/// exactly `max_attempts` of them.
#[hegel::test(test_cases = 500)]
fn allows_permits_exactly_max_attempts(tc: TestCase) {
    let policy = tc.draw(generators::retry_policy());

    // Bounded, so a policy that allows everything fails instead of hanging.
    let count = (0..=policy.max_attempts + 1)
        .take_while(|&attempts_made| policy.allows(attempts_made))
        .count();
    assert_eq!(count, policy.max_attempts as usize);
}

/// A server-provided `Retry-After` hint is honoured verbatim, capped at
/// `max_delay`, and ignores the sampled jitter entirely.
#[hegel::test(test_cases = 500)]
fn hint_caps_at_max_delay(tc: TestCase) {
    let policy = tc.draw(generators::retry_policy());
    let attempt = tc.draw(gs::integers::<u32>().min_value(1).max_value(1000));
    let jitter = tc.draw(
        gs::floats::<f64>()
            .min_value(0.0)
            .max_value(1.0)
            .allow_nan(false)
            .allow_infinity(false),
    );
    let hint = Duration::from_millis(
        tc.draw(gs::integers::<u64>().max_value(120_000)),
    );

    let delay = policy.delay_with_hint(attempt, jitter, Some(hint));

    assert_eq!(delay, hint.min(policy.max_delay));
}

/// The `code`s `classify` knows, with their class.
const KNOWN_CODES: [(&str, Class); 8] = [
    ("rate_limit_exceeded", Class::Retryable),
    ("server_error", Class::Retryable),
    ("context_length_exceeded", Class::ContextOverflow),
    ("insufficient_quota", Class::Fatal),
    ("billing_not_active", Class::Fatal),
    ("billing_hard_limit_reached", Class::Fatal),
    ("account_deactivated", Class::Fatal),
    ("invalid_api_key", Class::Fatal),
];

/// The `type`s (`kind`s) `classify` knows, with their class.
const KNOWN_KINDS: [(&str, Class); 3] = [
    ("server_error", Class::Retryable),
    ("api_error", Class::Retryable),
    ("invalid_request_error", Class::Fatal),
];

/// The classification table as a model: the first of `code`, `kind` and
/// `status` that is known decides; with none known, it is fatal.
fn classify_model(
    code: Option<&str>,
    kind: Option<&str>,
    status: Option<u16>,
) -> Class {
    let lookup = |table: &[(&str, Class)], name: Option<&str>| {
        table
            .iter()
            .find(|(known, _)| Some(*known) == name)
            .map(|(_, class)| *class)
    };
    lookup(&KNOWN_CODES, code)
        .or_else(|| lookup(&KNOWN_KINDS, kind))
        .or_else(|| match status? {
            408 | 429 | 500..=599 => Some(Class::Retryable),
            _ => Some(Class::Fatal),
        })
        .unwrap_or(Class::Fatal)
}

/// A `code` or `kind`: a name `classify` knows (as either field), or
/// arbitrary text.
#[hegel::composite]
fn error_name(tc: &TestCase) -> String {
    let known: Vec<String> = KNOWN_CODES
        .iter()
        .chain(&KNOWN_KINDS)
        .map(|(name, _)| (*name).to_owned())
        .collect();
    tc.draw(hegel::one_of!(gs::sampled_from(known), generators::text(8)))
}

/// `classify` agrees with the table's model over any mix of known and
/// unknown `code`s and `kind`s and any status at all: 1xx to 3xx and
/// 600 and up are fatal, like every status the table does not name.
#[hegel::test(test_cases = 500)]
fn classify_matches_the_first_known_wins_model(tc: TestCase) {
    let code = tc.draw(gs::optional(error_name()));
    let kind = tc.draw(gs::optional(error_name()));
    let status = tc.draw(gs::optional(gs::integers::<u16>()));
    let failure = Failure::Api {
        code: code.as_deref(),
        kind: kind.as_deref(),
        status,
    };
    assert_eq!(
        classify(&failure),
        classify_model(code.as_deref(), kind.as_deref(), status),
        "{failure:?}"
    );
}

/// Metamorphic half of the classification table: any 5xx status with no
/// known `code` or `kind` is retryable.
#[hegel::test(test_cases = 500)]
fn any_server_status_without_code_is_retryable(tc: TestCase) {
    let status = tc.draw(gs::integers::<u16>().min_value(500).max_value(599));
    let failure = Failure::Api {
        code: None,
        kind: None,
        status: Some(status),
    };
    assert_eq!(classify(&failure), Class::Retryable);
}

/// Metamorphic half of the classification table: any 4xx status with no
/// known `code` or `kind`, other than the two documented retryable
/// statuses (408, 429), is fatal.
#[hegel::test(test_cases = 500)]
fn other_4xx_without_code_is_fatal(tc: TestCase) {
    let status = tc.draw(gs::integers::<u16>().min_value(400).max_value(499));
    // Only 408 and 429 are excluded out of a 100-value range, so the
    // rejection rate is low.
    tc.assume(status != 408 && status != 429);
    let failure = Failure::Api {
        code: None,
        kind: None,
        status: Some(status),
    };
    assert_eq!(classify(&failure), Class::Fatal);
}

/// `retry-after` is seconds; any `u32` round-trips exactly.
#[hegel::test(test_cases = 500)]
fn parse_retry_after_seconds(tc: TestCase) {
    let seconds = tc.draw(gs::integers::<u32>());
    let parsed = parse_retry_after("retry-after", &seconds.to_string());
    assert_eq!(parsed, Some(Duration::from_secs(u64::from(seconds))));
}

/// `retry-after-ms` is milliseconds; any `u32` round-trips exactly.
#[hegel::test(test_cases = 500)]
fn parse_retry_after_ms(tc: TestCase) {
    let millis = tc.draw(gs::integers::<u32>());
    let parsed = parse_retry_after("retry-after-ms", &millis.to_string());
    assert_eq!(parsed, Some(Duration::from_millis(u64::from(millis))));
}

/// A `Retry-After` value: a near miss of a plain integer (a sign, a
/// fraction, too many digits, whitespace around it) or arbitrary text.
#[hegel::composite]
fn retry_after_value(tc: &TestCase) -> String {
    tc.draw(hegel::one_of!(
        gs::from_regex(r"[ \t]{0,2}[+-]?[0-9]{0,21}(\.[0-9]{0,2})?[ \t]{0,2}"),
        generators::text(16),
    ))
}

/// `retry-after` or `retry-after-ms`, each letter in either case.
#[hegel::composite]
fn retry_after_header(tc: &TestCase) -> String {
    let name = tc.draw(gs::sampled_from(vec!["retry-after", "retry-after-ms"]));
    name.chars()
        .map(|c| {
            if tc.draw(gs::booleans()) {
                c.to_ascii_uppercase()
            } else {
                c
            }
        })
        .collect()
}

/// Either header, in any letter case, is a whole number after trimming
/// whitespace, or nothing: seconds for `retry-after`, milliseconds for
/// `retry-after-ms`. Anything else (a negative or fractional number,
/// one past `u64::MAX`, blank or arbitrary text, an HTTP-date; see the
/// doc comment on [`parse_retry_after`]) is not a supported value. A
/// leading `+` is accepted, as Rust's integer parsing and pi's
/// `Number()` both do.
#[hegel::test(test_cases = 500)]
#[hegel::explicit_test_case(header = String::from("retry-after"), value = String::from("-1"))]
#[hegel::explicit_test_case(header = String::from("Retry-After"), value = String::from("+5"))]
#[hegel::explicit_test_case(header = String::from("retry-after"), value = String::from("1.5"))]
#[hegel::explicit_test_case(header = String::from("RETRY-AFTER-MS"), value = String::from(" "))]
#[hegel::explicit_test_case(
    header = String::from("retry-after"),
    value = String::from("18446744073709551616"),
)]
#[hegel::explicit_test_case(
    header = String::from("retry-after-ms"),
    value = String::from(" \t18446744073709551615 "),
)]
fn parse_retry_after_is_a_trimmed_whole_number(tc: TestCase) {
    let header = tc.draw(retry_after_header());
    let value = tc.draw(retry_after_value());
    let number = value.trim().parse::<u64>().ok();
    let expected = if header.eq_ignore_ascii_case("retry-after-ms") {
        number.map(Duration::from_millis)
    } else {
        number.map(Duration::from_secs)
    };
    assert_eq!(parse_retry_after(&header, &value), expected);
}

/// An unrecognized header name is never parsed, whatever its value.
#[hegel::test(test_cases = 500)]
fn parse_retry_after_unknown_header_is_none(tc: TestCase) {
    let seconds = tc.draw(gs::integers::<u32>());
    assert_eq!(
        parse_retry_after("x-unrelated-header", &seconds.to_string()),
        None
    );
}
