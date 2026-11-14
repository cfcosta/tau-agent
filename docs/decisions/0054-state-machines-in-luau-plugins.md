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
- **Effects are data** (`{ "check_token" }`, `{ "save", batch }`), which
  the hook carries out after `step`. This is the Elm shape the plugin
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
				return { state = "running", gen = s.gen, reading = s.reading + 1 }, { "save", e.batch }
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

To decide: how hegel's state machines reach a `tau.machine`. `t.machine`
(ADR 0049) already runs a model with rules and invariants on hegel's
engine. A machine declares its events and states, so a test could be
derived from it; what that test draws, what it checks for free, and what
each plugin adds is the open question below.

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

- **Testing a `tau.machine` with hegel.** What `t.machine_of(machine,
spec)` draws (events from the machine's own table, in any order, from
  any state, with generators for their data), what it checks for every
  machine (no step errors, every state in shape, effects well formed),
  and how a plugin adds its own invariants and its own model.
- **Effects across hooks.** Whether effects stay a plugin's own names,
  carried out by its hook, or some become host effects (`tau.start`,
  `tau.continue`, a store write) that `step`'s caller hands back as is.
