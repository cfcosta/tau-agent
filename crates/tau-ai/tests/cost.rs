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

use hegel::{
    TestCase,
    generators::{self as gs, Generator as _, PrintableGenerator},
};
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

/// Any service tier. `ServiceTier` is `tau-ai`'s own type, so drawn
/// values print through Debug.
fn service_tier() -> impl PrintableGenerator<ServiceTier> {
    gs::sampled_from(vec![
        ServiceTier::Default,
        ServiceTier::Flex,
        ServiceTier::PriorityOrFast,
    ])
    .print_as_debug()
}

/// Any model in the table.
fn any_model() -> impl PrintableGenerator<&'static Model> {
    gs::sampled_from(model::models().iter().collect::<Vec<_>>())
        .print_as_debug()
}

/// pi's formula written out again, apart from `tau_ai::cost`: the rates
/// the request's total input picks, each part at its rate, times the
/// service tier's factor from pi's `getServiceTierCostMultiplier`
/// (`packages/ai/src/api/openai-responses.ts:367-378`), as literals.
fn reference(model: &Model, usage: &Usage, tier: ServiceTier) -> [f64; 4] {
    let pricing = &model.pricing;
    let total_input = usage.input + usage.cache_read + usage.cache_write;
    let (input, output, cache_read, cache_write) = match pricing.long_context {
        Some(long) if total_input > long.input_tokens_above => {
            (long.input, long.output, long.cache_read, long.cache_write)
        }
        _ => (
            pricing.input,
            pricing.output,
            pricing.cache_read,
            pricing.cache_write,
        ),
    };
    let factor = match tier {
        ServiceTier::Default => 1.0,
        ServiceTier::Flex => 0.5,
        ServiceTier::PriorityOrFast if model.id == "gpt-5.5" => 2.5,
        ServiceTier::PriorityOrFast => 2.0,
    };
    let part =
        |rate: f64, tokens: u64| rate / 1_000_000.0 * tokens as f64 * factor;
    [
        part(input, usage.input),
        part(output, usage.output),
        part(cache_read, usage.cache_read),
        part(cache_write, usage.cache_write),
    ]
}

/// A usage for `model`. For a model with a long-context tier, half the
/// time the request's total input sits within three tokens of the
/// threshold, split at random across its three parts: the tier switch is
/// where a pricing bug hides.
fn usage_for(model: &'static Model) -> impl PrintableGenerator<Usage> {
    // Usage is tau's own type, so its drawn values print through Debug.
    usage_for_unprinted(model).print_as_debug()
}

