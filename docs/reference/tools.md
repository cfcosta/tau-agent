# Coding tools (`tau-tools`, optional)

These specs follow pi's built-in tools
(`packages/coding-agent/src/core/tools/`). Keep the limits and error
strings as written here. Models are tuned to them, and evals carry over.

The shared limits:

| Constant        | Value          |
| --------------- | -------------- |
| `MAX_LINES`     | 2000           |
| `MAX_BYTES`     | 50 KiB         |
| `GREP_MAX_LINE` | 500 characters |

The truncation helpers:

- `truncate_head` keeps whole lines from the start.
- `truncate_tail` keeps whole lines from the end. It may keep a partial
  last line, but always cuts at a UTF-8 boundary.

A tool that fails returns `Err`, and the loop marks the result
`is_error`. Every tool takes a root directory at construction, and all
paths resolve against it.

## read: `{ path, offset?, limit? }`

- `offset` counts lines from 1.
- **Path resolution:**
  - `~` expands to the home directory, and a leading `@` is stripped;
  - Unicode spaces are handled;
  - macOS screenshot filename variants are retried: NNBSP before AM/PM,
    NFD normalization, and curly apostrophes.
- **Images** are detected by magic bytes, not by file extension.
  - Unsupported formats are converted to PNG.
  - EXIF orientation is applied.
  - Images are resized to at most 2000×2000 and 4.5 MB of base64, as
    JPEG at quality 80.
- **Text:**
  - The result is `truncate_head`, followed by
    `[Showing lines X-Y of N. Use offset=K to continue.]`.
  - If the first line alone is over 50 KiB, the result suggests
    `sed -n 'Np' file | head -c 51200` instead.
- **Deliberate difference from pi:** tau-agent reads only the requested
  range. pi reads the whole file even when an offset is given.

## bash: `{ command, timeout? }`

- `timeout` is in seconds. There is no default.
- **Shell:** a configured shell path if one is set; otherwise
  `/bin/bash`, then `bash` on `PATH`, then `sh`. The command runs as
  `-c <command>`, and stdin is closed.
- **Process group:** the child runs in its own process group (`setsid`
  or `process_group(0)`). On cancel or timeout, the whole group gets
  SIGKILL. Exit code 128 + N is reported for signal N.
- **Late output:** after the child exits, keep reading until the pipes
  have been idle for 100 ms. Grandchildren that still hold the pipes
  would otherwise lose output.
- **Output:**
  - stdout and stderr are merged in arrival order;
  - a rolling tail of about 2 × `MAX_BYTES` is kept;
  - the result is `truncate_tail`;
  - when output was truncated, the full output is written to
    `$TMPDIR/tau-bash-<hex>.log`, and the notice says
    `Full output: <path>`.
- **Errors:** a non-zero exit, a timeout or an abort returns `Err`, for
  example `Command exited with code N`.
- **Progress:** partial output is sent as `ToolUpdate`, throttled.

## edit: `{ path, edits: [{ oldText, newText }] }`

- **Argument repair** (`prepare_arguments`) fixes common model mistakes:
  - `edits` sent as a JSON string is parsed;
  - a single object instead of a list is wrapped in a list;
  - top-level `oldText`/`newText` are moved into `edits`.
- **Algorithm:**
  1. Strip the BOM and remember the line ending. Normalize to LF.
  2. Find each `oldText` by exact search.
  3. If any edit fails to match, redo **all** edits against a
     normalized copy of the file: NFKC, trailing whitespace stripped per
     line, smart quotes and dashes mapped to ASCII, and special spaces
     mapped to a normal space.
  4. Every edit must match, and every match must be unique.
  5. Sort the edits and reject any that overlap.
  6. Apply the edits in reverse order. In fuzzy mode, only the lines an
     edit touches take their text from the normalized copy. Untouched
     lines keep their original bytes.
  7. If the file did not change, return `Err`. Otherwise restore the line
     endings and BOM, then write the file.
- **Deliberate difference from pi:** uniqueness is checked in the same
  space the match was found in. pi checks it in normalized space even for
  exact matches (`edit-diff.ts:328`). That rejects a match that is unique
  exactly but has a near-duplicate elsewhere.
- **Result details:** a unified diff (`similar`) and the first changed
  line.
- **Serialization:** each read-modify-write holds a per-path async mutex,
  keyed by canonical path. Parallel edits to the same file are
  serialized.

## write: `{ path, content }`

Creates parent directories, then writes the file. Returns
`Successfully wrote to <path>`. Holds the same per-path mutex as `edit`.

## grep: `{ pattern, path?, glob?, ignoreCase?, literal?, context?, limit? }`

- The default `limit` is 100 and the default `context` is 0.
- Search runs natively with `grep-searcher` + `grep-regex`, walking the
  tree with `ignore::WalkBuilder`. Hidden files are included and
  `.gitignore` is respected.
- **Output format:**
  - a match line is `path:N: text`;
  - a context line is `path-N- text`;
  - each line is capped at 500 characters, and the total at 50 KiB.
- **Deliberate difference from pi:** there is no `rg --json` subprocess.
  pi drops matches on lines that contain U+2028 or U+2029 (`grep.ts:169`).

## find: `{ pattern, path?, limit? }`

- `pattern` is a glob, and the default `limit` is 1000.
- A pattern that contains `/` matches against the full path, with an
  implicit `**/` prefix.
- Search uses `globset` together with `ignore::WalkBuilder`.
- The "limit reached" notice appears only when there actually were more
  results. pi shows it when the count equals the limit (`find.ts:145`).

## ls: `{ path?, limit? }`

- The default `limit` is 500.
- Entries are sorted case-insensitively. Directories get a trailing `/`,
  and dotfiles are included.
