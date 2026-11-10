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
stays where it is for the slots and services they fill. Two of them
moved later all the same, once Luau reached what they hold: tau-skills
(gap K, the person's files) and tau-memory (gap L, its notes' files,
once ADR 0052 took its embeddings away). Ten stay: tau-vcs, tau-mcp,
tau-tools, tau-agents, tau-sqlite, tau-chatgpt, tau-jev, tau-phones,
tau-direnv and tau-notify.

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

**H. A plugin may keep what a sign-in gave it.** Its Luau never sees the
token (chosen on 2026-11-10, in place of `ctx.secrets.put`). An
`http.request` may say `keep = { secret = "github", from =
"access_token" }`: the host saves that field of the JSON that comes back
as the secret, for a name `uses.secrets` declares, and takes it out of
the answer. A token the person types goes through `ui.secret(name)`, an
input whose text goes to the store as the settings pane's secrets do,
and `tau.forget_secret(name)` from an action empties one: signing out.
A device sign-in is polled by a job, which acts only while one waits.

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

- **A view draws the person's messages it knows**, in its
  `messages = { [text] = pieces }`: the message whose text is exactly
  `text` draws as `pieces`, and an empty stack draws nothing. A
  `/goal` draws as the goal it set, typed or as the model reads it,
  and a continuation a check's note says draws as nothing. The point (`tau.run.user_message`) left
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

### tau-constitution

Its SQLite of rules became the plugin's store in each repository, as
planned, and its page a page of the repository. Before the port, a view
drew on other tools' cards (`calls`), a page's action got the host's Jev,
and `start` could end the run (`tau.fail`). It needed four things more:

- **A view marks a card.** `calls[id].mark` is `"blocked"`, with the
  `reason` the model got, or `"flagged"`: the card is marked so, as a
  Rust plugin's verdict marked it, so the phone, the run's review list
  and codemode's rows show it. tau-luau-plugins marks it, once each.
- **A run's view links to the plugin's page of a repository**: a link to
  one of its own pages whose `params` name a `repo` opens that page of
  the repository, where it opened nothing before.
- **`tau.open_run`** opens the run `run`, for the review queue's Open in
  run.
- **A page knows a Jev key is saved**, as `ctx.run.jev`, from the
  interface, and is drawn again when that changes.

What the port found besides:

- **A run that fails at start did not say why.** The host told its
  caller only that the message could not be stored. It now says the
  run's error: "The chat did not start: … The constitution stored for
  repo is not valid: Rule R1: review (0.9) is above block (0.2)".
- **The test harness could not copy an array in a fake store**: an
  array's metatable reads as `false`, which `setmetatable` refuses, so
  any hook on a store holding one failed. It copies them as arrays now.
- **Everything the page shows is in the store**, as the person chose:
  the rules and settings, what waits for review (Looks fine takes it
  off), the last 30 things the rules handled, each run's counts, and the
  latest 24 calls and 3 answers to try a rule on. The old `constitution.db`,
  its migrations and the counts from every stored run's records are gone,
  and nothing is migrated.
- **The page's state is the store's too.** The rule being written, what
  trying it gave and a removal waiting to be confirmed were each
  window's UI state; they are the repository's now, on every device. A
  button cannot read an input's draft, so Enter in the rule's field keeps
  the rule, shown above it, and Save and Try it use what it kept; Enter
  in the field's field adds it, and its button is gone. Editing a rule
  shows its text above an empty field. "Asking Jev…" does not show: the
  page draws again once Try it is done. A save that fails is the
  action's error, which the interface shows, and the editor stays open.
- **Smaller differences.** The review queue names a run by its first
  message's first line, as a hook knows no title. A blocked call's meter
  shows the rule's marks when it was checked. Switches are buttons, and
  one tree draws wide and compact, where the Rust page hid some of it on
  a phone. Where a field is found under several keys of one object, its
  values come in the keys' sorted order, not the model's: a Luau table
  keeps none. A failed check's message is the error the Luau got from
  Jev. The catalog's row is one of tau-luau-plugins', and Jev's cost is
  booked to it.
- **The parity check** ran the Rust rules, checks and fold, copied from
  before the port, beside the Luau on drawn inputs: 400 texts of
  Unicode white space, cases and dots, trimmed and read as targets, and
  numbers written as Rust writes them; 400 rules checked; 300 stored
  constitutions read back; 300 runs of edits; 400 calls checked, on
  drawn rules and arguments, with Jev's scores at and near the marks or
  its failure, comparing what each rule reads, the request, the records
  and the reason; 300 final answers, with holds; 300 runs of drawn
  records folded into counts, notes and calls; 200 repositories' totals;
  300 drafts' steps; the starting line; and 300 holds' steps and dollar
  amounts. All equal. A doubled space in a reason, `>` for `>=` at the
  block mark, and one white space left out of the trim each failed it.
  It went with the Rust crates.

### Gap B

- **`rewrite_context` reads each message whole**, its `size` still its
  bytes. A message of 100 000 characters reaches the hook as it was
  sent.
- **A hook's input is a value, not a text.** It reached the VM as JSON
  that `json.decode` read, which takes a megabyte at most: a long run's
  transcript failed the hook. Codemode's `inputs` are values now, set
  as Luau tables after the sandbox, so the VM's memory is the bound.
- **`infer{ conversation = true }` asks in the run's conversation**, in
  `rewrite_context`, which holds it: the run's messages, then the task
  as written and the context unless it is null, on the run's connection
  with its own instructions, tools and effort. A number sends that many
  of the first messages, as an idle compaction summarizes only what it
  drops. It takes no schema, effort or model, which would change what
  the cache holds. The hook sends what it asks to `rewrite_context`,
  which asks the loop's conversation one at a time while the hook runs.
- **`infer` refuses an answer a plugin must not take as whole**: one
  that stopped with an error, was cut off at the model's limit, or
  called a tool, which an answer in the run's conversation can, as the
  run's tools go with it. The answer says why in `reason`.

### tau-compaction

