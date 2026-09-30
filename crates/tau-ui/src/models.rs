//! Choosing models: which ones the picker offers (the ChatGPT account's
//! own), the reasoning effort a run asks for, and the defaults the user
//! set.
//!
//! A run's model and effort hold while it works, so the session can keep
//! sending only what is new. A chat's next message can go to another
//! model: that request sends the whole conversation, and the new model
//! takes it from there.

use serde::{Deserialize, Serialize};
use tau_ai::responses::request::ReasoningEffort;

/// The reasoning effort a run asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    /// The model's default, or what a reasoning plugin picks.
    Auto,
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl Effort {
    pub fn label(self) -> &'static str {
        match self.reasoning() {
            Some(effort) => effort.as_str(),
            None => "auto",
        }
    }

    /// The effort to send, or `None` to leave it to the model.
    pub fn reasoning(self) -> Option<ReasoningEffort> {
        match self {
            Self::Auto => None,
            Self::None => Some(ReasoningEffort::None),
            Self::Minimal => Some(ReasoningEffort::Minimal),
            Self::Low => Some(ReasoningEffort::Low),
            Self::Medium => Some(ReasoningEffort::Medium),
            Self::High => Some(ReasoningEffort::High),
            Self::Xhigh => Some(ReasoningEffort::Xhigh),
            Self::Max => Some(ReasoningEffort::Max),
        }
    }

    pub fn of(effort: ReasoningEffort) -> Self {
        match effort {
            ReasoningEffort::None => Self::None,
            ReasoningEffort::Minimal => Self::Minimal,
            ReasoningEffort::Low => Self::Low,
            ReasoningEffort::Medium => Self::Medium,
            ReasoningEffort::High => Self::High,
            ReasoningEffort::Xhigh => Self::Xhigh,
            ReasoningEffort::Max => Self::Max,
        }
    }

    /// What `model` can run at: auto, then the efforts it takes, lowest
    /// first. Only auto when it does not reason.
    pub fn offered(model: &str) -> Vec<Self> {
        std::iter::once(Self::Auto)
            .chain(tau_ai::model::efforts(model).into_iter().map(Self::of))
            .collect()
    }

    /// The effort named `label`, as [`Self::label`] spells it.
    pub fn parse(label: &str) -> Option<Self> {
        match label {
            "auto" => Some(Self::Auto),
            _ => ReasoningEffort::parse(label).map(Self::of),
        }
    }
}

/// A model and the effort to run it at.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelChoice {
    pub model: String,
    pub effort: Effort,
}

impl ModelChoice {
    pub fn new(model: impl Into<String>, effort: Effort) -> Self {
        Self {
            model: model.into(),
            effort,
        }
    }

    /// The same choice, at auto when the model does not take its
    /// effort: a switch of model, or a saved default, can leave one the
    /// API would reject.
    pub fn fitted(self) -> Self {
        if Effort::offered(&self.model).contains(&self.effort) {
            self
        } else {
            Self::new(self.model, Effort::Auto)
        }
    }

    /// `gpt-6.1-sol · auto`.
    pub fn label(&self) -> String {
        format!("{} · {}", self.model, self.effort.label())
    }
}

impl Default for ModelChoice {
    fn default() -> Self {
        Self::new(DEFAULT_MODEL, Effort::Auto)
    }
}

/// What agents run on when nothing else is chosen: the first of the
/// plan's models, [`tau_ai::model::plan_models`].
pub const DEFAULT_MODEL: &str = "gpt-6.1-sol";

/// One model the picker can offer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelOption {
    pub id: String,
    /// What the picker shows: the model table's name for it; `None`
    /// shows the id.
    pub name: Option<String>,
    pub context: u64,
    /// Whether it accepts a reasoning effort.
    pub reasoning: bool,
}

impl ModelOption {
    /// What the picker shows for it.
    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }

    /// `1.05M`, `400k`.
    pub fn context_label(&self) -> String {
        if self.context >= 1_000_000 {
            let millions = self.context as f64 / 1e6;
            let text = format!("{millions:.2}");
            format!("{}M", text.trim_end_matches('0').trim_end_matches('.'))
        } else {
            format!("{}k", self.context / 1000)
        }
    }
}

