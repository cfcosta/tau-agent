//! The OpenAI model table.
//!
//! `tau-agent` only ever talks to the OpenAI Responses API, so this is a
//! table of OpenAI models only, hand-maintained the way pi maintains it:
//! start from [models.dev](https://models.dev)'s catalog (vendored at
//! `data/models-dev-openai.json`, refreshed by
//! `scripts/update-openai-models.sh`), then apply OpenAI-specific
//! corrections pi carries in
//! `packages/ai/scripts/generate-models.ts` (commit `2b0a123`), vendored
//! at `data/openai-overrides.json`. See that file for the exact pi line
//! each correction comes from.
//!
//! Building the table mirrors `generate-models.ts`'s "Process OpenAI
//! models" step (around line 1789) plus its OpenAI-specific correction
//! pass (around line 2803):
//!
//! 1. Keep only models with `tool_call == true` (generate-models.ts:1792).
//! 2. Drop [`excluded ids`](Overrides) (generate-models.ts:1794).
//! 3. Map `cost.{input,output,cache_read,cache_write}` (USD per million
//!    tokens) and `limit.{context,output}` directly, defaulting a zero
//!    `limit.context` or `limit.output` to 4096 (generate-models.ts:1799-1811).
//! 4. Cap `shortContextCappedIds` models to the long-context threshold
//!    and 128,000 max output.
//! 5. Add the long-context pricing tier to `longContextPricingIds`
//!    models, replacing the base rate with the pinned `standardCosts`
//!    entry when one exists.
//! 6. Apply one-off `maxOutputFixes`.
//! 7. Give each reasoning model the efforts it takes, from tau's own
//!    `data/openai-reasoning-efforts.json`.
//!
//! Excluded ids are simply absent from [`models()`].

use std::{collections::HashMap, sync::LazyLock};

use serde::Deserialize;

use crate::responses::request::ReasoningEffort;

/// The vendored models.dev snapshot: `openai` provider models, reduced to
/// the fields this module reads.
const VENDORED_MODELS_JSON: &str =
    include_str!("../data/models-dev-openai.json");

/// pi's OpenAI-specific corrections on top of the models.dev snapshot.
const OVERRIDES_JSON: &str = include_str!("../data/openai-overrides.json");

/// The reasoning efforts each model takes, as probed against the API.
const EFFORTS_JSON: &str =
    include_str!("../data/openai-reasoning-efforts.json");

/// One OpenAI model tau-agent can talk to over the Responses API.
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub id: String,
    pub name: String,
    /// Whether the model accepts a `reasoning` effort.
    pub reasoning: bool,
    /// The efforts it takes, lowest first; empty when it does not
    /// reason. Any other is rejected.
    pub efforts: Vec<ReasoningEffort>,
    /// Whether the model accepts image input.
    pub images: bool,
    pub context_window: u64,
    pub max_output: u64,
    pub pricing: Pricing,
}

/// USD per million tokens, plus an optional long-context tier.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pricing {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    /// The rate that applies once a request's total input tokens
    /// (`input + cache_read + cache_write`) exceed
    /// [`PricingTier::input_tokens_above`]. See [`crate::cost`] for how
    /// the threshold is compared.
    pub long_context: Option<PricingTier>,
}

/// A pricing tier that replaces the base rate above a token threshold.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PricingTier {
    pub input_tokens_above: u64,
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

/// A service tier multiplier on top of the base rate, per pi's
/// `getServiceTierCostMultiplier` (`packages/ai/src/api/openai-responses.ts:367`).
/// Not part of the model table: it depends on the `service_tier` a
/// particular response used, not on the model alone, so [`crate::cost`]
/// takes it as a separate argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ServiceTier {
    #[default]
    Default,
    Flex,
    /// OpenAI's Responses API calls this tier `priority`; the Codex API
    /// calls the same multiplier `fast`. Both map to the same number.
    PriorityOrFast,
}

impl ServiceTier {
    /// The cost multiplier for this tier, given the model it applies to.
    /// Mirrors pi's `getServiceTierCostMultiplier`
    /// (`packages/ai/src/api/openai-responses.ts:367-378`).
    pub(crate) fn multiplier(self, model_id: &str) -> f64 {
        let overrides = &overrides().service_tier_multipliers;
        match self {
            Self::Default => overrides.default,
            Self::Flex => overrides.flex,
            Self::PriorityOrFast => *overrides
                .priority_or_fast_overrides
                .by_id
                .get(model_id)
                .unwrap_or(&overrides.priority_or_fast),
        }
    }
}

// --- Raw shapes for the vendored JSON. ---

#[derive(Debug, Deserialize)]
struct VendoredCatalog {
    models: HashMap<String, VendoredModel>,
}

