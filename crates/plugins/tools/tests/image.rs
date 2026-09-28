//! Image detection and processing (`tau_tools::image`), ported from
//! pi's `mime.ts` and `image-resize-core.ts`.

use std::io::Cursor;

use base64::{Engine, engine::general_purpose::STANDARD};
use image::{DynamicImage, ImageFormat, RgbImage};
use tau_tools::image::{MAX_BASE64_BYTES, detect, process};

fn encoded(image: DynamicImage, format: ImageFormat) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    image.write_to(&mut out, format).unwrap();
    out.into_inner()
}

/// A PNG chunk: length, type, data, and a (here meaningless) CRC.
fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut c = (data.len() as u32).to_be_bytes().to_vec();
    c.extend_from_slice(kind);
    c.extend_from_slice(data);
    c.extend_from_slice(&[0; 4]);
    c
}

fn png_header() -> Vec<u8> {
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    png.extend(chunk(b"IHDR", &[0; 13]));
    png
}

/// A BMP header: file size, pixel data offset, DIB header size, then
/// planes and bits per pixel where that header kind keeps them.
fn bmp(
    file_size: u32,
    pixel_offset: u32,
    dib: u32,
    planes: u16,
    bits: u16,
) -> Vec<u8> {
    let mut b = b"BM".to_vec();
    b.extend_from_slice(&file_size.to_le_bytes());
    b.extend_from_slice(&[0; 4]);
    b.extend_from_slice(&pixel_offset.to_le_bytes());
    b.extend_from_slice(&dib.to_le_bytes());
    if dib == 12 {
        b.extend_from_slice(&[0; 4]);
        b.extend_from_slice(&planes.to_le_bytes());
        b.extend_from_slice(&bits.to_le_bytes());
    } else {
        b.extend_from_slice(&[0; 8]);
        b.extend_from_slice(&planes.to_le_bytes());
        b.extend_from_slice(&bits.to_le_bytes());
    }
    b.resize(b.len().max(64), 0);
    b
}

