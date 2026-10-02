//! `read` (`tau_tools::read`), on real files (`docs/reference/tools.md`,
//! "read"; `docs/reference/testing.md`, "tau-tools").

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
use tau_tools::{
    image::{MAX_BASE64_BYTES, MAX_DIMENSION},
    path::Root,
    read::Read,
    truncate::{Limit, MAX_BYTES, MAX_LINES, format_size, truncate_head},
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
            "[Line {first} is {}, exceeds {} limit. Use bash: sed -n '{first}p' {path} | head -c {MAX_BYTES}]",
            format_size(all[start].len()),
            format_size(MAX_BYTES)
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
                format_size(MAX_BYTES),
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
    let lengths = || gs::sampled_from(vec![0usize, 1, 7, 40, 300, 60_000]);
    let count = tc.draw(gs::integers::<usize>().max_value(2_600));
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
    let content = String::from_utf8_lossy(&bytes).into_owned();
    if std::str::from_utf8(&bytes).is_err() {
        tc.event("not valid UTF-8");
    }
    if content.contains('\r') {
        tc.event("CRLF lines");
    }
    let total = content.split('\n').count() as u64;
    let offset =
        tc.draw(gs::optional(gs::integers::<u64>().max_value(total + 2)));
    let limit = tc.draw(gs::optional(gs::integers::<u64>().max_value(2_500)));
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), &bytes).unwrap();
    let root = Root::new(dir.path());
    let mut args = json!({"path": "f.txt"});
    if let Some(offset) = offset {
        args["offset"] = json!(offset);
    }
    if let Some(limit) = limit {
        args["limit"] = json!(limit);
    }
    let got = call(&root, args)
        .map(|o| text_of(&o))
        .map_err(|e| e.to_string());
    assert_eq!(got, pi_read(&content, "f.txt", offset, limit));
}

/// The notices name the right lines: a file over the line limit shows
/// its first 2000 lines and says where to go on.
#[test]
fn a_long_file_says_where_to_continue() {
    let dir = tempfile::tempdir().unwrap();
    let lines: Vec<String> = (1..=2500).map(|i| format!("line {i}")).collect();
    std::fs::write(dir.path().join("f.txt"), lines.join("\n")).unwrap();
    let output =
        call(&Root::new(dir.path()), json!({"path": "f.txt"})).unwrap();
    let text = text_of(&output);
    assert!(text.starts_with("line 1\nline 2\n"), "{}", &text[..40]);
    assert!(
        text.ends_with("line 2000\n\n[Showing lines 1-2000 of 2500. Use offset=2001 to continue.]"),
        "{}",
        &text[text.len() - 100..]
    );
    assert_eq!(
        output.details.unwrap()["truncation"]["truncatedBy"],
        "lines"
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
    assert_eq!(tau_tools::image::detect(&data), Some("image/png"));

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
    assert!(
        read.description()
            .starts_with("Read the contents of a file.")
    );
    assert!(
        read.description()
            .contains("truncated to 2000 lines or 50KB")
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

/// A first line over 50 KiB points to `sed`, with the line's real size
/// (pi, `read.ts`).
#[test]
fn a_first_line_over_the_limit_points_to_sed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("f.txt"),
        format!("{}\nshort", "x".repeat(60_000)),
    )
    .unwrap();
    let output =
        call(&Root::new(dir.path()), json!({"path": "f.txt"})).unwrap();
    assert_eq!(
        text_of(&output),
        "[Line 1 is 58.6KB, exceeds 50.0KB limit. Use bash: sed -n '1p' f.txt | head -c 51200]"
    );
    let structured = output.structured.unwrap();
    assert_eq!(structured["text"], "");
    assert_eq!(structured["returned_lines"], 0);
    assert_eq!(structured["truncated"], true);
    assert_eq!(structured["truncated_by"], "bytes");
    assert_eq!(structured["first_line_exceeds_limit"], true);
    assert_eq!(structured["next_offset"], Value::Null);
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

/// A zero limit selects no text, preserves offset zero's existing
/// normalization to line one, and does not underflow a continuation.
#[test]
fn zero_limit_has_an_empty_range_without_underflow() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), "one\ntwo").unwrap();
    let output = call(
        &Root::new(dir.path()),
        json!({"path": "f.txt", "offset": 0, "limit": 0}),
    )
    .unwrap();
    assert_eq!(
        text_of(&output),
        "\n\n[2 more lines in file. Use offset=1 to continue.]"
    );
    let structured = output.structured.unwrap();
    assert_eq!(structured["offset"], 1);
    assert_eq!(structured["requested_limit"], 0);
    assert_eq!(structured["text"], "");
    assert_eq!(structured["returned_lines"], 0);
    assert_eq!(structured["next_offset"], 1);
    assert_eq!(structured["complete"], false);
}

