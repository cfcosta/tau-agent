# 0040: What a run shows the person goes on a canvas beside it

- Status: accepted. Built: `tau-canvas` (`crates/plugins/tau-canvas`)
  and the `PANE` point in `tau-ui-plugin`, which `tau-ui-remote` draws
  beside a run's transcript. Reference: [canvas.md](../reference/canvas.md).
- Date: 2026-10-13

## Context

A run could tell the person things only in prose. When the model
plotted a chart, took a screenshot or rendered a diagram, the person
had to find the file and open it themselves. A plan or a report longer
than a reply scrolled away with the transcript, and each change to it
meant reading the whole text again.

The model already reads images (`read` returns them as input), but
nothing went the other way, and no surface held what a run made: the
inspector is a narrow column of status, and tool cards sit in the
transcript's flow.

## Decision

### A pane beside the transcript, as an extension point

`points::PANE` (`Point<AtRun, PaneView>`): the first contribution draws
a wide pane for the run. `PaneView` is the pane's title, its body, and
`fresh`, a number that grows when the plugin has something new.

- On a computer the pane takes the inspector's place and shares the
  width with the transcript. On a phone it covers the run, with its own
  bar.
- The run header has a button that opens it, and the pane's bar one
  that closes it; Details closes it for the inspector. A phone's run bar
  has an icon for it. `Handle::open_pane` (`Request::OpenPane`) opens it
  from a plugin, as a card's click does. With the pane open, the header
  drops the context meter for room.
- The pane opens by itself when `fresh` grew since the run was last
  drawn: while the person watches, or when they come back to it. A run
  drawn for the first time with something there keeps it closed, so
  opening an old run does not cover its transcript.

tau-ui declares the point; it knows nothing of what a canvas is.

### A plugin, `tau-canvas`, with four tools

- `canvas_show { path, title, caption?, item? }` shows an image file:
  PNG, JPEG, GIF, WebP, BMP or SVG, detected from its bytes, at most
  8 MiB.
- `canvas_write { title, text, item? }` writes a Markdown document, at
  most 256 KiB.
- `canvas_edit { item, edits: [{ old, new }], title? }` revises one by
  exact replacements, each of which must match once.
- `canvas_read { item, revision? }` reads a revision back, for a model
  whose context lost it.

Each image or document is an item, numbered from 1 per run. Giving
`item` makes a new revision of it, and every revision is kept: the
person steps back through them in the pane. Calls in one run take
turns, so items made at once get different numbers.

### Records hold the documents, storage holds the images

The plugin publishes one record, `revised`, with the item, its title
and the revision's content. The tools fold the run's records along its
fork chain at each call (`PluginCtx::records`), so a fork or a resumed
run goes on with the canvas it inherits.

A document's text is in its record. An image's bytes are not: the tool
copies the file into an artifact store of the plugin's own
(`tau-artifacts`, in the repository's directory under `canvas/`), and
the record holds a reference with the type and size. The image still
shows once its file changes or goes, and a record stays small enough
to travel in every run's state to each phone.

The interface fetches an image through the host half's `act`, once
per window, and decodes it with gpui. The 8 MiB limit keeps the base64
reply within one WebSocket message to a phone.

## Consequences

- Every run declares four more tools, so every run's first request
  changed once; their order stays fixed (ADR 0022).
- The storage is never pruned yet: it is bounded by its quota (1 GiB
  per repository), and a call over it fails with the reason. Pruning
  against the retained runs, as the project's artifacts are, comes
  later.
- Documents grow a run's state with each revision. 256 KiB a revision
  bounds one, not their number.
- The person cannot edit a document in the pane yet; they ask the
  model.
- The person's own uploads are the other half of this: images the
  person attaches go to the model as input
  ([0041](0041-the-person-sends-images.md)). Its transient answers
  carry the canvas's image fetches too, so no image stays in the phones'
  replay feed.
