# 0057: The interface is flat and printed, and the app is heiko

- Status: accepted, being implemented. Follows
  [0007](0007-gpui-interface.md) and
  [0017](0017-plugins-bring-their-ui.md): the look stays in
  `heiko-ui-kit`. Adds pieces to the view tree of
  [0045](0045-everything-is-a-plugin.md).
- Date: 2026-11-14

## Context

The interface grew a screen at a time. Each new thing got a rounded
card: a tool call, a plugin's note, a landing, a setting. State was a
pill with a tinted fill, a quantity a smooth bar, and every action a
bordered button, so a screen's one main action looked like the others
beside it. A chat with twenty tool calls was twenty boxes, and what the
model said between them was hard to find.

The sidebar listed every repository with its chats open under it, then
the agents, then a section of what is the same everywhere. With a few
repositories and agents it scrolled, and the chat on screen was a row
somewhere in it. The first screen was an empty composer: what had
happened in other chats since the person last looked was in the sidebar
rows' lines, a repository at a time.

Costs were shown in dollars in many places: today's total, a run's
cost, a turn's, a plugin's spend, a column in History and on the
compare screen. The app runs on the person's ChatGPT plan
([0012](0012-chatgpt-sign-in-only.md)), so no one pays those amounts.
They are what the same tokens would cost through the API.

The app was called tau, a name it shares with other software and a
letter that says nothing of what it does.

## Decision

The interface is redrawn in one language, called flat printed here, and
the app is named heiko. Colors stay Tokyo Night. The language is in
`heiko-ui-kit`, and the test that keeps raw design values out of every
other crate (`design::check`) stays.

### Structure comes from rules, not boxes

Nothing is boxed. There are no rounded cards, no gradients, no soft
shadows, no glows and no pills.

- A section is a title over a 2 px light rule (`section_header`,
  `heavy_rule`).
- Rows are separated by 1 px hairlines (`rule`).
- A warm color (amber, orange, yellow) is never a background fill. It
  is for text, rules and outlines.
- Text is short and plain. A screen has no captions that explain it:
  what a graphic says is not also written.

Page and section titles are in RocknRoll One, the display face
(`Face::Display`, `Type::SECTION`, `Type::PAGE_TITLE`, `Type::FIGURE`).
Body text stays Geist, and code, paths, ids, counts and times stay
Geist Mono.

### One decorative device: the offset print

The primary action of a screen is a flat blue block with dark text over
a hard pink offset, as a print out of register (`primary_block`,
`Material::printed`). A selected option is drawn the same way. It is
used once or twice a screen. A small one marks an action that waits on
the person (`hot_action`).

Every other action is an underlined word (`link_action`). There are no
bordered buttons.

### State is marks, stamps, squares, slips and ticks

- **Marks** (`mark`, `counted_mark`): seven small shapes, one per
  state: working, asks, failing, met, limit, cap, idle. A row packs its
  marks at its right edge, with a count above one.
- **Stamps** (`stamp`): a word or two in mono, outlined in its tone:
  `exit 0`, `queued`, `Draft`.
- **Squares** (`square`): a small square with a number or a letter: a
  step, a check, a place in a queue. Quiet when not reached, outlined,
  or filled.
- **Slips** (`slip`, `dashed_slip`): a note or an id on an outlined
  label over an offset; dashed for one proposed and not kept yet.
- **Tick strips** (`ticks`, `marked_ticks`): any quantity is a row of
  square ticks, some lit, with thresholds as white ticks. No smooth
  progress bars.
- **Figures** (`big_figure`): a large number in the display face, over
  what it counts.

### The sidebar is a rail and a tree

The sidebar is two columns. The rail holds Home, a square tile per
repository with a way to add one, and a tile per agent. The tile whose
tree shows is printed over the offset, and a pip on a tile says
something in it needs the person.

Beside the rail is the tree of the tile chosen: a repository's name
with its branch, a new run and search, its chats with their forks and
sub-agents under them on thin lines, then what plugins keep for it
(memory, constitution, MCP servers). An agent's tree has its
conversation with its sub-agents at work under it, what plugins keep
for it, and its repository. Under every tree is what is the same
everywhere: plugins, history, what plugins list there, models and
accounts.

A row's state is its marks. A row never moves for its state
([attention.md](../reference/attention.md)).

### Home starts a run and lists what happened

Home is a place to start a run: what to do, large, between two rules,
then where it starts and on what as underlined words that open what
changes them, and Start as the offset block. Beside it is a ruled list
of what happened since the person last looked, across runs and agents:
when, its mark, whose it is, one line, and the one thing to do about it
(answer, read, land, push). The things that wait on the person have
the hot action.

### A tool call is a line

