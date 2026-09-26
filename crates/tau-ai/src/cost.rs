//! Cost from usage, ported from pi's `calculateCost`
//! (`packages/ai/src/models.ts:1187-1203`) and `applyServiceTierPricing`
//! (`packages/ai/src/api/openai-responses.ts:382-393`).
//!
//! ## Cached-token accounting
//!
//! [`Usage::input`] counts only *new* input tokens: tokens OpenAI read
//! from cache are reported separately, in [`Usage::cache_read`], and
//! priced at the (usually much cheaper) cache-read rate. So the three
//! token counts are additive parts of one request's input, not
//! overlapping views of it: `input`, `cache_read` and `cache_write` are
//! summed to get the request's total input tokens (pi:
//! `usage.input + usage.cacheRead + usage.cacheWrite`,
//! `packages/ai/src/models.ts:1188`), but each is billed at its own
//! rate:
//!
//! - `input` (new tokens) at [`Pricing::input`];
//! - `cache_read` at [`Pricing::cache_read`];
//! - `cache_write` at [`Pricing::cache_write`];
//! - `output` at [`Pricing::output`].
//!
//! `output` is unrelated to caching: it is billed in full at the output
//! rate, and [`Usage::reasoning`] is already a subset of it (see
//! `message.rs`), not billed again.
//!
//! ## The long-context tier
//!
//! Some models' [`Pricing::long_context`] tier replaces every rate above
//! once the request's total input tokens (as summed above) exceed
//! [`PricingTier::input_tokens_above`]. The comparison is **strictly
//! greater than**, matching pi's `inputTokens > tier.inputTokensAbove`
//! (`packages/ai/src/models.ts:1192`): a request with exactly the
//! threshold's worth of input tokens still bills at the base rate; one
//! token more switches the whole request to the tier's rates. pi's tier
//! list supports several thresholds and picks the highest one matched;
//! `tau-ai`'s vendored table only ever has zero or one tier, but
//! [`cost`] keeps the same "highest matched threshold wins" shape so a
//! future second tier costs nothing to add.
//!
//! ## Service tiers
//!
//! pi additionally multiplies the four cost fields (not the token
//! counts) by a flat factor when a response used a non-default
//! `service_tier`, in a step separate from `calculateCost`
//! (`applyServiceTierPricing`). [`Usage`] carries no service-tier field
//! (neither does pi's), so [`cost`] takes it as a separate
//! [`ServiceTier`] argument. This is the only cost adjustment pi makes
//! that is not part of the model table; there is no other service-tier
//! or spend-tier multiplier in pi's cost code.

use crate::{
    message::{Usage, UsageCost},
    model::{Model, ServiceTier},
};

/// One request's cost, from `usage` against `model`'s pricing, at the
/// given `service_tier`. See the module docs for the formula.
pub fn cost(
    model: &Model,
    usage: &Usage,
    service_tier: ServiceTier,
) -> UsageCost {
    let rates = rates_for(model, usage);

    let mut result = UsageCost {
        input: token_cost(rates.input, usage.input),
        output: token_cost(rates.output, usage.output),
        cache_read: token_cost(rates.cache_read, usage.cache_read),
        cache_write: token_cost(rates.cache_write, usage.cache_write),
        total: 0.0,
    };
    result.total = sum(&result);

    // packages/ai/src/api/openai-responses.ts:382-393: applied after the
    // base cost, and only recomputes `total` when it does anything.
    let multiplier = service_tier.multiplier(&model.id);
    if multiplier != 1.0 {
        result.input *= multiplier;
        result.output *= multiplier;
        result.cache_read *= multiplier;
        result.cache_write *= multiplier;
        result.total = sum(&result);
    }

    result
}

/// Computes `usage`'s cost against `model` at `service_tier` and stores
/// it on `usage.cost`, the way pi's `calculateCost` mutates `usage.cost`
/// in place.
pub fn apply(model: &Model, usage: &mut Usage, service_tier: ServiceTier) {
    usage.cost = cost(model, usage, service_tier);
}

/// The four USD-per-million-token rates that apply to one request.
struct Rates {
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write: f64,
}

/// Picks the base rate, or the long-context tier's rate when the
/// request's total input tokens exceed its threshold. Mirrors pi's tier
/// loop (`packages/ai/src/models.ts:1189-1196`), which keeps the tier
/// with the highest matched threshold; `tau-ai` only ever has zero or
/// one tier today.
fn rates_for(model: &Model, usage: &Usage) -> Rates {
    let total_input_tokens = usage.input + usage.cache_read + usage.cache_write;
    match &model.pricing.long_context {
        Some(tier) if total_input_tokens > tier.input_tokens_above => Rates {
            input: tier.input,
            output: tier.output,
            cache_read: tier.cache_read,
            cache_write: tier.cache_write,
        },
        _ => Rates {
            input: model.pricing.input,
            output: model.pricing.output,
            cache_read: model.pricing.cache_read,
            cache_write: model.pricing.cache_write,
        },
    }
}

fn token_cost(rate_per_million: f64, tokens: u64) -> f64 {
    (rate_per_million / 1_000_000.0) * tokens as f64
}

fn sum(cost: &UsageCost) -> f64 {
    cost.input + cost.output + cost.cache_read + cost.cache_write
}
