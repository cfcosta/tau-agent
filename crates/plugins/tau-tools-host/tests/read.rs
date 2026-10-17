//! `read` (`tau_tools_host::read`), on real files (`docs/reference/tools.md`,
//! "read"; `docs/reference/testing.md`, "tau-tools").

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

mod common;

use std::io::Cursor;

use base64::{Engine, engine::general_purpose::STANDARD};
use hegel::{TestCase, generators as gs};
use image::{DynamicImage, ImageFormat, RgbImage};
use serde_json::{Value, json};
use tau_agent::{
    error::ToolError,
    tool::{AgentTool, ToolCtx, ToolOutput},
};
use tau_ai::message::InputBlock;
use tau_tools_host::{
    image::{MAX_BASE64_BYTES, MAX_DIMENSION},
    path::Root,
    read::Read,
    truncate::{Limit, MAX_BYTES, MAX_LINES, truncate_head},
};
use tokio_util::sync::CancellationToken;

/// Calls `read` on a normal runtime: it does blocking I/O on the
/// blocking pool, which paused time would treat as idle.
fn call(root: &Root, args: Value) -> Result<ToolOutput, ToolError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(Read::new(root.clone()).call(
            args,
            ToolCtx::detached().cancelled_by(CancellationToken::new()),
        ))
}

fn text_of(output: &ToolOutput) -> String {
    match &output.content[0] {
        InputBlock::Text(text) => text.text.clone(),
        other => panic!("expected text, got {other:?}"),
    }
}

/// pi's `formatSize` (`truncate.ts`): `toFixed(1)` takes the nearest
/// tenth, and the larger one at a tie.
fn pi_format_size(bytes: usize) -> String {
    let fixed = |unit: f64| {
        // Exact: the sizes here are far below 2^53, and the unit is a
        // power of two.
        let scaled = bytes as f64 * 10.0 / unit;
        let tenths =
            scaled.floor() + f64::from(u8::from(scaled.fract() >= 0.5));
        format!("{:.1}", tenths / 10.0)
    };
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{}KB", fixed(1024.0))
    } else {
        format!("{}MB", fixed(1024.0 * 1024.0))
    }
}

/// pi's `read` over the whole file in memory (`read.ts`), the oracle for
/// the streaming reader.
fn pi_read(
    content: &str,
    path: &str,
    offset: Option<u64>,
    limit: Option<u64>,
) -> Result<String, String> {
    let all: Vec<&str> = content.split('\n').collect();
    let start = offset.map_or(0, |o| o.saturating_sub(1)) as usize;
    if start >= all.len() {
        return Err(format!(
            "Offset {} is beyond end of file ({} lines total)",
            offset.unwrap_or(0),
            all.len()
        ));
    }
    let end = limit.map(|l| (start + l as usize).min(all.len()));
    let selected = all[start..end.unwrap_or(all.len())].join("\n");
    let t = truncate_head(&selected, MAX_LINES, MAX_BYTES);
    let first = start + 1;
    Ok(if t.first_line_exceeds_limit {
        format!(
            "[Line {first} is {}, exceeds {} limit. Use exec_command: sed -n '{first}p' {path} | head -c {MAX_BYTES}]",
            pi_format_size(all[start].len()),
            pi_format_size(MAX_BYTES)
        )
    } else if t.truncated() {
        let last = first + t.output_lines - 1;
        match t.by {
            Some(Limit::Lines) => format!(
                "{}\n\n[Showing lines {first}-{last} of {}. Use offset={} to continue.]",
                t.content,
                all.len(),
                last + 1
            ),
            _ => format!(
                "{}\n\n[Showing lines {first}-{last} of {} ({} limit). Use offset={} to continue.]",
                t.content,
                all.len(),
                pi_format_size(MAX_BYTES),
                last + 1
            ),
        }
    } else {
        match end {
            Some(end) if end < all.len() => format!(
                "{}\n\n[{} more lines in file. Use offset={} to continue.]",
                t.content,
                all.len() - end,
                end + 1
            ),
            _ => t.content,
        }
    })
}

