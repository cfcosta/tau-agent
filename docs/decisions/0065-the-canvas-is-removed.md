# 0065: The canvas is removed

- Status: accepted. Supersedes [0040](0040-what-a-run-shows-goes-on-a-canvas.md).
- Date: 2026-11-20

## Context

ADR 0040 gave a run a canvas: `heiko-canvas`, a built-in Luau package
whose tools (`canvas_show`, `canvas_write`, `canvas_edit`,
`canvas_read`) put images and Markdown documents in a pane beside the
transcript, every revision kept. The shipped skills leaned on it: long
explanations, decision trails, resume notes, moodboards and screenshots
all went "on the canvas".

On 2026-11-20 the person said they did not like how the canvas is at
all, and asked for it to be removed.

## Decision

- **`heiko-canvas` goes:** its package, its place in
  `plugins/built-in.toml`, `names::CANVAS`, the demo's `canvas`,
  `canvas-image` and `canvas-document` screens, heiko-ui's canvas tests,
  `docs/reference/canvas.md` and the README's canvas screenshots.
- **The pane stays.** `points::PANE` and the pane heiko-ui-remote draws
  beside a run are a seam no built-in plugin fills now. A later design
  for what a run shows the person can fill it again.
- **The skills drop their canvas lines,** with nothing in their place.
  Where a step only made sense on the canvas (a hillclimb's decision
  table, marking a pickup on a trail), the step goes; a decision trail
  or a resume note is kept without saying where.

## Consequences

- A run has no way to show the person an image or a long document beside
  the chat. Screenshots and long text reach the person only through the
  reply and the files the run writes.
- Old runs that called canvas tools still list those calls; their lines
  show the tool's name and what it answered, with no pane.
- The demo's chart stays, for its uploads screen.
