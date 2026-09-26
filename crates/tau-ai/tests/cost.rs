//! Cost-from-usage properties. Oracle: pi's `calculateCost`
//! (`packages/ai/src/models.ts:1187-1203`) and `applyServiceTierPricing`
//! (`packages/ai/src/api/openai-responses.ts:382-393`), which
//! `tau_ai::cost` ports; see that module's doc comment for the formula.
//!
//! `RELATIVE_EPSILON` bounds the floating-point error from reassociating
//! sums of `rate * tokens` terms in a different order (e.g.
//! `cost(a) + cost(b)` sums per-field products in one order,
//! `cost(a + b)` in another). Doubles carry about 15-17 significant
//! decimal digits, so an epsilon of `1e-9` relative to the larger side
//! leaves ample room for a handful of reordered additions while still
//! catching a real formula bug (which is usually off by a constant
//! factor, not a rounding hair).

use hegel::TestCase;
use tau_ai::{
    cost,
    message::Usage,
    model::{self, Model, ServiceTier},
};
use tau_testing::generators;

const RELATIVE_EPSILON: f64 = 1e-9;

/// A model with no long-context tier, so any combination of token counts
/// stays in the same (only) pricing tier. Used by properties that are
/// not about the tier switch itself.
fn untiered_model() -> &'static Model {
    let m = model::find("gpt-4o").expect("gpt-4o is in the table");
    assert!(
        m.pricing.long_context.is_none(),
        "gpt-4o must stay untiered for this test to be valid"
    );
    m
}

/// A model with a long-context tier, for the tier-switch tests.
fn tiered_model() -> &'static Model {
    let m = model::find("gpt-5.5").expect("gpt-5.5 is in the table");
    assert!(
        m.pricing.long_context.is_some(),
        "gpt-5.5 must have a tier for this test to be valid"
    );
    m
}

fn assert_close(actual: f64, expected: f64, what: &str) {
    let scale = actual.abs().max(expected.abs()).max(1.0);
    let diff = (actual - expected).abs();
    assert!(
        diff <= RELATIVE_EPSILON * scale,
        "{what}: {actual} vs {expected} (diff {diff}, allowed {})",
        RELATIVE_EPSILON * scale
    );
}

fn add(a: &Usage, b: &Usage) -> Usage {
    Usage {
        input: a.input + b.input,
        output: a.output + b.output,
        cache_read: a.cache_read + b.cache_read,
        cache_write: a.cache_write + b.cache_write,
        reasoning: None,
        total_tokens: a.total_tokens + b.total_tokens,
        cost: Default::default(),
    }
}

/// Zero usage costs zero, at every service tier.
#[hegel::test(test_cases = 100)]
fn zero_usage_is_zero_cost(tc: TestCase) {
    let model = untiered_model();
    let tier = tc.draw(hegel::generators::sampled_from(vec![
        ServiceTier::Default,
        ServiceTier::Flex,
        ServiceTier::PriorityOrFast,
    ]));
    let usage = Usage::default();
    let result = cost::cost(model, &usage, tier);
    assert_eq!(result.input, 0.0);
    assert_eq!(result.output, 0.0);
    assert_eq!(result.cache_read, 0.0);
    assert_eq!(result.cache_write, 0.0);
    assert_eq!(result.total, 0.0);
}

/// **Cost is additive:** `cost(a) + cost(b) == cost(a + b)` for two
/// usages of the same model and the same tier (`testing.md`'s `tau-ai`
/// property inventory). Bounding each usage's tokens to a quarter of the
/// long-context threshold keeps both `a`, `b` and `a + b` in the base
/// tier, so "same tier" holds by construction rather than by `tc.assume`.
#[hegel::test(test_cases = 500)]
fn cost_is_additive_within_a_tier(tc: TestCase) {
    let model = untiered_model();
    let tier = tc.draw(hegel::generators::sampled_from(vec![
        ServiceTier::Default,
        ServiceTier::Flex,
        ServiceTier::PriorityOrFast,
    ]));
    let a = tc.draw(generators::usage_with_max_tokens(1 << 30));
    let b = tc.draw(generators::usage_with_max_tokens(1 << 30));

    let cost_a = cost::cost(model, &a, tier);
    let cost_b = cost::cost(model, &b, tier);
    let cost_sum = cost::cost(model, &add(&a, &b), tier);

    assert_close(cost_a.input + cost_b.input, cost_sum.input, "input");
    assert_close(cost_a.output + cost_b.output, cost_sum.output, "output");
    assert_close(
        cost_a.cache_read + cost_b.cache_read,
        cost_sum.cache_read,
        "cache_read",
    );
    assert_close(
        cost_a.cache_write + cost_b.cache_write,
        cost_sum.cache_write,
        "cache_write",
    );
    assert_close(cost_a.total + cost_b.total, cost_sum.total, "total");
}

