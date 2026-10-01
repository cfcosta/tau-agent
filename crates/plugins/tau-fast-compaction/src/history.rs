//! The conversation as Jev sees it, whole: split in order into segments
//! that each fit a budget, never truncated and never left out.
//!
//! Ported from `tamaratran/jev-pruner` (`src/history.ts`, `splitHistory`;
//! see `THIRD_PARTY_NOTICES.md`). A message too large for a segment of
//! its own becomes continuations, one per field it holds, each labeled
//! with the field's name and the character offset it starts at. Offsets
//! count characters, where the original counts UTF-16 units, so a
//! continuation never splits a character.

use serde::Serialize;

use crate::state::{Role, json_tenths};

/// One message of the history.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Record {
    /// The message's index in the transcript.
    pub i: usize,
    pub role: Role,
    pub text: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<CallRecord>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_results: Vec<ResultRecord>,
    /// Set on a continuation: the one field it holds part of.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub part: Option<Part>,
}

/// A tool call: its id, tool and input as JSON, and, where the results
/// themselves are not shown, a note of its result.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CallRecord {
    pub id: String,
    pub tool: String,
    pub input: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
}

/// A tool result, verbatim.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResultRecord {
    pub id: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_error: bool,
    pub result: String,
}

/// Which part of one field a continuation holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Part {
    pub field: Field,
    /// The characters of the field before this part.
    pub offset: usize,
    pub total_chars: usize,
}

/// A field a message can be split along.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Field {
    #[serde(rename = "text")]
    Text,
    #[serde(rename = "tool_calls.input")]
    CallInput,
    #[serde(rename = "tool_calls.result")]
    CallResult,
    #[serde(rename = "tool_results.result")]
    ResultText,
}

/// The characters a continuation is first tried at, before doubling.
const FIRST_PROBE: usize = 4096;

/// Tenths of a token for the two quotes of an empty JSON string.
const QUOTES_TENTHS: usize = 18;
/// Tenths of a token for a comma between entries.
const COMMA_TENTHS: usize = 9;

impl Record {
    /// This record, holding only `piece` of one field and labeled as
    /// such. `index` picks the call or result for fields of those.
    fn fragment(
        &self,
        field: Field,
        index: usize,
        piece: String,
        offset: usize,
        total_chars: usize,
    ) -> Record {
        let mut fragment = Record {
            i: self.i,
            role: self.role,
            text: String::new(),
            tool_calls: Vec::new(),
            tool_results: Vec::new(),
            part: Some(Part {
                field,
                offset,
                total_chars,
            }),
        };
        match field {
            Field::Text => fragment.text = piece,
            Field::CallInput => fragment.tool_calls.push(CallRecord {
                input: piece,
                result: None,
                ..self.tool_calls[index].clone()
            }),
            Field::CallResult => fragment.tool_calls.push(CallRecord {
                input: String::new(),
                result: Some(piece),
                ..self.tool_calls[index].clone()
            }),
            Field::ResultText => fragment.tool_results.push(ResultRecord {
                result: piece,
                ..self.tool_results[index].clone()
            }),
        }
        fragment
    }

    /// Every field this record splits along, in order: its text when it
    /// has one, then each call's input and result note, then each
    /// result.
    fn fields(&self) -> Vec<(Field, usize, &str)> {
        let mut fields = Vec::new();
        if !self.text.is_empty() {
            fields.push((Field::Text, 0, self.text.as_str()));
        }
        for (index, call) in self.tool_calls.iter().enumerate() {
            fields.push((Field::CallInput, index, call.input.as_str()));
            if let Some(result) = &call.result {
                fields.push((Field::CallResult, index, result.as_str()));
            }
        }
        for (index, result) in self.tool_results.iter().enumerate() {
            fields.push((Field::ResultText, index, result.result.as_str()));
        }
        fields
    }
}

