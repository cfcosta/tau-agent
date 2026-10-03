//! Syntax highlighting with tree-sitter: what kind of code each part of
//! a text is, for cards and replies to color it by the theme's
//! [`SyntaxLook`](crate::theme::SyntaxLook).
//!
//! A language comes from a file's extension or a fenced block's tag.
//! Text in no language this knows, or too long to be worth it, gets no
//! spans and stays plain.

use std::{cell::RefCell, collections::HashMap, ops::Range, sync::LazyLock};

use gpui::{Hsla, TextRun, font};
use tree_sitter_highlight::{
    HighlightConfiguration,
    HighlightEvent,
    Highlighter,
};

use crate::theme::{MONO, SyntaxLook};

/// The languages it highlights.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    Bash,
    Go,
    JavaScript,
    Json,
    Nix,
    Python,
    Rust,
    Toml,
    Tsx,
    TypeScript,
}

impl Lang {
    /// Every language, for tests and the configurations.
    pub const ALL: [Lang; 10] = [
        Lang::Bash,
        Lang::Go,
        Lang::JavaScript,
        Lang::Json,
        Lang::Nix,
        Lang::Python,
        Lang::Rust,
        Lang::Toml,
        Lang::Tsx,
        Lang::TypeScript,
    ];

    /// The language of a file, by its extension or its name.
    pub fn of_path(path: &str) -> Option<Self> {
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
        match name {
            "flake.lock" => return Some(Lang::Json),
            "Cargo.lock" => return Some(Lang::Toml),
            _ => {}
        }
        let (_, ext) = name.rsplit_once('.')?;
        Self::of_name(ext)
    }

    /// The language a fenced block's tag or an extension names.
    pub fn of_name(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "sh" | "bash" | "zsh" | "shell" | "console" => Lang::Bash,
            "go" => Lang::Go,
            "js" | "jsx" | "mjs" | "cjs" | "javascript" => Lang::JavaScript,
            "json" | "jsonc" | "json5" => Lang::Json,
            "nix" => Lang::Nix,
            "py" | "pyi" | "python" => Lang::Python,
            "rs" | "rust" => Lang::Rust,
            "toml" => Lang::Toml,
            "tsx" => Lang::Tsx,
            "ts" | "mts" | "cts" | "typescript" => Lang::TypeScript,
            _ => return None,
        })
    }

    fn configuration(self) -> HighlightConfiguration {
        use tree_sitter_javascript as js;
        use tree_sitter_typescript as ts;
        let (language, highlights) = match self {
            Lang::Bash => (
                tree_sitter_bash::LANGUAGE,
                tree_sitter_bash::HIGHLIGHT_QUERY.to_owned(),
            ),
            Lang::Go => {
                (tree_sitter_go::LANGUAGE, tree_sitter_go::HIGHLIGHTS_QUERY.into())
            }
            Lang::JavaScript => (
                js::LANGUAGE,
                format!("{}\n{}", js::JSX_HIGHLIGHT_QUERY, js::HIGHLIGHT_QUERY),
            ),
            Lang::Json => (
                tree_sitter_json::LANGUAGE,
                tree_sitter_json::HIGHLIGHTS_QUERY.into(),
            ),
            Lang::Nix => (
                tree_sitter_nix::LANGUAGE,
                tree_sitter_nix::HIGHLIGHTS_QUERY.into(),
            ),
            Lang::Python => (
                tree_sitter_python::LANGUAGE,
                tree_sitter_python::HIGHLIGHTS_QUERY.into(),
            ),
            Lang::Rust => (
                tree_sitter_rust::LANGUAGE,
                tree_sitter_rust::HIGHLIGHTS_QUERY.into(),
            ),
            Lang::Toml => (
                tree_sitter_toml_ng::LANGUAGE,
                tree_sitter_toml_ng::HIGHLIGHTS_QUERY.into(),
            ),
            // TypeScript's query only adds to JavaScript's; its own
            // patterns come first, so they win.
            Lang::Tsx => (
                ts::LANGUAGE_TSX,
                format!(
                    "{}\n{}\n{}",
                    ts::HIGHLIGHTS_QUERY,
                    js::JSX_HIGHLIGHT_QUERY,
                    js::HIGHLIGHT_QUERY
                ),
            ),
            Lang::TypeScript => (
                ts::LANGUAGE_TYPESCRIPT,
                format!("{}\n{}", ts::HIGHLIGHTS_QUERY, js::HIGHLIGHT_QUERY),
            ),
        };
        let mut config = HighlightConfiguration::new(
            language.into(),
            format!("{self:?}"),
            &highlights,
            "",
            "",
        )
        .unwrap_or_else(|error| panic!("{self:?}'s highlights query: {error}"));
        config.configure(&NAMES.map(|(name, _)| name));
        config
    }
}

/// What a part of the code is, as the theme colors it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Keyword,
    String,
    Escape,
    Comment,
    Function,
    Type,
    Number,
    Constant,
    Property,
    Attribute,
    Builtin,
    Operator,
    Punctuation,
    Tag,
}