/// Cost is monotone in each token count: adding tokens to any one field
/// never decreases that field's cost (every rate is non-negative, so
/// this is `cost` restated as "the rates never invent a discount").
#[hegel::test(test_cases = 500)]
fn cost_is_monotone_in_each_token_count(tc: TestCase) {
    let model = untiered_model();
    let base = tc.draw(generators::usage_with_max_tokens(1 << 30));
    let extra =
        tc.draw(hegel::generators::integers::<u64>().max_value(1 << 30));
    let tier = ServiceTier::Default;

    let base_cost = cost::cost(model, &base, tier);

    let mut with_more_input = base.clone();
    with_more_input.input += extra;
    assert!(cost::cost(model, &with_more_input, tier).input >= base_cost.input);

    let mut with_more_output = base.clone();
    with_more_output.output += extra;
    assert!(
        cost::cost(model, &with_more_output, tier).output >= base_cost.output
    );

    let mut with_more_cache_read = base.clone();
    with_more_cache_read.cache_read += extra;
    assert!(
        cost::cost(model, &with_more_cache_read, tier).cache_read
            >= base_cost.cache_read
    );

    let mut with_more_cache_write = base.clone();
    with_more_cache_write.cache_write += extra;
    assert!(
        cost::cost(model, &with_more_cache_write, tier).cache_write
            >= base_cost.cache_write
    );
}

/// `total` is always the sum of the four parts, at every tier and
/// service tier.
#[hegel::test(test_cases = 500)]
fn total_equals_sum_of_parts(tc: TestCase) {
    let model = tc.draw(hegel::generators::sampled_from(
        model::models().iter().collect::<Vec<_>>(),
    ));
    let tier = tc.draw(hegel::generators::sampled_from(vec![
        ServiceTier::Default,
        ServiceTier::Flex,
        ServiceTier::PriorityOrFast,
    ]));
    let usage = tc.draw(generators::usage_with_max_tokens(1 << 40));

    let result = cost::cost(model, &usage, tier);
    assert_close(
        result.input + result.output + result.cache_read + result.cache_write,
        result.total,
        "total",
    );
}

/// `cost::apply` fills `usage.cost` with exactly what `cost::cost`
/// returns.
#[hegel::test(test_cases = 100)]
fn apply_fills_usage_cost(tc: TestCase) {
    let model = untiered_model();
    let mut usage = tc.draw(generators::usage_with_max_tokens(1 << 30));
    let expected = cost::cost(model, &usage, ServiceTier::Default);
    cost::apply(model, &mut usage, ServiceTier::Default);
    assert_eq!(usage.cost, expected);
}

/// **Tier switch, at the threshold:** pi's comparison is
/// `inputTokens > tier.inputTokensAbove` (strictly greater than,
/// `packages/ai/src/models.ts:1192`). A request whose total input
/// tokens equal the threshold exactly still bills at the base rate; one
/// token more switches to the long-context tier's rate.
#[test]
fn tier_switch_is_strictly_greater_than_threshold() {
    let model = tiered_model();
    let tier = model.pricing.long_context.expect("tiered_model has a tier");
    let threshold = tier.input_tokens_above;

    let at_threshold = Usage {
        input: threshold,
        output: 0,
        cache_read: 0,
        cache_write: 0,
        reasoning: None,
        total_tokens: threshold,
        cost: Default::default(),
    };
    let just_over = Usage {
        input: threshold + 1,
        ..at_threshold.clone()
    };

    let at_cost = cost::cost(model, &at_threshold, ServiceTier::Default);
    let over_cost = cost::cost(model, &just_over, ServiceTier::Default);

    // At the threshold: base rate on all `threshold` tokens.
    assert_close(
        at_cost.input,
        model.pricing.input / 1_000_000.0 * threshold as f64,
        "at threshold",
    );

    // One token over: the WHOLE request (including the first `threshold`
    // tokens) bills at the tier rate, per pi's "highest matched
    // threshold applies to the full request" (types.ts:1029).
    assert_close(
        over_cost.input,
        tier.input / 1_000_000.0 * (threshold + 1) as f64,
        "just over threshold",
    );
    assert!(over_cost.input > at_cost.input);
}

