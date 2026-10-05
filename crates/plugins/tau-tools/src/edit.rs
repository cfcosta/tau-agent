//! `edit`: exact text replacement (`docs/reference/tools.md`, "edit"),
//! ported from pi's `edit.ts` and `edit-diff.ts`.
//!
//! **Deliberate difference from pi:** uniqueness is checked in the same
//! space a match was found in. pi always re-normalizes for the
//! occurrence count (`edit-diff.ts:328`), which rejects a match that is
//! unique exactly but has a near-duplicate elsewhere. Here, an edit
//! matched exactly is checked for uniqueness among exact matches; an
//! edit that needed fuzzy matching is checked among fuzzy matches.

use std::io;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use similar::{ChangeTag, TextDiff};
use tau_agent::{
    error::ToolError,
    tool::{AgentTool, ToolCtx, ToolOutput},
};
use tau_ai::message::{InputBlock, TextContent};
use unicode_normalization::UnicodeNormalization;

use crate::{ABORTED, lock, path::Root};

const DESCRIPTION: &str = "Edit a single file using exact text replacement. Every edits[].oldText must match a unique, non-overlapping region of the original file. If two changes affect the same block or nearby lines, merge them into one edit instead of emitting overlapping edits. Do not include large unchanged regions just to connect distant changes.";

/// One targeted replacement (`{ oldText, newText }`).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct EditEntry {
    /// Exact text for one targeted replacement. It must be unique in the original file and must not overlap with any other edits[].oldText in the same call.
    #[serde(rename = "oldText")]
    pub old_text: String,
    /// Replacement text for this targeted edit.
    #[serde(rename = "newText")]
    pub new_text: String,
}

/// Arguments of the `edit` tool.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct EditArgs {
    /// Path to the file to edit (relative or absolute)
    pub path: String,
    /// One or more targeted replacements. Each edit is matched against the original file, not incrementally. Do not include overlapping or nested edits. If two changes touch the same block or nearby lines, merge them into one edit instead.
    pub edits: Vec<EditEntry>,
}

/// The `edit` tool: exact (falling back to fuzzy) text replacement,
/// under the per-path lock shared with `write`.
pub struct Edit {
    root: Root,
    parameters: Value,
}

impl Edit {
    pub fn new(root: Root) -> Self {
        let parameters = serde_json::to_value(schemars::schema_for!(EditArgs))
            .expect("a generated schema is valid JSON");
        Self { root, parameters }
    }
}

#[async_trait]
impl AgentTool for Edit {
    fn name(&self) -> &str {
        "edit"
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn prepare_arguments(&self, raw: Value) -> Value {
        prepare_edit_arguments(raw)
    }

    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let args: EditArgs = serde_json::from_value(args)?;
        if args.edits.is_empty() {
            return Err(ToolError::from(
                "Edit tool input is invalid. edits must contain at least one replacement.",
            ));
        }

        let resolved = self.root.resolve(&args.path);
        let _guard = lock::lock(&resolved).await;

        if ctx.cancel.is_cancelled() {
            return Err(ToolError::from(ABORTED));
        }

        let raw = tokio::fs::read(&resolved)
            .await
            .map_err(|err| edit_io_error(&args.path, &err))?;
        if ctx.cancel.is_cancelled() {
            return Err(ToolError::from(ABORTED));
        }

        let raw_text = String::from_utf8_lossy(&raw).into_owned();
        let (bom, content) = split_bom(&raw_text);
        let ending = detect_line_ending(content);
        let normalized = normalize_to_lf(content);

        let applied = apply_edits_to_normalized_content(
            &normalized,
            &args.edits,
            &args.path,
        )?;
        if ctx.cancel.is_cancelled() {
            return Err(ToolError::from(ABORTED));
        }

        let final_content = format!(
            "{bom}{}",
            restore_line_endings(&applied.new_content, ending)
        );
        tokio::fs::write(&resolved, final_content.as_bytes())
            .await
            .map_err(|err| edit_io_error(&args.path, &err))?;
        if ctx.cancel.is_cancelled() {
            return Err(ToolError::from(ABORTED));
        }

        let (diff, first_changed_line) = generate_diff(
            &args.path,
            &applied.base_content,
            &applied.new_content,
        );

        Ok(ToolOutput {
            content: vec![InputBlock::Text(TextContent {
                text: format!(
                    "Successfully replaced {} block(s) in {}.",
                    args.edits.len(),
                    args.path
                ),
                text_signature: None,
            })],
            details: Some(json!({
                "diff": diff,
                "firstChangedLine": first_changed_line,
            })),
            structured: None,
        })
    }
}

