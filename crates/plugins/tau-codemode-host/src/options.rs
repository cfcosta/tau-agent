//! The script's source: an optional `-- @options: {...}` first line,
//! then Luau code.

use serde_json::{Map, Value};

/// Output tokens a script may print when its options do not say.
pub const DEFAULT_MAX_OUTPUT_TOKENS: u64 = 10_000;

/// The largest `timeout_ms`: 2^31 − 1, as pi allows.
pub const MAX_TIMEOUT_MS: u64 = (1 << 31) - 1;

const PREFIX: &str = "-- @options:";

/// What the options line set. A key it left out is `None`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    pub max_output_tokens: Option<u64>,
    pub timeout_ms: Option<u64>,
}

impl Options {
    /// The output budget, in tokens (chars / 4).
    pub fn max_output_tokens(&self) -> u64 {
        self.max_output_tokens.unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS)
    }

    /// The options line for these options, without a newline.
    pub fn to_line(&self) -> String {
        let mut map = Map::new();
        if let Some(tokens) = self.max_output_tokens {
            map.insert("max_output_tokens".into(), tokens.into());
        }
        if let Some(ms) = self.timeout_ms {
            map.insert("timeout_ms".into(), ms.into());
        }
        format!("{PREFIX} {}", Value::Object(map))
    }
}

/// A script ready to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub options: Options,
    /// The whole input, the options line included: it is a Luau
    /// comment, and keeping it keeps error line numbers right.
    pub code: String,
}

/// Why a script failed before it ran. Its text is the whole result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceError(pub String);

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SourceError {}

fn fail<T>(message: impl Into<String>) -> Result<T, SourceError> {
    Err(SourceError(message.into()))
}

/// Splits the options line off `input` and checks both parts.
pub fn parse(input: &str) -> Result<Source, SourceError> {
    let (first, rest) = match input.find('\n') {
        Some(at) => (input[..at].trim_end_matches('\r'), &input[at + 1..]),
        None => (input, ""),
    };
    let trimmed = first.trim_start_matches([' ', '\t']);
    let Some(json) = trimmed.strip_prefix(PREFIX) else {
        if input.trim().is_empty() {
            return fail("The code is empty: pass Luau source in `code`.");
        }
        return Ok(Source {
            options: Options::default(),
            code: input.to_owned(),
        });
    };
    let options = parse_options(json.trim())?;
    if rest.trim().is_empty() {
        return fail("@options must be followed by code on the next lines.");
    }
    Ok(Source {
        options,
        code: input.to_owned(),
    })
}

fn parse_options(json: &str) -> Result<Options, SourceError> {
    let value: Value = match serde_json::from_str(json) {
        Ok(value) => value,
        Err(error) => {
            return fail(format!("@options is not valid JSON: {error}"));
        }
    };
    let Value::Object(map) = value else {
        return fail("@options must be a JSON object.");
    };
    let mut options = Options::default();
    for (key, value) in map {
        match key.as_str() {
            "max_output_tokens" => {
                let Some(tokens) = value.as_u64() else {
                    return fail(
                        "`max_output_tokens` must be a non-negative integer.",
                    );
                };
                options.max_output_tokens = Some(tokens);
            }
            "timeout_ms" => match value.as_u64() {
                Some(ms) if (1..=MAX_TIMEOUT_MS).contains(&ms) => {
                    options.timeout_ms = Some(ms);
                }
                _ => {
                    return fail(format!(
                        "`timeout_ms` must be a positive integer up to \
                         {MAX_TIMEOUT_MS}."
                    ));
                }
            },
            other => {
                return fail(format!(
                    "@options only supports `max_output_tokens` and \
                     `timeout_ms`; got `{other}`"
                ));
            }
        }
    }
    Ok(options)
}
