//! The canvas tools in real runs: a scripted model, the store, and files
//! in a temporary workspace. What the tools publish folds into the
//! canvas the interface shows, the images they keep read back whole, and
//! a call asked wrongly tells the model why and publishes nothing.

#![cfg(feature = "host")]

use std::{io::Cursor, path::Path};

use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::StreamExt;
use serde_json::{Value, json};
use tau_agent::{agent::Agent, event::RunEvent, tool::RunId};
use tau_canvas::{
    Canvas,
    Content,
    EDIT,
    NAME,
    READ,
    SHOW,
    WRITE,
    host::{CanvasPlugin, Storage},
};
use tau_store::Store;
use tau_testing::scripted::ScriptedModel;

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tau_testing::block_on_io(async {
        tokio::time::timeout(std::time::Duration::from_secs(20), future)
            .await
            .expect("the run ends")
    })
}

/// A PNG of `width`×`height`.
fn png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    image::RgbImage::new(width, height)
        .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
        .unwrap();
    bytes
}

/// A workspace with a plot in it, and the canvas storage beside it.
struct Fixture {
    dir: tempfile::TempDir,
    store: Store,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("work")).unwrap();
        std::fs::write(dir.path().join("work/plot.png"), png(12, 8)).unwrap();
        std::fs::write(dir.path().join("work/notes.txt"), "not an image")
            .unwrap();
        Self {
            dir,
            store: block_on(tau_store_sqlite::memory()).unwrap(),
        }
    }

    fn storage(&self) -> Storage {
        Storage::open(&self.dir.path().join("canvas")).unwrap()
    }

    fn work(&self) -> &Path {
        self.dir.path()
    }

    /// Runs the scripted model to its end with the canvas tools, and
    /// gives the run and each tool result: its text and whether it
    /// failed.
    fn run(&self, llm: &ScriptedModel) -> (RunId, Vec<(String, bool)>) {
        let agent = Agent::new(llm.clone()).plugin(CanvasPlugin::new(
            self.storage(),
            self.work().join("work"),
        ));
        let (id, events) = block_on(async {
            let mut run = agent.start("show me", &self.store);
            let id = run.id();
            let events: Vec<RunEvent> = run.events().collect().await;
            run.outcome().await.unwrap();
            (id, events)
        });
        llm.assert_exhausted();
        let results = events
            .iter()
            .filter_map(|event| match event {
                RunEvent::ToolEnd {
                    output, is_error, ..
                } => Some((output.text_content(), *is_error)),
                _ => None,
            })
            .collect();
        (id, results)
    }

    fn canvas(&self, run: &RunId) -> Canvas {
        let bodies: Vec<Value> = block_on(self.store.records(&run.0, NAME))
            .unwrap()
            .iter()
            .map(|body| serde_json::from_str(body).unwrap())
            .collect();
        Canvas::from_records(&bodies)
    }
}

