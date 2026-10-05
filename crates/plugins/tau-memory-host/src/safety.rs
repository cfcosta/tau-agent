//! What never reaches a note: secrets, which are redacted, and text that
//! reads as instructions to the model, which is refused. Memory is data;
//! it is fenced as untrusted when read back, and checked here when
//! written (`docs/research/memory.md`, "Memory is data, not
//! instructions").

use std::sync::LazyLock;

use regex::Regex;

/// What a redacted secret becomes.
pub const REDACTED: &str = "[redacted]";

/// Credentials that have a recognizable shape.
static SECRETS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        // Private keys, whole blocks.
        r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----",
        // OpenAI and Anthropic style keys.
        r"\bsk-(?:ant-|proj-)?[A-Za-z0-9_\-]{20,}",
        // GitHub tokens.
        r"\bgh[pousr]_[A-Za-z0-9]{30,}",
        r"\bgithub_pat_[A-Za-z0-9_]{30,}",
        // AWS access keys.
        r"\b(?:AKIA|ASIA)[A-Z0-9]{16}\b",
        // Slack tokens.
        r"\bxox[abprs]-[A-Za-z0-9\-]{10,}",
        // Google API keys.
        r"\bAIza[A-Za-z0-9_\-]{35}",
        // Bearer tokens in headers.
        r"(?i)\bbearer\s+[A-Za-z0-9_\-\.=]{20,}",
        // `password = …`, `api_key: …` and the like, the value only.
        r#"(?i)\b(?:password|passwd|secret|token|api[_-]?key)\b\s*[:=]\s*['"]?[^\s'"]{8,}"#,
    ]
    .into_iter()
    .map(|pattern| Regex::new(pattern).expect("a valid pattern"))
    .collect()
});

/// `text` with every recognizable secret replaced by [`REDACTED`], and
/// how many were.
pub fn redact(text: &str) -> (String, usize) {
    let mut out = text.to_owned();
    let mut count = 0;
    for pattern in SECRETS.iter() {
        let found = pattern.find_iter(&out).count();
        if found > 0 {
            count += found;
            out = pattern.replace_all(&out, REDACTED).into_owned();
        }
    }
    (out, count)
}

/// Phrases that address the model rather than record anything.
static INSTRUCTIONS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)\bignore\s+(?:all\s+|any\s+)?(?:the\s+)?(?:previous|prior|above|earlier)\s+(?:instructions|messages|rules)",
        r"(?i)\bdisregard\s+(?:all\s+|any\s+)?(?:the\s+)?(?:previous|prior|above|system)\b",
        r"(?i)\byou\s+are\s+now\s+(?:a|an|in)\b",
        r"(?i)\bnew\s+system\s+prompt\b",
        r"(?i)<\|?\s*(?:im_start|im_end|system)\s*\|?>",
        r"(?i)\[\s*system\s*\]\s*:",
    ]
    .into_iter()
    .map(|pattern| Regex::new(pattern).expect("a valid pattern"))
    .collect()
});

/// Why `text` cannot be stored, when it reads as instructions to the
/// model or hides characters a reader would not see.
pub fn refusal(text: &str) -> Option<String> {
    if let Some(hidden) = text.chars().find(|c| is_hidden(*c)) {
        return Some(format!(
            "it holds an invisible character (U+{:04X}); write plain text",
            u32::from(hidden)
        ));
    }
    INSTRUCTIONS.iter().find_map(|pattern| {
        pattern.find(text).map(|found| {
            format!(
                "\"{}\" reads as an instruction to the model; memory records \
                 what is, not what to do",
                found.as_str()
            )
        })
    })
}

/// Zero-width and direction-changing characters, which hide text.
fn is_hidden(c: char) -> bool {
    matches!(
        c,
        '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
    )
}
