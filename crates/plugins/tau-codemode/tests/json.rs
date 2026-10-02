//! Properties: JSON text → Luau → JSON agrees with a double-normalized
//! serde_json oracle; JSON string escaping agrees with serde_json. Trees
//! and Unicode strings are valid by construction and shrink structurally.
//! Integer refusal and limits have boundary examples. Hegel profiles are
//! inherited from the workspace; no provider or wall-clock randomness.

mod common;

use std::sync::Arc;

use common::{FakeHost, error, script, texts};
use hegel::{TestCase, generators as gs};
use mlua::Lua;
use serde_json::{Map, Value, json};
use tau_codemode::{json as codec, value};
use tau_testing::block_on;

/// The oracle applies documented double semantics, not the codec itself.
fn doubles(value: &Value) -> Value {
    match value {
        Value::Number(n) => {
            let n = n.as_f64().unwrap();
            if n.fract() == 0.0 && n.abs() <= (1_u64 << 53) as f64 {
                json!(n as i64)
            } else {
                json!(n)
            }
        }
        Value::Array(items) => {
            Value::Array(items.iter().map(doubles).collect())
        }
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, value)| (key.clone(), doubles(value)))
                .collect::<Map<_, _>>(),
        ),
        other => other.clone(),
    }
}

/// Restrict integer literals by construction, without filtering trees.
fn exact_integers(value: Value) -> Value {
    match value {
        Value::Number(n) if n.is_i64() || n.is_u64() => {
            json!(
                n.as_f64()
                    .unwrap()
                    .clamp(-9_007_199_254_740_992.0, 9_007_199_254_740_992.0)
                    as i64
            )
        }
        Value::Array(items) => {
            Value::Array(items.into_iter().map(exact_integers).collect())
        }
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .map(|(key, value)| (key, exact_integers(value)))
                .collect(),
        ),
        other => other,
    }
}

#[hegel::test]
fn json_text_round_trip_matches_double_semantics(tc: TestCase) {
    let input = exact_integers(tc.draw(hegel::extras::serde_json::values()));
    let lua = Lua::new();
    let decoded = codec::decode(input.to_string().as_bytes()).unwrap();
    let encoded =
        codec::encode(&lua, &value::to_lua(&lua, &decoded).unwrap()).unwrap();
    let actual: Value = serde_json::from_str(&encoded).unwrap();
    assert_eq!(actual, doubles(&input));
}

#[hegel::test]
fn string_escaping_agrees_with_serde_json(tc: TestCase) {
    let text = tc.draw(gs::text().max_size(1_024));
    let lua = Lua::new();
    let value = mlua::Value::String(lua.create_string(&text).unwrap());
    assert_eq!(
        codec::encode(&lua, &value).unwrap(),
        serde_json::to_string(&text).unwrap()
    );
}

#[test]
fn integer_literals_refuse_silent_rounding_at_both_boundaries() {
    for literal in ["9007199254740992", "-9007199254740992"] {
        assert!(codec::decode(literal.as_bytes()).is_ok());
    }
    for literal in [
        "9007199254740993",
        "-9007199254740993",
        "18446744073709551615",
        "18446744073709551616",
        "-18446744073709551616",
        r#"{"nested":[9007199254740993]}"#,
    ] {
        assert!(
            codec::decode(literal.as_bytes())
                .unwrap_err()
                .contains("exact range")
        );
    }
}

#[test]
fn text_limits_apply_before_decode_and_during_encode() {
    let lua = Lua::new();
    let fits = "x".repeat(codec::MAX_BYTES - 2);
    let value = mlua::Value::String(lua.create_string(&fits).unwrap());
    let text = codec::encode(&lua, &value).unwrap();
    assert_eq!(text.len(), codec::MAX_BYTES);
    assert_eq!(codec::decode(text.as_bytes()).unwrap(), json!(fits));
    let too_large = mlua::Value::String(
        lua.create_string("x".repeat(codec::MAX_BYTES - 1)).unwrap(),
    );
    assert!(
        codec::encode(&lua, &too_large)
            .unwrap_err()
            .contains("byte limit")
    );
    assert!(
        codec::decode(&vec![b' '; codec::MAX_BYTES + 1])
            .unwrap_err()
            .contains("byte limit")
    );
}

#[test]
fn globals_preserve_shapes_and_raise_catchable_errors() {
    block_on(async {
        let host = Arc::new(FakeHost::default());
        let result = script(
            &host,
            r#"local value = json.decode('{"a":null,"b":[],"c":{}}')
               assert(value.a == json.null and #value.b == 0)
               local ok, err = pcall(function() json.decode('{') end)
               assert(not ok and type(err) == 'string')
               return json.encode(value)"#,
        )
        .await;
        assert!(!result.is_error(), "{:?}", result.failure);
        assert_eq!(texts(&result), [r#"{"a":null,"b":[],"c":{}}"#]);
        for code in [
            "json.decode(1)",
            "json.encode(function() end)",
            "local t = {}; t.self = t; json.encode(t)",
            "json.decode(string.char(255))",
        ] {
            let result = script(&host, code).await;
            assert!(result.is_error(), "{code}");
            assert!(error(&result).contains("json."), "{}", error(&result));
        }
    });
}
