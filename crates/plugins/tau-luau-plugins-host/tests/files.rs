//! What a Luau plugin's tools read (ADR 0050, gap G): the folders its
//! `uses.files` names and nothing past them, through `..` or a link
//! alike; what image a file is; and the blobs it keeps, read back whole
//! as the interface asks for them.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{io::Cursor, path::Path};

use serde_json::{Value, json};
use tau_luau_plugins_host::files::{Files, blob, blobs_dir};
use tau_testing::block_on_io;

/// A PNG of `width`×`height`.
fn png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    image::RgbImage::new(width, height)
        .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
        .unwrap();
    bytes
}

/// A workspace with a plot and a note in it, a folder beside it with a
/// secret, and the blobs' folder.
struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        std::fs::create_dir_all(path.join("work/sub")).unwrap();
        std::fs::write(path.join("work/plot.png"), png(12, 8)).unwrap();
        std::fs::write(path.join("work/notes.txt"), "a note").unwrap();
        std::fs::write(
            path.join("work/sub/chart.svg"),
            "<?xml version=\"1.0\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
        )
        .unwrap();
        std::fs::create_dir(path.join("outside")).unwrap();
        std::fs::write(path.join("outside/secret.txt"), "secret").unwrap();
        std::os::unix::fs::symlink(
            path.join("outside/secret.txt"),
            path.join("work/link.txt"),
        )
        .unwrap();
        Self { dir }
    }

    fn files(&self, declared: &[&str]) -> Files {
        let declared: Vec<String> =
            declared.iter().map(|folder| (*folder).to_owned()).collect();
        Files::new(
            &declared,
            Some(self.dir.path().join("work")),
            Some(self.dir.path().join("blobs")),
        )
    }

    fn path(&self, rest: &str) -> String {
        self.dir.path().join(rest).display().to_string()
    }
}

fn call(files: &Files, name: &str, args: Value) -> Result<Value, String> {
    block_on_io(files.call(name, &args)).map(|reply| reply.value)
}

#[test]
fn a_tool_reads_its_workspace_and_lists_it() {
    let f = Fixture::new();
    let files = f.files(&["workspace"]);
    assert_eq!(
        call(&files, "files_read", json!({ "path": "notes.txt" })),
        Ok(json!("a note"))
    );
    assert_eq!(
        call(&files, "files_read", json!({ "path": " @notes.txt " })),
        Ok(json!("a note"))
    );
    assert_eq!(
        call(
            &files,
            "files_read",
            json!({ "path": "notes.txt", "limit": 2 })
        ),
        Ok(json!("a "))
    );
    assert_eq!(
        call(
            &files,
            "files_read",
            json!({ "path": "notes.txt", "bytes": true })
        ),
        Ok(json!("YSBub3Rl"))
    );
    let listed = call(&files, "files_list", json!({ "path": "." })).unwrap();
    let names: Vec<&str> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["link.txt", "notes.txt", "plot.png", "sub"]);
    assert_eq!(listed[3]["dir"], json!(true));
    let error =
        call(&files, "files_read", json!({ "path": "plot.png" })).unwrap_err();
    assert!(error.contains("is not text"), "{error}");
}

#[test]
fn nothing_past_its_folders_is_read() {
    let f = Fixture::new();
    let files = f.files(&["workspace"]);
    for path in [
        "../outside/secret.txt".to_owned(),
        "link.txt".to_owned(),
        f.path("outside/secret.txt"),
        "sub/../../outside/secret.txt".to_owned(),
    ] {
        let error =
            call(&files, "files_read", json!({ "path": path })).unwrap_err();
        assert!(
            error.contains("its folder is not in `uses.files`"),
            "{path}: {error}"
        );
    }
    let error =
        call(&files, "files_read", json!({ "path": "gone.txt" })).unwrap_err();
    assert!(error.starts_with("Cannot read gone.txt: "), "{error}");
    // A folder declared by its path reaches what is in it, and only it.
    let outside = f.files(&[&f.path("outside")]);
    assert_eq!(
        call(
            &outside,
            "files_read",
            json!({ "path": f.path("outside/secret.txt") })
        ),
        Ok(json!("secret"))
    );
    let error = call(&outside, "files_read", json!({ "path": "notes.txt" }))
        .unwrap_err();
    assert!(
        error.contains("its folder is not in `uses.files`"),
        "{error}"
    );
}

#[test]
fn a_file_says_the_image_it_is_by_its_bytes() {
    let f = Fixture::new();
    let files = f.files(&["workspace"]);
    assert_eq!(
        call(&files, "files_image", json!({ "path": "plot.png" })),
        Ok(json!({
            "size": png(12, 8).len(),
            "mime": "image/png",
            "width": 12,
            "height": 8,
        }))
    );
    assert_eq!(
        call(&files, "files_image", json!({ "path": "sub/chart.svg" }))
            .unwrap()["mime"],
        json!("image/svg+xml")
    );
    assert_eq!(
        call(&files, "files_image", json!({ "path": "notes.txt" })),
        Ok(json!({ "size": 6 }))
    );
}

#[test]
fn a_blob_is_read_back_whole() {
    let f = Fixture::new();
    let files = f.files(&["workspace"]);
    let kept =
        call(&files, "blobs_put", json!({ "path": "plot.png" })).unwrap();
    assert_eq!(kept["size_bytes"], json!(png(12, 8).len()));
    let data = block_on_io(blob(f.dir.path().join("blobs"), &kept)).unwrap();
    use base64::Engine as _;
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .unwrap(),
        png(12, 8)
    );
    // The same bytes are one blob.
    let again =
        call(&files, "blobs_put", json!({ "path": "plot.png" })).unwrap();
    assert_eq!(again["digest"], kept["digest"]);
}

#[test]
fn a_repositorys_blobs_are_a_folder_of_their_own() {
    let root = Path::new("/blobs");
    assert_eq!(
        blobs_dir(root, "tau-canvas", "owner/repo"),
        root.join("tau-canvas").join("owner_repo")
    );
    assert_ne!(
        blobs_dir(root, "tau-canvas", "a"),
        blobs_dir(root, "other", "a")
    );
}

/// A new folder is new reach: the person allows it before a version
/// that reads it is active.
#[test]
fn a_new_folder_waits_for_an_allow() {
    let before = tau_luau_plugins::Uses {
        files: vec!["workspace".into()],
        ..Default::default()
    };
    let after = tau_luau_plugins::Uses {
        files: vec!["workspace".into(), "~/Pictures".into()],
        ..Default::default()
    };
    assert_eq!(after.grown_from(&before), ["reading files in `~/Pictures`"]);
    assert_eq!(
        before.grown_from(&Default::default()),
        ["reading the run's workspace"]
    );
}