/// The capture names the grammars' queries use, and the kind each is.
/// A capture takes the longest name here it starts with:
/// `function.method.call` is a [`Kind::Function`].
const NAMES: [(&str, Kind); 34] = [
    ("attribute", Kind::Attribute),
    ("boolean", Kind::Constant),
    ("character", Kind::String),
    ("comment", Kind::Comment),
    ("constant", Kind::Constant),
    ("constant.builtin", Kind::Constant),
    ("constructor", Kind::Type),
    ("embedded", Kind::Punctuation),
    ("escape", Kind::Escape),
    ("function", Kind::Function),
    ("function.builtin", Kind::Builtin),
    ("function.macro", Kind::Function),
    ("keyword", Kind::Keyword),
    ("label", Kind::Attribute),
    ("module", Kind::Type),
    ("number", Kind::Number),
    ("operator", Kind::Operator),
    ("property", Kind::Property),
    ("punctuation", Kind::Punctuation),
    ("punctuation.special", Kind::Operator),
    ("string", Kind::String),
    ("string.escape", Kind::Escape),
    ("string.special", Kind::String),
    ("string.special.key", Kind::Property),
    ("tag", Kind::Tag),
    ("tag.attribute", Kind::Attribute),
    ("type", Kind::Type),
    ("type.builtin", Kind::Type),
    ("variable.builtin", Kind::Builtin),
    ("variable.parameter", Kind::Property),
    ("variable.member", Kind::Property),
    ("float", Kind::Number),
    ("include", Kind::Keyword),
    ("repeat", Kind::Keyword),
];

/// Texts longer than this stay plain: highlighting them would hold up
/// a frame.
pub const MAX_BYTES: usize = 512 * 1024;

static CONFIGS: LazyLock<HashMap<Lang, HighlightConfiguration>> =
    LazyLock::new(|| {
        Lang::ALL
            .into_iter()
            .map(|lang| (lang, lang.configuration()))
            .collect()
    });

thread_local! {
    static HIGHLIGHTER: RefCell<Highlighter> = RefCell::new(Highlighter::new());
}

/// The highlighted parts of `text`, in order and apart; the innermost
/// capture names a nested part. Text between them is plain.
pub fn highlight(lang: Lang, text: &str) -> Vec<(Range<usize>, Kind)> {
    if text.len() > MAX_BYTES {
        return Vec::new();
    }
    let config = &CONFIGS[&lang];
    HIGHLIGHTER.with_borrow_mut(|highlighter| {
        let Ok(events) =
            highlighter.highlight(config, text.as_bytes(), None, None, |_| None)
        else {
            return Vec::new();
        };
        let mut spans: Vec<(Range<usize>, Kind)> = Vec::new();
        let mut stack: Vec<Kind> = Vec::new();
        for event in events {
            match event {
                Ok(HighlightEvent::HighlightStart(h)) => stack.push(NAMES[h.0].1),
                Ok(HighlightEvent::HighlightEnd) => {
                    stack.pop();
                }
                Ok(HighlightEvent::Source { start, end }) => {
                    let Some(&kind) = stack.last() else { continue };
                    match spans.last_mut() {
                        Some((range, last))
                            if *last == kind && range.end == start =>
                        {
                            range.end = end;
                        }
                        _ => spans.push((start..end, kind)),
                    }
                }
                // A parse that fails leaves the text plain.
                Err(_) => return Vec::new(),
            }
        }
        spans
    })
}

/// `text` highlighted line by line: each line's parts, by byte offsets
/// in that line. The lines are parsed together, so a string or comment
/// across lines is colored on each.
pub fn highlight_lines(
    lang: Lang,
    lines: &[impl AsRef<str>],
) -> Vec<Vec<(Range<usize>, Kind)>> {
    let text = lines
        .iter()
        .map(AsRef::as_ref)
        .collect::<Vec<_>>()
        .join("\n");
    let spans = highlight(lang, &text);
    let mut out = Vec::with_capacity(lines.len());
    let mut spans = spans.into_iter().peekable();
    let mut start = 0;
    for line in lines {
        let end = start + line.as_ref().len();
        let mut parts = Vec::new();
        while let Some((range, kind)) = spans.peek().cloned() {
            if range.start >= end {
                break;
            }
            let (from, to) = (range.start.max(start), range.end.min(end));
            if from < to {
                parts.push((from - start..to - start, kind));
            }
            if range.end <= end + 1 {
                spans.next();
            } else {
                break;
            }
        }
        out.push(parts);
        start = end + 1;
    }
    out
}

/// Runs over all of `text` in the monospace face: the highlighted parts
/// in their kind's color, the rest in `plain`.
pub fn runs(
    text: &str,
    spans: &[(Range<usize>, Kind)],
    plain: Hsla,
    look: &SyntaxLook,
) -> Vec<TextRun> {
    let run = |len: usize, kind: Option<Kind>| {
        let mut face = font(MONO);
        if kind == Some(Kind::Comment) {
            face = face.italic();
        }
        TextRun {
            len,
            font: face,
            color: kind.map_or(plain, |kind| look.color(kind)),
            background_color: None,
            underline: None,
            strikethrough: None,
        }
    };
    let mut out = Vec::new();
    let mut at = 0;
    for (range, kind) in spans {
        let range = range.start.max(at).min(text.len())..range.end.min(text.len());
        if range.start >= range.end {
            continue;
        }
        if range.start > at {
            out.push(run(range.start - at, None));
        }
        out.push(run(range.len(), Some(*kind)));
        at = range.end;
    }
    if at < text.len() {
        out.push(run(text.len() - at, None));
    }
    out
}
