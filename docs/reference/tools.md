# Coding tools (`tau-tools-host`, optional)

Related artifact references: [storage](codemode-artifact-storage.md),
[file artifacts](codemode-artifact-files.md),
[ranges](codemode-artifact-ranges.md), and
[retention](codemode-artifact-retention.md). The offline
[evaluation matrix](codemode-evaluation.md) exercises owned grants and pages.

These specs follow pi's built-in tools
(`packages/coding-agent/src/core/tools/`). Keep the limits and error
strings as written here. Models have seen them in pi's transcripts. pi
has no evals that measure tool behaviour, so a change to a string or a
limit cannot be checked against evals. Treat any change as a behaviour
change.

The shared limits:

| Constant        | Value          |
| --------------- | -------------- |
| `MAX_LINES`     | 2000           |
| `MAX_BYTES`     | 50 KiB         |
| `GREP_MAX_LINE` | 500 characters |

The truncation helpers:

- `truncate_head` keeps whole lines from the start.
- `truncate_tail` keeps whole lines from the end. When the last line
  alone is over the byte limit, it keeps the end of that line after
  `…`, cut at a UTF-8 boundary. The `…` counts toward the byte limit.
  A limit under 3 bytes cannot hold it, and keeps the end unmarked.
  **Deliberate difference from pi:** pi keeps the end without a mark.

A tool that fails returns `Err`, and the loop marks the result
`is_error`. Every tool takes a root directory (`tau_tools_host::path::Root`)
at construction, and all paths resolve against it.
`tau_tools_host::plugin::CodingTools::new(root)` adds all eight to an agent
as a plugin (`Agent::plugin`); `only` and `without` pick a subset, which
keeps pi's order. `tau_tools_host::coding_tools(&root)` returns the same
eight, for `Agent::tools`. `exec_command`, and so both, is unix-only.

## read: `{ path, offset?, limit? }`

- `offset` counts lines from 1.
- **Path resolution:**
  - `~` and a leading `~/` expand to the home directory; any other
    leading `~` is literal, so `~draft.md` is a file named `~draft.md`;
  - a leading `@` is stripped;
  - Unicode spaces are handled;
  - macOS screenshot filename variants are retried: NNBSP before AM/PM,
    NFD normalization, and curly apostrophes. AM/PM matches in either
    case, because some locales (such as en_AU) write `am` and `pm`.
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

## exec_command and write_stdin

Run a command, and go on with it ([0038](../decisions/0038-commands-run-in-the-background.md)).
They follow Codex's unified exec, not pi's `bash`: a command that is not
done when the call returns keeps running, and a second tool reads it.

### exec_command: `{ cmd, workdir?, yield_time_ms?, tty? }`

- `cmd` runs as `<shell> -c <cmd>` on a pseudo-terminal, in `workdir`
  (relative to the working directory; an error when it is not a
  directory) or the working directory.
- `yield_time_ms` is how long the call waits for the command to end:
  10000 by default, between 250 and 30000. A command that ends sooner
  answers at once.
- `tty` puts the command's stdin on the terminal, so `write_stdin` can
  send it input. Off by default: stdin is `/dev/null`, and a program
  that reads it gets end of file.
- **Shell:** `/bin/bash`, then `bash` on `PATH`, then `sh`
  (`exec::choose_shell`).
- **Environment:** the command inherits tau-ui's environment, and starts
  through what plugins give the repository, when one does: words before
  the shell and variables over its environment, such as `direnv exec
<workspace>` from tau-direnv ([environment.md](environment.md),
  [0025](../decisions/0025-agent-commands-run-in-an-environment-plugins-give.md)).
  The first command may wait for it; a cancel while it waits ends the
  call before anything ran.
- **The terminal:** 120 columns by 40 rows, fixed. stdout and stderr are
  the terminal. The command leads its own session and process group,
  with the terminal as its controlling terminal, and starts with
  `SIGHUP` ignored. `TERM=xterm-256color`, `PAGER=cat`, `GIT_PAGER=cat`,
  `MANPAGER=cat`, `LESS=-FRX`. Reading `/dev/tty` of a command without
  `tty` gets end of file at once.

