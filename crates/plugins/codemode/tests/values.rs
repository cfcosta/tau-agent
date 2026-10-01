//! JSON values in Luau, and the output budget.
//!
//! | Property | Oracle |
//! | --- | --- |
//! | JSON → Lua → JSON is the identity (numbers as doubles) | round trip |
//! | a cut keeps a prefix and a suffix within budget | algebraic |

use hegel::{TestCase, generators as gs};
use mlua::Lua;
use serde_json::{Map, Value, json};
use tau_codemode::{
    result::{Cut, cut, tokens},
    value,
};

/// What a JSON value is once its numbers are Luau doubles.
fn as_doubles(value: &Value) -> Value {
    match value {
        Value::Number(n) => value::number(n.as_f64().unwrap()),
        Value::Array(items) => {
            Value::Array(items.iter().map(as_doubles).collect())
        }
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), as_doubles(v)))
                .collect::<Map<_, _>>(),
        ),
        other => other.clone(),
    }
}

#[hegel::test]
fn json_to_lua_and_back_is_the_identity(tc: TestCase) {
    let json: Value = tc.draw(hegel::extras::serde_json::values());
    let lua = Lua::new();
    let lua_value = value::to_lua(&lua, &json).unwrap();
    let back = value::from_lua(&lua, &lua_value).unwrap();
    assert_eq!(back, as_doubles(&json));
}

#[test]
fn empty_arrays_and_nulls_survive() {
    let lua = Lua::new();
    for json in [
        json!([]),
        json!({}),
        json!(null),
        json!([null, [], {}]),
        json!({ "a": null }),
    ] {
        let back = value::from_lua(&lua, &value::to_lua(&lua, &json).unwrap())
            .unwrap();
        assert_eq!(back, json);
    }
}

#[hegel::test]
fn truncation_keeps_a_prefix_and_a_suffix_within_budget(tc: TestCase) {
    let text: String = tc.draw(gs::text().max_size(400));
    let max: u64 = tc.draw(gs::integers::<u64>().max_value(120));
    match cut(&text, max) {
        Cut::Whole => assert!(tokens(&text) <= max),
        Cut::Cut {
            head,
            tail,
            original_tokens,
            removed_tokens,
        } => {
            assert!(tokens(&text) > max);
            assert_eq!(original_tokens, tokens(&text));
            assert!(text.starts_with(head));
            assert!(text.ends_with(tail));
            let head_chars = head.chars().count();
            let tail_chars = tail.chars().count();
            assert!((head_chars + tail_chars) as u64 <= max * 4);
            // Half the kept characters from each end.
            assert!(tail_chars - head_chars <= 1);
            assert!(head.len() + tail.len() <= text.len());
            let middle = &text[head.len()..text.len() - tail.len()];
            assert_eq!(removed_tokens, tokens(middle));
        }
    }
}