/// Magic bytes decide the type, header by header, as pi's
/// `detectSupportedImageMimeType` does: real images of every format;
/// a PNG signature with or without a proper `IHDR`; an animated PNG
/// (`acTL` before `IDAT`) against a still one (`acTL` after); a lossless
/// JPEG; GIF87a and GIF89a; RIFF that is and is not WebP; and BMP
/// headers that are valid or break one rule each.
#[test]
fn detection_by_magic_bytes() {
    let pixel = || DynamicImage::ImageRgb8(RgbImage::new(2, 2));
    assert_eq!(
        detect(&encoded(pixel(), ImageFormat::Png)),
        Some("image/png")
    );
    assert_eq!(
        detect(&encoded(pixel(), ImageFormat::Jpeg)),
        Some("image/jpeg")
    );
    assert_eq!(
        detect(&encoded(pixel(), ImageFormat::Gif)),
        Some("image/gif")
    );
    assert_eq!(
        detect(&encoded(pixel(), ImageFormat::Bmp)),
        Some("image/bmp")
    );
    assert_eq!(detect(b"GIF87a..."), Some("image/gif"));
    assert_eq!(detect(b"GIF88a..."), None);
    assert_eq!(detect(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
    assert_eq!(detect(b"RIFF\0\0\0\0WAVEfmt "), None);
    assert_eq!(detect(b"RIFX\0\0\0\0WEBPVP8 "), None);
    assert_eq!(detect(&[0xff, 0xd8, 0xff, 0xe0]), Some("image/jpeg"));
    assert_eq!(detect(&[0xff, 0xd8, 0xff, 0xf7]), None, "lossless JPEG");
    assert_eq!(detect(b"plain text"), None);
    assert_eq!(detect(&[]), None);

    let still = png_header();
    assert_eq!(detect(&still), Some("image/png"));
    let mut animated = png_header();
    animated.extend(chunk(b"acTL", &[0; 8]));
    animated.extend(chunk(b"IDAT", &[0; 4]));
    assert_eq!(detect(&animated), None);
    let mut late = png_header();
    late.extend(chunk(b"IDAT", &[0; 4]));
    late.extend(chunk(b"acTL", &[0; 8]));
    assert_eq!(detect(&late), Some("image/png"));
    let mut cut = png_header();
    cut.extend_from_slice(&[0, 0, 0, 200, b'a', b'c']);
    assert_eq!(
        detect(&cut),
        Some("image/png"),
        "a cut chunk list is still a PNG"
    );
    let mut long = png_header();
    long.extend_from_slice(&[0xff, 0xff, 0xff, 0xff, b'z', b'z', b'z', b'z']);
    assert_eq!(detect(&long), Some("image/png"));
    let mut bad_ihdr = png_header();
    bad_ihdr[12..16].copy_from_slice(b"IHDX");
    assert_eq!(detect(&bad_ihdr), None);
    let mut bad_length = png_header();
    bad_length[11] = 12;
    assert_eq!(detect(&bad_length), None);
    assert_eq!(detect(&png_header()[..15]), None, "too short for IHDR");

    assert_eq!(detect(&bmp(100, 54, 40, 1, 24)), Some("image/bmp"));
    assert_eq!(
        detect(&bmp(0, 54, 40, 1, 24)),
        Some("image/bmp"),
        "size 0 is allowed"
    );
    assert_eq!(
        detect(&bmp(100, 26, 12, 1, 8)),
        Some("image/bmp"),
        "OS/2 header"
    );
    assert_eq!(
        detect(&bmp(100, 54, 124, 1, 32)),
        None,
        "offset inside the header"
    );
    assert_eq!(detect(&bmp(200, 138, 124, 1, 32)), Some("image/bmp"));
    assert_eq!(detect(&bmp(25, 54, 40, 1, 24)), None, "file size below 26");
    assert_eq!(detect(&bmp(26, 54, 40, 1, 24)), None, "pixels past the end");
    assert_eq!(detect(&bmp(54, 54, 40, 1, 24)), None, "pixels at the end");
    assert_eq!(detect(&bmp(100, 53, 40, 1, 24)), None, "offset too small");
    assert_eq!(detect(&bmp(100, 54, 40, 2, 24)), None, "two planes");
    assert_eq!(detect(&bmp(100, 54, 40, 1, 3)), None, "3 bits");
    assert_eq!(detect(&bmp(100, 54, 20, 1, 24)), None, "unknown header");
    assert_eq!(detect(&bmp(100, 54, 40, 1, 24)[..25]), None, "too short");
    assert_eq!(detect(&bmp(100, 54, 40, 1, 24)[..29]), None, "cut header");
    assert_eq!(
        detect(&bmp(100, 26, 12, 1, 8)[..26]),
        Some("image/bmp"),
        "an OS/2 header needs only 26 bytes"
    );
    assert_eq!(detect(&bmp(20, 26, 12, 1, 8)), None, "a file too small");
    for bits in [1, 4, 8, 16, 32] {
        assert_eq!(
            detect(&bmp(100, 54, 40, 1, bits)),
            Some("image/bmp"),
            "{bits} bits"
        );
    }
}

/// An image within the size limits but whose base64 is over 4.5 MB (a
/// 1500×1500 PNG of noise) is re-encoded: PNG stays too big, JPEG fits,
/// and the note says the size is unchanged.
#[test]
fn an_oversized_payload_is_re_encoded_as_jpeg() {
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let noise = RgbImage::from_fn(1500, 1500, |_, _| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        image::Rgb([seed as u8, (seed >> 8) as u8, (seed >> 16) as u8])
    });
    let png = encoded(DynamicImage::ImageRgb8(noise), ImageFormat::Png);
    assert!(png.len().div_ceil(3) * 4 >= MAX_BASE64_BYTES);
    let processed = process(&png, "image/png").unwrap();
    assert_eq!(processed.mime_type, "image/jpeg");
    assert!(processed.data.len() < MAX_BASE64_BYTES);
    let bytes = STANDARD.decode(&processed.data).unwrap();
    assert_eq!(detect(&bytes), Some("image/jpeg"));
    assert_eq!(
        processed.hints,
        vec![
            "[Image: original 1500x1500, displayed at 1500x1500. Multiply coordinates by 1.00 to map to original image.]"
        ]
    );
}

/// A tall image is fitted to 2000 high, and a GIF within the limits goes
/// as it is.
#[test]
fn tall_images_and_small_gifs() {
    let tall = encoded(
        DynamicImage::ImageRgb8(RgbImage::new(10, 4000)),
        ImageFormat::Png,
    );
    let processed = process(&tall, "image/png").unwrap();
    let image =
        image::load_from_memory(&STANDARD.decode(&processed.data).unwrap())
            .unwrap();
    assert_eq!((image.width(), image.height()), (5, 2000));
    assert_eq!(
        processed.hints,
        vec![
            "[Image: original 10x4000, displayed at 5x2000. Multiply coordinates by 2.00 to map to original image.]"
        ]
    );

    let gif = encoded(
        DynamicImage::ImageRgb8(RgbImage::new(3, 3)),
        ImageFormat::Gif,
    );
    let processed = process(&gif, "image/gif").unwrap();
    assert_eq!(processed.mime_type, "image/gif");
    assert_eq!(STANDARD.decode(&processed.data).unwrap(), gif);
    assert!(processed.hints.is_empty());
}

/// Bytes that are not an image are omitted with pi's messages.
#[test]
fn undecodable_images_are_omitted() {
    assert_eq!(
        process(b"BMnot really", "image/bmp").unwrap_err().0,
        "[Image omitted: could not be converted to a supported inline image format.]"
    );
    assert_eq!(
        process(b"\x89PNG broken", "image/png").unwrap_err().0,
        "[Image omitted: could not be resized below the inline image size limit.]"
    );
}
