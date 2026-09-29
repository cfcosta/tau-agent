//! Model and pricing table checks.
//!
//! Example tests, allowed by `testing.md`'s "Model and pricing table"
//! row: the table is a hand-maintained, finite data set, not a
//! computation with a property to state.

use std::collections::HashSet;

use tau_ai::model::{self, Model};

/// Every model has a positive context window and output limit, an
/// output limit that fits inside the context window, and non-negative
/// prices.
#[test]
fn every_model_has_sane_limits_and_prices() {
    for m in model::models() {
        assert!(m.context_window > 0, "{}: context_window is 0", m.id);
        assert!(m.max_output > 0, "{}: max_output is 0", m.id);
        assert!(
            m.max_output <= m.context_window,
            "{}: max_output {} exceeds context_window {}",
            m.id,
            m.max_output,
            m.context_window
        );
        for (label, price) in [
            ("input", m.pricing.input),
            ("output", m.pricing.output),
            ("cache_read", m.pricing.cache_read),
            ("cache_write", m.pricing.cache_write),
        ] {
            assert!(
                price >= 0.0,
                "{}: {label} price {price} is negative",
                m.id
            );
        }
        if let Some(tier) = &m.pricing.long_context {
            for (label, price) in [
                ("tier input", tier.input),
                ("tier output", tier.output),
                ("tier cache_read", tier.cache_read),
                ("tier cache_write", tier.cache_write),
            ] {
                assert!(
                    price >= 0.0,
                    "{}: {label} price {price} is negative",
                    m.id
                );
            }
        }
    }
}

/// Ids are unique and the table is sorted by id, so a diff of the
/// vendored data is reviewable and `find` can assume no duplicates.
#[test]
fn ids_are_unique_and_sorted() {
    let ids: Vec<&str> =
        model::models().iter().map(|m| m.id.as_str()).collect();
    let unique: HashSet<&str> = ids.iter().copied().collect();
    assert_eq!(unique.len(), ids.len(), "duplicate model id in {ids:?}");

    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted, "models() is not sorted by id");
}

/// `gpt-5.6` is a models.dev alias models.dev lists but OpenAI's API
/// does not accept (`MODELS_DEV_OPENAI_UNSUPPORTED_MODEL_IDS`,
/// generate-models.ts:356). It must not appear in the table.
#[test]
fn excluded_id_is_absent() {
    assert!(model::find("gpt-5.6").is_none());
    assert!(
        model::models().iter().all(|m| m.id != "gpt-5.6"),
        "gpt-5.6 should have been excluded"
    );
}

/// `OPENAI_SHORT_CONTEXT_CAPPED_MODEL_IDS` (generate-models.ts:380-388):
/// a capped model's context window is pinned to the long-context
/// threshold and its output to 128,000, even though models.dev reports
/// a much larger context window for it.
#[test]
fn short_context_capped_model_has_pinned_limits() {
    let m = get("gpt-5.4");
    assert_eq!(m.context_window, 272_000);
    assert_eq!(m.max_output, 128_000);
}

/// A model in `OPENAI_LONG_CONTEXT_PRICING_MODEL_IDS` but not in
/// `OPENAI_SHORT_CONTEXT_CAPPED_MODEL_IDS` (`gpt-5.4-pro`) keeps
/// models.dev's full context window: only the *pricing* tier is added,
/// the context window is not capped.
#[test]
fn long_context_pricing_without_short_context_cap_keeps_full_context() {
    let m = get("gpt-5.4-pro");
    assert_eq!(m.context_window, 1_050_000);
    assert_eq!(m.max_output, 128_000);
    assert!(m.pricing.long_context.is_some());
}

