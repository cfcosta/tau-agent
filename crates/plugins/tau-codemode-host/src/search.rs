//! `search_tools`: BM25 over the callable tools, as pi's tool search
//! ranks them.
//!
//! A tool's document is its name, its name with `_` as spaces, its
//! description, the property names and descriptions of its input
//! schema, and its namespace's name, description and instructions.
//! Tokens split on camelCase and on anything not a letter or digit, are
//! lowercased, lose a few stop words, and a plural `s`.
//!
//! One rule pi's ranker does not have: a tool whose name is the query
//! ranks first.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::host::{Namespace, ToolEntry};

const K1: f64 = 1.2;
const B: f64 = 0.75;

/// Results `search_tools` returns when the script gives no limit.
pub const DEFAULT_LIMIT: usize = 8;

const STOP_WORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "for", "from", "in", "is",
    "it", "of", "on", "or", "that", "the", "this", "to", "with",
];

/// How deep a schema is read for property names.
const MAX_SCHEMA_DEPTH: usize = 8;

/// `text` as search tokens.
pub fn tokenize(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let chars: Vec<char> = text.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if !c.is_alphanumeric() {
            flush(&mut word, &mut words);
            continue;
        }
        if let Some(&prev) = i.checked_sub(1).and_then(|p| chars.get(p)) {
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            let boundary = c.is_uppercase()
                && (prev.is_lowercase()
                    || prev.is_ascii_digit()
                    || (prev.is_uppercase() && next_lower));
            if boundary {
                flush(&mut word, &mut words);
            }
        }
        word.extend(c.to_lowercase());
    }
    flush(&mut word, &mut words);
    words
}

fn flush(word: &mut String, words: &mut Vec<String>) {
    if word.is_empty() {
        return;
    }
    let taken = std::mem::take(word);
    if !STOP_WORDS.contains(&taken.as_str()) {
        words.push(stem(&taken));
    }
}

/// A naive singular: `queries` → `query`, `boxes` → `box`, `files` →
/// `file`.
pub fn stem(word: &str) -> String {
    let n = word.chars().count();
    if n > 4 && word.ends_with("ies") {
        return format!("{}y", &word[..word.len() - 3]);
    }
    if n > 4 && word.ends_with("es") {
        let base = &word[..word.len() - 2];
        if ["s", "x", "z", "ch", "sh"]
            .iter()
            .any(|end| base.ends_with(end))
        {
            return base.to_owned();
        }
    }
    if n > 3
        && word.ends_with('s')
        && !["ss", "us", "is"].iter().any(|end| word.ends_with(end))
    {
        return word[..word.len() - 1].to_owned();
    }
    word.to_owned()
}

/// Namespace names compare without case, `-` as `_`, and with or
/// without `mcp__`.
pub fn same_namespace(a: &str, b: &str) -> bool {
    fn norm(name: &str) -> String {
        let name = name.to_lowercase().replace('-', "_");
        name.strip_prefix("mcp__")
            .map(str::to_owned)
            .unwrap_or(name)
    }
    norm(a) == norm(b)
}

struct Doc {
    name: String,
    namespace: Option<String>,
    terms: HashMap<String, u32>,
    len: usize,
}

/// The callable tools, ready to search.
pub struct Index {
    docs: Vec<Doc>,
    frequency: HashMap<String, usize>,
    average: f64,
}

impl Index {
    pub fn new(tools: &[ToolEntry], namespaces: &[Namespace]) -> Self {
        let mut docs = Vec::with_capacity(tools.len());
        let mut frequency: HashMap<String, usize> = HashMap::new();
        for tool in tools {
            let mut text = vec![
                tool.name.clone(),
                tool.name.replace('_', " "),
                tool.description.clone(),
            ];
            schema_text(&tool.input_schema, 0, &mut text);
            if let Some(space) = &tool.namespace {
                text.push(space.clone());
                if let Some(ns) = namespaces.iter().find(|ns| &ns.name == space)
                {
                    text.extend(ns.description.clone());
                    text.extend(ns.instructions.clone());
                }
            }
            let tokens = tokenize(&text.join(" "));
            let mut terms: HashMap<String, u32> = HashMap::new();
            for token in &tokens {
                *terms.entry(token.clone()).or_default() += 1;
            }
            for term in terms.keys() {
                *frequency.entry(term.clone()).or_default() += 1;
            }
            docs.push(Doc {
                name: tool.name.clone(),
                namespace: tool.namespace.clone(),
                terms,
                len: tokens.len(),
            });
        }
        let average = if docs.is_empty() {
            1.0
        } else {
            (docs.iter().map(|d| d.len).sum::<usize>() as f64
                / docs.len() as f64)
                .max(1.0)
        };
        Self {
            docs,
            frequency,
            average,
        }
    }

    /// The indexes of the best `limit` tools for `query`, best first.
    /// Tools that match no term are left out.
    pub fn search(
        &self,
        query: &str,
        limit: usize,
        namespace: Option<&str>,
    ) -> Vec<usize> {
        let terms: HashSet<String> = tokenize(query).into_iter().collect();
        let exact = query.trim().to_lowercase();
        let n = self.docs.len() as f64;
        let mut scored: Vec<(usize, f64, bool)> = Vec::new();
        for (i, doc) in self.docs.iter().enumerate() {
            if let Some(wanted) = namespace
                && !doc
                    .namespace
                    .as_deref()
                    .is_some_and(|space| same_namespace(space, wanted))
            {
                continue;
            }
            let mut score = 0.0;
            for term in &terms {
                let Some(&tf) = doc.terms.get(term) else {
                    continue;
                };
                let df = self.frequency.get(term).copied().unwrap_or(0) as f64;
                let idf = (1.0 + (n - df + 0.5) / (df + 0.5)).ln();
                let tf = f64::from(tf);
                let norm = 1.0 - B + B * doc.len as f64 / self.average;
                score += idf * tf * (K1 + 1.0) / (tf + K1 * norm);
            }
            let is_exact = doc.name.to_lowercase() == exact;
            if score > 0.0 || is_exact {
                scored.push((i, score, is_exact));
            }
        }
        scored.sort_by(|a, b| {
            b.2.cmp(&a.2).then(b.1.total_cmp(&a.1)).then(a.0.cmp(&b.0))
        });
        scored.into_iter().take(limit).map(|(i, ..)| i).collect()
    }
}

fn schema_text(schema: &Value, depth: usize, out: &mut Vec<String>) {
    if depth > MAX_SCHEMA_DEPTH {
        return;
    }
    let Value::Object(map) = schema else {
        return;
    };
    if let Some(Value::Object(properties)) = map.get("properties") {
        for (name, property) in properties {
            out.push(name.clone());
            if let Some(Value::String(text)) = property.get("description") {
                out.push(text.clone());
            }
            schema_text(property, depth + 1, out);
        }
    }
    if let Some(items) = map.get("items") {
        schema_text(items, depth + 1, out);
    }
    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(Value::Array(options)) = map.get(key) {
            for option in options {
                schema_text(option, depth + 1, out);
            }
        }
    }
}
