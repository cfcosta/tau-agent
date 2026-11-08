# 0049: Property tests in Luau, on hegel's engine

- Status: accepted.
- Date: 2026-11-07

## Context

A Luau plugin's tests are `tests/*.luau` files of example cases
(`t.case(name, fn)`, ADR 0027). tau runs them whenever it tries a
version of the plugin, and a version whose tests fail does not
activate. Each file runs in one codemode VM, with 10 s for the whole
file (`runtime.rs`, `run_tests`).

Examples check what their author thought of. tau's Rust code checks
the rest with hegel: generated inputs, shrunk to a minimal failure,
and state machines for anything with a lifecycle. tau-todo shows the
gap. Its Rust tests hold its Luau to a reference model with hegel
(`graph_laws.rs`, `store_model.rs`), but its own `tests/*.luau` are 20
examples. A plugin someone writes in tau has the examples and nothing
else.

hegel is not only a Rust library. Its engine, libhegel (the
`hegeltest-c` crate), is a C ABI (`hegel.h`) that every language's
frontend drives. The Rust crate tau uses is one frontend, and hegel-go
is another. The ABI is pull-style: the caller asks for the next test
case, draws from it, and says how it ended.

```
hegel_run_start
loop:
  hegel_next_test_case        -> a case, or none when the run is over
  hegel_generate_* / spans    -> the case's draws
  hegel_mark_complete(VALID | INVALID | OVERRUN | INTERESTING)
hegel_run_result              -> passed, or the failures, each with a
                                 reproduce blob
```

The engine never calls into the test. A caller can run anything
between two calls, including an async Luau VM. The ABI also has spans,
collections and recursion (so composite values shrink well), pools and
a state machine (`hegel_state_machine_next_rule`, invariants), replays
of a blob (`hegel_run_start_blob`), and an output callback.

## Decision

### Luau is one more hegel frontend

tau-luau-plugins-host links libhegel and drives it. `tau.luau` holds
the frontend's Luau half: generators, `t.property` and `t.machine`.
Plugin authors write properties in Luau, and tau runs them with the
engine its own tests use.

### The Luau API

```lua
local t = require("tau").test
local g = t.gen

local issue = g.records {
  id = g.from_regex("td-[0-9a-f]{4}"),
  priority = g.integers { min = 0, max = 4 },
  status = g.sampled_from { "open", "in_progress", "blocked", "closed" },
}

t.property("ready holds no blocked issue", function(draw)
  local issues = draw(g.lists(issue, { max = 8 }))
  local now = draw(g.integers { min = 0, max = 10 })
  t.assume(#issues > 0)
  for _, id in ipairs(graph.ready(issues, now)) do
    t.equal(graph.is_blocked(issues, id, now), false, id)
  end
end)
```

- **Generators** are values: `integers`, `floats`, `booleans`, `text`,
  `from_regex`, `bytes`, `lists` (`min`, `max`, `unique`), `dicts`,
  `records`, `sampled_from`, `one_of`, `just`, `optional`, and
  `composite(function(draw) … end)` for one built from others.
  `gen:map(f)` and `gen:filter(f)` derive new ones. Each generator
  draws inside a span, so the engine shrinks a list as a list and a
  record field by field.
- **`draw(gen)`** gives a value. `t.assume(cond)` drops the case
  (`INVALID`). `t.note(text)` adds a line to the failure's report.
- **`t.property(name, fn, options)`** registers a property next to the
  file's cases. `options` may set `cases` (no more than the host's
  limit) and `reproduce` (a blob, to replay one failure).
- **`t.machine(name, spec)`** is a state machine, as `#[hegel::state_machine]`
  is in Rust: `init(draw)` gives the model, `rules` are functions of
  the model and `draw` with weights, `invariant(model)` runs after
  each step, and `steps` is the most a case takes. A failure reports
  the shrunk steps.