/// A text file with lines drawn from a few lengths, some long enough
/// that the byte limit or a first line over it comes up, and enough of
/// them that the line limit does. Some lines end in `\r` (CRLF), carry
/// a byte that is not UTF-8 or a multi-byte sequence cut short, and the
/// file may start with a byte order mark.
#[hegel::composite]
fn file(tc: &TestCase) -> Vec<u8> {
    // Without the long lengths, a file over the line limit stays under
    // the byte limit, so the line cut comes first.
    let long = tc.draw(gs::weighted_booleans(0.3));
    let lengths = || {
        gs::sampled_from(if long {
            vec![0usize, 1, 7, 40, 300, 60_000]
        } else {
            vec![0usize, 1, 7, 40]
        })
    };
    // Often right at the 2000-line limit, which a uniform draw rarely
    // reaches.
    let boundary =
        || gs::sampled_from(vec![0usize, 1, 1_999, 2_000, 2_001, 2_600]);
    let count = tc.draw(hegel::one_of!(
        boundary(),
        boundary(),
        gs::integers::<usize>().max_value(2_600),
    ));
    let chars = ["a", "é", "€", "😀"];
    let extras: [&[u8]; 6] =
        [b"", b"\r", b"\xff", b"\xc3", b"\xe2\x82", b"\x80\r"];
    let dirty = tc.draw(gs::weighted_booleans(0.6));
    let mut text = Vec::new();
    if tc.draw(gs::weighted_booleans(0.2)) {
        text.extend_from_slice("\u{FEFF}".as_bytes());
    }
    for i in 0..count {
        if i > 0 {
            text.push(b'\n');
        }
        let len = if i % 97 == 0 {
            tc.draw(lengths())
        } else {
            i % 13
        };
        text.extend_from_slice(chars[i % chars.len()].repeat(len).as_bytes());
        if dirty && i % 7 == 0 {
            text.extend_from_slice(tc.draw(gs::sampled_from(&extras[..])));
        }
    }
    if tc.draw(gs::booleans()) {
        text.push(b'\n');
    }
    text
}

/// `read` with any `offset` and `limit` equals pi's read of the same
/// file held whole in memory and decoded as UTF-8 (invalid bytes
/// replaced, `\r` and a byte order mark kept): the same lines, the same
/// truncation and the same notices, or the same error.
#[hegel::test(test_cases = 50)]
fn read_equals_pis_whole_file_read(tc: TestCase) {
    let bytes = tc.draw(file());
    let content = String::from_utf8_lossy(&bytes);
    if std::str::from_utf8(&bytes).is_err() {
        tc.event("not valid UTF-8");
    }
    if content.contains('\r') {
        tc.event("CRLF lines");
    }
    let total = content.split('\n').count() as u64;
    // From the start, or at a line drawn up to past the end; with no
    // limit, a limit past the line cut, or any limit.
    let offset = tc.draw(gs::optional(hegel::one_of!(
        gs::just(1u64),
        gs::integers::<u64>().max_value(total + 2),
    )));
    let limit = tc.draw(gs::optional(hegel::one_of!(
        gs::sampled_from(vec![2_000u64, 2_001, 2_500]),
        gs::integers::<u64>().max_value(2_500),
    )));
    assert_reads_as_pi(&bytes, offset, limit);
}

/// `read` of `bytes` with `offset` and `limit` is pi's read of them
/// decoded whole.
fn assert_reads_as_pi(bytes: &[u8], offset: Option<u64>, limit: Option<u64>) {
    let content = String::from_utf8_lossy(bytes);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), bytes).unwrap();
    let root = Root::new(dir.path());
    let mut args = json!({"path": "f.txt"});
    if let Some(offset) = offset {
        args["offset"] = json!(offset);
    }
    if let Some(limit) = limit {
        args["limit"] = json!(limit);
    }
    let got = call(&root, args).map_err(|e| e.to_string());
    if let Ok(output) = &got {
        assert_matches_output_schema(output);
    }
    let got = got.map(|o| text_of(&o));
    assert_eq!(got, pi_read(&content, "f.txt", offset, limit));
}

