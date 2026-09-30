//! Choosing models: which ones the picker offers and at what price, the
//! reasoning effort a run asks for, and the defaults the user set.
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

    fn of(effort: ReasoningEffort) -> Self {
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

    /// `gpt-5.5 · auto`.
    pub fn label(&self) -> String {
        format!("{} · {}", self.model, self.effort.label())
    }
}

impl Default for ModelChoice {
    fn default() -> Self {
        Self::new(DEFAULT_MODEL, Effort::Auto)
    }
}

/// What agents run on when nothing else is chosen.
pub const DEFAULT_MODEL: &str = "gpt-5.5";

/// How the picker groups models, by what they cost to write with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Frontier,
    Balanced,
    Fast,
}

impl Tier {
    pub const ALL: [Self; 3] = [Self::Frontier, Self::Balanced, Self::Fast];

    pub fn label(self) -> &'static str {
        match self {
            Self::Frontier => "Frontier",
            Self::Balanced => "Balanced",
            Self::Fast => "Fast and cheap",
        }
    }
}

/// One model the picker can offer.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelOption {
    pub id: String,
    /// What the picker shows: the ChatGPT account's `display_name` on
    /// the plan; `None` shows the id.
    pub name: Option<String>,
    pub context: u64,
    /// USD per million input tokens.
    pub input: f64,
    /// USD per million output tokens.
    pub output: f64,
    /// Whether it accepts a reasoning effort.
    pub reasoning: bool,
    /// Whether the current sign-in can run it.
    pub available: bool,
}

impl ModelOption {
    /// What the picker shows for it.
    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }

    pub fn tier(&self) -> Tier {
        if self.output >= 40. {
            Tier::Frontier
        } else if self.output <= 5. {
            Tier::Fast
        } else {
            Tier::Balanced
        }
    }

    /// `$5 / $30`: input and output, per million tokens.
    pub fn price(&self) -> String {
        format!("{} / {}", dollars(self.input), dollars(self.output))
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

/// `$5`, `$1.75`, `$0.10`: whole dollars bare, else two decimals.
fn dollars(amount: f64) -> String {
    if amount.fract() == 0. {
        format!("${amount:.0}")
    } else {
        format!("${amount:.2}")
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
    /// Ask before a model whose output costs more than this, in USD per
    /// million tokens; never when `None`.
    pub ask_above: Option<f64>,
}

impl Default for ModelSettings {
    fn default() -> Self {
        Self {
            defaults: vec![("coder".into(), ModelChoice::default())],
            // Older generations, kept out of the way until wanted.
            hidden: [
                "gpt-5",
                "gpt-5-mini",
                "gpt-5-nano",
                "gpt-5-pro",
                "gpt-5.1",
                "gpt-5.2",
                "gpt-5.2-pro",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            plan_notice_seen: false,
            ask_above: Some(20.),
        }
    }
}

/// The steps the price threshold moves through, in USD per million
/// output tokens.
pub const ASK_ABOVE_STEPS: [Option<f64>; 6] =
    [Some(5.), Some(10.), Some(20.), Some(50.), Some(100.), None];

impl ModelSettings {
    /// `agent`'s default, or [`ModelChoice::default`].
    pub fn default_for(&self, agent: &str) -> ModelChoice {
        self.defaults
            .iter()
            .find(|(name, _)| name == agent)
            .map_or_else(ModelChoice::default, |(_, choice)| choice.clone())
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

    /// Moves the price threshold one step up or down.
    pub fn step_ask_above(&mut self, delta: i32) {
        let at = ASK_ABOVE_STEPS
            .iter()
            .position(|step| *step == self.ask_above)
            .unwrap_or(2) as i32;
        let at = (at + delta).clamp(0, ASK_ABOVE_STEPS.len() as i32 - 1);
        self.ask_above = ASK_ABOVE_STEPS[at as usize];
    }
}

/// A way to reach models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccessKind {
    /// A ChatGPT sign-in, using the plan.
    ChatGpt,
    /// An OpenAI API key.
    ApiKey,
}

/// Where ChatGPT users review and limit what apps use of their plan
/// (ChatGPT Settings → Usage).
pub const USAGE_SETTINGS_URL: &str = tau_ai::chatgpt::USAGE_SETTINGS_URL;

/// What a saved ChatGPT sign-in can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountState {
    /// Signed in, and the plan may be used.
    Plan,
    /// Signed in, but the user did not allow plan usage: runs cannot use
    /// it until they enable it, or add an API key.
    PlanDisabled,
    /// Signed out, or the sign-in died: sign in again.
    SignedOut,
}

/// A saved ChatGPT sign-in, for the account picker.
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccessInfo {
    /// What runs use: `ChatGPT plan`, `OpenAI API key`, or empty.
    pub label: String,
    /// Runs use the active ChatGPT account's plan.
    pub chatgpt: bool,
    /// Runs go through the API key.
    pub api_key: bool,
    /// What is saved, whether runs use it or not.
    pub saved: Vec<AccessKind>,
    /// A TypeSafe key is saved, so tau-constitution checks runs.
    pub jev: bool,
    /// The saved ChatGPT sign-ins, by label.
    pub accounts: Vec<ChatGptAccount>,
    /// Why the plan's models could not be listed, when they could not.
    pub models_error: Option<String>,
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
#[derive(Debug, Clone, Default, PartialEq)]
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

    /// Whether runs use the ChatGPT plan: the picker lists the
    /// account's models in the server's order, and nothing is priced
    /// per token.
    pub fn on_plan(&self) -> bool {
        self.access.chatgpt
    }

    /// The models the picker shows for `filter`, hidden ones left out:
    /// in tier order, or in the server's order on the plan.
    pub fn shown<'a>(&'a self, filter: &str) -> Vec<&'a ModelOption> {
        let filter = filter.trim().to_lowercase();
        let mut shown: Vec<&ModelOption> = self
            .options
            .iter()
            .filter(|option| !self.settings.is_hidden(&option.id))
            .filter(|option| {
                filter.is_empty()
                    || option.id.to_lowercase().contains(&filter)
                    || option.label().to_lowercase().contains(&filter)
            })
            .collect();
        if !self.on_plan() {
            shown.sort_by_key(|option| option.tier());
        }
        shown
    }

    /// Whether choosing `id` asks first, for its price. Never on the
    /// plan: it is not billed per token.
    pub fn needs_confirm(&self, id: &str) -> bool {
        if self.on_plan() {
            return false;
        }
        match (self.settings.ask_above, self.find(id)) {
            (Some(limit), Some(option)) => option.output > limit,
            _ => false,
        }
    }
}