#[derive(Debug, Deserialize)]
struct VendoredModel {
    name: String,
    reasoning: bool,
    tool_call: bool,
    modalities: VendoredModalities,
    cost: RawCost,
    limit: VendoredLimit,
}

#[derive(Debug, Deserialize)]
struct VendoredModalities {
    input: Vec<String>,
}

#[derive(Debug, Deserialize, Clone, Copy)]
struct RawCost {
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write: f64,
}

#[derive(Debug, Deserialize)]
struct VendoredLimit {
    context: u64,
    output: u64,
}

// --- Raw shapes for the overrides JSON. ---

#[derive(Debug, Deserialize)]
struct Overrides {
    #[serde(rename = "excludedIds")]
    excluded_ids: IdList,
    #[serde(rename = "longContextInputThreshold")]
    long_context_input_threshold: Threshold,
    #[serde(rename = "shortContextCappedIds")]
    short_context_capped_ids: ShortContextCap,
    #[serde(rename = "longContextPricingIds")]
    long_context_pricing_ids: LongContextPricing,
    #[serde(rename = "standardCosts")]
    standard_costs: StandardCosts,
    #[serde(rename = "maxOutputFixes")]
    max_output_fixes: MaxOutputFixes,
    #[serde(rename = "serviceTierMultipliers")]
    service_tier_multipliers: ServiceTierMultipliers,
}