/// Fixes common model mistakes (`docs/reference/tools.md`, "edit",
/// "Argument repair"), ported from pi's `prepareEditArguments`.
fn prepare_edit_arguments(raw: Value) -> Value {
    let Value::Object(mut map) = raw else {
        return raw;
    };

    if let Some(edits_value) = map.get("edits").cloned() {
        match edits_value {
            Value::String(s) => {
                if let Ok(parsed) = serde_json::from_str::<Value>(&s) {
                    if parsed.is_array() {
                        map.insert("edits".to_owned(), parsed);
                    } else if is_single_edit_input(&parsed) {
                        map.insert(
                            "edits".to_owned(),
                            Value::Array(vec![parsed]),
                        );
                    }
                }
            }
            other if is_single_edit_input(&other) => {
                map.insert("edits".to_owned(), Value::Array(vec![other]));
            }
            _ => {}
        }
    }

    let old_text = map
        .get("oldText")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let new_text = map
        .get("newText")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if let (Some(old_text), Some(new_text)) = (old_text, new_text) {
        let mut edits: Vec<Value> = match map.get("edits") {
            Some(Value::Array(arr)) => arr.clone(),
            _ => Vec::new(),
        };
        edits.push(json!({"oldText": old_text, "newText": new_text}));
        map.remove("oldText");
        map.remove("newText");
        map.insert("edits".to_owned(), Value::Array(edits));
    }

    Value::Object(map)
}

/// A `{ oldText: string, newText: string }` object, as a single edit
/// mistakenly sent instead of a one-element `edits` array.
fn is_single_edit_input(value: &Value) -> bool {
    matches!(
        value,
        Value::Object(o)
            if matches!(o.get("oldText"), Some(Value::String(_)))
                && matches!(o.get("newText"), Some(Value::String(_)))
    )
}

fn edit_io_error(path: &str, err: &io::Error) -> String {
    format!(
        "Could not edit file: {path}. Error code: {}.",
        crate::errno::code(err)
    )
}

/// Splits a leading UTF-8 byte order mark from decoded text.
fn split_bom(content: &str) -> (&'static str, &str) {
    match content.strip_prefix('\u{FEFF}') {
        Some(rest) => ("\u{FEFF}", rest),
        None => ("", content),
    }
}

/// `"\r\n"` if the file's first line ending is CRLF, else `"\n"`.
fn detect_line_ending(content: &str) -> &'static str {
    match content.find('\n') {
        Some(lf) if content[..lf].ends_with('\r') => "\r\n",
        _ => "\n",
    }
}

fn normalize_to_lf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn restore_line_endings(text: &str, ending: &str) -> String {
    if ending == "\r\n" {
        text.replace('\n', "\r\n")
    } else {
        text.to_owned()
    }
}

/// Whitespace that JavaScript's `String.prototype.trimEnd` strips:
/// Unicode whitespace, plus NBSP and the BOM, which Rust's
/// `char::is_whitespace` does not count as whitespace.
fn is_js_trim_whitespace(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\u{00A0}' | '\u{FEFF}')
}

/// Smart quotes, Unicode dashes and special spaces mapped to their
/// ASCII equivalents, as pi's `normalizeForFuzzyMatch` does.
fn map_fuzzy_char(c: char) -> char {
    match c {
        '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
        '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
        '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
        '\u{00A0}'
        | '\u{2002}'..='\u{200A}'
        | '\u{202F}'
        | '\u{205F}'
        | '\u{3000}' => ' ',
        other => other,
    }
}