#[hegel::composite]
fn usage_for_unprinted(tc: &TestCase, model: &'static Model) -> Usage {
    let near = model
        .pricing
        .long_context
        .filter(|_| tc.draw(gs::booleans()))
        .map(|long| long.input_tokens_above);
    let (input, cache_read, cache_write) = match near {
        Some(threshold) => {
            let offset =
                tc.draw(gs::integers::<i64>().min_value(-3).max_value(3));
            let total = threshold.saturating_add_signed(offset);
            let a = tc.draw(gs::integers::<u64>().max_value(total));
            let b = tc.draw(gs::integers::<u64>().max_value(total - a));
            (a, b, total - a - b)
        }
        None => {
            let tokens = || gs::integers::<u64>().max_value(1 << 30);
            (tc.draw(tokens()), tc.draw(tokens()), tc.draw(tokens()))
        }
    };
    let output = tc.draw(gs::integers::<u64>().max_value(1 << 30));
    Usage {
        input,
        output,
        cache_read,
        cache_write,
        reasoning: Some(output),
        total_tokens: input + output + cache_read + cache_write,
        cost: Default::default(),
    }
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

/// Zero usage costs exactly zero for every model and service tier.
#[test]
fn zero_usage_costs_zero_for_every_model_and_service_tier() {
    let usage = Usage::default();
    for model in model::models() {
        for tier in [
            ServiceTier::Default,
            ServiceTier::Flex,
            ServiceTier::PriorityOrFast,
        ] {
            let result = cost::cost(model, &usage, tier);
            assert_eq!(result.input, 0.0, "{} {tier:?}", model.id);
            assert_eq!(result.output, 0.0, "{} {tier:?}", model.id);
            assert_eq!(result.cache_read, 0.0, "{} {tier:?}", model.id);
            assert_eq!(result.cache_write, 0.0, "{} {tier:?}", model.id);
            assert_eq!(result.total, 0.0, "{} {tier:?}", model.id);
        }
    }
}

/// How many tokens each part of a usage may have so that two usages, and
/// their sum, stay in `model`'s base tier: a sixth of the threshold, as
/// three parts of two usages add up to at most the threshold.
fn within_base_tier(model: &Model) -> u64 {
    model
        .pricing
        .long_context
        .map_or(1 << 30, |long| long.input_tokens_above / 6)
}

/// **Cost is additive:** `cost(a) + cost(b) == cost(a + b)` for two
/// usages of the same model and the same tier (`testing.md`'s `tau-ai`
/// property inventory), on a model with a long-context tier too. Each
/// part is bounded by [`within_base_tier`], so "same tier" holds by
/// construction rather than by `tc.assume`.
#[hegel::test(test_cases = 500)]
fn cost_is_additive_within_a_tier(tc: TestCase) {
    let model = tc.draw(
        gs::sampled_from(vec![untiered_model(), tiered_model()])
            .print_as_debug(),
    );
    let tier = tc.draw(service_tier());
    let max = within_base_tier(model);
    let a = tc.draw(generators::usage_with_max_tokens(max));
    let b = tc.draw(generators::usage_with_max_tokens(max));

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

#[derive(Clone, Copy, Debug)]
enum TokenField {
    Input,
    Output,
    CacheRead,
    CacheWrite,
}

fn usage_with_increment(
    usage: &Usage,
    field: TokenField,
    increment: u64,
) -> Usage {
    let mut increased = usage.clone();
    let tokens = match field {
        TokenField::Input => &mut increased.input,
        TokenField::Output => &mut increased.output,
        TokenField::CacheRead => &mut increased.cache_read,
        TokenField::CacheWrite => &mut increased.cache_write,
    };
    *tokens = tokens
        .checked_add(increment)
        .expect("bounded token increment fits in u64");
    increased.total_tokens = increased
        .total_tokens
        .checked_add(increment)
        .expect("bounded total-token increment fits in u64");
    increased
}

fn field_cost(costs: [f64; 4], field: TokenField) -> f64 {
    match field {
        TokenField::Input => costs[0],
        TokenField::Output => costs[1],
        TokenField::CacheRead => costs[2],
        TokenField::CacheWrite => costs[3],
    }
}

fn uses_long_context_rates(model: &Model, usage: &Usage) -> bool {
    model.pricing.long_context.is_some_and(|long| {
        usage.input + usage.cache_read + usage.cache_write
            > long.input_tokens_above
    })
}

fn assert_matches_reference(model: &Model, usage: &Usage, tier: ServiceTier) {
    let [input, output, cache_read, cache_write] =
        reference(model, usage, tier);
    let expected_total = input + output + cache_read + cache_write;

    let result = cost::cost(model, usage, tier);
    assert_close(result.input, input, "cost input");
    assert_close(result.output, output, "cost output");
    assert_close(result.cache_read, cache_read, "cost cache_read");
    assert_close(result.cache_write, cache_write, "cost cache_write");
    assert_close(result.total, expected_total, "cost total");

    let before_apply = usage.clone();
    let mut applied = usage.clone();
    cost::apply(model, &mut applied, tier);
    assert_close(applied.cost.input, input, "apply input");
    assert_close(applied.cost.output, output, "apply output");
    assert_close(applied.cost.cache_read, cache_read, "apply cache_read");
    assert_close(applied.cost.cache_write, cache_write, "apply cache_write");
    assert_close(applied.cost.total, expected_total, "apply total");

    assert_eq!(applied.input, before_apply.input, "apply changed input");
    assert_eq!(applied.output, before_apply.output, "apply changed output");
    assert_eq!(
        applied.cache_read, before_apply.cache_read,
        "apply changed cache_read"
    );
    assert_eq!(
        applied.cache_write, before_apply.cache_write,
        "apply changed cache_write"
    );
    assert_eq!(
        applied.reasoning, before_apply.reasoning,
        "apply changed reasoning"
    );
    assert_eq!(
        applied.total_tokens, before_apply.total_tokens,
        "apply changed total_tokens"
    );
}

/// Property inventory: `cost` and `apply` match the independent literal
/// reference for every model/tier and threshold-adjacent usage; four paired
/// 1..=100 token increments also match after any pricing-tier switch. Usage
/// components stay bounded by 1<<30 (or are split around the model threshold),
/// and Hegel shrinks each draw while the composite preserves those bounds.
/// Monotonicity is checked from the reference only when both usages select the
/// same effective pricing rates.
#[hegel::test(test_cases = 1000)]
fn cost_matches_the_reference_formula(tc: TestCase) {
    let model = tc.draw(any_model());
    let tier = tc.draw(service_tier());
    let usage = tc.draw(usage_for(model));
    assert_matches_reference(model, &usage, tier);

    let increments = [
        (
            TokenField::Input,
            tc.draw(gs::integers::<u64>().min_value(1).max_value(100)),
        ),
        (
            TokenField::Output,
            tc.draw(gs::integers::<u64>().min_value(1).max_value(100)),
        ),
        (
            TokenField::CacheRead,
            tc.draw(gs::integers::<u64>().min_value(1).max_value(100)),
        ),
        (
            TokenField::CacheWrite,
            tc.draw(gs::integers::<u64>().min_value(1).max_value(100)),
        ),
    ];
    let base_rates = uses_long_context_rates(model, &usage);
    let base_costs = reference(model, &usage, tier);

    for (field, increment) in increments {
        let increased = usage_with_increment(&usage, field, increment);
        assert_matches_reference(model, &increased, tier);

        if base_rates == uses_long_context_rates(model, &increased) {
            let increased_costs = reference(model, &increased, tier);
            assert!(
                field_cost(increased_costs, field)
                    >= field_cost(base_costs, field),
                "{field:?} reference cost decreased at unchanged pricing rates"
            );
        }
    }
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
