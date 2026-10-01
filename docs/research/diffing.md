# Diffing in Rust

- Status: research
- Date: 2026-09-28

tau makes diffs in two places today. The `edit` tool
(`crates/plugins/tau-tools/src/edit.rs`, `generate_diff`) uses `similar` to
build a unified diff with 4 lines of context, and returns it to the
model. The GPUI app reads that text back (`parse_diff` in
`crates/tau-ui-remote/src/view.rs`) and draws it line by line in
`crates/tau-ui-remote/src/ui/transcript.rs`. It drops hunk headers, has no line
numbers, no intra-line highlights and no syntax colors.

This note surveys the Rust options and says which to use for each job.

## What tau needs

1. **The edit tool's diff for the model.** A unified diff. It must be
   compact (tokens cost money) and stable: the same edit must give the
   same bytes across runs and releases, or replay tests and cached
   prefixes break.
2. **The UI's rich diff.** Inline and side-by-side views, word or
   character highlights inside changed lines, syntax colors, line
   numbers. This wants structured hunks, not text to re-parse.
3. **Comparing forks of runs.** Two runs that fork from one checkpoint
   can leave two different working trees. We need whole-tree diffs:
   added, removed, renamed files, then per-file diffs.
4. **Large files and bad inputs.** Generated files, lockfiles and
   minified code. A diff must not take seconds or hang the agent loop.

## Background

- **Myers** finds a shortest edit script in O(ND) time, where D is the
  number of edits. It is fast when files are similar and slow when they
  are not. Production versions add heuristics that cap the work and give
  up on minimality.
- **Patience** matches lines that are unique in both files first, then
  recurses between them. Output reads better for code, because it
  anchors on lines such as function signatures.
- **Histogram** (from JGit, used by Git) extends Patience to lines that
  are rare, not only unique. It is usually the fastest and reads as well
  as Patience.