### write_stdin: `{ process_id, chars?, yield_time_ms? }`

- `process_id` is the id a still running answer named.
- `chars` is written to the process's terminal as typed: `\n` ends a
  line, `\u0003` is `^C`, which the terminal turns into `SIGINT`, and
  `\u0004` is end of input. A process started without `tty` has no
  input: the call fails, saying so.
- Without `chars`, the call only polls, and waits at least 5 seconds
  (5000 to 300000). With them it waits 1000 by default (250 to 30000).
- It answers with what the process wrote since the last call, as
  `exec_command` does. An unknown id fails: `No process N: it ended and
was reported, or was never started.`

### The answer

- **Text:** what the process wrote since the last call that read it,
  once. A call that waited for nothing new says `(no new output)`, and
  a command that ended without output `(no output)`.
- **Still running:** the text ends with `[process N is still running:
write_stdin with process_id N ...]`, and the call succeeds.
- **Ended:** zero exit is the output alone. A non-zero exit returns
  `Err` whose text is the output, then `Command exited with code N`
  (128 + N for signal N). The process leaves once its end is reported.
- **Plain text:** the terminal's bytes become the model's text through
  `exec::clean::Cleaner`: `\r\n` reads as a line end, a `\r` redraw
  (a progress bar) or an erase in line leaves only its last frame,
  backspace and tab move the cursor, colors, titles and other escape
  sequences are dropped. A line still being written is said as far as
  it is, and not said again once it ends. **Deliberate difference from
  0010:** this is not the screen's own text, which cannot be read
  incrementally between calls.
- **Limits:** the answer is `truncate_tail` of the new text (2000 lines
  or 50 KiB), headed by `[output cut to the last N of M lines]` when it
  cut. At most 1 MiB of unread text is kept per process, the oldest
  lines going first, and a note says how many.
- **`structured`** (for codemode): `{ process_id, running, exit_code,
output, truncated, dropped_lines }`.
- **Progress:** each burst of output is sent as a `ToolUpdate`, the raw
  bytes in its details:

  ```json
  { "term": { "seq": 0, "bytes": "<base64>" } }
  ```

  `seq` counts the call's chunks from 0. The chunks, decoded and joined,
  are every byte the call read, escape sequences included.

- **Result details**, on success and on failure:

  ```json
  {
    "term": {
      "cols": 120,
      "rows": 40,
      "status": "running",
      "exitCode": null,
      "process": 3,
      "chunks": 12,
      "outputBytes": 5120,
      "replay": "stream",
      "lost": false,
      "bytes": "<base64>"
    }
  }
  ```

  `status` is `running` or `exited`; `exitCode` is the exit code once
  it exited. `bytes` is the raw output this call read, the newest 4 MiB
  of it (`lost` says some was dropped), and `outputBytes` counts all the
  process wrote so far. tau-ui draws it as the card's terminal screen
  ([terminal.md](terminal.md), "In tau-ui: the command card").

### Processes

- A run's processes live in its `exec::Processes`, which the host keeps
  per workspace so they outlive a turn. A task per process reads its
  terminal as it writes, so output is kept between calls.
- **Cancel:** a cancelled call does not stop the process. It fails with
  `Operation aborted` and says the process goes on; what it wrote
  meanwhile is the next call's.
- **Limit:** 64 processes. Past it, the least recently used goes (one
  that exited first), and the 8 most recently used stay.
- **End:** dropping the processes, or closing the workspace they ran
  in, kills each process group (`SIGKILL`). Nothing outlives tau.

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
  8. Any failure leaves the file unchanged. Edits are all applied, or
     none are.
- **Line endings** (as pi does, `normalizeToLF` and
  `restoreLineEndings`):
  - the file's line ending is CRLF when its first `\n` ends a CRLF line,
    and LF otherwise;
  - `\r\n`, `\n` and a lone `\r` all count as line breaks, so a lone
    `\r` inside a line splits it in two;
  - the file is written back with its line ending on every line, so
    mixed endings become uniform, and a lone `\r` becomes that ending,
    even on lines no edit touches.
