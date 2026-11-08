# 0050: tau's own plugins, in Luau

- Status: accepted.
- Date: 2026-11-08

## Context

ADR 0045 gave Luau plugins the kernel's interface: every seam, slot,
point and service is a hook a Luau plugin writes, or is left out with a
reason (`tau-luau-plugins-host/src/parity.rs`). ADR 0048 built one of
tau's own plugins, tau-todo, in Luau, and ADR 0049 gave Luau tests
properties and state machines. Every other plugin tau ships is Rust.

A seam no plugin of tau's uses is a seam no one checks. The interface
is only as good as what has been built on it, and so far the Luau side
has been exercised by examples and one plugin. Moving tau's own
plugins to Luau puts real weight on every seam, the way a person's
plugins will, and finds where the interface falls short before they
do.

An inventory of the 25 plugin crates (tau-codemode, tau-luau-plugins
and tau-todo left aside, as they run Luau or are Luau already) sorted
them by what they reach outside plain logic:

- **Two can move now**: tau-compaction-chain (it only orders other
  rewriters) and tau-titles (a model call with a JSON schema).
- **Eleven can move once Luau reaches more**: tau-watcher, tau-reasoning,
  tau-goal, tau-constitution, tau-fast-compaction, tau-compaction,
  tau-tree-compaction, tau-ask, tau-schedule, tau-canvas and tau-github.
