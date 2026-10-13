# 0041: The person sends images, which the model reads and every interface draws

- Status: accepted. Built: images in `tau_agent::agent::Input` and
  `Prompt`; the composer's attachments, paste and drop in
  `tau-ui-remote`; the host's uploads in `tau-ui`; `Down::Transient` in
  `tau-remote`. Reference: [uploads.md](../reference/uploads.md).
- Date: 2026-10-13

## Context

The model reads images: `read` returns them, and the Responses input
takes them in a user message. But the person had no way to send one.
The composer's paperclip attached only text files of up to 200 KB,
inlined into the message. Everything between the composer and the
model carried a `String`: `Input`, steering, the workspace's events,
the host's methods.

Showing a sent image is the harder part. A run's view travels to every
phone, in snapshots and in the replay feed, which keeps the last 4,096
messages. Images inline in views would put megabytes in each.

## Decision

### The model gets the images in the user message

- `Input` has `images: Vec<ImageContent>`, and steering takes a
  `Prompt { text, images }`. Strings convert into both, so callers
  that send text are unchanged.
- The run's first user message is the plan's context, then the input,
  then the images. A steered message is its text, then its images.
  Text alone stays plain text. `RunPlan::images` lets a plugin see
  them; plugins rewrite only the text.
- The host's `start`, `fork`, `resume` and `steer` take
  `impl Into<Prompt>`. Messages a run never read go on with their texts
  joined and all their images. A message to a sub-agent that ended
  goes to its main chat as text only.

### Interfaces hold a reference, the host holds the bytes

- A transcript has `Item::Pictures` after the message the images came
  with. Each `Picture` is a key, the SHA-256 of the image's base64, and
  its type. A stored run's view gets them from its messages.
- The host keeps each image it receives under `uploads/` beside the
  run store, named by its key. An interface asks for one with
  `WorkspaceEvent::Picture` the first time it draws it. The host
  answers with `HostUpdate::Picture`, from the file, or from the run's
  messages when the file is gone (and keeps it again). A key must be
  64 hex digits, so it never names a path.
- The window that sent an image keeps it as it sends, and never asks.

### Answers to requests are not kept for phones

`Down::Transient` is a frame with no number, sent to the phones
connected now and not kept in the feed. The host sends
`HostUpdate::Picture` and `PluginReply` that way. A phone that was away
asks again: a picture still waiting after 10 seconds is asked for once
more, and so is a canvas image. The protocol is at version 3.

### The composer takes images three ways

The paperclip (now on phones too), a paste into the composer
(`TextInput::takes_images`, which emits `PastedImage`), and files
dropped on it. A file whose first bytes are PNG, JPEG, GIF or WebP is
an image, of at most 8 MiB. Any other file is attached as text, as
before. An image's chip shows the image.

## Consequences

- An image travels twice: up in the event, at most 8 MiB, which stays
  within one WebSocket message from a phone; and down once per window
  that draws it.
- Nothing prunes `uploads/`. It holds what runs' messages hold, and
  the store keeps those forever too.
- Images are not resized: they reach the model as the person sent
  them, within the 8 MiB the composer takes.
