//! Output too large for the model (`tau_agent::output`).
//!
//! | Property | Oracle |
//! | --- | --- |
//! | a cut keeps a prefix and a suffix within budget | algebraic |
//! | truncation keeps the whole text in a spill file | the file |
//! | a spill file is new, its owner's alone, and named by its prefix | the filesystem |

use std::os::unix::fs::PermissionsExt as _;

use hegel::{TestCase, generators as gs};
use tau_agent::output::{Cut, Spill, cut, estimate_tokens, truncated};

#[hegel::test]
fn truncation_keeps_a_prefix_and_a_suffix_within_budget(tc: TestCase) {
    let text: String = tc.draw(gs::text().max_size(400));
    let max: u64 = tc.draw(gs::integers::<u64>().max_value(120));
    match cut(&text, max) {
        Cut::Whole => assert!(estimate_tokens(&text) <= max),
        Cut::Cut {
            head,
            tail,
            original_tokens,
            removed_tokens,
        } => {
            assert!(estimate_tokens(&text) > max);
            assert_eq!(original_tokens, estimate_tokens(&text));
            assert!(text.starts_with(head));
            assert!(text.ends_with(tail));
            let head_chars = head.chars().count();
            let tail_chars = tail.chars().count();
            assert!((head_chars + tail_chars) as u64 <= max * 4);
            // Half the kept characters from each end.
            assert!(tail_chars - head_chars <= 1);
            assert!(head.len() + tail.len() <= text.len());
            let middle = &text[head.len()..text.len() - tail.len()];
            assert_eq!(removed_tokens, estimate_tokens(middle));
        }
    }
}

/// Within the budget the text is unchanged and nothing is written; past
/// it, the whole text is in one spill file and the cut text in the
/// warning.
#[hegel::test(test_cases = 50)]
fn truncation_spills_the_whole_text(tc: TestCase) {
    let dir = tempfile::tempdir().unwrap();
    let spill = Spill::new(dir.path(), "tau-test");
    let text: String = tc.draw(gs::text().max_size(120));
    let max: u64 = tc.draw(gs::integers::<u64>().min_value(1).max_value(25));
    let out = truncated(&text, max, |full| spill.write(full.as_bytes(), "txt"));
    let files: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    let Cut::Cut { head, tail, .. } = cut(&text, max) else {
        assert_eq!(out, text);
        assert!(files.is_empty());
        return;
    };
    assert!(out.starts_with("Warning: truncated output (original token count: "));
    assert_eq!(files.len(), 1);
    let path = files[0].as_ref().unwrap().path();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    assert!(out.contains(&format!("\n\n{head}…")));
    assert!(out.contains(&format!("…{tail}\n\n")));
    assert!(out.ends_with(&format!(
        "[Full output: {} (read it with offset/limit)]",
        path.display()
    )));
}

/// A spill file may land in a directory other users share: it is always
/// a new file, only its owner may read it, and its name says what wrote
/// it.
#[test]
fn spill_files_are_new_private_and_named() {
    let dir = tempfile::tempdir().unwrap();
    let spill = Spill::new(dir.path(), "tau-test");
    let paths: Vec<_> = (0..64)
        .map(|_| spill.write(b"secret", "log").unwrap())
        .collect();
    let mut names: Vec<String> = paths
        .iter()
        .map(|path| path.file_name().unwrap().to_str().unwrap().to_owned())
        .collect();
    for (path, name) in paths.iter().zip(&names) {
        let mode = std::fs::metadata(path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "{name}");
        let hex = name
            .strip_prefix("tau-test-")
            .and_then(|rest| rest.strip_suffix(".log"))
            .unwrap_or_else(|| panic!("{name}"));
        assert!(hex.len() >= 16 && hex.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(std::fs::read(path).unwrap(), b"secret");
    }
    names.sort();
    names.dedup();
    assert_eq!(names.len(), paths.len());
}