pi's rules moved to `luau/lib/compaction.luau` as they were, prompts and
all, and the plugin is `start` and `rewrite_context`. tau-luau-plugins
runs it after fast pruning and the tree, as `BUILT_IN` lists them. It
needed five things more, each any plugin's:

- **A view's message says more.** `tokens`, the loop's estimate of it,
  which the cut adds up; an answer's `thinking`; each call's `args`,
  `order` and `json`, the arguments as the model wrote them, which the
  summary prints key by key in the model's order, nested objects and
  all; and `images`, when a message has any. A result's text joins its
  blocks with a new line, as tau-ai's does, and `window` is left out
  when the model's is not known, so it is `nil`, not `json.null`.
- **A request of the plugin's own.** `infer{ instructions }` sends the
  task as written under those instructions and answers its text, with
  no bound but the VM's: a summary request holds the whole history,
  far past the 16 KiB a task takes. It takes an `effort`, the one the
  run started at, which `start` keeps.
- **`rewrite_context` has ten minutes** when it reaches the model or
  Jev: one summary of a long history, or many, takes longer than a
  tool's minute. The Rust plugin was bounded by the run's cancel alone.
- **A rewrite that fails tells the run.** When no plugin rewrites and
  one's hook failed, tau-luau-plugins answers the loop with its error,
  so an overflow fails with the summary's error after the overflow's,
  as it did.
- **`Active::built_in(name)`** loads a plugin tau comes with, for an
  agent of its own: the memory and RLM evaluations run tau-compaction
  so.

What the port found besides:

- **The rewrite is tau-luau-plugins'.** Its `context` entry and its
  `ContextRewritten` event name tau-luau-plugins; the entry's details
  name tau-compaction and hold the record. The record is also the
  plugin's state, which a fork inherits, in place of
  `RunPlan::last_rewrite`. A transcript opens with the summary when its
  first message is the person's with the summary's text; the Rust plugin
  compared its time too.
- **The summary alone cannot be picked.** tau-luau-plugins is the only
  filler of `tau.compaction` left, and fills it for the chain; the
  person picked tau-compaction there for the summary alone. The tree is
  off unless switched on, and pruning runs only with a Jev key.
- **A failed summary past the threshold is a line in the plugin's log**,
  not a plugin error of the run: three of those in a run would turn the
  plugin off for the rest of it, where the Rust one waited and tried
  again. On an overflow or an idle chat it is the hook's error, as it
  was.
- **Smaller differences.** An answer whose text blocks are all empty
  writes no `[Assistant]:` part: Luau reads their text joined. An answer
  that says an error without stopping for one is refused, where the
  Rust plugin took its text. The window, reserve and kept tokens are
  settings, which the pane does not show; it shows the switch for idle
  chats. The catalog's row is tau-luau-plugins'. The demo's screen of
  the plugin is its entry under tau-luau-plugins.
- **Tests moved.** The tests of the loop's token estimate and overflow
  check, which were tau-agent's, are in `tau-agent/tests/context.rs`;
  what makes an answer no summary is a property of `infer` in
  `tau-luau-plugins-host/tests/infer.rs`. The tests in the loop run
  `LuauPlugins` on real time: on paused time, a hook's limit ran out
  while the store worked on a thread of its own. tau-ui's tests of idle
  chats moved to `tests/compaction.rs`, which installs `tau::plugins()`.
  The Rust tree and pruning lost their tests of tau-compaction taking
  over, which their ports bring back.
- **The parity check** ran the Rust rules, copied from before the port,
  beside the Luau on drawn transcripts (tau-testing's, with thinking,
  images, nested arguments and results past the 2,000-character cut):
  400 serialized, cut and planned, on drawn budgets and earlier
  summaries; 300 summary, split-turn and idle requests, with drawn
  earlier summaries and focus, and summary messages; 300 file lists,
  carried from drawn records; 300 answers taken or refused, through
  `infer`'s refusals and the Luau's words for them; and 300 thresholds
  and waits. All equal. A cut at 1,999 characters, a `>` for `>=` in the
  cut's snap, `,` for `, ` between arguments and a reversed sort of the
  files each failed it. It went with the Rust crate.

### tau-tree-compaction

OptChat's history, tree and view moved to `luau/lib/tree.luau`, the
compactor to `luau/lib/build.luau`, and the plugin is `start`,
`rewrite_context` and `zoom`. It runs between fast pruning and
tau-compaction, as `BUILT_IN` lists them. It needed three things more,
each any plugin's:

- **A tool offered while a setting is on.** `tools.zoom.when = "on"`:
  the run's model sees `zoom` only while the tree is on, as the Rust
  plugin added it only then, and a call with it off is refused.
- **A request's earlier turns.** `infer{ messages = { { role, text } } }`
  goes before the task, and a `task` or a message's `text` may be a list
  of texts, each its own block: a line over its limit goes back in the
  same conversation, its first message the context and the step, as two
  blocks.
- **A message's parts.** A message with images has its `parts` in the
  view, `{ text }` or `{ image = true }` in order, so an entry shows
  `[image]` where the image was.

What the port found besides:

- **The history is the plugin's state.** The Rust plugin stored each
  compaction's new entries and lines as a `folded` record and the view
  as the rewrite's details, and a fork put them together again. The
  Luau state holds the whole history, which a fork inherits, and is
  stored again each time it changes. A transcript opens with the view
  when its first message has the view message's text; the Rust plugin
  compared its time too. The `compacted` report is the state's
  `compacted`, which the status line reads.
- **Lines are asked `jobs` at a time, in batches**, through `parallel`:
  a batch waits for its slowest line, where the Rust stream started the
  next line as each finished. The first failure is the first in the
  batch's order, not the first to come back.
- **Smaller differences.** A line the model cut off at its limit, or one
  that called a tool, fails the build, where the Rust plugin took its
  text. A line sent back carries the earlier tries' text, not their
  thinking. White space is Unicode's, as Rust's `split_whitespace` and
  `trim` read it, from a table, as tau-goal does. A failed line past
  the threshold is a line in the plugin's log, not a plugin error of the
  run, as with tau-compaction. The settings pane is the switch; the
  window, reserve, kept tokens, view budget and jobs are settings it
  does not show. The catalog's row is tau-luau-plugins', so it no longer
  says "(off)".