/// Differential example: a hand-computed cost for one realistic request
/// against `gpt-5` (no long-context tier, `input = 1.25`, `output = 10`,
/// `cache_read = 0.125`, `cache_write = 0` per million tokens; pinned
/// again in `tests/model.rs`).
#[test]
fn hand_computed_cost_for_a_realistic_request() {
    let model = model::find("gpt-5").expect("gpt-5 is in the table");
    let usage = Usage {
        input: 8_000,
        output: 1_200,
        cache_read: 40_000,
        cache_write: 0,
        reasoning: Some(300),
        total_tokens: 49_200,
        cost: Default::default(),
    };

    let result = cost::cost(model, &usage, ServiceTier::Default);

    let expected_input = 1.25 / 1_000_000.0 * 8_000.0;
    let expected_output = 10.0 / 1_000_000.0 * 1_200.0;
    let expected_cache_read = 0.125 / 1_000_000.0 * 40_000.0;
    let expected_cache_write = 0.0;

    assert_close(result.input, expected_input, "input");
    assert_close(result.output, expected_output, "output");
    assert_close(result.cache_read, expected_cache_read, "cache_read");
    assert_close(result.cache_write, expected_cache_write, "cache_write");
    assert_close(
        result.total,
        expected_input
            + expected_output
            + expected_cache_read
            + expected_cache_write,
        "total",
    );
}

/// Service tiers multiply the whole cost, not the token counts: `flex`
/// halves it, `priority`/`fast` doubles it (or 2.5x for `gpt-5.5`
/// specifically), matching `getServiceTierCostMultiplier`
/// (`packages/ai/src/api/openai-responses.ts:367-378`).
#[test]
fn service_tier_multiplies_the_whole_cost() {
    // gpt-6-sol prices every part, cache writes included.
    let model = model::find("gpt-6-sol").expect("gpt-6-sol is in the table");
    let usage = Usage {
        input: 1_000,
        output: 1_000,
        cache_read: 1_000,
        cache_write: 1_000,
        reasoning: None,
        total_tokens: 4_000,
        cost: Default::default(),
    };

    let default_cost = cost::cost(model, &usage, ServiceTier::Default);
    let flex_cost = cost::cost(model, &usage, ServiceTier::Flex);
    let priority_cost = cost::cost(model, &usage, ServiceTier::PriorityOrFast);

    assert_close(flex_cost.total, default_cost.total * 0.5, "flex");
    // Each part is scaled, not only the total.
    for (part, flex, default) in [
        ("input", flex_cost.input, default_cost.input),
        ("output", flex_cost.output, default_cost.output),
        ("cache_read", flex_cost.cache_read, default_cost.cache_read),
        (
            "cache_write",
            flex_cost.cache_write,
            default_cost.cache_write,
        ),
    ] {
        assert_close(flex, default * 0.5, part);
    }
    assert_close(priority_cost.total, default_cost.total * 2.0, "priority");

    // gpt-5.5 is pi's one exception: priority/fast is 2.5x, not 2x.
    let gpt_5_5 = model::find("gpt-5.5").expect("gpt-5.5 is in the table");
    let gpt_5_5_default = cost::cost(gpt_5_5, &usage, ServiceTier::Default);
    let gpt_5_5_priority =
        cost::cost(gpt_5_5, &usage, ServiceTier::PriorityOrFast);
    assert_close(
        gpt_5_5_priority.total,
        gpt_5_5_default.total * 2.5,
        "gpt-5.5 priority",
    );
}
