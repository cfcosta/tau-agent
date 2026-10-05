//! A Luau plugin's settings (ADR 0029): what its schema accepts, its
//! value with the defaults filled in, and the form drawn from the schema
//! when the plugin draws no page of its own.
//!
//! The schema is the subset forms can draw: an object of properties,
//! each a boolean, a string (one of an `enum`, or free), a number or an
//! integer, or an array of those. Keywords beyond that are not checked.

use serde_json::{Map, Value};

use crate::Declaration;

/// Checks `value` against `schema`: why not, when it does not hold. An
/// object with `properties` takes no other keys, so a page cannot write
/// keys its schema does not have.
pub fn check(schema: &Value, value: &Value) -> Result<(), String> {
    check_at(schema, value, "")
}

fn check_at(schema: &Value, value: &Value, at: &str) -> Result<(), String> {
    let name = || {
        if at.is_empty() {
            "the settings".to_owned()
        } else {
            format!("`{at}`")
        }
    };
    let Some(kind) = schema.get("type").and_then(Value::as_str) else {
        return Ok(());
    };
    match (kind, value) {
        ("object", Value::Object(fields)) => {
            let Some(properties) =
                schema.get("properties").and_then(Value::as_object)
            else {
                return Ok(());
            };
            for (key, field) in fields {
                let path = if at.is_empty() {
                    key.clone()
                } else {
                    format!("{at}.{key}")
                };
                let Some(property) = properties.get(key) else {
                    return Err(format!("`{path}` is not a setting"));
                };
                check_at(property, field, &path)?;
            }
            Ok(())
        }
        ("array", Value::Array(items)) => {
            let item = schema.get("items").unwrap_or(&Value::Null);
            items.iter().enumerate().try_for_each(|(n, value)| {
                check_at(item, value, &format!("{at}[{n}]"))
            })
        }
        ("string", Value::String(text)) => match schema.get("enum") {
            Some(Value::Array(options))
                if !options.iter().any(|o| o.as_str() == Some(text)) =>
            {
                Err(format!("{} is not one of its choices", name()))
            }
            _ => Ok(()),
        },
        ("boolean", Value::Bool(_)) => Ok(()),
        ("number", Value::Number(_)) => Ok(()),
        ("integer", Value::Number(number))
            if number.is_i64() || number.is_u64() =>
        {
            Ok(())
        }
        (kind, _) => Err(format!("{} should be {}", name(), article(kind))),
    }
}

fn article(kind: &str) -> String {
    match kind {
        "object" | "array" | "integer" => format!("an {kind}"),
        _ => format!("a {kind}"),
    }
}

/// What a run of the plugin takes: `saved` over its defaults, key by
/// key, when the schema accepts it; else the defaults, and why.
pub fn effective(
    declaration: &Declaration,
    saved: Option<&Value>,
) -> (Value, Option<String>) {
    let defaults = declaration.default_settings();
    let Some(saved) = saved else {
        return (defaults, None);
    };
    let value = match (&defaults, saved) {
        (Value::Object(defaults), Value::Object(saved)) => {
            let mut merged = defaults.clone();
            merged.extend(saved.clone());
            Value::Object(merged)
        }
        _ => saved.clone(),
    };
    match check(&schema_of(declaration), &value) {
        Ok(()) => (value, None),
        Err(why) => (
            defaults,
            Some(format!("{why}: the defaults hold until it is set again")),
        ),
    }
}

/// The plugin's settings schema, or none.
pub fn schema_of(declaration: &Declaration) -> Value {
    declaration
        .settings
        .as_ref()
        .and_then(|settings| settings.get("schema"))
        .cloned()
        .unwrap_or(Value::Null)
}

/// `value` with `key` (dotted: `limits.max`) set to `new`, making the
/// tables on its way.
pub fn set_key(value: &Value, key: &str, new: Value) -> Value {
    let mut value = value.clone();
    let mut at = &mut value;
    let mut parts = key.split('.').peekable();
    while let Some(part) = parts.next() {
        if !at.is_object() {
            *at = Value::Object(Map::new());
        }
        let object = at.as_object_mut().expect("made an object above");
        if parts.peek().is_none() {
            object.insert(part.to_owned(), new);
            break;
        }
        at = object.entry(part.to_owned()).or_insert(Value::Null);
    }
    value
}

/// The value at `key` (dotted) in `value`.
pub fn get_key<'v>(value: &'v Value, key: &str) -> &'v Value {
    key.split('.').fold(value, |at, part| &at[part])
}

