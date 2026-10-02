//! Inputs and golden answers. Expected answers are not computed by the
//! tool, parser, inference, or module implementation being evaluated.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// One file to create in the evaluation's temporary repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct File {
    pub path: String,
    pub text: String,
}

/// One independently checkable task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fixture {
    pub name: String,
    pub task: String,
    pub files: Vec<File>,
    pub expected: Value,
}

fn file(path: &str, text: &str) -> File {
    File {
        path: path.into(),
        text: text.into(),
    }
}

/// Paths and matched text that must not be reconstructed from display lines.
pub fn search() -> Fixture {
    search_with_path("src/a: one.rs")
}

/// A search case with a caller-chosen first path for path-format properties.
pub fn search_with_path(first_path: &str) -> Fixture {
    Fixture {
        name: "structured-search".into(),
        task: "Find the literal TODO: in every .rs file. Return an array of \
               {path, line, text} records, ordered by path then line."
            .into(),
        files: vec![
            file(first_path, "// TODO: repair retries\nlet n = 1;\n"),
            file("src/雪.rs", "let n = 2;\n// TODO: handle empty arrays\n"),
            file("src/clean.rs", "// T O D O is not a match\n"),
            file("notes.txt", "TODO: not a Rust file\n"),
        ],
        expected: json!([
            {"path": first_path, "line": 1, "text": "// TODO: repair retries"},
            {"path": "src/雪.rs", "line": 2, "text": "// TODO: handle empty arrays"}
        ]),
    }
}

/// Three failures separated by irrelevant lines. 2,400 noise lines put
/// both the head and middle failures outside a 2,000-line tail display.
/// Each noise line is long enough to exercise the byte cap too.
pub fn test_log(noise_lines: usize) -> Fixture {
    let mut text =
        String::from("FAIL\tdecode_empty\tempty array became object\n");
    for index in 0..noise_lines {
        if index == noise_lines / 2 {
            text.push_str("FAIL\tcancel_loop\tloop ignored cancellation\n");
        }
        text.push_str(
            "PASS\tboring_case\tno relevant failure in this successful case\n",
        );
    }
    // A zero-sized workload still has all three signals.
    if noise_lines == 0 {
        text.push_str("FAIL\tcancel_loop\tloop ignored cancellation\n");
    }
    text.push_str("FAIL\tcharge_once\tusage was charged twice\n");
    Fixture {
        name: "complete-test-log".into(),
        task: "Read tests.log and return every FAIL record as an array of \
               {test, message}, in log order. PASS records are irrelevant."
            .into(),
        files: vec![file("tests.log", &text)],
        expected: json!([
            {"test": "decode_empty", "message": "empty array became object"},
            {"test": "cancel_loop", "message": "loop ignored cancellation"},
            {"test": "charge_once", "message": "usage was charged twice"}
        ]),
    }
}

/// Explicit semantic evidence includes a non-breaking change as a distractor.
pub fn semantic_changes() -> Fixture {
    Fixture {
        name: "compatibility-extraction".into(),
        task: "Return the changes that require existing clients to change \
               their code, as {component, before, after}. Preserve document order.".into(),
        files: vec![file("changes.md", "# Changes\n\nThe connection option `retry_count` was removed. Clients must use `max_attempts` instead.\n\nThe default theme is now blue. Existing configuration keys are still accepted.\n\nThe lookup function now returns null when no item exists. It previously raised NotFound.\n")],
        expected: json!([
            {"component": "connection option", "before": "retry_count", "after": "max_attempts"},
            {"component": "lookup function", "before": "raises NotFound", "after": "returns null"}
        ]),
    }
}

/// Related tasks have different commands and different keys. A saved
/// program must accept inputs rather than capture the first task's answer.
pub fn repeated_workflow(changed: bool) -> Fixture {
    let (name, config, expected) = if changed {
        (
            "workflow-changed",
            r#"{"verify_command":"printf 'checked-v2\\n'"}"#,
            json!({"command": "printf 'checked-v2\\n'"}),
        )
    } else {
        (
            "workflow-original",
            r#"{"test_command":"printf 'checked-v1\\n'"}"#,
            json!({"command": "printf 'checked-v1\\n'"}),
        )
    };
    Fixture {
        name: name.into(),
        task: "Read commands.json. Return the command associated with \
               verify_command when present, otherwise test_command, as {command}.".into(),
        files: vec![file("commands.json", config)],
        expected,
    }
}

/// The initial deterministic corpus. Live runners can reuse these inputs.
pub fn corpus() -> Vec<Fixture> {
    vec![
        search(),
        test_log(2_400),
        semantic_changes(),
        repeated_workflow(false),
        repeated_workflow(true),
    ]
}

/// Original and changed inputs used by the Agent-owned matrix.
pub fn matrix_corpus() -> Vec<Fixture> {
    let mut changed_search = search_with_path("src/Å: next step.rs");
    changed_search.name = "structured-search-changed".into();
    changed_search.files[0].text =
        "// TODO: repair the changed path\nlet n = 3;\n".into();
    changed_search.expected[0]["text"] =
        json!("// TODO: repair the changed path");

    let changed_log = changed_log_with_page_marker(2_400, "雪");

    let mut changed_semantic = semantic_changes();
    changed_semantic.name = "compatibility-extraction-changed".into();
    changed_semantic.files[0].text = changed_semantic.files[0]
        .text
        .replace("retry_count", "retry_limit")
        .replace("max_attempts", "retry_ceiling")
        .replace("blue", "green");
    changed_semantic.expected[0]["before"] = json!("retry_limit");
    changed_semantic.expected[0]["after"] = json!("retry_ceiling");

    vec![
        search(),
        changed_search,
        test_log(2_400),
        changed_log,
        semantic_changes(),
        changed_semantic,
        repeated_workflow(false),
        repeated_workflow(true),
    ]
}

/// A changed log with a valid Unicode marker crossing the first artifact page.
pub fn changed_log_with_page_marker(
    noise_lines: usize,
    marker: &str,
) -> Fixture {
    let mut changed_log = test_log(noise_lines);
    changed_log.name = "complete-test-log-changed".into();
    changed_log.files[0].text = changed_log.files[0]
        .text
        .replace("decode_empty", "decode_雪")
        .replace("cancel_loop", "cancel_🚀")
        .replace("charge_once", "charge_final");
    // Put a multibyte marker across the first 8,192-byte page boundary.
    // The UTF-8 range reader must end before it and advance on the next page.
    let first_line_end = changed_log.files[0].text.find('\n').unwrap() + 1;
    let page_prefix = "PASS\tpage_boundary\t";
    let padding = 8_191 - first_line_end - page_prefix.len();
    let marker_line =
        format!("{page_prefix}{}{marker} marker\n", "x".repeat(padding));
    changed_log.files[0]
        .text
        .insert_str(first_line_end, &marker_line);
    changed_log.expected[0]["test"] = json!("decode_雪");
    changed_log.expected[1]["test"] = json!("cancel_🚀");
    changed_log.expected[2]["test"] = json!("charge_final");

    changed_log
}