/// What the user decided about models. Saved by the host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelSettings {
    /// Each agent's model when a run does not pick one, by agent name.
    pub defaults: Vec<(String, ModelChoice)>,
    /// Models the picker leaves out.
    pub hidden: Vec<String>,
    /// The user read the note on using their ChatGPT plan, shown once
    /// after the first sign-in that allows it.
    pub plan_notice_seen: bool,
}

impl Default for ModelSettings {
    fn default() -> Self {
        Self {
            defaults: vec![("coder".into(), ModelChoice::default())],
            hidden: Vec::new(),
            plan_notice_seen: false,
        }
    }
}

impl ModelSettings {
    /// `agent`'s default, or [`ModelChoice::default`]. A saved model
    /// the plan no longer offers falls back to the first it does.
    pub fn default_for(&self, agent: &str) -> ModelChoice {
        let saved = self
            .defaults
            .iter()
            .find(|(name, _)| name == agent)
            .map_or_else(ModelChoice::default, |(_, choice)| choice.clone());
        let offered = tau_ai::model::plan_models();
        if offered.iter().any(|model| model.id == saved.model) {
            return saved;
        }
        match offered.first() {
            Some(first) => ModelChoice {
                model: first.id.clone(),
                ..saved
            }
            .fitted(),
            None => saved,
        }
    }

    pub fn set_default(&mut self, agent: &str, choice: ModelChoice) {
        match self.defaults.iter_mut().find(|(name, _)| name == agent) {
            Some((_, current)) => *current = choice,
            None => self.defaults.push((agent.to_owned(), choice)),
        }
    }

    pub fn is_hidden(&self, id: &str) -> bool {
        self.hidden.iter().any(|hidden| hidden == id)
    }

    pub fn toggle_hidden(&mut self, id: &str) {
        match self.hidden.iter().position(|hidden| hidden == id) {
            Some(at) => {
                self.hidden.remove(at);
            }
            None => self.hidden.push(id.to_owned()),
        }
    }
}

/// Where ChatGPT users review and limit what apps use of their plan
/// (ChatGPT Settings → Usage).
pub const USAGE_SETTINGS_URL: &str = tau_ai::chatgpt::USAGE_SETTINGS_URL;

/// What a saved ChatGPT sign-in can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccountState {
    /// Signed in, and the plan may be used.
    Plan,
    /// Signed in, but the user did not allow plan usage: runs cannot use
    /// it until they enable it.
    PlanDisabled,
    /// Signed out, or the sign-in died: sign in again.
    SignedOut,
}

/// A saved ChatGPT sign-in, for the account picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatGptAccount {
    /// Its id in tau's store of sign-ins.
    pub id: String,
    /// Its email, kept distinct from the other accounts' labels.
    pub label: String,
    pub state: AccountState,
    /// The account tau signs in with.
    pub active: bool,
}

/// How the host reaches models, for the picker's footer and the
/// accounts on the Models screen.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessInfo {
    /// What runs use: `ChatGPT plan`, or `signed out`.
    pub label: String,
    /// Runs use the active ChatGPT account's plan.
    pub chatgpt: bool,
    /// A TypeSafe key is saved, so tau-constitution checks runs.
    pub jev: bool,
    /// The saved ChatGPT sign-ins, by label.
    pub accounts: Vec<ChatGptAccount>,
}

impl AccessInfo {
    /// The account tau signs in with, if one is saved.
    pub fn active_account(&self) -> Option<&ChatGptAccount> {
        self.accounts.iter().find(|account| account.active)
    }

    /// Whether the note on using the plan shows: runs use it, and the
    /// user has not dismissed the note yet.
    pub fn shows_plan_notice(&self, settings: &ModelSettings) -> bool {
        self.chatgpt && !settings.plan_notice_seen
    }
}

/// Everything about models the interface shows.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Models {
    pub options: Vec<ModelOption>,
    pub settings: ModelSettings,
    pub access: AccessInfo,
    /// The agents a default can be set for, with what they do.
    pub agents: Vec<(String, String)>,
}

impl Models {
    pub fn find(&self, id: &str) -> Option<&ModelOption> {
        self.options.iter().find(|option| option.id == id)
    }

    /// The models the picker shows for `filter`, hidden ones left out,
    /// in the plan's order.
    pub fn shown<'a>(&'a self, filter: &str) -> Vec<&'a ModelOption> {
        let filter = filter.trim().to_lowercase();
        self.options
            .iter()
            .filter(|option| !self.settings.is_hidden(&option.id))
            .filter(|option| {
                filter.is_empty()
                    || option.id.to_lowercase().contains(&filter)
                    || option.label().to_lowercase().contains(&filter)
            })
            .collect()
    }
}