- **Twelve stay Rust**, as they are runtimes, stream, or hold the
  operating system's resources: tau-vcs (jj-lib), tau-mcp (server
  processes and streams), tau-tools (files, terminals), tau-memory
  (embeddings, files), tau-agents (a runtime of its own), tau-sqlite
  (the store, open before any plugin), tau-chatgpt (a provider, which
  streams), tau-jev (it is the Jev that Luau calls), tau-phones (a
  listener), tau-direnv (runs `direnv`), tau-notify (D-Bus) and tau-skills
  (the person's files). The run loop, a slot of the kernel's, stays
  Rust too.

What holds the eleven back is mostly one thing. In a run, only a tool's
`call` may call the model (`infer`) or Jev (`ctx.jev`): every other run
hook is given no reach (`Reach`, "a tool handler gets tools; other
hooks, none", `runtime.rs`), and the hooks the Rust plugins call a
model from have the shortest time limits (`before_request` 200 ms,
`before_tool` 2 s). The skill does not say so: its `ctx` table lists
`ctx.jev` with no word of where. The Rust plugins call the model inline
from those same hooks, bounded only by the run's cancel.

## Decision

### Every portable plugin moves, and its Rust crate goes

The thirteen move to Luau, as built-in plugins like tau-todo: a crate
that carries the Luau files (`luau/`) and the Rust tests that hold
them to account, loaded as always active. Once a port passes what the
Rust plugin's tests checked, the Rust crate, both halves, is deleted
in the same change. Nothing switches between the two, and no shim
keeps the Rust names.

The twelve stay Rust. Each keeps its reason in this ADR, and parity
stays where it is for the slots and services they fill.

### The gaps, and how each closes

**A. A run hook may call the model and Jev.** Every run hook (`start`,
`before_request`, `before_tool`, `after_tool_result`,
`rewrite_context`, `rewritten`, `before_stop`, `turn_end`, `run_end`,
and a tool's `call` as now) gets `infer` and `ctx.jev` when the
plugin's `uses` names them. `infer` asks the run's model unless
`uses.model` names another, and takes an `effort`. Both are charged to
the run, through the same metered Jev and model calls the Rust plugins
use, and the run's cancel cancels them.

A hook of a plugin whose `uses` names `infer` or `jev` runs to the
tool limit, 60 s, in place of its own. A plugin that reaches neither
keeps the short limits, so the hooks that run on every request stay
cheap for every plugin that does not ask for more. `on_event` keeps
200 ms and no reach: it runs for every event a run makes.

`ctx.tools` stays a tool's own: none of the thirteen calls tools from
another hook.

**B. `rewrite_context` sees the whole conversation, and can ask about
it cheaply.** Its view carries each message's full text, not its
first 2 000 characters (a VM has 256 MiB; a transcript that does not
fit is a transcript no plugin can rewrite). `infer` takes
`{ conversation = true }` to send the run's own transcript ahead of
its prompt, as the Rust compaction asks on the conversation it
already sent, so the provider's cached prefix is reused.

**E. The points left out reach Luau.** In the view a plugin draws:

- `rewrite`, `context`, `user_message`, `plan_steps` and `pane` are
  trees, like `banner` and `inspector` (they were "not yet");
- `plan`, `picker_auto` and `context_trigger` are typed answers the
  kernel reads. A Luau plugin returns them as JSON that the host
  decodes into the kernel's type; one that does not decode is the
  hook's error, as a bad tree is;
- `composer` was left out as tau-ask's place. tau-ask is now Luau, so
  it is a tree too, and only one plugin may fill it, as before;
- `setup` was left out because onboarding runs before the person's
  plugins load. Built-in Luau plugins load with tau, so they may fill
  it; the person's still may not.

**G. A plugin may read a folder it declares.** `uses.files = { "~/x" }`
grants `ctx.files.read(path)` (text or bytes, base64) and
`ctx.files.list(path)` under those folders only, read-only, the
person allowing it as they allow any other reach. tau-canvas reads
the images a run made; tau-skills stays Rust (it writes the person's
skills too), and this gap is for canvas.

**C. An ask may be a form.** `tau.ask{ questions }`: one to four
questions, each with choices and, if it says so, free text, waiting
for the person as `tau.allow` does, answered from any device. It is
tau-ask's whole interface, built on the same wait.

**D. A job may steer a chat, and knows local time.** A job (`jobs`)
reaches the repository's store and `tau.start_chat`, and may send a
message to a chat it names, as a scheduled wake does. `ctx.now` gains
the person's time zone (`now.local`, `now.offset`).

**F. A plugin sets its own continuation cap and sees a turn's cost.**
`before_stop`'s cap of three becomes the plugin's to set in its
declaration (`continues = 10`), at most 20. `turn_end` gets the
turn's cost. `start` may replace the run's first message, as tau-goal
does to state the goal.

**H. A plugin may keep what a sign-in gave it.** `ctx.secrets.put(name,
value)`, for a name its `uses.secrets` declares: a token from a
device flow is kept as a secret the person typed is, never in the
plugin's state or store, and still stripped from what the model reads.

Each gap changes `parity.rs` (a `NOT_YET` or an exclusion becomes a
hook) and the skill, which also says, for every hook, what it may
reach and how long it has.

### How a port is checked

A port is done when it does what the Rust plugin did, checked the
same way:

1. **The Rust plugin's own tests move to Luau**, as cases, properties
   and machines (ADR 0049), against fake runs. A test that checked a
   Rust type's layout, not behaviour, is dropped and says why in the
   change.
2. **The tests outside the plugin pass unchanged** in what they check:
   tau-ui's and the kernel's tests that load the plugin by name now
   load the Luau one. Only how they load it changes.
3. **A property holds the Luau plugin to the Rust one** while both
   exist, on drawn runs, before the Rust crate is deleted: the same
   events in give the same records, effort, rewrites and views out.
   The property goes with the Rust crate.

Each port is one change: the Luau plugin, its tests, the tests that
load it, and the Rust crate deleted.

### Order

1. **Gap A**, the skill's `ctx` table fixed to say where each reach
   works and the limits each hook has.
2. **tau-compaction-chain and tau-titles**: nothing new needed. They
   check the built-in mechanism with a slot each.
3. **tau-watcher** (A only), then **tau-reasoning** and **tau-goal**
   (A, E, F), then **tau-constitution** (A; its SQLite of rules
   becomes the plugin's store and pages).
4. **Gap B**, then **tau-compaction**, **tau-tree-compaction** and
   **tau-fast-compaction** (also E).
5. **tau-ask** (C, E), **tau-schedule** (D), **tau-canvas** (E, G) and
   **tau-github** (H, E).

## Testing

- **Gap A**: a Luau plugin's `before_request`, `before_tool`,
  `before_stop` and `rewrite_context` each call `infer` and `ctx.jev`
  against a scripted model and a fake Jev, charged to the run; one
  without `uses.infer` gets neither and keeps the short limit; a
  cancelled run cancels the call.
- **Gap B**: a transcript with a message of 100 000 characters reaches
  `rewrite_context` whole; `infer{ conversation = true }` sends the
  run's transcript first, in the provider request.
- **Gap E**: each point a Luau plugin fills draws where the Rust one
  did; a typed answer that does not decode is the hook's error.
- **Gap G**: reads outside the declared folders, through `..` or a
  link, fail; a new folder in `uses.files` waits for the person's
  Allow.
- **Gaps C, D, F, H**: each through the plugin that needs it, in
  tau-ui's host tests with two devices where an answer may come from
  either.
- **Each port**: the three checks above. `tests/parity.rs` keeps the
  table honest: a `NOT_YET` that a port needed is gone.

## As built

### Gap A

- Every run hook but `on_event` gets the model and Jev when the
  plugin's `uses` names them, through a `RunReach` that asks with
  `PluginCtx::ask`, so the run is charged and its cancel cancels the
  ask. Those hooks then have a tool's 60 s (`Hook::limit_for`).
- `infer` takes an `effort`, in a run hook, outside runs and in
  codemode's own `infer`, which share their instructions.

### tau-compaction-chain and tau-titles

- Both are Luau carriers now, like tau-todo: `luau/` and a `FILES`
  list, loaded as built-ins. Their Rust (both halves of tau-titles) is
  gone, and tau-plugins and tau-ui name them no more.
- **A Luau plugin's slot is tau-luau-plugins'.** The kernel's slots
  are filled by Rust plugins: tau-luau-plugins fills `tau.title` and
  `tau.compaction` for whichever active Luau plugin fills them, and is
  the slots' first pick. The Plugins screen shows tau-luau-plugins as
  the filler, and the compaction rewriters gain `tau-luau-plugins`
  first (its plugins' own rewrites), before the chain's three.
- **What a Luau plugin asks outside a run is booked to it**, by its
  own name, not to tau-luau-plugins: tau-titles' requests still show
  as tau-titles'.
- **`uses.model` may name a family.** `"luna"` is the plan's newest
  Luna, else the model the hook would ask without one, as tau-titles'
  Rust picked; an effort the chosen model does not take is dropped.
- **The reply's schema goes as `infer_answer`**, not `run_title`; the
  test outside checks the schema itself.
- **Fake runs give `ctx.tools`, not the global `tools`.** The skill
  said to call `tools.infer`, which a test cannot fake; it now says
  `ctx.tools.infer`, which is the same table in a real hook.
- **The parity check** ran Rust's `request` and `parse` beside the
  Luau `request` and `clean` on 400 drawn tasks of one- to four-byte
  characters, Unicode white space and quotes: all equal. A one-byte
  change to the Luau budget failed it. It went with the Rust crate.
- tau-titles' settings pane is the form tau draws from its schema; its
  catalog line no longer changes when titles are off.
- tau-ui does not link Luau, so its tests that need these slots
  install `tau::plugins()`, which does.

### Gap I, and tau-watcher

The inventory had tau-watcher needing gap A alone. It needed five
things more, closed here as gap I:

- **`ctx.transcript()`**, with `uses.transcript` (new reach: the plugin
  reads all the person said), in `before_request` and
  `rewrite_context`, the hooks that hold the transcript: every message
  whole, an answer's calls with their arguments. `before_request` gets
  `replies`, so a plugin counts steps without reading it.
- **Keyed notes**: a view's `notes = { [key] = piece }`, each placed
  where the run is when its key first comes and drawn again in place.
  The one `note` a view had moved down the transcript each time it
  changed.
- **`tau.interface(action, args)`**: an action may end in one of tau's
  button actions, done on its run once its state is kept, as "Chat
  about it" puts the note in the composer.
- **`infer{ model }`**: a model or family over `uses.model`'s, for a
  model the person picks in the plugin's settings. Codemode's own
  `infer` refuses it: a script asks the run's model.
- **`ctx.models`**: the plan's models, newest of each family, for the
  settings to choose from.

What the port found besides:

- **A Luau table keeps no order**, and the Rust watcher wrote a call's
  arguments in the model's order. `ctx.transcript()` gives each call's
  `order` too.
- **Per-character Luau is slow on long text.** Cutting a result at
  2 000 characters by turning it into a list of code points made the
  conversation check take 80 s; `utf8.offset` and `utf8.len`, in C,
  take 0.6 s for the same.
- **A property's case stalled for good under codemode's sequential
  lock.** With one seed, a case of `lists` drawn to 240 elements
  stopped mid-case: the VM idle, nothing to wake it, until the 40 s
  limit. The `hegel` tool was sequential; it is not now (a property's
  VM is one thread, and the driver has its own lock), and the stall is
  gone. Five thousand calls of a sequential tool in codemode alone,
  plain or nested in `pcall`, did not stall, so the cause in codemode
  is not found; the first driver's tokio mutex stalled the same way.
- **The demo cannot run Luau**, as tau-ui does not link it. tau-watcher
  carries the views the demo shows as fixtures, each a state and the
  view drawn from it, and its tests check each against what the plugin
  draws (`BLESS_FIXTURES=1` writes them anew). tau-ui's own test draws
  the views with the plugin, through `tau::plugins()`.
- **The parity check** ran Rust and Luau side by side on 1 000 drawn
  replies, 300 written notes, 300 cadences, 300 chat texts, the prompt,
  200 conversations of drawn messages with calls, and 200 drawn note
  histories folded by Rust's `State` and by `watch`: all equal. Rust's
  `chat_text` breaks `ultra` at a word's start by Unicode's
  `is_alphanumeric`, which Luau has no table for: `watch.chat_text`
  takes ASCII exactly and, beyond it, anything outside the blocks of
  white space, punctuation and symbols as a letter, and the check drew
  from text both read alike.
- The note keeps to `ui.stack` and `ui.row` (Luau has no card or
  spacer piece). The instructions go in `infer`'s task, after its own
  fixed ones, not as the request's instructions. The catalog's
  "on"/"off" note is gone with the Rust half. Records gave way to the
  run's state, stored with it as every Luau plugin's is.

### Gap J, and tau-reasoning

tau-reasoning needed what the inventory did not list: to know the
run's effort and set it, and the plan's points. Closed here as gap J:

- **`start` knows the effort.** Its input has the run's `model`, the
  `efforts` it takes, `effort` (left out on auto) and `prefix_tokens`,
  what the conversation and the instructions hold. `tau.start{ effort }`
  sets it, on auto only; the first plugin that sets it wins.
- **The plan's points**: a view's `plan = { name, value }` is a plan
  field, set by the plugin, and its `plan_steps` the Plan screen's
  step. A declaration's `picker_auto = { jev, no_jev }` is the model
  picker's line for "auto", with a TypeSafe key or without.
- **A Jev call says its cost**, as a second value, so a plugin keeps
  what each choice cost.
- `before_request`'s `effort` is left out at the model's default, as
  `start`'s is: a JSON null would reach Luau as the truthy `json.null`.

What the port found besides:

- **A run at a chosen effort left the next message nothing.** The Rust
  host gave the plugin only to runs on auto, so a long conversation
  started at an effort the person chose switched to Jev's pick on its
  next message, against the plugin's own docs; only its unit test,
  which gave the plugin to every run, kept it. A drawn property of the
  Luau tests found it. With Jev, the Luau plugin is in every run, and
  a run at a chosen effort leaves its context.
- **What `start` leaves shows after the message.** The Rust plugin
  placed its choice after the message the run started on
  (`PLACE_MESSAGE`); history put a Luau note before it. The host now
  places the state and view a Luau plugin's `start` leaves there, for
  every plugin.
- **A model that takes one effort** is not scored: codemode's `Score`
  needs two levels, where the Rust plugin asked with one.
- **One tree for wide and compact.** On a phone the Rust note showed
  the effort as a link to the chart, and the page drew each step
  narrower; the Luau note draws the same tree on both, its chart in a
  closed section. Its page is tau-luau-plugins' page for the plugin,
  the newest run's, not one page per run; a note's section is opened
  by its key (`tau-reasoning/note-n0`), which runs share.
- **The catalog's row**: it is one of tau-luau-plugins' rows now, so
  "needs a TypeSafe key" moved to the picker's line for "auto".
- **The parity check** ran a run's start end to end through both, on
  drawn efforts, scores and conversations; `runs_at` on drawn picks,
  last efforts and prefixes; drawn choices folded by Rust's `State` and
  by `effort.record_choice`; and drawn transcripts read by both for the
  step and the lease: all equal. The `replay` example and its test run
  on the Luau plugin.

### Gap F, and tau-goal

Gap F closed as written: a declaration's `continues` (1 to 20) is how
often that plugin's `before_stop` may keep a run going, counted for
each plugin, no longer for all together; `turn_end` gets the turn's
`cost`; and `tau.start{ input }` gives the model a message in place of
the run's, the first plugin's that gives one. tau-goal needed four
things more:

- **A view draws the person's messages it knows**: `messages = {
  [text] = pieces }`, for the message whose text is exactly `text`,
  and an empty stack draws nothing. A `/goal` draws as the goal it set,
  typed or as the model reads it, and a continuation a check's note
  says draws as nothing. The point (`tau.run.user_message`) left
  `NOT_YET`.
- **A command has an icon and a popover**: `popover = { jev, no_jev }`,
  whose inputs' drafts reach `run` as `ctx.drafts`; an input whose
  `submit` is `"tau.submit"` sends the composer and keeps what it says.
  A listed command may now have a popover (`Manifest::listed_popover`).
- **A command works outside a chat**: it runs on no run and no state,
  and what it asks the interface, `tau.send`, starts a chat. An
  action's `ctx.run` says whether its run is going on (`live`) and a
  Jev key is saved (`jev`), as the interface sees them.
- **A command's prompt titles its chat**: `/name args` reads as `args`,
  its leading `--flag value` pairs dropped and one quoted string
  unquoted, for any command word, as tau-ui reads titles without Luau.

What the port found besides:

- **The parity check** ran `goal.parse`, `set_message`, `set_input`,
  `tail` and `usd` against the Rust ones on 8 000 drawn inputs: text
  with Unicode white space (`\u{a0}`, `\u{2028}`, `\u{3000}`), quotes
  and escapes, `+5`, `0x10`, `1e400`, `-0`, `inf` and `nan` as limits.
  All equal. Luau has no Unicode white space table, so `goal.luau`
  keeps Rust's `char::is_whitespace` set; its budget reads Rust's float
  syntax, not `tonumber`'s, which takes hex.
- **Records gave way to the run's state.** The interface no longer
  stores `paused`, `extended` or `cleared`: the banner's buttons are the
  plugin's `act` action, and the host keeps the state it leaves. The
  popover's budget takes what a budget does, a finite amount of at
  least zero, where Rust's took any number.
- **The phone's bar** is the banner itself, buttons and all, as Luau
  draws one tree for wide and compact; the Rust bar had a chevron to
  the run's details instead.
- **The catalog's row** is one of tau-luau-plugins', and "needs a
  TypeSafe key" moved to the banner and the popover. The demo lists the
  built-in plugins from fixtures of their declarations, which their
  tests check.

## Alternatives considered

- **Port the plugins and leave the gaps**, calling Jev over HTTP with
  a secret. That skips the metered Jev, so a run's cost no longer
  counts the plugin's calls, and it leaves the hooks' limits too short
  for any model call. The plugins would work around the interface
  instead of testing it.
- **Raise every hook's limit.** Hooks that run on every request would
  then cost every plugin that has them, model or not. Tying the longer
  limit to `uses.infer` and `uses.jev` keeps the cost on the plugins
  that ask for it.
- **Keep the Rust crates behind a setting.** Two copies of each plugin
  to keep alike, and a setting no one should need. The parity property
  checks the port against the Rust one before it goes instead.
- **Stretch the twelve too**: an ntfy-style notify, or skills over
  `uses.files` with writes. Each would be a different plugin, not a
  port, and is left for later.

## Consequences

- Thirteen plugins' Rust goes (fifteen crates with the two host
  halves): 22 300 lines of source, with their inline tests, and some
  14 500 lines of tests besides, which come back as Luau tests.
- The Luau interface gains the reach the Rust plugins had in every
  run hook, the points that were missing, file reads, form asks, jobs
  that steer chats, and kept sign-ins. A person's plugin can do what
  tau's own does.
- A Luau plugin that calls a model from `before_request` makes every
  request it is consulted on wait for it, as the Rust ones did.
- Every port runs under the VM's limits: 256 MiB, and the hook's time.
  A plugin that outgrows them is a finding about the interface, and
  this ADR's "As built" records it.

## Settled questions

- **Who may call a model from `before_request`.** Any plugin whose
  `uses` names `infer` or `jev`, a person's as well as tau's: the Allow
  click for that reach is where the person agrees to the cost.
- **Many model calls at once.** A plugin wraps its `infer` calls in
  codemode's `parallel(...)`; nothing new is built for it. If the
  compaction ports find that too little, "As built" says so.