#[derive(Debug, Deserialize)]
struct IdList {
    ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Threshold {
    tokens: u64,
}

#[derive(Debug, Deserialize)]
struct ShortContextCap {
    ids: Vec<String>,
    #[serde(rename = "contextWindow")]
    context_window: u64,
    #[serde(rename = "maxOutput")]
    max_output: u64,
}

#[derive(Debug, Deserialize)]
struct LongContextPricing {
    ids: Vec<String>,
    #[serde(rename = "tierInputMultiplier")]
    tier_input_multiplier: f64,
    #[serde(rename = "tierOutputMultiplier")]
    tier_output_multiplier: f64,
    #[serde(rename = "tierCacheReadMultiplier")]
    tier_cache_read_multiplier: f64,
    #[serde(rename = "tierCacheWriteMultiplier")]
    tier_cache_write_multiplier: f64,
}

#[derive(Debug, Deserialize)]
struct StandardCosts {
    costs: HashMap<String, StandardCost>,
}

#[derive(Debug, Deserialize, Clone, Copy)]
struct StandardCost {
    input: f64,
    output: f64,
    #[serde(rename = "cacheRead")]
    cache_read: f64,
    #[serde(rename = "cacheWrite")]
    cache_write: f64,
}

#[derive(Debug, Deserialize)]
struct MaxOutputFixes {
    fixes: HashMap<String, u64>,
}

#[derive(Debug, Deserialize)]
struct ServiceTierMultipliers {
    default: f64,
    flex: f64,
    #[serde(rename = "priorityOrFast")]
    priority_or_fast: f64,
    #[serde(rename = "priorityOrFastOverrides")]
    priority_or_fast_overrides: PriorityOrFastOverrides,
}

#[derive(Debug, Deserialize)]
struct PriorityOrFastOverrides {
    #[serde(rename = "byId")]
    by_id: HashMap<String, f64>,
}

/// Rounds like pi's `roundCost` (generate-models.ts:1228-1230): to 6
/// decimal places.
fn round_cost(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

fn overrides() -> &'static Overrides {
    static OVERRIDES: LazyLock<Overrides> = LazyLock::new(|| {
        serde_json::from_str(OVERRIDES_JSON)
            .expect("crates/tau-ai/data/openai-overrides.json is malformed")
    });
    &OVERRIDES
}

#[derive(Debug, Deserialize)]
struct Efforts {
    fallback: Vec<String>,
    models: HashMap<String, Vec<String>>,
}

fn efforts_of(id: &str, reasoning: bool) -> Vec<ReasoningEffort> {
    static EFFORTS: LazyLock<Efforts> = LazyLock::new(|| {
        serde_json::from_str(EFFORTS_JSON).expect(
            "crates/tau-ai/data/openai-reasoning-efforts.json is malformed",
        )
    });
    if !reasoning {
        return Vec::new();
    }
    EFFORTS
        .models
        .get(id)
        .unwrap_or(&EFFORTS.fallback)
        .iter()
        .map(|name| {
            ReasoningEffort::parse(name)
                .unwrap_or_else(|| panic!("unknown effort {name} for {id}"))
        })
        .collect()
}

fn build_models() -> Vec<Model> {
    let catalog: VendoredCatalog = serde_json::from_str(VENDORED_MODELS_JSON)
        .expect("crates/tau-ai/data/models-dev-openai.json is malformed");
    let overrides = overrides();

    let mut models: Vec<Model> = catalog
        .models
        .into_iter()
        // generate-models.ts:1792: `if (m.tool_call !== true) continue;`
        .filter(|(_, raw)| raw.tool_call)
        // generate-models.ts:1794: MODELS_DEV_OPENAI_UNSUPPORTED_MODEL_IDS.
        .filter(|(id, _)| !overrides.excluded_ids.ids.contains(id))
        .map(|(id, raw)| build_model(id, raw, overrides))
        .collect();

    // "models sorted by id", matching the vendored file's own order.
    models.sort_by(|a, b| a.id.cmp(&b.id));
    models
}

fn build_model(id: String, raw: VendoredModel, overrides: &Overrides) -> Model {
    // generate-models.ts:1799-1811: the base mapping.
    let images = raw.modalities.input.iter().any(|m| m == "image");
    let mut context_window = if raw.limit.context == 0 {
        4096
    } else {
        raw.limit.context
    };
    let mut max_output = if raw.limit.output == 0 {
        4096
    } else {
        raw.limit.output
    };
    let mut base_cost = raw.cost;

    // generate-models.ts:2807-2810: OPENAI_SHORT_CONTEXT_CAPPED_MODEL_IDS.
    if overrides.short_context_capped_ids.ids.contains(&id) {
        context_window = overrides.short_context_capped_ids.context_window;
        max_output = overrides.short_context_capped_ids.max_output;
    }

    // generate-models.ts:2811-2814: OPENAI_LONG_CONTEXT_PRICING_MODEL_IDS.
    let mut long_context = None;
    if overrides.long_context_pricing_ids.ids.contains(&id) {
        if let Some(standard) = overrides.standard_costs.costs.get(&id) {
            base_cost = RawCost {
                input: standard.input,
                output: standard.output,
                cache_read: standard.cache_read,
                cache_write: standard.cache_write,
            };
        }
        let lc = &overrides.long_context_pricing_ids;
        long_context = Some(PricingTier {
            input_tokens_above: overrides.long_context_input_threshold.tokens,
            input: round_cost(base_cost.input * lc.tier_input_multiplier),
            output: round_cost(base_cost.output * lc.tier_output_multiplier),
            cache_read: round_cost(
                base_cost.cache_read * lc.tier_cache_read_multiplier,
            ),
            cache_write: round_cost(
                base_cost.cache_write * lc.tier_cache_write_multiplier,
            ),
        });
    }

    // generate-models.ts:2820-2824: the gpt-5-pro maxTokens fix (and any
    // other one-off max-output correction).
    if let Some(&fixed) = overrides.max_output_fixes.fixes.get(&id) {
        max_output = fixed;
    }

    Model {
        efforts: efforts_of(&id, raw.reasoning),
        id,
        name: raw.name,
        reasoning: raw.reasoning,
        images,
        context_window,
        max_output,
        pricing: Pricing {
            input: base_cost.input,
            output: base_cost.output,
            cache_read: base_cost.cache_read,
            cache_write: base_cost.cache_write,
            long_context,
        },
    }
}

static MODELS: LazyLock<Vec<Model>> = LazyLock::new(build_models);

/// Every OpenAI model tau-agent knows about, sorted by id. Excluded ids
/// (see `data/openai-overrides.json`) are absent.
pub fn models() -> &'static [Model] {
    &MODELS
}

/// The efforts `id` takes, lowest first: the table's, or for a model
/// it does not know, the efforts every reasoning model takes.
pub fn efforts(id: &str) -> Vec<ReasoningEffort> {
    find(id).map_or_else(|| efforts_of(id, true), |model| model.efforts.clone())
}

/// Looks up a model by id.
pub fn find(id: &str) -> Option<&'static Model> {
    MODELS.iter().find(|m| m.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// pi falls back to 4096 when models.dev reports no context or output
    /// limit (`generate-models.ts:1809-1810`). No vendored model lacks
    /// them today, so this pins the fallback directly.
    #[test]
    fn missing_limits_default_to_4096() {
        let raw = VendoredModel {
            name: "Test".into(),
            reasoning: false,
            tool_call: true,
            modalities: VendoredModalities { input: vec![] },
            cost: RawCost {
                input: 1.0,
                output: 2.0,
                cache_read: 0.1,
                cache_write: 0.0,
            },
            limit: VendoredLimit {
                context: 0,
                output: 0,
            },
        };
        let model = build_model("test-model".into(), raw, overrides());
        assert_eq!((model.context_window, model.max_output), (4096, 4096));
    }
}
