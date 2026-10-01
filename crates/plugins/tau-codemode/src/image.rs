//! `image()`: a base64 image, checked and typed by its bytes.
//!
//! As pi does, the MIME type comes from the image's magic bytes, not
//! from what the script says: providers refuse an image whose declared
//! type does not match, and a bad image would be sent again every turn.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::Value;

/// An image a script added to its output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub mime_type: &'static str,
    /// Base64, standard alphabet, padded, no line breaks.
    pub data: String,
}

/// The image `item` describes: a `data:` URL, `{ image_url = url }`, or
/// an MCP ImageContent `{ type = "image", data, mimeType }`.
pub fn parse(item: &Value) -> Result<Image, String> {
    match item {
        Value::String(url) => from_url(url),
        Value::Object(map) => {
            if let Some(url) = map.get("image_url") {
                let Value::String(url) = url else {
                    return Err("image(): `image_url` must be a string".into());
                };
                return from_url(url);
            }
            if map.get("type").and_then(Value::as_str) == Some("image") {
                let Some(data) = map.get("data").and_then(Value::as_str) else {
                    return Err(
                        "image(): an ImageContent needs base64 `data`".into()
                    );
                };
                return from_base64(data);
            }
            Err(usage())
        }
        _ => Err(usage()),
    }
}

fn usage() -> String {
    "image() takes a base64 `data:` URL, `{ image_url = url }`, or an MCP \
     ImageContent such as `result.content[1]`"
        .into()
}

fn from_url(url: &str) -> Result<Image, String> {
    let lower = url.trim_start().to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Err(
            "image() cannot fetch URLs: pass a base64 `data:` URL".into()
        );
    }
    let Some(rest) = url.trim().strip_prefix("data:") else {
        return Err(usage());
    };
    let Some((header, data)) = rest.split_once(',') else {
        return Err("image(): the data: URL has no `,`".into());
    };
    if !header.to_ascii_lowercase().ends_with(";base64") {
        return Err("image(): the data: URL must be base64".into());
    }
    from_base64(data)
}

fn from_base64(data: &str) -> Result<Image, String> {
    let compact: String =
        data.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    let bytes = STANDARD
        .decode(compact.as_bytes())
        .map_err(|error| format!("image(): the data is not base64: {error}"))?;
    let mime_type = sniff(&bytes).ok_or_else(|| {
        "image(): the data is not a PNG, JPEG, GIF or WebP image".to_owned()
    })?;
    Ok(Image {
        mime_type,
        data: STANDARD.encode(&bytes),
    })
}

/// The image type `bytes` start with.
pub fn sniff(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12
        && &bytes[..4] == b"RIFF"
        && &bytes[8..12] == b"WEBP"
    {
        Some("image/webp")
    } else {
        None
    }
}