/// Short pieces of text: multi-byte characters, and bytes that decode
/// to U+FFFD (a stray byte, sequences cut short).
const PIECES: [&[u8]; 9] = [
    b"a",
    "\u{e9}".as_bytes(),
    "\u{20ac}".as_bytes(),
    "\u{1f600}".as_bytes(),
    b"\xff",
    b"\xc3",
    b"\xe2\x82",
    b"\xf0\x9f\x98",
    b"\xed\xa0\x80",
];

/// A line of a few drawn pieces, `""` included.
#[hegel::composite]
fn short_line(tc: &TestCase, valid: bool) -> Vec<u8> {
    let pieces = if valid { &PIECES[..4] } else { &PIECES[..] };
    let count = tc.draw(gs::integers::<usize>().max_value(3));
    (0..count)
        .flat_map(|_| tc.draw(gs::sampled_from(pieces)).to_vec())
        .collect()
}

/// A line that is often empty.
#[hegel::composite]
fn empty_or_short_line(tc: &TestCase) -> Vec<u8> {
    tc.draw(hegel::one_of!(gs::just(Vec::new()), short_line(true)))
}

/// A line of `width` bytes of `piece`, filled out with `a` where the
/// piece does not divide the width.
fn line_of(piece: &[u8], width: usize) -> Vec<u8> {
    let mut line = b"a".repeat(width % piece.len());
    line.extend(piece.repeat(width / piece.len()));
    line
}

/// Text cut right at an edge of `read`'s truncation, after a few lines
/// before `offset`:
/// - lines: 2000 lines, then the 2001st, often empty;
/// - bytes: lines of multi-byte text that end exactly 0 to 4 bytes
///   past 50 KiB, then a line that may be empty;
/// - first line: a first line at 50 KiB decoded, just over, or at a
///   size whose tenth of a KiB is a tie, with bytes that decode to
///   U+FFFD and maybe a multi-byte character across the 50 KiB + 1
///   bytes kept of it.
///
/// Then up to two more short lines, and maybe a trailing `\n`.
#[hegel::composite]
fn edge_file(tc: &TestCase) -> (Vec<u8>, u64) {
    let before = tc.draw(gs::sampled_from(vec![0usize, 1, 249]));
    let mut lines: Vec<Vec<u8>> = (0..before).map(|_| b"x".to_vec()).collect();
    let piece = || gs::sampled_from(&PIECES[1..4]);
    match tc.draw(gs::integers::<u8>().max_value(2)) {
        0 => {
            tc.event("cut by lines");
            for _ in 0..MAX_LINES {
                lines.push(tc.draw(short_line(true)));
            }
            lines.push(tc.draw(empty_or_short_line()));
        }
        1 => {
            tc.event("cut by bytes");
            // `count` lines of one width, joined, are MAX_BYTES - 1.
            let count = tc.draw(gs::sampled_from(vec![1usize, 2, 50]));
            let width = (MAX_BYTES - 1 - (count - 1)) / count;
            let filler = tc.draw(piece());
            for _ in 0..count {
                lines.push(line_of(filler, width));
            }
            let past = tc.draw(gs::integers::<usize>().max_value(4));
            lines.push(line_of(tc.draw(piece()), past));
            lines.push(tc.draw(empty_or_short_line()));
        }
        _ => {
            tc.event("first line at or over the limit");
            // Decoded sizes: at the limit, just over, and ties (50.25
            // KiB, 60.25 KiB, 1.25 MiB).
            let size = tc.draw(gs::sampled_from(vec![
                MAX_BYTES,
                MAX_BYTES + 1,
                MAX_BYTES + 2,
                MAX_BYTES + 256,
                60 * 1024 + 256,
                (1 << 20) + (1 << 18),
            ]));
            let decoded = |bytes: &[u8]| String::from_utf8_lossy(bytes).len();
            let mut line = tc.draw(short_line(false));
            let tail = tc.draw(short_line(false));
            if tc.draw(gs::booleans()) {
                // A character across the MAX_BYTES + 1 bytes kept.
                let shift =
                    tc.draw(gs::integers::<usize>().min_value(1).max_value(3));
                let width = (MAX_BYTES + 1 - shift).saturating_sub(line.len());
                line.extend(b"a".repeat(width));
                line.extend(tc.draw(piece()));
            }
            let short = size.saturating_sub(decoded(&line) + decoded(&tail));
            line.extend(b"a".repeat(short));
            line.extend(tail);
            lines.push(line);
        }
    }
    for _ in 0..tc.draw(gs::integers::<usize>().max_value(2)) {
        lines.push(tc.draw(empty_or_short_line()));
    }
    let mut text = lines.join(&b'\n');
    if tc.draw(gs::booleans()) {
        text.push(b'\n');
    }
    (text, before as u64)
}

