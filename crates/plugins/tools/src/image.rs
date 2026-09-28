//! Images for `read` (`docs/reference/tools.md`, "read"), ported from
//! pi's `mime.ts`, `image-process.ts` and `image-resize-core.ts`, with
//! the `image` crate in place of Photon.
//!
//! - The format comes from magic bytes, never the file extension.
//! - PNG, JPEG, GIF and WebP go inline as they are; anything else
//!   (BMP) is converted to PNG first.
//! - An image within 2000×2000 whose base64 is under 4.5 MB goes as it
//!   is. Otherwise it is resized to fit (Lanczos3) and encoded as PNG
//!   and as JPEG at qualities 80, 85, 70, 55 and 40; the first under
//!   the size limit wins, and the size shrinks by a quarter until one
//!   does. EXIF orientation is applied whenever pixels are decoded.

use std::io::Cursor;

use base64::{Engine, engine::general_purpose::STANDARD};
use image::{
    DynamicImage,
    ImageDecoder,
    ImageFormat,
    ImageReader,
    imageops::FilterType,
};

/// The largest width and height sent.
pub const MAX_DIMENSION: u32 = 2000;
/// The largest base64 payload sent: 4.5 MB, below Anthropic's 5 MB.
pub const MAX_BASE64_BYTES: usize = 4_718_592;
/// JPEG qualities to try, in order.
const QUALITIES: [u8; 5] = [80, 85, 70, 55, 40];

/// The image type `bytes` start with, if it is one `read` handles
/// (pi's `detectSupportedImageMimeType`). An animated PNG and a lossless
/// JPEG (`FF D8 FF F7`) are not.
pub fn detect(bytes: &[u8]) -> Option<&'static str> {
    const PNG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return (bytes.get(3) != Some(&0xf7)).then_some("image/jpeg");
    }
    if bytes.starts_with(&PNG) {
        return (is_png(bytes) && !is_animated_png(bytes))
            .then_some("image/png");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        return Some("image/webp");
    }
    if bytes.starts_with(b"BM") && is_bmp(bytes) {
        return Some("image/bmp");
    }
    None
}

/// The `N` bytes at `at`, or `None` past the end.
fn array<const N: usize>(bytes: &[u8], at: usize) -> Option<[u8; N]> {
    bytes.get(at..)?.get(..N)?.try_into().ok()
}

/// Past the end reads as 0, as pi's `buffer[i] ?? 0` does.
fn u16_le(bytes: &[u8], at: usize) -> u32 {
    array(bytes, at).map_or(0, |b| u32::from(u16::from_le_bytes(b)))
}

fn u32_le(bytes: &[u8], at: usize) -> u32 {
    array(bytes, at).map_or(0, u32::from_le_bytes)
}

fn u32_be(bytes: &[u8], at: usize) -> u32 {
    array(bytes, at).map_or(0, u32::from_be_bytes)
}

fn is_png(bytes: &[u8]) -> bool {
    bytes.len() >= 16 && u32_be(bytes, 8) == 13 && &bytes[12..16] == b"IHDR"
}

/// An `acTL` chunk before the first `IDAT` marks an animated PNG.
fn is_animated_png(bytes: &[u8]) -> bool {
    let mut offset = 8usize;
    while let Some(header) = array::<8>(bytes, offset) {
        match &header[4..] {
            b"acTL" => return true,
            b"IDAT" => return false,
            _ => {}
        }
        // Length, type, data and CRC; past the end, the loop ends.
        let length = u32_be(&header, 0) as usize;
        offset = offset.saturating_add(12).saturating_add(length);
    }
    false
}

