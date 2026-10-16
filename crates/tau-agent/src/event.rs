//! Run events: what a run reports while it works
//! (`docs/reference/agent-loop.md`, "Events").
//!
//! A run's events follow the grammar
//! `RunStart (TurnStart … TurnEnd)* RunEnd`, and nothing follows
//! `RunEnd`. Events from a child run carry `parent`, so one subscriber can
//! follow a whole workflow tree. An idle compaction
//! ([`crate::agent::Resumed::compact_idle`]) emits only what its rewrite
//! does, outside any start: plugins' charges, reports and errors, and
//! `ContextRewritten`.

use std::{sync::Arc, time::Duration};

use serde_json::Value;
use tau_ai::message::{StopReason as MessageStop, Usage};

use crate::{
    plugin::Trigger,
    tool::{RunId, ToolOutput},
};

/// Why a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum StopReason {
    /// The model finished.
    Stop,
    /// A limit was reached.
    Limit(LimitKind),
    Cancelled,
    /// The model's last response failed, or the loop could not continue.
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum LimitKind {
    Turns,
    Tokens,
    Usd,
    Time,
}

impl StopReason {
    /// The run's stop reason for a model response that ended the run.
    pub fn from_message(stop: MessageStop, error: Option<&str>) -> Self {
        match stop {
            MessageStop::Aborted => Self::Cancelled,
            MessageStop::Error => {
                Self::Error(error.unwrap_or("model error").to_owned())
            }
            _ => Self::Stop,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RunEvent {
    RunStart {
        run: RunId,
        parent: Option<RunId>,
        agent: Arc<str>,
        /// The parent's tool call that started the run: a sub-agent's.
        call: Option<Arc<str>>,
    },
    TurnStart {
        run: RunId,
        turn: u32,
    },
    TextDelta {
        run: RunId,
        parent: Option<RunId>,
        delta: String,
    },
    ThinkingDelta {
        run: RunId,
        delta: String,
    },
    ToolCallDelta {
        run: RunId,
        call_id: String,
        json_fragment: String,
    },
    /// A tool call starts. A nested call (`ToolCtx::call`) has the id
    /// of the call that made it as `parent`, and an id of its own of
    /// the form `<parent>/<n>`.
    ToolStart {
        run: RunId,
        call_id: String,
        tool: Arc<str>,
        args: Value,
        parent: Option<String>,
    },
    ToolUpdate {
        run: RunId,
        call_id: String,
        partial: Arc<ToolOutput>,
        parent: Option<String>,
    },
    ToolEnd {
        run: RunId,
        call_id: String,
        output: Arc<ToolOutput>,
        is_error: bool,
        parent: Option<String>,
    },
    TurnEnd {
        run: RunId,
        turn: u32,
        usage: Usage,
    },
    /// A plugin rewrote the context; the next request goes in full.
    ContextRewritten {
        run: RunId,
        plugin: Arc<str>,
        tokens_before: u64,
        tokens_after: u64,
        /// Why it was offered.
        trigger: Trigger,
    },
    /// A response failed in a way worth retrying; attempt `attempt`
    /// starts after `delay`. Comes inside the turn it retries.
    Retry {
        run: RunId,
        turn: u32,
        attempt: u32,
        delay: Duration,
        error: String,
    },
    /// The run read a steering message (`Run::steer`): it goes to the
    /// model as the user's, and the next turn starts. Comes after the
    /// turn's `TurnEnd`.
    Steered {
        run: RunId,
        text: String,
    },
    /// A plugin kept the run from stopping: `message` goes to the model
    /// as the user's, and another turn starts. Comes after the turn's
    /// `TurnEnd`.
    Continued {
        run: RunId,
        plugin: Arc<str>,
        message: String,
    },
    /// What a plugin decided, for interfaces to show and review: a
    /// blocked call, a flagged one, a held stop. `body` is the plugin's
    /// own JSON. Comes before the event it explains, such as the
    /// `ToolEnd` of a call it blocked.
    PluginReport {
        run: RunId,
        plugin: Arc<str>,
        body: Value,
    },
    /// A plugin charged usage to the run (`PluginCtx::charge`): a side
    /// request of its own, such as a Jev check. It is already part of
    /// the run's totals; this says whose it is. Comes before the event
    /// that follows the charge.
    PluginCharged {
        run: RunId,
        plugin: Arc<str>,
        usage: Usage,
    },
    /// A plugin failed at a seam where failing does not end the run.
    PluginError {
        run: RunId,
        plugin: Arc<str>,
        message: String,
    },
    RunEnd {
        run: RunId,
        parent: Option<RunId>,
        stop: StopReason,
        cost: f64,
    },
}

impl RunEvent {
    /// The run the event is about.
    pub fn run(&self) -> &RunId {
        match self {
            Self::RunStart { run, .. }
            | Self::TurnStart { run, .. }
            | Self::TextDelta { run, .. }
            | Self::ThinkingDelta { run, .. }
            | Self::ToolCallDelta { run, .. }
            | Self::ToolStart { run, .. }
            | Self::ToolUpdate { run, .. }
            | Self::ToolEnd { run, .. }
            | Self::TurnEnd { run, .. }
            | Self::ContextRewritten { run, .. }
            | Self::Retry { run, .. }
            | Self::Steered { run, .. }
            | Self::Continued { run, .. }
            | Self::PluginReport { run, .. }
            | Self::PluginCharged { run, .. }
            | Self::PluginError { run, .. }
            | Self::RunEnd { run, .. } => run,
        }
    }
}

#[cfg(all(test, feature = "serde"))]
mod tests {
    use hegel::{
        TestCase,
        generators::{self as gs, Generator as _},
    };
    use serde_json::json;
    use tau_ai::message::{ImageContent, InputBlock, TextContent, UsageCost};

    use super::*;

    /// Text with the characters JSON escapes, and some it does not, or
    /// none at all.
    fn text(tc: &TestCase) -> String {
        tc.draw(hegel::one_of!(
            gs::just(String::new()),
            gs::text().max_size(8),
            gs::text().alphabet("a\"\\\n\u{0}/é😀").max_size(8),
        ))
    }

    fn run_id(tc: &TestCase) -> RunId {
        RunId(Arc::from(
            tc.draw(gs::sampled_from(vec!["r1", "r2", "r1/2"])),
        ))
    }

    /// Any JSON value without floats (whose text form `serde_json` may
    /// not read back to the same bits), nested a little.
    fn value(tc: &TestCase, depth: u8) -> Value {
        let kinds = if depth == 0 { 4 } else { 6 };
        match tc.draw(gs::integers::<u8>().max_value(kinds - 1)) {
            0 => Value::Null,
            1 => json!(tc.draw(gs::booleans())),
            2 => json!(tc.draw(gs::integers::<i64>())),
            3 => json!(text(tc)),
            4 => {
                let n = tc.draw(gs::integers::<usize>().max_value(3));
                Value::Array((0..n).map(|_| value(tc, depth - 1)).collect())
            }
            _ => {
                let n = tc.draw(gs::integers::<usize>().max_value(3));
                Value::Object(
                    (0..n).map(|_| (text(tc), value(tc, depth - 1))).collect(),
                )
            }
        }
    }

    /// Costs in 1/1024ths of a dollar, which print and read back exactly.
    fn usage(tc: &TestCase) -> Usage {
        let tokens = || gs::integers::<u64>().max_value(1 << 40);
        let usd = || {
            gs::integers::<u32>()
                .max_value(1 << 20)
                .map(|n| f64::from(n) / 1024.0)
        };
        Usage {
            input: tc.draw(tokens()),
            output: tc.draw(tokens()),
            cache_read: tc.draw(tokens()),
            cache_write: tc.draw(tokens()),
            reasoning: tc.draw(gs::optional(tokens())),
            total_tokens: tc.draw(tokens()),
            cost: UsageCost {
                input: tc.draw(usd()),
                output: tc.draw(usd()),
                cache_read: tc.draw(usd()),
                cache_write: tc.draw(usd()),
                total: tc.draw(usd()),
            },
        }
    }

    fn output(tc: &TestCase) -> Arc<ToolOutput> {
        let blocks = tc.draw(gs::integers::<usize>().max_value(2));
        let content = (0..blocks)
            .map(|_| {
                if tc.draw(gs::booleans()) {
                    InputBlock::Text(TextContent {
                        text: text(tc),
                        text_signature: None,
                    })
                } else {
                    InputBlock::Image(ImageContent {
                        data: "AAAA".into(),
                        mime_type: "image/png".into(),
                    })
                }
            })
            .collect();
        // Not `Some(Value::Null)`: `None` and an explicit `null` both write
        // `null`, which reads back as `None`. Telling them apart would
        // change the JSON every interface reads, so it is left as it is.
        let present =
            |tc: &TestCase| Some(value(tc, 2)).filter(|value| !value.is_null());
        let details = if tc.draw(gs::booleans()) {
            present(tc)
        } else {
            None
        };
        let structured = if tc.draw(gs::booleans()) {
            present(tc)
        } else {
            None
        };
        Arc::new(ToolOutput {
            content,
            details,
            structured,
        })
    }

    fn arc(tc: &TestCase) -> Arc<str> {
        Arc::from(text(tc))
    }

    /// One event of any kind, every field drawn.
    fn event(tc: &TestCase) -> RunEvent {
        let run = run_id(tc);
        let parent_run = || tc.draw(gs::booleans()).then(|| run_id(tc));
        let parent_call = || tc.draw(gs::booleans()).then(|| text(tc));
        let turn = || tc.draw(gs::integers::<u32>());
        match tc.draw(gs::integers::<u8>().max_value(17)) {
            0 => RunEvent::RunStart {
                run,
                parent: parent_run(),
                agent: arc(tc),
                call: tc.draw(gs::booleans()).then(|| arc(tc)),
            },
            1 => RunEvent::TurnStart { run, turn: turn() },
            2 => RunEvent::TextDelta {
                run,
                parent: parent_run(),
                delta: text(tc),
            },
            3 => RunEvent::ThinkingDelta {
                run,
                delta: text(tc),
            },
            4 => RunEvent::ToolCallDelta {
                run,
                call_id: text(tc),
                json_fragment: text(tc),
            },
            5 => RunEvent::ToolStart {
                run,
                call_id: text(tc),
                tool: arc(tc),
                args: value(tc, 2),
                parent: parent_call(),
            },
            6 => RunEvent::ToolUpdate {
                run,
                call_id: text(tc),
                partial: output(tc),
                parent: parent_call(),
            },
            7 => RunEvent::ToolEnd {
                run,
                call_id: text(tc),
                output: output(tc),
                is_error: tc.draw(gs::booleans()),
                parent: parent_call(),
            },
            8 => RunEvent::TurnEnd {
                run,
                turn: turn(),
                usage: usage(tc),
            },
            9 => RunEvent::ContextRewritten {
                run,
                plugin: arc(tc),
                tokens_before: tc.draw(gs::integers::<u64>()),
                tokens_after: tc.draw(gs::integers::<u64>()),
                trigger: [
                    Trigger::TurnEnd,
                    Trigger::Start,
                    Trigger::Overflow,
                    Trigger::Idle,
                ][tc.draw(gs::integers::<usize>().max_value(3))],
            },
            10 => RunEvent::Retry {
                run,
                turn: turn(),
                attempt: turn(),
                delay: Duration::new(
                    tc.draw(gs::integers::<u64>()),
                    tc.draw(gs::integers::<u32>().max_value(999_999_999)),
                ),
                error: text(tc),
            },
            11 => RunEvent::Steered {
                run,
                text: text(tc),
            },
            12 => RunEvent::Continued {
                run,
                plugin: arc(tc),
                message: text(tc),
            },
            13 => RunEvent::PluginReport {
                run,
                plugin: arc(tc),
                body: value(tc, 2),
            },
            14 => RunEvent::PluginCharged {
                run,
                plugin: arc(tc),
                usage: usage(tc),
            },
            15 => RunEvent::PluginError {
                run,
                plugin: arc(tc),
                message: text(tc),
            },
            _ => RunEvent::RunEnd {
                run,
                parent: parent_run(),
                stop: match tc.draw(gs::integers::<u8>().max_value(3)) {
                    0 => StopReason::Stop,
                    1 => StopReason::Cancelled,
                    2 => StopReason::Error(text(tc)),
                    _ => StopReason::Limit(
                        [
                            LimitKind::Turns,
                            LimitKind::Tokens,
                            LimitKind::Usd,
                            LimitKind::Time,
                        ][tc.draw(gs::integers::<usize>().max_value(3))],
                    ),
                },
                cost: f64::from(tc.draw(gs::integers::<u32>())) / 1024.0,
            },
        }
    }

    /// Any event, of every kind, reads back from its JSON as it was
    /// written, so an interface across a wire (decision 0013) sees what
    /// the run emitted.
    #[hegel::test(test_cases = 500)]
    fn events_round_trip_through_json(tc: TestCase) {
        let event = event(&tc);
        let text = serde_json::to_string(&event).unwrap();
        tc.note(&text);
        assert_eq!(serde_json::from_str::<RunEvent>(&text).unwrap(), event);
    }
}