/// `read` equals pi's read where its cut is closest: the line or byte
/// limit reached right at the end of a line, the line that goes over
/// empty (pi does not count an empty last line), and a first line over
/// the limit whose size, decoded, is reported. From the first line
/// after those before `offset`, with no limit or one right around the
/// lines there are.
#[hegel::test(test_cases = 100)]
fn read_equals_pis_read_at_the_truncation_edges(tc: TestCase) {
    let (bytes, before) = tc.draw(edge_file());
    let total = bytes.split(|&b| b == b'\n').count() as u64;
    let offset = if before == 0 {
        tc.draw(gs::sampled_from(vec![None, Some(0), Some(1)]))
    } else {
        Some(before + 1)
    };
    let from = offset.map_or(1, |o| o.max(1));
    let left = total - (from - 1);
    let limit = tc.draw(gs::optional(gs::sampled_from(vec![
        left.saturating_sub(1),
        left,
        left + 1,
        2_000,
        2_001,
    ])));
    assert_reads_as_pi(&bytes, offset, limit);
}

/// `output` carries a structured result `read`'s output schema accepts
/// as it is.
fn assert_matches_output_schema(output: &ToolOutput) {
    let structured = output.structured.as_ref().expect("a structured result");
    common::assert_matches_output_schema(
        &Read::new(Root::new("/")),
        structured,
    );
}

/// The error strings of `read`: an offset past the end, and a missing
/// file as Node reports it.
#[test]
fn read_error_strings() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), "one\ntwo").unwrap();
    let root = Root::new(dir.path());
    let error = call(&root, json!({"path": "f.txt", "offset": 5})).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Offset 5 is beyond end of file (2 lines total)"
    );
    let error = call(&root, json!({"path": "nope.txt"})).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "ENOENT: no such file or directory, open '{}'",
            dir.path().join("nope.txt").display()
        )
    );
    let error = call(&root, json!({"path": "."})).unwrap_err();
    assert!(error.to_string().starts_with("EISDIR: "), "{error}");
}

/// A cancelled read fails with "Operation aborted".
#[test]
fn a_cancelled_read_is_aborted() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), "x").unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let error = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(Read::new(Root::new(dir.path())).call(
            json!({"path": "f.txt"}),
            ToolCtx::detached().cancelled_by(cancel),
        ))
        .unwrap_err();
    assert_eq!(error.to_string(), "Operation aborted");
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let image = RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x % 251) as u8, (y % 241) as u8, ((x ^ y) % 239) as u8])
    });
    let mut out = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(image)
        .write_to(&mut out, ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

fn image_of(output: &ToolOutput) -> (String, DynamicImage) {
    let InputBlock::Image(image) = &output.content[1] else {
        panic!("expected an image block: {:?}", output.content)
    };
    let bytes = STANDARD.decode(&image.data).unwrap();
    assert!(image.data.len() < MAX_BASE64_BYTES);
    (
        image.mime_type.clone(),
        image::load_from_memory(&bytes).unwrap(),
    )
}

/// Images, for a generated size: a PNG built in the test comes back
/// within 2000×2000 and 4.5 MB of base64 with its aspect ratio kept,
/// and is recognized by its bytes though its name says `.txt`.
#[hegel::test(test_cases = 10)]
fn images_come_back_within_the_limits(tc: TestCase) {
    let width = tc.draw(gs::integers::<u32>().min_value(1).max_value(3_000));
    let height = tc.draw(gs::integers::<u32>().min_value(1).max_value(600));
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("picture.txt"), png(width, height)).unwrap();
    let output =
        call(&Root::new(dir.path()), json!({"path": "picture.txt"})).unwrap();
    assert_matches_output_schema(&output);
    let (_, image) = image_of(&output);
    assert!(image.width() <= MAX_DIMENSION && image.height() <= MAX_DIMENSION);
    let (w, h) = (image.width() as f64, image.height() as f64);
    let expected = height as f64 * w / width as f64;
    assert!(
        (h - expected).abs() <= 1.0,
        "{width}x{height} became {w}x{h}"
    );
    let note = text_of(&output);
    if width > MAX_DIMENSION {
        assert!(
            note.contains(&format!(
                "[Image: original {width}x{height}, displayed at"
            )),
            "{note}"
        );
    } else {
        assert_eq!(note, "Read image file [image/png]");
    }
}

