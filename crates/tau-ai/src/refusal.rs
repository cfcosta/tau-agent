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

#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// A usage limit can arrive this way after streaming began.
    pub fn from_frame(frame: &Value) -> Option<Self> {
        let error = frame
            .get("response")
            .and_then(|response| response.get("error"))
            .filter(|error| error.is_object())
            .or_else(|| frame.get("error").filter(|error| error.is_object()))?;
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

    #[test]
    fn a_refused_upgrade_keeps_status_code_and_request_id() {
        let body = br#"{"error":{"code":"subscription_sharing_usage_limit_exceeded","message":"m"}}"#;
        let error = ApiError::new(429, Some("req_1".into()), body);
        let refusal = Refusal::from_api(&error);
        assert_eq!(refusal.recovery, Recovery::UsageLimit);
        assert_eq!(refusal.status, Some(429));
        assert_eq!(refusal.request_id.as_deref(), Some("req_1"));
        assert!(refusal.message.contains("req_1"));
        let busy = ApiError::new(503, None, br#"{"detail":"later"}"#);
        assert_eq!(Refusal::from_api(&busy).class(), Class::Retryable);
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