/// A requested subset reports its selected text and the first line after
/// that subset, while the direct continuation notice stays unchanged.
#[test]
fn subset_has_structured_text_and_continuation() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), "one\ntwo\nthree").unwrap();
    let output = call(
        &Root::new(dir.path()),
        json!({"path": "f.txt", "offset": 2, "limit": 1}),
    )
    .unwrap();
    assert_eq!(
        text_of(&output),
        "two\n\n[1 more lines in file. Use offset=3 to continue.]"
    );
    assert_eq!(
        output.structured.unwrap(),
        json!({
            "kind": "text",
            "path": "f.txt",
            "text": "two",
            "offset": 2,
            "requested_limit": 1,
            "total_lines": 3,
            "returned_lines": 1,
            "next_offset": 3,
            "truncated": false,
            "complete": false,
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
    assert_eq!(structured["truncated"], true);
    assert_eq!(structured["truncated_by"], "bytes");
    assert_eq!(structured["first_line_exceeds_limit"], true);
    assert_eq!(structured["next_offset"], Value::Null);
}

/// Unicode text remains intact in the structured range and counts the
/// same line boundaries as the streaming reader.
#[test]
fn structured_text_keeps_unicode_and_path() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("é.txt"), "α😀\n漢字").unwrap();
    let output = call(
        &Root::new(dir.path()),
        json!({"path": "é.txt", "offset": 2, "limit": 1}),
    )
    .unwrap();
    assert_eq!(text_of(&output), "漢字");
    let structured = output.structured.unwrap();
    assert_eq!(structured["path"], "é.txt");
    assert_eq!(structured["text"], "漢字");
    assert_eq!(structured["total_lines"], 2);
    assert_eq!(structured["returned_lines"], 1);
    assert_eq!(structured["next_offset"], Value::Null);
    assert_eq!(structured["complete"], false);
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

/// The public schema covers both tagged result variants and is exposed
/// through `AgentTool::output_schema`.
#[test]
fn read_declares_its_structured_output_schema() {
    let read = Read::new(Root::new("/"));
    let schema = read.output_schema().unwrap();
    assert_eq!(schema["oneOf"].as_array().unwrap().len(), 2);
    assert_eq!(schema["oneOf"][0]["properties"]["kind"]["const"], "text");
    assert_eq!(schema["oneOf"][1]["properties"]["kind"]["const"], "image");
    for field in [
        "text",
        "offset",
        "requested_limit",
        "total_lines",
        "returned_lines",
        "next_offset",
        "truncated",
        "complete",
        "truncated_by",
        "first_line_exceeds_limit",
    ] {
        assert!(
            schema["oneOf"][0]["required"]
                .as_array()
                .unwrap()
                .contains(&json!(field)),
            "missing required field {field}"
        );
    }
    assert!(
        schema["oneOf"][1]["properties"]["content"]["items"]["oneOf"]
            .as_array()
            .unwrap()
            .iter()
            .any(|block| block["properties"]["type"]["const"] == "image")
    );
}

/// Property inventory:
/// - `structured_text_matches_split_select_ranges` compares the structured
///   text, offsets, and continuation with an independent split/select oracle.
/// Generator plan: draw 1–8 nonempty Unicode lines without `\n`, then draw an
/// in-range offset and a limit from 0–10. Inputs are valid by construction;
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
    let offset =
        tc.draw(gs::integers::<u64>().min_value(1).max_value(total as u64));
    let limit = tc.draw(gs::integers::<u64>().max_value(10));
    let start = offset as usize - 1;
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
    assert_eq!(structured["offset"], offset);
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
