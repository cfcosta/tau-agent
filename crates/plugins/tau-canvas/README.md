# tau-canvas

Lets an agent show the person images and Markdown documents on a
canvas, a pane beside the run's transcript. `canvas_show` shows an
image file, `canvas_write` writes a document, `canvas_edit` revises it
by exact replacements, and `canvas_read` reads a revision back. Every
revision of an item is kept, and the person steps back through them.

## What it provides

| Item                                        | What it is                                                                    |
| ------------------------------------------- | ----------------------------------------------------------------------------- |
| `Record`                                    | What the plugin publishes: `Revised`, one revision of an item                 |
| `Content`, `ImageRef`, `Blob`               | A revision's image (kept in storage, by reference) or document text           |
| `Canvas`, `Item`, `Revision`                | The fold of a run's records: its items with every revision                    |
| `apply_edits`, `Edit`                       | `canvas_edit`'s replacements, each matching once                              |
| `NAME`, `SHOW`, `WRITE`, `EDIT`, `READ`     | `"tau-canvas"` and the tools' names                                           |
| `MAX_IMAGE_BYTES`, `MAX_DOCUMENT_BYTES`     | 8 MiB an image, 256 KiB a document                                            |
| `CanvasUi`                                  | The plugin's `UiPlugin`: the pane, the cards, and its status line             |
| `ui::Pick`, `ui::Ui`                        | The revision the person picked, and what a window keeps (fetched images)      |
| `host::CanvasPlugin`, `host::Storage`       | The agent plugin with the four tools, and the image storage (feature `host`)  |
| `CanvasHost`                                | The `HostHalf` for tau's plugin registry (feature `host`)                     |
| `demo::seed`, `demo::chart`                 | What tau-ui's demo puts on a canvas (feature `demo`)                          |

## How it fits

tau-canvas is a single crate holding both halves. The records, the fold
and the views always build. The tools and the storage sit behind the
default `host` feature, so `tau-ui-remote` and a phone build it with
`default-features = false` (decisions 0017, 0030).

- Builds on `tau-agent` (`Plugin`, `TypedTool`, `PluginCtx::publish`
  and `records`), `tau-artifacts` for the images, `tau-ui-plugin` and
  `tau-ui-kit`.
- Used by `tau-ui`, which registers `CanvasHost`, and `tau-ui-remote`,
  which lists `CanvasUi`. The pane is a contribution at
  `tau_ui_plugin::points::PANE`.
- Images live in the repository's directory, under `canvas/`. The
  interface fetches them through the host half's `act`.

## Usage

Outside tau's interface, give an agent the plugin and read the canvas
from the run's records:

```rust
use tau_agent::agent::Agent;
use tau_canvas::{Canvas, NAME, host::{CanvasPlugin, Storage}};

let storage = Storage::open(&repo_dir.join("canvas"))?;
let agent = Agent::new(llm).plugin(CanvasPlugin::new(storage, workspace));
let outcome = agent.run("Plot the latencies and show me", &store).await?;

let bodies: Vec<serde_json::Value> = store
    .records(&outcome.run.0, NAME)
    .await?
    .iter()
    .map(|body| serde_json::from_str(body))
    .collect::<Result<_, _>>()?;
let canvas = Canvas::from_records(&bodies);
```

`Storage::open` blocks; call it from `spawn_blocking` in async code.

## Testing

```sh
cargo nextest run --release -p tau-canvas
```

- `src/tests.rs`: the fold and the edits, with Hegel properties. Any
  sequence of revisions keeps every one in order, and an edit replaces
  only its text.
- `tests/run.rs`: the tools in real runs with a scripted model and the
  store. Images and documents land with every revision, the kept bytes
  read back whole after the file is gone, calls asked wrongly publish
  nothing, and calls made at once number their items apart.

## Further reading

- [docs/reference/canvas.md](../../../docs/reference/canvas.md)
- [docs/decisions/0040-what-a-run-shows-goes-on-a-canvas.md](../../../docs/decisions/0040-what-a-run-shows-goes-on-a-canvas.md)
