//! Why OpenAI, or the sign-in, refused a request or a connection, kept as
//! it came: the HTTP status, the error code, the request id and the body.
//!
//! OpenAI's plan-usage docs ask clients to keep all four, and to act on
//! the [`Recovery`] rather than on message text. A refusal whose recovery
//! is not [`Recovery::RetryLater`] stops the run: the transport fails the
//! request at once instead of reconnecting, and the retry policy does not
//! try again ([`Recovery::class`]).

use std::fmt;

use serde_json::Value;

use crate::{
    chatgpt::{ApiError, ChatGptError},
    retry::{Class, Recovery},
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Refusal {
    pub recovery: Recovery,
    /// The HTTP status, when the refusal came as an HTTP response or a
    /// frame that names one.
    pub status: Option<u16>,
    /// The error's `code`, when it had one.
    pub code: Option<String>,
    /// The `x-request-id` of the response, when there was one.
    pub request_id: Option<String>,
    /// The error body or frame, verbatim.
    pub body: String,
    /// What the run reports.
    pub message: String,
}

impl Refusal {
    /// How the retry policy treats it.
    pub fn class(&self) -> Class {
        self.recovery.class()
    }

    /// An HTTP error from `api.openai.com`, such as a refused WebSocket
    /// upgrade.
    pub fn from_api(error: &ApiError) -> Self {
        Self {
            recovery: error.recovery(),
            status: Some(error.status),
            code: error.code().map(str::to_owned),
            request_id: error.request_id.clone(),
            body: error.body.clone(),
            message: format!("OpenAI refused the request: {error}"),
        }
    }

    /// A sign-in that could not give a token for the connection. `None`
    /// for a network failure: nothing was refused, so it stays a plain
    /// transport failure.
    pub fn from_chatgpt(error: &ChatGptError) -> Option<Self> {
        match error {
            ChatGptError::Network(_) => None,
            ChatGptError::Api(api) => Some(Self::from_api(api)),
            other => Some(Self {
                recovery: other.recovery(),
                status: None,
                code: None,
                request_id: None,
                body: String::new(),
                message: other.to_string(),
            }),
        }
    }

    /// A failed response (`response.failed`, or an `error` frame) with a
    /// code OpenAI documents for plan usage; `None` for any other frame.
    /// A usage limit can arrive this way after streaming began. An `error`
    /// frame's details are read nested in `error` or, as the stream reads
    /// them and pi's Codex client does, at the frame's top.
    pub fn from_frame(frame: &Value) -> Option<Self> {
        let flat = (frame.get("type").and_then(Value::as_str) == Some("error"))
            .then_some(frame);
        let error = frame
            .get("response")
            .and_then(|response| response.get("error"))
            .filter(|error| error.is_object())
            .or_else(|| frame.get("error").filter(|error| error.is_object()))
            .or(flat)?;
        let code = error.get("code")?.as_str()?;
        let recovery = Recovery::of_code(code)?;
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("no message");
        Some(Self {
            recovery,
            status: frame
                .get("status")
                .and_then(Value::as_u64)
                .and_then(|status| u16::try_from(status).ok()),
            code: Some(code.to_owned()),
            request_id: None,
            body: frame.to_string(),
            message: format!("{code}: {message}"),
        })
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_usage_limit_mid_stream_is_a_refusal() {
        let frame = json!({
            "type": "response.failed",
            "response": {"error": {
                "code": "subscription_sharing_usage_limit_exceeded",
                "message": "limit",
            }},
        });
        let refusal = Refusal::from_frame(&frame).unwrap();
        assert_eq!(refusal.recovery, Recovery::UsageLimit);
        assert_eq!(refusal.class(), Class::Fatal);
        assert_eq!(
            refusal.code.as_deref(),
            Some("subscription_sharing_usage_limit_exceeded")
        );
        assert!(refusal.body.contains("response.failed"));
        let other = json!({"type": "error", "error": {"code": "server_error"}});
        assert_eq!(Refusal::from_frame(&other), None);
    }

    /// The plan's documented codes and what they ask for, written out
    /// apart from `Recovery::of_code`.
    const PLAN_CODES: [(&str, Recovery); 9] = [
        (
            "subscription_sharing_user_not_eligible",
            Recovery::Restricted,
        ),
        (
            "subscription_sharing_usage_limit_exceeded",
            Recovery::UsageLimit,
        ),
        (
            "subscription_sharing_usage_unavailable",
            Recovery::RetryLater,
        ),
        (
            "subscription_sharing_unsupported_capability",
            Recovery::FixRequest,
        ),
        (
            "subscription_sharing_route_not_supported",
            Recovery::FixRequest,
        ),
        ("subscription_sharing_invalid_user", Recovery::SignInAgain),
        ("chatpass_v2_scope_not_authorized", Recovery::Restricted),
        (
            "chatpass_v2_invalid_authorization_context",
            Recovery::Restricted,
        ),
        (
            "subscription_sharing_user_unavailable",
            Recovery::RetryLater,
        ),
    ];

    /// A plan refusal is read from every shape a failure frame comes in,
    /// as the stream reads its code: a failed response's `error`, an
    /// `error` frame's nested `error`, or an `error` frame with its details
    /// at the top, as pi's Codex client reads them (`extractCodexEventError`
    /// takes `event.code` first). It keeps the code, the frame's status,
    /// and the frame verbatim; any other code is no refusal.
    #[hegel::test(test_cases = 300)]
    fn a_plan_refusal_is_read_from_every_failure_frame_shape(
        tc: hegel::TestCase,
    ) {
        use hegel::generators as gs;
        // An index into the plan's codes, or past it for a code that is
        // not the plan's.
        let at =
            tc.draw(gs::integers::<usize>().max_value(PLAN_CODES.len() + 2));
        let (code, recovery) = match PLAN_CODES.get(at) {
            Some((code, recovery)) => ((*code).to_owned(), Some(*recovery)),
            None => {
                let other = ["server_error", "rate_limit_exceeded", ""];
                (other[at - PLAN_CODES.len()].to_owned(), None)
            }
        };
        let words = tc.draw(gs::optional(gs::text().max_size(12)));
        let status =
            tc.draw(gs::optional(gs::sampled_from(vec![400u16, 429, 503])));
        let mut details = json!({"code": code});
        if let Some(words) = &words {
            details["message"] = json!(words);
        }
        let shape = tc.draw(gs::integers::<u8>().max_value(2));
        let mut frame = match shape {
            0 => {
                json!({"type": "response.failed", "response": {"error": details}})
            }
            1 => json!({"type": "error", "error": details}),
            _ => {
                let mut flat = details.clone();
                flat["type"] = json!("error");
                flat
            }
        };
        if let Some(status) = status
            && shape != 0
        {
            frame["status"] = json!(status);
        }
        let refusal = Refusal::from_frame(&frame);
        let Some(recovery) = recovery else {
            assert_eq!(refusal, None, "{frame}");
            return;
        };
        let refusal =
            refusal.unwrap_or_else(|| panic!("no refusal in {frame}"));
        assert_eq!(refusal.recovery, recovery, "{frame}");
        assert_eq!(refusal.code.as_deref(), Some(code.as_str()));
        assert_eq!(refusal.status, status.filter(|_| shape != 0));
        assert_eq!(refusal.body, frame.to_string());
        assert_eq!(
            refusal.message,
            format!("{code}: {}", words.as_deref().unwrap_or("no message"))
        );
    }

    #[test]
    fn only_refusals_come_from_sign_in_errors() {
        let network = ChatGptError::Network(std::io::Error::other("down"));
        assert_eq!(Refusal::from_chatgpt(&network), None);
        let disabled =
            Refusal::from_chatgpt(&ChatGptError::PlanUsageDisabled).unwrap();
        assert_eq!(disabled.recovery, Recovery::EnablePlanUsage);
        assert_eq!(disabled.class(), Class::Fatal);
    }
}
