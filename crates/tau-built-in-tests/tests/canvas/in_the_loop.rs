//! tau-canvas in a real run (ADR 0050, gap G): `canvas_show` reads an
//! image from the run's workspace and keeps it as a blob of the
//! repository's, which reads back whole as the interface asks; a file
//! past the folders the plugin declares is not read.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::io::Cursor;

use serde_json::{Value, json};
use tau_agent::{agent::Agent, event::RunEvent};
use tau_luau_plugins_host::agent::{Active, LuauPlugins};
use tau_testing::{block_on_io, scripted::ScriptedModel};

/// A PNG of `width`×`height`.
fn png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    image::RgbImage::new(width, height)
        .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
        .unwrap();
    bytes
}

/// The run's events, the model calling `canvas_show` with `args` then
/// answering.
fn show(dir: &std::path::Path, args: Value) -> Vec<RunEvent> {
    use futures_util::StreamExt;
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("canvas_show", args))
        .turn(|t| t.text("Shown."));
    let loaded = super::luau::loaded();
    let settings = loaded.declaration.default_settings();
    let plugins = LuauPlugins::new(
        vec![Active { loaded, settings }],
        json!({ "kind": "main", "repo": "repo", "model": "m" }),
        None,
    )
    .with_files(Some(dir.join("work")), dir.join("blobs"));
    let agent = Agent::new(llm).plugin(plugins);
    block_on_io(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let mut run = agent.start("Plot it.", &store);
        run.events().collect().await
    })
}

/// What the call's result said.
fn said(events: &[RunEvent]) -> String {
    events
        .iter()
        .find_map(|event| match event {
            RunEvent::ToolEnd { output, .. } => Some(output.text_content()),
            _ => None,
        })
        .expect("the call ended")
}

/// The canvas the plugin left in the run's state.
fn canvas(events: &[RunEvent]) -> Value {
    events
        .iter()
        .rev()
        .find_map(|event| match event {
            RunEvent::PluginReport { body, .. }
                if body["plugin"] == tau_luau_plugins::names::CANVAS
                    && body["state"]["canvas"].is_object() =>
            {
                Some(body["state"]["canvas"].clone())
            }
            _ => None,
        })
        .expect("the canvas is in the run's state")
}

#[test]
fn an_image_from_the_workspace_is_kept_and_read_back_whole() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("work")).unwrap();
    std::fs::write(dir.path().join("work/plot.png"), png(12, 8)).unwrap();
    let events =
        show(dir.path(), json!({ "path": "plot.png", "title": "Plot" }));
    assert_eq!(
        said(&events),
        "Image 1, \"Plot\", is on the canvas (12×8 PNG)."
    );
    let image =
        canvas(&events)["items"][0]["revisions"][0]["content"]["image"].clone();
    let blobs = tau_luau_plugins_host::files::blobs_dir(
        &dir.path().join("blobs"),
        tau_luau_plugins::names::CANVAS,
        "repo",
    );
    let data =
        block_on_io(tau_luau_plugins_host::files::blob(blobs, &image["blob"]))
            .unwrap();
    use base64::Engine as _;
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .unwrap(),
        png(12, 8)
    );
    // The file may change: the canvas keeps its copy.
    std::fs::remove_file(dir.path().join("work/plot.png")).unwrap();
}

#[test]
fn a_file_past_its_folders_is_not_shown() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("work")).unwrap();
    std::fs::write(dir.path().join("secret.png"), png(2, 2)).unwrap();
    let path = dir.path().join("secret.png").display().to_string();
    let events = show(dir.path(), json!({ "path": path, "title": "x" }));
    // Outside home in a test's temporary folder, it is neither the
    // workspace nor `~`.
    if std::env::var_os("HOME")
        .is_some_and(|home| dir.path().starts_with(std::path::Path::new(&home)))
    {
        return;
    }
    assert!(
        said(&events).contains("its folder is not in `uses.files`"),
        "{}",
        said(&events)
    );
}
