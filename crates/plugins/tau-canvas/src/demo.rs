//! What tau-ui's demo puts on a run's canvas: a chart kept in the
//! repository's canvas storage as a real call would keep it, and a
//! document written, then edited.

use std::path::Path;

use serde_json::{Value, json};

use crate::{Content, ImageRef, Record, host::Storage};

/// A canvas call as the demo replays it: its card's call, and what the
/// tool published and answered.
pub struct Call {
    pub id: &'static str,
    pub tool: &'static str,
    pub args: Value,
    pub record: Record,
    pub text: String,
}

const PLAN: &str = "## What changes\n\n\
- `canvas_show` keeps a copy of the image in the repository's canvas \
storage, so it still shows once the file changes.\n\
- `canvas_write` and `canvas_edit` keep every revision of a document.\n\
- The pane opens beside the transcript when something new arrives.\n\n\
## Open questions\n\n\
1. Should the person edit a document in place?\n\
2. Should uploads land on the canvas too?\n";

/// Keeps the demo's chart in the canvas storage of the repository whose
/// directory is `repo_dir`, and gives the calls that show it and write
/// the plan.
pub async fn seed(repo_dir: &Path) -> anyhow::Result<Vec<Call>> {
    let dir = Storage::dir_of(repo_dir);
    let storage =
        tokio::task::spawn_blocking(move || Storage::open(&dir)).await??;
    let (width, height) = (1200, 640);
    let blob = storage
        .put(chart(width, height), &Default::default())
        .await?;
    let image = ImageRef {
        blob,
        mime: "image/png".into(),
        width: Some(width),
        height: Some(height),
        path: "target/plots/retry-latency.png".into(),
    };
    let edited = PLAN.replace(
        "2. Should uploads land on the canvas too?\n",
        "2. Uploads land on the canvas too: the person's images go to the \
         model as input.\n",
    );
    Ok(vec![
        Call {
            id: "canvas-show",
            tool: crate::SHOW,
            args: json!({
                "path": image.path,
                "title": "Retry latency",
                "caption": "p50 and p99 latency per attempt, before and after the backoff change",
            }),
            text: format!(
                "Image 1, \"Retry latency\", is on the canvas ({}).",
                image.describe()
            ),
            record: Record::Revised {
                call: "canvas-show".into(),
                item: 1,
                title: "Retry latency".into(),
                content: Content::Image {
                    image,
                    caption: "p50 and p99 latency per attempt, before and \
                              after the backoff change"
                        .into(),
                },
            },
        },
        Call {
            id: "canvas-write",
            tool: crate::WRITE,
            args: json!({ "title": "Canvas plan", "text": PLAN }),
            text: "Document 2, \"Canvas plan\", is on the canvas.".into(),
            record: Record::Revised {
                call: "canvas-write".into(),
                item: 2,
                title: "Canvas plan".into(),
                content: Content::Document { text: PLAN.into() },
            },
        },
        Call {
            id: "canvas-edit",
            tool: crate::EDIT,
            args: json!({ "item": 2, "edits": [{
                "old": "2. Should uploads land on the canvas too?",
                "new": "2. Uploads land on the canvas too: the person's images go to the model as input.",
            }] }),
            text: "Document 2, \"Canvas plan\", is at revision 2.".into(),
            record: Record::Revised {
                call: "canvas-edit".into(),
                item: 2,
                title: "Canvas plan".into(),
                content: Content::Document { text: edited },
            },
        },
    ])
}

/// A line chart of two falling series on a dark ground, as a PNG.
pub fn chart(width: u32, height: u32) -> Vec<u8> {
    use image::{Rgb, RgbImage};
    let mut pixels = RgbImage::from_pixel(width, height, Rgb([22, 24, 29]));
    let (left, right, top, bottom) = (80, width - 40, 40, height - 60);
    let mut line = |from: (f32, f32), to: (f32, f32), color, thick: i32| {
        let steps =
            ((to.0 - from.0).abs().max((to.1 - from.1).abs()) as i32).max(1);
        for step in 0..=steps {
            let k = step as f32 / steps as f32;
            let x = from.0 + (to.0 - from.0) * k;
            let y = from.1 + (to.1 - from.1) * k;
            for dx in -thick..=thick {
                for dy in -thick..=thick {
                    let (px, py) = (x as i32 + dx, y as i32 + dy);
                    if px >= 0
                        && py >= 0
                        && (px as u32) < width
                        && (py as u32) < height
                    {
                        pixels.put_pixel(px as u32, py as u32, color);
                    }
                }
            }
        }
    };
    let grid = Rgb([44, 48, 58]);
    for n in 0..=5 {
        let y = top as f32 + (bottom - top) as f32 * n as f32 / 5.;
        line((left as f32, y), (right as f32, y), grid, 0);
    }
    for n in 0..=8 {
        let x = left as f32 + (right - left) as f32 * n as f32 / 8.;
        line((x, top as f32), (x, bottom as f32), grid, 0);
    }
    let axis = Rgb([120, 126, 140]);
    line(
        (left as f32, top as f32),
        (left as f32, bottom as f32),
        axis,
        1,
    );
    line(
        (left as f32, bottom as f32),
        (right as f32, bottom as f32),
        axis,
        1,
    );
    let series = |scale: f32, floor: f32, wobble: f32| {
        (0..=40)
            .map(|n| {
                let t = n as f32 / 40.;
                let value =
                    floor + scale * (-3. * t).exp() + wobble * (t * 23.).sin();
                let x = left as f32 + (right - left) as f32 * t;
                let y = bottom as f32 - (bottom - top) as f32 * value;
                (x, y)
            })
            .collect::<Vec<_>>()
    };
    for (points, color) in [
        (series(0.78, 0.16, 0.012), Rgb([240, 150, 90])),
        (series(0.5, 0.08, 0.008), Rgb([110, 170, 250])),
    ] {
        for pair in points.windows(2) {
            line(pair[0], pair[1], color, 2);
        }
    }
    let mut bytes = Vec::new();
    pixels
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .expect("a PNG encodes");
    bytes
}