/// One setting of a form drawn from the schema.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub key: String,
    /// Its `title`, else its key.
    pub label: String,
    pub description: Option<String>,
    pub kind: FieldKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FieldKind {
    Switch,
    /// One of `options`, or several when `multi`.
    Choice {
        options: Vec<String>,
        multi: bool,
    },
    Text,
    Number {
        integer: bool,
    },
    /// A shape the form does not draw: shown with its value.
    Other,
}

/// The form for `schema`'s properties, by key: a Luau table keeps no
/// order of its own.
pub fn fields(schema: &Value) -> Vec<Field> {
    let Some(properties) = schema.get("properties").and_then(Value::as_object)
    else {
        return Vec::new();
    };
    let mut properties: Vec<(&String, &Value)> = properties.iter().collect();
    properties.sort_by_key(|(key, _)| *key);
    properties
        .into_iter()
        .map(|(key, property)| {
            let options = |schema: &Value| -> Option<Vec<String>> {
                schema.get("enum")?.as_array().map(|options| {
                    options
                        .iter()
                        .filter_map(|o| o.as_str().map(str::to_owned))
                        .collect()
                })
            };
            let kind = match property.get("type").and_then(Value::as_str) {
                Some("boolean") => FieldKind::Switch,
                Some("string") => match options(property) {
                    Some(options) => FieldKind::Choice {
                        options,
                        multi: false,
                    },
                    None => FieldKind::Text,
                },
                Some("number") => FieldKind::Number { integer: false },
                Some("integer") => FieldKind::Number { integer: true },
                Some("array") => {
                    match property.get("items").and_then(options) {
                        Some(options) => FieldKind::Choice {
                            options,
                            multi: true,
                        },
                        None => FieldKind::Other,
                    }
                }
                _ => FieldKind::Other,
            };
            let text = |name: &str| {
                property
                    .get(name)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            };
            Field {
                key: key.clone(),
                label: text("title").unwrap_or_else(|| key.clone()),
                description: text("description"),
                kind,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn days() -> Value {
        json!({ "type": "object", "properties": {
            "days": { "type": "array", "items": { "type": "string", "enum": ["Mon", "Fri"] } },
            "strict": { "type": "boolean" },
            "max": { "type": "integer" },
        } })
    }

    #[test]
    fn values_are_checked_against_the_schema() {
        assert!(
            check(
                &days(),
                &json!({ "days": ["Fri"], "strict": true, "max": 3 })
            )
            .is_ok()
        );
        assert_eq!(
            check(&days(), &json!({ "days": ["Sun"] })),
            Err("`days[0]` is not one of its choices".into())
        );
        assert_eq!(
            check(&days(), &json!({ "other": 1 })),
            Err("`other` is not a setting".into())
        );
        assert_eq!(
            check(&days(), &json!({ "max": 1.5 })),
            Err("`max` should be an integer".into())
        );
        assert!(check(&Value::Null, &json!({ "anything": 1 })).is_ok());
    }

    #[test]
    fn saved_values_fill_over_the_defaults_or_give_way() {
        let declaration = Declaration {
            settings: Some(
                json!({ "schema": days(), "default": { "days": ["Fri"], "strict": false } }),
            ),
            ..Declaration::named("p")
        };
        assert_eq!(
            effective(&declaration, Some(&json!({ "strict": true }))),
            (json!({ "days": ["Fri"], "strict": true }), None)
        );
        let (value, why) =
            effective(&declaration, Some(&json!({ "days": ["Sun"] })));
        assert_eq!(value, json!({ "days": ["Fri"], "strict": false }));
        assert!(why.unwrap().contains("not one of its choices"));
    }

    #[test]
    fn keys_set_and_read_through_tables() {
        let value = set_key(&json!({ "a": 1 }), "limits.max", json!(3));
        assert_eq!(value, json!({ "a": 1, "limits": { "max": 3 } }));
        assert_eq!(get_key(&value, "limits.max"), &json!(3));
        assert_eq!(get_key(&value, "limits.min"), &Value::Null);
    }

    #[test]
    fn the_form_follows_the_schema() {
        let kinds: Vec<FieldKind> =
            fields(&days()).into_iter().map(|f| f.kind).collect();
        assert_eq!(
            kinds,
            [
                FieldKind::Choice {
                    options: vec!["Mon".into(), "Fri".into()],
                    multi: true
                },
                FieldKind::Number { integer: true },
                FieldKind::Switch,
            ]
        );
    }
}
