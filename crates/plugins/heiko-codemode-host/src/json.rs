//! Bounded JSON text for scripts. Values use the ordinary Luau mapping.
//! Decode refuses integer literals a Luau double cannot preserve exactly.

use std::io::{self, Write};

use mlua::{Lua, Value as LuaValue};
use serde_json::Value;

use crate::value;

/// Maximum bytes in one JSON text, before decoding or while encoding.
pub const MAX_BYTES: usize = 1024 * 1024;
const EXACT_INTEGER: &[u8] = b"9007199254740992";

/// Decode one complete JSON text without rounding integer literals.
pub fn decode(text: &[u8]) -> Result<Value, String> {
    if text.len() > MAX_BYTES {
        return Err(format!("JSON text exceeds the {MAX_BYTES}-byte limit"));
    }
    let value: Value =
        serde_json::from_slice(text).map_err(|error| error.to_string())?;
    check_integer_literals(text)?;
    Ok(value)
}

/// The text has already passed serde_json's syntax/depth checks. Inspect
/// original literals because serde_json represents integers beyond u64
/// as doubles, losing the distinction before a Value visitor can see it.
fn check_integer_literals(text: &[u8]) -> Result<(), String> {
    let mut index = 0;
    while index < text.len() {
        match text[index] {
            b'"' => {
                index += 1;
                while index < text.len() && text[index] != b'"' {
                    if text[index] == b'\\' {
                        index += 1;
                    }
                    index += 1;
                }
                index += 1;
            }
            b'-' | b'0'..=b'9' => {
                let start = index;
                while index < text.len()
                    && matches!(
                        text[index],
                        b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E'
                    )
                {
                    index += 1;
                }
                let literal = &text[start..index];
                if literal
                    .iter()
                    .any(|byte| matches!(byte, b'.' | b'e' | b'E'))
                {
                    continue;
                }
                let digits = literal.strip_prefix(b"-").unwrap_or(literal);
                if digits.len() > EXACT_INTEGER.len()
                    || (digits.len() == EXACT_INTEGER.len()
                        && digits > EXACT_INTEGER)
                {
                    return Err(
                        "JSON integer is outside Luau's exact range of ±2^53"
                            .into(),
                    );
                }
            }
            _ => index += 1,
        }
    }
    Ok(())
}

/// Encode a script value without allocating an unbounded output string.
pub fn encode(lua: &Lua, value: &LuaValue) -> Result<String, String> {
    let value = value::from_lua(lua, value)?;
    let mut writer = JsonWriter(Vec::new());
    serde_json::to_writer(&mut writer, &value)
        .map_err(|error| error.to_string())?;
    String::from_utf8(writer.0).map_err(|error| error.to_string())
}

struct JsonWriter(Vec<u8>);

impl Write for JsonWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_BYTES - self.0.len() {
            return Err(io::Error::other(format!(
                "JSON text exceeds the {MAX_BYTES}-byte limit"
            )));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
