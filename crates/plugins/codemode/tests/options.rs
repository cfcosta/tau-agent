//! The options line.
//!
//! | Property | Oracle |
//! | --- | --- |
//! | parse ∘ print is the identity | round trip |
//! | an unknown key fails with pi's message | exact text |

use hegel::{TestCase, generators as gs};
use serde_json::json;
use tau_codemode::options::{self, Options};

#[hegel::composite]
fn options_value(tc: &TestCase) -> (Option<u64>, Option<u64>) {
    let tokens = tc.draw(gs::optional(gs::integers::<u64>()));
    let timeout = tc.draw(gs::optional(
        gs::integers::<u64>()
            .min_value(1)
            .max_value(options::MAX_TIMEOUT_MS),
    ));
    (tokens, timeout)
}

#[hegel::test]
fn options_line_round_trips(tc: TestCase) {
    let (max_output_tokens, timeout_ms) = tc.draw(options_value());
    let options = Options {
        max_output_tokens,
        timeout_ms,
    };
    // Code that is not blank, so the line has something after it.
    let body: String = tc.draw(gs::text().max_size(40));
    let input = format!("{}\nreturn 1{body}", options.to_line());
    let source = options::parse(&input).expect("a printed line parses");
    assert_eq!(source.options, options);
    assert_eq!(source.code, input);
}

#[hegel::test]
fn options_line_refuses_unknown_keys(tc: TestCase) {
    let key: String = tc.draw(gs::from_regex("[a-z_]{1,12}"));
    tc.assume(key != "max_output_tokens" && key != "timeout_ms");
    let input = format!("-- @options: {}\nreturn 1", json!({ key.clone(): 1 }));
    let error = options::parse(&input).expect_err("unknown keys fail");
    assert_eq!(
        error.0,
        format!(
            "@options only supports `max_output_tokens` and `timeout_ms`; got `{key}`"
        )
    );
}

#[test]
fn options_line_errors() {
    let fails = |input: &str| options::parse(input).unwrap_err().0;
    assert_eq!(
        fails("-- @options: {\"yield\": 1}\nx()"),
        "@options only supports `max_output_tokens` and `timeout_ms`; got `yield`"
    );
    assert!(
        fails("-- @options: {nope}\nx()")
            .starts_with("@options is not valid JSON")
    );
    assert_eq!(
        fails("-- @options: [1]\nx()"),
        "@options must be a JSON object."
    );
    assert_eq!(
        fails("-- @options: {}\n  \n"),
        "@options must be followed by code on the next lines."
    );
    assert!(fails("  \n").starts_with("The code is empty"));
    assert!(
        fails("-- @options: {\"timeout_ms\": 0}\nx()")
            .starts_with("`timeout_ms`")
    );
    assert!(
        fails("-- @options: {\"timeout_ms\": 2147483648}\nx()")
            .starts_with("`timeout_ms`")
    );
    assert!(
        fails("-- @options: {\"max_output_tokens\": -1}\nx()")
            .starts_with("`max_output_tokens`")
    );
    let plain = options::parse("return 1").unwrap();
    assert_eq!(plain.options.max_output_tokens(), 10_000);
    assert_eq!(plain.options.timeout_ms, None);
}
