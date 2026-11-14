# 0054: State machines in Luau plugins

- Status: proposed.
- Date: 2026-11-14

## Context

tau's Rust core keeps its lifecycles as explicit machines: fourteen on
statig, each tested by a hegel state-machine model that sends inputs in
orders the real caller never would. The sweeps that led there found the
same kind of bug every time: a state that is done in one place and not in
another, or an edge nobody wrote.

A sweep of the Luau plugins on 2026-11-14 found ten more of that kind:

- tau-github keeps a failed sign-in as `sign_in = { state = "failed" }`,
  and its job checks a typed token only when there is no `sign_in`: a
  corrected token is never checked. There is no edge from `failed`.
- tau-memory's import tick reads its checkpoint and proposals, waits on
  the model, then writes its old copies back over what Stop, Keep and
  Drop did meanwhile.
- tau-constitution keeps a call's last verdict, so a block followed by a
  flag shows the refused call as flagged.
- tau-todo's Undo restores a whole issue, claim included, after the
  claiming run ended; its Beads import sets a claim whatever the status.
- tau-schedule lists wake-ups that just fired from a copy of the store.
- tau-reasoning, tau-memory and tau-goal keep fields past the state they
  belong to (a status from an earlier message, `given`, cost a skipped
  check never counted), and tau-memory's import retries a failing chunk
  every second.

A plugin's state is data (ADR 0045): `ctx.state` and the plugin's store
hold JSON, persisted, folded from records and replicated to phones. A
hook that fails keeps none of what it wrote. So the half-written state of
a crash is not the problem. The problems are edges no one wrote, fields
that outlive their state, and copies that go stale.

Some plugins already keep a clean tag: tau-github's `sign_in.state`,
tau-goal's `status`, tau-todo's `issues.move` checked against its moves,
tau-constitution's records folded by kind. Each wrote its own guards,
and the bugs sit where code went around them.

## Decision

### A machine is data and a transition table

`tau.machine(spec)` makes a machine. It holds no state: a state is a
plain table the plugin keeps where it keeps state now, so it persists and
replicates as before.

```lua
local sign_in = tau.machine({
	-- Every state, and the fields it carries.
	states = {
		signed_out = {},
		checking = {},
		waiting = { "code", "deadline", "interval" },
		failed = { "error" },
		signed_in = { "user" },
	},
	initial = { state = "signed_out" },
	-- By event, then by the state it leaves.
	on = {
		token_saved = {
			signed_out = function() return { state = "checking" }, { "check_token" } end,
			failed = function() return { state = "checking" }, { "check_token" } end,
		},
		checked = {
			checking = function(_, e)
				if e.ok then
					return { state = "signed_in", user = e.user }
				end
				return { state = "failed", error = e.error }
			end,
		},
	},
})
```

- **A state is a tagged table**: `state` names it, and it carries the
  fields its entry in `states` lists, no others. Fields are JSON values.
- **`machine:step(state, event)`** returns the next state and a list of
  effects. `event` is a table whose `event` field names it; the rest is
  its data. `state` nil is `initial`.
- **A transition is a pure function** of the state and the event. It
  returns the next state, a new table, and optionally effects. It never
  writes `ctx`, the store or the files.
- **An event a state has no arm for changes nothing**: `step` returns the
  state as it was and no effects. This is the default, as an input out of
  order is common (a click on a stale button, a late tick). A plugin that
  wants an unknown pair to be an error says `strict = true`.
- **`step` checks the shape** of what a transition returns: a known tag,
  and exactly that tag's fields. A field left over from an earlier state
  is an error in the hook, which keeps nothing of what it wrote, and the
  plugin's tests find it.
- **Effects are data**: a list of them, each a name or a table whose
  first value names it (`{ "check_token" }`, `{ { "save", batch } }`),
  which the hook carries out after `step`. This is the Elm shape the plugin
  model already has: the machine decides, the hook does the IO.

`machine:is(state, tag)` and `machine:tags()` read a state, and
`machine:shape(state)` checks one without stepping, for state folded from
records.

### Stale work carries its generation

A job or a hook that waits (on the model, on the network) carries the
generation it began under, and the machine drops what comes back to
another one. This is how the phone's session news and direnv's loads
solved the same problem in Rust:

```lua
local import = tau.machine({
	states = { idle = {}, running = { "gen", "reading" }, stopped = {} },
	initial = { state = "idle" },
	on = {
		start = { idle = function(_, e) return { state = "running", gen = e.gen, reading = 1 } end },
		stop = { running = function() return { state = "stopped" }, { "drop_todo" } end },
		tick_done = {
			running = function(s, e)
				if e.gen ~= s.gen then
					return s
				end
				return { state = "running", gen = s.gen, reading = s.reading + 1 }, { { "save", e.batch } }
			end,
		},
	},
})
```

A tick that finishes after Stop finds no `stopped` arm, and saves
nothing.

### One copy

What a machine decides lives in one place, the store or `ctx.state`, and
a view reads it from there. A copy kept for a hook (tau-schedule's
`waiting`) is read again from the store, not kept.

### Where it lives

`tau.machine` is in `tau.luau`, the module every hook loads, in plain
Luau with no host function. A package needs nothing new in `tau.toml`.

### Testing

Two helpers in `tau.test`, both on `t.machine` (ADR 0049) and so on
hegel's engine: one tests a machine alone, the other a plugin through its
hooks.

#### `t.machine_of(name, machine, spec)`: a machine alone

The rules come from the machine. There is one rule per event in
`machine.on`, and it fires in every state, including those with no arm
for it: that is where inputs out of order come from. An event's data is
drawn from `spec.events[name]`, a generator; an event with none is drawn
with no data.