- **LCS table** is O(NM) time and memory. Only fine for small inputs.
- **Slider heuristics** (Git's indent heuristic) move an ambiguous hunk
  up or down so it starts and ends at natural boundaries. They change
  output, not correctness.
- **Semantic cleanup** (from diff-match-patch) merges tiny equal runs
  into the surrounding change, so a character diff reads like words.
- **Structural diffs** parse both sides (tree-sitter) and diff syntax
  trees. They ignore formatting and show moved nodes, but cost much more.

## Line and text diff crates

### similar (current)

- Version 3.2.0 (2026-08-17). Apache-2.0. MSRV 1.85. By Armin Ronacher.
  About 52M downloads in the last 90 days.
- No required dependencies. Optional: `bstr`, `unicode-segmentation`,
  `serde`, `hashbrown`, `web-time`.
- Algorithms: `Myers` (heuristic, the default), `RawMyers`, `Patience`,
  `Histogram`, `Hunt`, `Lcs`. All take a deadline and fall back when it
  passes.
- Text: diffs by lines, words, characters, graphemes, or any slice.
- Output: unified diff (`unified_diff()`), iterators of `DiffOp` and
  `Change`, and inline changes (`iter_inline_changes`, `inline`
  feature) that mark the changed words inside a changed line. 3.0 added
  `InlineChangeOptions` with semantic cleanup.
- 3.2.0 added `TextMerge`, a line-based three-way merge with merge and
  diff3 conflict styles, and `WhitespaceMode` (Git's `-b` and `-w`).
- 3.2.0 also changed `Myers` output to use Git-style bounded, non-minimal
  splits. The same inputs can give a different diff after an upgrade.
  `RawMyers` keeps the old output.
- No patch parser and no patch apply.

### imara-diff

- Version 0.2.0 (2025-06-14). Apache-2.0. MSRV 1.61 (follows Firefox).
  By Pascal Kuthe (Helix). About 9M downloads in the last 90 days.
- Dependencies: `hashbrown`, `memchr`.
- Algorithms: `Histogram` (port of Git's, the recommended one) and
  `Myers` (linear space, with preprocessing that avoids quadratic cases).
- Input is interned (`InternedInput`, `TokenSource`), so any token kind
  works: lines by default, words or characters with a custom source.
- `postprocess_lines` applies an indent heuristic like Git's.
- Output: `Diff::hunks()` (ranges), `is_removed`/`is_added` per token,
  and a unified diff printer (`unified_diff` feature, on by default)
  with a pluggable `UnifiedDiffPrinter`.
- No inline word diff built in, no merge, no patch parsing.
- The README claims Histogram beats Myers by 10% to 100%, and imara-diff
  beats `similar` by up to 30 times on real Git histories (Linux,
  rustc, VS Code, Helix). Those numbers predate `similar` 3.x. Our own
  run is below.
- Users: Helix, Zed (`crates/language/src/text_diff.rs`), difftastic
  (its line-diff fallback), Deno, Pijul, gitu, SpacetimeDB.
- No release since June 2025. gitoxide now keeps a modified copy,
  `gix-imara-diff` 0.3.0 (Apache-2.0, updated 2026-09-25), which is the
  better-maintained line.

### gix-diff

- Version 0.68.0 (2026-09-25). MIT OR Apache-2.0. Part of gitoxide,
  released often.
- `blob`: text diffs on top of `gix-imara-diff`, with Git's
  attribute-driven text conversion and binary detection.
- `tree`, `tree_with_rewrites`, `index`: tree-to-tree and index diffs,
  with rename and copy tracking (`Rewrites`).
- Pulls in `gix-object`, `gix-hash` and, for full features, a large part
  of gitoxide. It assumes Git objects.

### diffy

- Version 0.5.2 (2026-08-31). MIT OR Apache-2.0. `no_std` by default.
- Myers with a compaction pass.
- Makes, prints (optional ANSI colors), parses and applies unified
  patches. Apply searches forward and back when line numbers are off,
  like GNU patch.
- Three-way merge (`merge`, `merge_bytes`) with conflict markers.
- 0.5 added `patch_set` for multi-file Git patches, and binary patches
  behind a feature.
- Slow on bad inputs: 3.5 s on our reversed-file case below.
- A fork, `diffy-imara` 0.3.2 (2025-02), swaps in imara-diff; it looks
  inactive.

### dissimilar

- Version 1.0.11 (2026-03-15). Apache-2.0. By David Tolnay. No
  dependencies.
- A port of Google's diff-match-patch diff: Myers on characters plus
  semantic cleanup. Returns `Chunk::Equal`, `Delete`, `Insert` as
  `&str`.
- Good for one pair of short strings, such as two versions of a line.
  Not for whole files. Used by `expect-test`.

### diffs

- Version 0.5.1 (2023-03-10). MIT/Apache-2.0. No repository link on
  crates.io.
- Myers, Patience and a "replace" wrapper driven through a callback
  trait (`Diff`). Pijul used it before moving on.
- No releases in three years. Not a candidate.

### difflib

- Version 0.4.0 (2018-07-22). MIT.
- A port of Python's `difflib`: `SequenceMatcher`, `unified_diff`,
  `context_diff`, `Differ`, `get_close_matches`.
- Many downloads come through old test tools. Unmaintained. Not a
  candidate.

### diff

- Version 0.1.13 (2022-06-29). MIT OR Apache-2.0.
- `diff::lines`, `diff::chars`, `diff::slice`. LCS table, so O(NM)
  memory. No unified output.
- Very high downloads through `pretty_assertions`. Unmaintained. Not a
  candidate.

### git2

- Version 0.21.0 (2026-05-18). MIT OR Apache-2.0.
- Bindings to libgit2 (C) through `libgit2-sys`. Pulls OpenSSL on Unix
  unless features are turned off.
- Myers by default; `patience` and `minimal` flags; Git's indent
  heuristic. Tree-to-tree, tree-to-workdir and index diffs with rename
  detection. `Patch::from_buffers` diffs two buffers without a repo.
- Output: patch, stat, name-only, and a callback per file, hunk and
  line.
- A C build and a Git repository model. Not worth it next to `gix-diff`
  or `jj-lib`.

### jj-lib and jj-core

- jj-lib 0.45.1 (2026-09-03). Apache-2.0. The diff and merge code moved
  to a new `jj-core` crate (also 0.45.1), which jj-lib re-exports.
  Monthly releases, but the API changes between releases.
- `jj_core::diff`: its own Histogram-style algorithm. It builds
  histograms of rare lines, takes an LCS of them, and recurses.
  `ContentDiff` takes any number of inputs, not only two. It diffs by
  line, then refines changed regions by word and by non-word runs, so
  intra-line detail comes free. Whitespace modes: exact, ignore amount,
  ignore all.
- `jj_lib::diff_presentation::unified`: Git-style unified hunks with
  word-level tokens (`unified_diff_hunks`).
- `jj_lib::merged_tree::MergedTree::diff_stream`: whole-tree diffs, with
  copy records. Trees can hold conflicts.
- `jj_core::merge::Merge<T>` and `jj_lib::files::merge`: n-way merges.
  Conflicts are first-class values, and they print in `diff`,
  `snapshot` or `git` (diff3) marker styles.
- `jj-core` alone pulled 72 crates in our test build. `jj-lib` with the
  Git backend pulls `gix` too.
- jj's diff was not written for pathological input. On our reversed
  case it matched almost nothing (3 hunks) in 19 ms.

## Structural and syntax-aware diffing

- **difftastic** 0.72.0 (MIT, active). Parses both sides with
  tree-sitter (about 60 grammars), then finds a lowest-cost path through
  a graph of positions in both trees with Dijkstra. Above 1 MB
  (`DFT_BYTE_LIMIT`) or 3M graph vertices (`DFT_GRAPH_LIMIT`) it falls
  back to a line diff with imara-diff Histogram. The crate only ships the
  `difft` binary, no library. We could only run it as a subprocess.
- **diffsitter** 0.9.0 (MIT, April 2025). Diffs tree-sitter leaf nodes
  with an LCS. Has a lib target but calls itself "nowhere close to
  production ready". Low use.
- **tree-sitter** 0.27.0 and **tree-sitter-highlight** 0.27.0 (MIT,
  2026-08-30). Parsing and highlight queries. The base for syntax
  colors, and for our own structure-aware tricks (for example, choosing
  hunk boundaries at node edges).
- **syntect** 5.3.0 (MIT, 2025-09-27). Sublime Text grammars with regex
  engines (`onig` or pure-Rust `fancy-regex`). `two-face` 0.5.2 adds
  bat's extra grammars. Easier to set up than tree-sitter (one crate, no
  per-language C grammars), slower, and no parse tree.
- **gpui-component** 0.7.0 (Apache-2.0) has a tree-sitter code view for
  GPUI. It pins `gpui-pre =0.3.7`, the GPUI tau-ui uses since
  2026-09-30.

A structural diff is useful for review, not for the model. The model
applies edits by exact text, so it must see text diffs. For the UI, a
line diff with word highlights plus syntax colors covers most of the
value at a small cost.

## Intra-line highlights

Three ways, all cheap because they run only on changed hunks:

- `similar` `iter_inline_changes`: per changed line, the changed word
  ranges. Needs `inline` (and `unicode-segmentation` for Unicode word
  bounds).
- `dissimilar::diff` on each paired old/new line: character chunks with
  semantic cleanup. Good for short lines.
- `jj_core::diff::ContentDiff::by_word` on a changed hunk: word, then
  non-word refinement, the same output `jj diff` shows.
- `imara-diff` with a word `TokenSource`: works, but we would write the
  tokenizer and the pairing of old and new lines.

Pairing matters as much as the diff. When a hunk has 3 removed and 3
added lines, pair them by position. When counts differ, pair by
similarity and leave the rest as whole-line changes.

## Benchmark

We ran one on 2026-09-28 (AMD Ryzen 9 7950X3D, release build, mean of 5
runs). Each cell is diff plus unified text output, with 3 lines of
context. Inputs are real: `syn` 2.0.119 against `syn` 3.0.6.

| Case                         | similar Myers | similar Patience | similar Histogram | imara Histogram | imara Myers |   diffy |
| ---------------------------- | ------------: | ---------------: | ----------------: | --------------: | ----------: | ------: |
| One-line edit, 4.2k lines    |       0.65 ms |          1.08 ms |           0.27 ms |         0.14 ms |     0.14 ms | 0.32 ms |
| `expr.rs` 2 to 3, 4.2k lines |       0.99 ms |          1.21 ms |           6.58 ms |         0.23 ms |     0.23 ms | 0.55 ms |
| All `src/`, 28k lines        |       11.7 ms |          12.3 ms |           59.2 ms |          2.4 ms |      2.6 ms | 13.0 ms |
| 28k lines against reversed   |        135 ms |            33 ms |             63 ms |          3.7 ms |       41 ms | 3545 ms |

`jj_core::diff` (hunks only, no text): 1.0 ms, 1.0 ms, 6.7 ms and 19 ms
by line; 1.0, 1.3, 9.5 and 37 ms with word refinement.

Character diff of one 6.4k-character line pair: `dissimilar` 12 ms (32
chunks after cleanup), `similar` by chars 8.5 ms (1128 ops, no cleanup).

What this says:

- For an edit tool call, every crate is under 1.1 ms. Speed does not
  decide use (1).
- imara-diff Histogram is 4 to 36 times faster than `similar`'s default,
  and stays fast on the bad case.
- `similar`'s Histogram is slower than its Myers on real inputs, despite
  3.2.0's "avoid quadratic scans" note.
- `diffy` needs a size guard.
- Output sizes differ by under 2%. Histogram and Patience give slightly
  smaller diffs on code.

The bench code is not in the repo. It is 80 lines and easy to redo.

## Comparison

| Crate          | Version | Last release | License        | Algorithms                            | Unified out | Structured hunks | Intra-line    | Patch apply | 3-way merge       | Tree diff             | Deps             |
| -------------- | ------- | ------------ | -------------- | ------------------------------------- | ----------- | ---------------- | ------------- | ----------- | ----------------- | --------------------- | ---------------- |
| similar        | 3.2.0   | 2026-08      | Apache-2.0     | Myers, Patience, Histogram, Hunt, LCS | yes         | yes              | yes (words)   | no          | yes (`TextMerge`) | no                    | none required    |
| imara-diff     | 0.2.0   | 2025-06      | Apache-2.0     | Histogram, Myers                      | yes         | yes              | custom tokens | no          | no                | no                    | 2                |
| gix-imara-diff | 0.3.0   | 2026-09      | Apache-2.0     | Histogram, Myers                      | yes         | yes              | custom tokens | no          | no                | no                    | 2                |
| gix-diff       | 0.68.0  | 2026-09      | MIT/Apache-2.0 | via gix-imara-diff                    | yes         | yes              | no            | no          | no (gix-merge)    | yes, renames          | gitoxide         |
| diffy          | 0.5.2   | 2026-08      | MIT/Apache-2.0 | Myers                                 | yes         | partly           | no            | yes         | yes               | no (multi-file parse) | none             |
| dissimilar     | 1.0.11  | 2026-03      | Apache-2.0     | Myers on chars + cleanup              | no          | chunks           | yes (chars)   | no          | no                | no                    | none             |
| diffs          | 0.5.1   | 2023-03      | MIT/Apache-2.0 | Myers, Patience                       | no          | callbacks        | no            | no          | no                | no                    | none             |
| difflib        | 0.4.0   | 2018-07      | MIT            | Ratcliff/Obershelp                    | yes         | opcodes          | no            | no          | no                | no                    | none             |
| diff           | 0.1.13  | 2022-06      | MIT/Apache-2.0 | LCS table                             | no          | yes              | chars         | no          | no                | no                    | none             |
| git2           | 0.21.0  | 2026-05      | MIT/Apache-2.0 | Myers, patience, minimal              | yes         | callbacks        | no            | yes (repo)  | yes (repo)        | yes, renames          | libgit2, OpenSSL |
| jj-core/jj-lib | 0.45.1  | 2026-09      | Apache-2.0     | Histogram-like, n-way                 | yes (lib)   | yes              | yes (words)   | no          | yes, n-way        | yes, copies           | 72+ (lib more)   |
| difftastic     | 0.72.0  | 2026-09      | MIT            | tree-sitter + Dijkstra                | no          | binary only      | syntax nodes  | no          | no                | no                    | binary           |
| diffsitter     | 0.9.0   | 2025-04      | MIT            | tree-sitter leaves + LCS              | no          | yes              | syntax nodes  | no          | no                | no                    | tree-sitter      |

## Recommendation

Use different crates for different jobs, behind one small tau type.

1. **Edit tool (model-facing): keep `similar`, pin the algorithm.**
   Speed does not matter at this size, and `similar` has the smallest
   dependency cost. Set the algorithm explicitly (`Histogram` or
   `Patience` for code-shaped hunks; `RawMyers` if we want output that
   no heuristic change can move), pin the exact version, and keep a
   snapshot test of the diff text so any upgrade that changes bytes is a
   visible change. Keep 4 lines of context or drop to 3; both are fine.
2. **UI rich diff: send structure, not text.** Make the edit tool
   return hunks (`old_start`, `new_start`, lines with a kind) in its
   result details next to the text. Compute word highlights with
   `similar`'s inline changes on paired lines, falling back to
   `dissimilar` for short single-line changes. Color syntax with
   tree-sitter-highlight, one grammar per language we care about (Rust,
   TOML, Markdown, JSON, shell to start); syntect only if we want many
   languages fast and accept regex speed.
3. **Fork comparison: use jj-lib.** If tau adopts jj-lib for workspaces,
   `MergedTree::diff_stream` gives tree diffs with copies, and
   `jj_core::diff` gives line and word hunks in one pass. Its n-way
   `Merge<T>` also covers "merge fork B back into A". Without jj-lib,
   `gix-diff` is the next choice.
4. **Large files: guard, then imara-diff.** Before diffing, check size
   (for example 1 MB or 20k lines) and binary content. Above the limit
   show a stat, not a diff. If we need fast diffs on big text (whole-tree
   fork views of generated files), use `gix-imara-diff` Histogram, which
   is 4 to 36 times faster here. Always pass a deadline to `similar`.
5. **Patch apply and merge.** The edit tool does not need patch apply.
   If we add an `apply_patch` tool (the Codex format is not unified
   diff anyway), use `diffy` for unified patches with a size guard. For
   three-way merge on plain text use `similar::TextMerge`; for merges
   between runs, jj.

Not recommended: `diff`, `difflib`, `diffs` (unmaintained), `git2`
(C build, repo model), difftastic and diffsitter (no usable library,
cost too high for the value in an agent UI).

## Migration plan

No step needs to replace `similar`. Each step is small and can land
alone.

1. **Pin behavior.** Set `.algorithm(...)` and `.timeout(...)` in
   `generate_diff`. Add snapshot tests (insta-style, or plain string
   asserts) for a few edits: one-line change, insert at top, delete at
   end, no trailing newline, CRLF. Record the choice in `tools.md`.
2. **Add a diff model.** In `tau-ai` or a small `tau-diff` module:
   `FileDiff { path, hunks: Vec<Hunk> }`, `Hunk { old_start, new_start,
lines: Vec<Line> }`, `Line { kind, old_no, new_no, text, spans }`,
   where `spans` are changed byte ranges. Build it from `similar` ops.
   Render the unified text from this model too, so both views agree.
3. **Return it from `edit`.** Put the hunks in the tool result details
   (not in the model-facing text). Keep `diff` as text for pi
   compatibility.
4. **Use it in tau-ui.** Replace `parse_diff` with the structured
   hunks. Add line numbers, hunk headers, and highlighted spans. Keep
   `parse_diff` as a fallback for stored runs that only have text.
5. **Syntax colors.** Add `tree-sitter-highlight` with a few grammars
   in tau-ui only, behind a feature so the library stays light. Map
   highlight names to theme colors.
6. **Side-by-side.** A second layout over the same hunks: removed lines
   left, added lines right, context on both, blank rows to align.
7. **Fork diffs.** When jj-lib lands, add a tree diff view built from
   `diff_stream`, converting jj hunks into the same `FileDiff` model.
8. **Large-file path.** Add the size and binary guard. Swap the engine
   under `FileDiff` to `gix-imara-diff` only if profiling shows diffs on
   the hot path; the model type hides the change.

## Open questions

- Should the model see Histogram or Myers output? Histogram hunks read
  better in code, but we have no evaluation of model edit accuracy
  either way.
- Does tau-ui take `gpui-component`, or keep its own widgets and add
  tree-sitter directly?
- How big are real fork diffs? If they are small, jj's diff is enough
  and imara-diff is never needed.

## Sources

- similar: <https://github.com/mitsuhiko/similar>,
  <https://docs.rs/similar/3.2.0>, CHANGELOG in the 3.2.0 crate source.
- imara-diff: <https://github.com/pascalkuthe/imara-diff>,
  <https://docs.rs/imara-diff/0.2.0>.
- gix-imara-diff and gix-diff: <https://crates.io/crates/gix-imara-diff>,
  <https://docs.rs/gix-diff/latest/gix_diff/>,
  <https://github.com/GitoxideLabs/gitoxide>.
- diffy: <https://docs.rs/diffy/latest/diffy/>,
  <https://github.com/bmwill/diffy>.
- dissimilar: <https://github.com/dtolnay/dissimilar>.
- diffs: <https://crates.io/crates/diffs>.
- difflib: <https://github.com/DimaKudosh/difflib>.
- diff: <https://github.com/utkarshkukreti/diff.rs>.
- git2: <https://github.com/rust-lang/git2-rs>.
- jj: <https://github.com/jj-vcs/jj> (`core/src/diff.rs`,
  `core/src/merge.rs`, `lib/src/files.rs`, `lib/src/merged_tree.rs`,
  `lib/src/diff_presentation/unified.rs`, `lib/src/conflicts.rs`),
  <https://docs.rs/jj-lib/latest/jj_lib/diff/index.html>.
- difftastic: <https://difftastic.wilfred.me.uk/diffing.html>,
  <https://github.com/Wilfred/difftastic> (`Cargo.toml`,
  `src/options.rs`, `src/diff/lcs_diff.rs`).
- diffsitter: <https://github.com/afnanenayet/diffsitter>.
- tree-sitter: <https://github.com/tree-sitter/tree-sitter>.
- syntect: <https://github.com/trishume/syntect>; two-face:
  <https://crates.io/crates/two-face>.
- gpui-component: <https://crates.io/crates/gpui-component>.
- Zed's use of imara-diff:
  <https://github.com/zed-industries/zed/blob/main/crates/language/src/text_diff.rs>.
- Helix's use of imara-diff:
  <https://github.com/helix-editor/helix/blob/master/helix-vcs/Cargo.toml>.
- Versions, dates, licenses and download counts: crates.io API, read
  2026-09-28.

Unverified:

- The imara-diff README figures ("up to 30 times", "10% to 100%") are
  the author's, measured against `similar` 2.x.
- diffsitter's algorithm details come from its README, not its code.
- Users of imara-diff other than Helix, Zed and difftastic come from
  the crates.io reverse-dependency list, not from reading their code.