#[test]
fn images_and_documents_land_on_the_canvas_with_every_revision() {
    let fixture = Fixture::new();
    let llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call(
                SHOW,
                json!({ "path": "plot.png", "title": "Loss", "caption": "Training loss" }),
            )
        })
        .turn(|t| {
            t.tool_call(
                WRITE,
                json!({ "title": "Plan", "text": "# Plan\n\n- one\n- two\n" }),
            )
        })
        .turn(|t| {
            t.tool_call(
                EDIT,
                json!({ "item": 2, "edits": [{ "old": "- two", "new": "- 2" }] }),
            )
        })
        .turn(|t| t.tool_call(READ, json!({ "item": 2, "revision": 1 })))
        .turn(|t| t.tool_call(SHOW, json!({ "path": "plot.png", "title": "Loss, again", "item": 1 })))
        .turn(|t| t.text("Done."));
    let (run, results) = fixture.run(&llm);
    assert_eq!(
        results,
        vec![
            (
                "Image 1, \"Loss\", is on the canvas (12×8 PNG).".to_owned(),
                false
            ),
            ("Document 2, \"Plan\", is on the canvas.".to_owned(), false),
            ("Document 2, \"Plan\", is at revision 2.".to_owned(), false),
            ("# Plan\n\n- one\n- two\n".to_owned(), false),
            (
                "Image 1, \"Loss, again\", shows version 2 (12×8 PNG)."
                    .to_owned(),
                false
            ),
        ]
    );

    let canvas = fixture.canvas(&run);
    assert_eq!(canvas.items.len(), 2);
    assert_eq!(canvas.fresh, 4);
    assert_eq!(canvas.last, Some(1));
    let plan = canvas.item(2).unwrap();
    assert_eq!(plan.revisions.len(), 2);
    assert_eq!(
        plan.latest().content,
        Content::Document {
            text: "# Plan\n\n- one\n- 2\n".to_owned()
        }
    );
    let image = canvas.item(1).unwrap();
    assert_eq!(image.title, "Loss, again");
    let Content::Image { image, caption } = &image.revisions[0].content else {
        panic!("item 1 is an image");
    };
    assert_eq!(caption, "Training loss");
    assert_eq!((image.width, image.height), (Some(12), Some(8)));
    assert_eq!(image.mime, "image/png");
    // The canvas kept the bytes: they read back whole once the file is
    // gone.
    std::fs::remove_file(fixture.work().join("work/plot.png")).unwrap();
    let data = block_on(fixture.storage().get(&image.blob)).unwrap();
    assert_eq!(STANDARD.decode(data).unwrap(), png(12, 8));
}

#[test]
fn a_call_asked_wrongly_says_why_and_publishes_nothing() {
    let fixture = Fixture::new();
    let llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call(SHOW, json!({ "path": "notes.txt", "title": "Notes" }))
        })
        .turn(|t| {
            t.tool_call(SHOW, json!({ "path": "missing.png", "title": "Gone" }))
        })
        .turn(|t| t.tool_call(WRITE, json!({ "title": "Doc", "text": "a a" })))
        .turn(|t| {
            t.tool_call(
                SHOW,
                json!({ "path": "plot.png", "title": "Plot", "item": 1 }),
            )
        })
        .turn(|t| {
            t.tool_call(
                EDIT,
                json!({ "item": 1, "edits": [{ "old": "a", "new": "b" }] }),
            )
        })
        .turn(|t| {
            t.tool_call(
                WRITE,
                json!({ "title": "Doc", "text": "x", "item": 7 }),
            )
        })
        .turn(|t| t.tool_call(READ, json!({ "item": 1, "revision": 2 })))
        .turn(|t| t.text("Sorry."));
    let (run, results) = fixture.run(&llm);
    let errors: Vec<&str> = results
        .iter()
        .filter(|(_, error)| *error)
        .map(|(text, _)| text.as_str())
        .collect();
    assert_eq!(results.len(), 7);
    assert_eq!(errors.len(), 6, "{results:?}");
    assert!(
        errors[0].contains("is not a PNG, JPEG, GIF, WebP, BMP or SVG image")
    );
    assert!(errors[1].starts_with("Cannot read missing.png"));
    assert_eq!(errors[2], "Item 1 is a document, not an image.");
    assert!(errors[3].contains("occurs more than once"));
    assert_eq!(errors[4], "There is no item 7 on the canvas.");
    assert_eq!(errors[5], "Item 1 has revisions 1 to 1.");
    // Only the document was published.
    let canvas = fixture.canvas(&run);
    assert_eq!(canvas.items.len(), 1);
    assert_eq!(canvas.fresh, 1);
}

#[test]
fn calls_at_once_take_turns_and_number_their_items_apart() {
    let fixture = Fixture::new();
    let llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call(WRITE, json!({ "title": "One", "text": "1" }))
                .tool_call(WRITE, json!({ "title": "Two", "text": "2" }))
                .tool_call(
                    SHOW,
                    json!({ "path": "plot.png", "title": "Three" }),
                )
        })
        .turn(|t| t.text("Three items."));
    let (run, results) = fixture.run(&llm);
    assert!(results.iter().all(|(_, error)| !error), "{results:?}");
    let canvas = fixture.canvas(&run);
    let mut ids: Vec<u32> = canvas.items.iter().map(|item| item.id).collect();
    ids.sort_unstable();
    assert_eq!(ids, vec![1, 2, 3]);
}