/// `record` as it goes into segments: whole when it fits `max_tenths`
/// alone, otherwise as continuations that each do. A continuation takes
/// as many characters as fit, back to the last line break when that
/// keeps at least half of them. Fails when not even one character of a
/// field fits.
pub fn split_record(
    record: &Record,
    max_tenths: usize,
) -> Result<Vec<(Record, usize)>, String> {
    let whole = json_tenths(record);
    if whole <= max_tenths {
        return Ok(vec![(record.clone(), whole)]);
    }
    let mut fragments = Vec::new();
    for (field, index, text) in record.fields() {
        let chars: Vec<char> = text.chars().collect();
        let total = chars.len();
        let piece = |from: usize, len: usize| -> String {
            chars[from..from + len].iter().collect()
        };
        let mut offset = 0;
        loop {
            // The empty fragment's cost, and the piece's inside its
            // quotes: letters never run across a quote, so they add up.
            let base = json_tenths(&record.fragment(
                field,
                index,
                String::new(),
                offset,
                total,
            ));
            let cost = |len: usize| {
                base + json_tenths(&piece(offset, len)) - QUOTES_TENTHS
            };
            let fits = |len: usize| cost(len) <= max_tenths;
            let remaining = total - offset;
            if !fits(0) || (remaining > 0 && !fits(1)) {
                return Err(format!(
                    "a {} of message {} cannot fit in {} tokens",
                    field_name(field),
                    record.i,
                    max_tenths / 10
                ));
            }
            // Doubling from a first probe, then halving between the
            // last length that fit and the first that did not.
            let (mut low, mut high) = (0, remaining);
            let mut probe = FIRST_PROBE.min(remaining);
            while probe > low {
                if fits(probe) {
                    low = probe;
                    probe = (probe * 2).min(remaining);
                } else {
                    high = probe - 1;
                    break;
                }
            }
            while low < high {
                let mid = (low + high).div_ceil(2);
                if fits(mid) {
                    low = mid;
                } else {
                    high = mid - 1;
                }
            }
            if offset + low < total
                && let Some(newline) =
                    chars[offset..offset + low].iter().rposition(|c| *c == '\n')
                && 2 * newline >= low
            {
                low = newline + 1;
            }
            let fragment = record.fragment(
                field,
                index,
                piece(offset, low),
                offset,
                total,
            );
            let tenths = json_tenths(&fragment);
            fragments.push((fragment, tenths));
            offset += low;
            if offset >= total {
                break;
            }
        }
    }
    Ok(fragments)
}

fn field_name(field: Field) -> &'static str {
    match field {
        Field::Text => "text",
        Field::CallInput => "tool call's input",
        Field::CallResult => "tool call's result note",
        Field::ResultText => "tool result",
    }
}

/// `records` split, in order, into segments whose JSON estimate each
/// fits `max_tokens`, splitting a record that cannot fit alone into
/// continuations (see [`split_record`]). Every record, or all its
/// continuations, lands in exactly one segment. No records make one
/// empty segment.
pub fn split_history(
    records: &[Record],
    max_tokens: usize,
) -> Result<Vec<Vec<Record>>, String> {
    let max_tenths = max_tokens * 10;
    // An array's brackets; each entry after the first adds a comma.
    let empty = json_tenths(&Vec::<Record>::new());
    let mut segments = Vec::new();
    let mut current: Vec<Record> = Vec::new();
    let mut tenths = empty;
    for record in records {
        for (fragment, cost) in
            split_record(record, max_tenths.saturating_sub(empty))?
        {
            let comma = if current.is_empty() { 0 } else { COMMA_TENTHS };
            if !current.is_empty() && tenths + comma + cost > max_tenths {
                segments.push(std::mem::take(&mut current));
                tenths = empty;
            }
            let comma = if current.is_empty() { 0 } else { COMMA_TENTHS };
            tenths += comma + cost;
            current.push(fragment);
        }
    }
    if !current.is_empty() || segments.is_empty() {
        segments.push(current);
    }
    Ok(segments)
}