/// `withOpenAiLongContextPricing` (generate-models.ts:413-425): the tier
/// multiplies the base rate by x2 input, x1.5 output, x2 cache read, x2
/// cache write, active above 272,000 input tokens.
#[test]
fn long_context_tier_applies_pis_multipliers() {
    let m = get("gpt-5.5");
    let tier = m
        .pricing
        .long_context
        .expect("gpt-5.5 has a long-context tier");
    assert_eq!(tier.input_tokens_above, 272_000);
    assert_eq!(tier.input, m.pricing.input * 2.0);
    assert_eq!(tier.output, m.pricing.output * 1.5);
    assert_eq!(tier.cache_read, m.pricing.cache_read * 2.0);
    // gpt-5.5 has no cache_write rate; 0 x 2 is still 0.
    assert_eq!(tier.cache_write, m.pricing.cache_write * 2.0);
}

/// `OPENAI_STANDARD_COSTS` (generate-models.ts:428-438) replaces the
/// models.dev base rate for the ids it lists, before the long-context
/// tier is computed from it. Pinned against
/// `data/openai-overrides.json`'s `standardCosts`, itself pinned from
/// OpenAI's pricing page.
#[test]
fn standard_cost_override_replaces_base_rate() {
    let m = get("gpt-6-astra");
    assert_eq!(m.pricing.input, 10.0);
    assert_eq!(m.pricing.output, 50.0);
    assert_eq!(m.pricing.cache_read, 1.0);
    assert_eq!(m.pricing.cache_write, 12.5);
    let tier = m
        .pricing
        .long_context
        .expect("gpt-6-astra has a long-context tier");
    assert_eq!(tier.input, 20.0);
    assert_eq!(tier.output, 75.0);
    assert_eq!(tier.cache_read, 2.0);
    assert_eq!(tier.cache_write, 25.0);
}

/// generate-models.ts:2820-2824: models.dev reports `gpt-5-pro`'s output
/// limit as 272,000, a duplicate of its input sub-limit; the real max
/// output is 128,000.
#[test]
fn gpt_5_pro_max_output_is_fixed_up() {
    let m = get("gpt-5-pro");
    assert_eq!(m.max_output, 128_000);
    assert_eq!(m.context_window, 400_000);
}

/// Pinned prices for a handful of models, straight from
/// `OPENAI_STANDARD_COSTS` / models.dev, in USD per million tokens. A
/// silent price drift on a refresh should fail one of these.
#[test]
fn pinned_prices_for_a_few_models() {
    let gpt_5 = get("gpt-5");
    assert_eq!(gpt_5.pricing.input, 1.25);
    assert_eq!(gpt_5.pricing.output, 10.0);
    assert_eq!(gpt_5.pricing.cache_read, 0.125);

    let gpt_4o_mini = get("gpt-4o-mini");
    assert_eq!(gpt_4o_mini.pricing.input, 0.15);
    assert_eq!(gpt_4o_mini.pricing.output, 0.6);
    assert_eq!(gpt_4o_mini.pricing.cache_read, 0.075);

    let o3 = get("o3");
    assert_eq!(o3.pricing.input, 2.0);
    assert_eq!(o3.pricing.output, 8.0);
    assert_eq!(o3.pricing.cache_read, 0.5);

    let gpt_6_sol = get("gpt-6-sol");
    assert_eq!(gpt_6_sol.pricing.input, 2.0);
    assert_eq!(gpt_6_sol.pricing.output, 10.0);
    assert_eq!(gpt_6_sol.pricing.cache_read, 0.2);
    assert_eq!(gpt_6_sol.pricing.cache_write, 2.5);
}

