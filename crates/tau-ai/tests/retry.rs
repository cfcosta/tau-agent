//! Retry classification and backoff: see `docs/reference/agent-loop.md`
//! ("Retries") and `crates/tau-ai/src/retry.rs`'s module doc for the
//! classification table this file tests exhaustively.

use std::time::Duration;

use hegel::{TestCase, generators as gs};
use tau_ai::retry::{Class, Failure, classify, parse_retry_after};
use tau_testing::generators;

/// A transport failure is retryable before the first event and fatal
/// after it: once the stream has started, resending would repeat what
/// went out, and the agent sees an `error` event instead (see the module
/// doc comment).
#[test]
fn transport_failures_retry_only_before_the_first_event() {
    let transport = |before_first_event| {
        classify(&Failure::Transport { before_first_event })
    };
    assert_eq!(transport(true), Class::Retryable);
    assert_eq!(transport(false), Class::Fatal);
}

/// Full jitter, against a reference in integer nanoseconds: the delay is
/// `jitter × min(max_delay, base × 2^(attempt−1))`, to within the
/// rounding of one `f64` multiplication, for any attempt up to
/// `u32::MAX` and any jitter in `[0, 1]`. So the delay never passes the
/// cap, and the cap (at jitter 1) never shrinks as attempts grow.
#[hegel::test(test_cases = 500)]
fn delay_matches_full_jitter_reference(tc: TestCase) {
    let policy = tc.draw(generators::retry_policy());
    let attempt = tc.draw(hegel::one_of!(
        gs::integers::<u32>().min_value(1).max_value(80),
        gs::integers::<u32>().min_value(1),
        gs::sampled_from(vec![64, 65, 66, 257, 65_537, u32::MAX - 1, u32::MAX]),
    ));
    let jitter = tc.draw(hegel::one_of!(
        gs::floats::<f64>().min_value(0.0).max_value(1.0),
        gs::sampled_from(vec![0.0, 1.0]),
    ));
    let base = policy.base.as_nanos();
    // 200 doublings outgrow any `max_delay`; past them the cap is flat.
    let cap = (0..(attempt - 1).min(200))
        .fold(base, |d, _| d.saturating_mul(2).min(u128::MAX / 4))
        .min(policy.max_delay.as_nanos());
    let expected = cap as f64 * jitter;
    let got = policy.delay(attempt, jitter).as_nanos() as f64;
    tc.note(&format!("cap = {cap} ns, expected = {expected} ns"));
    assert!((got - expected).abs() <= 1_000.0, "{got} vs {expected}");
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

/// A server-provided `Retry-After` hint is the least the backoff waits,
/// and `max_delay` the most.
#[hegel::test(test_cases = 500)]
fn a_hint_bounds_the_backoff_from_below(tc: TestCase) {
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

    assert_eq!(
        delay,
        policy
            .delay(attempt, jitter)
            .max(hint)
            .min(policy.max_delay)
    );
}

/// The `code`s `classify` knows, with their class: OpenAI's, and the
/// ChatGPT plan's documented codes, of which only the two that ask to
/// retry later are retryable (a usage limit never loops).
const KNOWN_CODES: [(&str, Class); 17] = [
    ("subscription_sharing_user_not_eligible", Class::Fatal),
    ("subscription_sharing_usage_limit_exceeded", Class::Fatal),
    ("subscription_sharing_usage_unavailable", Class::Retryable),
    ("subscription_sharing_unsupported_capability", Class::Fatal),
    ("subscription_sharing_route_not_supported", Class::Fatal),
    ("subscription_sharing_invalid_user", Class::Fatal),
    ("chatpass_v2_scope_not_authorized", Class::Fatal),
    ("chatpass_v2_invalid_authorization_context", Class::Fatal),
    ("subscription_sharing_user_unavailable", Class::Retryable),
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
///
/// Pinned: `insufficient_quota` is fatal even at 429 (pi's bug, see
/// `docs/reference/pi-audit.md`), a known `code` or `kind` wins over
/// a status that alone would classify otherwise, and the plan's usage
/// limit stays fatal under a retryable type and status.
#[hegel::test(test_cases = 500)]
#[hegel::explicit_test_case(
    code = Some("insufficient_quota".to_owned()),
    kind = None::<String>,
    status = Some(429u16),
)]
#[hegel::explicit_test_case(
    code = Some("rate_limit_exceeded".to_owned()),
    kind = None::<String>,
    status = Some(400u16),
)]
#[hegel::explicit_test_case(
    code = None::<String>,
    kind = Some("invalid_request_error".to_owned()),
    status = Some(500u16),
)]
#[hegel::explicit_test_case(
    code = Some("subscription_sharing_usage_limit_exceeded".to_owned()),
    kind = Some("server_error".to_owned()),
    status = Some(429u16),
)]
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