- **The memory evaluation lost its `tree` arm**, which folded the first
  run with the Rust tree's library at its end and gave the second its
  view and `zoom`. Folding a whole run as it ends is not what the
  plugin does, and there is no Rust tree left to call.
- **The parity check** ran the Rust history and builder, copied from
  before the port, beside the Luau on drawn inputs: 400 transcripts
  (tau-testing's, with thinking, images and nested arguments) made into
  entries, and drawn texts of Unicode white space flattened, trimmed and
  capped; 300 histories of drawn entries and built lines, short and past
  1,200 characters, fitted, merged, rendered, zoomed into and asked for,
  context and step; 300 texts cut at drawn byte limits and sent back;
  and 200 histories grown against the same stand-in model, which
  answers some lines over the limit and some with ties, comparing every
  line, the view and every request. All equal. Ties going to the younger
  pair, one Unicode space left out of the table, recent entries cut at
  1,199 characters and the last of the shortest tries kept each failed
  it. It went with the Rust crate.

### tau-fast-compaction

The estimate, the history state and its partitions, the decisions, the
ledger and output pruning moved to `luau/lib/`, and the plugin is
`start`, `rewrite_context`, `after_tool_result` and its view. It runs
first of the compactions, as `BUILT_IN` lists them. It needed these
things more, each any plugin's:

- **An archive.** With `uses.archive`, `ctx.archive.path(kind)` names a
  new file in tau's archive of the repository, and
  `ctx.archive.write(path, text)` writes it, once, readable by its
  owner alone, in a directory only its owner can open. It writes only a
  path it gave. Naming it is new reach: "files of what it cuts".
  `LuauPlugins::with_archive` names the directory; the default is the
  temporary directory.
- **A rewrite that drops calls and replaces texts.** `tau.rewrite`
  takes `drop_calls`, calls that go with their results, an answer left
  with nothing going too, and `texts`, a person's message's or a
  result's new text. `ctx.measure(rewrite)` gives the bytes of JSON
  before and after it, to decide whether it saves enough.
- **A result shortened without a flag.** `tau.result(text, { flag =
false })` replaces a result without marking the call for review: what
  pruning keeps is verbatim.
- **`after_tool_result` sees more:** a result's `images`, whether a
  tool made the call (`nested`), and, with `uses.transcript`, what the
  model had read when it made it.
- **An answer's parts** in the view: its text and thinking in order, so
  the history keeps where the thinking sat.
- **A Jev state in order.** `state_json`, JSON text, stands for a
  request's `state` and keeps its keys in the order written: the Rust
  state's field order, which Jev reads and the estimate counts.
- **What a card shows of a rewrite.** A view's `calls[id].dropped` is
  `"result"` or `"call"`, and `calls[id].cut` says an output reached the
  model cut: the lines it kept, the archive and the tokens before and
  after. tau-ui draws them as it drew the Rust plugin's.

What the port found besides:

- **The rewrite is tau-luau-plugins'.** Its `context` entry, its
  `ContextRewritten` event and its Jev charges name tau-luau-plugins;
  the entry's details hold the ledger and the stats. The ledger, each
  pass and each pruned output are the plugin's state, which a fork
  inherits, in place of `RunPlan::last_rewrite` and the `ledger` and
  `output` reports. tau-luau-plugins' rewrites are not ones that keep
  the transcript, so after a pass the context meter's parts count from
  the pass, as after a summary.
- **What it draws moved.** The note at each pass is the view's
  `pass-N`, where the Rust plugin drew at the rewrite point, and the
  context meter has no mark at 60%: a Luau plugin has no view there
  yet. The ledger page has no turn column: the state does not know a
  call's turn. The catalog's row is tau-luau-plugins', so no row says
  it needs a key; its status line says so.
- **Jev is asked four requests at a time**, as Codemode limits a
  script, where the Rust plugin sent them all together.
- **An output past 256 KiB is left whole**: the hook gets only its
  first 256 KiB. tau's `exec_command` returns at most 50 KB, so only the
  evaluation's `--whole` outputs reach it: `all-noise` at three of the
  seeds 0 to 9. The evaluation drives the Luau plugin, reads its
  stats from the plugin's state, and measures with its own copy of the
  rules.
- **A failure is a line in the plugin's log**, not a plugin error of
  the run; on an overflow it is the hook's error, and the next plugin
  is offered the context. Archives go to the repository's archive
  directory, not the temporary one. The settings are flat (the output
  ones `output_*`); the pane does not show them.
- **Smaller differences.** The ledger keeps every message; tau's
  rebuild drops an answer whose calls all went and which has no text
  left, as the Rust ledger did.
- **Tests moved.** tau-ui's test of a large output pruned into the
  archive is in `tests/compaction.rs`; tau-ui-remote's tests of a pass
  on the cards draw the Luau view. The demo's run is the plugin's
  fixture, three steps of state and view, which `tests/luau.rs` draws
  and checks.
- **The parity check** ran the Rust rules, copied from before the port,
  beside the Luau on drawn inputs: 300 texts estimated (letters,
  digits, signs, ASCII and Unicode white space, characters of one and
  two UTF-16 units, control characters); 300 histories, goals and
  output histories, from tau-testing's transcripts with results long
  enough to cut; 300 histories split, at drawn budgets and at budgets
  just under where a prefix of records ends; 300 history-stage plans,
  and 200 output plans, renders and gates, each at drawn budgets and at
  the budget where the Rust plan changes and the one under it; and 300
  ledgers applied through tau's rebuild and measured, with heads drawn
  and heads that put a result at the cut's length. All equal. A vertical tab left out of the white space, an 8 for the 9
  tenths a comma costs in the history's split, the output's plan or the
  history stage's, a cut at `head + 119`, `line omitted` for `lines
omitted` and DEL left unescaped each failed it. It went with the Rust
  crate.

### Gap C, and tau-ask

Gap C closed as written, on the wait `tau.allow` has: a tool answers
`tau.ask{ questions, action }`, one to four questions, each with its
choices and, with `other = true`, the person's own answer, and its call
waits. The plugin's view draws the form, and one of its actions answers
`tau.answer{ call, answers }` or `tau.answer{ call, declined = true }`:
the first answers that fit, from whichever device, take it; the host
checks them against what was asked, then calls `actions[action]` in the
run with them, and the model reads what it answers. A run cancelled
first ends the call with an error. Of gap E, `composer` is a tree: the
first plugin by name whose view has one draws it, while the run goes
on. tau-ask needed these things more, each any plugin's:

- **`ctx.run.call`**, a tool call's id, so the state can say which
  call waits.
- **A tool the model's alone**: `model_only = true` is
  `Exposure::ModelOnly`, as a call that waits ends with a script.
- **Keys and focus in a view**: `ui.keys(id, keys, action, children, {
through, focus })`, an input's `focus`, and `ui.card`, `ui.spacer`
  and a table's `widths`, the pieces the panel is drawn with.
- **`tau.cancel`**, a button that stops the run.
- **An action that leaves the state as it was records nothing.** The
  interface recorded every action's state and view, and the run read
  the last state recorded. Send leaves the state as it was, so the
  answers the run took in it are not taken back by the state the
  interface sent, which came later.
- **A kind's runs leave out a built-in Luau plugin it names**, as they
  left out the Rust one: `LeftOut`, a run's service, names them, and
  tau-luau-plugins does not run them there. An agent's runs have no
  `ask`, as before.
- **The test harness** gives `run:tool(name, args, call)` its call's
  id.

What the port found besides:

- **Records gave way to the run's state.** `asked`, `answered` and
  `closed` are gone: the state's `calls` hold each call's questions,
  draft, focus and reply, and `start` and `run_end` close what waits.
  So a call whose run ended shows no Answer card; a run cut off, as when
  tau quit, still says its question waits until it goes on, though its
  panel shows only while the run does. A call whose future was dropped
  is closed by the run that goes on, not by a record of its own.
- **The draft is the run's**, not each window's: every key and click is
  an action on the run's state, so a phone sees and goes on with what
  the computer started. A button cannot read an input's draft, so the
  person's own answer and a note count once Enter is pressed in their
  field, where the Rust panel counted what they held; opening a note
  does not write it back into its field, which keeps what was typed.
- **No "Sending your answers…" and no refusal in the panel.** Send
  leaves the state be, so the panel goes when the call has the answers;
  answers the host refuses, as when another device answered first, are
  the action's error. What the panel sends always fits what was asked,
  as a property checks; only the host checks it in a run.
- **Two devices cannot undo an answer.** An action's result is recorded with
  the state it was carried out on (`Record::Acted`), and every reader of the
  run's records takes it only if the plugin's state is still that one. A key
  from a device that had not seen the answers is dropped, on the computer, on
  phones and in the run alike. This holds for every Luau plugin's actions. The
  cost: an action whose state moved on meanwhile loses its change, not only
  its stale copy. Two keys pressed before the first's result returns keep the
  first and lose the second, where they kept the second and lost the first; and
  a slow action on a live run, as one calling Jev, loses its change if a hook
  changes that plugin's state while it runs. What such an action did outside
  its state stands.
- **The card** is drawn from the state, on the call's card body: what
  was asked and each answer with its note. Its title is the tool's, not
  `ask · Waiting · Surfaces`, with no "2 answered" label or edge, and a
  call refused for its questions has no body. The status says how many
  questions wait while one does, never "not answered".
- **One tree for wide and compact**: the tabs' mode and the keys' hints
  show on a phone too, where the Rust panel left them out.
- **Smaller differences.** A call's shape that does not read is the
  Luau's own message (`questions[0]: \`header\` is not a string`), not