/// The picker's models on the ChatGPT plan: the account's, in the
/// server's order, named as it names them, with the context and prices
/// tau-ai's table knows for them.
pub fn plan_models(listed: &[tau_ai::chatgpt::ModelInfo]) -> Vec<ModelOption> {
    listed
        .iter()
        .map(|info| {
            let known = tau_ai::model::find(&info.slug);
            ModelOption {
                id: info.slug.clone(),
                name: Some(info.display_name.clone())
                    .filter(|name| *name != info.slug),
                context: known.map_or(0, |model| model.context_window),
                input: known.map_or(0., |model| model.pricing.input),
                output: known.map_or(0., |model| model.pricing.output),
                reasoning: known.is_some_and(|model| model.reasoning),
                available: true,
            }
        })
        .collect()
}

/// The models worth offering for coding, from tau-ai's table: reasoning
/// models of the current generations with a large window, without chat,
/// realtime or image variants. `available` says which the sign-in can
/// run.
pub fn coding_models(available: impl Fn(&str) -> bool) -> Vec<ModelOption> {
    let mut options: Vec<ModelOption> = tau_ai::model::models()
        .iter()
        .filter(|model| model.reasoning && model.context_window >= 200_000)
        .filter(|model| {
            (model.id.starts_with("gpt-5") || model.id.starts_with("gpt-6"))
                && !["chat-latest", "realtime", "image"]
                    .iter()
                    .any(|variant| model.id.contains(variant))
        })
        .map(|model| ModelOption {
            id: model.id.clone(),
            name: None,
            context: model.context_window,
            input: model.pricing.input,
            output: model.pricing.output,
            reasoning: model.reasoning,
            available: available(&model.id),
        })
        .collect();
    // Newest first within a tier: ids sort by generation.
    options.sort_by(|a, b| b.id.cmp(&a.id));
    options
}

