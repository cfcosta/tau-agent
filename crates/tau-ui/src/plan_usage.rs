//! What the interface says about using a ChatGPT plan, in the words
//! OpenAI's Sign in with ChatGPT guidelines ask for: the note shown once
//! after the first sign-in that allows it, the line by the composer while
//! runs use the plan, and what a run that stopped on the plan says next.
//!
//! A plan error stops the run; the plan is the only way tau reaches a
//! model. The alert says what to do instead: manage usage, sign in
//! again, enable plan use, or read why it stopped.

use serde::{Deserialize, Serialize};
use tau_ai::{refusal::Refusal, retry::Recovery};

/// The note shown once after the first sign-in with plan usage.
pub const NOTICE_TITLE: &str = "You're using your ChatGPT plan";
pub const NOTICE_BODY: &str = "Eligible usage in this app uses your ChatGPT \
                               plan. Manage usage in your ChatGPT settings.";

/// The line by the composer while runs use the plan.
pub const INDICATOR: &str = "Using ChatGPT plan";

/// The link to ChatGPT Settings → Usage, wherever it shows.
pub const MANAGE_USAGE: &str = "Manage usage";

/// What a run that stopped on the plan asks of the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlanAlert {
    /// The plan's usage limit, or this app's.
    UsageLimit,
    /// OpenAI no longer takes the sign-in.
    SignInAgain { detail: String },
    /// The sign-in does not allow plan usage.
    EnablePlanUsage,
    /// A restriction, or a request or client OpenAI refused: retrying
    /// will not help.
    Stopped { detail: String },
}

/// What an alert's buttons do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlanAction {
    /// Opens ChatGPT Settings → Usage.
    ManageUsage,
    /// Signs the active account in again.
    SignInAgain,
    /// Signs the active account in again, asking for plan usage.
    EnablePlanUsage,
    /// Closes the alert.
    Close,
}

impl PlanAlert {
    /// The alert for `refusal`; `None` for a temporary one, which the
    /// run's retries already went through.
    pub fn of(refusal: &Refusal) -> Option<Self> {
        let detail = refusal.message.clone();
        Some(match refusal.recovery {
            Recovery::RetryLater => return None,
            Recovery::UsageLimit => Self::UsageLimit,
            Recovery::SignInAgain => Self::SignInAgain { detail },
            Recovery::EnablePlanUsage => Self::EnablePlanUsage,
            Recovery::Restricted
            | Recovery::FixRequest
            | Recovery::FixClient => Self::Stopped { detail },
        })
    }

    pub fn title(&self) -> &'static str {
        match self {
            Self::UsageLimit => "Usage limit reached",
            Self::SignInAgain { .. } => "Sign in to ChatGPT again",
            Self::EnablePlanUsage => "ChatGPT plan use isn't enabled",
            Self::Stopped { .. } => "ChatGPT plan use stopped",
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::UsageLimit => "Review your plan or this app's limit in \
                                 ChatGPT settings."
                .into(),
            Self::SignInAgain { detail } => format!(
                "OpenAI no longer accepts this sign-in. Sign in again to \
                 keep using your plan. ({detail})"
            ),
            Self::EnablePlanUsage => "This sign-in does not allow tau to use \
                                      your ChatGPT plan. Enable plan use to \
                                      keep going."
                .into(),
            Self::Stopped { detail } => detail.clone(),
        }
    }

    /// The primary button, when there is something to do.
    pub fn primary(&self) -> Option<(&'static str, PlanAction)> {
        match self {
            Self::UsageLimit => Some((MANAGE_USAGE, PlanAction::ManageUsage)),
            Self::SignInAgain { .. } => {
                Some(("Sign in again", PlanAction::SignInAgain))
            }
            Self::EnablePlanUsage => {
                Some(("Enable ChatGPT plan use", PlanAction::EnablePlanUsage))
            }
            Self::Stopped { .. } => None,
        }
    }

    /// The other button.
    pub fn secondary(&self) -> (&'static str, PlanAction) {
        ("Close", PlanAction::Close)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refusal(recovery: Recovery) -> Refusal {
        Refusal {
            recovery,
            status: Some(429),
            code: Some("code".into()),
            request_id: Some("req_1".into()),
            body: String::new(),
            message: "HTTP 429 code (request req_1)".into(),
        }
    }

    #[test]
    fn a_usage_limit_offers_to_manage_usage() {
        let alert = PlanAlert::of(&refusal(Recovery::UsageLimit)).unwrap();
        assert_eq!(alert.title(), "Usage limit reached");
        assert_eq!(
            alert.message(),
            "Review your plan or this app's limit in ChatGPT settings."
        );
        assert_eq!(
            alert.primary(),
            Some(("Manage usage", PlanAction::ManageUsage))
        );
    }

    #[test]
    fn every_lasting_refusal_has_an_alert_and_retries_have_none() {
        assert_eq!(PlanAlert::of(&refusal(Recovery::RetryLater)), None);
        let again = PlanAlert::of(&refusal(Recovery::SignInAgain)).unwrap();
        assert_eq!(again.primary().unwrap().1, PlanAction::SignInAgain);
        assert!(again.message().contains("req_1"), "the request id stays");
        let enable =
            PlanAlert::of(&refusal(Recovery::EnablePlanUsage)).unwrap();
        assert_eq!(enable.primary().unwrap().1, PlanAction::EnablePlanUsage);
        assert_eq!(enable.secondary().1, PlanAction::Close);
        assert!(!enable.message().contains("API key"));
        for recovery in [
            Recovery::Restricted,
            Recovery::FixRequest,
            Recovery::FixClient,
        ] {
            let stopped = PlanAlert::of(&refusal(recovery)).unwrap();
            assert_eq!(stopped.primary(), None);
            assert_eq!(stopped.message(), "HTTP 429 code (request req_1)");
        }
    }
}
