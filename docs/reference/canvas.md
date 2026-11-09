# The canvas

The canvas is a pane beside a run's transcript that holds what the run
made for the person to look at: images and Markdown documents. The
plugin is `tau-canvas` (`plugins/tau-canvas`), a Luau plugin tau comes
with (ADR 0050), and the pane is `tau-ui-plugin`'s `PANE` point, which
a Luau view fills with `pane`. The decision is
[ADR 0040](../decisions/0040-what-a-run-shows-goes-on-a-canvas.md).

## Tools

| Tool           | Arguments                                 | What it does                                         |
| -------------- | ----------------------------------------- | ---------------------------------------------------- |
| `canvas_show`  | `path`, `title`, `caption?`, `item?`      | Shows an image file; with `item`, a new version      |
| `canvas_write` | `title`, `text`, `item?`                  | Writes a Markdown document; with `item`, replaces it |
| `canvas_edit`  | `item`, `edits: [{ old, new }]`, `title?` | Revises a document by exact replacements             |
| `canvas_read`  | `item`, `revision?`                       | Reads a revision back (the latest by default)        |

- Items are numbered from 1 in each run, images and documents alike.
  `item` must name an item of the tool's kind.
- `path` resolves against the run's workspace. `~/`, `file://` and an
  absolute path work too, and a leading `@` is dropped; the file must be
  in the workspace or the home folder, its `uses.files`, where it really
  is.
- An image is PNG, JPEG, GIF, WebP or BMP, detected from its first
  bytes, or SVG, detected as text with an `<svg` element. It is at most
  8 MiB (`MAX_IMAGE_BYTES`).
- A document is at most 256 KiB (`MAX_DOCUMENT_BYTES`), after edits.
- `canvas_edit` makes its edits in turn. Each `old` must occur exactly
  once in the text the edits before it left, or the call fails,
  naming the edit.
- A call that fails leaves the canvas as it was.
- Calls in one run take turns, on the run's state, so calls made at
  once number their items apart.

Each result is one line for the model, such as `Image 1, "Loss", is on
the canvas (1200×800 PNG).`.

## State

The canvas is the plugin's state in the run, `canvas`: the items in the
order they were made, each `{ id, title, revisions }`, every revision
`{ call, content }`, plus `fresh` (the number of revisions so far) and
`last` (the item revised last). A document's content is `{ "type":
"document", "text" }`; an image's is `{ "type": "image", "image": {
"blob": { "id", "digest", "size_bytes" }, "mime", "width", "height",
"path" }, "caption" }`, `width` and `height` absent for an SVG. What the
pane shows of it, when the person picked, is `pick`: `{ item, revision,
fresh }`, in the run's state too, so every device shows the same.

## Storage

An image's bytes are a blob of the plugin's in the repository
(`ctx.blobs.put_file`): a `tau-artifacts` store under tau-luau-plugins'
directory, `blobs/tau-canvas/<repository>`, with quotas of 8 MiB a blob
and 1 GiB in all. The state holds the blob's metadata. Nothing prunes
the store yet.

The interface asks tau-luau-plugins' host half for an image with
`{ "act": "blob", "plugin", "repo", "blob" }`, which `ui.image` makes.
The host half reads the blob in ranges and answers `{ "data" }` with
the bytes in base64, or `{ "error" }`. Each window fetches an image
once and keeps it.

## The pane

- `PaneView { title, fresh, body }`: the first plugin to contribute at
  `PANE` draws the pane.
- On a computer the pane takes the inspector's place, 45% of the width
  beside the sidebar. The run header's button opens it and its bar's
  closes it; Details closes it for the inspector. On a phone it covers
  the run, and the run bar has an icon for it.
- It opens by itself when `fresh` grew since the run was last drawn. A
  run drawn for the first time with something on its canvas keeps the
  pane closed.
- `Handle::open_pane` opens it from a plugin.

tau-canvas's pane shows a tab per item when there are several, then a
bar with the revision shown ("Revision 2 of 3") and the way to the
ones before and after it. Below that is the image, fitted to the pane
with its caption, type, size and file, or the document's title and
Markdown. It shows the item revised last until the person picks
another tab or revision, which its buttons keep in the run's state.
Whatever comes next takes its place again.

A canvas tool's card shows the revision its call made, from the view's
`calls`: what it is in place of the arguments, its size or word count
at the header's end, and a thumbnail of the image or the document's
first lines. A click opens the pane on that revision. The card of
`canvas_read`, or of a call that failed, is the tool's as the loop
draws it.

## Demo

`tau --demo --open canvas` shows a chart and a two-revision document on
the demo run, with the pane open on the document. The calls, the state
and the view come from `plugins/tau-canvas/fixtures`, which the
plugin's tests check against what it leaves and draws; the demo keeps
its chart as a blob and puts it in the fixtures' place. `--open canvas-image`
shows it on the chart, and `--open canvas-document` on the document's
first revision.