/// pi's known image cases (`tools.test.ts:49`, `:200`): a 1×1 BMP is
/// converted to PNG and says so, and a PNG named `.jpg` is read as PNG.
#[test]
fn image_known_cases() {
    let dir = tempfile::tempdir().unwrap();
    let root = Root::new(dir.path());
    let mut bmp = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(RgbImage::new(1, 1))
        .write_to(&mut bmp, ImageFormat::Bmp)
        .unwrap();
    std::fs::write(dir.path().join("dot.bmp"), bmp.into_inner()).unwrap();
    let output = call(&root, json!({"path": "dot.bmp"})).unwrap();
    assert_eq!(
        text_of(&output),
        "Read image file [image/png]\n[Image converted from image/bmp to image/png.]"
    );
    assert_eq!(image_of(&output).0, "image/png");
    let structured = output.structured.as_ref().unwrap();
    assert_eq!(structured["kind"], "image");
    assert_eq!(structured["path"], "dot.bmp");
    assert_eq!(structured["omitted"], false);
    assert_eq!(
        structured["content"],
        serde_json::to_value(&output.content).unwrap()
    );
    let data = STANDARD
        .decode(structured["content"][1]["data"].as_str().unwrap())
        .unwrap();
    assert_eq!(tau_tools_host::image::detect(&data), Some("image/png"));

    std::fs::write(dir.path().join("wrong.jpg"), png(3, 2)).unwrap();
    let output = call(&root, json!({"path": "wrong.jpg"})).unwrap();
    assert_eq!(image_of(&output).0, "image/png");
}

/// A JPEG whose EXIF orientation comes after an XMP segment
/// (`image-processing.test.ts:75`): when it is decoded to be resized,
/// the orientation is applied, so a 2400×1000 image turned a quarter
/// comes back portrait.
#[test]
fn exif_orientation_after_xmp_is_applied() {
    let mut jpeg = Cursor::new(Vec::new());
    let wide = DynamicImage::ImageRgb8(RgbImage::new(2400, 1000));
    wide.write_to(&mut jpeg, ImageFormat::Jpeg).unwrap();
    let jpeg = jpeg.into_inner();
    // APP1 XMP, then APP1 Exif with orientation 6 (rotate 90° clockwise).
    let xmp = b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta/>";
    let mut exif = b"Exif\0\0MM\0\x2a\0\0\0\x08\0\x01".to_vec();
    exif.extend_from_slice(&[
        0x01, 0x12, 0x00, 0x03, 0, 0, 0, 1, 0, 6, 0, 0, 0, 0, 0, 0,
    ]);
    let segment = |payload: &[u8]| {
        let mut s = vec![0xff, 0xe1];
        s.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
        s.extend_from_slice(payload);
        s
    };
    let mut bytes = jpeg[..2].to_vec();
    bytes.extend(segment(xmp));
    bytes.extend(segment(&exif));
    bytes.extend_from_slice(&jpeg[2..]);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("photo.jpg"), bytes).unwrap();
    let output =
        call(&Root::new(dir.path()), json!({"path": "photo.jpg"})).unwrap();
    let (_, image) = image_of(&output);
    assert!(
        image.height() > image.width(),
        "{}x{}",
        image.width(),
        image.height()
    );
    assert!(
        text_of(&output).contains("original 1000x2400"),
        "{}",
        text_of(&output)
    );
}

