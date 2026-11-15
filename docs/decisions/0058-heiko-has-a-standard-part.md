# 0058: `heiko` has a standard part, and packages share nothing else

- Status: accepted. Follows [0051](0051-luau-plugins-are-packages.md)
  (`require` is flat, within `src/`) and
  [0056](0056-luau-plugins-are-elm-over-coroutines.md) (commands).
- Date: 2026-11-15

## Context

A Luau package can require `heiko` and its own files, and nothing else.
So what several plugins need, each wrote. On 2026-11-15 the built-in
plugins held the same table of Unicode white space seven times, nine
`trim`s in five styles, eight cuts with an ellipsis, three ways to write
a count of tokens (`184k`, rounded or not), four wrappers that await the
store, four that ask Jev under `pcall`, and three that unwrap the
model's answer. The copies had drifted: two plugins rounded the same
number differently on the same screen.

Two ways out were open. Packages could depend on packages, as Lux has
it. Or heiko could give more.

## Decision

### `require("heiko")` gives a standard part

`heiko` gains `heiko.text`, `heiko.count`, `heiko.values`, `heiko.store`,
`heiko.jev`, `heiko.infer` and `heiko.now`. A function goes in when
several plugins wrote it by hand and it has one right answer. What one
plugin needs, what is a plugin's domain (a status's tone, a command's
arguments), and how things look stay out: the look is a question for the
view tree, not for this.

It holds no state. Nothing in it lets a plugin see another's model,
store or files.

### Packages still share nothing

No package requires another. A plugin that could read another's module
could come to depend on its tables, its state and its version, and a
person's plugin would break when a built-in changed. What is worth
sharing goes in the standard, where it is tested and kept.

`heiko` and every name starting with `heiko_` are heiko's: a package
that names a file so does not load.

### It is a package, treated differently

The source is `plugins/heiko-std`: a file a part, and tests as any
package's, with cases and properties. `built-in.toml` lists it under
`libraries`, so `heiko-plugin test` and CI run its tests with the
plugins', and nothing loads it as a plugin: it has no hooks, is not
listed, and cannot be turned off. Its files are read from heiko's
packages folder, once, as the plugins' are, and installed into `heiko`
in every VM, a plugin's and a test's alike, so a person's plugin has
them wherever heiko-plugin runs, and editing one compiles nothing.

### It is a contract

A function in the standard keeps its name and its meaning. Before 1.0
it may still change, with every caller in the same change; after, it is
added to, not altered. It is tested as the kernel is: properties for
each function, and `heiko.text.is_space` against Rust's
`char::is_whitespace` on every code point.

## Consequences

- Every built-in plugin uses the standard, and its copies are gone. Two
  outputs changed: heiko-reasoning's token counts round down, as
  heiko-fast-compaction's did, and a count of a million or more shows a
  tenth rounded down.
- `heiko.contains` is `heiko.values.contains`. `cmd.jev.noul`, `choice`
  and `score` are gone: no plugin used them, and `cmd.jev.ask` takes
  questions of each kind.
- heiko does not start agents without the standard's files, as it
  does not without a built-in plugin.
- What stays copied is what was left out on purpose: batched Jev and
  model calls through `heiko.all`, heiko-todo's buffered writes, and the
  layouts plugins repeat.
