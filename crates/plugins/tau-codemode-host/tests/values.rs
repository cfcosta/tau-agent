//! JSON values in Luau.
//!
//! | Property | Oracle |
//! | --- | --- |
//! | JSON → Lua → JSON is the identity (numbers as doubles), empty arrays, objects and nulls included | round trip |

use hegel::{TestCase, generators as gs};
use mlua::Lua;
use serde_json::{Map, Value, json};
use tau_codemode_host::value;

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

/// Any JSON value comes back from Luau as it went in, its numbers as
/// doubles. Empty arrays, empty objects and nulls, which a Lua table
/// cannot tell apart by itself, are drawn on purpose as well.
#[hegel::test]
fn json_to_lua_and_back_is_the_identity(tc: TestCase) {
    let json: Value = tc.draw(hegel::one_of!(
        gs::sampled_from(vec![
            json!([]),
            json!({}),
            json!(null),
            json!([null, [], {}]),
            json!({ "a": null }),
        ]),
        hegel::extras::serde_json::values(),
    ));
    let lua = Lua::new();
    let lua_value = value::to_lua(&lua, &json).unwrap();
    let back = value::from_lua(&lua, &lua_value).unwrap();
    assert_eq!(back, as_doubles(&json));
}