#[cfg(test)]
mod tests {
    use super::*;

    fn option(id: &str, output: f64) -> ModelOption {
        ModelOption {
            id: id.into(),
            name: None,
            context: 1_050_000,
            input: 1.,
            output,
            reasoning: true,
            available: true,
        }
    }

    #[test]
    fn tiers_follow_the_output_price() {
        assert_eq!(option("a", 180.).tier(), Tier::Frontier);
        assert_eq!(option("b", 30.).tier(), Tier::Balanced);
        assert_eq!(option("c", 1.2).tier(), Tier::Fast);
    }

    #[test]
    fn labels_read_as_the_design_writes_them() {
        let model = ModelOption {
            input: 1.75,
            output: 14.,
            context: 400_000,
            ..option("m", 14.)
        };
        assert_eq!(model.price(), "$1.75 / $14");
        assert_eq!(model.context_label(), "400k");
        assert_eq!(option("n", 1.).context_label(), "1.05M");
        assert_eq!(ModelChoice::default().label(), "gpt-5.5 · auto");
    }

    #[test]
    fn the_picker_hides_and_filters_and_asks_about_price() {
        let mut models = Models {
            options: vec![
                option("cheap", 1.),
                option("pro", 180.),
                option("mid", 30.),
            ],
            ..Models::default()
        };
        let ids = |models: &Models, filter: &str| -> Vec<String> {
            models.shown(filter).iter().map(|o| o.id.clone()).collect()
        };
        assert_eq!(ids(&models, ""), ["pro", "mid", "cheap"]);
        assert_eq!(ids(&models, "PR"), ["pro"]);
        models.settings.toggle_hidden("pro");
        assert_eq!(ids(&models, ""), ["mid", "cheap"]);
        assert!(models.needs_confirm("mid"));
        assert!(!models.needs_confirm("cheap"));
        models.settings.ask_above = None;
        assert!(!models.needs_confirm("mid"));
    }

    #[test]
    fn the_threshold_steps_and_defaults_are_per_agent() {
        let mut settings = ModelSettings::default();
        settings.step_ask_above(1);
        assert_eq!(settings.ask_above, Some(50.));
        settings.step_ask_above(10);
        assert_eq!(settings.ask_above, None);
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

    #[test]
    fn coding_models_come_from_the_table() {
        let models = coding_models(|id| id == "gpt-5.5");
        assert!(models.iter().any(|m| m.id == "gpt-5.5" && m.available));
        assert!(models.iter().any(|m| m.id == "gpt-6-sol" && !m.available));
        assert!(models.iter().all(|m| !m.id.contains("chat-latest")));
        assert!(models.iter().all(|m| m.context >= 200_000));
    }

    fn listed(slug: &str, name: &str) -> tau_ai::chatgpt::ModelInfo {
        tau_ai::chatgpt::ModelInfo {
            slug: slug.into(),
            display_name: name.into(),
        }
    }

    /// On the plan, the picker keeps the account's models in the
    /// server's order, by their display names, and never asks about
    /// price.
    #[test]
    fn plan_models_keep_the_servers_order_and_names() {
        let options = plan_models(&[
            listed("gpt-6-astra", "GPT-6-Astra"),
            listed("gpt-5.5", "gpt-5.5"),
            listed("gpt-next", "GPT Next"),
        ]);
        let labels: Vec<&str> = options.iter().map(|o| o.label()).collect();
        assert_eq!(labels, ["GPT-6-Astra", "gpt-5.5", "GPT Next"]);
        assert!(options.iter().all(|o| o.available));
        assert_eq!(options[2].context, 0, "unknown to tau-ai's table");
        let mut models = Models {
            options,
            ..Models::default()
        };
        models.access.chatgpt = true;
        let shown: Vec<&str> =
            models.shown("").iter().map(|o| o.id.as_str()).collect();
        assert_eq!(shown, ["gpt-6-astra", "gpt-5.5", "gpt-next"]);
        assert_eq!(models.shown("astra").len(), 1, "names are searched");
        assert!(!models.needs_confirm("gpt-6-astra"));
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