/// The ordinary status table is exhaustive for 4xx and 5xx statuses: only
/// 408, 429 and 500..=599 retry. In particular, 409 follows the ordinary
/// fatal rule.
#[test]
fn ordinary_statuses_without_code_or_kind_follow_the_retry_table() {
    for status in 400..=599 {
        let expected = if matches!(status, 408 | 429 | 500..=599) {
            Class::Retryable
        } else {
            Class::Fatal
        };
        let failure = Failure::Api {
            code: None,
            kind: None,
            status: Some(status),
        };
        assert_eq!(classify(&failure), expected, "status {status}");
    }
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
#[hegel::explicit_test_case(header = String::from("retry-after"), value = String::from("0"))]
#[hegel::explicit_test_case(header = String::from("retry-after-ms"), value = String::from("0"))]
#[hegel::explicit_test_case(header = String::from("retry-after"), value = String::from("1"))]
#[hegel::explicit_test_case(header = String::from("retry-after-ms"), value = String::from("1"))]
#[hegel::explicit_test_case(
    header = String::from("retry-after"),
    value = String::from("4294967295"),
)]
#[hegel::explicit_test_case(
    header = String::from("retry-after-ms"),
    value = String::from("4294967295"),
)]
#[hegel::explicit_test_case(
    header = String::from("ReTrY-AfTeR"),
    value = String::from(" \t18446744073709551615 \t"),
)]
#[hegel::explicit_test_case(
    header = String::from("rEtRy-AfTeR-Ms"),
    value = String::from(" \t18446744073709551615 \t"),
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

/// Inventory: `classify_matches_the_first_known_wins_model` compares against
/// the independent known-code/kind/status table; optional names and all `u16`
/// statuses are generated directly, and shrinking keeps those inputs valid.
/// The exhaustive ordinary-status table has a literal status-set oracle.
/// `parse_retry_after_is_a_trimmed_whole_number` uses trimmed `u64` parsing
/// and independent `Duration` constructors as its oracle; bounded text and
/// case choices shrink toward short values and canonical lowercase headers.
/// This property constructs each unsupported header from a known name and a
/// fixed prefix, suffix, or first-letter removal, without filtering. Its
/// canonical numeric control checks units independently; value and form
/// choices shrink to a small, intelligible unsupported-header example.
#[hegel::test(test_cases = 500)]
fn parse_retry_after_unknown_header_is_none(tc: TestCase) {
    let known_header = tc.draw(gs::sampled_from(vec![
        "retry-after".to_owned(),
        "retry-after-ms".to_owned(),
    ]));
    let unsupported_form =
        tc.draw(gs::sampled_from(vec!["prefix", "suffix", "remove-first"]));
    let unknown_header = match unsupported_form {
        "prefix" => format!("x-{known_header}"),
        "suffix" => format!("{known_header}-x"),
        "remove-first" => known_header.chars().skip(1).collect(),
        _ => unreachable!("unsupported form came from a fixed generator"),
    };

    let number = tc.draw(gs::integers::<u64>());
    let unknown_value_kind =
        tc.draw(gs::sampled_from(vec!["numeric", "invalid", "whitespace"]));
    let unknown_value = match unknown_value_kind {
        "numeric" => number.to_string(),
        "invalid" => format!("not-an-integer{}", tc.draw(generators::text(8))),
        "whitespace" => tc.draw(gs::from_regex(r"[ \t]{1,8}")),
        _ => unreachable!("unknown value kind came from a fixed generator"),
    };

    let canonical_value = number.to_string();
    let expected_known = if known_header == "retry-after-ms" {
        Duration::from_millis(number)
    } else {
        Duration::from_secs(number)
    };
    assert_eq!(
        parse_retry_after(&known_header, &canonical_value),
        Some(expected_known),
        "known header positive control: {known_header}"
    );
    assert_eq!(
        parse_retry_after(&unknown_header, &unknown_value),
        None,
        "unsupported header: {unknown_header:?}, value: {unknown_value:?}"
    );
}