/// Normalizes text for fuzzy matching (`tools.md`, "edit", step 3):
/// NFKC, trailing whitespace stripped per line, smart quotes and dashes
/// mapped to ASCII, and special spaces mapped to a normal space.
fn normalize_for_fuzzy_match(text: &str) -> String {
    let nfkc: String = text.nfkc().collect();
    let trimmed = nfkc
        .split('\n')
        .map(|line| line.trim_end_matches(is_js_trim_whitespace))
        .collect::<Vec<_>>()
        .join("\n");
    trimmed.chars().map(map_fuzzy_char).collect()
}

/// A match of `old_text` in `content`: exact if found verbatim,
/// otherwise fuzzy (both sides normalized).
struct FuzzyMatch {
    found: bool,
    index: usize,
    match_length: usize,
    used_fuzzy: bool,
}

fn fuzzy_find_text(content: &str, old_text: &str) -> FuzzyMatch {
    if let Some(index) = content.find(old_text) {
        return FuzzyMatch {
            found: true,
            index,
            match_length: old_text.len(),
            used_fuzzy: false,
        };
    }
    let fuzzy_content = normalize_for_fuzzy_match(content);
    let fuzzy_old = normalize_for_fuzzy_match(old_text);
    match fuzzy_content.find(&fuzzy_old) {
        Some(index) => FuzzyMatch {
            found: true,
            index,
            match_length: fuzzy_old.len(),
            used_fuzzy: true,
        },
        None => FuzzyMatch {
            found: false,
            index: 0,
            match_length: 0,
            used_fuzzy: false,
        },
    }
}

fn count_occurrences(haystack: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    haystack.matches(needle).count()
}

fn empty_old_text_error(path: &str, index: usize, total: usize) -> String {
    if total == 1 {
        format!("oldText must not be empty in {path}.")
    } else {
        format!("edits[{index}].oldText must not be empty in {path}.")
    }
}

fn not_found_error(path: &str, index: usize, total: usize) -> String {
    if total == 1 {
        format!(
            "Could not find the exact text in {path}. The old text must match exactly including all whitespace and newlines."
        )
    } else {
        format!(
            "Could not find edits[{index}] in {path}. The oldText must match exactly including all whitespace and newlines."
        )
    }
}

fn duplicate_error(
    path: &str,
    index: usize,
    total: usize,
    occurrences: usize,
) -> String {
    if total == 1 {
        format!(
            "Found {occurrences} occurrences of the text in {path}. The text must be unique. Please provide more context to make it unique."
        )
    } else {
        format!(
            "Found {occurrences} occurrences of edits[{index}] in {path}. Each oldText must be unique. Please provide more context to make it unique."
        )
    }
}

fn overlap_error(path: &str, first: usize, second: usize) -> String {
    format!(
        "edits[{first}] and edits[{second}] overlap in {path}. Merge them into one edit or target disjoint regions."
    )
}

fn no_change_error(path: &str, total: usize) -> String {
    if total == 1 {
        format!(
            "No changes made to {path}. The replacement produced identical content. This might indicate an issue with special characters or the text not existing as expected."
        )
    } else {
        format!(
            "No changes made to {path}. The replacements produced identical content."
        )
    }
}

/// A matched edit, positioned in whichever content space it matched.
#[derive(Clone)]
struct MatchedEdit {
    edit_index: usize,
    match_index: usize,
    match_length: usize,
    new_text: String,
}

struct AppliedEdits {
    base_content: String,
    new_content: String,
}

