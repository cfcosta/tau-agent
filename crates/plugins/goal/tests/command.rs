//! How `/goal` reads: a command written out with its limits reads back
//! as itself, a budget is a finite amount of at least zero, and the
//! condition a command set reads back from the input the model gets.

use hegel::{Generator as _, TestCase, generators as gs};
use tau_goal::{
    Command,
    DEFAULT_BUDGET,
    DEFAULT_CONTINUATIONS,
    set_input,
    set_message,
};

/// Whitespace between the words of a command, as a person may type it.
fn gap(tc: &TestCase) -> String {
    tc.draw(gs::sampled_from(vec![" ", "  ", "\t", "\n", " \n "]))
        .to_string()
}

/// A command written out: `/goal`, the limits it names, in the order
/// drawn, and the condition's words, with drawn whitespace between.
/// Returns the text and the command it should read as.
fn written(tc: &TestCase) -> (String, Command) {
    let continuations = tc.draw(gs::optional(gs::integers::<u32>()));
    let budget = tc.draw(gs::optional(
        gs::floats::<f64>()
            .min_value(0.0)
            .allow_nan(false)
            .allow_infinity(false),
    ));
    let dollar = tc.draw(gs::booleans());
    let budget_first = tc.draw(gs::booleans());
    let words = tc.draw(
        gs::vecs(hegel::one_of!(
            gs::from_regex(r"[a-z0-9$.,:-]{1,8}"),
            gs::just(String::from("clear")),
            gs::just(String::from("--budget")),
        ))
        .min_size(1)
        .max_size(5),
    );
    // A condition cannot start with a limit: it would be read as one.
    tc.assume(!words[0].starts_with("--"));

    let mut limits = Vec::new();
    if let Some(n) = continuations {
        limits.push(format!("--continuations{}{n}", gap(tc)));
    }
    if let Some(usd) = budget {
        let sign = if dollar { "$" } else { "" };
        limits.push(format!("--budget{}{sign}{usd}", gap(tc)));
    }
    if budget_first {
        limits.reverse();
    }
    let mut text = format!("{}/goal", gap(tc));
    for part in limits.iter().chain(&words) {
        text.push_str(&gap(tc));
        text.push_str(part);
    }
    text.push_str(&gap(tc));

    let condition = words.join(" ");
    let command = if condition == "clear" {
        tc.event("clear");
        Command::Clear
    } else {
        if continuations.is_some() || budget.is_some() {
            tc.event("limits");
        }
        Command::Set {
            condition,
            continuations: continuations.unwrap_or(DEFAULT_CONTINUATIONS),
            budget: budget.unwrap_or(DEFAULT_BUDGET),
        }
    };
    (text, command)
}

/// A command written out with any limits, in either order, with any
/// whitespace between its words, reads as those limits and the
/// condition's words joined by single spaces; `clear` alone clears.
#[hegel::test]
fn a_written_command_reads_as_itself(tc: TestCase) {
    let (text, command) = written(&tc);
    assert_eq!(Command::parse(&text), Some(command), "{text:?}");
}

/// A `--budget` is accepted exactly when it is a finite number of at
/// least zero: `nan`, `inf`, an overflow and negative amounts make the
/// input not a command.
#[hegel::test]
fn a_budget_is_a_finite_amount_not_below_zero(tc: TestCase) {
    let amount = tc.draw(hegel::one_of!(
        gs::floats::<f64>().map(|x| x.to_string()),
        gs::sampled_from(vec![
            "nan", "NaN", "inf", "-inf", "infinity", "-3", "-0.5", "-0",
            "1e400", "0", "0.25", "1e3",
        ])
        .map(String::from),
    ));
    let text = format!("/goal --budget {amount} tests pass");
    let expected = amount.parse::<f64>().unwrap();
    let accepted = expected.is_finite() && expected >= 0.0;
    tc.event(if accepted { "accepted" } else { "refused" });
    match Command::parse(&text) {
        Some(Command::Set { budget, .. }) => {
            assert!(accepted, "{text:?} read as a budget of {budget}");
            assert_eq!(budget, expected);
        }
        other => assert!(!accepted, "{text:?} read as {other:?}"),
    }
}

/// The condition a `/goal` sets reads back from the input the model
/// gets for it, whatever the person typed around it.
#[hegel::test]
fn a_set_condition_reads_back_from_the_models_input(tc: TestCase) {
    let typed = if tc.draw(gs::booleans()) {
        format!("/goal {}", tc.draw(gs::text()))
    } else {
        written(&tc).0
    };
    let Some(Command::Set { condition, .. }) = Command::parse(&typed) else {
        tc.reject();
    };
    assert_eq!(set_message(&set_input(&condition)), Some(condition));
}

/// Any condition reads back from the input the model gets for it: one
/// that would not read back written as is is quoted.
#[hegel::test]
fn any_condition_reads_back_from_the_models_input(tc: TestCase) {
    let condition = tc.draw(gs::text());
    tc.assume(!condition.trim().is_empty());
    assert_eq!(set_message(&set_input(&condition)), Some(condition));
}

/// An ordinary condition is written as is, as a person would type it,
/// quotes that are not one valid quoted string included.
#[test]
fn an_ordinary_condition_is_written_as_is() {
    for condition in ["cargo test passes", r#""a" and "b""#, r#""a\b""#] {
        let input = set_input(condition);
        assert_eq!(
            input.lines().next(),
            Some(format!("/goal {condition}").as_str()),
        );
    }
}

/// Each condition that would not read back as written is quoted, and
/// reads back exactly.
#[test]
fn a_colliding_condition_is_quoted() {
    for (condition, first_line) in [
        ("clear", r#"/goal "clear""#),
        ("--budget 5 tests pass", r#"/goal "--budget 5 tests pass""#),
        ("--continuations 3 x", r#"/goal "--continuations 3 x""#),
        ("tests\npass", r#"/goal "tests\npass""#),
        ("a\r\nb", r#"/goal "a\r\nb""#),
        ("tests  pass", r#"/goal "tests  pass""#),
        (" tests pass", r#"/goal " tests pass""#),
        (r#""quoted""#, r#"/goal "\"quoted\"""#),
        (r#""a\\b""#, r#"/goal "\"a\\\\b\"""#),
    ] {
        let input = set_input(condition);
        assert_eq!(input.lines().next(), Some(first_line), "{condition:?}");
        assert_eq!(set_message(&input).as_deref(), Some(condition));
    }
}

/// A quoted condition is read exactly, after any limits; one that is
/// not a whole quoted string reads as words, quotes included.
#[test]
fn a_quoted_condition_reads_exactly() {
    assert_eq!(
        Command::parse(r#"/goal --budget 3 "clear""#),
        Some(Command::Set {
            condition: "clear".into(),
            continuations: DEFAULT_CONTINUATIONS,
            budget: 3.0,
        }),
    );
    assert_eq!(
        Command::parse(r#"/goal "a" and  "b""#),
        Some(Command::Set {
            condition: r#""a" and "b""#.into(),
            continuations: DEFAULT_CONTINUATIONS,
            budget: DEFAULT_BUDGET,
        }),
    );
    assert_eq!(Command::parse(r#"/goal "  ""#), None);
}