/// The tool's name, description and parameters are pi's.
#[test]
fn read_describes_itself_as_pi() {
    let read = Read::new(Root::new("/"));
    assert_eq!(read.name(), "read");
    assert_eq!(
        read.description(),
        "Read the contents of a file. Supports text files and images (jpg, \
png, gif, webp, bmp). Images are sent as attachments. For text files, \
output is truncated to 2000 lines or 50KB (whichever is hit first). Use \
offset/limit for large files. When you need the full file, continue with \
offset until complete."
    );
    let properties = &read.parameters()["properties"];
    assert_eq!(
        properties["path"]["description"],
        "Path to the file to read (relative or absolute)"
    );
    assert_eq!(
        properties["offset"]["description"],
        "Line number to start reading from (1-indexed)"
    );
    assert_eq!(
        properties["limit"]["description"],
        "Maximum number of lines to read"
    );
    assert_eq!(read.parameters()["required"], json!(["path"]));
}

/// Empty output is a complete read of an empty file. The scan's line
/// count keeps the same split semantics used by the direct reader.
#[test]
fn empty_text_has_structured_range() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("empty.txt"), "").unwrap();
    let output =
        call(&Root::new(dir.path()), json!({"path": "empty.txt"})).unwrap();
    assert_eq!(text_of(&output), "");
    assert_eq!(
        output.structured.unwrap(),
        json!({
            "kind": "text",
            "path": "empty.txt",
            "text": "",
            "offset": 1,
            "requested_limit": null,
            "total_lines": 1,
            "returned_lines": 0,
            "next_offset": null,
            "truncated": false,
            "complete": true,
            "truncated_by": null,
            "first_line_exceeds_limit": false,
            "artifact_error": "artifact storage is unavailable",
        })
    );
}

/// The 2000-line and 50 KiB boundaries are inclusive. Crossing either
/// one reports the same display cut used by the direct output.
#[test]
fn structured_text_tracks_exact_and_exceeded_bounds() {
    let dir = tempfile::tempdir().unwrap();
    let root = Root::new(dir.path());
    let exact_lines = vec!["x"; MAX_LINES].join("\n");
    std::fs::write(dir.path().join("f.txt"), &exact_lines).unwrap();
    let output = call(&root, json!({"path": "f.txt"})).unwrap();
    assert_eq!(text_of(&output), exact_lines);
    let structured = output.structured.unwrap();
    assert_eq!(structured["total_lines"], MAX_LINES);
    assert_eq!(structured["returned_lines"], MAX_LINES);
    assert_eq!(structured["truncated"], false);
    assert_eq!(structured["complete"], true);
    assert_eq!(structured["next_offset"], Value::Null);

    let over_lines = vec!["x"; MAX_LINES + 1].join("\n");
    std::fs::write(dir.path().join("f.txt"), &over_lines).unwrap();
    let output = call(&root, json!({"path": "f.txt"})).unwrap();
    // The card's details say where the display was cut, and why no
    // artifact holds the rest.
    let details = output.details.unwrap();
    assert_eq!(details["truncation"]["truncatedBy"], "lines");
    assert_eq!(details["truncation"]["outputLines"], MAX_LINES);
    assert_eq!(details["artifact"], Value::Null);
    assert_eq!(details["artifact_error"], "artifact storage is unavailable");
    assert_eq!(details["source_complete"], true);
    assert_eq!(details["kind"], "text");
    assert_eq!(details["offset"], 1);
    assert_eq!(details["total_lines"], MAX_LINES + 1);
    assert_eq!(details["returned_lines"], MAX_LINES);
    let structured = output.structured.unwrap();
    assert_eq!(structured["total_lines"], MAX_LINES + 1);
    assert_eq!(structured["returned_lines"], MAX_LINES);
    assert_eq!(structured["truncated"], true);
    assert_eq!(structured["truncated_by"], "lines");
    assert_eq!(structured["next_offset"], MAX_LINES + 1);
    assert_eq!(structured["complete"], false);

    let exact_bytes = "x".repeat(MAX_BYTES);
    std::fs::write(dir.path().join("f.txt"), &exact_bytes).unwrap();
    let output = call(&root, json!({"path": "f.txt"})).unwrap();
    let structured = output.structured.unwrap();
    assert_eq!(structured["text"].as_str().unwrap().len(), MAX_BYTES);
    assert_eq!(structured["truncated"], false);
    assert_eq!(structured["complete"], true);

    let over_bytes = "x".repeat(MAX_BYTES + 1);
    std::fs::write(dir.path().join("f.txt"), &over_bytes).unwrap();
    let output = call(&root, json!({"path": "f.txt"})).unwrap();
    let structured = output.structured.unwrap();
    assert_eq!(structured["text"], "");
    assert_eq!(structured["returned_lines"], 0);
    assert_eq!(structured["truncated"], true);
    assert_eq!(structured["truncated_by"], "bytes");
    assert_eq!(structured["first_line_exceeds_limit"], true);
    assert_eq!(structured["next_offset"], Value::Null);
}

