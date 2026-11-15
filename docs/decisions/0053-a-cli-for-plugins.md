# 0053: heiko-plugin, a command line for Luau plugins

- Status: accepted.
- Date: 2026-11-10

## Context

A Luau plugin is a package (ADR 0051): `heiko.toml`, `src/`, `tests/`, and
for heiko's own `fixtures/`. What a person or an agent can do with one from
a shell today:

- `heiko --plugin-test <folder>` loads a package and runs its Luau cases
  and properties.
- `heiko --check-plugins` reads and loads heiko's own, as starting does.

Both are flags of the app's binary, which links gpui: testing a plugin
means building the interface first, and the flags sit beside `--demo` and
`--phone` in a parser that is not a command line.

Everything else needs Rust. Each built-in's fixtures, the JSON the demo
and the screenshots draw, are built by code of its own in
`heiko-built-in-tests` (`canvas/luau.rs` and nine more), written anew with
`BLESS_FIXTURES=1 cargo nextest ...`. What a plugin does in a real loop
(compaction, tree-compaction, fast-compaction, canvas) is Rust too: an
`Agent` with `ScriptedModel`, an in-memory store, sometimes a `FakeDecider` or
a fake tool. About 5,200 lines, for 13 plugins. A person's plugin can have
none of it, and nothing writes a package or puts one in the plugins
repository (ADR 0027) but a chat there.

## Decision

### A binary of its own

`heiko-plugin` is a crate, `crates/heiko-plugin`, and a binary of that name:
headless, on `heiko-luau-plugins-host`, with no gpui and no window. The Nix
package installs it beside `heiko`, so it finds the plugins heiko comes with
as `heiko` does (`../share/heiko/plugins`, or `HEIKO_PLUGINS_DIR`).

The work is in `heiko_luau_plugins_host::testing` and a new
`heiko_luau_plugins_host::packages` module; the binary parses and prints.
So the in-app `plugin_test` tool, `heiko-built-in-tests` and the CLI run
the same code, and a plugin passing in one passes in all three.

| Command                              | What it does                                                                                   |
| ------------------------------------ | ---------------------------------------------------------------------------------------------- |
| `heiko-plugin new <name> [--in <dir>]` | Writes a package that loads and passes its tests                                               |
| `heiko-plugin new --built-in <name>`   | The same under `plugins/`, with its line in `plugins/built-in.toml`                            |
| `heiko-plugin check [<folder>...]`     | Loads each and says what it declares and what it reaches                                       |
| `heiko-plugin test [<folder>...]`      | Cases, properties, fixtures and loops; `--bless`, `--watch`, `--filter`, `--verbose`, `--jobs` |
| `heiko-plugin install <folder>`        | Tests it, then commits it to the person's plugins repository                                   |