- **Deliberate difference from pi:** uniqueness is checked in the same
  space the match was found in. pi checks it in normalized space even for
  exact matches (`edit-diff.ts:328`). That rejects a match that is unique
  exactly but has a near-duplicate elsewhere.
- **Result details:** a unified diff (`similar`) and the first changed
  line.
- **Serialization:** each read-modify-write holds a per-path async mutex,
  keyed by canonical path. Parallel edits to the same file are
  serialized.

- **Diff consistency:** applying the returned diff to the original
  content gives exactly the bytes written.

## write: `{ path, content }`

Creates parent directories, then writes the file. Returns
`Successfully wrote to <path>`. Holds the same per-path mutex as `edit`.
Its details, for the card, are `diff` (a unified diff from what the
file held, or from nothing for a new file or one that was not text),
`firstChangedLine` and `created`. The card shows the diff folded.

## grep: `{ pattern, path?, glob?, ignoreCase?, literal?, context?, limit? }`

- The default `limit` is 100 and the default `context` is 0.
- Search runs natively on ripgrep's crates, `grep-searcher` +
  `grep-regex`, walking the tree with `ignore::WalkBuilder`. Hidden
  files are included and `.gitignore` is respected.
- The walk and the search use every core, as ripgrep does. Files are
  still reported in path order, and the search stops once the files
  finished in order hold more matches than `limit`, so the output is the
  same as a search of one file after another.
- **Output format:**
  - a match line is `path:N: text`;
  - a context line is `path-N- text`;
  - each line is capped at 500 characters, and the total at 50 KiB.
- **Deliberate difference from pi:** there is no `rg --json` subprocess.
  pi drops matches on lines that contain U+2028 or U+2029 (`grep.ts:169`).
- The "limit reached" notice appears only when there actually were more
  matches, as for `find` below. pi shows it when the count equals the
  limit.
- Only the tree's own `.gitignore` files apply, whether or not it is a
  git repository: the user's global gitignore and `.git/info/exclude`
  do not. The same holds for `find`.

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
- The "limit reached" notice appears only when there actually were more
  entries, as for `find`.
