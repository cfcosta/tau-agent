//! Syntax highlighting: every grammar loads, parts land where the code
//! is, and the per-line and run forms agree with the whole.

use hegel::generators as gs;
use tau_ui_kit::{
    syntax::{Kind, Lang, highlight, highlight_lines, runs},
    theme::Theme,
};

/// Code that each language's grammar should color, and a part of it.
const SAMPLES: [(Lang, &str, &str, Kind); 10] = [
    (Lang::Bash, "echo \"hi\" # note", "# note", Kind::Comment),
    (Lang::Go, "func main() { return }", "func", Kind::Keyword),
    (Lang::JavaScript, "const x = 'hi';", "'hi'", Kind::String),
    (Lang::Json, "{\"a\": 12}", "12", Kind::Number),
    (Lang::Nix, "{ a = \"hi\"; }", "\"hi\"", Kind::String),
    (Lang::Python, "def f():\n    return 1", "def", Kind::Keyword),
    (Lang::Rust, "fn main() { let x = 1; }", "fn", Kind::Keyword),
    (Lang::Toml, "[package]\nname = \"tau\"", "\"tau\"", Kind::String),
    (Lang::Tsx, "const a = <div>{x}</div>;", "const", Kind::Keyword),
    (Lang::TypeScript, "let n: number = 1;", "let", Kind::Keyword),
];

/// Each grammar's query loads and colors its sample.
#[test]
fn every_language_colors_its_sample() {
    for (lang, code, part, kind) in SAMPLES {
        let at = code.find(part).unwrap();
        let spans = highlight(lang, code);
        assert!(
            spans
                .iter()
                .any(|(range, k)| *k == kind && range.contains(&at)),
            "{lang:?}: {part:?} is not {kind:?} in {spans:?}"
        );
    }
}

#[test]
fn languages_come_from_paths_and_tags() {
    assert_eq!(Lang::of_path("src/main.rs"), Some(Lang::Rust));
    assert_eq!(Lang::of_path("a/b/navigation.ts"), Some(Lang::TypeScript));
    assert_eq!(Lang::of_path("Cargo.lock"), Some(Lang::Toml));
    assert_eq!(Lang::of_path("README"), None);
    assert_eq!(Lang::of_path("notes.md"), None);
    assert_eq!(Lang::of_name("Python"), Some(Lang::Python));
}

fn code() -> impl hegel::PrintableGenerator<String> {
    gs::text()
        .alphabet("fnletdefimport{}()[]\"'#/*\\ \n=;:.,<>é1x")
        .max_size(80)
}

/// Whatever the text, parts are in order, apart, inside it and on
/// character boundaries; runs cover it exactly.
#[hegel::test(test_cases = 300)]
fn parts_are_ordered_and_inside(tc: hegel::TestCase) {
    let lang = Lang::ALL[tc.draw(gs::integers::<usize>().max_value(Lang::ALL.len() - 1))];
    let text = tc.draw(code());
    let spans = highlight(lang, &text);
    let mut at = 0;
    for (range, _) in &spans {
        assert!(range.start >= at && range.start < range.end, "{spans:?}");
        assert!(range.end <= text.len());
        assert!(text.is_char_boundary(range.start));
        assert!(text.is_char_boundary(range.end));
        at = range.end;
    }
    let t = Theme::graphite();
    let runs = runs(&text, &spans, t.text_soft, &t.syntax);
    assert_eq!(runs.iter().map(|run| run.len).sum::<usize>(), text.len());
}

/// Line by line, the parts are the whole text's, cut at line ends.
#[hegel::test(test_cases = 300)]
fn lines_cut_the_whole(tc: hegel::TestCase) {
    let lang = Lang::ALL[tc.draw(gs::integers::<usize>().max_value(Lang::ALL.len() - 1))];
    let text = tc.draw(code());
    let lines: Vec<&str> = text.split('\n').collect();
    let per_line = highlight_lines(lang, &lines);
    assert_eq!(per_line.len(), lines.len());
    let mut kinds_by_line = vec![None; text.len()];
    let mut start = 0;
    for (line, parts) in lines.iter().zip(&per_line) {
        for (range, kind) in parts {
            assert!(range.end <= line.len());
            for i in range.clone() {
                kinds_by_line[start + i] = Some(*kind);
            }
        }
        start += line.len() + 1;
    }
    let mut kinds = vec![None; text.len()];
    for (range, kind) in highlight(lang, &text) {
        for i in range {
            // The newline itself belongs to no line.
            if text.as_bytes()[i] != b'\n' {
                kinds[i] = Some(kind);
            }
        }
    }
    assert_eq!(kinds_by_line, kinds);
}
