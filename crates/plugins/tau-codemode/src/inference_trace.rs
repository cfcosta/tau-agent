//! Private inference provenance and durable budget reconstruction.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_agent::tool::RunId;
use tau_ai::message::Usage;

/// The maximum stored UTF-8 bytes of provider text per inference.
pub const MAX_RAW_OUTPUT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum Record {
    Started {
        trace_id: String,
        owner: RunId,
        task: String,
        context: Value,
        schema: Option<Value>,
        model: String,
        effort: Option<String>,
    },
    /// Persisted before the provider can start this numbered attempt.
    Attempt {
        trace_id: String,
        owner: RunId,
        number: u32,
    },
    Finished {
        trace_id: String,
        owner: RunId,
        /// Every reserved attempt has a final SDK report. A terminal trace
        /// record can still exist when this is false.
        complete: bool,
        selected: Option<Value>,
        raw_output: Option<String>,
        raw_output_truncated: bool,
        error: Option<String>,
        attempts: Vec<Attempt>,
        total_usage: Usage,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attempt {
    pub number: u32,
    pub outcome: AttemptOutcome,
    /// The SDK value. Zero can mean the transport supplied its default.
    pub reported_usage: Usage,
    pub usage_provenance: UsageProvenance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptOutcome {
    Finished,
    Cancelled,
    BrokeGrammar,
    NoTerminal,
    OpenFailed,
    /// The permit ended without a terminal SDK report.
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageProvenance {
    /// A nonzero value reached the SDK observer.
    SdkReported,
    /// The SDK supplied zero; the provider may not have reported usage.
    SdkZeroOrDefault,
    /// Legacy label for an open failure. It does not establish whether the
    /// provider accepted the request or incurred usage.
    NoProviderResponse,
    /// Final usage is unknown; reported_usage can still contain a partial value.
    Unknown,
}

impl Record {
    pub fn trace_id(&self) -> &str {
        match self {
            Self::Started { trace_id, .. }
            | Self::Attempt { trace_id, .. }
            | Self::Finished { trace_id, .. } => trace_id,
        }
    }

    pub fn owner(&self) -> &RunId {
        match self {
            Self::Started { owner, .. }
            | Self::Attempt { owner, .. }
            | Self::Finished { owner, .. } => owner,
        }
    }
}

/// Keep a UTF-8 prefix and state whether bytes were removed.
pub fn bounded_raw_output(raw: &str) -> (String, bool) {
    if raw.len() <= MAX_RAW_OUTPUT_BYTES {
        return (raw.to_owned(), false);
    }
    let mut end = MAX_RAW_OUTPUT_BYTES;
    while !raw.is_char_boundary(end) {
        end -= 1;
    }
    (raw[..end].to_owned(), true)
}

/// The counters to restore for precisely one owner, ignoring inherited traces.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RestoredBudget {
    pub calls: usize,
    pub usage: Usage,
    pub incomplete_attempt: bool,
}

#[derive(Default)]
struct TraceState {
    started: bool,
    reservations: Vec<u32>,
    finished: Option<(bool, Vec<Attempt>, Usage)>,
}

/// Verify the ledger before it can authorize another provider attempt.
pub fn restore_budget(
    records: &[Value],
    owner: &RunId,
) -> Result<RestoredBudget, String> {
    let mut traces: BTreeMap<String, TraceState> = BTreeMap::new();
    for value in records {
        if value.get("kind").and_then(Value::as_str) != Some("inference") {
            continue;
        }
        let Some(record_owner) = value.get("owner").and_then(Value::as_str)
        else {
            return Err("inference record has invalid owner".into());
        };
        if record_owner != owner.0.as_ref() {
            continue;
        }
        let record: Record = serde_json::from_value(value.clone())
            .map_err(|error| format!("invalid inference record: {error}"))?;
        let state = traces.entry(record.trace_id().to_owned()).or_default();
        match record {
            Record::Started { .. } => {
                if state.started
                    || !state.reservations.is_empty()
                    || state.finished.is_some()
                {
                    return Err("duplicate inference start".into());
                }
                state.started = true;
            }
            Record::Attempt { number, .. } => {
                if !state.started
                    || state.finished.is_some()
                    || number != state.reservations.len() as u32 + 1
                {
                    return Err("invalid inference attempt reservation".into());
                }
                state.reservations.push(number);
            }
            Record::Finished {
                complete,
                attempts,
                total_usage,
                ..
            } => {
                if !state.started || state.finished.is_some() {
                    return Err("invalid inference terminal record".into());
                }
                state.finished = Some((complete, attempts, total_usage));
            }
        }
    }
    let mut restored = RestoredBudget::default();
    for state in traces.values() {
        if !state.started {
            return Err("inference attempt without start".into());
        }
        restored.calls += state.reservations.len();
        if let Some((complete, attempts, total)) = &state.finished {
            if attempts.iter().map(|a| a.number).collect::<Vec<_>>()
                != state.reservations
            {
                return Err(
                    "inference attempts do not match reservations".into()
                );
            }
            let mut summed = Usage::default();
            for attempt in attempts {
                summed += &attempt.reported_usage;
            }
            if summed != *total {
                return Err(
                    "inference total usage does not match attempts".into()
                );
            }
            restored.usage += total;
            if !complete
                || attempts.iter().any(|attempt| {
                    attempt.outcome != AttemptOutcome::Finished
                        || !matches!(
                            attempt.usage_provenance,
                            UsageProvenance::SdkReported
                                | UsageProvenance::SdkZeroOrDefault
                        )
                })
            {
                restored.incomplete_attempt = true;
            }
        } else if !state.reservations.is_empty() {
            restored.incomplete_attempt = true;
        }
    }
    Ok(restored)
}