fn is_bmp(bytes: &[u8]) -> bool {
    if bytes.len() < 26 {
        return false;
    }
    let file_size = u32_le(bytes, 2);
    let pixel_offset = u32_le(bytes, 10);
    let dib_size = u32_le(bytes, 14);
    // pi also rejects a file size below 26; the pixel data check below
    // already does, since the pixel offset is at least 26.
    if pixel_offset < 14u32.saturating_add(dib_size) {
        return false;
    }
    if file_size != 0 && pixel_offset >= file_size {
        return false;
    }
    let (planes, bits) = match dib_size {
        12 => (u16_le(bytes, 22), u16_le(bytes, 24)),
        40..=124 => (u16_le(bytes, 26), u16_le(bytes, 28)),
        _ => return false,
    };
    planes == 1 && [1, 4, 8, 16, 24, 32].contains(&bits)
}

/// An image ready to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Processed {
    /// Base64 of the image bytes.
    pub data: String,
    pub mime_type: String,
    /// Notes for the model: a conversion, a resize.
    pub hints: Vec<String>,
}

/// Why an image could not be sent. The message is pi's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Omitted(pub &'static str);

fn decode(bytes: &[u8]) -> Option<DynamicImage> {
    let mut decoder = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_decoder()
        .ok()?;
    let orientation = decoder.orientation().ok()?;
    let mut image = DynamicImage::from_decoder(decoder).ok()?;
    image.apply_orientation(orientation);
    Some(image)
}

fn encode(
    image: &DynamicImage,
    format: ImageFormat,
    quality: u8,
) -> Option<Vec<u8>> {
    let mut out = Cursor::new(Vec::new());
    match format {
        ImageFormat::Jpeg => {
            let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(
                &mut out, quality,
            );
            image.to_rgb8().write_with_encoder(encoder).ok()?;
        }
        _ => image.write_to(&mut out, format).ok()?,
    }
    Some(out.into_inner())
}

fn base64_len(bytes: usize) -> usize {
    bytes.div_ceil(3) * 4
}

/// Whether `bytes` bytes stay under the base64 limit (strictly, as pi's
/// `<`).
fn fits(bytes: usize) -> bool {
    base64_len(bytes) < MAX_BASE64_BYTES
}

/// Prepares an image of type `mime_type` (from [`detect`]) to send
/// inline (pi's `processImage` with resizing on).
pub fn process(bytes: &[u8], mime_type: &str) -> Result<Processed, Omitted> {
    let mut hints = Vec::new();
    let (bytes, mime_type) = match mime_type {
        "image/png" | "image/jpeg" | "image/gif" | "image/webp" => {
            (bytes.to_vec(), mime_type.to_owned())
        }
        other => {
            let converted = decode(bytes)
                .and_then(|image| encode(&image, ImageFormat::Png, 0))
                .ok_or(Omitted(
                    "[Image omitted: could not be converted to a supported inline image format.]",
                ))?;
            hints.push(format!("[Image converted from {other} to image/png.]"));
            (converted, "image/png".to_owned())
        }
    };
    let too_big = Omitted(
        "[Image omitted: could not be resized below the inline image size limit.]",
    );
    let image = decode(&bytes).ok_or(too_big.clone())?;
    let (width, height) = (image.width(), image.height());
    if width <= MAX_DIMENSION && height <= MAX_DIMENSION && fits(bytes.len()) {
        return Ok(Processed {
            data: STANDARD.encode(&bytes),
            mime_type,
            hints,
        });
    }

    let (mut w, mut h) = fit(width, height);
    loop {
        let resized = image.resize_exact(w, h, FilterType::Lanczos3);
        let candidates = std::iter::once((ImageFormat::Png, 0))
            .chain(QUALITIES.iter().map(|&q| (ImageFormat::Jpeg, q)));
        for (format, quality) in candidates {
            let Some(encoded) = encode(&resized, format, quality) else {
                continue;
            };
            if fits(encoded.len()) {
                hints.push(format!(
                    "[Image: original {width}x{height}, displayed at {w}x{h}. Multiply coordinates by {:.2} to map to original image.]",
                    f64::from(width) / f64::from(w)
                ));
                let mime_type = match format {
                    ImageFormat::Jpeg => "image/jpeg",
                    _ => "image/png",
                };
                return Ok(Processed {
                    data: STANDARD.encode(&encoded),
                    mime_type: mime_type.to_owned(),
                    hints,
                });
            }
        }
        match shrink(w, h) {
            Some(next) => (w, h) = next,
            None => return Err(too_big),
        }
    }
}

