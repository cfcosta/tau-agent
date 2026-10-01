//! JSON values to Luau and back.
//!
//! - JSON `null` is `json.null` (mlua's null light userdata), so a null
//!   field keeps its key in a table.
//! - Arrays carry mlua's array metatable, so an empty array comes back
//!   as `[]`, not `{}`. A table without it is an array when its keys are
//!   exactly `1..=n` with `n > 0`, and an object otherwise.
//! - Numbers are Luau doubles. Integral values within ±2^53 come back
//!   as JSON integers.
//! - Object keys come back sorted, so output does not depend on Luau's
//!   table order.

use mlua::{Lua, LuaSerdeExt, Table, Value as LuaValue};
use serde_json::{Map, Number, Value};

/// How deep a table may nest before it is refused, which also stops
/// cycles.
pub const MAX_DEPTH: usize = 128;

/// How far past its entry count an array's keys may reach (its holes
/// become `null`).
const MAX_HOLES: usize = 1024;

/// The largest integer a double holds exactly.
const SAFE_INTEGER: f64 = 9_007_199_254_740_992.0;

/// `value` as a Luau value.
pub fn to_lua(lua: &Lua, value: &Value) -> mlua::Result<LuaValue> {
    Ok(match value {
        Value::Null => lua.null(),
        Value::Bool(b) => LuaValue::Boolean(*b),
        Value::Number(n) => LuaValue::Number(n.as_f64().unwrap_or(f64::NAN)),
        Value::String(s) => LuaValue::String(lua.create_string(s)?),
        Value::Array(items) => {
            let table = lua.create_table_with_capacity(items.len(), 0)?;
            for (index, item) in items.iter().enumerate() {
                table.raw_set(index + 1, to_lua(lua, item)?)?;
            }
            table.set_metatable(Some(lua.array_metatable()))?;
            LuaValue::Table(table)
        }
        Value::Object(map) => {
            let table = lua.create_table_with_capacity(0, map.len())?;
            for (key, item) in map {
                table.raw_set(key.as_str(), to_lua(lua, item)?)?;
            }
            LuaValue::Table(table)
        }
    })
}

/// `value` as JSON, or why it has none.
pub fn from_lua(lua: &Lua, value: &LuaValue) -> Result<Value, String> {
    convert(lua, value, 0)
}

fn convert(lua: &Lua, value: &LuaValue, depth: usize) -> Result<Value, String> {
    if depth > MAX_DEPTH {
        return Err(format!(
            "a table nests deeper than {MAX_DEPTH} levels (is it cyclic?)"
        ));
    }
    match value {
        LuaValue::Nil => Ok(Value::Null),
        LuaValue::LightUserData(data) if data.0.is_null() => Ok(Value::Null),
        LuaValue::Boolean(b) => Ok(Value::Bool(*b)),
        LuaValue::Integer(i) => Ok(number(*i as f64)),
        LuaValue::Number(n) => Ok(number(*n)),
        LuaValue::String(s) => Ok(Value::String(s.to_string_lossy())),
        LuaValue::Table(table) => table_to_json(lua, table, depth),
        other => {
            Err(format!("a {} cannot be encoded as JSON", other.type_name()))
        }
    }
}

/// A double as JSON: integral values in the safe range as integers,
/// non-finite values as `null`, as `JSON.stringify` does.
pub fn number(n: f64) -> Value {
    if !n.is_finite() {
        return Value::Null;
    }
    if n.fract() == 0.0 && n.abs() <= SAFE_INTEGER {
        return Value::from(n as i64);
    }
    Number::from_f64(n).map_or(Value::Null, Value::Number)
}

fn table_to_json(
    lua: &Lua,
    table: &Table,
    depth: usize,
) -> Result<Value, String> {
    let marked = table
        .metatable()
        .is_some_and(|meta| meta == lua.array_metatable());
    let mut entries = Vec::new();
    for pair in table.pairs::<LuaValue, LuaValue>() {
        entries.push(pair.map_err(|error| error.to_string())?);
    }
    let length = entries.len();
    let is_sequence = length > 0
        && entries.iter().all(|(key, _)| {
            index(key).is_some_and(|i| i >= 1 && i as usize <= length)
        });
    if marked || is_sequence {
        let mut items = vec![Value::Null; length];
        let mut extent = 0;
        for (key, item) in &entries {
            let Some(i) = index(key).filter(|i| *i >= 1) else {
                return Err(
                    "an array table has a key that is not a positive integer"
                        .into(),
                );
            };
            let i = i as usize;
            if i > length + MAX_HOLES {
                return Err("an array table is too sparse to encode".into());
            }
            if i > items.len() {
                items.resize(i, Value::Null);
            }
            items[i - 1] = convert(lua, item, depth + 1)?;
            extent = extent.max(i);
        }
        items.truncate(extent);
        return Ok(Value::Array(items));
    }
    let mut keyed = Vec::with_capacity(length);
    for (key, item) in &entries {
        let key = match key {
            LuaValue::String(s) => s.to_string_lossy(),
            LuaValue::Integer(i) => i.to_string(),
            LuaValue::Number(n) => match number(*n) {
                Value::Number(n) => n.to_string(),
                _ => return Err("a table key is not a finite number".into()),
            },
            other => {
                return Err(format!(
                    "a table key is a {}; JSON keys are strings",
                    other.type_name()
                ));
            }
        };
        keyed.push((key, convert(lua, item, depth + 1)?));
    }
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(Value::Object(keyed.into_iter().collect::<Map<_, _>>()))
}

fn index(key: &LuaValue) -> Option<i64> {
    match key {
        LuaValue::Integer(i) => Some(*i),
        LuaValue::Number(n) if n.fract() == 0.0 && n.abs() <= SAFE_INTEGER => {
            Some(*n as i64)
        }
        _ => None,
    }
}

/// How `text` and `return` show a value: strings as they are, anything
/// else as compact JSON.
pub fn display(lua: &Lua, value: &LuaValue) -> Result<String, String> {
    match value {
        LuaValue::String(s) => Ok(s.to_string_lossy()),
        other => Ok(from_lua(lua, other)?.to_string()),
    }
}