/// Applies `edits` to `normalized_content` (`tools.md`, "edit",
/// "Algorithm" steps 2-7).
fn apply_edits_to_normalized_content(
    normalized_content: &str,
    edits: &[EditEntry],
    path: &str,
) -> Result<AppliedEdits, ToolError> {
    let normalized_edits: Vec<(String, String)> = edits
        .iter()
        .map(|e| (normalize_to_lf(&e.old_text), normalize_to_lf(&e.new_text)))
        .collect();
    let total = normalized_edits.len();

    for (i, (old, _)) in normalized_edits.iter().enumerate() {
        if old.is_empty() {
            return Err(ToolError::from(empty_old_text_error(path, i, total)));
        }
    }

    let initial_matches: Vec<FuzzyMatch> = normalized_edits
        .iter()
        .map(|(old, _)| fuzzy_find_text(normalized_content, old))
        .collect();
    let used_fuzzy = initial_matches.iter().any(|m| m.used_fuzzy);
    let replacement_base_content = if used_fuzzy {
        normalize_for_fuzzy_match(normalized_content)
    } else {
        normalized_content.to_owned()
    };

    let mut matched: Vec<MatchedEdit> = Vec::with_capacity(total);
    for (i, (old, new)) in normalized_edits.iter().enumerate() {
        let match_result = fuzzy_find_text(&replacement_base_content, old);
        if !match_result.found {
            return Err(ToolError::from(not_found_error(path, i, total)));
        }

        // Deliberate difference from pi: count occurrences in the same
        // space the match was found in, not always in fuzzy space.
        let occurrences = if used_fuzzy {
            count_occurrences(
                &replacement_base_content,
                &normalize_for_fuzzy_match(old),
            )
        } else {
            count_occurrences(&replacement_base_content, old)
        };
        if occurrences > 1 {
            return Err(ToolError::from(duplicate_error(
                path,
                i,
                total,
                occurrences,
            )));
        }

        matched.push(MatchedEdit {
            edit_index: i,
            match_index: match_result.index,
            match_length: match_result.match_length,
            new_text: new.clone(),
        });
    }

    matched.sort_by_key(|m| m.match_index);
    for pair in matched.windows(2) {
        let (previous, current) = (&pair[0], &pair[1]);
        if previous.match_index + previous.match_length > current.match_index {
            return Err(ToolError::from(overlap_error(
                path,
                previous.edit_index,
                current.edit_index,
            )));
        }
    }

    let base_content = normalized_content.to_owned();
    let new_content = if used_fuzzy {
        apply_replacements_preserving_unchanged_lines(
            normalized_content,
            &replacement_base_content,
            &matched,
        )?
    } else {
        apply_replacements(&replacement_base_content, &matched, 0)
    };

    if base_content == new_content {
        return Err(ToolError::from(no_change_error(path, total)));
    }

    Ok(AppliedEdits {
        base_content,
        new_content,
    })
}

/// Splices `replacements` into `content` in reverse order, so earlier
/// offsets stay valid. `offset` is subtracted from each match index
/// first, for replacing within a slice of a larger content.
fn apply_replacements(
    content: &str,
    replacements: &[MatchedEdit],
    offset: usize,
) -> String {
    let mut result = content.to_owned();
    for replacement in replacements.iter().rev() {
        let start = replacement.match_index - offset;
        let end = start + replacement.match_length;
        result = format!(
            "{}{}{}",
            &result[..start],
            replacement.new_text,
            &result[end..]
        );
    }
    result
}

struct LineSpan {
    start: usize,
    end: usize,
}

/// Splits `content` into lines that each keep their trailing `\n`, as
/// the regex `/[^\n]*\n|[^\n]+/g` does; a last partial line keeps no
/// `\n`, and an empty string has no lines.
fn split_lines_with_endings(content: &str) -> Vec<&str> {
    if content.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut start = 0;
    for (i, b) in content.bytes().enumerate() {
        if b == b'\n' {
            out.push(&content[start..=i]);
            start = i + 1;
        }
    }
    let rest = &content[start..];
    if !rest.is_empty() {
        out.push(rest);
    }
    out
}

fn line_spans(content: &str) -> Vec<LineSpan> {
    let mut offset = 0;
    split_lines_with_endings(content)
        .into_iter()
        .map(|line| {
            let span = LineSpan {
                start: offset,
                end: offset + line.len(),
            };
            offset = span.end;
            span
        })
        .collect()
}