After every step it checks, for every machine and with nothing written:

- the step raised no error;
- the state is in shape: a known tag, exactly its fields;
- an event with no arm left the state as it was, and gave no effects;
- the effects are well formed: lists whose first value names them, and
  only names in `machine.effects` when the machine declares them;
- stepping again from the same state with the same event gives the same
  answer (transitions are pure);
- the state reads back the same from JSON (it persists and replicates).

A plugin adds what is its own: `spec.model`, which starts a model and is
told each event, as the Rust models are, and `spec.invariant(state,
model, effects)`, the rule that matters: "a saved token is always
checked", "after Stop, nothing is saved".

A failure notes its shrunk events as a `t.case` that steps the machine
through them, with what failed as a comment, ready to paste into the
tests and give its own check: what the model finds becomes a plain test,
as for the Rust models.

#### `t.hooks(name, plugin, spec)`: a plugin through its hooks

The bugs the sweep found fall mostly between hooks: a job's tick and an
act, a run's end and an undo. This helper's rules are the plugin's own
ways in, on the fake run `t.run` already gives: a run's hooks (`start`,
`after_tool_result`, `before_stop`, `run_end` and the rest), its actions
and acts, and its jobs' ticks, with the inputs `spec` draws for each.

A hook that waits (on Jev, the model, a tool, the network) is split in
two: one rule starts it and runs it to its wait, another lets one waiting
hook go on with an answer `spec` draws, or a failure. So two of them can
be under way at once, in any order, as on the host. What they share
follows the host's rules, or the test would find races the host cannot
have and miss those it can:

- **A store is taken in turns.** A hook holds its plugin's store from its
  first use to its end, and another hook that uses it waits until then.
  What a hook writes goes in when it ends well, and not at all when it
  fails.
- **Files are shared.** What `env.files` writes, the next read sees, from
  any hook, at once.
- **A run's state is the hook's to change.** A hook gets the run's
  `ctx.state` as it is, and the run keeps what the hook leaves only when
  it ends well.

`spec.invariant(world)` holds after every step: the stores, the files,
each run's state, what was published and what the host was asked to do.
A failure notes its steps, which waits each way in reached and how each
was answered; the inputs are drawn inside the test's own functions, so
the case to keep is written from those notes.

## As built

- `tau.machine` is in `tau.luau`: `step`, `shape`, `is`, `handles`,
  `tags` and `events`. A field marked `name?` may be absent. Effects are
  checked against `effects` when the machine declares them.
- `t.machine_of` and `t.hooks` are in `tau_test.luau`, on `t.machine`.
  `t.hooks` splits a wait with a coroutine: the functions of `spec.run`'s
  `jev`, `tools`, `http` and `infer` yield when a hook calls them, and the
  rules `answer` (the function called with `draw` after its arguments)
  and `fail` let one go on. A hook that raises fails the test unless
  `spec.may_fail(hook, why)`. Each way in gets the world as a third
  argument, to note what it did.
- The fake run follows the host for what ways in under way share, which
  it needed before they could be under way at once. A call no longer
  swaps the run's data or state while it runs: each says which state it
  uses (the run's, or a job's) and what `ctx.run` it sees. A hook gets
  its own copy of the state, kept when it ends well and changed something,
  over an action that came meanwhile, as `catch_up` and the published
  state do on the host; an action's state is kept only while the state is
  the one it began on (`still_on`), and `run.dropped` counts the rest. A
  hook that fails now keeps none of what it wrote to its state, as on the
  host; before, its writes stood.
- `tests/machines.rs` holds them: a stray field, the missing `failed`
  edge of tau-github's sign-in (three events, its case noted), an impure
  transition, store hooks taking turns (they lose a write without turns),
  a failed wait, and a Pause pressed while a check waits on Jev, which the
  check's state then overwrites. That last is what the host does today:
  tau-goal's Pause, pressed while its check asks Jev, is lost when the
  check ends.

## Alternatives considered

- **The idiom alone, no helper.** A tag and a table of functions in each
  plugin. It is what the clean plugins do now, and each wrote its own
  guards; the bugs are where code went around them. A shared helper
  gives every plugin the shape check and the no-arm rule.
- **Machines with closures or metatables.** Luau can, but state must be
  JSON to persist, fold and replicate (ADR 0045), and a machine object in
  `ctx.state` would be none of those.
- **Luau types alone** (`--!strict`, a union of tagged table types). No
  plugin uses the type checker, and the host does not run it. Types may
  come later as an addition: the runtime shape check holds without them.
- **A machine on the Rust side,** declared from Luau and run by the host.
  It would cross the VM boundary on every event for what a few lines of
  Luau do, and a plugin could not read its own transitions.

## Consequences

- The plugins with the bugs above move their lifecycles onto
  `tau.machine`, one commit each: tau-github's sign-in, tau-memory's
  import, tau-constitution's verdicts, tau-todo's claims, tau-schedule's
  wake-ups, tau-reasoning's status, tau-memory's `given`, tau-goal's goal.
  Each bug gets a failing model first and a plain test for what the model
  finds.
- A hook that steps a machine returns effects to carry out; plugins that
  wrote state from several hooks write it from one place.
- A state folded from records is checked as it is folded.
- `docs/reference/plugins.md` gains the pattern, and `tau-plugin new`
  writes a machine in its example.

## Open questions

- **Start states and arm coverage.** Drawing any state in shape, not only
  those reachable from `initial` (what records folded from an older
  version hand back), and reporting arms no case took. Left for later.
- **Effects across hooks.** Whether effects stay a plugin's own names,
  carried out by its hook, or some become host effects (`tau.start`,
  `tau.continue`, a store write) that `step`'s caller hands back as is.