- **Result details:** a `Listing` for callers that draw it: `dir` (the
  resolved directory), `entries` and `truncated` (the entry limit or the
  byte cap cut the listing). `entries` are the ones the model got, in
  its order, each with `name`, `kind` (`dir`, `file`, `symlink` or
  `symlink_dir`), and when they apply `size` (files, in bytes),
  `modified` (Unix seconds), `items` (a directory's entry count),
  `target` (a symlink's) and `ignored` (`.gitignore` leaves it out, by
  the rules `find` and `grep` walk by). With `tau-vcs`, each entry that
  differs in `@` also gets a `change` (`vcs.md`, "ls").

## http: `{ method?, url, headers?, body?, timeout? }` (an agent's runs)

- Added by `CodingTools::with_http(store)`, which tau-ui calls for the
  runs of an agent's repository only (ADR 0037). It is `Nested`:
  scripts call it as `http.request` ([codemode.md](codemode.md),
  "HTTP"); the model is never declared it.
- A header is a string or a secret's marker, `{ "$secret": name,
"prefix"?: text }`. The body is a string sent as it is, a marker sent
  as its value, or any JSON value sent as `application/json` with its
  markers put in. A secret the store lacks fails the call, naming the
  secret, and nothing is sent.
- `timeout` is in seconds: 30 by default, at most 60. Methods: `GET`
  (default), `HEAD`, `POST`, `PUT`, `PATCH`, `DELETE`, `OPTIONS`. Only
  `http` and `https`. Redirects are not followed: a 3xx comes back as
  it is, so a secret never reaches a host the request did not name.
- **Result:** `{ status, headers, body, json, artifact, truncated }`.
  Headers are joined by `, ` when repeated. A body of at most 64 KiB is
  `body`, its bytes as UTF-8 (lossy); `json` is it decoded when the
  answer says it is JSON and it is at most 1 MiB, else null. A longer
  body is an artifact (`artifact`: `{ id, digest, size_bytes }`, read
  with `artifact_read`) when the run has artifact storage, else cut at
  64 KiB with `truncated` set. At most 16 MiB is read; past it,
  `truncated` is set too.
- **Scrubbing:** every value in the agent's secret store of at least 4 bytes
  is replaced by `[secret:<name>]` in the status's headers, the body
  (and the artifact's bytes) and every error, whether or not the
  request named it.
- **The store** is the agent's own, `secrets/<agent>.json` in tau's
  configuration directory (`SecretStore::of_agent`): `{ name: value }`,
  owner-only (0600, its directory 0700), replaced whole in one rename
  (`tau_tools_host::secrets::SecretStore`). An agent's requests reach
  its own keys and no other agent's. Names
  are 1 to 64 letters, digits, `_`, `-` and `.`. An agent's page lists
  the names its `agent.luau` uses and saves or removes values
  (`Act::SetSecret`, `Act::RemoveSecret`, each naming the agent); no interface reads a value
  back.
- `tau_tools_host::http::fetch` is the same request for the host
  itself, without a run (an agent's polls): scrubbed, its body whole as
  text (to 16 MiB) and decoded when it is JSON.

## Error strings

These strings are part of the spec. `<path>` is the path as the model
gave it. `N`, `M` and `K` are numbers.

| Tool                          | Condition                        | Message                                                                                                                                                             |
| ----------------------------- | -------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| all                           | cancelled                        | `Operation aborted`                                                                                                                                                 |
| `read`                        | offset past the end              | `Offset N is beyond end of file (M lines total)`                                                                                                                    |
| `edit`                        | file missing or not accessible   | `Could not edit file: <path>. Error code: <CODE>.` (for example `ENOENT`, `EACCES`)                                                                                 |
| `edit`                        | no edits                         | `Edit tool input is invalid. edits must contain at least one replacement.`                                                                                          |
| `edit`                        | empty `oldText`, one edit        | `oldText must not be empty in <path>.`                                                                                                                              |
| `edit`                        | empty `oldText`, several edits   | `edits[K].oldText must not be empty in <path>.`                                                                                                                     |
| `edit`                        | not found, one edit              | `Could not find the exact text in <path>. The old text must match exactly including all whitespace and newlines.`                                                   |
| `edit`                        | not found, several edits         | `Could not find edits[K] in <path>. The oldText must match exactly including all whitespace and newlines.`                                                          |
| `edit`                        | not unique, one edit             | `Found N occurrences of the text in <path>. The text must be unique. Please provide more context to make it unique.`                                                |
| `edit`                        | not unique, several edits        | `Found N occurrences of edits[K] in <path>. Each oldText must be unique. Please provide more context to make it unique.`                                            |
| `edit`                        | overlapping edits                | `edits[J] and edits[K] overlap in <path>. Merge them into one edit or target disjoint regions.`                                                                     |
| `edit`                        | no change, one edit              | `No changes made to <path>. The replacement produced identical content. This might indicate an issue with special characters or the text not existing as expected.` |
| `edit`                        | no change, several edits         | `No changes made to <path>. The replacements produced identical content.`                                                                                           |
| `exec_command`, `write_stdin` | non-zero exit                    | output, then `Command exited with code N`                                                                                                                           |
| `exec_command`                | `workdir` not a directory        | `workdir is not a directory: <path>`                                                                                                                                |
| `write_stdin`                 | unknown process                  | `No process N: it ended and was reported, or was never started.`                                                                                                    |
| `write_stdin`                 | input to a process without `tty` | `Process N has no input: its stdin is closed. Start it with `tty: true` to send it input.`                                                                          |

Taken from pi at `2b0a123`: `edit.ts:181`, `edit-diff.ts:253-347`,
`read.ts:143`. The command tools follow Codex's unified exec
(`codex-rs/core/src/unified_exec`).
