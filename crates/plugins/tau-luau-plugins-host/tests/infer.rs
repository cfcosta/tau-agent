//! What `infer` takes of an answer in a run hook (ADR 0050): only a
//! whole one. An answer that stopped with an error, was cut off at the
//! model's limit or called a tool is refused, in that order, with why,
//! as pi's compaction refuses a summary (`docs/reference/compaction.md`,
//! "Rejected summaries").

use hegel::{TestCase, generators as gs};
use tau_ai::message::{AssistantBlock, StopReason};
use tau_luau_plugins_host::infer::{Refused, answer_text};
use tau_testing::generators;

#[hegel::test(test_cases = 300)]
fn only_a_whole_answer_is_taken(tc: TestCase) {
    let mut answer = tc.draw(generators::assistant_message());
    if answer.stop_reason == StopReason::Error && tc.draw(gs::booleans()) {
        answer.error_message = None;
    }
    let calls = answer
        .content
        .iter()
        .any(|block| matches!(block, AssistantBlock::ToolCall(_)));
    let text: Vec<&str> = answer
        .content
        .iter()
        .filter_map(|block| match block {
            AssistantBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect();
    let refused = |error: &str, reason| {
        Err(Refused {
            error: error.to_owned(),
            reason: Some(reason),
        })
    };
    let want = match answer.stop_reason {
        StopReason::Error => refused(
            answer.error_message.as_deref().unwrap_or("Unknown error"),
            "error",
        ),
        StopReason::Length => refused(
            "the answer hit the model's output limit and is incomplete",
            "length",
        ),
        _ if calls => refused("the model called a tool", "tool_call"),
        _ => match &answer.error_message {
            Some(error) => refused(error, "error"),
            None => Ok(text.join("\n")),
        },
    };
    tc.event(if want.is_ok() { "taken" } else { "refused" });
    assert_eq!(answer_text(&answer), want);
}
