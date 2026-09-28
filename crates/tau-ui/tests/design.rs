//! The design language lives in `src/theme.rs` (tokens) and
//! `src/ui/components.rs` (components). Everything else takes its look
//! from there, so a change to the look is a change to those two files.
//! This test keeps it that way: it fails when any other source file sets
//! a raw font size or weight, a color literal, or a numeric radius or
//! spacing.

use std::path::{Path, PathBuf};

/// The files allowed to hold design values.
const DESIGN: [&str; 2] = ["theme.rs", "ui/components.rs"];

/// What only the design files may write, and what to use instead.
const RULES: [(&str, &str); 5] = [
    ("text_size(", "a `Type` with `.typeset(...)`"),
    ("FontWeight::", "a `theme::weight` token"),
    ("rgb(0x", "a `Theme` color"),
    ("rgba(0x", "a `Theme` color"),
    ("hsla(", "a `Theme` color"),
];

/// Style methods that must take a token, not a number in `px(...)`.
const SPACED: [&str; 17] = [
    ".gap(",
    ".p(",
    ".px(",
    ".py(",
    ".pt(",
    ".pb(",
    ".pl(",
    ".pr(",
    ".m(",
    ".mt(",
    ".mb(",
    ".ml(",
    ".mr(",
    ".rounded(",
    ".rounded_t(",
    ".rounded_b(",
    ".rounded_l(",
];

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn only_the_design_files_hold_design_values() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&src, &mut files);
    let mut found = Vec::new();
    for file in files {
        let name = file
            .strip_prefix(&src)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if DESIGN.contains(&name.as_str()) {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap();
        for (n, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            for (pattern, instead) in RULES {
                if code.contains(pattern) {
                    found.push(format!(
                        "{name}:{}: `{pattern}`; use {instead}",
                        n + 1
                    ));
                }
            }
            for method in SPACED {
                let Some(at) = code.find(&format!("{method}px(")) else {
                    continue;
                };
                // A number, or a choice between numbers, is a raw value;
                // an expression (`-size / 2.`) is geometry.
                let rest = &code[at + method.len() + 3..];
                let number = rest.trim_start_matches('-');
                let raw = number.starts_with(|c: char| c.is_ascii_digit())
                    || rest.starts_with("if ");
                if raw {
                    found.push(format!(
                        "{name}:{}: `{method}px(...)`; use `sp(...)` or a `radius` token",
                        n + 1
                    ));
                }
            }
        }
    }
    assert!(
        found.is_empty(),
        "design values outside theme.rs and ui/components.rs:\n{}",
        found.join("\n")
    );
}