- `t.run{}` and `t.agent{}` work inside a property as they do in a
  case: a property can drive a fake run's hooks and tools.

### The Rust half

- **Linking.** tau-luau-plugins-host depends on `hegeltest-c` as an
  rlib, pinned to the engine version tau's `hegeltest` uses (0.44.1
  for 0.48.1). The engine has no C build of its own: it is Rust with
  small dependencies (dashu-int, hashbrown, libm, miniz_oxide, rand,
  toml). The rlib exports its C functions as Rust items, so tau calls
  them as `hegel_c::hegel_*` with no declarations of its own (hegeltest
  does the same with its `static-engine` feature).
  `tau_luau_plugins_host::hegel` wraps them: each handle is owned by one
  value and freed when it drops, and every `unsafe` of the plugins'
  host is there.
- **The run loop.** Running a file first registers its cases and
  properties. Each case runs as it does today. For each property, tau
  starts a run and one VM on the file, which runs every case the
  engine offers, one after another (`tau._run_cases`): it asks for the
  next with `hegel { op = "next" }`, runs the property, and says how it
  ended with `hegel { op = "end" }`. A draw is a call of the same tool,
  `hegel`, which the host gives only that VM (`Host::call_tool`; a
  plugin's `uses` cannot name it). Asking for the next case waits on
  the engine off the runtime's threads, with no lock held meanwhile.
- **How a case ends:**

  | In Luau                                       | Marked        |
  | --------------------------------------------- | ------------- |
  | the property returns                          | `VALID`       |
  | `t.assume(false)`                             | `INVALID`     |
  | a draw past the engine's budget (`STOP_TEST`) | `OVERRUN`     |
  | an error, a failed `t.equal`, or a time out   | `INTERESTING` |

  An `INTERESTING` case's origin is its error's first line, without
  values, so distinct failures stay apart.

- **Settings, fixed by the host.** The plugin cannot change them:
  - `derandomize`, with a seed from the version's digest and the
    property's name. A version gives the same result each time tau
    tries it, as the cache of tried versions (by digest) assumes.
  - no database: a fixed seed finds the same failure again, and there
    is nothing a plugin should write outside its own store;
  - 100 cases a property, and the `too_slow` health check off, as on
    CI;
  - 30 s for a property's cases, shrinking included. At 30 s the
    engine is asked for no more: a property that found no failure by
    then passes, one that did fails with its failure as far as it
    shrank, saying so. The VM stops at 40 s whatever it is doing, so
    a case that never ends fails the property.
- **Reports.** A failed property is one failed `TestResult`. Its error
  is the failure's message, the shrunk draws and notes (from the
  engine's final replay, the case it marks for capture), and the
  reproduce blob. The output callback takes the engine's own lines
  into the same report, never to stderr.

### Where properties run

- **When tau tries a version**, with the fixed settings above. A
  failing property keeps the version from activating, as a failing
  case does.
- **Under `cargo nextest`**, through `Loaded::test`, as tau-todo's
  `luau.rs` runs its cases today. The same settings apply, so a
  property that passes there passes in the app.
- **Phones** do not run plugin tests, and need no engine.

## Testing

- **The frontend's own properties, in Rust.** Luau properties with
  known minimal failures shrink to them: an integer at least 5 shrinks
  to 5, a list whose sum passes 10 shrinks to one element, a record
  shrinks field by field. A property that passes passes. `assume`
  drops cases and a property that always assumes false fails. A case
  that draws without end is an overrun, not a failure, and a property
  whose every case does fails a health check.
- **Determinism.** The same files give the same results and the same
  report, run twice.
- **A state machine** with a planted bug (a counter that skips at 3)
  fails with the shortest sequence of steps.
- **Reproduce.** A blob from a report, given as `reproduce`, replays
  exactly that failure.
- **tau-todo** gains Luau properties for what its Rust laws check
  today (ready and blocked, claims, steps), and keeps the Rust ones as
  the oracle (0048, "6. Oracles").
- **The skill** gets a section and an example plugin with properties.
  `tests/shipped.rs` checks it as it checks the rest.

## As built

- **One VM per property, not per case.** A fresh VM per case cost
  about 6 ms in tau-todo (the VM, tau's library and the plugin's
  modules, compiled each time). Shrinking an eight-issue graph to its
  two-issue failure took 12 600 cases, 83 s. With one VM it takes 8 s,
  and tau-todo's Luau tests went from 5.6 s to 0.7 s. The price: a
  property's cases share the file's top-level values, as Hypothesis's
  do in Python. The skill says to keep what a case changes inside it.
  So there is no per-case limit, only the property's.
- **No lock across an await.** The first driver held a tokio mutex
  while it waited for the engine's next case, and a VM stopped for
  good mid-case under it. The driver now takes the run out, waits with
  no lock held, and puts it back; every other call takes a short
  `std` lock.
- **Machines are sequential**: one group, one worker, the rules'
  weights given to the engine. The model's invariant runs after every
  step, in Luau, rather than when the engine samples it.
- **No version check at run time.** The engine is linked, pinned to
  the exact version (`hegeltest-c = "=0.44.1"`), so the build is the
  check.
- **tau-todo.** The issue graph's laws moved to
  `luau/tests/graph.luau`, with their reference model, and
  `tests/graph_laws.rs` is gone; `luau.rs` checks by name that they
  run. The rest stay in Rust, as Luau cannot reach what they check: the
  store model runs tools at once on the real SQLite store, the crash
  model crashes it mid-write, the beads oracle runs `br`, and the
  schema test checks Rust's strict schemas. A planted bug in
  `graph.is_blocked` (in progress counted as blocked) fails "ready and
  blocked are the model's", shrunk to one in-progress issue blocked by
  one open one.
- **A case asks the engine at most 50 000 times**, then it is an
  overrun. The engine does not end a case that only draws booleans,
  and a case that went on for about 330 000 draws ran mlua out of its
  reference slots (a panic, not a Lua error). Cases that end free them:
  1.2 million draws over 200 cases are fine.
- **Size.** The engine adds 3.0 MB to the `tau` binary: 184.4 MB
  before, 187.4 MB after (release, Linux x86_64).
- The skill's `short-titles` example has a property.

## Alternatives considered

- **A property engine written in Luau.** One choice sequence that
  every draw reads, shrunk by deleting and lowering choices: about 400
  lines, with no native code. Rejected: it would be a second engine to
  maintain, with weaker search and shrinking than hegel's, and plugin
  properties would behave differently from tau's own.
- **The Rust crate's `hegel::Hegel`.** Its test function is
  synchronous and reports failure by panicking. Driving async VMs from
  it means blocking inside the closure and catching panics to read
  results. The C ABI asks for neither.
- **Loading libhegel as a shared library**, as hegel-go does. That
  ships a second file and resolves it at run time on every platform.
  Linking the rlib keeps tau one binary.
- **Properties only under cargo.** Plugins written in tau have no
  cargo tests. Their tests are what gates activation.

## Consequences

- tau ships hegel's engine in its binary (see "As built" for its size).
- hegel is in beta and may break its ABI. tau pins the engine, and an
  upgrade changes `hegel.rs` and this frontend together, under the
  frontend's own tests.
- Trying a version takes longer: up to 30 s for each property. A
  plugin with many properties activates more slowly than one with
  examples only.
- Plugin authors get the same tool tau's code uses, and tau-todo's
  Luau gets checked from its own tests, not only from Rust.

## Open questions

- Should tau keep failures found when a version was tried, so the next
  version replays them first, as the example database does? That
  needs somewhere in the plugin's data, and a version's result would
  then depend on more than its digest.
- Should the person be able to raise the case count for one plugin,
  for a deeper run on demand from its page?