serde's. A header that is too long is quoted as Rust's `{:?}`quotes
it, from a table of the characters it escapes, read off Rust itself.
The result's`structured`is`{ call, answers }`or`{ call, declined
  }`. The catalog's row is one of tau-luau-plugins'.
- **The parity check** ran the Rust plugin beside the Luau on drawn
  inputs: 3 000 calls' arguments, with Unicode white space, quotes,
  escapes, combining and unprinted characters, "Other" in its cases,
  labels and questions repeated with white space around them, missing
  fields and fields of the wrong type, read and checked; 3 000 replies
  to questions that check out, too many or too few, stray, repeated and
  unordered picks, blank and padded answers of the person's own and
  notes, checked and read back, and their cards; and 400 runs of up to
  30 keys and clicks on the panel, in a field and not, with rows and
  tabs past the end, negative and fractional, comparing the draft, the
  focus, the answers sent and the panel's tree after each. All equal.
  Each of these failed it: a white space left out of the trim, `>=`
  for `>` at the header's limit, "Other" matched with its case, a
  note's lines indented by seven, Down past the last row, a checklist's
  picks unsorted, Enter in the field going on from a checklist, picks
  out of order taken, the person's own answer read untrimmed, U+2028
  left unescaped, the card's decline worded otherwise, Enter in the
  field keeping the keys there, and keys in a field taken as the
  panel's. A ninth row without its key did not, as no question has
  more than five rows. It went with the Rust crate.

### Gap D, and tau-schedule

Gap D closed on a wake a job asks for and sees the end of, rather than
an answer the host carries out once the job ends, so the plugin keeps
the at-least-once logic it had:

- **`ctx.chats.wake(run, text)`**, in a job of a plugin with
  `uses.chats`: a turn of the chat `run` on tau's message `text`,
  answering `"started"`, `"busy"` or `"gone"` as
  `Conversations::wake` does, and failing when the turn did not start.
  `uses.chats` is new reach, allowed as any other.
- **A job reaches every repository's store**: `ctx.repos()` and
  `ctx.stores(repo)`, through the host's `repo_store_*` calls, a
  session opened as the job first names each. What the job wrote is
  kept just before each wake: the turn it starts runs `start` hooks
  that open the same store, and a job still holding it waited on them
  for good, as the host test found with the commit taken out.
- **Ticks**: a job may run `every = 1`. One more often than every
  minute is not shown among the person's jobs, and when it last ran is
  kept in memory, its file written only when its state changes. The
  host looks for due jobs each second, not each 30.
- **Local time**: `ctx.now` gains `local` and `offset`, and every hook
  has `ctx.time.local(at)`, `ctx.time.parse(text)` and
  `ctx.time.duration(text)`, the offset at the time asked about. They
  read `tau-clock`, a crate of tau-schedule's clock, which tau-agents'
  schedules go by too.
- **Not built**: a job starting a chat (`tau.start_chat`) stays an
  action's; tau-schedule did not need it.
- **Of gap E**: `ui.due(id, at)`, a countdown, and a button's `icon`.
- **The test harness** gives a job a state of its own (`run.job_state`;
  it shared the run's), a fixed `offset` for local time, other
  repositories' `stores`, and `wake`'s answer, keeping each wake in
  `run.woken`.

What the port found besides:

- **The store replaced `wakes.json`**: each wake-up is
  `wake/<chat>/<name>` in its chat's repository's store, and the old
  file is left, with no migration. The chat's state holds what its
  inspector draws, which each turn's `start` reads again from the store,
  and the cards of the messages it woke on, by their text.
- **An agent's runs leave it out**, as they leave out tau-ask (`LeftOut`).
- **Smaller differences.** The catalog's row is tau-luau-plugins', so it
  no longer says how many wait. A field the schema does not allow is
  refused with Luau's own words, not serde's. A tick's store keeps a
  connection to each repository's store it opened while tau runs.
- **The parity check** ran the Rust plugin beside the Luau on 1 200
  drawn runs of up to 25 steps: two chats scheduling with names,
  notes, durations and local times written well and badly, `in` and
  `on` together or neither, repeats, cancels and listings, time
  passing by the retry's and the soonest's edges, chats busy, gone and
  failing to start a turn, and ticks; comparing every answer, every
  message a chat woke on and what waited after each step. It found the
  Luau refusing a repeat before an empty note, where the Rust checked
  the note first. A retry a second late it did not find; the case at
  thirty seconds does. It went with the Rust crate.
- **The model** moved to Luau as a machine: tau closing after the k-th
  wake of a tick started its turn leaves the store as it was when that
  wake was asked for, and each wake-up woke its chat once, and once more
  at most for each such close. Deleting a chat's due wake-ups before
  waking it fails it.

### Gap G, and tau-canvas

Gap G closed with the run's workspace among the folders, as tau-canvas
shows what a run made there:

- **`uses.files`** names folders a plugin's tools read, read-only:
  `"workspace"` the run's own, `"~"` or `"~/x"` under home, or an
  absolute path. Each is new reach, allowed as any other. In a tool's
  `call`, `ctx.files.read(path, { bytes, limit })`, `list(path)` and
  `image(path)`; a path relative to the workspace, `~/` from home or
  absolute is read where it really is, so `..` and links that leave the
  folders are refused, as the host tests check with the check taken out.
- **`ctx.files.image`** says a file's size and, by its first bytes,
  which image it is and its size in pixels: the host's, with the image
  crate, as the Rust plugin had it.
- **Blobs**: `ctx.blobs.put_file(path)` keeps a file of at most 8 MiB as
  a blob of the plugin's in the run's repository (`tau-artifacts`, under
  tau-luau-plugins' directory), its bytes never through the VM.
  `ui.image(blob, mime, repo, height)` draws one; tau-luau-plugins' host
  half answers the interface's fetch (`Act::Blob`), once a window.
- **Of gap E**: `pane = { title, fresh, body }` fills `PANE`; `calls`
  gains `head` and `label`, so a plugin draws its own tools' cards from
  its state; `tau.pane` opens the pane, also as an action's
  `tau.interface` answer.
- **`tau_test`**: the test harness left `tau.luau` for a module of its
  own, which only tests load. With gap G `tau.luau` passed codemode's
  64 KiB a module may have.

What the port found besides:

- **The canvas is the run's state**, not records: the items, their
  revisions, `fresh` and `last` as the Rust fold had them. What the
  pane shows when the person picked is in the state too, so every
  device shows the same, where the Rust pick was each window's.
- **Cards** draw from the state, so a stored run shows them as the live
  one did. A call that failed, or is still going, and `canvas_read`'s
  have the loop's card: the Rust plugin drew `canvas · <title>` and
  `canvas · read item N` for them.
- **Smaller differences.** The results carry no `details` or structured
  value, only their text. A field a tool does not take is refused in
  Luau's words. A path past the workspace and home is refused, where
  the Rust plugin read any file. A Luau plugin's views go to every
  device with its state, so a document's text does too, as the Rust
  records did.
- **The parity check** ran the Rust plugin beside the Luau in real runs
  on one workspace: 600 runs of up to 8 calls, showing images, notes
  and missing files by paths padded, marked with `@` or `file://`,
  absolute and through `..`, replacing items of the wrong kind or none,
  writing documents empty, Markdown and past 256 KiB, editing with
  empty, missing, repeated and overlapping text, and reading revisions
  past either end; comparing every result and whether it failed. All
  equal; a result's wording changed fails it. It went with the Rust
  crate.

