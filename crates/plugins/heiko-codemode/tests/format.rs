//! Scripts as the cards show them: formatted when they parse, as they
//! came when they do not.
//!
//! | Property | Oracle |
//! | --- | --- |
//! | a parsed script reads one statement a line, blocks indented by two, the options line on top, its tokens unchanged | layout model of the generated statements |
//! | formatting a formatted script changes nothing | idempotence |
//! | any script keeps its tokens (a table's trailing comma aside) and parses | the generated tokens, read back with tree-sitter |
//! | every shipped script keeps its tokens and text | tree-sitter's tokens of the file |
//!
//! The first generator builds valid Luau from a small statement grammar
//! (locals, `+=`, `for`, `if`, a final `return`) and lays its tokens out
//! with arbitrary spaces and line breaks, so a one-line script and a
//! ragged one are both drawn. The second draws wider Luau as tokens:
//! operators, unary minus, long strings, calls, tables, functions,
//! `elseif`, and comments. Shrinking removes statements and nesting.

use hegel::{TestCase, generators as gs};
use heiko_codemode::format::formatted;

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
        // The table is one word, so line breaks fall only between
        // statements' tokens and the layout model stays one line each.
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

/// The tokens of `code`, comments included, as tree-sitter reads them,
/// or `None` when it does not parse.
fn tokens_of(code: &str) -> Option<Vec<String>> {
    fn walk(node: tree_sitter::Node, code: &str, out: &mut Vec<String>) {
        if node.child_count() == 0
            || matches!(node.kind(), "string" | "comment")
        {
            out.push(code[node.byte_range()].trim_end().to_owned());
            return;
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            walk(child, code, out);
        }
    }
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_luau::LANGUAGE.into())
        .expect("the Luau grammar loads");
    let tree = parser.parse(code, None)?;
    if tree.root_node().has_error() {
        return None;
    }
    let mut out = Vec::new();
    walk(tree.root_node(), code, &mut out);
    Some(out)
}

/// `tokens` without a table's trailing separator, which the layout
/// writes only when the table breaks.
fn untrailed(tokens: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(tokens.len());
    for token in tokens {
        if token == "}" {
            // The comma may sit before the comments that end the table.
            let comments =
                out.iter().rev().take_while(|t| t.starts_with("--")).count();
            let at = out.len() - comments;
            if at > 0 && (out[at - 1] == "," || out[at - 1] == ";") {
                out.remove(at - 1);
            }
        }
        out.push(token);
    }
    out
}

/// Every Luau file heiko ships, laid out, keeps its tokens, a table's
/// trailing comma aside, and its text but for whitespace; laying it out
/// again changes nothing.
#[test]
fn every_shipped_script_keeps_its_tokens() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../plugins");
    let mut files = Vec::new();
    let mut dirs = vec![root];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).expect("plugins is readable") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "luau") {
                files.push(path);
            }
        }
    }
    assert!(files.len() > 100, "found {} scripts", files.len());
    let mut formatted_any = 0;
    for path in files {
        let code = std::fs::read_to_string(&path).expect("a script");
        let Some(before) = tokens_of(&code) else {
            continue;
        };
        let before_text = before.concat();
        let shown = formatted(&code);
        let name = path.display();
        let after = tokens_of(&shown)
            .unwrap_or_else(|| panic!("{name}: does not parse:\n{shown}"));
        assert_eq!(
            untrailed(after.clone()),
            untrailed(before),
            "{name}: tokens changed:\n{shown}"
        );
        // The tokens hold all the text, in and out, so none was lost.
        assert_eq!(strip(&before_text), strip(&code), "{name}: untokenized");
        assert_eq!(
            strip(&after.concat()),
            strip(&shown),
            "{name}: text outside the tokens:\n{shown}"
        );
        assert_eq!(formatted(&shown), shown, "{name}: not idempotent");
        for line in shown.lines() {
            assert_eq!(line.trim_end(), line, "{name}: trailing space");
        }
        formatted_any += 1;
    }
    assert!(formatted_any > 100, "only {formatted_any} scripts parsed");
}