A tool call is one line of the transcript: the tool's name in the color
of its kind (list, search, read, edit, run, blocked), its arguments,
and what came back at the line's end, as counts or a stamp. Its body,
when open, sits under it on a flat band. A refused call stands under a
red rule with what it proposed and why. The code and the references
still call a call's entry its card (`AtCard`, `CardView`, a Luau
plugin's `card`); it is drawn as a line.

The person's message is large type between two hairlines. The model's
words are plain prose in the column.

### Plugin notes draw their own small graphics

A plugin's note is a rule-topped entry with no box: a title, and a
small graphic of the plugin's own made from the pieces above. A goal
shows each check as a square and its continuations as ticks; the
constitution shows a score as ticks against its thresholds; fast
compaction shows the window before and after as two strips. A note may
lead with a large kanji as its plugin's mark (的 for a goal, 止 for a
block, 返 for an answer sent back, 報 for the watcher's note).

The view tree gets the pieces, so Rust and Luau plugins both draw them
and a phone draws the same: `Node::Ticks`, `Mark`, `Stamp`, `Square`,
`Slip`, `Figure` and `Kanji`; in Luau `ui.ticks`, `ui.mark`,
`ui.stamp`, `ui.square`, `ui.slip`, `ui.figure` and `ui.kanji`. A
button's kind chooses the offset block (`primary`) or the underlined
word.

Kanji are used only where the kit and the built-in plugins have them:
the 並 mark, numerals for turns, and the plugins' marks. They are
accents, and nothing can be understood only from one.

### No dollar amounts

The interface shows no amount of money. Today's total, a run's and a
turn's cost, plugins' spend and the cost columns go. This is
presentation only: the store keeps its cost fields, a plugin's charge
still counts toward its run's limits, and the evaluations still report
cost.

A limit set in dollars reads as the share of it used.

### The name and the wordmark

The app is heiko, from 並行 (heikō), "in parallel": it runs several
agents side by side. The crates, the binaries (`heiko`, `heiko-plugin`,
`heiko-phone`), the plugins (`heiko-memory`), the Luau global and the
folders it keeps (`~/.local/share/heiko`, `~/.config/heiko`) are named
for it. There are no users, so nothing is migrated.

The wordmark is 並行 and `heiko` in the display face, overprinted in
blue and pink like the primary action (`wordmark`). Small places show
並 alone (`logo`).

## Alternatives considered

- **The earlier directions.** Several were drawn before this one. Each
  was a theme laid over the same layout: other colors, radii and
  shadows on the same cards, pills and sidebar. They were rejected for
  that. What was wrong was the structure, and a theme does not change
  it.
- **Keep the cards and restyle them flat.** A chat is still a column of
  boxes. Lines let the model's words lead.
- **Keep one long sidebar, with sections that fold.** The person folds
  and unfolds by hand, and what needs them in a folded section is
  hidden. A rail shows every repository and agent at once, each with a
  pip.
- **Keep dollar amounts as "API equivalent".** They read as a bill no
  one gets, and they are not what limits the person: their plan's
  window is.
- **Icons for state.** The seven marks are fewer and are read at the
  size of a row's small print.

## Consequences

- A chat shows more of itself: a tool call takes a line where it took a
  card.
- A plugin can draw a graphic with no gpui, from seven more nodes. Old
  fixtures of the built-in plugins are made again.
- Every screenshot in the README and under `docs/screenshots` is of the
  old interface until `scripts/readme-screenshots.sh` takes them again.
- Nothing tells the person what a run used. Until plan usage is known
  (below), tokens and turns are the only measure shown.
- The kit keeps some old names with the new look (`card` and `panel`
  are unboxed blocks under a hairline, `link` is the underlined action,
  `bar` draws ticks), so screens not yet redrawn already fit.
  Screens move to the new pieces, and the old names go when nothing
  uses them.

## Open questions

- **Plan usage.** The interface should show how much of the plan's
  window is used ("38% of this 5-hour window") where it showed dollars,
  and the rail's foot is kept for it. Nothing reports it yet: the data
  model has no such figure, so nothing is shown and none is made up.
- **An agent's daily cap** was set in dollars; it is now a number of
  tokens a day ([0037](0037-agents.md)), so it is no longer open.
- **`/goal --budget`** was in dollars; it is now a number of tokens,
  `500k` or `2m`, and so is its popover's field
  ([goal.md](../reference/goal.md)), so it is no longer open.

## Steps

1. The kit: the display face, the theme's `bright`, `rule`, `hairline`
   and `offset`, and the new components. Done.
2. The view tree's new nodes and their Luau builders.
3. The chrome: the rail and tree, Home, the status bar, the phone's
   lists and headers.
4. The transcript: tool calls as lines, the person's message, notes.
5. Each screen, and each built-in plugin's notes, pages and panes.
6. Money out of every screen.
7. The docs and the screenshots.