/// The size an image of `width`×`height` is first resized to: within
/// [`MAX_DIMENSION`] on both sides, aspect ratio kept, at least 1×1.
fn fit(width: u32, height: u32) -> (u32, u32) {
    let (mut w, mut h) = (f64::from(width), f64::from(height));
    let max = f64::from(MAX_DIMENSION);
    if w > max {
        h = (h * max / w).round();
        w = max;
    }
    if h > max {
        w = (w * max / h).round();
        h = max;
    }
    ((w as u32).max(1), (h as u32).max(1))
}

/// The next smaller size to try: three quarters on each side, never
/// below 1. `None` once the image is 1×1.
fn shrink(w: u32, h: u32) -> Option<(u32, u32)> {
    if w == 1 && h == 1 {
        return None;
    }
    let side = |n: u32| if n == 1 { 1 } else { (n * 3 / 4).max(1) };
    Some((side(w), side(h)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wide, tall and doubly oversized images fit within 2000 on both
    /// sides with their aspect ratio; small ones are left alone.
    #[test]
    fn images_fit_within_the_maximum() {
        assert_eq!(fit(4000, 1000), (2000, 500));
        assert_eq!(fit(1000, 4000), (500, 2000));
        assert_eq!(fit(6000, 3000), (2000, 1000));
        assert_eq!(fit(3000, 6000), (1000, 2000));
        assert_eq!(fit(2001, 3), (2000, 3));
        assert_eq!(fit(2000, 2000), (2000, 2000));
        assert_eq!(fit(1999, 7), (1999, 7));
        assert_eq!(fit(9000, 1), (2000, 1));
    }

    /// Shrinking takes three quarters of each side, keeps a side of 1,
    /// and stops at 1×1.
    #[test]
    fn shrinking_takes_three_quarters_until_one_by_one() {
        assert_eq!(shrink(2000, 1000), Some((1500, 750)));
        assert_eq!(shrink(1, 8), Some((1, 6)));
        assert_eq!(shrink(8, 1), Some((6, 1)));
        assert_eq!(shrink(2, 3), Some((1, 2)));
        assert_eq!(shrink(1, 1), None);
    }

    /// The limit is strict: a payload whose base64 is exactly 4.5 MB does
    /// not fit, one three bytes smaller does.
    #[test]
    fn the_size_limit_is_strict() {
        assert!(fits(3_538_941));
        assert!(!fits(3_538_942));
        assert!(!fits(3_538_944));
    }

    /// JPEG candidates use the quality asked for: a lower quality gives
    /// a smaller file.
    #[test]
    fn jpeg_quality_is_used() {
        let image = DynamicImage::ImageRgb8(image::RgbImage::from_fn(
            64,
            64,
            |x, y| {
                image::Rgb([
                    (x * 4) as u8,
                    (y * 4) as u8,
                    ((x * y) % 256) as u8,
                ])
            },
        ));
        let low = encode(&image, ImageFormat::Jpeg, 10).unwrap();
        let high = encode(&image, ImageFormat::Jpeg, 95).unwrap();
        assert!(low.len() < high.len(), "{} vs {}", low.len(), high.len());
    }

    /// Base64 takes four characters per three bytes, rounded up; the
    /// limit is strict, as pi's `<`.
    #[test]
    fn base64_sizes() {
        assert_eq!(base64_len(0), 0);
        assert_eq!(base64_len(1), 4);
        assert_eq!(base64_len(3), 4);
        assert_eq!(base64_len(4), 8);
        assert_eq!(MAX_BASE64_BYTES, 4_718_592);
    }
}