### Gap K, and tau-skills

Added on 2026-11-10: tau-skills, kept in Rust above for writing the
person's skills, writes only its own: it copied the 51 skills it ships
out of the binary into `<data>/skills` as the host started. As a
package, they are files of the package, read where it is installed, and
it writes nothing. What it lacked is reach outside a run:

- **`app(ctx)`**: what the plugin keeps of the whole app, as
  `repo_data` does of a repository, with nothing as input and
  `ctx.files` as its tools have it. The host runs it as it draws the
  catalog, at most once a second; its `nav` field gives the sidebar
  entry's `detail` and `badge`, as `repo_data`'s does under a
  repository.
- **Commands from it**: `commands` may be `{ list, run }`: `list(app)`
  gives the commands by name, each `{ description, args, icon }`, and
  `run(name, args, ctx)` carries one out, as a declared command's
  action does.
- **A page of the app**: `page(app, ctx)` draws the page `nav` opens,
  in place of the plugin's card there; its buttons are its actions.
- **`ctx.files` in `start`**, so a run's instructions can list what a
  folder holds as the run starts.
- **`"package"`**, **`"data"`** and **`"skills"`** in `uses.files`: the
  package's own folder, wherever it is installed; tau's data folder, where
  other hosts put the skills they ship; and the person's skills folder as
  the host has it (`~/.agents/skills`, or a test's), each with folders in
  it as `package/x`; `ctx.files.roots()` says where each is.
- **`ctx.files.frontmatter(path)`**: a Markdown file's YAML frontmatter
  as a table, and the text after it, read by the host (saphyr) as
  `ctx.files.image` reads an image.
- **`tau.interface("reveal", { path })`** shows a folder `uses.files`
  names in the file manager.

As built:

- tau-skills is `plugins/tau-skills`: `src/scan.luau` reads the folders
  as `scan.rs` did, word for word, and `src/plugin.luau` lists them in
  `start`, loads one with `skill`, draws the Skills page, the sidebar's
  count and a card per loaded skill, and lists `/name` commands. The 51
  skills it ships moved to `plugins/tau-skills/skills`; nothing copies
  them now. A parity property held the Luau plugin to the Rust one over
  2,000 drawn pairs of folders (frontmatter missing, unclosed, not YAML,
  names and descriptions as numbers, flags, null or lists, block
  scalars, wide spaces, a BOM, CRLF, hidden and clashing folders): the
  skills found, the folders that are not, the instructions' list and
  each skill loaded were the same, and changing one reason failed it.
  Then the Rust crate went.
- `SkillsDir` moved to tau-ui-plugin, with the service's name and
  `SHIPPED_SKILLS`, the data folder's `skills` that tau-plugins and
  tau-agents write theirs to.
- The `reveal` action is `tau.reveal` on a page of the app, its path
  held to the folders the host resolved.
- Differences: the tool is declared even when no skill is found (tau
  ships 51, so a run always has some); a failed `skill` call has the
  default card; the Skills entry is in the sidebar with no skills too.

### Gaps H and E, and tau-github

Added on 2026-11-10, the last port.

- **The token past the plugin (H).** `http.request{ keep = { secret,
from } }` saves a field of the JSON that comes back as a secret the
  plugin declares, and takes it out of the answer and anything else it
  came back in; the HTTP client never sees `keep`. `ui.secret(name)` is
  a secret input whose Enter is `Act::SetSecret`, as the settings pane's.
  `ctx.secrets.has(name)` and `ctx.secrets.forget(name)` say whether one
  is set and empty one, never its value. `ctx.http` is the `http`
  global through the hook's tools, so a test fakes it (`spec.http`).
- **The setup step and the Models section (E).** `setup(app)` and
  `models(app)`, of what `app` keeps; a person's plugin draws no setup
  step, as its plugins load after setup. Their buttons may name
  `tau.copy`, `tau.open_url` and `tau.setup_done`; an app action's
  `tau.interface` reaches the interface as a run's does.
- **A store of the app.** With no repository, `ctx.store` is the
  plugin's store of the app (`.app`, which `ctx.repos` never lists), in
  `app`, an app action, a job and a forge's hooks.
- **The forge's URLs.** `LuauForge`'s `repository_of` reads Git URLs as
  tau-github's `url.rs` did, for any forge's host: `www.` and `ssh.`
  too, any case, an owner and a name the forge allows.
- **tau-github** is `plugins/tau-github`: its API in `src/github.luau`
  word for word, the sign-in drawn in `src/sign_in.luau`, a job polling
  a device code at GitHub's interval, checking a typed token once and
  forgetting an expired one. A parity property held its forge to the
  Rust API over 1,500 drawn fake GitHubs (installations or none, pages
  of a hundred, failed pages, duplicates, push permissions and every
  status, check runs, a pull request and its reviewers): the same
  answers and the same errors, and changing one message failed it. Then
  tau-github and tau-github-host went.
- **Differences.** A sign-in kept in `github.json` is not read: sign in
  again (there are no users to move). A typed token is checked by the
  job within a second rather than as it is typed, and the setup step
  waits for Continue rather than ending itself once signed in.

### Gap L, and tau-memory

Decided on 2026-11-10. tau-memory was one of the twelve to stay Rust,
for its embeddings and its files. The embeddings went with ColBERT (ADR
0052), so it moves too; what it needed besides is gap L.

- **The write gap.** A folder of `uses.files` named `{ name, write =
true }` is written as well as read (`Uses::writes`):
  `ctx.files.write(path, text)` writes a file whole, to a file beside it
  then in its place, making the folders it is in, at most 8 MiB;
  `remove`, `rename` and `mkdir` do what they say. A path is written
  where it really is: the nearest part of it that is there is followed
  through its links, the rest may not climb out with `..`, and it must
  land in a written folder and not be that folder. `repository` names
  tau's own directory for the run's repository, beside its rules and
  archive. A plugin's own folders, `data/<name>` and
  `repository/<name>`, are written with no Allow; any other folder it
  writes waits for an Allow of its own, apart from reading it
  (`Uses::grown_from` names it, "writing files in `~/x`"). `ctx.files`
  reaches `rewritten`, `turn_end` and `run_end`, jobs and actions too;
  outside a run, `ctx.files.repository(name)` is tau's directory for
  each repository the host has, by name.
- **`infer` takes tools.** `ctx.tools.infer{ task, context, tools,
wait_for?, rounds?, instructions?, effort?, model? }` offers the
  plugin's own tools, by name and no other: `tau.luau` asks one round
  at a time (`infer_turn`, in a run's hooks and outside them alike),
  calls each tool the model asks for with the hook's own `ctx`, in the
  same VM, and sends back what each answered, as tau-memory's
  `write_notes_from` did: up to four rounds, ending at a round that
  calls none of `wait_for` (its searches), an unknown tool or one that
  raises answering the model with the error. A tool called so changes
  `ctx.state` as the hook does, so nothing waits on the run's state
  twice. The message is the task as written, then the context as JSON,
  with the plugin's instructions; the answer is `{ ok, value, calls }`.
- **`turn_end` hears the commit.** In a run with a workspace, its input
  has `commit = { change, paths }`: the commit the turn left, as the
  run's `TurnHooks` tell it, which the workspace's plugin hears before
  the Luau plugins hear the turn end. A turn with no commit has none.
- **`ctx.files.remote(path)`**: the Git remote of the repository a
  folder it reads is in, read as tau-memory's importers read it, from
  the Git config of a checkout, a worktree (its `gitdir` file and
  `commondir`) or a jj repository's Git store, `origin` first, else the
  first listed: `{ url, normalized }`, normalised as tau-memory's
  `normalize_remote` does, or nil. The importers parse the config
  themselves rather than through gix, so the helper does the same, to
  read as they did.
- **Tests.** `t.run` fakes the writes into `spec.files` (`run.files`,
  `run.folders`), `spec.remotes`, `spec.repositories` and `infer`'s
  rounds (`spec.infer`, `run.infers`); `t.loop` takes `data`,
  `repository` and `commits`, and gives back `data` and `repository`
  as the loop left them.
- **The rest moves to Luau** as the user decided: imports (memory
  files, JSONL chats, proposals, the checkpoint, the import job), BM25,
  and the safety checks, in Luau patterns, held to the Rust by a parity
  property. tau-memory-e2e runs the Luau plugin through
  `Active::built_in`.

As built, the core (`plugins/tau-memory`, with the imports beside it):

- **Where notes are.** The repository's scope in `repository/tau-memory`
  and the person's in `data/tau-memory`, the plugin's own folders, so
  no Allow; read and written through `ctx.files`. There are no notes to
  move: tau has no users yet.
- **Notes' files** are written byte for byte as the toml crate (1.1,
  `toml_writer`) wrote them (`toml.luau`, `note.luau`), and read by a
  parser of TOML 1.1 whole: comments, every string kind, inline tables
  and dotted keys, as a person editing one might write; a key it does
  not know is left alone, as serde did, and a file that does not read
  is reported, not dropped.
- **Words** are Rust's: `char::is_alphanumeric` runs and `_`, lowered as
  `str::to_lowercase` lowers them, final sigma and the dotted capital I
  included, and white space as `trim` reads it, from tables
  (`unicode.luau`) that tau-built-in-tests writes from Rust's own and the
  regex crate's and holds the file to (`BLESS=1` writes it again).
- **BM25** works in single precision as the Rust did, each step rounded
  with `string.pack("f")`. The scores agree to two units in the last
  place: Rust's `ln` is the platform's `logf`, which glibc does not
  always round to the nearest (one input in 1,000 or so), where Luau's
  is.
- **Secrets and instructions** are matched by a small backtracking
  matcher over the Rust patterns written as trees, reading them as the
  regex crate does: leftmost first, `\b` and `\s` by Unicode, and `(?i)`
  folding the Kelvin sign into `k` and the long s into `s`.
- **What the host gained for it**: `ctx.transcript()` in `rewritten` is
  what the rewrite replaced, and in `run_end` the run's conversation;
  `after_tool_result` reaches `ctx.files`; `ctx.now.ms`, as notes keep
  milliseconds. While the Rust plugin, of the same name and tools, was
  there, `built_in::NOT_YET` kept the Luau one out of the app; it went
  with the Rust crates.
- **What interfaces draw** is the run's state, not records: `recalled`
  (the notes the task found), `log` (each `recalled`, `saved`, `marked`
  and `error`, in order) and `given` (what the conversation has of
  memory, which a fork or the chat's next run inherits; a rewrite that
  drops the conversation empties it).
- **Differences kept.** A conversation shown to a memory request lists
  an answer's text before its calls, as Luau's messages keep no order of
  blocks, and a call's nested arguments in JSON's own order. Stale
  marks of a turn's commit reach the notes last written before that turn
  began, every turn moving the mark, as the reference says; the Rust
  moved it only on the turns its hook heard. Why a file does not read
  is said in Luau's words. A whole number past 2^53 in a note edited by
  hand reads rounded.
- **Parity**, in tau-built-in-tests (`memory::parity`), each over 1,000
  drawn cases: a note's file both ways, byte for byte, and what
  `validate` refuses; words, `trim` and `lines`; BM25's scores and
  order; recall; redaction and refusal; sequences of writes, links,
  supersessions, stale marks and searches, answer by answer and file by
  file, history included; and the four tools call by call in both
  scopes. The prompts, the tools' descriptions and their arguments are
  the Rust's.

As built, the imports (`import_read.luau`, the readers;
`import.luau`, the proposals, the checkpoint and the job):

- **Modules are flat.** A package's modules are `src/<name>.luau`,
  loaded by `require("name")`; none sit in folders of `src/`.
- **What the host gained for it**: `ctx.files.read` takes an `offset`
  and `lossy`, and a listing's entries carry `modified` in milliseconds;
  `ctx.tools.infer{ tools }` takes tools given as tables
  (`{ name, description, parameters, call }`) beside names of the
  plugin's own, as the import's `memory_search` and `memory_propose`
  are not the agent's; and `run:call(f)` runs `f(ctx)` in tests.
- **Claude Code's front matter** is read by the Rust's own small parser,
  ported, not by `ctx.files.frontmatter`, so a note reads as it did.
- **Differences kept.** The import is a job that reads one chunk of
  32 kB a tick, where the Rust read three files at once; a chat past
  8 MiB is read in parts across imports. A model ask that fails stops
  the import, where the Rust waited for a restart. A remote is read only
  of a folder the plugin may read, so places match listed repositories
  through `ctx.files.repositories()`; and decoding a Claude Code
  project's folder from its name walks from `/`, which the app's
  plugin does not read, so a project no chat records the `cwd` of gets
  no place.
- **Parity**, in tau-built-in-tests (`memory_import::parity`), over
  1,000 drawn homes of Claude Code and Codex files: what each reader
  finds and reads, the proposals and places of an import of the saved
  notes, the tallies, and what a second import reads after drawn
  changes.
- **The runtime's lost wake-up.** The parity found a Luau script making
  thousands of host calls sometimes never woke again, stopped by its
  60 s limit with every thread idle; it is fixed in the runtime in
  the same change.

As built, the UI, and what the interface gained for it (gap L's part of
gap E):

- **Pages of the app.** A plugin's `pages` are drawn outside any
  repository too: a link of a page of the app, or of a run's view, that
  names no `repo` opens that page of the app (`app_page_link`), drawn on
  the host as a repository's page is, `ctx.files` reaching its folders,
  and fetched again whenever what `app` keeps changes. A repository's
  page and `repo_data` reach `ctx.files` too, of that repository.
  `nav.page` makes the sidebar entry open one of them in place of the
  plugin's card. A map's nodes (`ui.graph`) open the plugin's pages as
  links do.
- **What a page reads as typed.** A plugin's `drafts` names inputs whose
  drafts its pages read, as `params.drafts`: a page is drawn again as
  one changes, the search over memory's notes and over what imports
  propose.
- **`tau.open`**: a button, or an action's `tau.interface("open", { page,
params, repo })`, opens one of the plugin's own pages, of `repo` or of
  the app. A repository page's actions answer what they ask of the
  interface as an app page's do.
- **`ui.switch(label, detail, on, action, args)`**: a switch that
  carries out an action, as memory's review page ticks a proposal.
- **The repository's menu.** `repo_menu = { label, page, icon }` is an
  entry in each repository's menu opening that page of it, and what
  `app` keeps as `menu`, a tree of the app's, is drawn there after it
  (memory's way into an import).
- **Keys.** `keys = { ["ctrl-2"] = "notes" }` binds a key tau leaves to
  Luau plugins (`KEYS`) to one of its pages of the repository the person
  is in, the first active plugin by name taking it, an earlier plugin's
  key winning: `KEYS` is ctrl-2, which tau-memory binds to its notes.
- **Setup.** Each of tau's own plugins draws its own step (it was the
  first one only), and with repositories picked the step's body is
  drawn again of them, as `ctx.run.picked` (`{ name, id, folder,
remote }` each).
- **The repositories tau lists**: `ctx.files.repositories()`, `{ id,
name, folder, remote, normalized }` each, for a plugin whose
  `uses.files` names `repository`: memory's imports match proposals'
  places by folder or remote as the Rust half did with the host's list,
  without reading project folders. Outside any repository, a
  `repository/...` folder of `uses.files` is that folder of each
  repository tau has, which keeping a proposal into one writes.
- **Tests.** `run:app_page_of(name, params)` draws a page of the app,
  `run:setup(app, picked)` the step of picked repositories, and
  `spec.repos` is what `ctx.files.repositories()` lists.
- **Memory's UI** (`ui_notes.luau`, `ui_import.luau`,
  `ui_settings.luau`): the notes pages, a repository's (its sidebar
  entry, menu entry and ctrl-2) and the person's (a page of the app,
  its sidebar entry counting their notes); a note open, with its map
  and links; the transcript's marks and the plugin list's line from the
  run's state; the banner, the menu's way into an import and the review
  page; the Memories step; and the settings pane. What a page drew in
  the Rust UI's view state (the setup step's choices, what keeping last
  did) is a file in `data/tau-memory`.
- **Differences kept.** The settings pane binds its settings
  (`ui.toggle`, `ui.choice`), so a switch's line under it is text below
  it and each choice is the interface's own control, not a button per
  family and effort. A page of a Luau plugin has no title of its own in
  the frame. A stale mark is not drawn in the transcript, as in the Rust
  UI.
- **Parity**, in tau-built-in-tests (`memory::ui`): a scope's notebook
  from the same files (500 cases); and the notes pages with every note
  open, the transcript's marks, the banner and menu entry, the review
  page in each tab and the setup step, piece by piece against the Rust
  UI's trees over drawn notebooks and imports (300 to 500 cases each).
  What a click asks, and where a link goes, are each side's own, so the
  comparison leaves them out and the Luau tests hold them.

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

### tau-memory's Rust crates go

With the core, the imports and the UI held to them, tau-memory and
tau-memory-host were deleted (2026-11-10), and the parity properties
with them (`memory::parity`, `memory::ui`, `memory_import::parity`);
what Rust can say and Luau cannot stays in tau-built-in-tests: the
Unicode tables, and a remote read from real Git configs.

- **The app** runs the Luau plugin among tau's own; ctrl-2 is in
  `KEYS`. Tests that turned memory's pass after each run off set it as
  a Luau plugin's settings (`Host::with_luau_settings`).
- **The demo** draws memory from its fixture `demo`
  (`plugins/tau-memory/tests/demo.luau`): the notes of tau-agent,
  docbert and homelab.nix and their pages, a note open, the review page
  and banner of synthetic imports, the setup step, the settings pane,
  and what the demo run recalled and saved, each made by the plugin's
  own functions and held to it by its tests.
- **tau-memory-e2e** runs the Luau plugin as tau does
  (`Active::built_in`), on the run's own model, with its notes in a
  directory of the trial's. The commit between a trial's runs reaches
  it as a turn's commit does (`turn_end`), whether the second run
  started with memory is read from its first message, and the
  `transcripts` arm keeps a BM25 and the conversation's text of its own.
- **The interface's window tests of imports** (tau-ui-remote's
  `memory_import`) went: the Luau tests hold what they checked, the
  pages' trees being the plugin's.