/// Tokens of a generated Luau expression, at most `depth` deep.
#[hegel::composite]
fn luau_expression(tc: &TestCase, depth: u8) -> Vec<String> {
    let kinds = if depth == 0 { 3 } else { 9 };
    let words = |items: &[&str]| -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    };
    match tc.draw(gs::integers::<u8>().max_value(kinds)) {
        0 => vec![tc.draw(gs::integers::<u16>().max_value(999)).to_string()],
        1 => words(&[tc.draw(gs::sampled_from(NAMES.to_vec()))]),
        2 => words(&[tc.draw(gs::sampled_from(vec![
            "\"a-b\"",
            "'--'",
            "[[x]]",
            "[=[ ]] ]=]",
            "`n {n}`",
        ]))]),
        3 => words(&[tc.draw(gs::sampled_from(vec!["nil", "true", "..."]))]),
        4 => {
            let op = tc.draw(gs::sampled_from(vec!["-", "not", "#"]));
            let mut out = words(&[op]);
            out.extend(tc.draw(luau_expression(depth - 1)));
            out
        }
        5 => {
            let mut out = tc.draw(luau_expression(depth - 1));
            out.push(
                tc.draw(gs::sampled_from(vec![
                    "+", "-", "*", "..", "and", "or", "==", "<", "//",
                ]))
                .to_owned(),
            );
            out.extend(tc.draw(luau_expression(depth - 1)));
            out
        }
        6 => {
            // A call: `f(...)`, `t.f(...)` or `t:f(...)`.
            let mut out = words(&[tc.draw(gs::sampled_from(NAMES.to_vec()))]);
            match tc.draw(gs::integers::<u8>().max_value(2)) {
                0 => {}
                1 => out.extend(words(&[".", "read"])),
                _ => out.extend(words(&[":", "grep"])),
            }
            out.push("(".into());
            let count = tc.draw(gs::integers::<usize>().max_value(3));
            for index in 0..count {
                if index > 0 {
                    out.push(",".into());
                }
                out.extend(tc.draw(luau_expression(depth - 1)));
            }
            out.push(")".into());
            out
        }
        7 => {
            // A table, its fields positional, named or keyed, with or
            // without a trailing separator.
            let mut out = words(&["{"]);
            let count = tc.draw(gs::integers::<usize>().max_value(4));
            for index in 0..count {
                if index > 0 {
                    out.push(tc.draw(gs::sampled_from(vec![",", ";"])).into());
                }
                match tc.draw(gs::integers::<u8>().max_value(2)) {
                    0 => {}
                    1 => out.extend(words(&[
                        tc.draw(gs::sampled_from(NAMES.to_vec())),
                        "=",
                    ])),
                    _ => {
                        out.push("[".into());
                        out.extend(tc.draw(luau_expression(depth - 1)));
                        out.extend(words(&["]", "="]));
                    }
                }
                out.extend(tc.draw(luau_expression(depth - 1)));
                if tc.draw(gs::booleans()) {
                    out.push("-- a field".into());
                }
            }
            if count > 0 && tc.draw(gs::booleans()) {
                out.push(",".into());
            }
            out.push("}".into());
            out
        }
        8 => {
            // An index: `t[e]`.
            let mut out =
                words(&[tc.draw(gs::sampled_from(NAMES.to_vec())), "["]);
            out.extend(tc.draw(luau_expression(depth - 1)));
            out.push("]".into());
            out
        }
        _ => {
            let mut out = words(&["function", "(", "n", ",", "...", ")"]);
            out.extend(tc.draw(luau_block(depth - 1)));
            out.push("end".into());
            out
        }
    }
}

/// Tokens of one to three generated statements, at most `depth` deep;
/// a comment token is followed by a line break when laid out.
#[hegel::composite]
fn luau_block(tc: &TestCase, depth: u8) -> Vec<String> {
    let mut out = Vec::new();
    let count = tc.draw(gs::integers::<usize>().min_value(1).max_value(3));
    for _ in 0..count {
        let name = tc.draw(gs::sampled_from(NAMES.to_vec())).to_owned();
        let kinds = if depth == 0 { 3 } else { 5 };
        match tc.draw(gs::integers::<u8>().max_value(kinds)) {
            0 => {
                out.extend(["local".to_owned(), name, "=".into()]);
                out.extend(tc.draw(luau_expression(depth)));
            }
            1 => {
                out.extend([name, "=".into()]);
                out.extend(tc.draw(luau_expression(depth)));
            }
            2 => out.extend([name, "(".into(), ")".into()]),
            3 => out.push(
                tc.draw(gs::sampled_from(vec![
                    "-- a note",
                    "--[[ a - note ]]",
                ]))
                .into(),
            ),
            4 => {
                let arms = tc.draw(gs::integers::<usize>().max_value(2));
                out.push("if".into());
                out.extend(tc.draw(luau_expression(depth - 1)));
                out.push("then".into());
                out.extend(tc.draw(luau_block(depth - 1)));
                for _ in 0..arms {
                    out.push("elseif".into());
                    out.extend(tc.draw(luau_expression(depth - 1)));
                    out.push("then".into());
                    out.extend(tc.draw(luau_block(depth - 1)));
                }
                if tc.draw(gs::booleans()) {
                    out.push("else".into());
                    out.extend(tc.draw(luau_block(depth - 1)));
                }
                out.push("end".into());
            }
            _ => {
                out.push("while".into());
                out.extend(tc.draw(luau_expression(depth - 1)));
                out.push("do".into());
                out.extend(tc.draw(luau_block(depth - 1)));
                out.push("end".into());
            }
        }
    }
    out
}