/// The vendored file parses, and every model it lists with
/// `tool_call: true` (other than the one excluded id) made it into
/// [`model::models`]. A future refresh that silently drops a model, or
/// stops reporting `tool_call`, fails this test instead of shrinking the
/// catalog unnoticed.
#[test]
fn every_tool_call_model_in_vendored_data_is_in_the_table() {
    let vendored: serde_json::Value =
        serde_json::from_str(include_str!("../data/models-dev-openai.json"))
            .expect("vendored models.dev snapshot is valid JSON");
    let models = vendored["models"].as_object().expect("models is an object");
    assert!(!models.is_empty(), "vendored snapshot has no models");

    let overrides: serde_json::Value =
        serde_json::from_str(include_str!("../data/openai-overrides.json"))
            .expect("overrides file is valid JSON");
    let excluded: HashSet<&str> = overrides["excludedIds"]["ids"]
        .as_array()
        .expect("excludedIds.ids is an array")
        .iter()
        .map(|v| v.as_str().expect("id is a string"))
        .collect();

    let expected_ids: HashSet<&str> = models
        .iter()
        .filter(|(_, v)| v["tool_call"].as_bool() == Some(true))
        .map(|(id, _)| id.as_str())
        .filter(|id| !excluded.contains(id))
        .collect();

    let actual_ids: HashSet<&str> =
        model::models().iter().map(|m| m.id.as_str()).collect();

    assert_eq!(
        actual_ids, expected_ids,
        "models() disagrees with the vendored data filtered the same way"
    );
}

fn get(id: &str) -> &'static Model {
    model::find(id).unwrap_or_else(|| panic!("model {id} not found"))
}

/// Image input follows models.dev's input modalities: `gpt-5` takes
/// images, `o3-mini` and `gpt-4` take text only.
#[test]
fn image_input_follows_modalities() {
    let images = |id| {
        model::find(id)
            .unwrap_or_else(|| panic!("{id} missing"))
            .images
    };
    assert!(images("gpt-5"));
    assert!(!images("o3-mini"));
    assert!(!images("gpt-4"));
}

/// Each model offers the efforts the API takes for it, as probed: no
/// `minimal` past gpt-5, `max` only from gpt-5.6, and no `none` on
/// gpt-6-astra or gpt-6.1-sol.
#[test]
fn each_model_takes_its_own_efforts() {
    use tau_ai::responses::request::ReasoningEffort::*;
    let efforts = |id: &str| model::find(id).unwrap().efforts.clone();
    assert_eq!(efforts("gpt-5"), [Minimal, Low, Medium, High]);
    assert_eq!(efforts("gpt-5.5"), [None, Low, Medium, High, Xhigh]);
    assert_eq!(efforts("gpt-6-luna"), [None, Low, Medium, High, Xhigh, Max]);
    assert_eq!(efforts("gpt-6-astra"), [Low, Medium, High, Xhigh, Max]);
    assert_eq!(efforts("gpt-6.1-sol"), [Low, Medium, High, Xhigh, Max]);
    // Unprobed: Codex refuses it to ChatGPT accounts.
    assert_eq!(efforts("gpt-daybreak-red-latest"), [Low, Medium, High]);
    assert_eq!(efforts("gpt-5-pro"), [High]);
    assert!(efforts("gpt-4.1").is_empty(), "it does not reason");
    for model in model::models() {
        assert_eq!(model.reasoning, !model.efforts.is_empty(), "{}", model.id);
        assert!(
            model.efforts.windows(2).all(|pair| pair[0] < pair[1]),
            "{} lists its efforts lowest first",
            model.id
        );
    }
}

/// gpt-6.1-sol (released 2026-09-29) takes models.dev's price, which
/// halves gpt-6-sol's cache read, and the same 272,000-token cap and
/// long-context tier as the rest of the gpt-6 family.
#[test]
fn gpt_6_1_sol_is_capped_and_tiered_like_gpt_6() {
    let m = get("gpt-6.1-sol");
    assert_eq!((m.context_window, m.max_output), (272_000, 128_000));
    assert_eq!(m.pricing.input, 2.0);
    assert_eq!(m.pricing.output, 10.0);
    assert_eq!(m.pricing.cache_read, 0.1);
    assert_eq!(m.pricing.cache_write, 2.5);
    let tier = m
        .pricing
        .long_context
        .expect("gpt-6.1-sol has a long-context tier");
    assert_eq!(tier.input_tokens_above, 272_000);
    assert_eq!((tier.input, tier.output), (4.0, 15.0));
    assert_eq!((tier.cache_read, tier.cache_write), (0.2, 5.0));
}
