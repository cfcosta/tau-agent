//! Scripts as the cards show them: formatted when they parse, as they
//! came when they do not.
//!
//! | Property | Oracle |
//! | --- | --- |
//! | a parsed script reads one statement a line, blocks indented by two, the options line on top, its tokens unchanged | layout model of the generated statements |
//! | formatting a formatted script changes nothing | idempotence |
//!
//! The generator builds valid Luau from a small statement grammar (locals,
//! `+=`, `for`, `if`, a final `return`) and lays its tokens out with
//! arbitrary spaces and line breaks, so a one-line script and a ragged
//! one are both drawn. Shrinking removes statements and nesting.

use hegel::{TestCase, generators as gs};
use tau_codemode::format::formatted;

/// One statement of a generated script.
#[derive(Debug, Clone)]
enum Statement {
    Local(&'static str, String),
    Add(&'static str, u16),
    For(&'static str, u16, u16, Vec<Statement>),
    If(&'static str, Vec<Statement>),
}

hegel::pretty_print_as_debug!(Statement);

const NAMES: [&str; 4] = ["n", "total", "count", "item"];

/// An expression a statement can hold, as one token: a number, a name, a
/// string, or a tool call written compactly.
#[hegel::composite]
fn expression(tc: &TestCase) -> String {
    match tc.draw(gs::integers::<u8>().max_value(3)) {
        0 => tc.draw(gs::integers::<u16>().max_value(999)).to_string(),
        1 => tc.draw(gs::sampled_from(NAMES.to_vec())).to_owned(),
        2 => format!(
            "\"{}\"",
            tc.draw(gs::from_regex("[a-z ]{0,8}").fullmatch(true))
        ),
        _ => format!(
            "tools.{}({{{}={}}})",
            tc.draw(gs::sampled_from(vec!["read", "grep"])),
            tc.draw(gs::sampled_from(NAMES.to_vec())),
            tc.draw(gs::integers::<u16>().max_value(99))
        ),
    }
}

/// One to three statements; blocks nest at most `depth` deep.
#[hegel::composite]
fn block(tc: &TestCase, depth: u8) -> Vec<Statement> {
    let count = tc.draw(gs::integers::<usize>().min_value(1).max_value(3));
    (0..count)
        .map(|_| {
            let name = tc.draw(gs::sampled_from(NAMES.to_vec()));
            let kinds = if depth == 0 { 1 } else { 3 };
            match tc.draw(gs::integers::<u8>().max_value(kinds)) {
                0 => Statement::Local(name, tc.draw(expression())),
                1 => Statement::Add(
                    name,
                    tc.draw(gs::integers::<u16>().max_value(999)),
                ),
                2 => Statement::For(
                    name,
                    tc.draw(gs::integers::<u16>().max_value(9)),
                    tc.draw(gs::integers::<u16>().max_value(99)),
                    tc.draw(block(depth - 1)),
                ),
                _ => Statement::If(name, tc.draw(block(depth - 1))),
            }
        })
        .collect()
}

/// The statement's tokens, in order.
fn tokens(statement: &Statement, out: &mut Vec<String>) {
    let mut push = |items: &[&str]| {
        out.extend(items.iter().map(|item| (*item).to_owned()));
    };
    match statement {
        Statement::Local(name, value) => push(&["local", name, "=", value]),
        Statement::Add(name, value) => push(&[name, "+=", &value.to_string()]),
        Statement::For(name, from, to, body) => {
            push(&[
                "for",
                name,
                "=",
                &from.to_string(),
                ",",
                &to.to_string(),
                "do",
            ]);
            body.iter().for_each(|statement| tokens(statement, out));
            out.push("end".into());
        }
        Statement::If(name, body) => {
            push(&["if", name, "then"]);
            body.iter().for_each(|statement| tokens(statement, out));
            out.push("end".into());
        }
    }
}

/// The lines the formatted statement reads as, by depth, whitespace
/// removed: one a statement, `end` on its own line.
fn lines(statement: &Statement, depth: usize, out: &mut Vec<(usize, String)>) {
    let line = |out: &mut Vec<(usize, String)>, text: String| {
        out.push((depth, text));
    };
    match statement {
        Statement::Local(name, value) => {
            line(out, format!("local{name}={}", strip(value)));
        }
        Statement::Add(name, value) => line(out, format!("{name}+={value}")),
        Statement::For(name, from, to, body) => {
            line(out, format!("for{name}={from},{to}do"));
            body.iter().for_each(|s| lines(s, depth + 1, out));
            line(out, "end".into());
        }
        Statement::If(name, body) => {
            line(out, format!("if{name}then"));
            body.iter().for_each(|s| lines(s, depth + 1, out));
            line(out, "end".into());
        }
    }
}

fn strip(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

/// A script as the model might send it, ragged or on one line, reads as
/// the statements it holds: the options line first, as it was; then one
/// line a statement, each block two spaces further in, with nothing but
/// whitespace changed. Formatting it again changes nothing.
#[hegel::test(test_cases = 200)]
fn a_parsed_script_reads_one_statement_a_line(tc: TestCase) {
    let statements = tc.draw(block(2));
    let returned: Option<(&str, String)> = tc.draw(gs::optional(
        hegel::tuples!(gs::sampled_from(NAMES.to_vec()), expression()),
    ));
    let options: Option<u32> = tc.draw(gs::optional(
        gs::integers::<u32>().min_value(1).max_value(60_000),
    ));

    let mut words = Vec::new();
    statements.iter().for_each(|s| tokens(s, &mut words));
    if let Some((key, value)) = &returned {
        // StyLua keeps a table the input broke over lines expanded, so the
        // table is one word: line breaks fall only between statements'
        // tokens.
        words.push("return".into());
        words.push(format!("{{ {key} = {value} }}"));
    }
    let mut code = String::new();
    let options_line =
        options.map(|ms| format!("-- @options: {{\"timeout_ms\": {ms}}}"));
    if let Some(line) = &options_line {
        code.push_str(line);
        code.push('\n');
    }
    for (index, word) in words.iter().enumerate() {
        if index > 0 {
            code.push_str(
                tc.draw(gs::sampled_from(vec![" ", "  ", "\n", " \n "])),
            );
        }
        code.push_str(word);
    }
    tc.note(&code);

    let mut expected = Vec::new();
    statements.iter().for_each(|s| lines(s, 0, &mut expected));
    if let Some((key, value)) = &returned {
        expected.push((0, format!("return{{{key}={}}}", strip(value))));
    }

    let shown = formatted(&code);
    let mut shown_lines: Vec<&str> = shown.lines().collect();
    if let Some(line) = &options_line {
        assert_eq!(shown_lines.first(), Some(&line.as_str()), "{shown}");
        shown_lines.remove(0);
    }
    let actual: Vec<(usize, String)> = shown_lines
        .iter()
        .map(|line| {
            let indent = line.len() - line.trim_start_matches(' ').len();
            assert_eq!(indent % 2, 0, "odd indent in:\n{shown}");
            assert_eq!(line.trim_end(), *line, "trailing space in:\n{shown}");
            (indent / 2, strip(line))
        })
        .collect();
    assert_eq!(actual, expected, "formatted:\n{shown}");
    assert_eq!(formatted(&shown), shown, "formatting is not idempotent");
}

/// A script still streaming in does not parse; it shows as it is.
#[test]
fn a_partial_script_is_left_alone() {
    let code = "local x = tools.read({ path =";
    assert_eq!(&*formatted(code), code);
}