/// The picker's models: the plan's, from tau-ai's model table
/// ([`tau_ai::model::plan_models`]), one per family, in its order and
/// named as it names them.
///
/// Not the account's `GET /v1/models`: that catalog leaves out models
/// the plan runs.
pub fn plan_models() -> Vec<ModelOption> {
    tau_ai::model::plan_models()
        .into_iter()
        .map(|model| ModelOption {
            id: model.id.clone(),
            name: Some(model.name.clone()),
            context: model.context_window,
            reasoning: model.reasoning,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn option(id: &str) -> ModelOption {
        ModelOption {
            id: id.into(),
            name: None,
            context: 1_050_000,
            reasoning: true,
        }
    }

    #[test]
    fn labels_read_as_the_design_writes_them() {
        let model = ModelOption {
            context: 400_000,
            ..option("m")
        };
        assert_eq!(model.context_label(), "400k");
        assert_eq!(option("n").context_label(), "1.05M");
        assert_eq!(ModelChoice::default().label(), "gpt-6.1-sol · auto");
    }

    #[test]
    fn the_picker_hides_and_filters() {
        let mut models = Models {
            options: vec![option("pro"), option("mid"), option("cheap")],
            ..Models::default()
        };
        let ids = |models: &Models, filter: &str| -> Vec<String> {
            models.shown(filter).iter().map(|o| o.id.clone()).collect()
        };
        assert_eq!(ids(&models, ""), ["pro", "mid", "cheap"]);
        assert_eq!(ids(&models, "PR"), ["pro"]);
        models.settings.toggle_hidden("pro");
        assert_eq!(ids(&models, ""), ["mid", "cheap"]);
    }

    #[test]
    fn defaults_are_per_agent() {
        let mut settings = ModelSettings::default();
        settings.set_default(
            "reviewer",
            ModelChoice::new("gpt-6-luna", Effort::Low),
        );
        assert_eq!(settings.default_for("reviewer").model, "gpt-6-luna");
        assert_eq!(settings.default_for("nobody"), ModelChoice::default());
        let text = serde_json::to_string(&settings).unwrap();
        let back: ModelSettings = serde_json::from_str(&text).unwrap();
        assert_eq!(back, settings);
        // A file from before a field existed still reads.
        let old: ModelSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(old, ModelSettings::default());
    }

    /// The picker offers the plan's four, from the model table, by
    /// the table's names; the default is the first of them.
    #[test]
    fn plan_models_come_from_the_table() {
        let options = plan_models();
        let labels: Vec<&str> = options.iter().map(|o| o.label()).collect();
        assert_eq!(
            labels,
            ["GPT-6.1 Sol", "GPT-6 Luna", "GPT-6 Astra", "GPT-5.6 Terra"]
        );
        assert_eq!(options[0].id, DEFAULT_MODEL);
        assert!(options.iter().all(|o| o.context > 0));
        let models = Models {
            options,
            ..Models::default()
        };
        assert_eq!(models.shown("astra").len(), 1, "names are searched");
    }

    /// A saved default the plan no longer offers falls back to the first
    /// model it does; one it offers stays.
    #[test]
    fn a_default_off_the_plan_falls_back_to_the_first() {
        let mut settings = ModelSettings::default();
        settings
            .set_default("coder", ModelChoice::new("gpt-5.5", Effort::Auto));
        assert_eq!(settings.default_for("coder").model, DEFAULT_MODEL);
        settings
            .set_default("coder", ModelChoice::new("gpt-6-luna", Effort::Auto));
        assert_eq!(settings.default_for("coder").model, "gpt-6-luna");
    }

    /// The note on using the plan shows while runs use it, until it is
    /// dismissed.
    #[test]
    fn the_plan_notice_shows_once() {
        let mut settings = ModelSettings::default();
        let mut access = AccessInfo::default();
        assert!(!access.shows_plan_notice(&settings), "not on the plan");
        access.chatgpt = true;
        assert!(access.shows_plan_notice(&settings));
        settings.plan_notice_seen = true;
        assert!(!access.shows_plan_notice(&settings));
        let text = serde_json::to_string(&settings).unwrap();
        let back: ModelSettings = serde_json::from_str(&text).unwrap();
        assert!(back.plan_notice_seen, "saved with the settings");
    }
}