/// A recognized but undecodable image is marked omitted and carries the
/// same text block the direct output returned.
#[test]
fn an_omitted_image_has_structured_text_content() {
    let dir = tempfile::tempdir().unwrap();
    let mut invalid_png = b"\x89PNG\r\n\x1a\n".to_vec();
    invalid_png.extend_from_slice(&13_u32.to_be_bytes());
    invalid_png.extend_from_slice(b"IHDR");
    std::fs::write(dir.path().join("broken.png"), invalid_png).unwrap();
    let output =
        call(&Root::new(dir.path()), json!({"path": "broken.png"})).unwrap();
    assert_eq!(
        text_of(&output),
        "Read image file [image/png]\n[Image omitted: could not be resized below the inline image size limit.]"
    );
    let structured = output.structured.as_ref().unwrap();
    assert_eq!(structured["kind"], "image");
    assert_eq!(structured["omitted"], true);
    assert_eq!(structured["content"].as_array().unwrap().len(), 1);
    assert_eq!(
        structured["content"],
        serde_json::to_value(&output.content).unwrap()
    );
}

/// Property inventory:
/// - `structured_text_matches_split_select_ranges` compares the structured
///   text, offsets, and continuation with an independent split/select oracle.
/// Generator plan: draw 1–8 nonempty Unicode lines without `\n`, then draw an
/// in-range offset (0 included, which reads as 1) and a limit from 0–10. Inputs are valid by construction;
/// shrinking reduces line count, line text, offset, and limit without rejects.
/// CI uses Hegel's detected `ci` profile (derandomized, database disabled,
/// and `TooSlow` suppressed); the local 100-case override keeps this focused
/// differential property bounded.
#[hegel::composite]
fn small_valid_lines(tc: &TestCase) -> Vec<String> {
    tc.draw(
        gs::vecs(gs::text().exclude_characters("\n").min_size(1).max_size(12))
            .min_size(1)
            .max_size(8),
    )
}

#[hegel::test(test_cases = 100)]
fn structured_text_matches_split_select_ranges(tc: TestCase) {
    let lines = tc.draw(small_valid_lines());
    let total = lines.len();
    let offset = tc.draw(gs::integers::<u64>().max_value(total as u64));
    let limit = tc.draw(gs::integers::<u64>().max_value(10));
    // Offset 0 is the first line, as 1 is.
    let start = offset.saturating_sub(1) as usize;
    let end = start.saturating_add(limit as usize).min(total);
    let content = lines.join("\n");
    let expected_text =
        content.split('\n').collect::<Vec<_>>()[start..end].join("\n");

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), content).unwrap();
    let output = call(
        &Root::new(dir.path()),
        json!({"path": "f.txt", "offset": offset, "limit": limit}),
    )
    .unwrap();
    let structured = output.structured.unwrap();
    assert_eq!(structured["text"], expected_text);
    assert_eq!(structured["offset"], start + 1);
    assert_eq!(structured["requested_limit"], limit);
    assert_eq!(structured["total_lines"], total);
    assert_eq!(structured["returned_lines"], end - start);
    assert_eq!(
        structured["next_offset"],
        if end < total {
            json!(end + 1)
        } else {
            Value::Null
        }
    );
    assert_eq!(structured["complete"], start == 0 && end == total);
    assert_eq!(structured["truncated"], false);
    assert_eq!(structured["truncated_by"], Value::Null);
    assert_eq!(structured["first_line_exceeds_limit"], false);
}