With no folder, `check` and `test` take the folder they are run in when it
holds a `heiko.toml`, else every plugin heiko comes with. Each exits 0 when
all is well, 1 when a test fails or a package does not load, 2 when it
was asked wrongly. `--json` prints what it found as one JSON value, for
an agent or a script. _(`--json`, `--filter` and the exit codes are
choices of this ADR's, not asked.)_

`heiko --plugin-test` and `heiko --check-plugins` go, with `crates/heiko/tests/
flags.rs`'s cases of them; the READMEs, `docs/reference/plugins.md`, ADR
0051's table and the flake's install check say `heiko-plugin` instead.

### new

`new <name>` writes `<dir>/<name>/`: `heiko.toml` (name, version `0.1.0`,
a description to fill in, `entry = "src/plugin.luau"`), a
`src/plugin.luau` with one tool, a status in its view and a button
action, a `tests/plugin.luau` with a case and a property over them, and a
README. It refuses a folder that exists. The test after `new` passes:
`new` runs it before it says it is done.

`--built-in` writes the same under the repository's `plugins/` and adds
the name to `plugins/built-in.toml`, last, since order is only what the
compactions rewrite in. It needs nothing in `heiko-built-in-tests`: that
crate runs every built-in's tests through one test (below). The name must
be lowercase words joined by `-` and, for a built-in, begin `heiko-`.

### check

`check` loads each package as starting does and prints, per plugin, its
version and digest, its hooks, tools, jobs, commands and views, and what
it reaches in the words the Allow uses: `Uses::grown_from(&nothing)`,
"reading the run's workspace", "calling `read`". A package that does not
load says why, with the file and line.

### test

`test` runs four kinds of thing, in this order, and prints a line per
package, one per failure, and with `--verbose` one per thing:

1. **Cases and properties**, as now.
2. **Fixtures.** A test file declares one with `t.fixture(name, fn)`:

   ```luau
   t.fixture("canvas-document", function()
     local r = t.run({ files = FILES })
     r:tool("canvas_write", { title = "Plan", text = PLAN }, "c1")
     return { state = r.state, view = r:view() }
   end)
   ```

   The value `fn` returns is held to `fixtures/<name>.json`, compared as
   JSON; a difference names the first path that differs and both values
   there. `fixtures/declaration.json`, which every built-in has, is the
   package's declaration as heiko reads it, with no `t.fixture` for it. A
   file in `fixtures/` that nothing declares fails, so a fixture cannot
   outlive the test that made it. `--bless` writes each fixture anew,
   formatted as the repository formats JSON, and removes the undeclared
   ones; it leaves the cases' and properties' results as they are. A
   value comes back from Luau as JSON, so a blessed file's keys are in
   order.

3. **Loops.** `t.loop(name, spec, check)` runs the package's plugin in
   a real `Agent` loop:

   ```luau
   t.loop("an image is kept and read back", {
     files = { ["plot.png"] = t.png(12, 8) },
     model = {
       { tool = "canvas_show", args = { path = "plot.png", title = "Plot" } },
       { text = "Shown." },
     },
   }, function(run)
     t.equal(run.results[1].text, 'Image 1, "Plot", is on the canvas (12×8 PNG).')
   end)
   ```

   `spec` gives what the Rust tests gave, as data (the `looping`
   module's own documentation is the full list):

   - `prompt`, the task, and `steer`, a message steered in at the
     start; `model`, the turns, each `{ text }`, `{ thinking }`,
     `{ tool, args }`, `{ usage }`, `{ error }`, `{ stop }` or a list of
     them; `name`, the run's model. A compaction's window is one of its
     `settings`, so there is no `window` of the loop's own.
   - `files`: the run's workspace, path to text, `t.bytes(base64)` or
     `t.png(w, h)`.
   - `settings`, over the plugin's defaults, and the run's `kind` and
     `repo`.
   - `decide`: a fake decider, `{ error = 503 }` or rules answering each
     question by the first that matches its id's prefix or a text in
     what the request holds of it. The host cannot call back into the
     test's Luau while the loop runs, so the answers are data.
   - `tools`: fake tools beside the plugin's own, each name to what it
     answers, as text or a list taken in order, `{name}` in it being the
     call's argument `name` and `{x*name}` `x` that many times.
   - `then`: forks, each from where the run before it ended, with its
     own `prompt`, `steer`, `settings` and `decide`.

   `run` holds what the Rust tests read: `results`, each call's text and
   whether it failed; `outcome` (`text`, `stop`, `usage`); `events`;
   `state`, the plugin's last; from the store, the context's `rewrites`
   and the plugin's `logs`; `charged`; `archive`, each file the plugin
   archived with its permission bits; and on the first run `forks`,
   `requests` (what the model was sent each turn), `decide` (what the decider was
   asked), `workspace` (each file as written) and `blobs` (each blob the
   state names, read back). A file's VM registers its loops; the host
   runs each one itself, with a limit of 60 s, then runs the file again
   in a fresh VM to call `check` with what the run came to, as it runs a
   property: a loop never holds a file's 10 s. The host loads the same
   package again, and builds the `Agent` with a scripted model,
   `heiko_store_sqlite::memory()`, a folder of its own for the workspace,
   the blobs and the archive, and retries a millisecond apart.
   `ScriptedModel` moves out of `heiko-testing` into a crate of its own,
   `heiko-scripted`, on `heiko-ai` alone, which `heiko-testing` re-exports; it
   and `heiko-store-sqlite` become normal dependencies of
   `heiko-luau-plugins-host`, behind no feature, since a person's tests use
   them in the app too. `heiko-testing` stays a dev-dependency. `FakeDecider`
   is already `heiko_decide::fake`.

4. Nothing else: a test file that declares nothing fails, as now.

`--filter <text>` runs what has `text` in its file or name. `--watch`
runs once, then again each time the package's digest (`Files::digest`)
changes, checked twice a second; it needs no file-watching crate, and
stops at Ctrl-C. _(Polling the digest is this ADR's choice.)_

### install

`install <folder>` runs `test` on the package and stops if anything
fails. Then it copies the package into the plugins repository
(`$XDG_DATA_HOME/heiko/luau-plugins`, ADR 0027), replacing a folder of the
same name, and commits it on the trunk through `heiko-vcs-host`, as a chat
landing there would, with the description `install <name> <version>`.
The plugin is then a new version like any other: it is active once the
person allows what it reaches on the Plugins page. A package named like
one of heiko's own is refused. _(Testing before installing, and refusing
built-in names, are this ADR's choices.)_

### heiko's own tests move to Luau

Each built-in's Rust fixture builders become `t.fixture`s and its
`in_the_loop` tests `t.loop`s, in its `tests/`. A moved fixture is right
when `test --bless` leaves `fixtures/` as it was; a moved loop when it
fails where its Rust one failed (one assertion inverted). Then the Rust
goes. `heiko-built-in-tests` keeps:

- one test that runs `testing::test` over every built-in and fails with
  their failures, which replaces each plugin's `the_plugin_loads_and_
passes_its_tests`;
- what Luau cannot say: heiko-todo's `crash_model`, `store_model`,
  `beads_oracle` and `schemas` (the SQLite store under a fuse, an
  external oracle, providers' schema rules), heiko-reasoning's `replayed`
  (stored runs from `heiko-store`), and `binaries` and `package`;
- heiko-ui's pane and card tests stay in heiko-ui.

## Alternatives considered

- **Subcommands of `heiko`.** One binary to install, but testing a plugin
  would still build and link the interface.
- **A cargo xtask.** Nothing would ship to a person writing a plugin.
- **JSON replay specs for fixtures, scenario files for loops.** Data a
  CLI reads without Luau, but no assertions, and a second way of saying
  what `t.run` already says.
- **A file-watching crate for `--watch`.** Events instead of polling;
  one more dependency for a loop that reads a few dozen small files.

## Consequences

- A plugin is written, tested, its fixtures blessed and installed with
  no Rust and no interface build; CI and the Nix install check run
  `heiko-plugin test` over heiko's own.
- About 4,000 of `heiko-built-in-tests`' 5,200 lines move to Luau, so a
  change to a built-in's tests compiles nothing.
- The scripted model ships in heiko's binaries, in `heiko-scripted`, without
  `heiko-testing`'s turmoil, hegeltest and ring.
- The in-app `plugin_test` tool runs fixtures and loops too, so an agent
  in the plugins repository can test a plugin in a real loop before it
  lands.
- A later model for plugins (an Elm-style `update`, considered after
  heiko-github) changes what `t.run` and `t.loop` drive, not this command.

## As built

- **The commands.** `check`, `test` (`--bless`, `--watch`, `--filter`,
  `--verbose`, `--json`), `new` (`--in`, `--built-in`) and `install`,
  in `crates/heiko-plugin`; the work in heiko-luau-plugins-host's
  `packages`, `scaffold` and `looping` modules. `new` writes a tool
  `<name>_greet`, a status and a button, and a case and a property over
  them, and runs them before it says it is done. `--watch` waits out a
  package that cannot be read for a moment, as one half saved.
- **Install.** The plugins repository is `luau-plugins` in heiko's data
  directory, found as the app finds it, and made with the files the app
  makes it with when it is not there. A package goes on trunk through a
  new `ProjectRepo::commit_folder_on_trunk`: the folder's files replace
  what trunk held there, in a commit on top of trunk, and trunk moves to
  it; the same files again commit nothing. The registry reads it at its
  next look at trunk, as a version like any other.
- **Fixtures moved.** Nine plugins' builders became `tests/fixtures.luau`;
  blessing them all left every fixture's value as it was (compared as
  JSON, file by file), and only the key order changed. heiko-watcher gained
  the `declaration.json` it lacked. `t.run` gained `repo`.
- **Loops moved.** canvas's 2, compaction's 13, tree-compaction's 9 and
  fast-compaction's 13 Rust loop tests are `t.loop`s, with every
  assertion; each loop fails when one of its assertions is changed. That
  took more of the spec than first written: `then` steps (a fork, the
  same run resumed, or idle compaction), `steer` as a list, `reasoning`,
  `clock`, `retry`, `plugins` (others of heiko's own beside it), model
  parts `cost`, `delay` and `failure`, and in what a run came to its
  `record`, `transcript`, `outcome.request` and `most_at_once`, the most
  model responses going on at once, which a plugin's limit on its own
  requests shows.
- **What stays Rust.** `heiko-built-in-tests` went from about 7,000 lines
  to 2,600: `built_ins.rs` runs `heiko-plugin test`'s checks over every
  built-in (now `packages.rs`, below); heiko-todo's store under a fuse, its model, the `br` oracle and
  the providers' schema rules; heiko-reasoning's stored runs; heiko-schedule
  waking a chat through the real host; heiko-constitution against the real
  decider; and the one-binary and package checks.
- **A fix it found.** Changing a README changed heiko-tree-compaction's
  digest, so one property drew new cases, and with mlua 0.12.1 its
  script slept until its limit about one run in two: the interrupt's
  yield lost the wake-up of a call that waited. mlua 0.12.2 fixed it,
  landed on its own before this.

### Tests at once, 2026-11-14

`heiko-plugin test` over every package took about 51 s on one core of 32,
95% of it hegel's properties and machines, each package, file, property
and loop one after another; in nextest, `every_built_in_passes_heiko_plugin_test`
ran all of it as one test, about 100 s under the suite's load.

- **A pool of jobs** (`heiko_luau_plugins_host::pool`): threads of their
  own, each with its own runtime, taking the costliest job waiting
  first. A job runs to its end on one thread, as hegel's error channel
  is the thread's. Each file's cases go first, as what they register is
  what else runs; then every property and loop, by what it took the last
  time, or what it asks for. Results come in the order the files declare
  them, and seeds are each property's own, so what they come to does not
  depend on the order they ran in.
- **Every package at once** in `heiko-plugin test`, its tree drawing each
  package going on with its tests going on. `--jobs <n>` runs n at once,
  else one a core; what each test took is kept in heiko's cache folder.
  Inside heiko, as a plugin is installed, the pool takes half the cores.
- **A nextest test a package**: `heiko-built-in-tests`' `packages` binary
  speaks enough of libtest's command line to list one test a built-in,
  so nextest runs them at once and `test(heiko-memory)` takes one; each
  takes a quarter of the cores for its own tests.
- Every package went from about 92 s with `--jobs 1` to about 30 s, and
  the built-ins in nextest from about 100 s to about 31 s: each bound by
  heiko-ask's view machine, which on this version runs to the property's
  30 s limit by itself.
