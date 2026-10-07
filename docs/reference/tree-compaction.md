# Tree compaction

`tau-tree-compaction` (`crates/plugins/tau-tree-compaction`) compacts a
run without losing what it drops. It is a port of OptChat's memory
([the spec](https://gist.github.com/VictorTaelin/91837951a5ce5b38f341ec1ba1df6449)),
fitted to a run of tau's. It is off by default: the Plugins screen
switches it on, and until an evaluation shows it does better than
tau-compaction's summary, tau-compaction stays the default.

[compaction.md](compaction.md) replaces the older messages with one
summary, and what the summary leaves out is gone for the rest of the
run. Tree compaction keeps those messages word for word, grows a binary
tree of one-line summaries over them, and puts a view of the tree in
their place. The `zoom` tool opens any line back into the two it was
made from, down to a message whole.

## Using it

```rust
use tau_compaction::Compaction;
use tau_tree_compaction::TreeCompaction;

let agent = Agent::new(llm)
    .plugin(TreeCompaction::default())
    // After it: summarizes when a line cannot be built.
    .plugin(Compaction::default());
```

Context plugins are offered the context in order, and the first that
rewrites wins. Put tree compaction after pruning and before
tau-compaction: when building a line fails, tree compaction writes
nothing, and tau-compaction summarizes in the same turn.

| Setting              | Default  | Meaning                                                        |
| -------------------- | -------- | -------------------------------------------------------------- |
| `reserve_tokens`     | 16,384   | Compacts once the estimate passes `context_window` minus this  |
| `keep_recent_tokens` | 20,000   | The newest messages kept as they are, as tau-compaction keeps  |
| `context_window`     | registry | For a model the registry does not know                         |
| `view_bytes`         | 64,000   | The view's budget, in bytes of its lines (about 16,000 tokens) |
| `jobs`               | 8        | Compactor requests at once                                     |

It triggers as tau-compaction does: after a turn past the threshold,
before the first request of a run on an inherited transcript, and on an
overflow. A failed compaction waits 2, 4, 8 and up to 32 turns before
it tries again.

## The history

Every message folded so far is kept as entries:

| Kind   | What                                                  |
| ------ | ----------------------------------------------------- |
| `user` | A user message: its text, an image as `[image]`       |
| `talk` | An assistant message's text                           |
| `tool` | One tool call: its name and its arguments as JSON     |
| `echo` | A tool result: `[tool] text`, or `[tool failed] text` |

An assistant message gives its text, then one entry per call. Its
thinking is left out, as OptChat leaves it out. An entry keeps at most
30,000 characters: a longer one keeps its head and tail around a note
of what was cut.

Entry ids count from 0 across the run and never change.

## The tree

Node `(l, i)` covers entries `[i·2^l, (i+1)·2^l)`. A level-0 node
compresses one entry; a node above merges its two children. A node is
named `id+n`, its first entry and how many it covers, which is the name
the model reads in the view and passes to `zoom`.

A line aims at 512 bytes. A line needs no model when its source fits:

- a level-0 line whose entry, as `kind: text`, fits in 512 bytes is that
  entry, word for word, so a short user message stays verbatim until it
  is merged;
- a parent whose two children fit together, one per line, is the two of
  them.

## The view

The view is a list of nodes that tiles every entry in order. Folding
appends a level-0 node per new entry, then merges until the view fits in
`view_bytes`:

1. Of every two siblings side by side, merge the most due: a pair at
   level `l` from entry `start` is due `(T - start) / 2^(l+2)`, where `T`
   is the number of entries. Ties go to the older pair.
2. A pair whose parent is not built is passed over.
3. Lines are never split: the view only appends and coarsens.

Older lines cover more messages, and each level keeps about as many
lines. Because a merge only coarsens, the start of the view stays the
same between compactions.

## Building lines

Lines are built when a compaction is due, not as messages arrive: most
runs never compact. First every new level-0 line, `jobs` at a time;
then, while the view is over budget, the parents the fit will want,
found by simulating it with each unbuilt line at the most it can be.
Those parents are built a level at a time, `jobs` at once.

Each line is one request to the run's model and effort, with
`COMPACT_PROMPT` as instructions. This is OptChat's compactor prompt,
written for a run: the user's words first, near verbatim, then lasting
effects and failures, then findings and replies, and tool output last,
described rather than copied. The request is one user message with two
text blocks:

1. **Context.** `<chat>` holds the view's built lines before the line,
   bare. For a level-0 line, `<recent>` follows with the entries from
   there up to its own, newest kept, within 6,000 characters. These
   are entries of the same batch whose lines are still being built.
2. **The step.** A line of exactly 512 bytes for scale, then the entry
   whole ("Compress this message into one line…"), or the two child
   lines ("Merge these two lines into one…").

No request shows a label: a compactor shown `id+n|text` lines copies
the format into its output (OptChat, section 4.2).

A reply over 512 bytes goes back in the same conversation, with its
size and the line cut where the limit falls. There are up to 5 tries,
and the shortest is kept. A request that fails, or answers nothing,
fails the compaction, which writes nothing.

## The rewrite

The transcript becomes the view message, followed by the kept messages.
The view message is a user message:

```
The conversation before this point was compacted into the lines below: …
zoom(id, n) opens line id+n into the two lines of n/2 messages …

<chat>
0+64|user: …; talk: …; echo: …
64+16|…
…
311+1|echo: [bash] cargo test: 41 passed
</chat>
```

When the transcript opens with the last view message, a later
compaction folds only what follows it. If another plugin replaced that
view message, whatever the transcript opens with is folded like any
other message.

## Zoom

`zoom(id, n)`:

- `n = 1` gives entry `id` whole, as `id+1|kind: text`;
- a larger `n`, a power of two that divides `id`, gives the two lines
  under `id+n`, labelled;
- anything else is `No line id+n.`;
- before any compaction, it says nothing has been compacted yet.

## What is stored

- **The rewrite** is a `context` entry whose body is the view (its
  nodes), how many entries it covers, the tokens before and the view
  message's timestamp.
- **A `folded` record per compaction** holds the entries it added, from
  `first` on, and the lines it built. These are records, not the
  rewrite, so each compaction stores only what it added.
- **A `compacted` record** says what the compaction did, for the run's
  plugin list: "312 messages in 140 lines".

A fork or a resumed run rebuilds the history from its `folded` records
and the last rewrite's view, so `zoom` still reaches everything folded
before it.

## Deviations from OptChat

- **Within a run.** OptChat answers every user message with a fresh
  call on the view alone. A tau run is one conversation whose tool loop
  needs its latest messages whole, so the view replaces only what falls
  before tau-compaction's cut, and the kept tail stays as it was.
- **Built when compacting**, not in the background as messages arrive.
  So a level-0 line's context is the view before the batch plus
  `<recent>`, not the view up to the line, which would build the
  batch's lines one at a time.
- **A smaller view:** 64,000 bytes, not 128,000, since it shares the
  window with the kept tail.
- **No `date` tool**, no subagents, and no cache breakpoints inside the
  view: the view changes only when the run compacts.