/// The `[start, end)` line range (0-based, end exclusive) a replacement
/// touches in `lines`.
fn replacement_line_range(
    lines: &[LineSpan],
    match_index: usize,
    match_length: usize,
) -> Result<(usize, usize), ToolError> {
    let start = match_index;
    let end = match_index + match_length;
    let start_line = lines
        .iter()
        .position(|line| start >= line.start && start < line.end)
        .ok_or_else(|| {
            ToolError::from("Replacement range is outside the base content.")
        })?;
    let end_line = lines[start_line..]
        .iter()
        .position(|line| line.end >= end)
        .map(|offset| start_line + offset)
        .ok_or_else(|| {
            ToolError::from("Replacement range is outside the base content.")
        })?;
    Ok((start_line, end_line + 1))
}

struct Group {
    start_line: usize,
    end_line: usize,
    replacements: Vec<MatchedEdit>,
}

/// Applies `replacements` (matched against `base_content`) to
/// `original_content`, a differently-normalized view of the same text,
/// while keeping the original bytes of every line no replacement
/// touches (`tools.md`, "edit", step 6, fuzzy mode). Ported from pi's
/// `applyReplacementsPreservingUnchangedLines`.
fn apply_replacements_preserving_unchanged_lines(
    original_content: &str,
    base_content: &str,
    replacements: &[MatchedEdit],
) -> Result<String, ToolError> {
    let original_lines = split_lines_with_endings(original_content);
    let base_lines = line_spans(base_content);
    if original_lines.len() != base_lines.len() {
        return Err(ToolError::from(
            "Cannot preserve unchanged lines because the base content has a different line count.",
        ));
    }

    let mut sorted: Vec<MatchedEdit> = replacements.to_vec();
    sorted.sort_by_key(|r| r.match_index);

    let mut groups: Vec<Group> = Vec::new();
    for replacement in sorted {
        let (start_line, end_line) = replacement_line_range(
            &base_lines,
            replacement.match_index,
            replacement.match_length,
        )?;
        if let Some(last) = groups.last_mut()
            && start_line < last.end_line
        {
            last.end_line = last.end_line.max(end_line);
            last.replacements.push(replacement);
            continue;
        }
        groups.push(Group {
            start_line,
            end_line,
            replacements: vec![replacement],
        });
    }

    let mut result = String::new();
    let mut original_line_index = 0;
    for group in &groups {
        result.push_str(
            &original_lines[original_line_index..group.start_line].concat(),
        );

        let group_start_offset = base_lines[group.start_line].start;
        let group_end_offset = base_lines[group.end_line - 1].end;
        let slice = &base_content[group_start_offset..group_end_offset];
        result.push_str(&apply_replacements(
            slice,
            &group.replacements,
            group_start_offset,
        ));

        original_line_index = group.end_line;
    }
    result.push_str(&original_lines[original_line_index..].concat());

    Ok(result)
}

/// A unified diff (`similar`) of the change, and the first changed line
/// number in `new` (`tools.md`, "edit", "Result details"). Lines end at
/// `\n` only, as `read` numbers them: `similar`'s own split also ends
/// one at a lone `\r`.
pub(crate) fn generate_diff(
    path: &str,
    old: &str,
    new: &str,
) -> (String, usize) {
    let old: Vec<&str> = old.split_inclusive('\n').collect();
    let new: Vec<&str> = new.split_inclusive('\n').collect();
    let diff = TextDiff::from_slices(&old, &new);
    let text = diff
        .unified_diff()
        .context_radius(4)
        .header(path, path)
        .to_string();

    let mut new_line = 1usize;
    let mut first_changed = None;
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Equal => new_line += 1,
            ChangeTag::Insert | ChangeTag::Delete => {
                first_changed = Some(new_line);
                break;
            }
        }
    }
    (text, first_changed.unwrap_or(1))
}
