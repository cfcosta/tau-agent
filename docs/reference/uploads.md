# Uploads

The person sends images with a message: from the composer's paperclip,
by pasting, or by dropping files on it. The model reads them in the
user message, and every interface shows them under the message. The
decision is [ADR 0041](../decisions/0041-the-person-sends-images.md).

## In the composer

- A picked or dropped file whose first bytes are PNG, JPEG, GIF or
  WebP is attached as an image, of at most 8 MiB (`attach::MAX_IMAGE`).
  Any other file must be UTF-8 text of at most 200 KB, and goes in the
  message as `<attached file="...">`, as before.
- A pasted image is attached as `pasted-1.png`, `pasted-2.jpeg`, and
  so on. The composer's `TextInput` is built `takes_images()` and
  emits `PastedImage` for each image on the clipboard.
- An image's chip shows a thumbnail. Removing the chip drops it.
- Sending takes the attachments: text files into the text, images into
  the event's `images`. `NewRun`, `Say` and `Fork` carry them.

## From the host to the model

| Host method | Takes                | What the run gets                                         |
| ----------- | -------------------- | --------------------------------------------------------- |
| `start`     | `impl Into<Prompt>`  | `Input::new(text).images(images)` as the fork's input     |
| `fork`      | `impl Into<Prompt>`  | The same                                                  |
| `resume`    | `impl Into<Prompt>`  | The same, as the resumed run's input                      |
| `steer`     | `impl Into<Prompt>`  | `RunControl::steer(prompt)`, read after the tool batch    |

In tau-agent the first user message is `Blocks`: the plan's context,
the input, then each image. A steered message is its text, then its
images. Without images, both stay `Text`.

`Host::steer_read(run, text)` gives back the message the run read,
images included. The host then applies `HostUpdate::Pictures` so the
run's view shows them under the steered text. Messages a run never
read go on with their texts joined by blank lines, and all their
images.

## Pictures

```rust
pub struct Picture { pub key: String, pub mime: String }
```

`key` is the lowercase hex SHA-256 of the image's base64.
`Picture::of(&ImageContent)` makes one, and `Picture::is_key` checks a
key. `Item::Pictures(Vec<Picture>)` follows the message the images came
with: live, from `Host::view`, `HostUpdate::Resumed` and
`HostUpdate::Pictures`; stored, from the user message's image blocks.

The host keeps each image it receives in `uploads/<key>` beside the
run store, as base64, written whole through a temporary file.
`Host::picture(run, picture)` reads it, or finds the image in the run's
messages and keeps it again.

## Fetching

- A window draws a picture from its cache. If the picture is missing,
  it emits `WorkspaceEvent::Picture { run, picture }` and draws a
  placeholder.
- The host answers with `HostUpdate::Picture { picture, data, error }`.
  The window decodes the data with gpui, or shows why it has none.
- A picture still waiting after `pictures::RETRY` (10 seconds) is asked
  for again.
- The window that sent an image puts it in its cache as it sends.

Phones get `HostUpdate::Picture` and `PluginReply` as
`Down::Transient`: unnumbered, only while connected, never replayed
(`phone_server::transient`). This keeps images out of the feed that
phones replay when they come back.

## Demo

`tau --demo --open uploads` shows the demo run steered with a message
and a picture, and another image attached in the composer.