/// Any script, laid out however, reads as the same tokens after
/// formatting (a table's trailing separator aside), holds no text
/// outside them, and parses; formatting it again changes nothing.
#[hegel::test(test_cases = 300)]
fn a_formatted_script_keeps_its_tokens(tc: TestCase) {
    let words = tc.draw(luau_block(3));
    let mut code = String::new();
    for (index, word) in words.iter().enumerate() {
        if index > 0 {
            let after_comment = words[index - 1].starts_with("-- ");
            code.push_str(if after_comment {
                "\n"
            } else {
                tc.draw(gs::sampled_from(vec![" ", "  ", "\n", " \n "]))
            });
        }
        code.push_str(word);
    }
    tc.note(&code);
    assert_eq!(tokens_of(&code).as_ref(), Some(&words), "the script parses");

    let shown = formatted(&code);
    let after = tokens_of(&shown)
        .unwrap_or_else(|| panic!("the layout does not parse:\n{shown}"));
    assert_eq!(untrailed(after.clone()), untrailed(words), "{shown}");
    assert_eq!(strip(&after.concat()), strip(&shown), "{shown}");
    for line in shown.lines() {
        assert_eq!(line.trim_end(), line, "trailing space in:\n{shown}");
    }
    assert_eq!(formatted(&shown), shown, "not idempotent:\n{shown}");
}

/// `code`, formatted, reads as `shown`.
#[track_caller]
fn shows(code: &str, shown: &str) {
    assert_eq!(&*formatted(code), shown);
}

/// A minus before a minus keeps its space: `--` would be a comment.
#[test]
fn a_minus_before_a_minus_keeps_its_space() {
    shows("local x = - -y", "local x = - -y");
}

/// A long string indexing a table keeps its space after `[`: `[[[`
/// would open the string a bracket early.
#[test]
fn a_long_string_index_keeps_its_space() {
    shows("local s = t[ [[a]] ]", "local s = t[ [[a]]]");
    shows("local s = t[ [=[a]=] ]", "local s = t[ [=[a]=]]");
}

/// A comment on the line a block opens stays there.
#[test]
fn a_comment_after_then_stays_on_its_line() {
    shows("if a then -- why\nb() end", "if a then -- why\n  b()\nend");
}

/// A line comment in a table breaks it, one field a line, and the table
/// ends in a comma.
#[test]
fn a_line_comment_in_a_table_breaks_it() {
    shows(
        "local t = { a = 1, -- one\n b = 2 }",
        "local t = {\n  a = 1, -- one\n  b = 2,\n}",
    );
}

/// A line comment on a table's last field keeps the table broken, so
/// it does not hide the closing brace.
#[test]
fn a_line_comment_on_the_last_field_keeps_the_brace() {
    shows(
        "local t = { a = 1 -- last\n}",
        "local t = {\n  a = 1, -- last\n}",
    );
}

/// A trailing comma after a table's closing comment moves up to its
/// field, so neither the comment hides the brace nor the comma takes a
/// line.
#[test]
fn a_comma_after_the_closing_comment_moves_up() {
    shows("n = { 0 -- a field\n, }", "n = {\n  0, -- a field\n}");
}

/// A table on one line has no trailing comma, whatever it was written
/// with.
#[test]
fn a_table_on_one_line_drops_its_trailing_comma() {
    shows("local u = { 1, 2, 3, }", "local u = { 1, 2, 3 }");
}

/// An `elseif` and an `else` start their own lines, with no blank line
/// before them.
#[test]
fn an_elseif_chain_reads_one_arm_a_line() {
    shows(
        "if a then b() elseif c then d() else e() end",
        "if a then\n  b()\nelseif c then\n  d()\nelse\n  e()\nend",
    );
}

/// A concatenation too long for a line breaks before each `..`.
#[test]
fn a_long_concatenation_breaks_before_its_operators() {
    shows(
        &format!(
            "local s = {a} .. {b} .. {a}",
            a = "a".repeat(30),
            b = "b".repeat(30)
        ),
        &format!(
            "local s = {a}\n  .. {b}\n  .. {a}",
            a = "a".repeat(30),
            b = "b".repeat(30)
        ),
    );
}

/// A call whose last argument is a table hugs it when the call's first
/// line fits, and puts one argument a line when it does not.
#[test]
fn a_call_hugs_its_last_table_when_its_first_line_fits() {
    shows(
        "tools.write({ path = \"src/main.rs\", content = \"fn main() {}\", mode = \"create\", force = true })",
        "tools.write({\n  path = \"src/main.rs\",\n  content = \"fn main() {}\",\n  mode = \"create\",\n  force = true,\n})",
    );
    let long = "x".repeat(77);
    shows(
        &format!("f({long}, {{ a = 1 }})"),
        &format!("f(\n  {long},\n  {{ a = 1 }}\n)"),
    );
}
